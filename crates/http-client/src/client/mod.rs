//! router.go's `http.Client`s on reqwest: [`Builder`] turns one client's settings into a reqwest
//! client, and [`Client::send`] sends a request the way bidder.go's `ctxhttp.Do` does, under the
//! request's deadline.

use std::{future::Future, net::IpAddr, sync::Arc, time::Duration};

use bytes::Bytes;
use hyper_util::client::legacy::connect::HttpInfo;
use reqwest::{
    Proxy, Request, StatusCode, Url, Version,
    header::{
        CONTENT_LENGTH, HOST, HeaderMap, HeaderValue, TRAILER, TRANSFER_ENCODING, USER_AGENT,
    },
    redirect::{Attempt, Policy},
};
use tokio::{
    sync::OwnedSemaphorePermit,
    time::{Instant, timeout_at},
};
use url::Host;

use self::{
    limit::{HostLimiter, Key},
    resolve::{Resolver, traced},
};
use crate::{
    error::{CGI_REFUSAL, ErrorKind, RedirectRefusal, RequestError},
    metrics::{Connections, Event, Trace},
    proxy::{Choice, ProxyEnv, ProxyRules, ProxyUrl},
    roots::Roots,
    settings::{HttpClientSettings, KEEP_ALIVE_DEFAULT, KEEP_ALIVE_PROBES},
};

mod limit;
mod resolve;

/// The `User-Agent` Go's `Request.write` sends when the caller set none
/// (net/http/request.go `defaultUserAgent`; D-UA).
pub const GO_USER_AGENT: &str = "Go-http-client/1.1";

/// Go's `defaultCheckRedirect` refuses the redirect that would make the eleventh request.
const MAX_REQUESTS: usize = 10;

/// Builds one of router.go's clients.
#[derive(Debug, Clone)]
pub struct Builder {
    settings: HttpClientSettings,
    proxy: ProxyEnv,
    roots: Roots,
    /// The loopback guard: the port a URL without one reaches (0: its scheme's).
    loopback: Option<u16>,
}

impl Builder {
    /// A client with `settings`, taking its proxies from `proxy` (production passes
    /// [`ProxyEnv::from_process`]), trusting the system roots only.
    #[must_use]
    pub const fn new(settings: HttpClientSettings, proxy: ProxyEnv) -> Self {
        Self {
            settings,
            proxy,
            roots: Roots::none(),
            loopback: None,
        }
    }

    /// Trusts `roots` as well as the system roots: the general client's `certificates_file`.
    #[must_use]
    pub fn roots(mut self, roots: Roots) -> Self {
        self.roots = roots;
        self
    }

    /// The loopback guard for tests and the parity harness: every host name resolves to
    /// 127.0.0.1, at `default_port` for a URL that names no port (0: its scheme's), and a
    /// request, redirect or proxy for an IP address off the loopback interface fails with
    /// [`ErrorKind::NotLoopback`], so nothing leaves the machine.
    #[must_use]
    pub const fn loopback_only(mut self, default_port: u16) -> Self {
        self.loopback = Some(default_port);
        self
    }

    /// The client.
    ///
    /// # Errors
    ///
    /// reqwest could not build it: the TLS roots could not be loaded ([`ErrorKind::Build`]).
    pub fn build(self) -> Result<Client, RequestError> {
        let Self {
            settings,
            proxy,
            roots,
            loopback,
        } = self;
        let loopback_only = loopback.is_some();
        let rules = Arc::new(proxy.rules());
        let keep_alive = settings.dialer.keep_alive();
        let mut builder = reqwest::Client::builder()
            .http1_only()
            .gzip(true)
            .redirect(redirect_policy(Arc::clone(&rules), loopback_only))
            .pool_max_idle_per_host(settings.max_idle_per_host())
            .pool_idle_timeout(settings.idle_timeout())
            .tcp_keepalive(keep_alive)
            .tcp_keepalive_interval(keep_alive.map(|_| KEEP_ALIVE_DEFAULT))
            .tcp_keepalive_retries(keep_alive.map(|_| KEEP_ALIVE_PROBES))
            .dns_resolver(Arc::new(Resolver { loopback }));
        if let Some(timeout) = settings.dialer.connect_timeout() {
            builder = builder.connect_timeout(timeout);
        }
        builder = if rules.has_proxies() {
            let [https, http] = proxies(&rules);
            builder.proxy(https).proxy(http)
        } else {
            builder.no_proxy()
        };
        if !roots.is_empty() {
            let mut certificates = Vec::new();
            for der in roots.certificates() {
                certificates
                    .push(reqwest::Certificate::from_der(der).map_err(RequestError::build)?);
            }
            builder = builder.tls_certs_merge(certificates);
        }
        let http = builder.build().map_err(RequestError::build)?;
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                rules,
                limiter: settings.per_host_limit().map(HostLimiter::new),
                connections: Connections::default(),
                idle_timeout: settings.idle_timeout(),
                loopback_only,
            }),
        })
    }
}

/// One of router.go's clients. Clones share the connection pool, as Go's clients share their
/// Transport.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    rules: Arc<ProxyRules>,
    limiter: Option<HostLimiter>,
    connections: Connections,
    idle_timeout: Option<Duration>,
    loopback_only: bool,
}

impl Client {
    /// Sends `request` and waits for the response head, all before `deadline` (none: no bound),
    /// recording its connection events in `trace`: bidder.go's `ctxhttp.Do(ctx, client, req)`
    /// with the trace hooks of `addClientTrace`.
    ///
    /// The request's headers are written as Go's `Request.write` writes `Header`: a `Host`,
    /// `Content-Length`, `Transfer-Encoding` or `Trailer` entry is dropped (Go takes those from
    /// the URL and the body), `User-Agent` keeps its first value, an empty one sends none, and a
    /// request without one sends `Go-http-client/1.1`.
    ///
    /// # Errors
    ///
    /// The deadline passed ([`ErrorKind::DeadlineExceeded`]), the proxy environment or the
    /// loopback guard refused the request, or reqwest failed to send it or follow a redirect.
    pub async fn send(
        &self,
        mut request: Request,
        deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> Result<Response, RequestError> {
        let inner = &*self.inner;
        let method = request.method().clone();
        let shown = shown_url(request.url());
        let target = request.url().clone();
        let proxy = route(&inner.rules, inner.loopback_only, &target)
            .map_err(|(kind, message)| RequestError::own(kind, &method, &shown, message))?;
        go_request_headers(request.headers_mut());

        let start = Instant::now();
        let permit = match &inner.limiter {
            Some(limiter) => until(deadline, limiter.acquire(limit_key(&target, proxy)))
                .await
                .ok_or_else(RequestError::deadline)?,
            None => None,
        };
        let wait = start.elapsed();

        let response = until(deadline, traced(trace.clone(), inner.http.execute(request)))
            .await
            .ok_or_else(RequestError::deadline)?
            .map_err(|e| RequestError::reqwest(e, &method, &shown))?;
        if let Some(info) = response.extensions().get::<HttpInfo>() {
            let reused = inner.connections.observe(
                info.local_addr(),
                info.remote_addr(),
                Instant::now(),
                inner.idle_timeout,
            );
            if let Some(trace) = &trace {
                trace.emit(Event::GotConn { reused, wait });
            }
        }
        Ok(Response {
            inner: response,
            deadline,
            permit,
        })
    }
}

/// A response whose head has arrived. It holds its host's `max_connections_per_host` slot until
/// its body is read or it is dropped.
#[derive(Debug)]
pub struct Response {
    inner: reqwest::Response,
    deadline: Option<Instant>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Response {
    /// The status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// The headers. reqwest removes `Content-Encoding` and `Content-Length` from a gzip response
    /// it decompresses, as Go's Transport does for the responses it decompresses.
    #[must_use]
    pub fn headers(&self) -> &HeaderMap {
        self.inner.headers()
    }

    /// The HTTP version.
    #[must_use]
    pub fn version(&self) -> Version {
        self.inner.version()
    }

    /// The final URL, after redirects.
    #[must_use]
    pub fn url(&self) -> &Url {
        self.inner.url()
    }

    /// The whole body, read before the request's deadline: bidder.go's `io.ReadAll`, whose error
    /// bidder.go returns unclassified, a deadline included (exchange/bidder.go:641-646).
    ///
    /// # Errors
    ///
    /// The deadline passed ([`ErrorKind::DeadlineExceeded`]) or the body could not be read or
    /// decompressed ([`ErrorKind::Body`]).
    pub async fn bytes(self) -> Result<Bytes, RequestError> {
        let Self {
            inner,
            deadline,
            permit,
        } = self;
        let read = until(deadline, inner.bytes()).await;
        drop(permit);
        read.ok_or_else(RequestError::deadline)?
            .map_err(RequestError::body)
    }
}

/// Runs `future` until `deadline`: `None` when the deadline passed first.
async fn until<F: Future>(deadline: Option<Instant>, future: F) -> Option<F::Output> {
    match deadline {
        Some(deadline) => timeout_at(deadline, future).await.ok(),
        None => Some(future.await),
    }
}

/// The URL as Go's `url.Error` shows it: the password, if any, as `***` (net/http/client.go
/// `stripPassword`).
fn shown_url(url: &Url) -> String {
    if url.password().is_none() {
        return url.to_string();
    }
    let mut shown = url.clone();
    match shown.set_password(Some("***")) {
        Ok(()) => shown.to_string(),
        Err(()) => url.to_string(),
    }
}

/// Go's `Request.write` reading of `Header` (net/http/request.go): the entries it never writes
/// from the map go, and `User-Agent` is the first value, none when that is empty, Go's own when
/// the caller set none.
fn go_request_headers(headers: &mut HeaderMap) {
    for name in [HOST, CONTENT_LENGTH, TRANSFER_ENCODING, TRAILER] {
        drop(headers.remove(name));
    }
    let first = headers.get(USER_AGENT).cloned();
    drop(headers.remove(USER_AGENT));
    let user_agent = match first {
        None => HeaderValue::from_static(GO_USER_AGENT),
        Some(value) if value.is_empty() => return,
        Some(value) => trim_go(value),
    };
    drop(headers.insert(USER_AGENT, user_agent));
}

/// `textproto.TrimString`: the value without leading and trailing spaces and tabs (a header
/// value holds no other ASCII white space).
fn trim_go(value: HeaderValue) -> HeaderValue {
    let bytes = value.as_bytes();
    let start = bytes.iter().position(|b| !matches!(b, b' ' | b'\t'));
    let end = bytes.iter().rposition(|b| !matches!(b, b' ' | b'\t'));
    let trimmed = match (start, end) {
        (Some(start), Some(end)) => bytes.get(start..=end).unwrap_or_default(),
        _ => &[],
    };
    if trimmed.len() == bytes.len() {
        return value;
    }
    HeaderValue::from_bytes(trimmed).unwrap_or(value)
}

/// Where a request for `url` goes, once the proxy rules and the loopback guard allow it: through
/// this proxy, or straight to the host.
fn route<'a>(
    rules: &'a ProxyRules,
    loopback_only: bool,
    url: &Url,
) -> Result<Option<&'a ProxyUrl>, (ErrorKind, String)> {
    let proxy = match rules.choose(url) {
        Choice::Refused => return Err((ErrorKind::Proxy, CGI_REFUSAL.to_owned())),
        Choice::Direct => None,
        Choice::Proxy(proxy) => {
            if proxy.usable().is_none() {
                // The value is not repeated: it may hold the proxy's credentials.
                let variable = if url.scheme() == "https" {
                    "HTTPS_PROXY"
                } else {
                    "HTTP_PROXY"
                };
                let message = proxy.url().map_or_else(
                    || format!("{variable} is not a proxy URL"),
                    |parsed| {
                        format!(
                            "{variable} names a {:?} proxy, and only http and https proxies are \
                             supported",
                            parsed.scheme()
                        )
                    },
                );
                return Err((ErrorKind::Proxy, message));
            }
            Some(proxy)
        }
    };
    if loopback_only {
        for host in [Some(url), proxy.and_then(ProxyUrl::usable)]
            .into_iter()
            .flatten()
        {
            if !on_loopback(host) {
                return Err((
                    ErrorKind::NotLoopback,
                    format!("the loopback guard refuses {host}"),
                ));
            }
        }
    }
    Ok(proxy)
}

/// Whether a connection to `url`'s host stays on the loopback interface under the guard, whose
/// resolver gives 127.0.0.1 for every name.
fn on_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(_)) => true,
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).to_canonical().is_loopback(),
        None => false,
    }
}

/// Go's `connectMethod.key()` for a request for `url` through `proxy`.
fn limit_key(url: &Url, proxy: Option<&ProxyUrl>) -> Key {
    let scheme = url.scheme().to_owned();
    let addr = if proxy.is_some() && scheme == "http" {
        String::new()
    } else {
        format!(
            "{}:{}",
            url.host_str().unwrap_or_default(),
            url.port_or_known_default().unwrap_or_default()
        )
    };
    Key {
        proxy: proxy.map_or_else(String::new, |p| p.text().to_owned()),
        scheme,
        addr,
    }
}

/// The two proxies reqwest consults in turn: one for `https` URLs, whose CONNECT carries Go's
/// `User-Agent` (Go's `Request.write` adds it to the CONNECT request), and one for `http` URLs,
/// which carry the request's own.
fn proxies(rules: &Arc<ProxyRules>) -> [Proxy; 2] {
    let pick = |scheme: &'static str| {
        let rules = Arc::clone(rules);
        move |url: &Url| {
            if url.scheme() != scheme {
                return None;
            }
            match rules.choose(url) {
                Choice::Proxy(proxy) => proxy.usable().cloned(),
                Choice::Direct | Choice::Refused => None,
            }
        }
    };
    let mut connect_headers = HeaderMap::new();
    drop(connect_headers.insert(USER_AGENT, HeaderValue::from_static(GO_USER_AGENT)));
    [
        Proxy::custom(pick("https")).headers(connect_headers),
        Proxy::custom(pick("http")),
    ]
}

/// Go's `defaultCheckRedirect` (at most ten requests), then the proxy rules and the loopback
/// guard for the next URL, which Go's Transport applies to every request it sends.
fn redirect_policy(rules: Arc<ProxyRules>, loopback_only: bool) -> Policy {
    Policy::custom(move |attempt: Attempt<'_>| {
        let next = attempt.url();
        let refusal = if attempt.previous().len() >= MAX_REQUESTS {
            Some((ErrorKind::Redirect, "stopped after 10 redirects".to_owned()))
        } else {
            route(&rules, loopback_only, next).err()
        };
        match refusal {
            Some((kind, message)) => {
                let url = shown_url(next);
                attempt.error(RedirectRefusal { kind, url, message })
            }
            None => attempt.follow(),
        }
    })
}
