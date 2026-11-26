//! Ports of the Prebid Server Go `util` packages, one module per Go package, on std and crates,
//! with the parts of Go's standard library whose results Prebid's output depends on.
//!
//! `util/jsonutil` lives in `pbs-jsonutil`. Everything else that non-test Go code uses is here:
//!
//! - [`iputil`]: `ParseIP` with version detection and the private-network validator, on std's
//!   addresses and ipnet's networks, with Go's `ParseCIDR`, `IPNet.Contains` and `IP.Mask`
//!   ([`iputil::net`]).
//! - [`httputil`]: `FindIP` over `True-Client-IP`, `X-Forwarded-For`, `X-Real-IP` and the peer
//!   address; `ContentEncoding`; the 1x1 tracking pixel.
//! - [`task`]: `TickerTask`, a run now, then every interval until stopped, on tokio.
//! - [`randomutil`], [`timeutil`], [`uuidutil`]: injectable random numbers (rand), clock and time
//!   layouts (the time crate, tokio), and UUIDs (uuid).
//! - [`goregex`]: Go `regexp` patterns translated to the regex crate's syntax with Go's meaning.
//! - [`urlutil`]: `IsSafeHost`, and `net/url`'s `QueryEscape`, `PathEscape` and `Values.Encode`.
//! - [`iosutil`], [`maputil`], [`mathutil`], [`ptrutil`], [`sliceutil`], [`stringutil`],
//!   [`iterutil`]: the small helpers.
//!
//! Go tests are ported next to each module, and the behaviour is replayed from goldens written
//! by the Go programs under `testdata/go`; the seeded sweeps and edge lists of the crate-backed
//! modules are the integration tests under `tests/`.

pub mod goregex;
pub mod httputil;
pub mod iosutil;
pub mod iputil;
pub mod iterutil;
pub mod maputil;
pub mod mathutil;
pub mod ptrutil;
pub mod randomutil;
pub mod sliceutil;
pub mod stringutil;
pub mod task;
pub mod timeutil;
pub mod urlutil;
pub mod uuidutil;

#[cfg(test)]
pub mod golden;
