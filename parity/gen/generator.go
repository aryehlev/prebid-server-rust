package gen

import (
	"errors"
	"fmt"
	"sort"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Corpus name and tags of generated cases.
const (
	// CorpusGenerated is Case.Corpus of every generated or derived case.
	CorpusGenerated = "gen"
	// TagGenerated marks every case of this package.
	TagGenerated = "generated"
	// TagValidIntent marks cases built only from validity-preserving steps on
	// a valid start: Go is expected to accept them.
	TagValidIntent = "gen_valid"
	// TagUncertain marks cases with structural steps whose effect on validity
	// is not known in advance.
	TagUncertain = "gen_uncertain"
	// TagInvalidIntent marks cases made invalid on purpose (well-formed JSON).
	TagInvalidIntent = "gen_invalid"
	// TagMalformed marks cases whose body or HTTP framing is malformed.
	TagMalformed = "gen_malformed"
)

// Base names: Chain[0] of a generated case.
const (
	BaseAuction = "auction"
	BaseAMP     = "amp"
	BaseVideo   = "video"
)

// Default Options values.
const (
	DefaultMaxSteps    = 4
	DefaultCorpusShare = 0.4
)

// Options tune how Case draws new cases. They never affect how an existing
// case ID regenerates.
type Options struct {
	// MaxSteps caps the mutation steps drawn per case (default 4).
	MaxSteps int
	// CorpusShare is the share of cases derived from corpus cases rather than
	// built from scratch (default 0.4). Set NoCorpus to draw none.
	CorpusShare float64
	// NoCorpus disables corpus-derived cases.
	NoCorpus bool
}

func (o Options) withDefaults() Options {
	if o.MaxSteps <= 0 {
		o.MaxSteps = DefaultMaxSteps
	}
	if o.CorpusShare <= 0 {
		o.CorpusShare = DefaultCorpusShare
	}
	if o.NoCorpus {
		o.CorpusShare = 0
	}
	return o
}

// Generator builds parity cases deterministically from case IDs. It is safe
// for concurrent use: generation only reads its state.
type Generator struct {
	corpus   *corpus.Corpus
	pools    *Pools
	catalog  *catalog
	bases    []*corpus.Case
	baseByID map[string]*corpus.Case
	seeds    map[string][]corpus.Seed
	opts     Options
}

// Load loads every corpus of repo and builds a generator over it.
func Load(repo *corpus.Repo, opts Options) (*Generator, error) {
	c, err := corpus.LoadAll(repo)
	if err != nil {
		return nil, fmt.Errorf("gen: loading corpus: %w", err)
	}
	return New(repo, c, opts)
}

// New builds a generator over an already loaded corpus of repo.
func New(repo *corpus.Repo, c *corpus.Corpus, opts Options) (*Generator, error) {
	if repo == nil || c == nil {
		return nil, errors.New("gen: New needs a repository and its corpus")
	}
	pools, err := LoadPools()
	if err != nil {
		return nil, err
	}
	cat, err := newCatalog(repo, c)
	if err != nil {
		return nil, err
	}
	g := &Generator{
		corpus:   c,
		pools:    pools,
		catalog:  cat,
		baseByID: map[string]*corpus.Case{},
		seeds:    map[string][]corpus.Seed{},
		opts:     opts.withDefaults(),
	}
	for i := range c.Cases {
		cs := &c.Cases[i]
		if derivable(cs) {
			g.bases = append(g.bases, cs)
			g.baseByID[cs.ID] = cs
		}
	}
	for _, s := range c.Seeds {
		if s.Request != nil {
			g.seeds[s.Domain] = append(g.seeds[s.Domain], s)
		}
	}
	return g, nil
}

// derivable reports whether a corpus case can start a derived case: an exec
// envelope on one of the auction endpoints that the shared contract accepts.
func derivable(cs *corpus.Case) bool {
	if cs.Envelope == nil || cs.HasTag(corpus.TagContractGap) {
		return false
	}
	switch cs.Endpoint {
	case corpus.EndpointAuction, corpus.EndpointAMP, corpus.EndpointVideo:
		return true
	}
	return false
}

// Bases returns the IDs of the corpus cases derived cases can start from.
func (g *Generator) Bases() []string {
	out := make([]string, len(g.bases))
	for i, cs := range g.bases {
		out[i] = cs.ID
	}
	return out
}

// Case draws the index-th case of the run seeded by seed. The returned case's
// ID regenerates it exactly (Regenerate).
func (g *Generator) Case(seed, index uint64) (corpus.Case, error) {
	plan := Derive(seed, "plan", u64(index))
	id := ID{Seed: seed, Index: index}
	minSteps := 0
	if len(g.bases) > 0 && plan.Chance(g.opts.CorpusShare) {
		id.Source = Pick(plan, g.bases).ID
		minSteps = 1
	} else {
		id.Chain = []Step{{Name: drawBase(plan)}}
	}
	d, err := g.start(id)
	if err != nil {
		return corpus.Case{}, err
	}
	steps := plan.Range(minSteps, g.opts.MaxSteps)
	if err := g.drawSteps(plan, d, steps, 0); err != nil {
		return corpus.Case{}, err
	}
	if err := d.ensureStep(); err != nil {
		return corpus.Case{}, err
	}
	return d.finish()
}

// fallbackStep applies to every draft; it keeps derived IDs non-empty.
const fallbackStep = "http.remote_addr"

// ensureStep gives a derived case at least one step, as its ID requires.
func (d *draft) ensureStep() error {
	if d.id.Generated() || len(d.id.Chain) > 0 {
		return nil
	}
	return d.apply(Step{Name: fallbackStep})
}

// drawBase draws a base, auction weighted up.
func drawBase(r *RNG) string {
	return []string{BaseAuction, BaseAMP, BaseVideo}[r.Weighted([]int{75, 13, 12})]
}

// Regenerate rebuilds the case with the given ID.
func (g *Generator) Regenerate(caseID string) (corpus.Case, error) {
	id, err := ParseID(caseID)
	if err != nil {
		return corpus.Case{}, err
	}
	return g.Build(id)
}

// Build replays a parsed case ID.
func (g *Generator) Build(id ID) (corpus.Case, error) {
	d, err := g.replay(id)
	if err != nil {
		return corpus.Case{}, err
	}
	return d.finish()
}

// Child derives a new case from parent by appending one or two steps drawn
// with salt. Feedback uses it to dig below cases that reached new behaviour.
// parentID is a generated or derived case ID, or the ID of a corpus case that
// can start derived cases.
func (g *Generator) Child(parentID string, salt uint64) (corpus.Case, error) {
	parent, err := ParseID(parentID)
	if err != nil {
		if _, ok := g.baseByID[parentID]; !ok {
			return corpus.Case{}, err
		}
		parent = ID{Source: parentID, Seed: salt}
	}
	d, err := g.replay(parent)
	if err != nil {
		return corpus.Case{}, err
	}
	plan := Derive(parent.Seed, "child", parentID, u64(salt))
	variant := uint32(plan.Range(1, 1<<20))
	if err := g.drawSteps(plan, d, plan.Range(1, 2), variant); err != nil {
		return corpus.Case{}, err
	}
	if len(d.id.Chain) == len(parent.Chain) {
		return corpus.Case{}, fmt.Errorf("gen: no step applies below %s", parentID)
	}
	return d.finish()
}

// replay rebuilds the draft of an ID without finishing it.
func (g *Generator) replay(id ID) (*draft, error) {
	steps := id.Chain
	start := id
	if id.Generated() {
		if len(steps) == 0 {
			return nil, fmt.Errorf("%w: generated case without a base", ErrInvalidID)
		}
		start.Chain = steps[:1]
		steps = steps[1:]
	} else {
		start.Chain = nil
	}
	d, err := g.start(start)
	if err != nil {
		return nil, err
	}
	for _, step := range steps {
		if err := d.apply(step); err != nil {
			return nil, err
		}
	}
	return d, nil
}

// drawSteps draws and applies up to n steps that apply to d's current state.
func (g *Generator) drawSteps(plan *RNG, d *draft, n int, variant uint32) error {
	used := map[string]bool{}
	for _, s := range d.id.Chain {
		used[s.Name] = true
	}
	for i := 0; i < n; i++ {
		m := drawMutator(plan, d, used)
		if m == nil {
			return nil
		}
		used[m.name] = true
		if err := d.apply(Step{Name: m.name, Variant: variant}); err != nil {
			return err
		}
	}
	return nil
}

// drawMutator picks a weighted mutator that applies to d now.
func drawMutator(plan *RNG, d *draft, used map[string]bool) *mutator {
	var cands []*mutator
	var weights []int
	for _, m := range registry() {
		if used[m.name] || m.phase < d.phase || !m.appliesTo(d) {
			continue
		}
		cands = append(cands, m)
		weights = append(weights, m.weight)
	}
	i := plan.Weighted(weights)
	if i < 0 {
		return nil
	}
	return cands[i]
}

// Mutators describes every registered step, sorted by name.
func Mutators() []MutatorInfo {
	reg := registry()
	out := make([]MutatorInfo, 0, len(reg))
	for _, m := range reg {
		out = append(out, m.info())
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// sourceCase returns the corpus case a derived ID starts from.
func (g *Generator) sourceCase(id string) (*corpus.Case, error) {
	cs, ok := g.baseByID[id]
	if !ok {
		return nil, fmt.Errorf("gen: corpus case %q is not a derivation source", id)
	}
	return cs, nil
}

// envelopeDecisions returns the decision IDs in registry order as strings.
func envelopeDecisions() []string {
	all := parity.AllDecisions()
	out := make([]string, len(all))
	for i, d := range all {
		out[i] = string(d)
	}
	return out
}
