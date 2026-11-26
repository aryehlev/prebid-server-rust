//! A bidder request over the pool: its redirects, which reqwest would follow inside its client
//! and the pool follows hop by hop as tower-http and reqwest's policy do, and the ways it leaves
//! the pool for reqwest's clients.
//!
//! - Over TLS, an origin that picked `http/1.1` gets the request over the bidder client's own
//!   reqwest client, which offers the same ALPN; so does a redirect's hop to a URL the pool does
//!   not take (an `http` URL, or one through a proxy), which reqwest follows on from there, Go's
//!   ten requests counting the hops before it.
//! - Over h2c (`http-client.h2c-demotion`, `http-client.h2c-redirects`), a redirect is followed
//!   only on the origin; one to another origin comes back as the answer and moves the origin back
//!   to HTTP/1.1. A request the origin read none of moves it back and goes again over HTTP/1.1,
//!   once; one that failed after the origin may have read it moves it back and fails.

use std::time::Duration;

use bytes::Bytes;
use reqwest::{
    Method, Request, StatusCode, Url,
    header::{
        AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HeaderMap,
        HeaderValue, LOCATION, PROXY_AUTHORIZATION, REFERER, TRANSFER_ENCODING, WWW_AUTHENTICATE,
    },
};

use super::{
    Core, Http2, check_redirect,
    http2::{Origin, stays_on},
    limit_key,
    pool::{Answer, Failure, Hop},
    route,
};
use crate::{
    Response,
    error::{ErrorKind, RequestError},
    metrics::{Event, Trace},
    proxy::ProxyUrl,
};

/// What a redirect leads to.
enum Next {
    /// The answer is the response.
    Answer,
    /// The next hop, over the pool.
    Pool(Hop),
    /// The next hop, over the bidder client's reqwest client.
    Reqwest(Hop),
}

/// Sends `request`, whose body is `body`, over `http2`'s pool, to its h2c origin `h2c` or over
/// TLS, and reports the connection to `trace`.
pub(super) async fn send(
    core: &Core,
    http2: &Http2,
    request: Request,
    body: Option<Bytes>,
    trace: Option<Trace>,
    h2c: Option<Origin>,
) -> Result<Response, RequestError> {
    let method = request.method().clone();
    let mut hop = Hop::new(
        method.clone(),
        request.url().clone(),
        request.headers().clone(),
        body,
    );
    // The URLs of the hops sent, as reqwest's policy counts them.
    let mut requested: Vec<Url> = Vec::new();
    let mut wait = Duration::ZERO;
    loop {
        let sent = http2
            .pool
            .send(&hop, core.limiter.as_ref(), trace.as_ref())
            .await;
        let answer = match sent {
            Ok(answer) => answer,
            Err(failure) => {
                let off = Off {
                    core,
                    http2,
                    trace,
                    wait,
                };
                return off
                    .failed(failure, request, hop, requested.len(), h2c.as_ref())
                    .await;
            }
        };
        wait = wait.saturating_add(answer.wait);
        requested.push(hop.url.clone());
        let next = redirect(
            core,
            http2,
            &hop,
            &answer,
            &requested,
            h2c.as_ref(),
            &method,
        )?;
        match next {
            Next::Answer => {
                if let Some(trace) = &trace {
                    trace.emit(Event::GotConn {
                        reused: answer.reused,
                        wait,
                    });
                }
                let Answer {
                    status,
                    headers,
                    body: stream,
                    ..
                } = answer;
                // reqwest's response URL is the last request's target, without a fragment.
                let mut url = hop.url;
                url.set_fragment(None);
                return Ok(Response::pooled(url, status, headers, stream));
            }
            Next::Pool(next) => hop = next,
            Next::Reqwest(next) => {
                drop(answer);
                let key = limit_key(&next.url, proxy_of(core, &next.url));
                return core
                    .over(
                        &http2.tls,
                        reqwest_request(next),
                        trace,
                        key,
                        wait,
                        requested.len(),
                    )
                    .await;
            }
        }
    }
}

/// A request leaving the pool.
struct Off<'a> {
    core: &'a Core,
    http2: &'a Http2,
    trace: Option<Trace>,
    /// The time it waited in the pool.
    wait: Duration,
}

impl Off<'_> {
    /// What becomes of `request` once its hop `hop`, after `requested` hops, failed over the
    /// pool with `failure`.
    async fn failed(
        self,
        failure: Failure,
        request: Request,
        hop: Hop,
        requested: usize,
        h2c: Option<&Origin>,
    ) -> Result<Response, RequestError> {
        let unread = match failure {
            Failure::Http1 { wait } => {
                let wait = self.wait.saturating_add(wait);
                let request = if requested == 0 {
                    request
                } else {
                    reqwest_request(hop)
                };
                let key = limit_key(request.url(), None);
                // An h2c origin moved back to HTTP/1.1 goes over the HTTP/1.1 connections the
                // general client shares, as it does once its verdict is read.
                let client = if request.url().scheme() == "https" {
                    &self.http2.tls
                } else {
                    &self.core.http1
                };
                return self
                    .core
                    .over(client, request, self.trace, key, wait, requested)
                    .await;
            }
            Failure::Target(error) => {
                return Err(RequestError::chained(
                    ErrorKind::Request,
                    "builder error",
                    error,
                ));
            }
            Failure::Dial(error) if error.in_handshake() => error.request_error(),
            Failure::Dial(error) => return Err(error.request_error()),
            Failure::Unread(error) => sending(error),
            Failure::Failed(error) => {
                if let Some(origin) = h2c {
                    demote(self.http2, origin, &hop.url);
                }
                return Err(sending(error));
            }
        };
        let Some(origin) = h2c else {
            return Err(unread);
        };
        // The origin read none of it: back to HTTP/1.1, and the request again over it, once.
        demote(self.http2, origin, &hop.url);
        let key = limit_key(request.url(), None);
        self.core
            .over(&self.core.http1, request, self.trace, key, self.wait, 0)
            .await
    }
}

/// Moves the h2c origin `origin`, which `url` is on, back to HTTP/1.1: its verdict, and its
/// connections in the pool, which leave it and give their `max_connections_per_host` slots back
/// once their streams have ended.
fn demote(http2: &Http2, origin: &Origin, url: &Url) {
    http2.h2c.demote(origin);
    http2.pool.demote(&limit_key(url, None));
}

/// The error of a request whose HTTP/2 stream or connection failed with `error`, worded as
/// reqwest words it.
fn sending(error: h2::Error) -> RequestError {
    RequestError::chained(
        ErrorKind::Request,
        "error sending request: client error (SendRequest): http2 error",
        error,
    )
}

/// The proxy a request for `url` goes through, once [`route`] has let it.
fn proxy_of<'a>(core: &'a Core, url: &Url) -> Option<&'a ProxyUrl> {
    route(&core.rules, core.loopback_only, url).ok().flatten()
}

/// The reqwest request of `hop`.
fn reqwest_request(hop: Hop) -> Request {
    let Hop {
        method,
        url,
        headers,
        body,
    } = hop;
    let mut request = Request::new(method, url);
    *request.headers_mut() = headers;
    *request.body_mut() = body.map(reqwest::Body::from);
    request
}

/// Where `answer`, the answer to `hop` after the hops `requested` (its own last), leads:
/// tower-http's `FollowRedirect` with reqwest's policy adapter and the crate's policy, the origin
/// `h2c`'s redirects followed only on it.
///
/// # Errors
///
/// The redirect is refused: Go's ten requests, the proxy rules, the loopback guard, or a scheme
/// reqwest does not follow.
fn redirect(
    core: &Core,
    http2: &Http2,
    hop: &Hop,
    answer: &Answer,
    requested: &[Url],
    h2c: Option<&Origin>,
    method: &Method,
) -> Result<Next, RequestError> {
    let mut next = hop.clone();
    match answer.status {
        // A user agent may make a POST a GET (RFC 9110 section 15.4.2 and 15.4.3).
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => {
            if next.method == Method::POST {
                next.method = Method::GET;
                next.body = None;
                drop_payload_fields(&mut next.headers);
            }
        }
        StatusCode::SEE_OTHER => {
            if next.method != Method::HEAD {
                next.method = Method::GET;
            }
            next.body = None;
            drop_payload_fields(&mut next.headers);
        }
        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => {}
        _ => return Ok(Next::Answer),
    }
    let Some(url) = answer
        .headers
        .get(LOCATION)
        .and_then(|location| str::from_utf8(location.as_bytes()).ok())
        .and_then(|location| hop.url.join(location).ok())
    else {
        return Ok(Next::Answer);
    };
    if let Some(origin) = h2c
        && requested
            .first()
            .is_some_and(|first| !stays_on(first, &url))
    {
        // Off the h2c origin: the redirect is the answer, and the origin goes back to HTTP/1.1.
        demote(http2, origin, &hop.url);
        return Ok(Next::Answer);
    }
    if let Some(refusal) = check_redirect(&core.rules, core.loopback_only, &url, requested.len()) {
        return Err(RequestError::refused_redirect(refusal, method));
    }
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(RequestError::own(
            ErrorKind::Request,
            "builder error: URL scheme is not allowed".to_owned(),
        ));
    }
    if let Some(previous) = requested.last() {
        remove_sensitive_fields(&mut next.headers, &url, previous);
        if let Some(referer) = referer(&url, previous) {
            drop(next.headers.insert(REFERER, referer));
        }
    }
    let pooled = h2c.is_some() || (url.scheme() == "https" && proxy_of(core, &url).is_none());
    next.url = url;
    Ok(if pooled {
        Next::Pool(next)
    } else {
        Next::Reqwest(next)
    })
}

/// tower-http's: the fields of a body a redirect drops with it.
fn drop_payload_fields(headers: &mut HeaderMap) {
    for name in [
        CONTENT_TYPE,
        CONTENT_LENGTH,
        CONTENT_ENCODING,
        TRANSFER_ENCODING,
    ] {
        drop(headers.remove(name));
    }
}

/// reqwest's `remove_sensitive_headers`: a redirect to another scheme, host or port carries no
/// credentials.
fn remove_sensitive_fields(headers: &mut HeaderMap, next: &Url, previous: &Url) {
    let other = next.host_str() != previous.host_str()
        || next.port_or_known_default() != previous.port_or_known_default()
        || next.scheme() != previous.scheme();
    if other {
        for name in [AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION, WWW_AUTHENTICATE] {
            drop(headers.remove(name));
        }
        drop(headers.remove("cookie2"));
    }
}

/// reqwest's `make_referer`: the previous URL without its credentials and fragment, but none from
/// `https` to `http`.
fn referer(next: &Url, previous: &Url) -> Option<HeaderValue> {
    if next.scheme() == "http" && previous.scheme() == "https" {
        return None;
    }
    let mut referer = previous.clone();
    // Neither fails on an http or https URL, the only ones a redirect is followed from.
    if referer.set_username("").is_err() || referer.set_password(None).is_err() {
        return None;
    }
    referer.set_fragment(None);
    HeaderValue::from_str(referer.as_str()).ok()
}
