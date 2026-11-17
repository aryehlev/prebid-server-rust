//! Ports of the Prebid Server Go `util` packages, one module per Go package.
//!
//! `util/jsonutil` lives in `pbs-gojson`. Everything else that non-test Go code uses is here,
//! with the Go semantics:
//!
//! - [`iputil`]: `ParseIP` with version detection, the private-network validator, and the Go `net`
//!   and `net/netip` types and parsers they rest on ([`iputil::net`], [`iputil::netip`]).
//! - [`httputil`]: `FindIP` over `True-Client-IP`, `X-Forwarded-For`, `X-Real-IP` and the remote
//!   address; `ContentEncoding`; the 1x1 tracking pixel.
//! - [`task`]: `TickerTask`, a run now, then every interval until stopped, on tokio.
//! - [`randomutil`], [`timeutil`], [`uuidutil`]: injectable random numbers, clock and UUIDs.
//! - [`iosutil`], [`maputil`], [`mathutil`], [`ptrutil`], [`sliceutil`], [`stringutil`],
//!   [`urlutil`], [`iterutil`]: the small helpers.
//!
//! Go tests are ported next to each module, and the `net`, `iputil`, `httputil` and timing
//! behaviour is replayed from goldens written by the Go programs under `testdata/go`.

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
