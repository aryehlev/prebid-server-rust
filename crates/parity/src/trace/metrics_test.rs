use std::collections::BTreeMap;

use pbs_metrics::prometheus::registry::{
    Bucket, Histogram, LabelPair, Metric, MetricFamily, MetricType, MetricValue,
};
use serde_json::Value;

use super::{MetricKind, MetricsSnapshot, Series, format_bound, series_key, value_delta};
use crate::{
    golden::{Json, TestResult, check, check_eq, check_same_json, golden_json},
    tracedoc::MetricDelta,
};

fn labels(pairs: &[(&str, &str)]) -> Vec<LabelPair> {
    pairs
        .iter()
        .map(|(name, value)| LabelPair {
            name: (*name).to_owned(),
            value: (*value).to_owned(),
        })
        .collect()
}

fn counter(name: &str, series: &[(&[(&str, &str)], f64)]) -> MetricFamily {
    MetricFamily {
        name: name.to_owned(),
        help: String::new(),
        metric_type: MetricType::Counter,
        metrics: series
            .iter()
            .map(|(l, v)| Metric {
                labels: labels(l),
                value: MetricValue::Counter(*v),
            })
            .collect(),
    }
}

fn histogram(name: &str, sum: f64, count: u64, buckets: &[(f64, u64)]) -> MetricFamily {
    MetricFamily {
        name: name.to_owned(),
        help: String::new(),
        metric_type: MetricType::Histogram,
        metrics: vec![Metric {
            labels: Vec::new(),
            value: MetricValue::Histogram(Box::new(Histogram {
                sample_count: count,
                sample_sum: sum,
                buckets: buckets
                    .iter()
                    .map(|(le, c)| Bucket {
                        cumulative_count: *c,
                        upper_bound: *le,
                    })
                    .collect(),
            })),
        }],
    }
}

fn gauge(value: f64) -> Series {
    Series {
        name: "connections".to_owned(),
        kind: MetricKind::Gauge,
        value,
        ..Series::default()
    }
}

// Go: parity/trace/session_test.go TestMetricsDelta.
#[test]
fn delta_reports_changed_series() -> TestResult {
    let unchanged: &[(&str, &str)] = &[];
    let mut before = MetricsSnapshot::from_families(&[
        counter("requests", &[(&[("type", "amp")], 2.0)]),
        histogram("latency_seconds", 0.0, 0, &[(0.1, 0), (1.0, 0)]),
        counter("unchanged", &[(unchanged, 1.0)]),
    ]);
    before.insert(gauge(0.0));
    let mut after = MetricsSnapshot::from_families(&[
        counter(
            "requests",
            &[(&[("type", "amp")], 3.0), (&[("type", "web")], 3.0)],
        ),
        histogram("latency_seconds", 5.5, 2, &[(0.1, 0), (1.0, 1)]),
        counter("unchanged", &[(unchanged, 1.0)]),
    ]);
    after.insert(gauge(-2.0));
    let got = before.delta(&after);
    let want = vec![
        MetricDelta {
            name: "connections".to_owned(),
            metric_type: "gauge".to_owned(),
            labels: Some(BTreeMap::new()),
            delta: -2.0,
            count_delta: None,
            bucket_deltas: None,
        },
        MetricDelta {
            name: "latency_seconds".to_owned(),
            metric_type: "histogram".to_owned(),
            labels: Some(BTreeMap::new()),
            delta: 5.5,
            count_delta: Some(2),
            bucket_deltas: Some(BTreeMap::from([("1".to_owned(), 1)])),
        },
        MetricDelta {
            name: "requests".to_owned(),
            metric_type: "counter".to_owned(),
            labels: Some(BTreeMap::from([("type".to_owned(), "amp".to_owned())])),
            delta: 1.0,
            count_delta: None,
            bucket_deltas: None,
        },
        MetricDelta {
            name: "requests".to_owned(),
            metric_type: "counter".to_owned(),
            labels: Some(BTreeMap::from([("type".to_owned(), "web".to_owned())])),
            delta: 3.0,
            count_delta: None,
            bucket_deltas: None,
        },
    ];
    check_eq(&got, &want, "delta")?;
    check_eq(
        &MetricsSnapshot::new().delta(&MetricsSnapshot::new()),
        &Vec::new(),
        "empty",
    )?;
    check_eq(
        &MetricsSnapshot::gather(None).map_err(|e| e.to_string()),
        &Err("parity: no Prometheus gatherer (metrics.prometheus.port is 0)".to_owned()),
        "no registry",
    )
}

// Go: TestValueDelta and TestFormatBound.
#[test]
fn values_and_bounds() -> TestResult {
    check_eq(&value_delta(1.5, 4.0), &2.5, "finite")?;
    check_eq(&value_delta(3.0, 3.0), &0.0, "unchanged")?;
    check_eq(&value_delta(f64::NAN, f64::NAN), &0.0, "both NaN")?;
    check_eq(&value_delta(1.0, f64::NAN), &0.0, "becomes NaN")?;
    check_eq(&value_delta(1.0, f64::INFINITY), &0.0, "becomes infinite")?;
    check_eq(
        &value_delta(f64::INFINITY, f64::INFINITY),
        &0.0,
        "both infinite",
    )?;
    // Go 1.26.2's strconv.FormatFloat(le, 'g', -1, 64) of each bound (and `+Inf`).
    for (le, go) in [
        (0.005, "0.005"),
        (1.0, "1"),
        (2.5, "2.5"),
        (1e21, "1e+21"),
        (1e6, "1e+06"),
        (123_456.0, "123456"),
        (1_234_567.0, "1.234567e+06"),
        (999_999.0, "999999"),
        (12_345_678.0, "1.2345678e+07"),
        (123_456.7, "123456.7"),
        (0.0001, "0.0001"),
        (0.000_01, "1e-05"),
        (1.5e-7, "1.5e-07"),
        (5e-324, "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
        (0.0, "0"),
        (-0.0, "-0"),
        (-1e6, "-1e+06"),
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
    ] {
        check_eq(&format_bound(le), &go.to_owned(), go)?;
    }
    check_eq(
        &series_key(
            "requests",
            &BTreeMap::from([
                ("z".to_owned(), "\"q\"".to_owned()),
                ("a".to_owned(), String::new()),
            ]),
        ),
        &r#"requests{a="",z="\"q\""}"#.to_owned(),
        "series key",
    )?;
    check_eq(&MetricKind::default().as_str(), &"untyped", "default kind")
}

fn kind_of(name: &str) -> Result<MetricKind, String> {
    [
        MetricKind::Counter,
        MetricKind::Gauge,
        MetricKind::Histogram,
        MetricKind::Summary,
        MetricKind::Untyped,
    ]
    .into_iter()
    .find(|k| k.as_str() == name)
    .ok_or_else(|| format!("unknown metric type {name}"))
}

/// The snapshot of the neutral series the Go program gathered: counters and histograms go
/// through [`MetricsSnapshot::from_families`] (what a pbs-metrics registry gathers), the other
/// types through [`MetricsSnapshot::insert`].
fn snapshot_of(series: &[Json]) -> Result<MetricsSnapshot, String> {
    let mut families = Vec::new();
    let mut others = Vec::new();
    for s in series {
        let name = s.string("name")?;
        let kind = kind_of(&s.string("type")?)?;
        let pairs: Vec<LabelPair> = s.get("labels")?.0.as_object().map_or_default(|m| {
            m.iter()
                .map(|(k, v)| LabelPair {
                    name: k.clone(),
                    value: v.as_str().unwrap_or_default().to_owned(),
                })
                .collect()
        });
        let value: f64 = s
            .string("value")?
            .parse()
            .map_err(|e| format!("value of {name}: {e}"))?;
        let count = s
            .get("count")?
            .0
            .as_u64()
            .ok_or_else(|| format!("count of {name}"))?;
        let buckets: Vec<(f64, u64)> = s
            .array("buckets")?
            .iter()
            .map(|b| {
                // A number, or (the generated registries) its exact text.
                let le = b.get("le")?.0;
                let le = match le.as_str() {
                    Some(text) => text.parse().map_err(|e| format!("le {text}: {e}"))?,
                    None => le.as_f64().ok_or("le")?,
                };
                let c = b.get("count")?.0.as_u64().ok_or("bucket count")?;
                Ok((le, c))
            })
            .collect::<Result<_, String>>()?;
        match kind {
            MetricKind::Counter => families.push(MetricFamily {
                name,
                help: String::new(),
                metric_type: MetricType::Counter,
                metrics: vec![Metric {
                    labels: pairs,
                    value: MetricValue::Counter(value),
                }],
            }),
            MetricKind::Histogram => families.push(MetricFamily {
                name,
                help: String::new(),
                metric_type: MetricType::Histogram,
                metrics: vec![Metric {
                    labels: pairs,
                    value: MetricValue::Histogram(Box::new(Histogram {
                        sample_count: count,
                        sample_sum: value,
                        buckets: buckets
                            .iter()
                            .map(|(le, c)| Bucket {
                                cumulative_count: *c,
                                upper_bound: *le,
                            })
                            .collect(),
                    })),
                }],
            }),
            MetricKind::Gauge | MetricKind::Summary | MetricKind::Untyped => others.push(Series {
                name,
                kind,
                labels: pairs.into_iter().map(|p| (p.name, p.value)).collect(),
                value,
                count,
                buckets: buckets
                    .iter()
                    .map(|(le, c)| (format_bound(*le), *c))
                    .collect(),
            }),
        }
    }
    let mut snap = MetricsSnapshot::from_families(&families);
    for s in others {
        snap.insert(s);
    }
    Ok(snap)
}

/// The deltas of a `metrics_delta` list sorted by metric name and labels, as JSON.
fn by_series(deltas: &[u8]) -> Result<Vec<u8>, String> {
    let mut list: Vec<Value> = serde_json::from_slice(deltas).map_err(|e| e.to_string())?;
    list.sort_by_cached_key(|d| {
        (
            d.get("name").map(ToString::to_string),
            d.get("labels").map(ToString::to_string),
        )
    });
    serde_json::to_vec(&list).map_err(|e| e.to_string())
}

// Generated registries (`metricsfuzz.go`): every metric type, metric and label names that prefix
// each other, label values that need quoting, NaN, infinite and denormal values and random
// bucket layouts, changed between two gathers. Each delta is Go's; where a label value holds a
// control or unprintable character, which Rust's `{:?}` quotes otherwise than Go's
// `strconv.Quote`, the deltas can come in another order (metrics.parity-delta-order).
#[test]
fn fuzzed_deltas_match_go() -> TestResult {
    let cases = golden_json("metrics_fuzz.json")?
        .get("golden")?
        .elements()?;
    check_eq(&cases.len(), &150, "registries")?;
    let mut reordered = 0;
    for (i, case) in cases.iter().enumerate() {
        let before = snapshot_of(&case.array("before")?)?;
        let after = snapshot_of(&case.array("after")?)?;
        let delta = serde_json::to_vec(&before.delta(&after)).map_err(|e| e.to_string())?;
        let go = case.string("delta")?;
        let what = format!("registry {i}");
        if check_same_json(&delta, go.as_bytes(), &what).is_err() {
            check_same_json(&by_series(&delta)?, &by_series(go.as_bytes())?, &what)?;
            reordered += 1;
        }
    }
    check_eq(
        &reordered,
        &3,
        "registries whose deltas come in another order",
    )
}

// The deltas are ordered by their series keys, which quote each label value: Go with
// `strconv.Quote`, the port with `{:?}`. The two agree on printable text and differ on a control
// or unprintable character: Go writes a form feed `\f` and U+001F `\x1f`, so `a\f` comes first
// ('f' < 'x'); Rust writes `\u{c}` and `\u{1f}`, so `a\u{1f}` does ('1' < 'c')
// (metrics.parity-delta-order).
#[test]
fn delta_order_follows_rust_quoting() -> TestResult {
    let key = |v: &str| series_key("m", &BTreeMap::from([("l".to_owned(), v.to_owned())]));
    check_eq(
        &key("a<b> \"q\""),
        &r#"m{l="a<b> \"q\""}"#.to_owned(),
        "printable",
    )?;
    check_eq(&key("a\u{c}"), &r#"m{l="a\u{c}"}"#.to_owned(), "form feed")?;
    check_eq(&key("a\u{1f}"), &r#"m{l="a\u{1f}"}"#.to_owned(), "U+001F")?;
    check(key("a\u{1f}") < key("a\u{c}"), "U+001F first")
}

// The metrics_delta Go computes for a registry with every metric type, NaN sums, an exponent
// bound and labels that need quoting.
#[test]
fn delta_matches_go() -> TestResult {
    let golden = golden_json("metrics.json")?.get("golden")?;
    let before = snapshot_of(&golden.array("before")?)?;
    let after = snapshot_of(&golden.array("after")?)?;
    let delta = serde_json::to_vec(&before.delta(&after)).map_err(|e| e.to_string())?;
    check_same_json(&delta, golden.string("delta")?.as_bytes(), "delta")?;
    let empty = serde_json::to_vec(&MetricsSnapshot::new().delta(&MetricsSnapshot::new()))
        .map_err(|e| e.to_string())?;
    check_same_json(&empty, golden.string("empty")?.as_bytes(), "empty")?;
    check_eq(
        &MetricsSnapshot::gather(None).err().map(|e| e.to_string()),
        &Some(golden.string("nil_gather")?),
        "no registry",
    )?;
    check(before.series.len() > 5, "the golden covers every type")
}
