//! What the server observes while one parity case is in flight (Go: `parity/trace`).
//!
//! Exactly one case runs per process at a time. In parity mode (the `trace` feature) the exec
//! handler runs each case inside `begin_case`, which takes a process-wide exec lock and publishes
//! the [`Session`] that [`current`] returns. Product code reaches the session through one-line
//! hooks:
//!
//! ```
//! use pbs_parity::{StageId, trace};
//!
//! let value = 7_isize;
//! if let Some(s) = trace::current() {
//!     s.record(StageId::BidderRequest, "appnexus", &value);
//! }
//! // or, when the recorded value still has to be assembled from values the code already built:
//! trace::with(|s| s.record_with(StageId::BidderRequest, "appnexus", || vec![value, value]));
//! ```
//!
//! Without the feature [`current`] is a `const fn` returning `None` and [`Session`] cannot be
//! constructed, so both hooks compile to nothing and the recorded values are never built. With
//! the feature, a hook outside a case costs one atomic load.
//!
//! The decision points ([`decide`], [`decide_bool`], [`decide_chance`], [`decide_string`],
//! [`intn`]) return the envelope's pinned value during a case and otherwise call their default
//! closure, which holds the original random computation; without the feature they call it
//! directly.
//!
//! Every [`Session`] method is safe to call concurrently (bidder tasks record concurrently).
//!
//! Observer-effect rule: hooks only snapshot values the code has already built. They never
//! rebuild a request wrapper or re-marshal live request state for tracing, and never mutate
//! state for tracing purposes.

mod errors;
#[cfg(not(feature = "trace"))]
mod inert;
#[cfg(feature = "trace")]
mod live;
#[cfg(feature = "trace")]
mod metrics;

pub use self::errors::{error_record_of, error_records_of, nil_error_record};
#[cfg(not(feature = "trace"))]
pub use self::inert::{Session, Tracked, current};
#[cfg(feature = "trace")]
pub use self::live::{Case, Session, Tracked, begin_case, current};
#[cfg(feature = "trace")]
pub use self::metrics::{GatherError, MetricKind, MetricsSnapshot, Series};
use crate::ids::DecisionId;

/// Runs `f` with the in-flight session, if a parity case is running.
///
/// Without the `trace` feature `f` is never called (and is optimized away).
#[inline]
pub fn with<R, F: FnOnce(&Session) -> R>(f: F) -> Option<R> {
    current().map(|s| f(&s))
}

/// The semantic decision point (Go: `Session.Decide`).
///
/// During a case it returns the envelope's integer override for `point` (validated against the
/// point's value domain); otherwise `def()`, the value computed from the real random source.
#[inline]
pub fn decide<F: FnOnce() -> isize>(point: DecisionId, def: F) -> isize {
    match current() {
        Some(s) => s.decide(point, def),
        None => def(),
    }
}

/// [`decide`] for a binary point whose Go site computes a boolean (Go: `Session.DecideBool`):
/// `draw` is the original random computation and `true` is the decision value 1.
#[inline]
pub fn decide_bool<F: FnOnce() -> bool>(point: DecisionId, draw: F) -> bool {
    match current() {
        Some(s) => s.decide_bool(point, draw),
        None => draw(),
    }
}

/// [`decide_bool`] for a binary point whose Go site draws against a configured rate
/// (Go: `Session.DecideChance`).
///
/// `uncertain` reports whether `draw` can return both outcomes for the configured rates; when
/// it cannot, the envelope value is ignored (see [`Session::decide_chance`]).
#[inline]
pub fn decide_chance<F: FnOnce() -> bool>(point: DecisionId, uncertain: bool, draw: F) -> bool {
    match current() {
        Some(s) => s.decide_chance(point, uncertain, draw),
        None => draw(),
    }
}

/// The string-valued decision point (Go: `Session.DecideString`): the envelope override when it
/// is one of the point's allowed values, otherwise `def()`.
#[inline]
pub fn decide_string<F: FnOnce() -> String>(point: DecisionId, def: F) -> String {
    match current() {
        Some(s) => s.decide_string(point, def),
        None => def(),
    }
}

/// The index-valued decision point (Go: `Session.Intn`): the envelope override for `point` when
/// it is an integer in `[0, n)`, otherwise `def(n)`.
#[inline]
pub fn intn<F: FnOnce(isize) -> isize>(point: DecisionId, n: isize, def: F) -> isize {
    match current() {
        Some(s) => s.intn(point, n, def),
        None => def(n),
    }
}

#[cfg(test)]
#[path = "trace_test.rs"]
mod tests;
