use std::collections::BTreeMap;

use pbs_gojson::stdjson;
use pbs_metrics::prometheus::registry::{
    Bucket, Histogram, LabelPair, Metric, MetricFamily, MetricType, MetricValue,
};

use super::{MetricKind, MetricsSnapshot, Series, format_bound, series_key, value_delta};
use crate::{
    golden::{Json, TestResult, check, check_eq, golden_json},
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
    check_eq(&format_bound(0.005), &"0.005".to_owned(), "0.005")?;
    check_eq(&format_bound(1.0), &"1".to_owned(), "1")?;
    check_eq(&format_bound(2.5), &"2.5".to_owned(), "2.5")?;
    check_eq(&format_bound(1e21), &"1e+21".to_owned(), "exponent")?;
    check_eq(&format_bound(f64::INFINITY), &"+Inf".to_owned(), "+Inf")?;
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

// Generated registries (`metricsfuzz.go`): every metric type, metric and label names that prefix
// each other, label values that need quoting, NaN, infinite and denormal values and random
// bucket layouts, changed between two gathers.
#[test]
fn fuzzed_deltas_match_go() -> TestResult {
    let cases = golden_json("metrics_fuzz.json")?
        .get("golden")?
        .elements()?;
    check_eq(&cases.len(), &150, "registries")?;
    for (i, case) in cases.iter().enumerate() {
        let before = snapshot_of(&case.array("before")?)?;
        let after = snapshot_of(&case.array("after")?)?;
        let delta = stdjson::marshal(&before.delta(&after)).map_err(|e| e.to_string())?;
        check_eq(
            &String::from_utf8_lossy(&delta).into_owned(),
            &case.string("delta")?,
            &format!("registry {i}"),
        )?;
    }
    Ok(())
}

// The metrics_delta Go computes for a registry with every metric type, NaN sums, an exponent
// bound and labels that need quoting.
#[test]
fn delta_matches_go() -> TestResult {
    let golden = golden_json("metrics.json")?.get("golden")?;
    let before = snapshot_of(&golden.array("before")?)?;
    let after = snapshot_of(&golden.array("after")?)?;
    let delta = stdjson::marshal(&before.delta(&after)).map_err(|e| e.to_string())?;
    check_eq(
        &String::from_utf8_lossy(&delta).into_owned(),
        &golden.string("delta")?,
        "delta",
    )?;
    let empty = stdjson::marshal(&MetricsSnapshot::new().delta(&MetricsSnapshot::new()))
        .map_err(|e| e.to_string())?;
    check_eq(
        &String::from_utf8_lossy(&empty).into_owned(),
        &golden.string("empty")?,
        "empty",
    )?;
    check_eq(
        &MetricsSnapshot::gather(None).err().map(|e| e.to_string()),
        &Some(golden.string("nil_gather")?),
        "no registry",
    )?;
    check(before.series.len() > 5, "the golden covers every type")
}
