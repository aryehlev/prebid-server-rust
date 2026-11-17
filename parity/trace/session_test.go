package trace

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"runtime"
	"sync"
	"testing"
	"time"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/collectors"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestMain registers the json.RawMessage encoder main.go installs, so
// jsonutil.Marshal behaves as it does in the server.
func TestMain(m *testing.M) {
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
	os.Exit(m.Run())
}

func newEnv(caseID string) *parity.Envelope {
	return &parity.Envelope{CaseID: caseID, Request: parity.ExecRequest{Path: "/openrtb2/auction"}}
}

// TestNilSessionIsNoOp checks the disabled path: every recorder on a nil
// session must be safe and return the production default.
func TestNilSessionIsNoOp(t *testing.T) {
	var s *Session
	require.Nil(t, Current())
	tests := []struct {
		name string
		run  func() any
		want any
	}{
		{name: "Envelope", run: func() any { return s.Envelope() }, want: (*parity.Envelope)(nil)},
		{name: "CaseID", run: func() any { return s.CaseID() }, want: ""},
		{name: "NextSeq", run: func() any { return s.NextSeq() }, want: 0},
		{name: "Wants", run: func() any { return s.Wants(parity.StageHTTPIn) }, want: false},
		{name: "Intn", run: func() any { return s.Intn(parity.DecisionFloorsModelGroup, 5, func(int) int { return 3 }) }, want: 3},
		{name: "Decide", run: func() any { return s.Decide(parity.DecisionFloorsSkip, func() int { return 1 }) }, want: 1},
		{name: "DecideString", run: func() any { return s.DecideString(parity.DecisionUsersyncShuffle, func() string { return "rng" }) }, want: "rng"},
		{name: "DecideBool", run: func() any { return s.DecideBool(parity.DecisionFloorsEnforce, func() bool { return true }) }, want: true},
		{name: "DecideChance", run: func() any { return s.DecideChance(parity.DecisionFloorsSkip, true, func() bool { return true }) }, want: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, tt.run())
		})
	}
	assert.NotPanics(t, func() {
		s.Record(parity.StageHTTPIn, "", map[string]int{"a": 1})
		s.RecordRaw(parity.StageHTTPIn, "", []byte("{}"))
		s.Errors(parity.StageHTTPIn, "", []error{errors.New("x")})
		s.Mark(parity.StageHTTPIn, parity.StageCaptured)
		s.Note("n")
		s.AddOutbound(parity.OutboundCall{})
		s.AddAnalytics(parity.AnalyticsAuction, map[string]int{})
	})
	snap := s.Snapshot()
	assert.Equal(t, parity.SchemaTrace, snap.Schema)
	assert.NotNil(t, snap.HTTPOut)
	assert.NotNil(t, snap.Notes)
}

func TestBeginPublishesAndEndReleases(t *testing.T) {
	src := fakes.Install()
	t.Cleanup(fakes.Uninstall)

	env := newEnv("case-1")
	env.Fakes = &parity.FakeValues{Int63: 77}
	s, end := Begin(env)
	assert.Same(t, s, Current())
	assert.Equal(t, "case-1", s.CaseID())
	assert.Equal(t, int64(77), src.GenerateInt63(), "envelope fakes are applied for the case")

	end()
	end() // idempotent
	assert.Nil(t, Current())
	assert.Equal(t, fakes.DefaultInt63, src.GenerateInt63(), "fakes reset to defaults after the case")

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	s2, end2, err := BeginContext(ctx, newEnv("case-2"))
	require.NoError(t, err, "the exec lock must be free again")
	assert.Same(t, s2, Current())
	end2()
}

func TestBeginContextGivesUpWhileLocked(t *testing.T) {
	_, end := Begin(newEnv("holder"))
	defer end()

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	s, end2, err := BeginContext(ctx, newEnv("waiter"))
	require.Error(t, err)
	assert.ErrorIs(t, err, context.DeadlineExceeded)
	assert.Nil(t, s)
	assert.NotPanics(t, end2)
	assert.Equal(t, "holder", Current().CaseID())
}

func TestRecordKeepsLexicalForm(t *testing.T) {
	s := newSession(newEnv("lex"))
	tests := []struct {
		name     string
		record   func()
		stage    parity.StageID
		key      string
		wantRaw  *string
		wantB64  bool
		wantJSON string
	}{
		{
			name:     "jsonutil escapes html like Go",
			record:   func() { s.Record(parity.StageRequestValidated, "", map[string]string{"adm": "<b>&"}) },
			stage:    parity.StageRequestValidated,
			key:      parity.UnkeyedStage,
			wantRaw:  strPtr(htmlEscapedADM),
			wantJSON: htmlEscapedADM,
		},
		{
			name:     "raw bytes are exact",
			record:   func() { s.RecordRaw(parity.StageHTTPResponse, "", []byte("{\"a\":1.0}\n")) },
			stage:    parity.StageHTTPResponse,
			key:      parity.UnkeyedStage,
			wantRaw:  strPtr("{\"a\":1.0}\n"),
			wantJSON: "{\"a\":1.0}\n",
		},
		{
			name:    "non utf8 goes to raw_b64",
			record:  func() { s.RecordRaw(parity.StageAdapterBids, "appnexus#0", []byte{0xff}) },
			stage:   parity.StageAdapterBids,
			key:     "appnexus#0",
			wantB64: true,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tt.record()
			e := s.Snapshot().Stages[tt.stage][tt.key]
			assert.Equal(t, tt.wantRaw, e.Raw)
			assert.Equal(t, tt.wantB64, e.RawB64 != nil)
			assert.Equal(t, tt.wantJSON, string(e.JSON))
			assert.Equal(t, 1, e.Writes)
		})
	}
}

func TestRecordOverwriteCountsWrites(t *testing.T) {
	s := newSession(newEnv("w"))
	s.Record(parity.StageBidderRequest, "appnexus", 1)
	s.Record(parity.StageBidderRequest, "appnexus", 2)
	e := s.Snapshot().Stages[parity.StageBidderRequest]["appnexus"]
	assert.Equal(t, 2, e.Writes)
	assert.Equal(t, "2", string(e.JSON))
	assert.Equal(t, 2, e.Seq)
}

func TestConcurrentRecord(t *testing.T) {
	s := newSession(newEnv("race"))
	const workers = 100
	var wg sync.WaitGroup
	wg.Add(workers)
	for i := 0; i < workers; i++ {
		go func(i int) {
			defer wg.Done()
			bidder := fmt.Sprintf("bidder%03d", i)
			s.Record(parity.StageBidderRequest, bidder, map[string]int{"i": i})
			s.Errors(parity.StageBidderResult, bidder, []error{errors.New("e")})
			s.AddOutbound(parity.OutboundCall{Seq: s.NextSeq(), Bidder: bidder})
			s.Decide(parity.DecisionCategoryDedupe, func() int { return i % 2 })
			s.Note("n%d", i)
		}(i)
	}
	wg.Wait()
	snap := s.Snapshot()
	assert.Len(t, snap.Stages[parity.StageBidderRequest], workers)
	assert.Len(t, snap.Stages[parity.StageBidderResult], workers)
	assert.Len(t, snap.HTTPOut, workers)
	assert.Len(t, snap.Decisions, workers)
	assert.Len(t, snap.Notes, workers)
	seqs := map[int]bool{}
	for _, e := range snap.Stages[parity.StageBidderRequest] {
		assert.False(t, seqs[e.Seq], "seq values are unique")
		seqs[e.Seq] = true
	}
}

func TestCaptureFilter(t *testing.T) {
	env := newEnv("filter")
	env.Capture.Stages = json.RawMessage(`["http.in"]`)
	no := false
	env.Capture.Analytics = &no
	s := newSession(env)

	s.Record(parity.StageHTTPIn, "", 1)
	s.Record(parity.StageBidderRequest, "a", 1)
	s.AddOutbound(parity.OutboundCall{})
	s.AddAnalytics(parity.AnalyticsAuction, 1)
	snap := s.Snapshot()

	assert.Contains(t, snap.Stages, parity.StageHTTPIn)
	assert.NotContains(t, snap.Stages, parity.StageBidderRequest)
	assert.Empty(t, snap.HTTPOut)
	assert.Empty(t, snap.Analytics)
	assert.Equal(t, parity.StageNotReached, snap.StageStatus[parity.StageBidderRequest])
}

// TestAddAnalyticsKeepsTraceEncodable: jsonutil.Marshal writes nothing for a
// json.RawMessage member that does not compact, leaving `"ext":}` (as for the
// auction object of a request whose regs.ext Go left invalid); such an object
// is noted, not embedded.
func TestAddAnalyticsKeepsTraceEncodable(t *testing.T) {
	type regs struct {
		GDPR int             `json:"gdpr"`
		Ext  json.RawMessage `json:"ext,omitempty"`
	}
	type record struct {
		Regs *regs `json:"regs"`
	}
	tests := []struct {
		name     string
		object   any
		recorded bool
	}{
		{name: "valid object", object: record{Regs: &regs{GDPR: 1, Ext: json.RawMessage(`{}`)}}, recorded: true},
		{name: "invalid raw member", object: record{Regs: &regs{GDPR: 1, Ext: json.RawMessage(`{"dsa":`)}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newSession(newEnv("analytics"))
			s.AddAnalytics(parity.AnalyticsAuction, tt.object)
			snap := s.Snapshot()

			_, err := json.Marshal(snap)
			require.NoError(t, err, "the trace must encode")
			if tt.recorded {
				assert.Len(t, snap.Analytics, 1)
				return
			}
			assert.Empty(t, snap.Analytics)
			require.Len(t, snap.Notes, 1)
			assert.Contains(t, snap.Notes[0], `not valid JSON`)
			assert.Contains(t, snap.Notes[0], `\"ext\":}`)
		})
	}
}

func TestSnapshotStageStatus(t *testing.T) {
	s := newSession(newEnv("status"))
	s.Record(parity.StageHTTPIn, "", 1)
	s.Mark(parity.StageMetricsDelta, parity.StageCaptured)
	s.Record(parity.StageID("made.up"), "", 1)
	snap := s.Snapshot()

	for _, stage := range parity.AllStages() {
		assert.Contains(t, snap.StageStatus, stage, "every registered stage has a status")
	}
	assert.Equal(t, parity.StageCaptured, snap.StageStatus[parity.StageHTTPIn])
	assert.Equal(t, parity.StageCaptured, snap.StageStatus[parity.StageMetricsDelta])
	assert.Equal(t, parity.StageNotReached, snap.StageStatus[parity.StageHTTPResponse])
	assert.Contains(t, snap.Notes, "unregistered stage recorded: made.up")
	assert.Equal(t, "/openrtb2/auction", snap.Endpoint)
}

func TestInvalidCaptureFilterIsNoted(t *testing.T) {
	env := newEnv("bad-filter")
	env.Capture.Stages = json.RawMessage(`"some"`)
	s := newSession(env)
	assert.True(t, s.Wants(parity.StageHTTPIn), "an invalid filter captures everything")
	require.Len(t, s.Snapshot().Notes, 1)
	assert.Contains(t, s.Snapshot().Notes[0], "capture.stages ignored")
}

func TestDecisions(t *testing.T) {
	tests := []struct {
		name       string
		decisions  map[string]json.RawMessage
		run        func(s *Session) any
		want       any
		wantValue  string
		wantSource string
		wantNote   bool
	}{
		{
			name:      "binary override honoured",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsSkip): json.RawMessage(`1`)},
			run: func(s *Session) any {
				return s.Decide(parity.DecisionFloorsSkip, func() int { return 0 })
			},
			want: 1, wantValue: "1", wantSource: parity.DecisionSourceEnvelope,
		},
		{
			name:      "binary override out of domain falls back",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsSkip): json.RawMessage(`2`)},
			run: func(s *Session) any {
				return s.Decide(parity.DecisionFloorsSkip, func() int { return 0 })
			},
			want: 0, wantValue: "0", wantSource: parity.DecisionSourceRNG, wantNote: true,
		},
		{
			name: "no override uses rng",
			run: func(s *Session) any {
				return s.Decide(parity.DecisionCategoryDedupe, func() int { return 1 })
			},
			want: 1, wantValue: "1", wantSource: parity.DecisionSourceRNG,
		},
		{
			name:      "index override in range",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsModelGroup): json.RawMessage(`2`)},
			run: func(s *Session) any {
				return s.Intn(parity.DecisionFloorsModelGroup, 3, func(int) int { return 0 })
			},
			want: 2, wantValue: "2", wantSource: parity.DecisionSourceEnvelope,
		},
		{
			name:      "index override out of range falls back",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsModelGroup): json.RawMessage(`3`)},
			run: func(s *Session) any {
				return s.Intn(parity.DecisionFloorsModelGroup, 3, func(n int) int { return n - 1 })
			},
			want: 2, wantValue: "2", wantSource: parity.DecisionSourceRNG, wantNote: true,
		},
		{
			name:      "string override",
			decisions: map[string]json.RawMessage{string(parity.DecisionUsersyncShuffle): json.RawMessage(`"identity"`)},
			run: func(s *Session) any {
				return s.DecideString(parity.DecisionUsersyncShuffle, func() string { return "rng" })
			},
			want: parity.UsersyncShuffleIdentity, wantValue: `"identity"`, wantSource: parity.DecisionSourceEnvelope,
		},
		{
			name: "string without override",
			run: func(s *Session) any {
				return s.DecideString(parity.DecisionUsersyncShuffle, func() string { return "rng" })
			},
			want: "rng", wantValue: `"rng"`, wantSource: parity.DecisionSourceRNG,
		},
		{
			name:      "string override out of domain falls back",
			decisions: map[string]json.RawMessage{string(parity.DecisionUsersyncShuffle): json.RawMessage(`"reverse"`)},
			run: func(s *Session) any {
				return s.DecideString(parity.DecisionUsersyncShuffle, func() string { return "rng" })
			},
			want: "rng", wantValue: `"rng"`, wantSource: parity.DecisionSourceRNG, wantNote: true,
		},
		{
			name:      "bool override honoured",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsEnforce): json.RawMessage(`0`)},
			run: func(s *Session) any {
				return s.DecideBool(parity.DecisionFloorsEnforce, func() bool { return true })
			},
			want: false, wantValue: "0", wantSource: parity.DecisionSourceEnvelope,
		},
		{
			name: "bool without override uses draw",
			run: func(s *Session) any {
				return s.DecideBool(parity.DecisionFloorsDataRate, func() bool { return true })
			},
			want: true, wantValue: "1", wantSource: parity.DecisionSourceRNG,
		},
		{
			name:      "chance override honoured when the rate leaves it to chance",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsSkip): json.RawMessage(`0`)},
			run: func(s *Session) any {
				return s.DecideChance(parity.DecisionFloorsSkip, true, func() bool { return true })
			},
			want: false, wantValue: "0", wantSource: parity.DecisionSourceEnvelope,
		},
		{
			name:      "chance override ignored when the rate fixes the outcome",
			decisions: map[string]json.RawMessage{string(parity.DecisionFloorsSkip): json.RawMessage(`0`)},
			run: func(s *Session) any {
				return s.DecideChance(parity.DecisionFloorsSkip, false, func() bool { return true })
			},
			want: true, wantValue: "1", wantSource: parity.DecisionSourceRNG, wantNote: true,
		},
		{
			name: "chance without override and a fixed outcome records the draw",
			run: func(s *Session) any {
				return s.DecideChance(parity.DecisionFloorsEnforce, false, func() bool { return false })
			},
			want: false, wantValue: "0", wantSource: parity.DecisionSourceRNG,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			env := newEnv("decide")
			env.Decisions = tt.decisions
			s := newSession(env)
			assert.Equal(t, tt.want, tt.run(s))
			decisions := s.Snapshot().Decisions
			require.Len(t, decisions, 1)
			assert.Equal(t, tt.wantValue, string(decisions[0].Value))
			assert.Equal(t, tt.wantSource, decisions[0].Source)
			assert.Equal(t, tt.wantNote, len(decisions[0].Notes) > 0)
		})
	}
}

func TestErrorRecordOf(t *testing.T) {
	tests := []struct {
		name string
		err  error
		want parity.ErrorRecord
	}{
		{
			name: "timeout",
			err:  &errortypes.Timeout{Message: "t"},
			want: parity.ErrorRecord{Code: errortypes.TimeoutErrorCode, Kind: "Timeout", Message: "t", Scope: parity.ScopeGeneral, Severity: parity.SeverityFatal, Fatal: true},
		},
		{
			name: "bad input",
			err:  &errortypes.BadInput{Message: "b"},
			want: parity.ErrorRecord{Code: errortypes.BadInputErrorCode, Kind: "BadInput", Message: "b", Scope: parity.ScopeGeneral, Severity: parity.SeverityFatal, Fatal: true},
		},
		{
			name: "bad server response",
			err:  &errortypes.BadServerResponse{Message: "s"},
			want: parity.ErrorRecord{Code: errortypes.BadServerResponseErrorCode, Kind: "BadServerResponse", Message: "s", Scope: parity.ScopeGeneral, Severity: parity.SeverityFatal, Fatal: true},
		},
		{
			name: "warning",
			err:  &errortypes.Warning{Message: "w", WarningCode: errortypes.MultiBidWarningCode},
			want: parity.ErrorRecord{Code: errortypes.MultiBidWarningCode, Kind: "MultiBidWarning", Message: "w", Scope: parity.ScopeGeneral, Severity: parity.SeverityWarning, Fatal: false},
		},
		{
			name: "debug warning",
			err:  &errortypes.DebugWarning{Message: "d", WarningCode: errortypes.BidderLevelDebugDisabledWarningCode},
			want: parity.ErrorRecord{Code: errortypes.BidderLevelDebugDisabledWarningCode, Kind: "BidderLevelDebugDisabledWarning", Message: "d", Scope: parity.ScopeDebug, Severity: parity.SeverityWarning, Fatal: false},
		},
		{
			name: "plain error is unknown and fatal",
			err:  errors.New("plain"),
			want: parity.ErrorRecord{Code: errortypes.UnknownErrorCode, Kind: "UnknownError", Message: "plain", Scope: parity.ScopeGeneral, Severity: parity.SeverityUnknown, Fatal: true},
		},
		{
			name: "nil error",
			err:  nil,
			want: parity.ErrorRecord{Code: errortypes.UnknownErrorCode, Kind: "UnknownError", Scope: parity.ScopeGeneral, Severity: parity.SeverityUnknown, Fatal: true},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, ErrorRecordOf(tt.err))
		})
	}
}

func TestErrorsSplitsWarnings(t *testing.T) {
	s := newSession(newEnv("errs"))
	s.Errors(parity.StageRequestValidated, "", []error{
		&errortypes.BadInput{Message: "b"},
		&errortypes.Warning{Message: "w", WarningCode: errortypes.InvalidPrivacyConsentWarningCode},
	})
	s.Errors(parity.StageBidderResult, "x", nil)
	snap := s.Snapshot()

	validated := snap.Stages[parity.StageRequestValidated][parity.UnkeyedStage]
	require.Len(t, validated.Errors, 1)
	require.Len(t, validated.Warnings, 1)
	assert.Equal(t, errortypes.BadInputErrorCode, validated.Errors[0].Code)
	assert.Equal(t, errortypes.InvalidPrivacyConsentWarningCode, validated.Warnings[0].Code)

	empty := snap.Stages[parity.StageBidderResult]["x"]
	assert.NotNil(t, empty.Errors, "recorded-empty is [] not null")
	assert.NotNil(t, empty.Warnings)
	assert.Empty(t, empty.Errors)
}

func TestMetricsDelta(t *testing.T) {
	reg := prometheus.NewRegistry()
	counter := prometheus.NewCounterVec(prometheus.CounterOpts{Name: "requests"}, []string{"type"})
	gauge := prometheus.NewGauge(prometheus.GaugeOpts{Name: "connections"})
	hist := prometheus.NewHistogram(prometheus.HistogramOpts{Name: "latency_seconds", Buckets: []float64{0.1, 1}})
	unchanged := prometheus.NewCounter(prometheus.CounterOpts{Name: "unchanged"})
	reg.MustRegister(counter, gauge, hist, unchanged)
	counter.WithLabelValues("amp").Add(2)
	unchanged.Inc()

	before, err := GatherMetrics(reg)
	require.NoError(t, err)
	counter.WithLabelValues("amp").Inc()
	counter.WithLabelValues("web").Add(3)
	gauge.Set(-2)
	hist.Observe(0.5)
	hist.Observe(5)
	after, err := GatherMetrics(reg)
	require.NoError(t, err)

	got := MetricsDelta(before, after)
	two := int64(2)
	want := []parity.MetricDelta{
		{Name: "connections", Type: parity.MetricGauge, Labels: map[string]string{}, Delta: -2},
		{Name: "latency_seconds", Type: parity.MetricHistogram, Labels: map[string]string{}, Delta: 5.5, CountDelta: &two, BucketDeltas: map[string]int64{"1": 1}},
		{Name: "requests", Type: parity.MetricCounter, Labels: map[string]string{"type": "amp"}, Delta: 1},
		{Name: "requests", Type: parity.MetricCounter, Labels: map[string]string{"type": "web"}, Delta: 3},
	}
	assert.Equal(t, want, got)

	_, err = GatherMetrics(nil)
	assert.Error(t, err)
}

func TestValueDelta(t *testing.T) {
	tests := []struct {
		name          string
		before, after float64
		want          float64
	}{
		{name: "finite", before: 1.5, after: 4, want: 2.5},
		{name: "unchanged", before: 3, after: 3, want: 0},
		{name: "both NaN", before: math.NaN(), after: math.NaN(), want: 0},
		{name: "becomes NaN", before: 1, after: math.NaN(), want: 0},
		{name: "becomes infinite", before: 1, after: math.Inf(1), want: 0},
		{name: "both infinite", before: math.Inf(1), after: math.Inf(1), want: 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, valueDelta(tt.before, tt.after))
		})
	}
}

// TestMetricsDeltaWithGoCollectorEncodes reproduces the registry PBS builds
// (metrics/prometheus/prometheus.go registers NewGoCollector): its runtime
// histograms carry NaN sums, and the delta must still encode as JSON.
func TestMetricsDeltaWithGoCollectorEncodes(t *testing.T) {
	reg := prometheus.NewRegistry()
	reg.MustRegister(collectors.NewGoCollector())
	before, err := GatherMetrics(reg)
	require.NoError(t, err)
	runtime.GC()
	after, err := GatherMetrics(reg)
	require.NoError(t, err)

	got := MetricsDelta(before, after)
	_, err = json.Marshal(got)
	require.NoError(t, err)
	for _, d := range got {
		assert.False(t, math.IsNaN(d.Delta) || math.IsInf(d.Delta, 0), "%s delta %v", d.Name, d.Delta)
	}
}

func TestFormatBound(t *testing.T) {
	tests := []struct {
		in   float64
		want string
	}{
		{in: 0.005, want: "0.005"},
		{in: 1, want: "1"},
		{in: 2.5, want: "2.5"},
	}
	for _, tt := range tests {
		t.Run(tt.want, func(t *testing.T) {
			assert.Equal(t, tt.want, formatBound(tt.in))
		})
	}
}

func TestBidderContext(t *testing.T) {
	tests := []struct {
		name   string
		ctx    context.Context
		want   string
		wantOK bool
	}{
		{name: "set", ctx: WithBidder(context.Background(), "appnexus"), want: "appnexus", wantOK: true},
		{name: "unset", ctx: context.Background(), want: "", wantOK: false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, ok := BidderFrom(tt.ctx)
			assert.Equal(t, tt.wantOK, ok)
			assert.Equal(t, tt.want, got)
		})
	}
}

func strPtr(s string) *string { return &s }

// htmlEscapedADM is {"adm":"<b>&"} as Go's HTML-escaping encoders write it
// (each of < > & as a backslash-u escape).
var htmlEscapedADM = `{"adm":"` + uEsc("003c") + "b" + uEsc("003e") + uEsc("0026") + `"}`

func uEsc(hex string) string { return `\` + "u" + hex }
