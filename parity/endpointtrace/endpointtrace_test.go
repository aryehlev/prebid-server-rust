package endpointtrace

import (
	"encoding/json"
	"errors"
	"testing"

	"github.com/prebid/go-gdpr/consentconstants"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/gdpr"
	"github.com/prebid/prebid-server/v4/hooks/hookanalytics"
	"github.com/prebid/prebid-server/v4/hooks/hookexecution"
	"github.com/prebid/prebid-server/v4/metrics"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/stored_responses"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/prebid/prebid-server/v4/util/ptrutil"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// begin starts a session capturing stages (nil for all) and ends it with the
// test.
func begin(t *testing.T, stages []parity.StageID) *trace.Session {
	t.Helper()
	env := &parity.Envelope{CaseID: t.Name(), Request: parity.ExecRequest{Path: "/openrtb2/auction"}}
	if stages != nil {
		raw, err := json.Marshal(stages)
		require.NoError(t, err)
		env.Capture.Stages = raw
	}
	s, end := trace.Begin(env)
	t.Cleanup(end)
	return s
}

func entry(t *testing.T, s *trace.Session, stage parity.StageID) parity.StageEntry {
	t.Helper()
	snap := s.Snapshot()
	require.Equal(t, parity.StageCaptured, snap.StageStatus[stage], "stage %s status", stage)
	e, ok := snap.Stages[stage][parity.UnkeyedStage]
	require.True(t, ok, "stage %s has no unkeyed entry", stage)
	return e
}

func newWrapper(t *testing.T, raw string) *openrtb_ext.RequestWrapper {
	t.Helper()
	req := &openrtb2.BidRequest{}
	require.NoError(t, jsonutil.UnmarshalValid([]byte(raw), req))
	return &openrtb_ext.RequestWrapper{BidRequest: req}
}

type podError struct {
	PodId    int
	PodIndex int
	ErrMsgs  []string
}

type ampResponse struct {
	Targeting map[string]string `json:"targeting"`
}

// hookArgs holds the values the handlers pass to the hooks, built up front so
// the allocation test measures only the hooks.
type hookArgs struct {
	body     []byte
	stored   map[string]json.RawMessage
	errs     []error
	err      error
	reject   *hookexecution.RejectError
	rw       *openrtb_ext.RequestWrapper
	resp     *openrtb2.BidResponse
	final    any
	amp      ampResponse
	pods     []podError
	debugLog *exchange.DebugLog
	tcf2     gdpr.TCF2ConfigReader
	executor hookexecution.HookStageExecutor
}

// callAllHooks calls every hook once.
func callAllHooks(a *hookArgs) {
	Entrypoint(a.body, a.reject)
	RawAuction(a.body, a.reject)
	StoredFetch("sr", true, a.stored, a.stored, a.errs)
	StoredRequestFetch("sr", a.stored["sr"], a.errs)
	Account("acct", nil, a.errs)
	Merged(a.body)
	MergedRequest(a.rw.BidRequest, a.errs)
	MergeError(a.err)
	MergeErrors(a.errs)
	Normalized(a.rw)
	NormalizeError(a.err)
	NormalizeErrors(a.errs)
	StoredResponses(a.stored, nil, nil, a.errs)
	Validated(a.rw, a.errs)
	GDPR(a.tcf2, gdpr.SignalYes, true, metrics.ReqTypeORTB2Web, a.errs)
	AuctionResult(a.err)
	SeatNonBidResult(a.err)
	AuctionResponse(a.resp)
	AMPTargeting(a.amp)
	Exitpoint(a.final)
	ResponseEncodeError(a.err)
	HookOutcomes(a.executor)
	VideoPods(a.pods, nil, a.err)
	VideoCache(a.debugLog, a.err)
}

// TestHooksDisabledDoNotAllocate checks the disabled path: with no session in
// flight every hook is a no-op that allocates nothing and changes nothing.
func TestHooksDisabledDoNotAllocate(t *testing.T) {
	require.Nil(t, trace.Current())
	resp := &openrtb2.BidResponse{ID: "r"}
	a := &hookArgs{
		body:     []byte(`{"id":"r"}`),
		stored:   map[string]json.RawMessage{"sr": json.RawMessage(`{"id":"s"}`)},
		errs:     []error{&errortypes.Warning{Message: "w"}},
		err:      &errortypes.BadInput{Message: "bad"},
		reject:   &hookexecution.RejectError{NBR: 1},
		rw:       newWrapper(t, `{"id":"r","imp":[{"id":"i"}]}`),
		resp:     resp,
		final:    resp,
		amp:      ampResponse{Targeting: map[string]string{"k": "v"}},
		pods:     []podError{{PodId: 1}},
		debugLog: &exchange.DebugLog{CacheKey: "k"},
		tcf2:     fakeTCF2{},
		executor: hookexecution.EmptyHookExecutor{},
	}
	before := marshal(t, a.rw.BidRequest)

	allocs := testing.AllocsPerRun(100, func() { callAllHooks(a) })

	assert.Zero(t, allocs, "disabled hooks allocate")
	assert.Equal(t, before, marshal(t, a.rw.BidRequest))
}

// TestHooksRecordEveryStage runs every hook in one session: each endpoint
// stage this package owns is captured.
func TestHooksRecordEveryStage(t *testing.T) {
	s := begin(t, nil)
	resp := &openrtb2.BidResponse{ID: "r"}
	callAllHooks(&hookArgs{
		body: []byte(`{"id":"r"}`), rw: newWrapper(t, `{"id":"r","imp":[]}`), resp: resp, final: resp,
		pods: []podError{}, debugLog: &exchange.DebugLog{}, tcf2: fakeTCF2{}, executor: hookexecution.EmptyHookExecutor{},
	})
	status := s.Snapshot().StageStatus
	for _, stage := range []parity.StageID{
		stageEntrypoint, stageStoredFetch, stageAccount, stageRawAuction, stageMerged, stageNormalized,
		stageStoredResponses, stageValidated, stageGDPR, stageExchangeResponse, stageSeatNonBids,
		stageAuctionResponse, stageAMPTargeting, stageExitpoint, stageHookOutcomes, stageVideoPods, stageVideoCache,
	} {
		assert.Equal(t, parity.StageCaptured, status[stage], stage)
	}
}

func TestHooksRecordStages(t *testing.T) {
	warning := &errortypes.Warning{Message: "careful"}
	fatal := &errortypes.BadInput{Message: "bad"}
	reject := &hookexecution.RejectError{NBR: 123, Stage: "entrypoint"}
	tests := []struct {
		name         string
		run          func()
		stage        parity.StageID
		wantJSON     string
		wantRaw      *string
		wantErrors   []string
		wantWarnings []string
	}{
		{
			name:     "entrypoint body is recorded exactly",
			run:      func() { Entrypoint([]byte(`{"id": "r"}`), nil) },
			stage:    parity.StageHooksEntrypoint,
			wantJSON: `{"id":"r"}`,
			wantRaw:  ptrutil.ToPtr(`{"id": "r"}`),
		},
		{
			name:         "entrypoint reject has warning severity",
			run:          func() { Entrypoint(nil, reject) },
			stage:        parity.StageHooksEntrypoint,
			wantJSON:     `null`,
			wantWarnings: []string{reject.Error()},
		},
		{
			name:     "raw auction body",
			run:      func() { RawAuction([]byte(`{"id":"after-hook"}`), nil) },
			stage:    parity.StageHooksRawAuction,
			wantJSON: `{"id":"after-hook"}`,
		},
		{
			name: "stored fetch keeps unparsable stored data",
			run: func() {
				StoredFetch("sr", true, map[string]json.RawMessage{"sr": json.RawMessage(`{"id":"s"}`)},
					map[string]json.RawMessage{"imp": json.RawMessage(`not json`)}, []error{fatal})
			},
			stage: parity.StageStoredFetch,
			wantJSON: `{"stored_request_id":"sr","has_stored_request":true,
				"requests":{"sr":{"text":"{\"id\":\"s\"}","json":{"id":"s"}}},
				"imps":{"imp":{"text":"not json"}}}`,
			wantErrors: []string{"bad"},
		},
		{
			name:     "stored fetch without stored data",
			run:      func() { StoredFetch("", false, nil, nil, nil) },
			stage:    parity.StageStoredFetch,
			wantJSON: `{"stored_request_id":"","has_stored_request":false,"requests":{},"imps":{}}`,
		},
		{
			name:       "video stored request fetch that found nothing",
			run:        func() { StoredRequestFetch("vid", nil, []error{fatal}) },
			stage:      parity.StageStoredFetch,
			wantJSON:   `{"stored_request_id":"vid","has_stored_request":true,"requests":{},"imps":{}}`,
			wantErrors: []string{"bad"},
		},
		{
			name:     "account as Go marshals it",
			run:      func() { Account("acct", &config.Account{ID: "acct", Disabled: true}, nil) },
			stage:    parity.StageAccountResolved,
			wantJSON: accountJSON(t, "acct", &config.Account{ID: "acct", Disabled: true}),
		},
		{
			name:       "account resolution failure",
			run:        func() { Account("", nil, []error{fatal}) },
			stage:      parity.StageAccountResolved,
			wantJSON:   `{"id":"","account":null}`,
			wantErrors: []string{"bad"},
		},
		{
			name:     "merged request bytes",
			run:      func() { Merged([]byte(`{"id":"merged"}`)) },
			stage:    parity.StageRequestMerged,
			wantJSON: `{"id":"merged"}`,
		},
		{
			name:         "merged AMP request and its errors",
			run:          func() { MergedRequest(&openrtb2.BidRequest{ID: "amp"}, []error{warning}) },
			stage:        parity.StageRequestMerged,
			wantJSON:     `{"id":"amp","imp":null}`,
			wantWarnings: []string{"careful"},
		},
		{
			name:       "merge failure keeps json absent",
			run:        func() { MergeErrors([]error{fatal}); MergeError(nil) },
			stage:      parity.StageRequestMerged,
			wantErrors: []string{"bad"},
		},
		{
			name:         "normalized request with step warnings",
			run:          func() { NormalizeErrors([]error{warning}); Normalized(newWrapper(t, `{"id":"n","imp":[]}`)) },
			stage:        parity.StageRequestNormalized,
			wantJSON:     `{"id":"n","imp":[]}`,
			wantWarnings: []string{"careful"},
		},
		{
			name:       "failing normalization step",
			run:        func() { NormalizeError(fatal) },
			stage:      parity.StageRequestNormalized,
			wantErrors: []string{"bad"},
		},
		{
			name: "stored responses",
			run: func() {
				StoredResponses(
					stored_responses.ImpsWithBidResponses{"imp1": json.RawMessage(`{"id":"resp"}`)},
					stored_responses.ImpBidderStoredResp{"imp2": {"appnexus": json.RawMessage(`{"id":"bid"}`)}},
					stored_responses.BidderImpReplaceImpID{"appnexus": {"imp2": true}},
					nil)
			},
			stage: parity.StageStoredResponses,
			wantJSON: `{"auction":{"imp1":{"text":"{\"id\":\"resp\"}","json":{"id":"resp"}}},
				"bid":{"imp2":{"appnexus":{"text":"{\"id\":\"bid\"}","json":{"id":"bid"}}}},
				"replace_impid":{"appnexus":{"imp2":true}}}`,
		},
		{
			name:         "validated request with errors and warnings",
			run:          func() { Validated(newWrapper(t, `{"id":"v","imp":[]}`), []error{fatal, warning}) },
			stage:        parity.StageRequestValidated,
			wantJSON:     `{"id":"v","imp":[]}`,
			wantErrors:   []string{"bad"},
			wantWarnings: []string{"careful"},
		},
		{
			name:     "gdpr without a TCF2 reader",
			run:      func() { GDPR(nil, gdpr.SignalAmbiguous, false, metrics.ReqTypeAMP, nil) },
			stage:    parity.StageGDPRContext,
			wantJSON: `{"signal":-1,"enforced":false,"channel":"amp","tcf2":null}`,
		},
		{
			name:       "auction result error only",
			run:        func() { AuctionResult(fatal) },
			stage:      parity.StageExchangeResponse,
			wantErrors: []string{"bad"},
		},
		{
			name:  "seat non bid result without error",
			run:   func() { SeatNonBidResult(nil) },
			stage: parity.StageSeatNonBids,
		},
		{
			name:     "auction response",
			run:      func() { AuctionResponse(&openrtb2.BidResponse{ID: "resp"}) },
			stage:    parity.StageHooksAuctionResponse,
			wantJSON: `{"id":"resp"}`,
		},
		{
			name:     "amp targeting response",
			run:      func() { AMPTargeting(ampResponse{Targeting: map[string]string{"hb_pb": "1.00"}}) },
			stage:    parity.StageAMPTargetingResponse,
			wantJSON: `{"targeting":{"hb_pb":"1.00"}}`,
		},
		{
			name:       "exitpoint response and encode error",
			run:        func() { Exitpoint(map[string]int{"a": 1}); ResponseEncodeError(fatal); ResponseEncodeError(nil) },
			stage:      parity.StageHooksExitpoint,
			wantJSON:   `{"a":1}`,
			wantErrors: []string{"bad"},
		},
		{
			name:     "hook outcomes of the empty executor",
			run:      func() { HookOutcomes(hookexecution.EmptyHookExecutor{}) },
			stage:    parity.StageHookOutcomes,
			wantJSON: `[]`,
		},
		{
			name: "video pods",
			run: func() {
				VideoPods([]podError{{PodId: 1, PodIndex: 0, ErrMsgs: []string{"m"}}}, &openrtb_ext.BidResponseVideo{AdPods: []*openrtb_ext.AdPod{}}, nil)
			},
			stage:    parity.StageVideoPods,
			wantJSON: `{"pod_errors":[{"PodId":1,"PodIndex":0,"ErrMsgs":["m"]}],"response":{"adPods":[]}}`,
		},
		{
			name:       "all video pods incorrect",
			run:        func() { VideoPods[podError](nil, nil, fatal) },
			stage:      parity.StageVideoPods,
			wantJSON:   `{"pod_errors":[],"response":null}`,
			wantErrors: []string{"bad"},
		},
		{
			name: "video cache",
			run: func() {
				VideoCache(&exchange.DebugLog{CacheKey: "k", CacheType: "xml", TTL: 3600, CacheString: "<log/>"}, nil)
			},
			stage:    parity.StageVideoCache,
			wantJSON: `{"cache_key":"k","cache_type":"xml","ttl":3600,"data":"<log/>"}`,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := begin(t, nil)
			tt.run()
			e := entry(t, s, tt.stage)
			if tt.wantJSON == "" {
				assert.Nil(t, e.JSON)
			} else {
				assert.JSONEq(t, tt.wantJSON, string(e.JSON))
			}
			if tt.wantRaw != nil {
				require.NotNil(t, e.Raw)
				assert.Equal(t, *tt.wantRaw, *e.Raw)
			}
			assert.Equal(t, nonNil(tt.wantErrors), messages(e.Errors), "errors")
			assert.Equal(t, nonNil(tt.wantWarnings), messages(e.Warnings), "warnings")
		})
	}
}

func accountJSON(t *testing.T, id string, account *config.Account) string {
	t.Helper()
	dump, err := accountDump(account)
	require.NoError(t, err)
	raw, err := jsonutil.Marshal(accountPayload{ID: id, Account: dump})
	require.NoError(t, err)
	return string(raw)
}

// TestAccountDumpUsesMapstructureKeys checks that the account is dumped under
// its config keys (mapstructure tags), not Go field names or json tags.
func TestAccountDumpUsesMapstructureKeys(t *testing.T) {
	account := &config.Account{
		ID:       "acct",
		Disabled: true,
		CacheTTL: config.DefaultTTLs{Banner: 300},
		Privacy:  config.AccountPrivacy{PrivacySandbox: config.PrivacySandbox{TopicsDomain: "topics.example"}},
	}
	dump, err := accountDump(account)
	require.NoError(t, err)
	raw := marshal(t, dump)

	tests := []struct {
		name string
		path []string
		want any
	}{
		{name: "top-level key", path: []string{"disabled"}, want: true},
		{name: "nested key without json tag", path: []string{"cache_ttl", "banner"}, want: float64(300)},
		{name: "deeply nested key without json tag", path: []string{"privacy", "privacysandbox", "topicsdomain"}, want: "topics.example"},
	}
	var doc map[string]any
	require.NoError(t, json.Unmarshal([]byte(raw), &doc))
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var cur any = doc
			for _, k := range tt.path {
				m, ok := cur.(map[string]any)
				require.True(t, ok, "%v is not an object at %s", cur, k)
				cur = m[k]
			}
			assert.Equal(t, tt.want, cur)
		})
	}

	none, err := accountDump(nil)
	assert.NoError(t, err)
	assert.Nil(t, none)
}

func messages(records []parity.ErrorRecord) []string {
	out := make([]string, 0, len(records))
	for _, r := range records {
		out = append(out, r.Message)
	}
	return out
}

func marshal(t *testing.T, v any) string {
	t.Helper()
	raw, err := jsonutil.Marshal(v)
	require.NoError(t, err)
	return string(raw)
}

func TestStageFilterSkipsUncapturedStages(t *testing.T) {
	s := begin(t, []parity.StageID{parity.StageRequestMerged})
	rw := newWrapper(t, `{"id":"r","imp":[]}`)
	Entrypoint([]byte(`{}`), nil)
	Validated(rw, nil)
	HookOutcomes(hookexecution.EmptyHookExecutor{})
	Merged([]byte(`{"id":"r"}`))

	snap := s.Snapshot()
	assert.Equal(t, parity.StageCaptured, snap.StageStatus[parity.StageRequestMerged])
	for _, stage := range []parity.StageID{parity.StageHooksEntrypoint, parity.StageRequestValidated, parity.StageHookOutcomes} {
		assert.Equal(t, parity.StageNotReached, snap.StageStatus[stage], stage)
	}
}

// TestRequestSnapshotsHaveNoObserverEffect checks that Normalized and
// Validated record the request as Go's later rebuild produces it while leaving
// the live wrapper, its BidRequest and its dirty ext state untouched.
func TestRequestSnapshotsHaveNoObserverEffect(t *testing.T) {
	tests := []struct {
		name   string
		record func(rw *openrtb_ext.RequestWrapper)
		stage  parity.StageID
	}{
		{name: "normalized", record: Normalized, stage: parity.StageRequestNormalized},
		{name: "validated", record: func(rw *openrtb_ext.RequestWrapper) { Validated(rw, nil) }, stage: parity.StageRequestValidated},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := begin(t, nil)
			rw := newWrapper(t, `{"id":"r","imp":[{"id":"i","ext":{"tid":"a"}}],"ext":{"prebid":{"debug":true}}}`)
			reqExt, err := rw.GetRequestExt()
			require.NoError(t, err)
			reqExt.SetPrebid(&openrtb_ext.ExtRequestPrebid{Integration: "web"})
			impExt, err := rw.GetImp()[0].GetImpExt()
			require.NoError(t, err)
			impExt.SetTid("b")
			before := marshal(t, rw.BidRequest)

			tt.record(rw)

			assert.Equal(t, before, marshal(t, rw.BidRequest), "live BidRequest changed")
			require.NoError(t, rw.RebuildRequest())
			assert.JSONEq(t, marshal(t, rw.BidRequest), string(entry(t, s, tt.stage).JSON))
		})
	}
}

func TestRequestSnapshotFailureIsNoted(t *testing.T) {
	s := begin(t, nil)
	Normalized(nil)
	e := entry(t, s, parity.StageRequestNormalized)
	assert.JSONEq(t, `null`, string(e.JSON))
	assert.Contains(t, s.Snapshot().Notes, "request.normalized: request snapshot failed: shadow: request wrapper has no BidRequest")
}

// fakeTCF2 is a TCF2ConfigReader with per-purpose answers.
type fakeTCF2 struct {
	purposes map[consentconstants.Purpose]purposePayload
}

func (f fakeTCF2) BasicEnforcementVendors() map[string]struct{} {
	return map[string]struct{}{"b": {}, "a": {}}
}
func (f fakeTCF2) FeatureOneEnforced() bool                              { return true }
func (f fakeTCF2) FeatureOneVendorException(openrtb_ext.BidderName) bool { return false }
func (f fakeTCF2) IsEnabled() bool                                       { return true }
func (f fakeTCF2) PurposeOneTreatmentEnabled() bool                      { return true }
func (f fakeTCF2) PurposeOneTreatmentAccessAllowed() bool                { return false }

func (f fakeTCF2) ChannelEnabled(channel config.ChannelType) bool {
	return channel == config.ChannelWeb
}

func (f fakeTCF2) PurposeEnforced(p consentconstants.Purpose) bool {
	return f.purposes[p].Enforced
}

func (f fakeTCF2) PurposeEnforcingVendors(p consentconstants.Purpose) bool {
	return f.purposes[p].EnforceVendors
}

func (f fakeTCF2) PurposeEnforcementAlgo(p consentconstants.Purpose) config.TCF2EnforcementAlgo {
	switch f.purposes[p].Algo {
	case "basic":
		return config.TCF2BasicEnforcement
	case "full":
		return config.TCF2FullEnforcement
	default:
		return config.TCF2UndefinedEnforcement
	}
}
func (f fakeTCF2) PurposeVendorExceptions(p consentconstants.Purpose) map[string]struct{} {
	set := map[string]struct{}{}
	for _, v := range f.purposes[p].VendorExceptions {
		set[v] = struct{}{}
	}
	return set
}

func TestGDPRPayload(t *testing.T) {
	tcf2 := fakeTCF2{purposes: map[consentconstants.Purpose]purposePayload{
		1:  {Enforced: true, Algo: "full", EnforceVendors: true, VendorExceptions: []string{"z", "y"}},
		2:  {Enforced: true, Algo: "basic"},
		10: {EnforceVendors: true},
	}}
	got := newGDPRPayload(tcf2, gdpr.SignalYes, true, config.ChannelWeb)

	assert.Equal(t, 1, got.Signal)
	assert.True(t, got.Enforced)
	assert.Equal(t, "web", got.Channel)
	require.NotNil(t, got.TCF2)
	assert.True(t, got.TCF2.Enabled)
	assert.True(t, got.TCF2.ChannelEnabled)
	assert.True(t, got.TCF2.FeatureOneEnforced)
	assert.Equal(t, purposeOneTreatment{Enabled: true, AccessAllowed: false}, got.TCF2.PurposeOneTreatment)
	assert.Equal(t, []string{"a", "b"}, got.TCF2.BasicEnforcementVendors)
	require.Len(t, got.TCF2.Purposes, tcf2Purposes)
	tests := []struct {
		purpose string
		want    purposePayload
	}{
		{purpose: "1", want: purposePayload{Enforced: true, Algo: "full", EnforceVendors: true, VendorExceptions: []string{"y", "z"}}},
		{purpose: "2", want: purposePayload{Enforced: true, Algo: "basic", VendorExceptions: []string{}}},
		{purpose: "5", want: purposePayload{VendorExceptions: []string{}}},
		{purpose: "10", want: purposePayload{EnforceVendors: true, VendorExceptions: []string{}}},
	}
	for _, tt := range tests {
		t.Run("purpose "+tt.purpose, func(t *testing.T) {
			assert.Equal(t, tt.want, got.TCF2.Purposes[tt.purpose])
		})
	}
}

func TestGDPRPayloadFromHostConfig(t *testing.T) {
	host := config.TCF2{Enabled: true, Purpose1: config.TCF2Purpose{EnforcePurpose: true, EnforceAlgoID: config.TCF2FullEnforcement, EnforceVendors: true}}
	host.PurposeConfigs = map[consentconstants.Purpose]*config.TCF2Purpose{1: &host.Purpose1}
	for p := consentconstants.Purpose(2); p <= tcf2Purposes; p++ {
		host.PurposeConfigs[p] = &config.TCF2Purpose{}
	}
	got := newGDPRPayload(gdpr.NewTCF2Config(host, config.AccountGDPR{}), gdpr.SignalNo, false, config.ChannelApp)
	require.NotNil(t, got.TCF2)
	assert.True(t, got.TCF2.ChannelEnabled)
	assert.Equal(t, purposePayload{Enforced: true, Algo: "full", EnforceVendors: true, VendorExceptions: []string{}}, got.TCF2.Purposes["1"])
}

func TestHookOutcomesProjection(t *testing.T) {
	outcomes := []hookexecution.StageOutcome{{
		Stage:         "entrypoint",
		Entity:        "http-request",
		ExecutionTime: hookexecution.ExecutionTime{ExecutionTimeMillis: 5},
		Groups: []hookexecution.GroupOutcome{{
			ExecutionTime: hookexecution.ExecutionTime{ExecutionTimeMillis: 4},
			InvocationResults: []hookexecution.HookOutcome{{
				HookID:        hookexecution.HookID{ModuleCode: "mod", HookImplCode: "impl"},
				Status:        hookexecution.StatusSuccess,
				Action:        hookexecution.ActionUpdate,
				Message:       "msg",
				Errors:        []string{"e1"},
				AnalyticsTags: hookanalytics.Analytics{},
			}, {
				HookID:   hookexecution.HookID{ModuleCode: "mod", HookImplCode: "other"},
				Status:   hookexecution.StatusFailure,
				Action:   hookexecution.ActionNone,
				Warnings: []string{"w1"},
			}},
		}},
	}}
	raw, err := jsonutil.Marshal(newHookOutcomes(outcomes))
	require.NoError(t, err)
	assert.JSONEq(t, `[{"stage":"entrypoint","execution_time_millis":5,"entity":"http-request","groups":[{
		"execution_time_millis":4,"invocation_results":[
		{"analytics_tags":{},"hook_id":{"module_code":"mod","hook_impl_code":"impl"},"status":"success","action":"update","message":"msg","errors":["e1"],"warnings":[]},
		{"analytics_tags":{},"hook_id":{"module_code":"mod","hook_impl_code":"other"},"status":"failure","action":"no_action","message":"","errors":[],"warnings":["w1"]}
	]}]}]`, string(raw))
}

func TestErrorList(t *testing.T) {
	err := errors.New("x")
	tests := []struct {
		name string
		in   error
		want []error
	}{
		{name: "nil", in: nil, want: nil},
		{name: "one", in: err, want: []error{err}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Equal(t, tt.want, errorList(tt.in))
		})
	}
}
