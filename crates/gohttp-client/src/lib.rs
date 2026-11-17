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
//!
//! On macOS Go ignores `SSL_CERT_FILE` and `SSL_CERT_DIR` and verifies with the platform verifier
//! (Security.framework, so keychain roots and trust settings apply); it runs its own verifier only
//! when that one fails and certificates were appended to the system pool. [`CertPool::system`]
//! reads the Linux locations on every platform, which on macOS finds the `/etc/ssl/cert.pem`
//! bundle. The [`tls`] module docs explain the choice of TLS stack.
//!
//! [`x509`] verifies server certificates with Go's order of checks and Go's error texts over
//! rustls-webpki's path building; `testdata/golden/x509.json`, written by Go, pins both. Where
//! webpki decides differently, the port follows webpki:
//!
//! - A leaf that is a CA certificate (`cA` set) is rejected with `x509: certificate signed by
//!   unknown authority`. Go accepts it.
//! - A leaf or intermediate with a critical certificate policies, policy mappings, policy
//!   constraints or inhibit anyPolicy extension is rejected with `x509: unhandled critical
//!   extension`. Go handles these four; in a root both accept them.
//! - An empty extended key usage extension is rejected with `x509: EmptyEkuExtension`. Go reads it
//!   as no restriction.
//! - Go's RFC 5280 policy validation (`policiesValid`) is not run, so a chain that Go rejects for
//!   its policy constraints (`x509: no valid chains built: ... candidate chains with invalid
//!   policies`) verifies.
//! - A name constraint violation is reported as `x509: NameConstraintViolation`, without Go's text
//!   (`x509: a root or intermediate certificate is not authorized to sign for this name: ...`).
//!   Go's checks of the constraint values when it parses a certificate (`x509: failed to parse
//!   dnsName constraint ...` and the like) are not made; webpki judges the values as it verifies.
//! - When a candidate issuer's signature does not verify (another key, or an algorithm webpki does
//!   not take), the error is the plain `x509: certificate signed by unknown authority`, without
//!   Go's `(possibly because of ... while trying to verify candidate authority certificate ...)`.
//! - Signatures are checked with rustls's aws-lc-rs algorithms, whose RSA ones take keys of 2048 to
//!   8192 bits. Go verifies with RSA keys from 1024 bits.
//! - Public keys are not parsed with their certificates, so Go's key errors (`x509: invalid RSA
//!   modulus`, `x509: unsupported elliptic curve`, ...) and its `tls: server sent certificate
//!   containing RSA key larger than 8192 bits` are not reported: such a key fails a signature check
//!   later, with another error. URI names are not parsed either, so a certificate Go rejects for
//!   one (`x509: cannot parse URI ...`) is accepted.
//! - A path through a certificate whose key is on rustls-webpki 0.103's built-in distrust list
//!   (eight CA keys, in that crate's `src/data/distrusted`) is rejected with `x509: certificate
//!   signed by unknown authority`. Go has no such list.
//! - A path holds at most six intermediates, and path building has a budget of calls, as webpki
//!   sets them; past them the error is `x509: certificate signed by unknown authority`. Go limits
//!   only the signature checks, to 100 as webpki does.
//! - A leaf or intermediate whose basic constraints carry a path length above 255 is rejected with
//!   `x509: BadDer` (webpki reads the value as one byte). Go accepts it.
//! - When several paths fail, or one path fails several checks, the error reported follows webpki's
//!   order of checks and its ranking of errors where they differ from Go's.
//! - The TLS alert sent for a rejected certificate is `certificate_unknown`, where Go sends
//!   `bad_certificate`, or `decode_error` for a certificate it cannot parse.

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
pub mod x509;

pub use client::{CheckRedirect, Client};
pub use context::{CancelCauseFunc, CancelFunc, Context};
pub use error::{ContextError, HttpError};
pub use net::{DialedConn, Dialer};
pub use request::{Body, GetBody, Request};
pub use response::{Response, ResponseBody};
pub use tls::{CertPool, TlsConfig};
pub use transport::{RoundTripper, Transport, TransportOptions};
