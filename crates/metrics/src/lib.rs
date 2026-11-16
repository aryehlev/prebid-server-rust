//! Prebid Server metrics engine with Go-identical metric names and labels.
//!
//! A port of the Go `metrics/` tree:
//!
//! - [`metrics`] (Go `metrics/metrics.go`): the label types with their exact string values, the
//!   label structs, and the [`MetricsEngine`] interface with all of Go's methods.
//! - [`metrics_mock`] (Go `metrics/metrics_mock.go`): a recording mock for tests.
//! - [`goconv`]: the Go `strconv`/`strings` conversions the engines need.
//! - [`options`]: the `metrics` section of the host configuration as plain structs (Go
//!   `config.Metrics`, `config.PrometheusMetrics`, `config.DisabledMetrics`, ...).
//!
//! Bidder (adapter), syncer and module names are parameters: the engines never read the bidder
//! registry themselves.

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

pub mod goconv;
pub mod metrics;
pub mod metrics_mock;
pub mod options;

pub use self::metrics::{GoDuration, MetricsEngine};
