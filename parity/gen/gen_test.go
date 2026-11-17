package gen

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"

	"github.com/prebid/go-gdpr/vendorconsent"
	gpplib "github.com/prebid/go-gpp"
	"github.com/prebid/openrtb/v20/openrtb2"
	analyticsBuild "github.com/prebid/prebid-server/v4/analytics/build"
	"github.com/prebid/prebid-server/v4/config"
	endpointsOpenrtb2 "github.com/prebid/prebid-server/v4/endpoints/openrtb2"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/hooks"
	metricsConfig "github.com/prebid/prebid-server/v4/metrics/config"
	"github.com/prebid/prebid-server/v4/ortb"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/oracle"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/privacy/ccpa"
	"github.com/prebid/prebid-server/v4/stored_requests"
	"github.com/prebid/prebid-server/v4/stored_requests/backends/file_fetcher"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// Seeds and sizes of the generated runs the tests inspect.
const (
	testSeed  = 20261117
	testCases = 400
)

var (
	testOnce sync.Once
	testGen  *Generator
	testRepo *corpus.Repo
	testErr  error

	runOnce  sync.Once
	runCases []corpus.Case
	runErr   error
)

// loadGen builds the generator over the repository corpus once per binary.
func loadGen(t *testing.T) *Generator {
	t.Helper()
	testOnce.Do(func() {
		testRepo, testErr = corpus.FindRepo(".")
		if testErr != nil {
			return
		}
		testGen, testErr = Load(testRepo, Options{})
	})
	require.NoError(t, testErr)
	return testGen
}

// generatedRun returns cases 0..testCases-1 of testSeed.
func generatedRun(t *testing.T) []corpus.Case {
	t.Helper()
	g := loadGen(t)
	runOnce.Do(func() {
		for i := uint64(0); i < testCases; i++ {
			cs, err := g.Case(testSeed, i)
			if err != nil {
				runErr = err
				return
			}
			runCases = append(runCases, cs)
		}
	})
	require.NoError(t, runErr)
	return runCases
}

func mustJSON(t *testing.T, v any) []byte {
	t.Helper()
	raw, err := json.Marshal(v)
	require.NoError(t, err)
	return raw
}

// caseBytes encodes a case for byte comparison. Stored data is compared as
// text: corpus sources carry the Go tests' deliberately malformed stored data,
// which json.RawMessage refuses to marshal.
func caseBytes(t *testing.T, cs corpus.Case) string {
	t.Helper()
	text := func(m map[string]json.RawMessage) map[string]string {
		out := map[string]string{}
		for k, v := range m {
			out[k] = string(v)
		}
		return out
	}
	stored := []map[string]string{text(cs.Stored.Requests), text(cs.Stored.Imps), text(cs.Stored.Responses), text(cs.Stored.Accounts)}
	cs.Stored = corpus.Stored{}
	return string(mustJSON(t, struct {
		Case   corpus.Case
		Stored []map[string]string
	}{cs, stored}))
}

func TestRNGKnownAnswers(t *testing.T) {
	tests := []struct {
		name string
		got  func() any
		want any
	}{
		{"derive is stable", func() any { return Derive(1, "a", "b").Uint64() }, Derive(1, "a", "b").Uint64()},
		{"labels are separated", func() any { return Derive(1, "ab").Uint64() == Derive(1, "a", "b").Uint64() }, false},
		{"seed matters", func() any { return Derive(1, "a").Uint64() == Derive(2, "a").Uint64() }, false},
		{"uint64n of zero", func() any { return NewRNG(1, 2).Uint64n(0) }, uint64(0)},
		{"intn of negative", func() any { return NewRNG(1, 2).Intn(-3) }, 0},
		{"range inverted", func() any { return NewRNG(1, 2).Range(5, 1) }, 5},
		{"weighted without weight", func() any { return NewRNG(1, 2).Weighted([]int{0, -1}) }, -1},
		{"weighted single", func() any { return NewRNG(1, 2).Weighted([]int{0, 3, 0}) }, 1},
		{"pick empty", func() any { return Pick(NewRNG(1, 2), []string{}) }, ""},
		{"sample keeps order", func() any { return Sample(NewRNG(3, 4), []int{1, 2, 3, 4, 5}, 5) }, []int{1, 2, 3, 4, 5}},
		{"hex length", func() any { return len(NewRNG(1, 2).Hex(13)) }, 13},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) { assert.Equal(t, tt.want, tt.got()) })
	}
}

var (
	pinnedStream = []uint64{652, 651, 30}
	pinnedUUID   = "cb9cc800-a55c-41ba-bc50-98532ef96973"
)

func TestRNGPinnedStream(t *testing.T) {
	// Pinned values: a change means every generated case ID changes meaning.
	r := Derive(42, "pinned")
	got := []uint64{r.Uint64n(1000), r.Uint64n(1000), r.Uint64n(1000)}
	assert.Equal(t, pinnedStream, got)
	assert.Equal(t, pinnedUUID, Derive(42, "uuid").UUID())
}

func TestRNGBounds(t *testing.T) {
	r := Derive(7, "bounds")
	counts := map[int]int{}
	for i := 0; i < 5000; i++ {
		v := r.Range(-2, 2)
		require.GreaterOrEqual(t, v, -2)
		require.LessOrEqual(t, v, 2)
		counts[v]++
		s := Sample(r, []int{1, 2, 3, 4, 5, 6}, 3)
		require.Len(t, s, 3)
		require.True(t, sort.IntsAreSorted(s), "sample keeps input order")
	}
	assert.Len(t, counts, 5)
	uuid := r.UUID()
	assert.Len(t, uuid, 36)
	assert.Equal(t, byte('4'), uuid[14])
	assert.Contains(t, "89ab", string(uuid[19]))
}

func TestParseID(t *testing.T) {
	tests := []struct {
		in   string
		want ID
		err  bool
	}{
		{in: "gen:1:2:auction", want: ID{Seed: 1, Index: 2, Chain: []Step{{Name: "auction"}}}},
		{in: "gen:18446744073709551615:0:amp+amp.size+privacy.gdpr@7", want: ID{Seed: 1<<64 - 1, Chain: []Step{{Name: "amp"}, {Name: "amp.size"}, {Name: "privacy.gdpr", Variant: 7}}}},
		{in: "adapters/a/b.json#lift:auction#mut:3:4:raw.bom", want: ID{Source: "adapters/a/b.json#lift:auction", Seed: 3, Index: 4, Chain: []Step{{Name: "raw.bom"}}}},
		{in: "", err: true},
		{in: "gen:", err: true},
		{in: "gen:1:2:", err: true},
		{in: "gen:x:2:auction", err: true},
		{in: "gen:1:-2:auction", err: true},
		{in: "gen:1:2:Auction", err: true},
		{in: "gen:1:2:auction@0", err: true},
		{in: "gen:1:2:auction@x", err: true},
		{in: "gen:1:2:a..b", err: true},
		{in: "gen:1:2:a+", err: true},
		{in: "#mut:1:2:raw.bom", err: true},
		{in: "case#mut:1:2", err: true},
		{in: "plain-case-id", err: true},
	}
	for _, tt := range tests {
		t.Run(tt.in, func(t *testing.T) {
			got, err := ParseID(tt.in)
			if tt.err {
				require.Error(t, err)
				assert.True(t, errors.Is(err, ErrInvalidID))
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
			assert.Equal(t, tt.in, got.String())
		})
	}
}

func TestMutatorRegistry(t *testing.T) {
	infos := Mutators()
	require.NotEmpty(t, infos)
	seen := map[string]bool{BaseAuction: true, BaseAMP: true, BaseVideo: true}
	for _, m := range infos {
		assert.True(t, validStepName(m.Name), m.Name)
		assert.False(t, seen[m.Name], "duplicate step %s", m.Name)
		seen[m.Name] = true
		assert.NotEmpty(t, m.Doc, m.Name)
		assert.NotEmpty(t, m.Endpoints, m.Name)
		assert.Positive(t, m.Weight, m.Name)
		assert.NotEqual(t, "base", m.Phase, m.Name)
	}
	_, ok := lookupMutator(fallbackStep)
	assert.True(t, ok)
}

func TestSameSeedGivesIdenticalCases(t *testing.T) {
	g := loadGen(t)
	other, err := New(testRepo, g.corpus, Options{})
	require.NoError(t, err)
	for _, cs := range generatedRun(t)[:120] {
		id, err := ParseID(cs.ID)
		require.NoError(t, err)
		again, err := other.Case(id.Seed, id.Index)
		require.NoError(t, err)
		assert.Equal(t, string(mustJSON(t, cs.Envelope)), string(mustJSON(t, again.Envelope)), cs.ID)
		assert.Equal(t, caseBytes(t, cs), caseBytes(t, again), cs.ID)
	}
}

func TestCaseIDRegenerates(t *testing.T) {
	g := loadGen(t)
	for _, cs := range generatedRun(t) {
		again, err := g.Regenerate(cs.ID)
		require.NoError(t, err, cs.ID)
		require.Equal(t, caseBytes(t, cs), caseBytes(t, again), cs.ID)
	}
}

func TestChildRegenerates(t *testing.T) {
	g := loadGen(t)
	parents := []string{}
	for _, cs := range generatedRun(t)[:60] {
		parents = append(parents, cs.ID)
	}
	parents = append(parents, g.Bases()[:5]...)
	for i, p := range parents {
		child, err := g.Child(p, uint64(i+1))
		if err != nil {
			continue
		}
		assert.True(t, strings.HasPrefix(child.ID, strings.SplitN(p, "#mut:", 2)[0]), child.ID)
		again, err := g.Regenerate(child.ID)
		require.NoError(t, err, child.ID)
		require.Equal(t, caseBytes(t, child), caseBytes(t, again), child.ID)
	}
}

func TestRegenerateRejects(t *testing.T) {
	g := loadGen(t)
	tests := []struct {
		name string
		id   string
	}{
		{"unknown base", "gen:1:2:banner"},
		{"base with variant", "gen:1:2:auction@3"},
		{"unknown step", "gen:1:2:auction+no.such.step"},
		{"wrong endpoint", "gen:1:2:auction+amp.size"},
		{"unknown source", "no/such/case.json#mut:1:2:raw.bom"},
		{"malformed id", "gen:1"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := g.Regenerate(tt.id)
			assert.Error(t, err)
		})
	}
}

func TestGeneratedEnvelopesMatchContract(t *testing.T) {
	stats := map[string]int{}
	for _, cs := range generatedRun(t) {
		require.NotNil(t, cs.Envelope, cs.ID)
		assert.Equal(t, cs.ID, cs.Envelope.CaseID)
		assert.Equal(t, cs.Profile.Name(), cs.Envelope.Profile, cs.ID)
		assert.Equal(t, CorpusGenerated, cs.Corpus)
		assert.True(t, cs.HasTag(TagGenerated), cs.ID)
		assert.Len(t, cs.Envelope.Decisions, len(parity.AllDecisions()), cs.ID)
		assert.NoError(t, schema.Validate(mustJSON(t, cs.Envelope)), cs.ID)
		for _, tag := range []string{TagValidIntent, TagUncertain, TagInvalidIntent, TagMalformed} {
			if cs.HasTag(tag) {
				stats[tag]++
			}
		}
		stats[cs.Endpoint]++
		if strings.HasPrefix(cs.ID, GeneratedPrefix) {
			stats["generated"]++
		} else {
			stats["derived"]++
		}
	}
	t.Logf("generated run: %v", stats)
	for _, key := range []string{TagValidIntent, TagInvalidIntent, TagMalformed, corpus.EndpointAuction, corpus.EndpointAMP, corpus.EndpointVideo, "generated", "derived"} {
		assert.Positive(t, stats[key], key)
	}
}

func TestOracleAcceptsGeneratedPlans(t *testing.T) {
	loadGen(t)
	o, err := oracle.Start(oracle.Options{RepoRoot: testRepo.Root()})
	require.NoError(t, err)
	defer func() { assert.NoError(t, o.Close()) }()
	for _, cs := range generatedRun(t) {
		assert.NoError(t, o.Register(oracle.PlanFromCase(&cs)), cs.ID)
	}
}

func TestStoredDataMerges(t *testing.T) {
	_, errs := corpus.MergeStored(generatedRun(t))
	assert.Empty(t, errs, "content-addressed stored IDs never conflict")
}

func TestEveryMutatorApplies(t *testing.T) {
	g := loadGen(t)
	pending := map[string]*mutator{}
	for _, m := range registry() {
		pending[m.name] = m
	}
	for idx := uint64(0); idx < 300 && len(pending) > 0; idx++ {
		for _, base := range []string{BaseAuction, BaseAMP, BaseVideo} {
			for _, name := range sortedKeys(pending) {
				d, err := g.start(ID{Seed: 99, Index: idx, Chain: []Step{{Name: base}}})
				require.NoError(t, err)
				if !pending[name].appliesTo(d) {
					continue
				}
				require.NoError(t, d.apply(Step{Name: name}), name)
				cs, err := d.finish()
				require.NoError(t, err, name)
				require.NoError(t, schema.Validate(mustJSON(t, cs.Envelope)), cs.ID)
				again, err := g.Regenerate(cs.ID)
				require.NoError(t, err, cs.ID)
				require.Equal(t, caseBytes(t, cs), caseBytes(t, again), cs.ID)
				delete(pending, name)
			}
		}
	}
	assert.Empty(t, sortedKeys(pending), "steps that never applied")
}

func TestStepValidityTags(t *testing.T) {
	g := loadGen(t)
	tests := []struct {
		id  string
		tag string
	}{
		{"gen:5:1:auction", TagValidIntent},
		{"gen:5:1:auction+imp.none", TagInvalidIntent},
		{"gen:5:1:auction+raw.bom", TagMalformed},
		{"gen:5:1:auction+struct.null", TagUncertain},
		{"gen:5:1:auction+imp.none+raw.whitespace", TagInvalidIntent},
	}
	for _, tt := range tests {
		t.Run(tt.id, func(t *testing.T) {
			cs, err := g.Regenerate(tt.id)
			require.NoError(t, err)
			assert.True(t, cs.HasTag(tt.tag), cs.Tags)
		})
	}
}

func TestParamsValidity(t *testing.T) {
	g := loadGen(t)
	cat := g.catalog
	var valid, validOK, raw, rawOK, near, nearFail, firstFail int
	labels := map[string]int{}
	for _, b := range cat.bidders {
		for i := 0; i < 4; i++ {
			r := Derive(testSeed, "params", b.name, u64(uint64(i)))
			p, _ := cat.params(r, b, ParamsValid)
			valid++
			if cat.valid(b.name, mustJSON(t, p)) {
				validOK++
			}
			raw++
			if cat.valid(b.name, mustJSON(t, b.schema.instance(r))) {
				rawOK++
			}
			np, label := cat.params(r, b, ParamsNearMiss)
			near++
			labels[strings.SplitN(label, ":", 3)[1]]++
			if !cat.valid(b.name, mustJSON(t, np)) {
				nearFail++
			}
			if ms := b.schema.nearMisses(r, p); len(ms) > 0 && !cat.valid(b.name, mustJSON(t, ms[0].value)) {
				firstFail++
			}
		}
	}
	t.Logf("bidders=%d valid=%d/%d raw-schema=%d/%d near-miss-fail=%d/%d first-candidate-fail=%d/%d labels=%v",
		len(cat.bidders), validOK, valid, rawOK, raw, nearFail, near, firstFail, near, labels)
	assert.GreaterOrEqual(t, float64(validOK)/float64(valid), 0.70, "valid params pass the production validator")
	assert.GreaterOrEqual(t, float64(rawOK)/float64(raw), 0.70, "raw schema instances pass the production validator")
	assert.GreaterOrEqual(t, float64(nearFail)/float64(near), 0.90, "near-miss params fail the production validator")
	assert.GreaterOrEqual(t, float64(firstFail)/float64(near), 0.90, "the first near-miss candidate already fails")
}

func TestValidAuctionsPassGoValidation(t *testing.T) {
	g := loadGen(t)
	cases := generatedRun(t)
	serve := newValidatingServer(t, g, cases)
	total, passed := map[bool]int{}, map[bool]int{}
	failures := map[string]int{}
	for _, cs := range cases {
		if !cs.HasTag(TagValidIntent) || (cs.Endpoint != corpus.EndpointAuction && cs.Endpoint != corpus.EndpointAMP) {
			continue
		}
		fresh := strings.HasPrefix(cs.ID, GeneratedPrefix)
		total[fresh]++
		rec := serve(cs)
		if rec.Code == http.StatusOK {
			passed[fresh]++
			continue
		}
		failures[MessageTemplate(strings.TrimSpace(rec.Body.String()))+" ("+cs.ID+")"]++
	}
	for _, k := range sortedKeys(failures)[:min(len(failures), 10)] {
		t.Logf("rejected: %s", k)
	}
	t.Logf("valid-intent cases passing Go validation: generated %d/%d, derived %d/%d", passed[true], total[true], passed[false], total[false])
	require.Positive(t, total[true])
	all := float64(passed[true]+passed[false]) / float64(total[true]+total[false])
	assert.GreaterOrEqual(t, all, 0.50)
	assert.GreaterOrEqual(t, float64(passed[true])/float64(total[true]), 0.50)

	var invalid, rejected int
	for _, cs := range cases {
		if !cs.HasTag(TagInvalidIntent) || !strings.HasPrefix(cs.ID, GeneratedPrefix) || cs.Endpoint == corpus.EndpointVideo {
			continue
		}
		invalid++
		if rec := serve(cs); rec.Code != http.StatusOK {
			rejected++
		} else {
			t.Logf("accepted invalid-intent case %s", cs.ID)
		}
	}
	t.Logf("invalid-intent generated cases Go rejects: %d/%d", rejected, invalid)
	require.Positive(t, invalid)
	assert.GreaterOrEqual(t, float64(rejected)/float64(invalid), 0.80)
}

// TestStepValidityMatchesGo applies each Preserves and Invalidates step to
// valid base requests and checks Go's in-process verdict agrees.
func TestStepValidityMatchesGo(t *testing.T) {
	g := loadGen(t)
	type applied struct {
		step string
		cs   corpus.Case
	}
	var runs []applied
	for _, m := range registry() {
		if m.validity != Preserves && m.validity != Invalidates {
			continue
		}
		found := 0
		for idx := uint64(0); idx < 200 && found < 3; idx++ {
			for _, base := range []string{BaseAuction, BaseAMP} {
				if found == 3 {
					break
				}
				d, err := g.start(ID{Seed: 31, Index: idx, Chain: []Step{{Name: base}}})
				require.NoError(t, err)
				if !m.appliesTo(d) {
					continue
				}
				require.NoError(t, d.apply(Step{Name: m.name}))
				cs, err := d.finish()
				require.NoError(t, err)
				runs = append(runs, applied{m.name, cs})
				found++
			}
		}
	}
	cases := make([]corpus.Case, len(runs))
	for i, r := range runs {
		cases[i] = r.cs
	}
	serve := newValidatingServer(t, g, cases)
	var mismatches []string
	for _, r := range runs {
		rec := serve(r.cs)
		accepted := rec.Code == http.StatusOK
		if accepted != r.cs.HasTag(TagValidIntent) {
			mismatches = append(mismatches, r.cs.ID+" -> "+strconv.Itoa(rec.Code)+" "+MessageTemplate(strings.TrimSpace(rec.Body.String())))
		}
	}
	for _, m := range mismatches {
		t.Logf("mismatch: %s", m)
	}
	t.Logf("steps checked against Go: %d cases, %d mismatches", len(runs), len(mismatches))
	assert.Empty(t, mismatches)
}

// acceptingExchange accepts every auction.
type acceptingExchange struct{}

func (acceptingExchange) HoldAuction(_ context.Context, r *exchange.AuctionRequest, _ *exchange.DebugLog) (*exchange.AuctionResponse, error) {
	return &exchange.AuctionResponse{BidResponse: &openrtb2.BidResponse{ID: r.BidRequestWrapper.ID, Ext: json.RawMessage(`{}`)}}, nil
}

type fixedUUID struct{}

func (fixedUUID) Generate() (string, error) { return "parity-uuid", nil }

// newValidatingServer serves the auction and AMP endpoints with the
// production validator, the bidder-info bidder maps and the cases' stored data.
func newValidatingServer(t *testing.T, g *Generator, cases []corpus.Case) func(corpus.Case) *httptest.ResponseRecorder {
	t.Helper()
	infoDir, err := testRepo.Resolve(corpus.RootBidderInfo)
	require.NoError(t, err)
	infos, err := loadBidderInfos(infoDir)
	require.NoError(t, err)
	bidderMap := exchange.GetActiveBidders(infos)
	disabled := exchange.GetDisabledBidderWarningMessages(infos)
	stored, errs := corpus.MergeStored(cases)
	require.Empty(t, errs)
	fetcher := storedFetcher(t, stored)
	validator := ortb.NewRequestValidator(bidderMap, disabled, g.catalog.validator)
	return func(cs corpus.Case) *httptest.ResponseRecorder {
		cfg := profileConfig(t, cs.Profile)
		build := endpointsOpenrtb2.NewEndpoint
		if cs.Endpoint == corpus.EndpointAMP {
			build = endpointsOpenrtb2.NewAmpEndpoint
		}
		handle, err := build(fixedUUID{}, acceptingExchange{}, validator, fetcher, fetcher, cfg, &metricsConfig.NilMetricsEngine{}, analyticsBuild.New(&config.Analytics{}), disabled, nil, bidderMap, fetcher, hooks.EmptyPlanBuilder{}, nil)
		require.NoError(t, err)
		rec := httptest.NewRecorder()
		handle(rec, envelopeHTTPRequest(t, cs.Envelope), nil)
		return rec
	}
}

// profileConfig applies the profile settings that affect request validation.
func profileConfig(t *testing.T, p corpus.Profile) *config.Configuration {
	t.Helper()
	cfg := &config.Configuration{MaxRequestSize: defaultMaxRequest, AccountRequired: p.AccountRequired, BlockedApps: p.BlockedApps, BlockedAppsLookup: map[string]bool{}}
	for _, app := range p.BlockedApps {
		cfg.BlockedAppsLookup[app] = true
	}
	require.NoError(t, cfg.MarshalAccountDefaults())
	return cfg
}

// storedFetcher writes stored data in the file fetcher layout.
func storedFetcher(t *testing.T, s corpus.Stored) stored_requests.AllFetcher {
	t.Helper()
	dir := t.TempDir()
	dirs := map[string]map[string]json.RawMessage{"stored_requests": s.Requests, "stored_imps": s.Imps, "stored_responses": s.Responses, "accounts": s.Accounts}
	for name, files := range dirs {
		require.NoError(t, os.MkdirAll(filepath.Join(dir, name), 0o755))
		for id, raw := range files {
			require.NoError(t, os.WriteFile(filepath.Join(dir, name, id+".json"), raw, 0o600))
		}
	}
	f, err := file_fetcher.NewFileFetcher(dir)
	require.NoError(t, err)
	return f
}

func envelopeHTTPRequest(t *testing.T, env *parity.Envelope) *http.Request {
	t.Helper()
	body, err := env.Request.Body.WireBytes()
	require.NoError(t, err)
	target := env.Request.Path
	if env.Request.Query != "" {
		target += "?" + env.Request.Query
	}
	req := httptest.NewRequest(env.Request.Method, target, strings.NewReader(string(body)))
	for _, h := range env.Request.Headers {
		req.Header.Add(h[0], h[1])
	}
	req.RemoteAddr = env.Request.RemoteAddr
	return req
}

func TestPoolClassification(t *testing.T) {
	p, err := LoadPools()
	require.NoError(t, err)
	tcf := []struct {
		name    string
		pool    []string
		version uint8
		valid   bool
	}{
		{"tcf_valid", p.Privacy.TCFValid, 2, true},
		{"tcf_v1", p.Privacy.TCFv1, 1, true},
		{"tcf_invalid", p.Privacy.TCFInvalid, 0, false},
	}
	for _, tt := range tcf {
		for _, s := range tt.pool {
			t.Run(tt.name+"/"+s, func(t *testing.T) {
				c, err := vendorconsent.ParseString(s)
				if !tt.valid {
					assert.Error(t, err)
					return
				}
				require.NoError(t, err)
				assert.Equal(t, tt.version, c.Version())
			})
		}
	}
	for _, e := range p.Privacy.GPP {
		t.Run("gpp/"+e.GPP, func(t *testing.T) {
			c, errs := gpplib.Parse(e.GPP)
			assert.Equal(t, e.HeaderValid, len(c.SectionTypes) > 0, errs)
			if e.HeaderValid {
				var sids []int
				for _, s := range c.SectionTypes {
					sids = append(sids, int(s))
				}
				assert.Equal(t, e.SIDs, sids)
			}
		})
	}
	usp := []struct {
		pool  []string
		valid bool
	}{{p.Privacy.USPrivacyValid, true}, {p.Privacy.USPrivacyInvalid, false}}
	for _, tt := range usp {
		for _, s := range tt.pool {
			assert.Equal(t, tt.valid, ccpa.ValidateConsent(s), s)
		}
	}
}

// testTrace builds a trace that captured stages, with status and messages.
func testTrace(status int, stages []parity.StageID, errMsg string, nonBids string) *parity.Trace {
	tr := &parity.Trace{
		HTTPResponse: &parity.HTTPResponse{Status: status},
		Stages:       map[parity.StageID]map[string]parity.StageEntry{},
		StageStatus:  map[parity.StageID]parity.StageStatus{},
	}
	for _, s := range stages {
		tr.StageStatus[s] = parity.StageCaptured
		tr.Stages[s] = map[string]parity.StageEntry{"": {JSON: json.RawMessage(`{}`)}}
	}
	if errMsg != "" {
		tr.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{"": {Errors: []parity.ErrorRecord{{Code: 999, Message: errMsg}}}}
	}
	if nonBids != "" {
		tr.Stages[parity.StageSeatNonBids] = map[string]parity.StageEntry{"": {JSON: json.RawMessage(nonBids)}}
		tr.StageStatus[parity.StageSeatNonBids] = parity.StageCaptured
	}
	return tr
}

func TestFeedbackRewardsDepthAndNovelty(t *testing.T) {
	fb, err := LoadFeedback()
	require.NoError(t, err)
	shallow := testTrace(400, []parity.StageID{parity.StageHTTPIn, parity.StageRequestValidated}, `request.imp[0] missing required field: "id"`, "")
	deep := testTrace(200, []parity.StageID{parity.StageHTTPIn, parity.StageRequestValidated, parity.StageBidderRequest, parity.StageAuctionWinners, parity.StageExchangeResponse}, "", `{"appnexus":[{"impid":"1","statuscode":303}]}`)

	first := fb.Observe("a", shallow)
	assert.True(t, first.Kept)
	assert.Contains(t, first.Signatures, `err:request.validated:999:request.imp[#] missing required field: "*"`)
	again := fb.Observe("b", testTrace(400, []parity.StageID{parity.StageHTTPIn, parity.StageRequestValidated}, `request.imp[7] missing required field: "x"`, ""))
	assert.False(t, again.Kept, "templated messages collapse: %v", again.New)
	d := fb.Observe("c", deep)
	assert.True(t, d.Kept)
	assert.Contains(t, d.Signatures, "nonbid:303")
	assert.Greater(t, d.Depth, first.Depth)
	assert.Equal(t, parity.StageExchangeResponse, d.DeepestStage)
	assert.Equal(t, 2, fb.Len())

	r := Derive(1, "pick")
	picks := map[string]int{}
	for i := 0; i < 200; i++ {
		id, ok := fb.Pick(r)
		require.True(t, ok)
		picks[id]++
	}
	assert.Greater(t, picks["c"], 0)
	assert.Greater(t, picks["a"], 0, "decay lets shallow parents back in")
	assert.NotEmpty(t, fb.Coverage())
}

func TestMessageTemplate(t *testing.T) {
	tests := []struct{ in, want string }{
		{`request.imp[12].banner.format[0] is invalid`, `request.imp[#].banner.format[#] is invalid`},
		{`unknown bidder: "foo". Did you forget an alias`, `unknown bidder: "*". Did you forget an alias`},
		{`it's 'x'`, `it"*"x'`},
		{strings.Repeat("a", 300), strings.Repeat("a", maxTemplate)},
	}
	for _, tt := range tests {
		t.Run(tt.in[:min(len(tt.in), 20)], func(t *testing.T) { assert.Equal(t, tt.want, MessageTemplate(tt.in)) })
	}
}

func TestSchedulerCasesRegenerate(t *testing.T) {
	g := loadGen(t)
	fb, err := LoadFeedback()
	require.NoError(t, err)
	s := NewScheduler(g, fb, 77, SchedulerOptions{ChildShare: 0.6})
	children := 0
	for i := 0; i < 40; i++ {
		cs, err := s.Next()
		require.NoError(t, err)
		if strings.Count(cs.ID, "@") > 0 {
			children++
		}
		stages := []parity.StageID{parity.StageHTTPIn}
		if i%3 == 0 {
			stages = append(stages, parity.StageID("bidder.request"), parity.StageID("auction.winners"))
		}
		fb.Observe(cs.ID, testTrace(200+i, stages, "", ""))
		again, err := g.Regenerate(cs.ID)
		require.NoError(t, err, cs.ID)
		require.Equal(t, caseBytes(t, cs), caseBytes(t, again), cs.ID)
	}
	assert.Positive(t, children, "the scheduler derives children of kept cases")
}
