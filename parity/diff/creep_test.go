package diff

import (
	"encoding/json"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

// TestNarrowShapesStillReport: the time-valued rules mask only values shaped
// like Go's (millisecond timestamps, non-negative millisecond durations), so a
// Rust value with the right JSON type but the wrong unit, sign or presence is
// still a mismatch. Each pair would be normalization creep if it compared
// equal.
func TestNarrowShapesStillReport(t *testing.T) {
	const goTS, secondsTS = "1700000000000", "1700000000"
	tests := []struct {
		name     string
		pair     ruleCase
		wantPath string
	}{
		{"auctiontimestamp in seconds", responsePair(
			`{"id":"r1","ext":{"prebid":{"auctiontimestamp":`+goTS+`}}}`,
			`{"id":"r1","ext":{"prebid":{"auctiontimestamp":`+secondsTS+`}}}`, nil, nil), "$.ext.prebid.auctiontimestamp"},
		{"auctiontimestamp in microseconds", entryPair(parity.StageEventsTracking, parity.UnkeyedStage,
			`{"auction_timestamp_ms":`+goTS+`}`, `{"auction_timestamp_ms":1700000000000000}`), "$.auction_timestamp_ms"},
		{"event ts in seconds", responsePair(eventAdm(goTS), eventAdm(secondsTS), nil, nil), "$.seatbid[0].bid[0].adm"},
		{"responsetimemillis as a string", responsePair(
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":12}}}`,
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":"12"}}}`, nil, nil), "$.ext.responsetimemillis.appnexus"},
		{"responsetimemillis as a fraction", responsePair(
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":12}}}`,
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":12.5}}}`, nil, nil), "$.ext.responsetimemillis.appnexus"},
		{"responsetimemillis negative", responsePair(
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":12}}}`,
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":-1}}}`, nil, nil), "$.ext.responsetimemillis.appnexus"},
		{"hook execution time missing", entryPair(parity.StageHookOutcomes, parity.UnkeyedStage,
			`[{"stage":"entrypoint","execution_time_millis":3,"groups":[]}]`,
			`[{"stage":"entrypoint","groups":[]}]`), "$[0].execution_time_millis"},
		{"hook execution time as a string", entryPair(parity.StageHookOutcomes, parity.UnkeyedStage,
			`[{"stage":"entrypoint","execution_time_millis":3,"groups":[]}]`,
			`[{"stage":"entrypoint","execution_time_millis":"3ms","groups":[]}]`), "$[0].execution_time_millis"},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c, g, r := tt.pair()
			doc := compareTraces(t, e, c, g, r)
			for _, m := range doc.Mismatches {
				if m.JSONPath == tt.wantPath {
					return
				}
			}
			t.Fatalf("no mismatch at %s: %v", tt.wantPath, ids(doc))
		})
	}
}

// TestNarrowShapesAcceptGoNoise: values of Go's own shape still compare equal
// whatever their value (two runs of the same case).
func TestNarrowShapesAcceptGoNoise(t *testing.T) {
	tests := []struct {
		name string
		pair ruleCase
	}{
		{"auctiontimestamp", responsePair(`{"id":"r1","ext":{"prebid":{"auctiontimestamp":1700000000000}}}`,
			`{"id":"r1","ext":{"prebid":{"auctiontimestamp":1794881417787}}}`, nil, nil)},
		{"event ts", responsePair(eventAdm("1700000000000"), eventAdm("1794881417787"), nil, nil)},
		{"responsetimemillis", responsePair(`{"id":"r1","ext":{"responsetimemillis":{"appnexus":0}}}`,
			`{"id":"r1","ext":{"responsetimemillis":{"appnexus":731}}}`, nil, nil)},
		{"hook execution time", entryPair(parity.StageHookOutcomes, parity.UnkeyedStage,
			`[{"stage":"entrypoint","execution_time_millis":0,"groups":[]}]`,
			`[{"stage":"entrypoint","execution_time_millis":17,"groups":[]}]`)},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c, g, r := tt.pair()
			if doc := compareTraces(t, e, c, g, r); len(doc.Mismatches) != 0 {
				t.Fatalf("Go-shaped noise reported: %v", ids(doc))
			}
		})
	}
}

// TestUnrecordedBytesAreReported: a side that records only the semantic json
// of a value cannot take part in any lexical comparison. Before, that was a
// note and the pair compared equal, so a Rust trace writer that left out raw
// bytes passed every lexical gate (here: an HTML escape Go writes and Rust
// would not). Now it is a lexical "unrecorded" record; two semantic-only
// sides compare as before.
func TestUnrecordedBytesAreReported(t *testing.T) {
	escaped, plain := esc(`{"u":"a^u0026b"}`), `{"u":"a&b"}`
	semantic := func(js string) parity.StageEntry {
		return parity.StageEntry{Seq: 1, Writes: 1, JSON: json.RawMessage(js), Errors: []parity.ErrorRecord{}, Warnings: []parity.ErrorRecord{}}
	}
	tests := []struct {
		name string
		g, r *parity.Trace
		want []brief
	}{
		{"rust entry without raw", entryTrace(parity.ImplGo, rawEntry(escaped)), entryTrace(parity.ImplRust, semantic(plain)),
			[]brief{{parity.KindLexical, parity.LexicalUnrecorded, "$"}}},
		{"go entry without raw", entryTrace(parity.ImplGo, semantic(escaped)), entryTrace(parity.ImplRust, rawEntry(escaped)),
			[]brief{{parity.KindLexical, parity.LexicalUnrecorded, "$"}}},
		{"neither side has raw", entryTrace(parity.ImplGo, semantic(escaped)), entryTrace(parity.ImplRust, semantic(plain)), nil},
		{"both sides have raw", entryTrace(parity.ImplGo, rawEntry(escaped)), entryTrace(parity.ImplRust, rawEntry(plain)),
			[]brief{{parity.KindLexical, parity.LexicalHTMLEscape, "$.u"}}},
		{"rust body without text", bodyTrace(parity.ImplGo, parity.TextBody(escaped)), bodyTrace(parity.ImplRust, &parity.Body{JSON: json.RawMessage(plain)}),
			[]brief{{parity.KindLexical, parity.LexicalUnrecorded, "$"}}},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := briefs(compareTraces(t, e, Case{CaseID: "unrecorded"}, tt.g, tt.r).Mismatches)
			if !equalBriefs(got, tt.want) && !(len(got) == 0 && len(tt.want) == 0) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
		})
	}
}

func entryTrace(impl string, e parity.StageEntry) *parity.Trace {
	t := newTrace(impl)
	t.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{parity.UnkeyedStage: e}
	return t
}

func bodyTrace(impl string, b *parity.Body) *parity.Trace {
	t := newTrace(impl)
	t.HTTPResponse = &parity.HTTPResponse{Status: 200, Headers: parity.HeaderList{}, Body: b}
	return t
}

// TestGoRuntimeMetricNames: only the Go runtime collector's own families are
// ignored, with or without a namespace/subsystem prefix; a product metric
// whose name merely contains "_go_" is compared.
func TestGoRuntimeMetricNames(t *testing.T) {
	tests := []struct {
		name string
		want bool
	}{
		{"go_goroutines", true},
		{"go_memstats_alloc_bytes", true},
		{"go_gc_duration_seconds", true},
		{"pbs_go_gc_pauses_seconds_total", true},
		{"ns_sub_go_sched_goroutines_goroutines", true},
		{"go_info", true},
		{"requests", false},
		{"adapter_go_requests", false},
		{"pbs_cargo_gc_total", false},
		{"go_requests", false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isGoRuntimeMetric(tt.name); got != tt.want {
				t.Fatalf("isGoRuntimeMetric(%q) = %v, want %v", tt.name, got, tt.want)
			}
		})
	}
}
