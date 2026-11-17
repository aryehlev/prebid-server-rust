//! Go `net/http` client and transport wire behaviour.
//!
//! This crate is an HTTP/1.1 client whose externally visible behaviour matches Go's
//! `http.Client` and `http.Transport` as Prebid Server configures them (`router.go`'s general,
//! cache and floors clients): the bytes written for a request, the connection pooling and reuse
//! rules, redirects, transparent gzip, timeouts, proxies, TLS and Go's error texts.
//!
//! It is written directly over `tokio` sockets rather than a generic HTTP library because Go's
//! observable behaviour depends on details such libraries normalise away: raw request URIs,
//! header key spelling and order, framing decisions, and when bytes are flushed.
//!
//! # Use
//!
//! ```no_run
//! use std::{sync::Arc, time::Duration};
//!
//! use pbs_gohttp_client::{
//!     Client, Context, Dialer, Request, Transport, TransportOptions, ctxhttp,
//! };
//!
//! # async fn run() -> Result<(), pbs_gohttp_client::HttpError> {
//! let transport = Transport::new(TransportOptions {
//!     proxy: Some(Arc::new(
//!         pbs_gohttp_client::httpproxy::proxy_from_environment,
//!     )),
//!     dialer: Dialer {
//!         timeout: Duration::from_secs(30),
//!         keep_alive: Some(Duration::from_secs(30)),
//!     },
//!     max_idle_conns_per_host: 10,
//!     idle_conn_timeout: Duration::from_secs(50),
//!     ..TransportOptions::default()
//! });
//! let client = Client {
//!     transport: Some(Arc::new(transport)),
//!     ..Client::default()
//! };
//! let (ctx, _cancel) = Context::background().with_timeout(Duration::from_millis(500));
//! let req = Request::new("GET", "http://example.com/", pbs_gohttp_client::Body::Nil)?;
//! let mut resp = ctxhttp::do_request(&ctx, Some(&client), req).await?;
//! let body = resp.body.read_all().await?;
//! resp.body.close();
//! # let _ = body;
//! # Ok(())
//! # }
//! ```
//!
//! # Differences from Go
//!
//! HTTP/2 is not implemented: Prebid's transports set `DialContext` (and `TLSClientConfig`)
//! without `ForceAttemptHTTP2`, which disables HTTP/2 in Go as well. Go's `DefaultTransport`
//! would negotiate HTTP/2 with TLS servers; [`client::default_transport`] does not.

pub mod bufio;
pub mod chunked;
pub mod client;
pub mod context;
pub mod ctxhttp;
pub mod error;
#[cfg(test)]
mod golden_tests;
pub mod gzip;
pub mod httpguts;
pub mod httpproxy;
pub mod httptrace;
pub mod net;
pub mod request;
pub mod response;
#[cfg(test)]
pub mod testutil;
pub mod textproto;
pub mod tls;
pub mod transfer;
pub mod transport;

pub use client::{CheckRedirect, Client};
pub use context::{CancelCauseFunc, CancelFunc, Context};
pub use error::{ContextError, HttpError};
pub use net::{DialedConn, Dialer};
pub use request::{Body, GetBody, Request};
pub use response::{Response, ResponseBody};
pub use tls::{CertPool, TlsConfig};
pub use transport::{RoundTripper, Transport, TransportOptions};
