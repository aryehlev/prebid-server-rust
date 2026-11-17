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
//! [`x509`] is a port of Go 1.26's server certificate verification (`crypto/tls`'s
//! `verifyServerCertificate` over `crypto/x509`'s parser and `Certificate.Verify`): Go's parsing
//! rules, chain building, policy, extended key usage and name constraint checks, and error texts.
//! `testdata/golden/x509.json` and `testdata/golden/x509_adversarial.json`, written by Go, pin
//! them. What differs:
//!
//! - Signatures are verified with rustls's aws-lc-rs algorithms, whose RSA ones take keys of 2048
//!   to 8192 bits and which have no ECDSA on P-224. Go verifies RSA signatures from 1024 bits (a
//!   root's key has no upper limit) and P-224 ones. The port fails such a signature with `x509:
//!   cannot verify signature: algorithm unimplemented`, so the chain fails as one whose issuer is
//!   unknown, with that error as the candidate's.
//! - aws-lc-rs judges encodings at the edges of its algorithms (a non-canonical Ed25519 key, for
//!   one) by its own rules, not those of Go's `crypto/ed25519`, `crypto/ecdsa` and `crypto/rsa`.
//! - The handshake's own signature (TLS 1.3 `CertificateVerify`, TLS 1.2 `ServerKeyExchange`) is
//!   verified by rustls, which reads the leaf with rustls-webpki: a leaf that Go's verifier accepts
//!   but webpki does not read (a version 1 or 2 certificate, or a critical extension webpki does
//!   not know, such as certificate policies) fails the handshake there, with another error.
//! - Go's `net.ParseIP` (for the server name and URI name constraints) is approximated by Rust's
//!   `IpAddr` parser, which takes no zones; rustls's `ServerName` refuses some names Go would send
//!   (an IPv6 address with a zone, a name with a space) before the handshake.
//! - Go sorts the constraints of one name type with `slices.SortFunc`, which is not stable past
//!   twelve elements; the port sorts stably, so among many constraints that compare equal, the one
//!   quoted in an error may differ.
//! - PEM is decoded by rustls-pki-types, not Go's `encoding/pem`: a block with headers, or text Go
//!   skips between blocks, may be read differently.
//! - A P-256 coordinate that is not below the prime gets the text of Go's amd64 and arm64 builds
//!   (`invalid P256 element encoding`); other architectures say `invalid P256Element encoding`.
//! - The TLS alert sent for a rejected certificate is `certificate_unknown`, where Go sends
//!   `bad_certificate`, `decode_error` for a certificate it cannot parse, or
//!   `unsupported_certificate` for a leaf key it does not sign with.

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
