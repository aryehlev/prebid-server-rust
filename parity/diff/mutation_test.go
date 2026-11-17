package diff

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// The mutation tests seed known Go-vs-Rust differences into the Rust copy of
// a recorded Go trace and check that the engine detects each one and
// attributes it to the stage that introduced it, with the right kind (and,
// for lexical and wire records, class). A bug that propagates is seeded at
// its origin and in everything recorded after it, so the test also checks
// that the later records are consequential. The noise mutations are the
// run-to-run differences the allowlist covers; they must compare equal.

// Values of the baseline trace the mutations edit.
const (
	baseTimestamp = "1794881417787"
	impTID        = "9faad7da-880b-4948-b7c9-c76991b3d520"
	cacheUUID     = "2a32c3d0-ec69-8ab3-a69e-91fc1ac4ef95"
	// otherCacheUUID is another UUID a (mock) Prebid Cache could return.
	otherCacheUUID = "5b1e7c2a-3d4f-8a9b-b1c2-d3e4f5a6b7c8"
	xPrebid        = `,["X-Prebid","pbs-go/unknown"]`
)

// TestMutationBaseline: the baseline compared with itself has no mismatch,
// so every record in the other tests comes from its mutation.
func TestMutationBaseline(t *testing.T) {
	for _, name := range []string{"auction.go.trace.json", "auction.envelope.json"} {
		raw, err := os.ReadFile(filepath.Join(mutationDir, name))
		if err != nil {
			t.Fatal(err)
		}
		if err := schema.Validate(raw); err != nil {
			t.Fatalf("%s does not validate: %v", name, err)
		}
	}
	c, g, r := loadMutationCase(t)
	if doc := compareTraces(t, newTestEngine(t), c, g, r); len(doc.Mismatches) != 0 {
		t.Fatalf("baseline A/A: %v", ids(doc))
	}
}

type mutationCase struct {
	name   string
	mutate func(m *mutant)
	want   expect
}

// TestSemanticMutationsAreDetected: value, structure, status, error,
// metric, decision and outbound differences.
func TestSemanticMutationsAreDetected(t *testing.T) {
	runMutations(t, []mutationCase{
		{"bidder request value", func(m *mutant) {
			m.cascade(parity.StageBidderRequest, "appnexus", `"secure":1`, `"secure":0`)
		}, expect{stage: parity.StageBidderRequest, kind: parity.KindValue, pathSuffix: "$.request.imp[0].secure", downstream: parity.StageHTTPOut}},
		{"validated request loses a member", func(m *mutant) {
			m.cascade(parity.StageRequestValidated, "", `"at":1,`, ``)
		}, expect{stage: parity.StageRequestValidated, kind: parity.KindMissingInRust, pathSuffix: "$.at", downstream: parity.StageBidderRequest}},
		{"X-Prebid dropped from the final adapter request", func(m *mutant) {
			m.editEntry(parity.StageAdapterRequestsFinal, "appnexus", xPrebid, ``)
			c := m.call(parity.OutboundBidder, "appnexus")
			c.Request.Header = dropHeader(m.t, c.Request.Header, "X-Prebid")
		}, expect{stage: parity.StageAdapterRequestsFinal, kind: parity.KindHeader, pathSuffix: ".headers['X-Prebid']", downstream: parity.StageHTTPOut}},
		{"bid adjustment factor", func(m *mutant) {
			m.editEntry(parity.StageBidsAdjusted, "pubmatic", `"factor":1,`, `"factor":0.9,`)
		}, expect{stage: parity.StageBidsAdjusted, kind: parity.KindValue, pathSuffix: "$.bids[0].factor"}},
		{"targeting price bucket", func(m *mutant) {
			m.cascade(parity.StageTargeting, "", `"hb_pb":"5.00"`, `"hb_pb":"4.50"`)
		}, expect{stage: parity.StageTargeting, kind: parity.KindValue, pathSuffix: ".hb_pb", downstream: parity.StageHTTPResponse}},
		{"seat non-bid", func(m *mutant) {
			m.editEntry(parity.StageSeatNonBids, parity.UnkeyedStage, `{}`, `{"appnexus":[{"impid":"my-imp-id","statuscode":303}]}`)
		}, expect{stage: parity.StageSeatNonBids, kind: parity.KindExtraInRust, pathSuffix: "$.appnexus"}},
		{"analytics status", func(m *mutant) {
			m.r.Analytics[0].Object = json.RawMessage(strings.Replace(string(m.r.Analytics[0].Object), `"status":200`, `"status":400`, 1))
		}, expect{stage: parity.StageAnalytics, kind: parity.KindValue, pathSuffix: ".object.status"}},
		{"response status", func(m *mutant) { m.r.HTTPResponse.Status = 400 },
			expect{stage: parity.StageHTTPResponse, kind: parity.KindStatus}},
		{"response header dropped", func(m *mutant) {
			m.r.HTTPResponse.Headers = dropHeader(m.t, m.r.HTTPResponse.Headers, "X-Prebid")
		}, expect{stage: parity.StageHTTPResponse, kind: parity.KindHeader}},
		{"response header value", func(m *mutant) {
			m.r.HTTPResponse.Headers = setHeader(m.t, m.r.HTTPResponse.Headers, "Cache-Control", "no-cache")
		}, expect{stage: parity.StageHTTPResponse, kind: parity.KindHeader}},
		{"warning code", func(m *mutant) {
			m.r.Stages[parity.StageRequestValidated][parity.UnkeyedStage].Warnings[0].Code = 10002
		}, expect{stage: parity.StageRequestValidated, kind: parity.KindErrorCode}},
		{"warning text", func(m *mutant) {
			m.r.Stages[parity.StageRequestValidated][parity.UnkeyedStage].Warnings[0].Message = "request.imp[0].ext contains unknown bidder: arbitraryObject"
		}, expect{stage: parity.StageRequestValidated, kind: parity.KindErrorText}},
		{"stage unimplemented", func(m *mutant) {
			delete(m.r.Stages, parity.StageTargeting)
			m.r.StageStatus[parity.StageTargeting] = parity.StageUnimplemented
		}, expect{stage: parity.StageTargeting, kind: parity.KindStageMissing}},
		{"stage not reached", func(m *mutant) {
			delete(m.r.Stages, parity.StageCacheResult)
			m.r.StageStatus[parity.StageCacheResult] = parity.StageNotReached
		}, expect{stage: parity.StageCacheResult, kind: parity.KindStageMissing}},
		{"stage reached only by rust", func(m *mutant) {
			m.r.Stages[parity.StageFloorsEnforce] = map[string]parity.StageEntry{parity.UnkeyedStage: rawEntry(`{"enforced":false}`)}
			m.r.StageStatus[parity.StageFloorsEnforce] = parity.StageCaptured
		}, expect{stage: parity.StageFloorsEnforce, kind: parity.KindStageExtra}},
		{"metric label", func(m *mutant) {
			m.metric("requests", map[string]string{"request_status": "ok"}).Labels = map[string]string{"request_status": "badinput", "request_type": "openrtb2-web"}
		}, expect{stage: parity.StageMetricsDelta, kind: parity.KindMetric, pathSuffix: "$.requests"}},
		{"metric delta", func(m *mutant) {
			m.metric("adapter_requests", map[string]string{"adapter": "appnexus"}).Delta = 2
		}, expect{stage: parity.StageMetricsDelta, kind: parity.KindMetric, pathSuffix: "$.adapter_requests"}},
		{"metric name", func(m *mutant) {
			m.metric("requests", map[string]string{"request_status": "ok"}).Name = "requests_total"
		}, expect{stage: parity.StageMetricsDelta, kind: parity.KindMetric, pathSuffix: "$.requests_total"}},
		{"timing histogram observation count", func(m *mutant) {
			two := int64(2)
			m.metric("request_time_seconds", nil).CountDelta = &two
		}, expect{stage: parity.StageMetricsDelta, kind: parity.KindMetric, pathSuffix: "$.request_time_seconds"}},
		{"bid price histogram sum", func(m *mutant) {
			m.metric("adapter_prices", map[string]string{"adapter": "pubmatic"}).Delta = 18700
		}, expect{stage: parity.StageMetricsDelta, kind: parity.KindMetric, pathSuffix: "$.adapter_prices"}},
		{"decision value", func(m *mutant) {
			m.g.Decisions = []parity.Decision{{Point: parity.DecisionFloorsSkip, Value: json.RawMessage("0"), Source: parity.DecisionSourceEnvelope}}
			m.r.Decisions = []parity.Decision{{Point: parity.DecisionFloorsSkip, Value: json.RawMessage("1"), Source: parity.DecisionSourceEnvelope}}
		}, expect{stage: parity.StageFloorsEnrich, kind: parity.KindDecision}},
		{"decision source", func(m *mutant) {
			m.g.Decisions = []parity.Decision{{Point: parity.DecisionFloorsEnforce, Value: json.RawMessage("1"), Source: parity.DecisionSourceRNG}}
			m.r.Decisions = []parity.Decision{{Point: parity.DecisionFloorsEnforce, Value: json.RawMessage("1"), Source: parity.DecisionSourceEnvelope}}
		}, expect{stage: parity.StageFloorsEnforce, kind: parity.KindDecision}},
		{"decision consulted by rust only", func(m *mutant) {
			m.r.Decisions = []parity.Decision{{Point: parity.DecisionCategoryDedupe, Value: json.RawMessage("0"), Source: parity.DecisionSourceRNG}}
		}, expect{stage: parity.StageCategoryMapping, kind: parity.KindDecision}},
		{"cache TTL sent to Prebid Cache", func(m *mutant) {
			replaceBody(m.t, m.call(parity.OutboundCache, "").Request.Body, `{"puts":[{"type":"json",`, `{"puts":[{"type":"json","ttlseconds":300,`)
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindExtraInRust, pathSuffix: ".ttlseconds"}},
		{"cache UUID other than the one Prebid Cache returned", func(m *mutant) {
			m.cascade(parity.StageCacheResult, "", cacheUUID, otherCacheUUID)
		}, expect{stage: parity.StageCacheResult, kind: parity.KindValue, pathSuffix: "$.puts[0].id", downstream: parity.StageHTTPResponse}},
		{"extra outbound call", func(m *mutant) {
			extra := *m.call(parity.OutboundCache, "")
			extra.Seq = 99
			m.r.HTTPOut = append(m.r.HTTPOut, extra)
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindExtraInRust}},
		{"unmocked outbound call", func(m *mutant) {
			extra := *m.call(parity.OutboundBidder, "appnexus")
			extra.Seq, extra.OrigURL, extra.Mock = 99, "https://unmocked.example/openrtb2", nil
			m.r.HTTPOut = append(m.r.HTTPOut, extra)
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindUnexpectedOutbound}},
	})
}

// TestLexicalAndWireMutationsAreDetected: byte-level differences of
// semantically equal JSON and transport differences gate parity like any
// other (orchestrator decision); none is normalized away.
func TestLexicalAndWireMutationsAreDetected(t *testing.T) {
	runMutations(t, []mutationCase{
		{"number spelling", func(m *mutant) {
			m.editEntry(parity.StageBidsAdjusted, "pubmatic", `"factor":1,`, `"factor":1.0,`)
		}, expect{stage: parity.StageBidsAdjusted, kind: parity.KindLexical, class: parity.LexicalNumberFormat, pathSuffix: "$.bids[0].factor"}},
		{"HTML escaping", func(m *mutant) {
			m.editEntry(parity.StageAccountResolved, parity.UnkeyedStage, esc(`^u0026`), `&`)
		}, expect{stage: parity.StageAccountResolved, kind: parity.KindLexical, class: parity.LexicalHTMLEscape}},
		{"trailing newline", func(m *mutant) {
			body := strings.TrimSuffix(*m.r.HTTPResponse.Body.Text, "\n")
			m.r.HTTPResponse.Body = parity.NewBody([]byte(body), "")
		}, expect{stage: parity.StageHTTPResponse, kind: parity.KindLexical, class: parity.LexicalTrailingNewline}},
		{"stage entry without raw bytes", func(m *mutant) {
			e := m.r.Stages[parity.StageAccountResolved][parity.UnkeyedStage]
			e.Raw = nil
			m.r.Stages[parity.StageAccountResolved][parity.UnkeyedStage] = e
		}, expect{stage: parity.StageAccountResolved, kind: parity.KindLexical, class: parity.LexicalUnrecorded}},
		{"response header name case", func(m *mutant) {
			m.r.HTTPResponse.Headers = renameHeader(m.t, m.r.HTTPResponse.Headers, "Content-Type", "content-type")
		}, expect{stage: parity.StageHTTPResponse, kind: parity.KindWire, class: parity.WireHeaderCase}},
		{"outbound User-Agent", func(m *mutant) {
			c := m.call(parity.OutboundBidder, "appnexus")
			c.Request.Header = setHeader(m.t, c.Request.Header, "User-Agent", "reqwest/0.12")
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindWire, class: parity.WireDefaultHeader}},
		{"outbound Accept-Encoding dropped", func(m *mutant) {
			c := m.call(parity.OutboundBidder, "appnexus")
			c.Request.Header = dropHeader(m.t, c.Request.Header, "Accept-Encoding")
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindWire, class: parity.WireDefaultHeader}},
		{"outbound header name case", func(m *mutant) {
			c := m.call(parity.OutboundBidder, "appnexus")
			c.Request.Header = renameHeader(m.t, c.Request.Header, "Content-Type", "content-type")
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindWire, class: parity.WireHeaderCase}},
		{"outbound protocol", func(m *mutant) {
			m.call(parity.OutboundBidder, "appnexus").Request.Proto = "HTTP/2.0"
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindWire, class: parity.WireHTTPVersion}},
		{"outbound Content-Length", func(m *mutant) {
			c := m.call(parity.OutboundBidder, "appnexus")
			c.Request.Header = setHeader(m.t, c.Request.Header, "Content-Length", "504")
		}, expect{stage: parity.StageHTTPOut, kind: parity.KindWire, class: parity.WireContentLength}},
	})
}

// TestTimeValueMutationsAreDetected: values the time-valued rules mask are
// still compared when they have the wrong unit or shape.
func TestTimeValueMutationsAreDetected(t *testing.T) {
	runMutations(t, []mutationCase{
		{"auction timestamp in seconds", func(m *mutant) {
			m.cascade(parity.StageExchangeResponse, "", baseTimestamp, baseTimestamp[:10])
		}, expect{stage: parity.StageExchangeResponse, kind: parity.KindValue, pathSuffix: ".auctiontimestamp", downstream: parity.StageHTTPResponse}},
		{"event auction timestamp in seconds", func(m *mutant) {
			m.editEntry(parity.StageEventsTracking, parity.UnkeyedStage, baseTimestamp, baseTimestamp[:10])
		}, expect{stage: parity.StageEventsTracking, kind: parity.KindValue, pathSuffix: "$.auction_timestamp_ms"}},
		{"fractional responsetimemillis", func(m *mutant) {
			m.cascade(parity.StageExchangeResponse, "", `"responsetimemillis":{"appnexus":0,`, `"responsetimemillis":{"appnexus":0.5,`)
		}, expect{stage: parity.StageExchangeResponse, kind: parity.KindValue, pathSuffix: ".responsetimemillis.appnexus", downstream: parity.StageHTTPResponse}},
		{"hook execution time missing", func(m *mutant) {
			m.g.Stages[parity.StageHookOutcomes][parity.UnkeyedStage] = rawEntry(`[{"stage":"entrypoint","execution_time_millis":3,"groups":[]}]`)
			m.r.Stages[parity.StageHookOutcomes][parity.UnkeyedStage] = rawEntry(`[{"stage":"entrypoint","groups":[]}]`)
		}, expect{stage: parity.StageHookOutcomes, kind: parity.KindMissingInRust, pathSuffix: "$[0].execution_time_millis"}},
	})
}

func runMutations(t *testing.T, cases []mutationCase) {
	t.Helper()
	e := newTestEngine(t)
	for _, tt := range cases {
		t.Run(tt.name, func(t *testing.T) {
			c, g, r := loadMutationCase(t)
			tt.mutate(&mutant{t: t, g: g, r: r})
			checkAttribution(t, compareTraces(t, e, c, g, r), tt.want)
		})
	}
}

// TestNoiseMutationsCompareEqual: run-to-run differences of two Go
// processes, each covered by an allowlist rule, produce no mismatch.
func TestNoiseMutationsCompareEqual(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(m *mutant)
	}{
		{"bidder elapsed times", func(m *mutant) {
			m.cascade(parity.StageExchangeResponse, "", `"responsetimemillis":{"appnexus":0,"pubmatic":1}`, `"responsetimemillis":{"appnexus":12,"pubmatic":40}`)
		}},
		{"another millisecond auction timestamp", func(m *mutant) {
			m.cascade(parity.StageEventsTracking, "", baseTimestamp, "1794881499999")
		}},
		{"seat order in the response", func(m *mutant) {
			body := swapFirstTwo(m.t, *m.r.HTTPResponse.Body.Text, "seatbid")
			m.r.HTTPResponse.Body = parity.NewBody([]byte(body), "")
		}},
		{"another generated imp tid", func(m *mutant) {
			m.cascade(parity.StageRequestValidated, "", impTID, "0c6f2a8e-5b7d-4e1f-9a3c-2d4b6e8f0a1c")
		}},
		{"Prebid Cache returned another UUID", func(m *mutant) {
			replaceBody(m.t, m.call(parity.OutboundCache, "").Response.Body, cacheUUID, otherCacheUUID)
			m.cascade(parity.StageCacheResult, "", cacheUUID, otherCacheUUID)
		}},
		{"outbound completion order and sequence", func(m *mutant) {
			out := m.r.HTTPOut
			for i, j := 0, len(out)-1; i < j; i, j = i+1, j-1 {
				out[i], out[j] = out[j], out[i]
			}
			for i := range out {
				out[i].Seq = 200 + i
			}
		}},
		{"response Date header", func(m *mutant) {
			m.g.HTTPResponse.Headers = append(m.g.HTTPResponse.Headers, [2]string{"Date", "Mon, 16 Nov 2026 10:00:00 GMT"})
			m.r.HTTPResponse.Headers = append(m.r.HTTPResponse.Headers, [2]string{"Date", "Mon, 16 Nov 2026 10:00:07 GMT"})
		}},
		{"capture sequence numbers", func(m *mutant) {
			for _, keyed := range m.r.Stages {
				for key, e := range keyed {
					e.Seq += 100
					keyed[key] = e
				}
			}
		}},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c, g, r := loadMutationCase(t)
			tt.mutate(&mutant{t: t, g: g, r: r})
			if doc := compareTraces(t, e, c, g, r); len(doc.Mismatches) != 0 {
				t.Fatalf("noise reported: %v", ids(doc))
			}
		})
	}
}
