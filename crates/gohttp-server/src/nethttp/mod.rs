//! Go `net/http`, the server half, as Prebid Server uses it.
//!
//! The connection state machine is a port of `server.go` (`conn.serve`, `readRequest`,
//! `response`, `chunkWriter`), `request.go`, `transfer.go` and `internal/chunked.go` over tokio
//! sockets, so that what a client sees on the wire, byte for byte and in the order Go writes
//! it, is Go's.

mod body;
mod bufio;
mod context;
mod fs;
mod helpers;
mod lex;
mod listener;
mod mime;
mod netconn;
mod request;
mod response;
mod servemux;
mod server;
mod sniff;
mod status;
mod writer;

use core::{
    fmt,
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    task::{Context, Poll},
};
use std::{any::Any, panic};

pub use body::{Body, BodyLimitNotifier, BodyReader, MaxBytesError, max_bytes_reader};
pub use context::{Canceled, RequestContext};
pub use fs::{Dir, FileServer, serve_file};
pub use helpers::{
    NotFoundHandler, RedirectHandler, StripPrefix, clean_path, error, html_escape, not_found,
    redirect,
};
pub use listener::{ConnMetrics, ListenError, Listener};
pub use mime::type_by_extension;
pub use request::{Cookie, ERR_NO_COOKIE, FormError, Request};
pub use response::{
    ERR_BODY_NOT_ALLOWED, ERR_CONTENT_LENGTH, Handler, HandlerFunc, ResponseWriter, TIME_FORMAT,
    TRAILER_PREFIX,
};
pub use servemux::{PatternError, ServeMux};
pub use server::{ErrorLog, ServeError, Server, ServerConfig, ShutdownError};
pub use sniff::detect_content_type;
pub use status::*;

/// A boxed, sendable future: what the handler and writer interfaces return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a panicking handler unwound with (Go: the value `recover()` returns).
pub type PanicPayload = Box<dyn Any + Send>;

/// Runs a handler future, turning a panic while it is polled into `Err` with the payload (Go's
/// deferred `recover()` around `ServeHTTP`).
pub struct CatchUnwind<'a> {
    inner: BoxFuture<'a, ()>,
}

impl fmt::Debug for CatchUnwind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CatchUnwind")
    }
}

impl<'a> CatchUnwind<'a> {
    /// Wraps `inner`.
    #[must_use]
    pub const fn new(inner: BoxFuture<'a, ()>) -> Self {
        Self { inner }
    }
}

impl Future for CatchUnwind<'_> {
    type Output = Result<(), PanicPayload>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.inner;
        match panic::catch_unwind(AssertUnwindSafe(|| inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(())) => Poll::Ready(Ok(())),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}
