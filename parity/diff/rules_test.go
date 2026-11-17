package diff

import (
	"encoding/json"
	"sort"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

func loadRegistry(t *testing.T) schema.NormalizationRegistry {
	t.Helper()
	reg, err := schema.Normalization()
	if err != nil {
		t.Fatalf("schema.Normalization: %v", err)
	}
	return reg
}

// TestAllowlistMatchesImplementations: every rule in normalization.json is
// implemented and every implementation is allowlisted, so neither can drift.
func TestAllowlistMatchesImplementations(t *testing.T) {
	ids := mustRules(t).IDs()
	sort.Strings(ids)
	if !equalStrings(ids, ImplementedRules()) {
		t.Fatalf("allowlist rules %v, implemented %v", ids, ImplementedRules())
	}
}

// TestNotNormalizedClassesAreKnown: every lexical and wire class the differ
// can emit is documented as never normalized in normalization.json.
func TestNotNormalizedClassesAreKnown(t *testing.T) {
	reg := loadRegistry(t)
	for _, tc := range []struct {
		policy  schema.ClassPolicy
		kind    parity.MismatchKind
		classes []string
	}{
		{reg.NotNormalized.Lexical, parity.KindLexical, []string{
			parity.LexicalHTMLEscape, parity.LexicalUnicodeEscape, parity.LexicalNumberFormat,
			parity.LexicalTrailingNewline, parity.LexicalWhitespace, parity.LexicalMemberOrder, parity.LexicalOther,
		}},
		{reg.NotNormalized.Wire, parity.KindWire, []string{
			parity.WireHeaderCase, parity.WireHeaderOrder, parity.WireDefaultHeader, parity.WireHTTPVersion,
			parity.WireContentLength, parity.WireTransferEncoding, parity.WireOther,
		}},
	} {
		if tc.policy.Kind != tc.kind {
			t.Errorf("not_normalized kind %q, want %q", tc.policy.Kind, tc.kind)
		}
		for _, class := range tc.classes {
			if _, ok := tc.policy.Classes[class]; !ok {
				t.Errorf("%s class %q is not documented in not_normalized", tc.kind, class)
			}
		}
	}
}

// TestNewRulesRejects: a rule the differ cannot honour exactly is a load error.
func TestNewRulesRejects(t *testing.T) {
	tests := []struct {
		name    string
		rule    string
		mutate  func(*schema.NormalizationRule)
		wantErr string
	}{
		{"unknown rule", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.ID = "made_up" }, "no implementation"},
		{"no go_source", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.GoSource = nil }, "Go nondeterminism source"},
		{"blank go_source", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.GoSource = []string{" "} }, "empty go_source"},
		{"no justification", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.Justification = " " }, "justification"},
		{"other action", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.Action = actionDrop }, "action"},
		{"other keys", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.Keys = []string{"seat"} }, "keys"},
		{"other drop", RuleHTTPOutOrder, func(r *schema.NormalizationRule) { r.Drop = nil }, "drop"},
		{"target lacks selector", RuleTIDUUID, func(r *schema.NormalizationRule) { r.Target = "$.source.tid" }, "does not mention"},
		{"unregistered stage", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.AppliesTo = []string{"http.nowhere"} }, "unregistered stage"},
		{"no stages", RuleSeatbidOrder, func(r *schema.NormalizationRule) { r.AppliesTo = nil }, "applies_to is empty"},
		{"bad shape type", RuleAuctionTimestamp, func(r *schema.NormalizationRule) { r.Shape = map[string]any{"type": "object"} }, "shape type"},
		{"bad shape pattern", RuleEventsTSParam, func(r *schema.NormalizationRule) {
			r.Shape = map[string]any{"type": "string", "pattern": "("}
		}, "shape pattern"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			reg := loadRegistry(t)
			reg.Rules = append([]schema.NormalizationRule(nil), reg.Rules...)
			found := false
			for i := range reg.Rules {
				if reg.Rules[i].ID == tt.rule {
					tt.mutate(&reg.Rules[i])
					found = true
				}
			}
			if !found {
				t.Fatalf("rule %s not in the allowlist", tt.rule)
			}
			_, err := NewRules(reg)
			if err == nil || !strings.Contains(err.Error(), tt.wantErr) {
				t.Fatalf("NewRules error = %v, want it to contain %q", err, tt.wantErr)
			}
		})
	}
}

func TestNewRulesRejectsDocument(t *testing.T) {
	reg := loadRegistry(t)
	reg.Schema = "pbs-parity-stages/1"
	if _, err := NewRules(reg); err == nil {
		t.Fatal("accepted a document of another schema")
	}
	reg = loadRegistry(t)
	reg.Rules = append(append([]schema.NormalizationRule(nil), reg.Rules...), reg.Rules[0])
	if _, err := NewRules(reg); err == nil || !strings.Contains(err.Error(), "listed twice") {
		t.Fatalf("duplicate rule: err = %v", err)
	}
}

// engineWithout returns an engine whose allowlist lacks one rule.
func engineWithout(t *testing.T, id string) *Engine {
	t.Helper()
	reg := loadRegistry(t)
	kept := reg.Rules[:0:0]
	for _, r := range reg.Rules {
		if r.ID != id {
			kept = append(kept, r)
		}
	}
	if len(kept) == len(reg.Rules) {
		t.Fatalf("rule %s not in the allowlist", id)
	}
	reg.Rules = kept
	rules, err := NewRules(reg)
	if err != nil {
		t.Fatalf("NewRules: %v", err)
	}
	stages, err := schema.Stages()
	if err != nil {
		t.Fatalf("schema.Stages: %v", err)
	}
	e, err := NewEngine(stages, rules)
	if err != nil {
		t.Fatalf("NewEngine: %v", err)
	}
	return e
}

// ruleCase is a pair of traces that differ only in what one rule normalizes.
type ruleCase func() (Case, *parity.Trace, *parity.Trace)

func canaryNoise() (Case, *parity.Trace, *parity.Trace) {
	return Case{CaseID: "noise", Envelope: canaryEnvelope()}, canaryTrace(goSide), canaryTrace(rustSide)
}

func entryPair(stage parity.StageID, key, goRaw, rustRaw string) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		put(g, stage, key, goRaw)
		put(r, stage, key, rustRaw)
		return Case{CaseID: "rule"}, g, r
	}
}

func responsePair(goBody, rustBody string, goHeaders, rustHeaders [][2]string) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		g.HTTPResponse = textResponse(200, goBody, goHeaders...)
		r.HTTPResponse = textResponse(200, rustBody, rustHeaders...)
		return Case{CaseID: "rule"}, g, r
	}
}

func adapterBidsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	first := `{"request_data":{"uri":"https://a.example/1","body":{"text":"{}"}},"bidder_response":{"currency":"USD","bids":[]}}`
	second := `{"request_data":{"uri":"https://a.example/2","body":{"text":"{}"}},"bidder_response":{"currency":"EUR","bids":[]}}`
	put(g, parity.StageAdapterBids, "appnexus#0", first)
	put(g, parity.StageAdapterBids, "appnexus#1", second)
	put(r, parity.StageAdapterBids, "appnexus#0", second)
	put(r, parity.StageAdapterBids, "appnexus#1", first)
	return Case{CaseID: "rule"}, g, r
}

func metricsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	one := int64(1)
	g.MetricsDelta = []parity.MetricDelta{{Name: "pbs_adapter_request_time_seconds", Type: parity.MetricHistogram,
		Labels: map[string]string{"adapter": "appnexus"}, Delta: 0.01, CountDelta: &one, BucketDeltas: map[string]int64{"0.05": 1}}}
	r.MetricsDelta = []parity.MetricDelta{{Name: "pbs_adapter_request_time_seconds", Type: parity.MetricHistogram,
		Labels: map[string]string{"adapter": "appnexus"}, Delta: 0.3, CountDelta: &one, BucketDeltas: map[string]int64{"0.5": 1}}}
	return Case{CaseID: "rule"}, g, r
}

// runtimeMetricsPair differs only in Go runtime collector series, the way two
// Go processes do after the same case.
func runtimeMetricsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	one := int64(1)
	requests := parity.MetricDelta{Name: "requests", Type: parity.MetricCounter, Labels: map[string]string{"request_type": "openrtb2-web"}, Delta: 1}
	g.MetricsDelta = []parity.MetricDelta{
		{Name: "go_memstats_alloc_bytes", Type: parity.MetricGauge, Labels: map[string]string{}, Delta: 1024},
		{Name: "pbs_go_gc_pauses_seconds_total", Type: parity.MetricHistogram, Labels: map[string]string{}, CountDelta: &one, BucketDeltas: map[string]int64{"+Inf": 1}},
		requests,
	}
	r.MetricsDelta = []parity.MetricDelta{
		{Name: "go_memstats_alloc_bytes", Type: parity.MetricGauge, Labels: map[string]string{}, Delta: 4096},
		requests,
	}
	return Case{CaseID: "rule"}, g, r
}

// validatedWarningsPair holds request.validated entries whose disabled-bidder
// warnings come in another order, as ranging over imp.ext.prebid.bidder gives.
func validatedWarningsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	req := `{"id":"r1","imp":[{"id":"i1"}]}`
	put(g, parity.StageRequestValidated, parity.UnkeyedStage, req)
	put(r, parity.StageRequestValidated, parity.UnkeyedStage, req)
	a := parity.ErrorRecord{Code: 6, Kind: "BidderTemporarilyDisabled", Message: `Bidder "appnexus" has been disabled`, Scope: "general", Severity: "warning"}
	b := parity.ErrorRecord{Code: 6, Kind: "BidderTemporarilyDisabled", Message: `Bidder "rubicon" has been disabled`, Scope: "general", Severity: "warning"}
	ge, re := g.Stages[parity.StageRequestValidated][parity.UnkeyedStage], r.Stages[parity.StageRequestValidated][parity.UnkeyedStage]
	ge.Warnings, re.Warnings = []parity.ErrorRecord{a, b}, []parity.ErrorRecord{b, a}
	g.Stages[parity.StageRequestValidated][parity.UnkeyedStage] = ge
	r.Stages[parity.StageRequestValidated][parity.UnkeyedStage] = re
	return Case{CaseID: "rule"}, g, r
}

// cachePutsPair holds one Prebid Cache call whose puts arrive in another
// order, with the content-addressed UUIDs the oracle answers in put order.
func cachePutsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	putA, putB := `{"type":"json","value":{"id":"a","price":1}}`, `{"type":"json","value":{"id":"b","price":2}}`
	respA, respB := `{"uuid":"`+uuidA+`"}`, `{"uuid":"`+uuidB+`"}`
	mk := func(puts, resps string) []parity.OutboundCall {
		return []parity.OutboundCall{{Seq: 1, Kind: parity.OutboundCache, OrigURL: "https://cache.example/cache",
			Request:  parity.OutboundRequest{Method: "POST", URL: "http://cache.example/cache", Header: parity.HeaderList{}, Body: parity.TextBody(`{"puts":[` + puts + `]}`)},
			Mock:     &parity.MockRef{Source: "rule", ID: "cache"},
			Response: &parity.OutboundResponse{Status: 200, Headers: parity.HeaderList{}, Body: parity.TextBody(`{"responses":[` + resps + `]}`)}}}
	}
	g.HTTPOut = mk(putA+","+putB, respA+","+respB)
	r.HTTPOut = mk(putB+","+putA, respB+","+respA)
	put(g, parity.StageCacheResult, parity.UnkeyedStage, `{"puts":[{"kind":"bid","bid_id":"a","id":"`+uuidA+`","assigned":"`+uuidA+`"},{"kind":"bid","bid_id":"b","id":"`+uuidB+`","assigned":"`+uuidB+`"}]}`)
	put(r, parity.StageCacheResult, parity.UnkeyedStage, `{"puts":[{"kind":"bid","bid_id":"a","id":"`+uuidA+`","assigned":"`+uuidA+`"},{"kind":"bid","bid_id":"b","id":"`+uuidB+`","assigned":"`+uuidB+`"}]}`)
	return Case{CaseID: "rule"}, g, r
}

func outHeadersPair(goName, rustName string) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		mk := func(name string) []parity.OutboundCall {
			return []parity.OutboundCall{{Seq: 1, Kind: parity.OutboundBidder, Bidder: "appnexus", OrigURL: "https://a.example",
				Request: parity.OutboundRequest{Method: "POST", URL: "https://a.example", Header: parity.HeaderList{{name, "1"}}},
				Mock:    &parity.MockRef{Source: "inline", ID: "x"}, Response: &parity.OutboundResponse{Status: 204}}}
		}
		g.HTTPOut, r.HTTPOut = mk(goName), mk(rustName)
		return Case{CaseID: "rule"}, g, r
	}
}

func analyticsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	g.Analytics = []parity.AnalyticsRecord{{Kind: "auction", Object: json.RawMessage(`{"start_time_set":true,"status":200}`)}}
	r.Analytics = []parity.AnalyticsRecord{{Kind: "auction", Object: json.RawMessage(`{"start_time_set":false,"status":200}`)}}
	return Case{CaseID: "rule"}, g, r
}

const (
	uuidA = "2e8c1f6a-4b3d-4c2e-9f10-0a1b2c3d4e5f"
	uuidB = "7d9e0a1b-2c3d-4e5f-8a6b-7c8d9e0f1a2b"
	// uuidV1 is a version-1 UUID: a cache may return any UUID version.
	uuidV1 = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
)

func eventAdm(ts string) string {
	return `{"id":"r1","cur":"USD","seatbid":[{"seat":"a","bid":[{"id":"b","impid":"i","price":1,"adm":"<img src='https://pbs.example/event?t=imp&b=b&ts=` + ts + `'>"}]}]}`
}

func hookOutcomes(ms int) string {
	return `{"stages":[{"stage":"entrypoint","outcomes":[{"groups":[{"invocation_results":[{"hook_id":{"module_code":"m"},"status":"success","execution_time_millis":` +
		strings.Repeat("1", ms) + `}]}],"execution_time_millis":` + strings.Repeat("2", ms) + `}]}]}`
}

// TestRemovingRuleReenablesDiff: each rule normalizes exactly the difference
// it cites. With the rule the pair compares equal; without it the difference
// is reported at the expected path.
func TestRemovingRuleReenablesDiff(t *testing.T) {
	tests := []struct {
		rule     string
		pair     ruleCase
		wantPath string // a path that must be reported without the rule
	}{
		{RuleSeatbidOrder, canaryNoise, "$.seatbid[0]"},
		{RuleWarningsErrorsOrder, canaryNoise, "$.ext.warnings.general"},
		{RuleHTTPOutOrder, canaryNoise, "$[0]"},
		{RuleResponseTimeMillis, canaryNoise, "$.ext.responsetimemillis.appnexus"},
		{RuleAuctionTimestamp, canaryNoise, "$.ext.prebid.auctiontimestamp"},
		{RuleTIDUUID, canaryNoise, "$.source.tid"},
		{RuleGeneratedBidID, canaryNoise, "$.seatbid[1].bid[0].ext.prebid.bidid"},
		{RuleStageEntrySeq, canaryNoise, "$['@seq']"},
		{RuleSeatNonBidOrder, entryPair(parity.StageSeatNonBids, parity.UnkeyedStage,
			`[{"seat":"b","nonbid":[{"impid":"i2","statuscode":301},{"impid":"i1","statuscode":300}]},{"seat":"a","nonbid":[]}]`,
			`[{"seat":"a","nonbid":[]},{"seat":"b","nonbid":[{"impid":"i1","statuscode":300},{"impid":"i2","statuscode":301}]}]`), "$[0]"},
		{RuleHTTPCallsOrder, responsePair(
			`{"id":"r1","ext":{"debug":{"httpcalls":{"appnexus":[{"uri":"u1","requestbody":"a","status":200},{"uri":"u2","requestbody":"b","status":200}]}}}}`,
			`{"id":"r1","ext":{"debug":{"httpcalls":{"appnexus":[{"uri":"u2","requestbody":"b","status":200},{"uri":"u1","requestbody":"a","status":200}]}}}}`,
			nil, nil), "$.ext.debug.httpcalls.appnexus"},
		{RuleAdapterBidsCallKey, adapterBidsPair, "$.request_data.uri"},
		{RuleEventsTSParam, responsePair(eventAdm("1629473330823"), eventAdm("1700000000000"), nil, nil), "$.seatbid[0].bid[0].adm"},
		{RuleAnalyticsStartTime, analyticsPair, "$[0].object.start_time_set"},
		{RuleRequestIDUUID, entryPair(parity.StageRequestValidated, parity.UnkeyedStage,
			`{"id":"`+uuidA+`","imp":[{"id":"i1"}]}`, `{"id":"`+uuidB+`","imp":[{"id":"i1"}]}`), "$.id"},
		{RuleCacheUUID, entryPair(parity.StageCacheResult, parity.UnkeyedStage,
			`{"puts":[{"kind":"bid","id":"`+uuidV1+`","assigned":"`+uuidV1+`"}]}`,
			`{"puts":[{"kind":"bid","id":"`+uuidA+`","assigned":"`+uuidA+`"}]}`), "$.puts[0].id"},
		{RuleHookExecutionTime, entryPair(parity.StageHookOutcomes, parity.UnkeyedStage, hookOutcomes(1), hookOutcomes(2)),
			"$.stages[0].outcomes[0].execution_time_millis"},
		{RuleTimingHistograms, metricsPair, "$.pbs_adapter_request_time_seconds"},
		{RuleGoRuntimeMetrics, runtimeMetricsPair, "$.go_memstats_alloc_bytes"},
		{RuleCachePutsOrder, cachePutsPair, "$[0].request.body.puts"},
		{RuleResponseTimeMillis, entryPair(parity.StageExchangeResponse, parity.UnkeyedStage,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"responsetimemillis":{"appnexus":12}}}`,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"responsetimemillis":{"appnexus":40}}}`), "$.ext_bid_response.responsetimemillis.appnexus"},
		{RuleResponseTimeMillis, entryPair(parity.StageHooksAuctionResponse, parity.UnkeyedStage,
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":12}}}`, `{"id":"r1","ext":{"responsetimemillis":{"appnexus":40}}}`),
			"$.ext.responsetimemillis.appnexus"},
		{RuleAuctionTimestamp, entryPair(parity.StageExchangeResponse, parity.UnkeyedStage,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"prebid":{"auctiontimestamp":1700000000000}}}`,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"prebid":{"auctiontimestamp":1700000000123}}}`), "$.ext_bid_response.prebid.auctiontimestamp"},
		{RuleAuctionTimestamp, entryPair(parity.StageHooksExitpoint, parity.UnkeyedStage,
			`{"id":"r1","ext":{"prebid":{"auctiontimestamp":1700000000000}}}`, `{"id":"r1","ext":{"prebid":{"auctiontimestamp":1700000000123}}}`),
			"$.ext.prebid.auctiontimestamp"},
		{RuleAuctionTimestamp, entryPair(parity.StageEventsTracking, parity.UnkeyedStage,
			`{"enabled_for_request":true,"auction_timestamp_ms":1700000000000}`, `{"enabled_for_request":true,"auction_timestamp_ms":1700000000123}`),
			"$.auction_timestamp_ms"},
		{RuleWarningsErrorsOrder, entryPair(parity.StageExchangeResponse, parity.UnkeyedStage,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"warnings":{"general":[{"code":1,"message":"a"},{"code":2,"message":"b"}]}}}`,
			`{"bid_response":{"id":"r1"},"ext_bid_response":{"warnings":{"general":[{"code":2,"message":"b"},{"code":1,"message":"a"}]}}}`),
			"$.ext_bid_response.warnings.general"},
		{RuleWarningsErrorsOrder, entryPair(parity.StageHooksAuctionResponse, parity.UnkeyedStage,
			`{"id":"r1","ext":{"warnings":{"general":[{"code":6,"message":"a"},{"code":6,"message":"b"}]}}}`,
			`{"id":"r1","ext":{"warnings":{"general":[{"code":6,"message":"b"},{"code":6,"message":"a"}]}}}`),
			"$.ext.warnings.general"},
		{RuleWarningsErrorsOrder, validatedWarningsPair, "$['@warnings']"},
		{RuleBidArrivalOrder, entryPair(parity.StageBidsAdjusted, "adtrgtme",
			`{"bids":[{"call":"adtrgtme#1","imp_id":"i2","bid_id":"b"},{"call":"adtrgtme#0","imp_id":"i1","bid_id":"b"}]}`,
			`{"bids":[{"call":"adtrgtme#0","imp_id":"i1","bid_id":"b"},{"call":"adtrgtme#1","imp_id":"i2","bid_id":"b"}]}`), "$.bids"},
		{RuleBidArrivalOrder, entryPair(parity.StageBidderResult, "adtrgtme",
			`{"seat_bids":[{"seat":"adtrgtme","bids":[{"bid":{"id":"b","impid":"i2"}},{"bid":{"id":"b","impid":"i1"}}]}]}`,
			`{"seat_bids":[{"seat":"adtrgtme","bids":[{"bid":{"id":"b","impid":"i1"}},{"bid":{"id":"b","impid":"i2"}}]}]}`), "$.seat_bids[0].bids"},
		{RuleBidArrivalOrder, entryPair(parity.StageHooksAllProcessedBidResponses, parity.UnkeyedStage,
			`{"adtrgtme":{"bids":[{"bid":{"id":"b2","impid":"i"}},{"bid":{"id":"b1","impid":"i"}}]}}`,
			`{"adtrgtme":{"bids":[{"bid":{"id":"b1","impid":"i"}},{"bid":{"id":"b2","impid":"i"}}]}}`), "$.adtrgtme.bids"},
		{RuleBidArrivalOrder, entryPair(parity.StageEventsTracking, parity.UnkeyedStage,
			`{"bids":{"adtrgtme":[{"bid_id":"b2"},{"bid_id":"b1"}]}}`,
			`{"bids":{"adtrgtme":[{"bid_id":"b1"},{"bid_id":"b2"}]}}`), "$.bids.adtrgtme"},
		{RuleHeaderNames, outHeadersPair("X-Prebid", "x-prebid"), "$[0].request.headers"},
		{RuleDateHeader, responsePair(`{"id":"r1"}`, `{"id":"r1"}`,
			[][2]string{{"Date", "Mon, 01 Jan 2024 00:00:00 GMT"}}, [][2]string{{"Date", "Tue, 02 Jan 2024 00:00:00 GMT"}}), "$['@headers'].Date"},
	}
	full := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.rule, func(t *testing.T) {
			c, g, r := tt.pair()
			if doc := compareTraces(t, full, c, g, r); len(doc.Mismatches) != 0 {
				t.Fatalf("with the rule: %v", ids(doc))
			}
			c, g, r = tt.pair()
			doc := compareTraces(t, engineWithout(t, tt.rule), c, g, r)
			for _, m := range doc.Mismatches {
				if m.JSONPath == tt.wantPath {
					return
				}
			}
			t.Fatalf("without the rule no mismatch at %s: %v", tt.wantPath, ids(doc))
		})
	}
}

// TestJSONMemberOrderRule: member order is never normalized away. With the
// rule it is lexical (member_order); without it, it is semantic (order).
func TestJSONMemberOrderRule(t *testing.T) {
	for _, tt := range []struct {
		name string
		e    *Engine
		want brief
	}{
		{"with rule", newTestEngine(t), brief{parity.KindLexical, parity.LexicalMemberOrder, "$"}},
		{"without rule", engineWithout(t, RuleJSONMemberOrder), brief{parity.KindOrder, "", "$"}},
	} {
		t.Run(tt.name, func(t *testing.T) {
			got := briefs(diffEntries(t, tt.e, parity.StageRequestValidated, `{"a":1,"b":2}`, `{"b":2,"a":1}`))
			if !equalBriefs(got, []brief{tt.want}) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
		})
	}
}

// TestRuleStageScope: a rule only applies to the stages it lists. The same
// seatbid permutation is normalized in http.response but reported in
// bidder.result, which seatbid_order does not list.
func TestRuleStageScope(t *testing.T) {
	e := newTestEngine(t)
	g := `{"id":"r1","seatbid":[{"seat":"b","bid":[]},{"seat":"a","bid":[]}]}`
	r := `{"id":"r1","seatbid":[{"seat":"a","bid":[]},{"seat":"b","bid":[]}]}`
	if got := diffEntries(t, e, parity.StageBidderResult, g, r); len(got) != 1 || got[0].Kind != parity.KindOrder {
		t.Fatalf("bidder.result: %v", briefs(got))
	}
	_, gt, rt := responsePair(g, r, nil, nil)()
	if doc := compareTraces(t, e, Case{}, gt, rt); len(doc.Mismatches) != 0 {
		t.Fatalf("http.response: %v", ids(doc))
	}
}

// TestUUIDInputExclusion: a UUID the caller supplied is never a placeholder,
// so a Rust side that regenerates it is caught; and a UUID that is not v4 is
// not treated as Go-generated.
func TestUUIDInputExclusion(t *testing.T) {
	e := newTestEngine(t)
	req := func(id string) string { return `{"id":"` + id + `","imp":[{"id":"i1"}]}` }
	tests := []struct {
		name      string
		goID      string
		rustID    string
		knownIn   [][]byte
		wantDiffs int
	}{
		{"generated v4 on both sides", uuidA, uuidB, nil, 0},
		{"Go kept the caller's id", uuidA, uuidB, [][]byte{[]byte(req(uuidA))}, 1},
		{"not version 4", uuidV1, uuidB, nil, 1},
		{"same id", uuidA, uuidA, [][]byte{[]byte(req(uuidA))}, 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
			put(g, parity.StageRequestValidated, parity.UnkeyedStage, req(tt.goID))
			put(r, parity.StageRequestValidated, parity.UnkeyedStage, req(tt.rustID))
			doc := compareTraces(t, e, Case{CaseID: "uuid", KnownInputs: tt.knownIn}, g, r)
			if len(doc.Mismatches) != tt.wantDiffs {
				t.Fatalf("got %v, want %d mismatches", ids(doc), tt.wantDiffs)
			}
		})
	}
}
