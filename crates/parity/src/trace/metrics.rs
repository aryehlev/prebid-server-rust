//! The `metrics_delta` of a trace (Go: `parity/trace/metrics.go`).

use core::{error::Error, fmt};
use std::collections::BTreeMap;

use pbs_gocompat::strconv::{format_float, quote};
use pbs_metrics::prometheus::registry::{MetricFamily, MetricType, MetricValue, Registry};

use crate::tracedoc::{
    METRIC_COUNTER, METRIC_GAUGE, METRIC_HISTOGRAM, METRIC_SUMMARY, METRIC_UNTYPED, MetricDelta,
};

/// The type of a metric series (Go: the `dto.MetricType` cases of `toSeries`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MetricKind {
    /// A counter.
    Counter,
    /// A gauge.
    Gauge,
    /// A histogram.
    Histogram,
    /// A summary.
    Summary,
    /// An untyped metric.
    #[default]
    Untyped,
}

impl MetricKind {
    /// The kind as [`MetricDelta::metric_type`] spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Counter => METRIC_COUNTER,
            Self::Gauge => METRIC_GAUGE,
            Self::Histogram => METRIC_HISTOGRAM,
            Self::Summary => METRIC_SUMMARY,
            Self::Untyped => METRIC_UNTYPED,
        }
    }
}

/// One series of a snapshot (Go: `trace.metricSeries`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    /// The family name.
    pub name: String,
    /// The type.
    pub kind: MetricKind,
    /// The labels.
    pub labels: BTreeMap<String, String>,
    /// The value: the counter, gauge or untyped value, or the sample sum of a histogram or
    /// summary.
    pub value: f64,
    /// The sample count of a histogram or summary.
    pub count: u64,
    /// The cumulative bucket counts of a histogram, by upper bound as the Prometheus text
    /// format writes it.
    pub buckets: BTreeMap<String, u64>,
}

/// A metrics snapshot that could not be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatherError(&'static str);

impl fmt::Display for GatherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for GatherError {}

/// A point-in-time copy of every series of a registry, keyed by name and canonical labels
/// (Go: `trace.MetricsSnapshot`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricsSnapshot {
    series: BTreeMap<String, Series>,
}

impl MetricsSnapshot {
    /// An empty snapshot.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            series: BTreeMap::new(),
        }
    }

    /// Snapshots `registry` (Go: `trace.GatherMetrics`). The exec handler calls it before and
    /// after a case.
    ///
    /// # Errors
    ///
    /// Go's error when the server has no Prometheus registry.
    pub fn gather(registry: Option<&Registry>) -> Result<Self, GatherError> {
        registry
            .map(|r| Self::from_families(&r.gather()))
            .ok_or(GatherError(
                "parity: no Prometheus gatherer (metrics.prometheus.port is 0)",
            ))
    }

    /// Snapshots gathered metric families (Go: the loop of `trace.GatherMetrics`).
    #[must_use]
    pub fn from_families(families: &[MetricFamily]) -> Self {
        let mut snap = Self::new();
        for family in families {
            for metric in &family.metrics {
                let mut series = Series {
                    name: family.name.clone(),
                    labels: metric
                        .labels
                        .iter()
                        .map(|lp| (lp.name.clone(), lp.value.clone()))
                        .collect(),
                    ..Series::default()
                };
                match (&family.metric_type, &metric.value) {
                    (MetricType::Counter, value) => {
                        series.kind = MetricKind::Counter;
                        series.value = match value {
                            MetricValue::Counter(v) => *v,
                            MetricValue::Histogram(_) => 0.0,
                        };
                    }
                    (MetricType::Histogram, value) => {
                        series.kind = MetricKind::Histogram;
                        if let MetricValue::Histogram(h) = value {
                            series.value = h.sample_sum;
                            series.count = h.sample_count;
                            series.buckets = h
                                .buckets
                                .iter()
                                .map(|b| (format_bound(b.upper_bound), b.cumulative_count))
                                .collect();
                        }
                    }
                }
                snap.insert(series);
            }
        }
        snap
    }

    /// Adds `series`, replacing the series with the same name and labels.
    pub fn insert(&mut self, series: Series) {
        let _replaced = self
            .series
            .insert(series_key(&series.name, &series.labels), series);
    }

    /// The series that changed between `self` (before) and `after`, sorted by name and labels
    /// (Go: `trace.MetricsDelta`). Counters, gauges and untyped metrics report the value change;
    /// histograms and summaries report the sum change in `delta` plus the count change and
    /// (histograms only) the changed bucket counts. Series absent from `self` count as zero.
    #[must_use]
    pub fn delta(&self, after: &Self) -> Vec<MetricDelta> {
        let zero = Series::default();
        after
            .series
            .iter()
            .filter_map(|(key, a)| series_delta(self.series.get(key).unwrap_or(&zero), a))
            .collect()
    }
}

/// Go: `trace.seriesDelta`.
fn series_delta(before: &Series, after: &Series) -> Option<MetricDelta> {
    let mut d = MetricDelta {
        name: after.name.clone(),
        metric_type: after.kind.as_str().to_owned(),
        labels: Some(after.labels.clone()),
        delta: value_delta(before.value, after.value),
        count_delta: None,
        bucket_deltas: None,
    };
    if !matches!(after.kind, MetricKind::Histogram | MetricKind::Summary) {
        return (d.delta != 0.0).then_some(d);
    }
    let count_delta = after
        .count
        .cast_signed()
        .wrapping_sub(before.count.cast_signed());
    d.count_delta = Some(count_delta);
    if after.kind == MetricKind::Histogram {
        d.bucket_deltas = Some(
            after
                .buckets
                .iter()
                .filter_map(|(le, count)| {
                    let was = before.buckets.get(le).copied().unwrap_or(0);
                    let delta = count.cast_signed().wrapping_sub(was.cast_signed());
                    (delta != 0).then(|| (le.clone(), delta))
                })
                .collect(),
        );
    }
    (count_delta != 0 || d.delta != 0.0).then_some(d)
}

/// `after - before` when that is finite, else 0 (Go: `trace.valueDelta`): JSON cannot carry
/// NaN or infinities, and Go's runtime histograms always report a NaN sum.
fn value_delta(before: f64, after: f64) -> f64 {
    let d = after - before;
    if d.is_finite() { d } else { 0.0 }
}

/// A bucket upper bound as the Prometheus text format writes it (Go: `trace.formatBound`).
fn format_bound(le: f64) -> String {
    if le.is_infinite() && le.is_sign_positive() {
        return "+Inf".to_owned();
    }
    format_float(le, b'g', -1, 64)
}

/// Go: `trace.seriesKey`: the name plus the labels sorted by name, values quoted.
fn series_key(name: &str, labels: &BTreeMap<String, String>) -> String {
    let pairs: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect();
    format!("{name}{{{}}}", pairs.join(","))
}

#[cfg(test)]
#[path = "metrics_test.rs"]
mod tests;
