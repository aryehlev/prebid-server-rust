//! Prebid Server metrics engine with Go-identical metric names and labels.
//!
//! A port of the Go `metrics/` tree:
//!
//! - [`metrics`] (Go `metrics/metrics.go`): the label types with their exact string values, the
//!   label structs, and the [`MetricsEngine`] interface with all of Go's methods.
//! - [`prometheus`] (Go `metrics/prometheus/`): the Prometheus engine on the prometheus crate.
//!   Family names, help strings, label names, label values, types, buckets, the disabled-metric
//!   toggles and the startup preloading are Go's; [`prometheus::Metrics::text_exposition`] is the
//!   crate's text format of the registry, served as [`prometheus::TEXT_FORMAT`], with the same
//!   families, samples and values as Go's `client_golang` v1.12.1 scrape (the ledger's `metrics`
//!   entries name what is spelled or negotiated otherwise).
//! - [`config`] (Go `metrics/config/metrics.go`): engine selection ([`new_metrics_engine`]),
//!   [`MultiMetricsEngine`] fan-out and [`NilMetricsEngine`].
//! - [`metrics_mock`] (Go `metrics/metrics_mock.go`): a recording mock for tests.
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
//! - The HTTP handler (`promhttp`) and its content negotiation, which belong to the server.

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
pub mod gometrics;
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

// Integer to float conversions as Go's `float64(n)` makes them: the nearest `f64`, ties to even.
// `n` is split into `hi * 2^32 + lo`, with `hi` in `i32`'s or `u32`'s range and `lo` in `u32`'s:
// both convert exactly, `hi * 2^32` is exact, and the fused multiply-add rounds the exact sum
// once, which is the rounding Go's conversion (and std's `as`, which the workspace's lints rule
// out) applies.

/// 2^32.
const TWO_POW_32: f64 = 4_294_967_296.0;

/// Go: `float64(n)` of an `int64`.
pub(crate) fn float_from_i64(n: i64) -> f64 {
    let hi = i32::try_from(n >> 32).unwrap_or_default();
    let lo = u32::try_from(n & 0xFFFF_FFFF).unwrap_or_default();
    f64::from(hi).mul_add(TWO_POW_32, f64::from(lo))
}

/// Go: `float64(n)` of a `uint64`.
pub(crate) fn float_from_u64(n: u64) -> f64 {
    let hi = u32::try_from(n >> 32).unwrap_or_default();
    let lo = u32::try_from(n & 0xFFFF_FFFF).unwrap_or_default();
    f64::from(hi).mul_add(TWO_POW_32, f64::from(lo))
}

/// Go: `float64(n)` of an `int` that is not negative, which the port holds as a `usize`.
pub(crate) fn float_from_usize(n: usize) -> f64 {
    float_from_u64(u64::try_from(n).unwrap_or(u64::MAX))
}

/// 2^63 (`0x43E0_0000_0000_0000` is its IEEE-754 encoding).
const TWO_POW_63: f64 = f64::from_bits(0x43E0_0000_0000_0000);

/// Go: `int64(f)` as amd64 computes it (`CVTTSD2SQ`, D-ARCH): the value truncated toward zero,
/// and `math.MinInt64` for NaN and for values outside `int64`'s range, where std's `as`
/// saturates.
pub(crate) fn i64_from_float(f: f64) -> i64 {
    if f.is_nan() || f >= TWO_POW_63 || f < -TWO_POW_63 {
        return i64::MIN;
    }
    let bits = f.to_bits();
    let exponent = i64::try_from((bits >> 52) & 0x7FF).unwrap_or_default() - 1023;
    if exponent < 0 {
        return 0;
    }
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let magnitude = if exponent >= 52 {
        mantissa << u32::try_from(exponent - 52).unwrap_or_default()
    } else {
        mantissa >> u32::try_from(52 - exponent).unwrap_or_default()
    };
    // -2^63 is the one magnitude that does not fit, and its negation is itself.
    let magnitude = i64::try_from(magnitude).unwrap_or(i64::MIN);
    if f.is_sign_negative() {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

#[cfg(test)]
#[path = "float_test.rs"]
mod float_tests;

/// What the unit tests share; public in test builds only.
#[cfg(test)]
pub mod testutil;
