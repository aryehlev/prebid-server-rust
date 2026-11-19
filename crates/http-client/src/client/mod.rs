//! router.go's `http.Client`s on reqwest: [`Builder`] turns one client's settings into a reqwest
//! client, and [`Client::send`] sends a request the way bidder.go's `ctxhttp.Do` does, under the
//! request's deadline. A client sends through a [`Transport`], Go's `http.RoundTripper`: the
//! reqwest client a [`Builder`] builds, or one of the caller's (a test's fake, the parity
//! transport).

use std::{fmt, future::Future, net::IpAddr, sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use hyper_util::client::legacy::connect::HttpInfo;
use reqwest::{
    Proxy, Request, Url,
    header::{
        CONTENT_LENGTH, HOST, HeaderMap, HeaderValue, TRAILER, TRANSFER_ENCODING, USER_AGENT,
    },
    redirect::{Attempt, Policy},
};
use tokio::time::{Instant, timeout_at};
use url::Host;

pub use self::response::Response;
use self::{
    limit::{HostLimiter, Key},
    resolve::{Resolver, traced},
};
use crate::{
    error::{AWAITING_HEADERS, CGI_REFUSAL, ErrorKind, RedirectRefusal, RequestError},
    metrics::{Connections, Event, Trace},
    proxy::{Choice, ProxyEnv, ProxyRules, ProxyUrl},
    roots::Roots,
    settings::{HttpClientSettings, KEEP_ALIVE_DEFAULT, KEEP_ALIVE_PROBES},
};

mod limit;
mod resolve;
mod response;

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
    /// Go's `http.ProxyURL`: one proxy for every request, in place of the environment's.
    proxy_url: Option<Url>,
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
            proxy_url: None,
            roots: Roots::none(),
            loopback: None,
        }
    }

    /// Sends every request through `proxy`, loopback ones included, in place of the proxy
    /// environment: Go's `Transport.Proxy = http.ProxyURL(proxy)`, which the parity harness sets
    /// on a clone of each production Transport to reach its oracle
    /// (parity/transport/transport.go:159-173).
    #[must_use]
    pub fn proxy_url(mut self, proxy: Url) -> Self {
        self.proxy_url = Some(proxy);
        self
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

    /// The client: Go's `&http.Client{Transport: transport}` over the transport
    /// [`build_transport`](Self::build_transport) builds.
    ///
    /// # Errors
    ///
    /// As [`build_transport`](Self::build_transport).
    pub fn build(self) -> Result<Client, RequestError> {
        Ok(Client::from_transport(self.build_transport()?))
    }

    /// The transport alone: Go's `http.Transport`, for a [`Transport`] of the caller's to send
    /// through, as the parity transport sends through a clone of each production Transport with
    /// its oracle as the proxy ([`proxy_url`](Self::proxy_url),
    /// parity/transport/transport.go:119-173). It fails as Go's `RoundTrip` does, without the
    /// `url.Error` that the [`Client`] around it adds with the URL of the request it was handed,
    /// and it follows redirects itself, under the rules Go's Client applies.
    ///
    /// # Errors
    ///
    /// reqwest could not build it: the TLS roots could not be loaded ([`ErrorKind::Build`]).
    pub fn build_transport(self) -> Result<Arc<dyn Transport>, RequestError> {
        let Self {
            settings,
            proxy,
            proxy_url,
            roots,
            loopback,
        } = self;
        let loopback_only = loopback.is_some();
        let rules = Arc::new(proxy_url.map_or_else(|| proxy.rules(), ProxyRules::fixed));
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
        Ok(Arc::new(Reqwest {
            http,
            rules,
            limiter: settings.per_host_limit().map(HostLimiter::new),
            connections: Connections::default(),
            idle_timeout: settings.idle_timeout(),
            loopback_only,
        }))
    }
}

/// What a [`Client`] sends its requests through: Go's `http.RoundTripper`.
///
/// [`Client::send`] hands it each request with the deadline it must answer by (the request's, or
/// the client's timeout when that is earlier) and the request's trace, waits for it until that
/// deadline, and reads what it returns as Go's `client.Do` and `ctxhttp.Do` read a
/// `RoundTripper`'s answer: an error gains Go's `url.Error` prefix, `<Method> "<url>": `, unless
/// it has one, and an error that comes once the deadline has passed is the deadline's. A
/// transport reports connection events with [`Trace::emit`], answers with a [`Response`]
/// ([`Response::new`] when it does not go to the network) and fails with a [`RequestError`]
/// ([`RequestError::new`], or [`RequestError::deadline`] once the deadline has passed).
///
/// [`Client::send`] polls the transport's future in the caller's task, so a value the caller
/// scopes with tokio's `task_local!` reaches the transport, as Go's request context values reach
/// a `RoundTripper` (the bidder the parity transport labels a call with,
/// parity/trace/context.go:10-21). The client follows no redirect itself: a [`Builder`]'s
/// transport follows them, under the rules Go's Client applies, and another transport's redirect
/// answer comes back as it is.
pub trait Transport: fmt::Debug + Send + Sync {
    /// Sends `request` and waits for the response head, before `deadline` (none: no bound),
    /// recording its connection events in `trace`.
    fn round_trip(
        &self,
        request: Request,
        deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> BoxFuture<'_, Result<Response, RequestError>>;
}

/// One of router.go's clients: a [`Transport`], and Go's `Client.Timeout`. Clones share the
/// transport, and so its connection pool, as Go's clients share their Transport.
#[derive(Debug, Clone)]
pub struct Client {
    transport: Arc<dyn Transport>,
    /// Go's `Client.Timeout`: how long a request may take, its body included.
    timeout: Option<Duration>,
}

impl Client {
    /// A client that sends through `transport`: Go's `&http.Client{Transport: transport}`.
    #[must_use]
    pub fn from_transport(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            timeout: None,
        }
    }

    /// A client with this one's transport, and so its connections, and Go's `Client.Timeout` of
    /// `timeout` (zero: none): scope3's `&http.Client{Timeout: ..., Transport:
    /// deps.HTTPClient.Transport}` (modules/scope3/rtd/module.go:77-80). A request it sends ends
    /// `timeout` after it is sent, its body read included, when that is earlier than its
    /// deadline, with Go's texts for `Client.Timeout` ([`ErrorKind::ClientTimeout`]).
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
            timeout: (!timeout.is_zero()).then_some(timeout),
        }
    }

    /// Sends `request` and waits for the response head, all before `deadline` (none: no bound),
    /// recording its connection events in `trace`: bidder.go's `ctxhttp.Do(ctx, client, req)`
    /// with the trace hooks of `addClientTrace`.
    ///
    /// The client's timeout counts when it ends before `deadline`, as Go's `setRequestCancel`
    /// lets it (net/http/client.go:356-377), and an error once it has passed reads as Go's
    /// `Client.Timeout` error. An error once `deadline` has passed is ctxhttp's bare `context
    /// deadline exceeded`, whose [`RequestError::do_text`] keeps `client.Do`'s text.
    ///
    /// A [`Builder`]'s client writes the request's headers as Go's `Request.write` writes
    /// `Header`: a `Host`, `Content-Length`, `Transfer-Encoding` or `Trailer` entry is dropped
    /// (Go takes those from the URL and the body), `User-Agent` keeps its first value, an empty
    /// one sends none, and a request without one sends `Go-http-client/1.1`.
    ///
    /// # Errors
    ///
    /// The deadline passed ([`ErrorKind::DeadlineExceeded`]) or the client's timeout did
    /// ([`ErrorKind::ClientTimeout`]), the proxy environment or the loopback guard refused the
    /// request, reqwest failed to send it or follow a redirect, or the transport failed.
    pub async fn send(
        &self,
        request: Request,
        deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> Result<Response, RequestError> {
        let method = request.method().clone();
        let shown = shown_url(request.url());
        let client_deadline = self
            .timeout
            .and_then(|timeout| Instant::now().checked_add(timeout))
            .filter(|end| deadline.is_none_or(|deadline| *end < deadline));
        let bound = client_deadline.or(deadline);
        let error = match until(bound, self.transport.round_trip(request, bound, trace)).await {
            Some(Ok(response)) => return Ok(response.bounded(bound, client_deadline)),
            Some(Err(error)) => error,
            None => RequestError::deadline(),
        }
        .sent(&method, &shown);
        let now = Instant::now();
        // Client.do: once Client.Timeout has passed, the error names it
        // (net/http/client.go:733-738).
        let error = if client_deadline.is_some_and(|end| now >= end) {
            error.client_timeout(AWAITING_HEADERS)
        } else {
            error
        };
        // ctxhttp.Do: once the context is done, the error is ctx.Err() (x/net ctxhttp.go:27-36).
        Err(if deadline.is_some_and(|deadline| now >= deadline) {
            error.context_done()
        } else {
            error
        })
    }
}

/// The transport a [`Builder`] builds: Go's `http.Transport`, on reqwest.
#[derive(Debug)]
struct Reqwest {
    http: reqwest::Client,
    rules: Arc<ProxyRules>,
    limiter: Option<HostLimiter>,
    connections: Connections,
    idle_timeout: Option<Duration>,
    loopback_only: bool,
}

impl Transport for Reqwest {
    fn round_trip(
        &self,
        request: Request,
        _deadline: Option<Instant>,
        trace: Option<Trace>,
    ) -> BoxFuture<'_, Result<Response, RequestError>> {
        Box::pin(self.execute(request, trace))
    }
}

impl Reqwest {
    /// Sends `request` past the proxy rules, the loopback guard and the per-host limit, and
    /// records its connection events. [`Client::send`] stops it at the deadline.
    async fn execute(
        &self,
        mut request: Request,
        trace: Option<Trace>,
    ) -> Result<Response, RequestError> {
        let method = request.method().clone();
        let target = request.url().clone();
        let proxy = route(&self.rules, self.loopback_only, &target)
            .map_err(|(kind, message)| RequestError::own(kind, message))?;
        go_request_headers(request.headers_mut());

        let start = Instant::now();
        let permit = match &self.limiter {
            Some(limiter) => limiter.acquire(limit_key(&target, proxy)).await,
            None => None,
        };
        let wait = start.elapsed();

        let response = traced(trace.clone(), self.http.execute(request))
            .await
            .map_err(|e| RequestError::reqwest(e, &method))?;
        if let Some(info) = response.extensions().get::<HttpInfo>() {
            let reused = self.connections.observe(
                info.local_addr(),
                info.remote_addr(),
                Instant::now(),
                self.idle_timeout,
            );
            if let Some(trace) = &trace {
                trace.emit(Event::GotConn { reused, wait });
            }
        }
        Ok(Response::received(response, permit))
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
