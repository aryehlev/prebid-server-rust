//! Prebid Server's listeners on axum, hyper-util and tower-http, and gziphandler's compression
//! on fastgzip.
//!
//! server/server.go runs each of Prebid's servers as a net/http `http.Server`: the main server
//! on TCP or a Unix socket with 15 s read and write budgets, the admin and Prometheus servers
//! with none, gziphandler around the main handler when `compression.response.enable_gzip` is
//! set, and a graceful `Shutdown` with a 10 s deadline on SIGTERM or SIGINT. Here [`serve()`]
//! serves an axum router on hyper-util's HTTP/1.1 connection builder with the same budgets,
//! net/http's header limit and no library body limit, and stops it the same way
//! ([`shutdown_signal`], [`SHUTDOWN_TIMEOUT`]); [`compression`] holds the gzip layer, which
//! decides as gziphandler does and compresses with fastgzip.
//!
//! What a client can see differ from net/http (header order and case, framing, sniffing, the
//! edges of the limits, malformed requests and the bytes of request targets, the budgets, bodies
//! a handler leaves unread, the gzip bytes, long streamed answers and q values only Go's
//! `ParseFloat` or only std reads) is ledgered in this crate's `differences.toml` and pinned in
//! `tests/` against goldens the Go program in `testdata/go` writes from Prebid's own server
//! constructors.

pub mod compression;
pub mod serve;

pub use serve::{Budget, SHUTDOWN_TIMEOUT, StopSignal, Stopped, serve, shutdown_signal};
