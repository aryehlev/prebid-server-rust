//! The Go server's outbound HTTP clients, as `router/router.go` builds them, on reqwest.
//!
//! One [`Client`] stands for each Go client: the general client (bidders, modules, stored requests
//! and the vendor list), the cache and floors clients, and the plain clients that stand for Go's
//! `&http.Client{}` and `http.DefaultClient` (currency rates, analytics, data files). A
//! [`Builder`] turns Go's `http_client` settings ([`HttpClientSettings`]) into reqwest's options:
//!
//! - `User-Agent: Go-http-client/1.1` when the caller set none (D-UA);
//! - rustls with the platform verifier, plus the `certificates_file` roots for the general client
//!   ([`roots`], D-TLS);
//! - reqwest's gzip decompression;
//! - proxies from the environment by Go's rules ([`proxy`], Rule B (c)), loopback never proxied;
//! - `max_connections_per_host` through a per-host semaphore (Rule B (c));
//! - deadlines that end a request with Go's `context deadline exceeded`, classified as a timeout
//!   ([`RequestError::is_timeout`]);
//! - the connection events bidder.go's httptrace hooks record ([`metrics`]).
//!
//! Every client speaks HTTP/1.1, as router.go's do. `docs/DIFFERENCES.md` lists where the clients
//! differ from Go's (its `http-client` and `tls` categories).

pub mod error;
pub mod metrics;
pub mod proxy;
pub mod roots;
pub mod settings;

mod client;

pub use client::{Builder, Client, GO_USER_AGENT, Response};
pub use error::{ErrorKind, RequestError};
pub use metrics::{Event, Trace};
pub use proxy::ProxyEnv;
pub use reqwest;
pub use roots::Roots;
pub use settings::{DialerSettings, HttpClientSettings};
