package gen

import (
	"bytes"
	"encoding/json"
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"sync"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/parity/stagedata"
)

// Coverage feedback. A trace is reduced to signatures: the stages it reached,
// the HTTP status, each error and warning (code plus a message template), seat
// non-bid codes, decision values, outbound call outcomes and analytics kinds.
// A case whose trace adds a signature nobody produced before is kept as a
// parent. Its energy grows with novelty and with depth, the highest stage
// order (schema/stages.json) it reached, so cases that get past validation
// into the exchange are mutated further than cases rejected at the door.

// Feedback tuning.
const (
	depthWeight   = 4.0
	noveltyWeight = 1.0
	energyDecay   = 0.8
	energyFloor   = 0.05
	maxTemplate   = 120
)

// Observation is what one trace contributed.
type Observation struct {
	CaseID       string         `json:"case_id"`
	Depth        int            `json:"depth"`
	DeepestStage parity.StageID `json:"deepest_stage,omitempty"`
	Signatures   []string       `json:"signatures"`
	New          []string       `json:"new,omitempty"`
	Kept         bool           `json:"kept"`
	Energy       float64        `json:"energy,omitempty"`
}

// parent is a kept case.
type parent struct {
	caseID string
	energy float64
	picks  int
}

// Feedback accumulates signatures across traces. It is safe for concurrent
// use.
type Feedback struct {
	mu       sync.Mutex
	order    map[parity.StageID]int
	maxOrder int
	seen     map[string]int
	parents  []*parent
	byID     map[string]*parent
}

// NewFeedback builds feedback over the stage registry's orders.
func NewFeedback(stages schema.StageRegistry) *Feedback {
	f := &Feedback{order: map[parity.StageID]int{}, seen: map[string]int{}, byID: map[string]*parent{}}
	for _, s := range stages.Stages {
		f.order[s.ID] = s.Order
		if s.Order > f.maxOrder {
			f.maxOrder = s.Order
		}
	}
	return f
}

// LoadFeedback builds feedback over the embedded stage registry.
func LoadFeedback() (*Feedback, error) {
	stages, err := schema.Stages()
	if err != nil {
		return nil, fmt.Errorf("gen: loading stage registry: %w", err)
	}
	return NewFeedback(stages), nil
}

// Observe records the trace of caseID (normally the Go trace, the reference
// implementation) and reports what it added.
func (f *Feedback) Observe(caseID string, tr *parity.Trace) Observation {
	sigs := TraceSignatures(tr)
	depth, deepest := f.depth(tr)
	f.mu.Lock()
	defer f.mu.Unlock()
	obs := Observation{CaseID: caseID, Depth: depth, DeepestStage: deepest, Signatures: sigs}
	for _, s := range sigs {
		if f.seen[s] == 0 {
			obs.New = append(obs.New, s)
		}
		f.seen[s]++
	}
	if len(obs.New) == 0 {
		return obs
	}
	obs.Kept = true
	obs.Energy = f.energy(depth, len(obs.New))
	if p, ok := f.byID[caseID]; ok {
		p.energy += obs.Energy
		return obs
	}
	p := &parent{caseID: caseID, energy: obs.Energy}
	f.parents = append(f.parents, p)
	f.byID[caseID] = p
	return obs
}

// energy rewards depth (normalised stage order) and novelty.
func (f *Feedback) energy(depth, novel int) float64 {
	d := 0.0
	if f.maxOrder > 0 {
		d = float64(depth) / float64(f.maxOrder)
	}
	return (1 + depthWeight*d) * (1 + noveltyWeight*float64(novel))
}

// depth returns the highest stage order the trace captured.
func (f *Feedback) depth(tr *parity.Trace) (int, parity.StageID) {
	best, id := 0, parity.StageID("")
	for _, s := range capturedStages(tr) {
		if o := f.order[s]; o > best || (o == best && id == "") {
			best, id = o, s
		}
	}
	return best, id
}

// Pick returns a kept case drawn by energy and decays its energy.
func (f *Feedback) Pick(r *RNG) (string, bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	weights := make([]int, len(f.parents))
	for i, p := range f.parents {
		weights[i] = int(p.energy*1000) + 1
	}
	i := r.Weighted(weights)
	if i < 0 {
		return "", false
	}
	p := f.parents[i]
	p.picks++
	p.energy *= energyDecay
	if p.energy < energyFloor {
		p.energy = energyFloor
	}
	return p.caseID, true
}

// Len returns the number of kept cases.
func (f *Feedback) Len() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.parents)
}

// Coverage returns the distinct signatures seen so far, sorted.
func (f *Feedback) Coverage() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return sortedKeys(f.seen)
}

// capturedStages lists the stages a trace captured, sorted.
func capturedStages(tr *parity.Trace) []parity.StageID {
	if tr == nil {
		return nil
	}
	set := map[parity.StageID]bool{}
	for id, st := range tr.StageStatus {
		if st == parity.StageCaptured {
			set[id] = true
		}
	}
	for id, entries := range tr.Stages {
		if len(entries) > 0 && tr.StageStatus[id] != parity.StageNotReached {
			set[id] = true
		}
	}
	out := make([]parity.StageID, 0, len(set))
	for id := range set {
		out = append(out, id)
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out
}

// TraceSignatures reduces a trace to its sorted, distinct coverage signatures.
func TraceSignatures(tr *parity.Trace) []string {
	if tr == nil {
		return nil
	}
	set := map[string]bool{}
	for _, s := range capturedStages(tr) {
		set["stage:"+string(s)] = true
	}
	if tr.HTTPResponse != nil {
		set["status:"+strconv.Itoa(tr.HTTPResponse.Status)] = true
	}
	for id, entries := range tr.Stages {
		for _, e := range entries {
			for _, er := range e.Errors {
				set[errorSignature("err", id, er)] = true
			}
			for _, w := range e.Warnings {
				set[errorSignature("warn", id, w)] = true
			}
		}
	}
	for _, code := range nonBidCodes(tr) {
		set["nonbid:"+strconv.Itoa(code)] = true
	}
	for _, d := range tr.Decisions {
		set["decision:"+string(d.Point)+"="+compactJSON(d.Value)] = true
	}
	for _, c := range tr.HTTPOut {
		set[outboundSignature(c)] = true
	}
	for _, a := range tr.Analytics {
		set["analytics:"+a.Kind] = true
	}
	return sortedKeys(set)
}

func errorSignature(kind string, stage parity.StageID, e parity.ErrorRecord) string {
	return kind + ":" + string(stage) + ":" + strconv.Itoa(e.Code) + ":" + MessageTemplate(e.Message)
}

func outboundSignature(c parity.OutboundCall) string {
	switch {
	case c.Error != nil:
		return "out:" + c.Kind + ":error"
	case c.Response != nil:
		return "out:" + c.Kind + ":" + strconv.Itoa(c.Response.Status)
	}
	return "out:" + c.Kind
}

// nonBidCodes decodes the seat_non_bids stage entries.
func nonBidCodes(tr *parity.Trace) []int {
	set := map[int]bool{}
	for _, e := range tr.Stages[parity.StageSeatNonBids] {
		var snb stagedata.SeatNonBids
		if json.Unmarshal(e.JSON, &snb) != nil {
			continue
		}
		for _, list := range snb {
			for _, nb := range list {
				set[nb.StatusCode] = true
			}
		}
	}
	out := make([]int, 0, len(set))
	for c := range set {
		out = append(out, c)
	}
	sort.Ints(out)
	return out
}

func compactJSON(raw json.RawMessage) string {
	var buf bytes.Buffer
	if json.Compact(&buf, raw) != nil {
		return string(raw)
	}
	return buf.String()
}

var (
	quotedRE = regexp.MustCompile(`"[^"]*"|'[^']*'`)
	digitsRE = regexp.MustCompile(`[0-9]+`)
)

// MessageTemplate abstracts the variable parts of an error message: quoted
// text becomes "*" and digit runs become #, so "request.imp[3].id" and
// "request.imp[0].id" share one signature.
func MessageTemplate(msg string) string {
	t := quotedRE.ReplaceAllString(msg, `"*"`)
	t = digitsRE.ReplaceAllString(t, "#")
	if len(t) > maxTemplate {
		t = t[:maxTemplate]
	}
	return t
}

// SchedulerOptions tune a Scheduler.
type SchedulerOptions struct {
	// ChildShare is the chance that Next derives a child of a kept case once
	// one exists (default 0.5).
	ChildShare float64
}

// Scheduler interleaves fresh cases with children of kept cases. Its order
// depends on what was observed, but every case it returns regenerates from
// its own ID.
type Scheduler struct {
	g     *Generator
	fb    *Feedback
	seed  uint64
	rng   *RNG
	share float64
	fresh uint64
	salt  uint64
}

// NewScheduler returns a scheduler drawing fresh cases of run seed.
func NewScheduler(g *Generator, fb *Feedback, seed uint64, opts SchedulerOptions) *Scheduler {
	share := opts.ChildShare
	if share <= 0 {
		share = 0.5
	}
	return &Scheduler{g: g, fb: fb, seed: seed, rng: Derive(seed, "scheduler"), share: share}
}

// Next returns the next case to run.
func (s *Scheduler) Next() (corpus.Case, error) {
	if s.fb != nil && s.fb.Len() > 0 && s.rng.Chance(s.share) {
		if id, ok := s.fb.Pick(s.rng); ok {
			s.salt++
			if cs, err := s.g.Child(id, s.salt); err == nil {
				return cs, nil
			}
		}
	}
	cs, err := s.g.Case(s.seed, s.fresh)
	s.fresh++
	return cs, err
}
