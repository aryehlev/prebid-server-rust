package trace

import (
	"errors"
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prometheus/client_golang/prometheus"
	dto "github.com/prometheus/client_model/go"
)

// MetricsSnapshot is a point-in-time copy of every series of a Prometheus
// gatherer, keyed by name plus canonical labels.
type MetricsSnapshot struct {
	series map[string]metricSeries
}

type metricSeries struct {
	name    string
	typ     string
	labels  map[string]string
	value   float64
	count   uint64
	buckets map[string]uint64
}

// GatherMetrics snapshots g. The exec handler calls it before and after a case.
func GatherMetrics(g prometheus.Gatherer) (MetricsSnapshot, error) {
	if g == nil {
		return MetricsSnapshot{}, errors.New("parity: no Prometheus gatherer (metrics.prometheus.port is 0)")
	}
	families, err := g.Gather()
	if err != nil {
		return MetricsSnapshot{}, fmt.Errorf("parity: gathering metrics: %w", err)
	}
	snap := MetricsSnapshot{series: make(map[string]metricSeries)}
	for _, family := range families {
		for _, m := range family.GetMetric() {
			s := toSeries(family, m)
			snap.series[seriesKey(s.name, s.labels)] = s
		}
	}
	return snap, nil
}

// MetricsDelta returns the series that changed between before and after,
// sorted by name and labels. Counters, gauges and untyped metrics report the
// value change; histograms and summaries report the sum change in Delta plus
// count and (histogram only) per-bucket count changes. Series absent from
// before count as zero.
func MetricsDelta(before, after MetricsSnapshot) []parity.MetricDelta {
	keys := make([]string, 0, len(after.series))
	for key := range after.series {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	out := make([]parity.MetricDelta, 0)
	for _, key := range keys {
		if d, changed := seriesDelta(before.series[key], after.series[key]); changed {
			out = append(out, d)
		}
	}
	return out
}

func seriesDelta(before, after metricSeries) (parity.MetricDelta, bool) {
	d := parity.MetricDelta{
		Name:   after.name,
		Type:   after.typ,
		Labels: after.labels,
		Delta:  valueDelta(before.value, after.value),
	}
	if after.typ != parity.MetricHistogram && after.typ != parity.MetricSummary {
		return d, d.Delta != 0
	}
	countDelta := int64(after.count) - int64(before.count)
	d.CountDelta = &countDelta
	if after.typ == parity.MetricHistogram {
		d.BucketDeltas = make(map[string]int64)
		for le, count := range after.buckets {
			if delta := int64(count) - int64(before.buckets[le]); delta != 0 {
				d.BucketDeltas[le] = delta
			}
		}
	}
	return d, countDelta != 0 || d.Delta != 0
}

// valueDelta is after - before when that is finite, else 0. JSON cannot carry
// NaN or infinities, and the histograms the Go runtime collector exports
// (metrics/prometheus/prometheus.go NewGoCollector: go_gc_pauses_seconds_total
// and others built on runtime/metrics) always report a NaN sum. Such a series
// is reported only through its count and bucket changes.
func valueDelta(before, after float64) float64 {
	d := after - before
	if math.IsNaN(d) || math.IsInf(d, 0) {
		return 0
	}
	return d
}

func toSeries(family *dto.MetricFamily, m *dto.Metric) metricSeries {
	s := metricSeries{
		name:   family.GetName(),
		labels: make(map[string]string, len(m.GetLabel())),
	}
	for _, lp := range m.GetLabel() {
		s.labels[lp.GetName()] = lp.GetValue()
	}
	switch family.GetType() {
	case dto.MetricType_COUNTER:
		s.typ, s.value = parity.MetricCounter, m.GetCounter().GetValue()
	case dto.MetricType_GAUGE:
		s.typ, s.value = parity.MetricGauge, m.GetGauge().GetValue()
	case dto.MetricType_HISTOGRAM:
		h := m.GetHistogram()
		s.typ, s.value, s.count = parity.MetricHistogram, h.GetSampleSum(), h.GetSampleCount()
		s.buckets = make(map[string]uint64, len(h.GetBucket()))
		for _, b := range h.GetBucket() {
			s.buckets[formatBound(b.GetUpperBound())] = b.GetCumulativeCount()
		}
	case dto.MetricType_SUMMARY:
		sum := m.GetSummary()
		s.typ, s.value, s.count = parity.MetricSummary, sum.GetSampleSum(), sum.GetSampleCount()
	default:
		s.typ, s.value = parity.MetricUntyped, m.GetUntyped().GetValue()
	}
	return s
}

// formatBound renders a bucket upper bound the way the Prometheus text
// exposition format does ("+Inf" for infinity, shortest float otherwise).
func formatBound(le float64) string {
	if math.IsInf(le, 1) {
		return "+Inf"
	}
	return strconv.FormatFloat(le, 'g', -1, 64)
}

func seriesKey(name string, labels map[string]string) string {
	names := make([]string, 0, len(labels))
	for k := range labels {
		names = append(names, k)
	}
	sort.Strings(names)
	var b strings.Builder
	b.WriteString(name)
	b.WriteByte('{')
	for i, k := range names {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteString(k)
		b.WriteString("=")
		b.WriteString(strconv.Quote(labels[k]))
	}
	b.WriteByte('}')
	return b.String()
}
