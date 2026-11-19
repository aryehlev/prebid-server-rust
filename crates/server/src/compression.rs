//! Response compression as server/server.go sets it up.
//!
//! `getCompressionEnabledHandler` (server/server.go:120-125) wraps the main and Unix-socket
//! handlers in `gziphandler.GzipHandler` when `compression.response.enable_gzip` is set. Here that
//! is tower-http's `CompressionLayer`, gzip only, with gziphandler's 1,400-byte `DefaultMinSize`
//! as its predicate. Which answers it compresses, how it reads Accept-Encoding, where it writes
//! Vary, what it does with ranges, the framing and the compressed bytes are tower-http's, each
//! ledgered (`server.vary`, `server.gzip-negotiation`, `server.gzip-ranges`, `server.gzip-bytes`,
//! `server.framing`).

use axum::Router;
use tower_http::compression::{CompressionLayer, predicate::SizeAbove};

/// gziphandler's `DefaultMinSize` (gzip.go v1.1.1), which Prebid keeps: an answer of fewer bytes
/// is sent as it is.
pub const MIN_SIZE: u16 = 1400;

/// The compression `gziphandler.GzipHandler` gives Prebid's servers.
///
/// Gzip for answers of at least [`MIN_SIZE`] bytes, whatever their type.
#[must_use]
pub fn gzip() -> CompressionLayer<SizeAbove> {
    CompressionLayer::new()
        .no_br()
        .no_deflate()
        .no_zstd()
        .compress_when(SizeAbove::new(MIN_SIZE))
}

/// `getCompressionEnabledHandler`: `router` behind [`gzip`] when `enable_gzip` is set.
///
/// `enable_gzip` is the value of `compression.response.enable_gzip`; without it `router` is
/// served as it is.
pub fn compression_enabled(router: Router, enable_gzip: bool) -> Router {
    if enable_gzip {
        router.layer(gzip())
    } else {
        router
    }
}
