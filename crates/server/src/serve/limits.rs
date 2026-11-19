//! Glue (Rule B (a)): net/http's request limits, where hyper's and axum's defaults would refuse
//! requests real traffic sends and Go serves. net/http reads at most `DefaultMaxHeaderBytes` (1
//! MiB, which Prebid's servers keep) plus the 4096 bytes of slack `initialReadLimitSize` adds of
//! request line and headers, where hyper's buffer stops at 408 KiB (`server.max-header-bytes`),
//! and it sets no limit of its own on request bodies, where axum's `DefaultBodyLimit` refuses a
//! body over 2 MiB that Prebid reads up to `max_request_size` (`server.body-limit`). Pinned
//! against the goldens' `header_limit` cases (`tests/wire.rs`) and `body_limit` cases
//! (`crates/router/tests/routing.rs`).

use axum::{Router, extract::DefaultBodyLimit};

/// The most bytes of request line and headers net/http reads (1 MiB + 4096): hyper's
/// `max_buf_size`, the size of the buffer a request head is read into.
pub(super) const MAX_BUF_SIZE: usize = (1 << 20) + 4096;

/// `router` without axum's default limit on the bodies its extractors read.
pub(super) fn without_body_limit(router: Router) -> Router {
    router.layer(DefaultBodyLimit::disable())
}
