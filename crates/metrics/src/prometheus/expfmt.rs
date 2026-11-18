//! The Prometheus text exposition format (version 0.0.4).
//!
//! Go: `github.com/prometheus/common/expfmt` v0.32.1 (`text_create.go`), which
//! `promhttp.HandlerFor` uses for every scrape that does not negotiate protobuf.

use super::registry::{Histogram, Metric, MetricFamily, MetricType, MetricValue};
use crate::goconv;

/// The `Content-Type` of a text scrape.
///
/// Go: `expfmt.FmtText` (`text/plain; version=0.0.4; charset=utf-8`).
pub const FMT_TEXT: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Encodes gathered families one after another, as `promhttp` writes a text scrape.
#[must_use]
pub fn encode_text(families: &[MetricFamily]) -> String {
    let mut out = String::new();
    for family in families {
        metric_family_to_text(&mut out, family);
    }
    out
}

/// Go: `expfmt.MetricFamilyToText`. Families without series are skipped (Go returns an error,
/// and a gathered family always has series).
pub fn metric_family_to_text(out: &mut String, family: &MetricFamily) {
    if family.metrics.is_empty() || family.name.is_empty() {
        return;
    }
    let name = family.name.as_str();
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    write_escaped(out, &family.help, false);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(match family.metric_type {
        MetricType::Counter => " counter\n",
        MetricType::Histogram => " histogram\n",
    });
    for metric in &family.metrics {
        match &metric.value {
            MetricValue::Counter(value) => write_sample(out, name, "", metric, None, *value),
            MetricValue::Histogram(histogram) => write_histogram(out, name, metric, histogram),
        }
    }
}

fn write_histogram(out: &mut String, name: &str, metric: &Metric, histogram: &Histogram) {
    let mut inf_seen = false;
    for bucket in &histogram.buckets {
        let count = goconv::u64_to_f64(bucket.cumulative_count);
        write_sample(
            out,
            name,
            "_bucket",
            metric,
            Some(bucket.upper_bound),
            count,
        );
        if bucket.upper_bound.is_infinite() && bucket.upper_bound > 0.0 {
            inf_seen = true;
        }
    }
    let count = goconv::u64_to_f64(histogram.sample_count);
    if !inf_seen {
        write_sample(out, name, "_bucket", metric, Some(f64::INFINITY), count);
    }
    write_sample(out, name, "_sum", metric, None, histogram.sample_sum);
    write_sample(out, name, "_count", metric, None, count);
}

/// Go: `writeSample`; `le` is the additional bucket label.
fn write_sample(
    out: &mut String,
    name: &str,
    suffix: &str,
    metric: &Metric,
    le: Option<f64>,
    value: f64,
) {
    out.push_str(name);
    out.push_str(suffix);
    if !metric.labels.is_empty() || le.is_some() {
        let mut separator = '{';
        for pair in &metric.labels {
            out.push(separator);
            out.push_str(&pair.name);
            out.push_str("=\"");
            write_escaped(out, &pair.value, true);
            out.push('"');
            separator = ',';
        }
        if let Some(le) = le {
            out.push(separator);
            out.push_str("le=\"");
            write_float(out, le);
            out.push('"');
        }
        out.push('}');
    }
    out.push(' ');
    write_float(out, value);
    out.push('\n');
}

/// Go: `writeEscapedString` — `\` and newline are escaped, and `"` too inside label values.
fn write_escaped(out: &mut String, value: &str, include_double_quote: bool) {
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' if include_double_quote => out.push_str("\\\""),
            c => out.push(c),
        }
    }
}

/// Go: `writeFloat` — fixed spellings for 1, 0, -1, NaN and the infinities, otherwise
/// `strconv.AppendFloat(f, 'g', -1, 64)`.
pub(crate) fn write_float(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("NaN");
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "+Inf" } else { "-Inf" });
    } else if f.total_cmp(&1.0).is_eq() {
        out.push('1');
    } else if f.total_cmp(&-1.0).is_eq() {
        out.push_str("-1");
    } else if f == 0.0 {
        // Go compares `f == 0`, which also holds for negative zero.
        out.push('0');
    } else {
        out.push_str(&goconv::format_float_g(f));
    }
}

#[cfg(test)]
#[path = "expfmt_test.rs"]
mod tests;
