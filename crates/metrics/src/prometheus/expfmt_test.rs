//! Go: `github.com/prometheus/common/expfmt` `text_create_test.go` cases for the metric types
//! the engine produces.

use std::error::Error;

use super::{FMT_TEXT, encode_text, metric_family_to_text, write_float};
use crate::prometheus::registry::{
    Bucket, Histogram, LabelPair, Metric, MetricFamily, MetricType, MetricValue,
};

type TestResult = Result<(), Box<dyn Error>>;

fn pair(name: &str, value: &str) -> LabelPair {
    LabelPair {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn float(f: f64) -> String {
    let mut out = String::new();
    write_float(&mut out, f);
    out
}

/// Go: `writeFloat` special cases and the `'g'` formatting of everything else.
#[test]
fn write_float_matches_go() -> TestResult {
    let cases = [
        (1.0, "1"),
        (0.0, "0"),
        (-0.0, "0"),
        (-1.0, "-1"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (0.05, "0.05"),
        (2.5, "2.5"),
        (75000.0, "75000"),
        (1e6, "1e+06"),
        (1_234_567.0, "1.234567e+06"),
        (1e-5, "1e-05"),
        (0.0001, "0.0001"),
        (-3.0, "-3"),
    ];
    for (f, expected) in cases {
        ensure_eq!(float(f), expected, "{f:?}");
    }
    Ok(())
}

/// Go: `TestCreate` "counter" and "counter with escaped help and label values".
#[test]
fn counter_family_text() -> TestResult {
    let family = MetricFamily {
        name: "name".to_owned(),
        help: "two-line\n doc  str\\ing".to_owned(),
        metric_type: MetricType::Counter,
        metrics: vec![
            Metric {
                labels: vec![pair("labelname", "val1"), pair("basename", "basevalue")],
                value: MetricValue::Counter(f64::NAN),
            },
            Metric {
                labels: vec![pair("labelname", "val2"), pair("basename", "basevalue")],
                value: MetricValue::Counter(0.23),
            },
            Metric {
                labels: vec![
                    pair("labelname", "va\"l\\u\ne"),
                    pair("basename", "basevalue"),
                ],
                value: MetricValue::Counter(-1.0),
            },
        ],
    };
    let mut out = String::new();
    metric_family_to_text(&mut out, &family);
    ensure_eq!(
        out,
        "# HELP name two-line\\n doc  str\\\\ing\n\
         # TYPE name counter\n\
         name{labelname=\"val1\",basename=\"basevalue\"} NaN\n\
         name{labelname=\"val2\",basename=\"basevalue\"} 0.23\n\
         name{labelname=\"va\\\"l\\\\u\\ne\",basename=\"basevalue\"} -1\n",
        "counter text"
    );
    Ok(())
}

/// Go: `TestCreate` "histogram" (without timestamps, which the engine never sets).
#[test]
fn histogram_family_text() -> TestResult {
    let family = MetricFamily {
        name: "request_duration_microseconds".to_owned(),
        help: "The response latency.".to_owned(),
        metric_type: MetricType::Histogram,
        metrics: vec![Metric {
            labels: Vec::new(),
            value: MetricValue::Histogram(Box::new(Histogram {
                sample_count: 2693,
                sample_sum: 1_756_047.3,
                buckets: vec![
                    Bucket {
                        cumulative_count: 123,
                        upper_bound: 100.0,
                    },
                    Bucket {
                        cumulative_count: 412,
                        upper_bound: 120.0,
                    },
                    Bucket {
                        cumulative_count: 592,
                        upper_bound: 144.0,
                    },
                    Bucket {
                        cumulative_count: 2693,
                        upper_bound: 172.8,
                    },
                ],
            })),
        }],
    };
    ensure_eq!(
        encode_text(&[family]),
        "# HELP request_duration_microseconds The response latency.\n\
         # TYPE request_duration_microseconds histogram\n\
         request_duration_microseconds_bucket{le=\"100\"} 123\n\
         request_duration_microseconds_bucket{le=\"120\"} 412\n\
         request_duration_microseconds_bucket{le=\"144\"} 592\n\
         request_duration_microseconds_bucket{le=\"172.8\"} 2693\n\
         request_duration_microseconds_bucket{le=\"+Inf\"} 2693\n\
         request_duration_microseconds_sum 1.7560473e+06\n\
         request_duration_microseconds_count 2693\n",
        "histogram text"
    );
    Ok(())
}

/// Go: `TestCreate` "histogram with missing +Inf bucket" keeps an explicit `+Inf` bucket as is.
#[test]
fn histogram_with_explicit_inf_bucket_is_not_duplicated() -> TestResult {
    let family = MetricFamily {
        name: "h".to_owned(),
        help: "h".to_owned(),
        metric_type: MetricType::Histogram,
        metrics: vec![Metric {
            labels: vec![pair("a", "b")],
            value: MetricValue::Histogram(Box::new(Histogram {
                sample_count: 3,
                sample_sum: 0.5,
                buckets: vec![
                    Bucket {
                        cumulative_count: 1,
                        upper_bound: 1.0,
                    },
                    Bucket {
                        cumulative_count: 3,
                        upper_bound: f64::INFINITY,
                    },
                ],
            })),
        }],
    };
    ensure_eq!(
        encode_text(&[family]),
        "# HELP h h\n\
         # TYPE h histogram\n\
         h_bucket{a=\"b\",le=\"1\"} 1\n\
         h_bucket{a=\"b\",le=\"+Inf\"} 3\n\
         h_sum{a=\"b\"} 0.5\n\
         h_count{a=\"b\"} 3\n",
        "explicit +Inf"
    );
    Ok(())
}

/// Families without series are not written.
#[test]
fn empty_family_is_skipped() -> TestResult {
    let family = MetricFamily {
        name: "empty".to_owned(),
        help: "nothing".to_owned(),
        metric_type: MetricType::Counter,
        metrics: Vec::new(),
    };
    ensure_eq!(encode_text(&[family]), "", "empty family");
    ensure_eq!(
        FMT_TEXT,
        "text/plain; version=0.0.4; charset=utf-8",
        "content type"
    );
    Ok(())
}
