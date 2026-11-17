package diff

import (
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
