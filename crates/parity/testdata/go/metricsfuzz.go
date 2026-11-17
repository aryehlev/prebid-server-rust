package main

// Generated metric changes (metrics_fuzz.json): random registries of every
// metric type, with metric and label names that prefix each other, label
// values that need quoting, NaN, infinite and denormal values and random
// bucket layouts, changed between two gathers; and the delta Go reports.

import (
	"math"
	"sort"
	"strconv"

	"github.com/prometheus/client_golang/prometheus"

	"github.com/prebid/prebid-server/v4/parity/trace"
)

// fuzzRegistries is the number of generated registries.
const fuzzRegistries = 150

// fuzzBucket is a bucket with its bound as text, so the Rust side parses the
// exact float.
type fuzzBucket struct {
	LE    string `json:"le"`
	Count uint64 `json:"count"`
}

type fuzzSeries struct {
	Name    string            `json:"name"`
	Type    string            `json:"type"`
	Labels  map[string]string `json:"labels"`
	Value   string            `json:"value"`
	Count   uint64            `json:"count"`
	Buckets []fuzzBucket      `json:"buckets"`
}

func fuzzGather(reg *prometheus.Registry) ([]fuzzSeries, trace.MetricsSnapshot, error) {
	gathered, snap, err := gather(reg)
	if err != nil {
		return nil, snap, err
	}
	out := make([]fuzzSeries, 0, len(gathered))
	for _, s := range gathered {
		fs := fuzzSeries{Name: s.Name, Type: s.Type, Labels: s.Labels, Value: s.Value, Count: s.Count, Buckets: []fuzzBucket{}}
		for _, b := range s.Buckets {
			fs.Buckets = append(fs.Buckets, fuzzBucket{LE: strconv.FormatFloat(b.UpperBound, 'g', -1, 64), Count: b.Count})
		}
		out = append(out, fs)
	}
	return out, snap, nil
}

var (
	fuzzMetricNames = []string{"req", "req_total", "req:x", "a", "a_b", "A", "Z_", "_x", "requests", "requests_seconds", "b"}
	fuzzLabelNames  = []string{"a", "a_b", "A", "b", "type", "z", "_x", "x1", "Type"}
)

// metricValue is a value for a gauge or an observation.
func (f *fuzzer) metricValue() float64 {
	switch f.intn(6) {
	case 0:
		return []float64{0, math.Copysign(0, -1), 1, -1, 0.5, 2.5, 1e21, 1e-320, 5e-324, 1e308, -1e308,
			math.Inf(1), math.Inf(-1), math.NaN(), 0.1, 0.2, 0.30000000000000004, 1234567, 123456}[f.intn(19)]
	case 1:
		return float64(f.intn(100))
	default:
		return f.float()
	}
}

// counterAdd is a value a counter accepts (Add panics below zero).
func (f *fuzzer) counterAdd() float64 {
	v := f.metricValue()
	if v < 0 {
		return -v
	}
	return v
}

// buckets are strictly increasing finite bounds.
func (f *fuzzer) buckets() []float64 {
	set := map[float64]bool{}
	for i, n := 0, f.intn(6)+1; i < n; i++ {
		var b float64
		switch f.intn(4) {
		case 0:
			b = []float64{0.005, 0.1, 1, 2.5, 1e21, 1e-7, 1234567, 123456, 1e6, 999999, 0, -1, 5e-324, 1e308}[f.intn(14)]
		case 1:
			b = float64(f.intn(50))
		default:
			b = f.float()
		}
		if !math.IsNaN(b) && !math.IsInf(b, 0) {
			set[b] = true
		}
	}
	out := make([]float64, 0, len(set))
	for b := range set {
		out = append(out, b)
	}
	sort.Float64s(out)
	return out
}

// labelValues are the label values of one series.
func (f *fuzzer) labelValues(names []string) []string {
	values := make([]string, len(names))
	for i := range values {
		values[i] = f.pick("", "amp", "web", `"q"`, `\`, "a\nb", cp(0x2028), "<&>", cp(0xa0), cp(0xe9), cp(0x1f600), "\x7f", "\t") + f.text()
	}
	return values
}

// fuzzMetricsGolden builds random registries and records the series of a
// gather before and after random changes, and Go's delta between them.
func fuzzMetricsGolden() (any, error) {
	f := newFuzzer(fuzzSeed + 3)
	cases := make([]map[string]any, 0, fuzzRegistries)
	for i := 0; i < fuzzRegistries; i++ {
		reg := prometheus.NewRegistry()
		var later []func()
		used := map[string]bool{}
		for j, n := 0, f.intn(5)+1; j < n; j++ {
			name := fuzzMetricNames[f.intn(len(fuzzMetricNames))]
			if used[name] {
				continue
			}
			used[name] = true
			var labels []string
			seen := map[string]bool{}
			for k, m := 0, f.intn(4); k < m; k++ {
				l := fuzzLabelNames[f.intn(len(fuzzLabelNames))]
				if !seen[l] {
					seen[l] = true
					labels = append(labels, l)
				}
			}
			tuples := make([][]string, f.intn(3)+1)
			for k := range tuples {
				tuples[k] = f.labelValues(labels)
			}
			// now runs a change at once (before the first gather) or later.
			now := func(change func()) {
				if f.chance(50) {
					change()
				} else {
					later = append(later, change)
				}
			}
			switch f.intn(5) {
			case 0:
				vec := prometheus.NewCounterVec(prometheus.CounterOpts{Name: name}, labels)
				reg.MustRegister(vec)
				for _, t := range tuples {
					c, v := vec.WithLabelValues(t...), f.counterAdd()
					now(func() { c.Add(v) })
				}
			case 1:
				vec := prometheus.NewGaugeVec(prometheus.GaugeOpts{Name: name}, labels)
				reg.MustRegister(vec)
				for _, t := range tuples {
					g, v := vec.WithLabelValues(t...), f.metricValue()
					now(func() { g.Set(v) })
				}
			case 2:
				vec := prometheus.NewHistogramVec(prometheus.HistogramOpts{Name: name, Buckets: f.buckets()}, labels)
				reg.MustRegister(vec)
				for _, t := range tuples {
					h := vec.WithLabelValues(t...)
					for k, m := 0, f.intn(4); k < m; k++ {
						v := f.metricValue()
						now(func() { h.Observe(v) })
					}
				}
			case 3:
				vec := prometheus.NewSummaryVec(prometheus.SummaryOpts{Name: name}, labels)
				reg.MustRegister(vec)
				for _, t := range tuples {
					s := vec.WithLabelValues(t...)
					for k, m := 0, f.intn(3); k < m; k++ {
						v := f.metricValue()
						now(func() { s.Observe(v) })
					}
				}
			default:
				value := f.metricValue()
				reg.MustRegister(prometheus.NewUntypedFunc(prometheus.UntypedOpts{Name: name}, func() float64 { return value }))
				next := f.metricValue()
				now(func() { value = next })
			}
		}
		before, beforeSnap, err := fuzzGather(reg)
		if err != nil {
			return nil, err
		}
		for _, change := range later {
			change()
		}
		after, afterSnap, err := fuzzGather(reg)
		if err != nil {
			return nil, err
		}
		delta, err := marshalString(trace.MetricsDelta(beforeSnap, afterSnap))
		if err != nil {
			return nil, err
		}
		cases = append(cases, map[string]any{"before": before, "after": after, "delta": delta})
	}
	return cases, nil
}
