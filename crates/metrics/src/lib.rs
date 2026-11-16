//! Prebid Server metrics engine with Go-identical metric names and labels.
//!
//! A port of the Go `metrics/` tree:
//!
//! - [`metrics`] (Go `metrics/metrics.go`): the label types with their exact string values, the
//!   label structs, and the [`MetricsEngine`] interface with all of Go's methods.
//! - [`prometheus`] (Go `metrics/prometheus/`): the Prometheus engine. Family names, help strings,
//!   label names, label values, buckets, the disabled-metric toggles and the startup preloading are
//!   Go's, and [`prometheus::Metrics::text_exposition`] (or [`prometheus::expfmt`] over
//!   [`prometheus::registry::Registry::gather`]) produces the same bytes as Go's `client_golang`
//!   v1.12.1 text scrape, with Content-Type [`prometheus::expfmt::FMT_TEXT`].
//! - [`config`] (Go `metrics/config/metrics.go`): engine selection ([`new_metrics_engine`]),
//!   [`MultiMetricsEngine`] fan-out and [`NilMetricsEngine`].
//! - [`metrics_mock`] (Go `metrics/metrics_mock.go`): a recording mock for tests.
//! - [`goconv`]: the Go `strconv`/`strings` conversions the engines need.
//! - [`options`]: the `metrics` section of the host configuration as plain structs (Go
//!   `config.Metrics`, `config.PrometheusMetrics`, `config.DisabledMetrics`, ...).
//!
//! Bidder (adapter), syncer and module names are parameters: the engines never read the bidder
//! registry themselves.
//!
//! Not ported:
//!
//! - The go-metrics engine and its `InfluxDB` reporter (Go `metrics/go_metrics.go`);
//!   [`new_metrics_engine`] returns [`MetricsEngineError::InfluxNotSupported`] when
//!   `metrics.influxdb.host` is set.
//! - Go's runtime collector (`collectors.NewGoCollector()`, the `<prefix>go_*` families).
//! - The protobuf exposition format and the HTTP handler (`promhttp`), which belong to the server.

/// `assert!` for tests that return `Result`: returns an error with the message instead of
/// panicking.
#[cfg(test)]
macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err(format!("{}:{}: {}", file!(), line!(), format_args!($($msg)+)).into());
        }
    };
}

/// `assert_eq!` for tests that return `Result`: returns an error with both values and the message
/// instead of panicking.
#[cfg(test)]
macro_rules! ensure_eq {
    ($actual:expr, $expected:expr, $($msg:tt)+) => {
        // A `match` keeps temporaries in the operands alive, as in `assert_eq!`.
        match (&$actual, &$expected) {
            (actual, expected) => {
                if PartialEq::ne(actual, expected) {
                    return Err(format!(
                        "{}:{}: {}\n  actual: {:?}\nexpected: {:?}",
                        file!(),
                        line!(),
                        format_args!($($msg)+),
                        actual,
                        expected,
                    )
                    .into());
                }
            }
        }
    };
}

pub mod config;
pub mod goconv;
pub mod metrics;
pub mod metrics_mock;
pub mod options;
pub mod prometheus;

pub use self::{
    config::{
        DetailedMetricsEngine, MetricsEngineError, MultiMetricsEngine, NilMetricsEngine,
        new_metrics_engine,
    },
    metrics::{GoDuration, MetricsEngine},
};
