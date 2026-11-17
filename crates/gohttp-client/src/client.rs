//! Go `net/http.Client`: `Do` with `Timeout`, redirects (`CheckRedirect`, header copying and
//! stripping, `Referer`), URL-userinfo basic auth and the `*url.Error` wrapping of every error.

use std::{
    borrow::Cow,
    fmt,
    future::poll_fn,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use bytes::Bytes;
use pbs_gocompat::{
    base64,
    http::{Header, canonical_header_key},
    strconv::quote,
    url::{Url, UrlError as ParseUrlError, path_unescape},
};

use crate::{
    context::Context,
    error::{HttpError, UrlError},
    httpguts::idna_to_ascii,
    request::{Body, GetBody, Request},
    response::{ClientTimer, Response},
    transport::{RoundTripper, Transport, TransportOptions},
};

/// Go: `Client.CheckRedirect`: called before following a redirect with the upcoming request
/// and the requests made so far, oldest first.
pub type CheckRedirect = Arc<dyn Fn(&Request, &[Request]) -> Result<(), HttpError> + Send + Sync>;

/// Go: `*http.Client` (without a cookie `Jar`).
#[derive(Clone, Default)]
pub struct Client {
    /// Go: `Transport` (`None`: the process-wide default transport).
    pub transport: Option<Arc<dyn RoundTripper>>,
    /// Go: `CheckRedirect` (`None`: stop after 10 redirects).
    pub check_redirect: Option<CheckRedirect>,
    /// Go: `Timeout` (zero: none).
    pub timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("transport", &self.transport)
            .field("check_redirect", &self.check_redirect.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Go: `http.DefaultTransport` (HTTP/1.1 only).
#[must_use]
pub fn default_transport() -> Arc<dyn RoundTripper> {
    static DEFAULT: OnceLock<Arc<dyn RoundTripper>> = OnceLock::new();
    Arc::clone(
        DEFAULT.get_or_init(|| Arc::new(Transport::new(TransportOptions::default_transport()))),
    )
}

/// Go: `http.DefaultClient`.
#[must_use]
pub fn default_client() -> &'static Client {
    static DEFAULT: OnceLock<Client> = OnceLock::new();
    DEFAULT.get_or_init(Client::default)
}

/// Go: `http.ErrSchemeMismatch`.
pub const ERR_SCHEME_MISMATCH: HttpError =
    HttpError::Static("http: server gave HTTP response to HTTPS client");

/// Go: `resolvePath`.
fn resolve_path(base: &str, reference: &str) -> String {
    let full = if reference.is_empty() {
        base.to_owned()
    } else if !reference.starts_with('/') {
        let keep = base.rfind('/').map_or(0, |i| i + 1);
        format!("{}{reference}", base.get(..keep).unwrap_or_default())
    } else {
        reference.to_owned()
    };
    if full.is_empty() {
        return String::new();
    }
    let mut dst = String::from("/");
    let mut first = true;
    let mut last = "";
    for elem in full.split('/') {
        last = elem;
        if elem == "." {
            first = false;
            continue;
        }
        if elem == ".." {
            let kept = dst.get(1..).unwrap_or_default();
            let index = kept.rfind('/');
            let prefix = index
                .and_then(|i| kept.get(..i))
                .unwrap_or_default()
                .to_owned();
            dst = String::from("/");
            if index.is_none() {
                first = true;
            } else {
                dst.push_str(&prefix);
            }
        } else {
            if !first {
                dst.push('/');
            }
            dst.push_str(elem);
            first = false;
        }
    }
    if last == "." || last == ".." {
        dst.push('/');
    }
    if dst.as_bytes().get(1) == Some(&b'/') {
        let _slash = dst.remove(0);
    }
    dst
}

/// Go: `URL.setPath` (its error is ignored by `ResolveReference`).
fn set_path(url: &mut Url, p: &str) {
    let Ok(path) = path_unescape(p) else {
        return;
    };
    let escaped = if path == "*" {
        "%2A".to_owned()
    } else {
        Url {
            path: path.clone(),
            ..Url::default()
        }
        .escaped_path()
    };
    url.path = path;
    url.raw_path = if p == escaped {
        String::new()
    } else {
        p.to_owned()
    };
}

/// Go: `URL.ResolveReference`.
#[must_use]
pub fn resolve_reference(base: &Url, reference: &Url) -> Url {
    let mut url = reference.clone();
    if reference.scheme.is_empty() {
        url.scheme.clone_from(&base.scheme);
    }
    if !reference.scheme.is_empty() || !reference.host.is_empty() || reference.user.is_some() {
        set_path(&mut url, &resolve_path(&reference.escaped_path(), ""));
        return url;
    }
    if !reference.opaque.is_empty() {
        url.user = None;
        url.host.clear();
        url.path.clear();
        return url;
    }
    if reference.path.is_empty() && !reference.force_query && reference.raw_query.is_empty() {
        url.raw_query.clone_from(&base.raw_query);
        if reference.fragment.is_empty() {
            url.fragment.clone_from(&base.fragment);
            url.raw_fragment.clone_from(&base.raw_fragment);
        }
    }
    if reference.path.is_empty() && !base.opaque.is_empty() {
        url.opaque.clone_from(&base.opaque);
        url.user = None;
        url.host.clear();
        url.path.clear();
        return url;
    }
    url.host.clone_from(&base.host);
    url.user.clone_from(&base.user);
    set_path(
        &mut url,
        &resolve_path(&base.escaped_path(), &reference.escaped_path()),
    );
    url
}

/// Go: `URL.Parse(ref)`: `ref` parsed and resolved against `base`.
///
/// # Errors
///
/// The `*url.Error` of parsing `reference`.
pub fn parse_url_reference(base: &Url, reference: &str) -> Result<Url, ParseUrlError> {
    Url::parse(reference).map(|r| resolve_reference(base, &r))
}

/// Go: `stripPassword`.
fn strip_password(u: &Url) -> String {
    let s = u.to_string();
    match &u.user {
        Some(user) if user.password().is_some() => {
            s.replacen(&format!("{user}@"), &format!("{}:***@", user.username()), 1)
        }
        Some(_) | None => s,
    }
}

/// Go: `urlErrorOp`.
fn url_error_op(method: &str) -> String {
    if method.is_empty() {
        return "Get".to_owned();
    }
    // Go: ascii.ToLower succeeds only for printable ASCII; anything else is returned as is.
    match method.split_at_checked(1) {
        Some((first, rest)) if method.bytes().all(|b| (b' '..=b'~').contains(&b)) => {
            format!("{first}{}", rest.to_ascii_lowercase())
        }
        _ => method.to_owned(),
    }
}

/// Go: `refererForURL`.
fn referer_for_url(last: &Url, new: &Url, explicit: &str) -> String {
    if last.scheme == "https" && new.scheme == "http" {
        return String::new();
    }
    if !explicit.is_empty() {
        return explicit.to_owned();
    }
    let referer = last.to_string();
    match &last.user {
        Some(user) => referer.replacen(&format!("{user}@"), "", 1),
        None => referer,
    }
}

/// Go: `isDomainOrSubdomain`.
fn is_domain_or_subdomain(sub: &str, parent: &str) -> bool {
    if sub == parent {
        return true;
    }
    if sub.contains([':', '%']) {
        return false;
    }
    if !sub.ends_with(parent) {
        return false;
    }
    sub.len()
        .checked_sub(parent.len() + 1)
        .and_then(|i| sub.as_bytes().get(i))
        == Some(&b'.')
}

/// Go: `idnaASCIIFromURL`.
fn idna_ascii_from_url(u: &Url) -> String {
    let host = u.hostname();
    idna_to_ascii(host).unwrap_or_else(|_err| host.to_owned())
}

/// Go: `shouldCopyHeaderOnRedirect`.
fn should_copy_header_on_redirect(initial: &Url, dest: &Url) -> bool {
    is_domain_or_subdomain(&idna_ascii_from_url(dest), &idna_ascii_from_url(initial))
}

/// What Go's `Client.do` keeps of `reqs[0]`.
struct FirstRequest {
    method: String,
    url: Url,
    get_body: Option<GetBody>,
    content_length: i64,
    outgoing_length: i64,
    ctx: Context,
    /// Go: `makeHeadersCopier`'s `ireqhdr`.
    header: Header,
}

/// Go: `redirectBehavior`: `(redirect method, should redirect, include body)`.
fn redirect_behavior(method: &str, status: u16, first: &FirstRequest) -> (String, bool, bool) {
    match status {
        301..=303 => {
            let m = if method != "GET" && method != "HEAD" {
                "GET"
            } else {
                method
            };
            (m.to_owned(), true, false)
        }
        307 | 308 => {
            let follow = first.get_body.is_some() || first.outgoing_length == 0;
            (method.to_owned(), follow, true)
        }
        _ => (String::new(), false, false),
    }
}

/// Go: the copier `makeHeadersCopier` returns.
fn copy_headers(src: &Header, dst: &mut Header, strip_sensitive: bool, strip_body: bool) {
    for (k, vv) in src.iter() {
        let canon = canonical_header_key(k);
        let sensitive = matches!(
            canon.as_str(),
            "Authorization"
                | "Www-Authenticate"
                | "Cookie"
                | "Cookie2"
                | "Proxy-Authorization"
                | "Proxy-Authenticate"
        );
        let body = matches!(
            canon.as_str(),
            "Content-Encoding" | "Content-Language" | "Content-Location" | "Content-Type"
        );
        let stripped = (sensitive && strip_sensitive) || (body && strip_body);
        if !stripped {
            dst.insert_raw(k, vv.to_vec());
        }
    }
}

/// Go: `defaultCheckRedirect`.
const fn default_check_redirect(_req: &Request, via: &[Request]) -> Result<(), HttpError> {
    if via.len() >= 10 {
        return Err(HttpError::Static("stopped after 10 redirects"));
    }
    Ok(())
}

/// Go: `basicAuth`.
fn basic_auth(username: &str, password: &str) -> String {
    base64::STD_ENCODING.encode_to_string(format!("{username}:{password}").as_bytes())
}

/// Go: `&url.Error{Op: urlErrorOp(method), URL: url, Err: err}`.
fn url_error(method: &str, url: String, err: HttpError) -> HttpError {
    HttpError::Url(Box::new(UrlError {
        op: url_error_op(method),
        url,
        err,
    }))
}

/// Go: `maxBodySlurpSize`: how much of a redirect response is read so its connection can be
/// reused.
const MAX_BODY_SLURP_SIZE: i64 = 2 << 10;

/// Go's `(resp, err)` from `Client.Do`.
type DoResult = (Option<Response>, Option<HttpError>);

/// What `Client.do` uses of the request a hop sent (Go: `req`).
struct SentRequest {
    method: String,
    url: Url,
    host: String,
}

impl SentRequest {
    /// The request for `CheckRedirect`'s `via` when the round tripper did not hand it back.
    fn into_request(self, ctx: &Context) -> Request {
        Request {
            method: self.method,
            url: self.url,
            header: Header::new(),
            body: Body::Nil,
            get_body: None,
            content_length: 0,
            transfer_encoding: Vec::new(),
            close: false,
            host: self.host,
            trailer: None,
            ctx: ctx.clone(),
        }
    }
}

/// The redirect state of Go's `Client.do` loop.
struct Redirects {
    /// Go: `reqs` (all but the request being prepared).
    via: Vec<Request>,
    /// Go: `includeBody`.
    include_body: bool,
    /// Go: `stripSensitiveHeaders`.
    strip_sensitive: bool,
}

/// Go: `Client.do` building the next hop (`req = &Request{...}` through the `Referer`).
fn build_hop(
    first: &FirstRequest,
    state: &mut Redirects,
    sent: &SentRequest,
    method: String,
    url: Url,
    loc: &str,
) -> Result<Request, HttpError> {
    // Go: a custom Host survives a relative redirect.
    let host = if !sent.host.is_empty()
        && sent.host != sent.url.host
        && Url::parse(loc).is_ok_and(|l| !l.is_abs())
    {
        sent.host.clone()
    } else {
        String::new()
    };
    let mut hop = Request {
        method,
        url,
        header: Header::new(),
        body: Body::Nil,
        get_body: None,
        content_length: 0,
        transfer_encoding: Vec::new(),
        close: false,
        host,
        trailer: None,
        ctx: first.ctx.clone(),
    };
    if state.include_body
        && let Some(get_body) = &first.get_body
    {
        hop.body = get_body.call()?;
        hop.get_body = Some(get_body.clone());
        hop.content_length = first.content_length;
    }
    // Go: copy the first request's headers before setting the Referer.
    if !state.strip_sensitive
        && first.url.host != hop.url.host
        && !should_copy_header_on_redirect(&first.url, &hop.url)
    {
        state.strip_sensitive = true;
    }
    copy_headers(
        &first.header,
        &mut hop.header,
        state.strip_sensitive,
        !state.include_body,
    );
    let referer = referer_for_url(&sent.url, &hop.url, hop.header.get("Referer"));
    if !referer.is_empty() {
        hop.header.set("Referer", referer);
    }
    Ok(hop)
}

impl Client {
    /// Go: `c.transport()`.
    fn round_tripper(&self) -> Arc<dyn RoundTripper> {
        self.transport.clone().unwrap_or_else(default_transport)
    }

    /// Go: `c.send` and `send`: basic auth from the URL, the `Timeout` deadline and the
    /// round trip. The flag is Go's `didTimeout()`.
    async fn send(
        &self,
        mut req: Request,
        deadline: Option<Instant>,
    ) -> (Result<Response, HttpError>, bool) {
        if let Some(user) = &req.url.user
            && req.header.get("Authorization").is_empty()
        {
            let auth = basic_auth(user.username(), user.password().unwrap_or(""));
            req.header.set("Authorization", format!("Basic {auth}"));
        }
        // Go: setRequestCancel for a known transport: a context deadline unless the request's
        // context ends sooner.
        let mut timer = match deadline {
            Some(d)
                if req
                    .ctx
                    .deadline()
                    .is_none_or(|ctx_deadline| d < ctx_deadline) =>
            {
                let (ctx, cancel) = req.ctx.with_deadline(d);
                req.ctx = ctx;
                Some(ClientTimer {
                    deadline: Some(d),
                    stop: Some(cancel),
                })
            }
            Some(_) | None => None,
        };
        match self.round_tripper().round_trip(req).await {
            Err(e) => {
                if let Some(stop) = timer.as_mut().and_then(|t| t.stop.take()) {
                    stop.cancel();
                }
                let did_timeout = timer
                    .as_ref()
                    .and_then(|t| t.deadline)
                    .is_some_and(|d| Instant::now() > d);
                // Go: a TLS record header that reads "HTTP/".
                let e = if matches!(&e, HttpError::RecordHeader(h) if h == b"HTTP/") {
                    ERR_SCHEME_MISMATCH
                } else {
                    e
                };
                (Err(e), did_timeout)
            }
            Ok(mut resp) => {
                if deadline.is_some() {
                    resp.body.set_timer(timer.unwrap_or(ClientTimer {
                        deadline: None,
                        stop: None,
                    }));
                }
                (Ok(resp), false)
            }
        }
    }

    /// Go: `Client.Do(req)` with both results: on a `CheckRedirect` error Go returns the last
    /// response (its body closed) together with the error.
    pub async fn do_full(&self, req: Request) -> DoResult {
        let deadline = (!self.timeout.is_zero()).then(|| Instant::now() + self.timeout);
        let first = FirstRequest {
            method: req.method.clone(),
            url: req.url.clone(),
            get_body: req.get_body.clone(),
            content_length: req.content_length,
            outgoing_length: req.outgoing_length(),
            ctx: req.ctx.clone(),
            header: req.header.clone(),
        };
        let mut state = Redirects {
            via: Vec::new(),
            include_body: true,
            strip_sensitive: false,
        };
        let mut next = req;
        loop {
            let sent = SentRequest {
                method: next.method.clone(),
                url: next.url.clone(),
                host: next.host.clone(),
            };
            let (result, did_timeout) = self.send(next, deadline).await;
            let resp = match result {
                Ok(r) => r,
                Err(e) => {
                    let e = if deadline.is_some() && did_timeout {
                        HttpError::Timeout(Cow::Owned(format!(
                            "{e} (Client.Timeout exceeded while awaiting headers)"
                        )))
                    } else {
                        e
                    };
                    return (
                        None,
                        Some(url_error(&first.method, strip_password(&sent.url), e)),
                    );
                }
            };
            let (method, should_redirect, include_body_on_hop) =
                redirect_behavior(&sent.method, resp.status_code, &first);
            if !should_redirect {
                return (Some(resp), None);
            }
            if !include_body_on_hop {
                state.include_body = false;
            }
            next = match self.next_hop(&first, &mut state, sent, method, resp).await {
                Ok(hop) => hop,
                Err(done) => return *done,
            };
        }
    }

    /// Go: the top of `Client.do`'s loop after a redirect response: the next request, or the
    /// final result.
    async fn next_hop(
        &self,
        first: &FirstRequest,
        state: &mut Redirects,
        sent: SentRequest,
        method: String,
        mut resp: Response,
    ) -> Result<Request, Box<DoResult>> {
        let loc = resp.header.get("Location").to_owned();
        if loc.is_empty() {
            return Err(Box::new((Some(resp), None)));
        }
        let built = parse_url_reference(&sent.url, &loc)
            .map_err(|e| {
                HttpError::Message(format!(
                    "failed to parse Location header {}: {e}",
                    quote(&loc)
                ))
            })
            .and_then(|url| build_hop(first, state, &sent, method, url, &loc));
        let hop = match built {
            Ok(hop) => hop,
            Err(e) => {
                resp.body.close();
                // Go: uerr reports the URL of the request that produced the response.
                let prev = resp.request.as_ref().map_or(&sent.url, |r| &r.url);
                let err = url_error(&first.method, strip_password(prev), e);
                return Err(Box::new((None, Some(err))));
            }
        };
        let echoed = resp.request.take();
        let had_request = echoed.is_some();
        state
            .via
            .push(echoed.map_or_else(|| sent.into_request(&first.ctx), |r| *r));
        let checked = self.check_redirect.as_ref().map_or_else(
            || default_check_redirect(&hop, &state.via),
            |f| f(&hop, &state.via),
        );
        // Go: resp.Request stays the request that produced the response.
        let mut give_back = |last_resp: &mut Response| {
            let last = state.via.pop();
            if had_request {
                last_resp.request = last.map(Box::new);
            }
        };
        if checked == Err(HttpError::UseLastResponse) {
            give_back(&mut resp);
            return Err(Box::new((Some(resp), None)));
        }
        // Go: read a little of the body so the connection can be reused, then close it.
        if resp.content_length == -1 || resp.content_length <= MAX_BODY_SLURP_SIZE {
            drain(&mut resp, MAX_BODY_SLURP_SIZE.unsigned_abs()).await;
        }
        resp.body.close();
        if let Err(e) = checked {
            give_back(&mut resp);
            let err = HttpError::Url(Box::new(UrlError {
                op: url_error_op(&first.method),
                url: loc,
                err: e,
            }));
            return Err(Box::new((Some(resp), Some(err))));
        }
        Ok(hop)
    }

    /// Go: `Client.Do(req)`.
    ///
    /// # Errors
    ///
    /// A `*url.Error` ([`HttpError::Url`]) wrapping the transport or redirect error.
    pub async fn do_request(&self, req: Request) -> Result<Response, HttpError> {
        match self.do_full(req).await {
            (Some(resp), None) => Ok(resp),
            (_, Some(e)) => Err(e),
            (None, None) => Err(HttpError::Static("http: nil Response")),
        }
    }

    /// Go: `Client.Get(url)`.
    ///
    /// # Errors
    ///
    /// The `NewRequest` error, or as [`Client::do_request`].
    pub async fn get(&self, url: &str) -> Result<Response, HttpError> {
        let req = Request::new("GET", url, Body::Nil)?;
        self.do_request(req).await
    }

    /// Go: `Client.Head(url)`.
    ///
    /// # Errors
    ///
    /// The `NewRequest` error, or as [`Client::do_request`].
    pub async fn head(&self, url: &str) -> Result<Response, HttpError> {
        let req = Request::new("HEAD", url, Body::Nil)?;
        self.do_request(req).await
    }

    /// Go: `Client.Post(url, contentType, body)`.
    ///
    /// # Errors
    ///
    /// The `NewRequest` error, or as [`Client::do_request`].
    pub async fn post(
        &self,
        url: &str,
        content_type: &str,
        body: Body,
    ) -> Result<Response, HttpError> {
        let mut req = Request::new("POST", url, body)?;
        req.header.set("Content-Type", content_type);
        self.do_request(req).await
    }

    /// A `POST` of an in-memory body (Go: `Client.Post(url, contentType, bytes.NewReader(b))`).
    ///
    /// # Errors
    ///
    /// As [`Client::post`].
    pub async fn post_bytes(
        &self,
        url: &str,
        content_type: &str,
        body: Bytes,
    ) -> Result<Response, HttpError> {
        self.post(url, content_type, Body::Bytes(body)).await
    }
}

/// Go: `io.CopyN(io.Discard, resp.Body, n)`.
async fn drain(resp: &mut Response, n: u64) {
    let mut left = usize::try_from(n).unwrap_or(usize::MAX);
    let mut buf = vec![0_u8; left.min(8 << 10)];
    while left > 0 {
        let want = left.min(buf.len());
        let Some(dst) = buf.get_mut(..want) else {
            return;
        };
        let (got, err) = poll_fn(|cx| resp.body.poll_read_go(cx, dst)).await;
        left = left.saturating_sub(got);
        if err.is_some() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    fn resolve(base: &str, r: &str) -> Result<String, String> {
        let b = Url::parse(base).map_err(|e| e.to_string())?;
        parse_url_reference(&b, r)
            .map(|u| u.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn resolve_reference_rfc3986() -> Result<(), String> {
        let base = "http://a/b/c/d;p?q";
        let cases = [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../..", "http://a/"),
            ("../../../g", "http://a/g"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("https://other/x", "https://other/x"),
        ];
        for (r, want) in cases {
            ensure_eq(&resolve(base, r)?, &want.to_owned(), r)?;
        }
        Ok(())
    }

    #[test]
    fn redirect_helpers() -> Result<(), String> {
        ensure_eq(&url_error_op(""), &"Get".to_owned(), "empty op")?;
        ensure_eq(&url_error_op("POST"), &"Post".to_owned(), "post op")?;
        ensure_eq(&url_error_op("get"), &"get".to_owned(), "lower op")?;
        // Go returns a method that is not printable ASCII unchanged, even when its first
        // character is not ASCII.
        ensure_eq(
            &url_error_op("\u{dc}BER"),
            &"\u{dc}BER".to_owned(),
            "non-ASCII op",
        )?;
        ensure_eq(&url_error_op("GE\tT"), &"GE\tT".to_owned(), "control op")?;
        ensure_eq(
            &is_domain_or_subdomain("a.foo.com", "foo.com"),
            &true,
            "subdomain",
        )?;
        ensure_eq(
            &is_domain_or_subdomain("afoo.com", "foo.com"),
            &false,
            "suffix only",
        )?;
        ensure_eq(&is_domain_or_subdomain("[::1]", "::1"), &false, "ipv6")?;
        let u = Url::parse("http://user:pw@host/p").map_err(|e| e.to_string())?;
        ensure_eq(
            &strip_password(&u),
            &"http://user:***@host/p".to_owned(),
            "strip",
        )?;
        let last = Url::parse("https://u:p@a.example/x").map_err(|e| e.to_string())?;
        let new_http = Url::parse("http://b.example/").map_err(|e| e.to_string())?;
        let new_https = Url::parse("https://b.example/").map_err(|e| e.to_string())?;
        ensure_eq(
            &referer_for_url(&last, &new_http, ""),
            &String::new(),
            "downgrade",
        )?;
        ensure_eq(
            &referer_for_url(&last, &new_https, ""),
            &"https://a.example/x".to_owned(),
            "userinfo stripped",
        )
    }
}
