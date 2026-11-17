//! Go `net/http` server behaviour, `github.com/julienschmidt/httprouter` v1.3.0 and
//! `github.com/rs/cors` v1.11.0, as Prebid Server configures them.
//!
//! What a client sees on the wire is Go's, byte for byte: the HTTP/1.x connection state machine
//! is a port of `net/http`'s server (`conn.serve`, `readRequest`, `response`, `chunkWriter`)
//! over tokio sockets, not a wrapper around another HTTP library. That covers the status line,
//! `Date`, `Content-Type` sniffing ([`detect_content_type`]), `Content-Length` versus chunked
//! replies (the 2048-byte buffer), `Connection: close` decisions, `HEAD`,
//! `Expect: 100-continue`, the error replies to malformed requests (400, 431, 501, 505 with
//! Go's texts), keep-alive, read and write timeouts, and graceful shutdown.
//!
//! - [`Server`] / [`ServerConfig`] / [`Listener`]: Go's `http.Server` with Prebid's
//!   `newTCPListener` / `newUnixListener` (keep-alive listener, optional connection metrics).
//! - [`Handler`], [`ResponseWriter`], [`Request`], [`Body`]: the handler interfaces. A middleware
//!   (such as a gzip handler) wraps the `&mut dyn ResponseWriter` it is given.
//! - [`error`], [`not_found`], [`redirect`], [`StripPrefix`], [`ServeMux`], [`FileServer`],
//!   [`serve_file`], [`max_bytes_reader`]: the `net/http` helpers Prebid uses.
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use pbs_gohttp_server::{
//!     BoxFuture, HandlerFunc, Listener, Request, ResponseWriter, Server, ServerConfig,
//! };
//!
//! fn status<'a>(w: &'a mut dyn ResponseWriter, _r: &'a mut Request) -> BoxFuture<'a, ()> {
//!     Box::pin(async move {
//!         let _written = w.write(b"ok").await;
//!     })
//! }
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let server = Server::new(ServerConfig::new(":8000", Arc::new(HandlerFunc(status))));
//! let listener = Listener::bind_tcp(":8000").await?;
//! server.serve(listener).await?;
//! # Ok(())
//! # }
//! ```

mod nethttp;

pub use nethttp::*;
pub use pbs_gocompat::http::Header;
