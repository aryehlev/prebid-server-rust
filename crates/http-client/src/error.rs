//! Glue (Rule B (c)): the error a request ends with, classified as bidder.go classifies Go's
//! (exchange/bidder.go:616-647), which Prebid's error codes and timeout metrics follow.
//!
//! bidder.go sends through `ctxhttp.Do`, which returns `ctx.Err()` once the request's context is
//! done, so a request that runs past its deadline fails with Go's bare `context deadline
//! exceeded`, whatever the Transport was doing; bidder.go then turns a send error that is
//! `context.DeadlineExceeded` under `errors.Is` into `errortypes.Timeout`, and returns an error
//! reading the body as it is (exchange/bidder.go:641-646), a deadline included.
//! [`RequestError::is_timeout`] is that test, and [`ErrorKind::DeadlineExceeded`] keeps Go's text
//! (`http-client.timeout-classification`, restored; pin
//! `crates/http-client/tests/errors.rs::deadline_is_timeout` against
//! `testdata/golden/errors.json`). The same pin holds the texts of the callers that call
//! `client.Do` without ctxhttp ([`RequestError::do_text`]: the floors fetcher, the currency
//! fetch, scope3) and of Go's `Client.Timeout` ([`ErrorKind::ClientTimeout`], scope3's client),
//! which name the timeout. Other errors read `<Method> "<url>": <reqwest's text>`, Go's
//! `url.Error` shape around reqwest's words (`http-client.error-texts`).
//!
//! An error keeps its cause, a transport's or reqwest's, behind [`Error::source`], where
//! [`RequestError::is_connection_refused`] and [`RequestError::is_dns_not_found`] look for the
//! causes bidder.go reports as an unreachable bidder (see [`DnsError`]).

use std::{error::Error, fmt, io, iter, sync::Arc};

use reqwest::Method;

pub use crate::unreachable::DnsError;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request's deadline passed, while it waited for a connection, for the response head or
    /// for the body: Go's `context.DeadlineExceeded`.
    DeadlineExceeded,
    /// The timeout of a client from [`Client::with_timeout`](crate::Client::with_timeout) passed
    /// before the request's deadline: Go's `Client.Timeout`, whose error is
    /// `context.DeadlineExceeded` under `errors.Is` as well and reads as `client.Do`'s, naming the
    /// timeout.
    ClientTimeout,
    /// The dial timed out (`dialer.timeout_seconds`): Go's `i/o timeout`, which is
    /// `context.DeadlineExceeded` under `errors.Is` as well.
    ConnectTimeout,
    /// The proxy environment refused the request: `HTTP_PROXY` under `REQUEST_METHOD` (Go's CGI
    /// rule), or a proxy URL reqwest cannot use.
    Proxy,
    /// The connection or the TLS handshake failed.
    Connect,
    /// A redirect could not be followed: the tenth, or a bad `Location`.
    Redirect,
    /// The response body could not be read or decompressed.
    Body,
    /// The request failed in another way.
    Request,
    /// The loopback guard refused a host that is not on the loopback interface.
    NotLoopback,
    /// The client could not be built.
    Build,
}

/// What a request failed with, shared by the clones of its error.
type Cause = Arc<dyn Error + Send + Sync>;

/// A failed request, or a client that could not be built.
#[derive(Debug, Clone)]
pub struct RequestError {
    kind: ErrorKind,
    /// Go's `url.Error` prefix, `<Method> "<url>"`, for the errors of sending a request, which
    /// Go's `client.Do` wraps in one.
    prefix: Option<String>,
    /// The words after the prefix: the text of the error `client.Do` wraps, or of the body
    /// reader's.
    message: String,
    source: Option<Cause>,
}

/// Go's `context.DeadlineExceeded` text.
const DEADLINE_EXCEEDED: &str = "context deadline exceeded";

/// What Go's `Client.do` adds to a send error once `Client.Timeout` has passed
/// (net/http/client.go:736-738).
pub(crate) const AWAITING_HEADERS: &str = " (Client.Timeout exceeded while awaiting headers)";

/// What Go's `cancelTimerBody` adds to a body error once `Client.Timeout` has passed
/// (net/http/client.go:993-995).
pub(crate) const READING_BODY: &str =
    " (Client.Timeout or context cancellation while reading body)";

/// Go's refusal of `HTTP_PROXY` in a CGI environment (golang.org/x/net/http/httpproxy).
pub(crate) const CGI_REFUSAL: &str =
    "refusing to use HTTP_PROXY value in CGI environment; see golang.org/s/cgihttpproxy";

impl RequestError {
    /// A transport's error of `kind`, reading as `error` reads: what Go's `RoundTripper` returns
    /// (a test's `errors.New("expected")`, the parity transport's refused dial). Sent through
    /// [`Client::send`](crate::Client::send), it gains Go's `url.Error` prefix, `<Method>
    /// "<url>": `; as the error of a body
    /// ([`Response::with_body_error`](crate::Response::with_body_error)) it reads as it is. `error`
    /// and its sources stay behind [`Error::source`], where
    /// [`is_connection_refused`](Self::is_connection_refused) and
    /// [`is_dns_not_found`](Self::is_dns_not_found) look.
    #[must_use]
    pub fn new<E>(kind: ErrorKind, error: E) -> Self
    where
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        let cause: Cause = Arc::from(error.into());
        Self {
            kind,
            prefix: None,
            message: cause.to_string(),
            source: Some(cause),
        }
    }

    /// The request ran past its deadline: Go's `context.DeadlineExceeded`, as a transport returns
    /// it once its request's deadline has passed.
    #[must_use]
    pub fn deadline() -> Self {
        Self {
            kind: ErrorKind::DeadlineExceeded,
            prefix: None,
            message: DEADLINE_EXCEEDED.to_owned(),
            source: None,
        }
    }

    /// What went wrong.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Whether Go's `errors.Is(err, context.DeadlineExceeded)` holds: the request ran past its
    /// deadline or its client's timeout, or its dial timed out. bidder.go reports such an error
    /// from [`Client::send`](crate::Client::send) as `errortypes.Timeout`, and returns one from
    /// [`Response::bytes`](crate::Response::bytes) unclassified.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::DeadlineExceeded | ErrorKind::ClientTimeout | ErrorKind::ConnectTimeout
        )
    }

    /// Whether Go's `errors.Is(err, syscall.ECONNREFUSED)` holds: the host or the proxy refused
    /// the connection. With [`is_dns_not_found`](Self::is_dns_not_found) it is bidder.go's
    /// `isBidderUnreachableError` (exchange/non_bid_reason.go:53-57), seat non-bid code 103.
    #[must_use]
    pub fn is_connection_refused(&self) -> bool {
        self.causes().any(is_refused)
    }

    /// Whether Go's `errors.As(err, &dnsErr) && dnsErr.IsNotFound` holds: the host's name does
    /// not exist or has no address ([`DnsError::is_not_found`]).
    #[must_use]
    pub fn is_dns_not_found(&self) -> bool {
        self.causes().any(|cause| {
            cause
                .downcast_ref::<DnsError>()
                .is_some_and(DnsError::is_not_found)
        })
    }

    /// The text Go's `client.Do` gives where [`Display`](fmt::Display) gives ctxhttp's: for a
    /// caller that calls `client.Do` itself (`floors/fetcher.go:270`,
    /// `currency/rate_converter.go:58`, `modules/scope3/rtd/module.go:430`). A deadline that
    /// passed while the request was sent keeps Go's `url.Error` around it, `Post "<url>":
    /// context deadline exceeded`; every other error reads as it displays.
    #[must_use]
    pub fn do_text(&self) -> String {
        self.prefix.as_ref().map_or_else(
            || self.message.clone(),
            |prefix| format!("{prefix}: {}", self.message),
        )
    }

    /// The error and its sources, outermost first.
    fn causes(&self) -> impl Iterator<Item = &(dyn Error + 'static)> {
        iter::successors(self.source(), |&cause| cause.source())
    }

    /// The error `client.Do` returns for this error of sending the request `method url`: Go's
    /// `url.Error` around it, unless it has one (a stopped redirect's names the next URL).
    #[must_use]
    pub(crate) fn sent(self, method: &Method, url: &str) -> Self {
        match self.prefix {
            Some(_) => self,
            None => Self {
                prefix: Some(prefix(method, url)),
                ..self
            },
        }
    }

    /// The error once Go's `Client.Timeout` has passed: its text followed by `suffix`, which
    /// says whether the request awaited its head or read its body. Go keeps the text only, so
    /// the cause goes.
    #[must_use]
    pub(crate) fn client_timeout(self, suffix: &str) -> Self {
        Self {
            kind: ErrorKind::ClientTimeout,
            prefix: self.prefix,
            message: format!("{}{suffix}", self.message),
            source: None,
        }
    }

    /// ctxhttp's error once the request's context is done: `ctx.Err()`, Go's bare deadline,
    /// while [`do_text`](Self::do_text) keeps `client.Do`'s text. Go's `ctx.Err()` carries no
    /// cause, so the cause goes.
    #[must_use]
    pub(crate) fn context_done(self) -> Self {
        Self {
            kind: ErrorKind::DeadlineExceeded,
            source: None,
            ..self
        }
    }

    /// An error of the crate's own about the request it was handed, which
    /// [`Client::send`](crate::Client::send) wraps in the request's `url.Error`.
    pub(crate) const fn own(kind: ErrorKind, message: String) -> Self {
        Self {
            kind,
            prefix: None,
            message,
            source: None,
        }
    }

    /// reqwest could not build the client.
    pub(crate) fn build(error: reqwest::Error) -> Self {
        Self {
            kind: ErrorKind::Build,
            prefix: None,
            message: error_chain(&error),
            source: Some(Arc::new(error)),
        }
    }

    /// reqwest's error reading a response body. Go returns the reader's error as it is
    /// (bidder.go's `io.ReadAll`), without a `url.Error` around it.
    pub(crate) fn body(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            ErrorKind::DeadlineExceeded
        } else {
            ErrorKind::Body
        };
        Self {
            kind,
            prefix: None,
            message: error_chain(&error),
            source: Some(Arc::new(error)),
        }
    }

    /// reqwest's error sending a request whose method is `method`: Go's `RoundTrip` error, which
    /// [`Client::send`](crate::Client::send) wraps in the request's `url.Error`, unless the
    /// redirect policy stopped the chain, whose `url.Error` names the next URL.
    pub(crate) fn reqwest(error: reqwest::Error, method: &Method) -> Self {
        if let Some(refusal) = redirect_refusal(&error) {
            return Self {
                kind: refusal.kind,
                prefix: Some(prefix(method, &refusal.url)),
                message: refusal.message.clone(),
                source: Some(Arc::new(error)),
            };
        }
        let kind = if error.is_connect() && error.is_timeout() {
            ErrorKind::ConnectTimeout
        } else if error.is_timeout() {
            ErrorKind::DeadlineExceeded
        } else if error.is_connect() {
            ErrorKind::Connect
        } else if error.is_redirect() {
            ErrorKind::Redirect
        } else {
            ErrorKind::Request
        };
        Self {
            kind,
            prefix: None,
            message: error_chain(&error),
            source: Some(Arc::new(error)),
        }
    }
}

/// Whether `cause` is a refused connection's I/O error: Go's `syscall.ECONNREFUSED`.
fn is_refused(cause: &(dyn Error + 'static)) -> bool {
    cause
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::ConnectionRefused)
}

/// Go's `url.Error` prefix: the method as `urlErrorOp` writes it ("Get", "Post") and the URL,
/// quoted as `%q` quotes it.
fn prefix(method: &Method, url: &str) -> String {
    let name = method.as_str();
    let op = match name.split_at_checked(1) {
        Some((first, rest)) if name.is_ascii() => format!("{first}{}", rest.to_ascii_lowercase()),
        _ => name.to_owned(),
    };
    format!("{op} {url:?}")
}

/// reqwest's error and its sources, joined as Go joins wrapped errors' texts, without the URL
/// reqwest puts in its own text.
fn error_chain(error: &reqwest::Error) -> String {
    let mut text = without_url(error);
    let mut source = error.source();
    while let Some(next) = source {
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text
}

/// reqwest's own text for `error`, without its ` for url (...)`.
fn without_url(error: &reqwest::Error) -> String {
    let text = error.to_string();
    error
        .url()
        .and_then(|url| text.strip_suffix(&format!(" for url ({url})")))
        .map_or_else(|| text.clone(), str::to_owned)
}

/// The crate's own refusal inside `error`, when its redirect policy stopped the chain.
fn redirect_refusal(error: &reqwest::Error) -> Option<&RedirectRefusal> {
    let mut source = error.source();
    while let Some(next) = source {
        if let Some(own) = next.downcast_ref::<RedirectRefusal>() {
            return Some(own);
        }
        source = next.source();
    }
    None
}

/// Why the crate's redirect policy stopped a redirect chain: Go's `defaultCheckRedirect` refused
/// the next request, or the Transport would have refused to send it. Go's `url.Error` then names
/// the next URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedirectRefusal {
    pub(crate) kind: ErrorKind,
    pub(crate) url: String,
    pub(crate) message: String,
}

impl fmt::Display for RedirectRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for RedirectRefusal {}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.kind == ErrorKind::DeadlineExceeded {
            return f.write_str(DEADLINE_EXCEEDED);
        }
        match &self.prefix {
            Some(prefix) => write!(f, "{prefix}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl Error for RequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        let source: &(dyn Error + 'static) = self.source.as_deref()?;
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use reqwest::Method;

    use super::{AWAITING_HEADERS, DnsError, ErrorKind, READING_BODY, RequestError, prefix};

    // Go: net/http/client.go urlErrorOp and net/url/url.go Error.Error.
    #[test]
    fn prefix_is_go_url_error_shape() {
        assert_eq!(
            prefix(&Method::POST, "http://a.test/p"),
            "Post \"http://a.test/p\"",
            "POST"
        );
        assert_eq!(
            prefix(&Method::GET, "http://a.test/"),
            "Get \"http://a.test/\"",
            "GET"
        );
        let deadline = RequestError::deadline();
        assert!(deadline.is_timeout(), "a deadline is a timeout");
        assert_eq!(
            deadline.to_string(),
            "context deadline exceeded",
            "Go's text"
        );
        let own = RequestError::own(ErrorKind::Proxy, "no".to_owned())
            .sent(&Method::GET, "http://a.test/");
        assert!(!own.is_timeout(), "a proxy refusal is not a timeout");
        assert_eq!(own.to_string(), "Get \"http://a.test/\": no", "prefixed");
    }

    // Go: net/http/client.go Client.do (uerr and the Client.Timeout suffix) and
    // golang.org/x/net/context/ctxhttp Do (ctx.Err() once the context is done).
    #[test]
    fn readings_like_go() {
        let sent = RequestError::deadline().sent(&Method::POST, "http://a.test/e");
        assert_eq!(sent.to_string(), "context deadline exceeded", "ctxhttp's");
        assert_eq!(
            sent.do_text(),
            "Post \"http://a.test/e\": context deadline exceeded",
            "client.Do's"
        );
        let client = RequestError::deadline()
            .sent(&Method::POST, "http://a.test/e")
            .client_timeout(AWAITING_HEADERS);
        assert_eq!(
            (client.kind(), client.is_timeout(), client.to_string()),
            (
                ErrorKind::ClientTimeout,
                true,
                "Post \"http://a.test/e\": context deadline exceeded (Client.Timeout exceeded \
                 while awaiting headers)"
                    .to_owned()
            ),
            "Client.Timeout while awaiting headers"
        );
        let body = RequestError::deadline().client_timeout(READING_BODY);
        assert_eq!(
            body.to_string(),
            "context deadline exceeded (Client.Timeout or context cancellation while reading \
             body)",
            "Client.Timeout while reading the body"
        );
        let refused = RequestError::new(
            ErrorKind::Connect,
            io::Error::from(io::ErrorKind::ConnectionRefused),
        )
        .sent(&Method::GET, "http://a.test/");
        assert!(refused.is_connection_refused(), "a refused dial");
        assert!(
            !RequestError::new(
                ErrorKind::Request,
                io::Error::from(io::ErrorKind::ConnectionReset)
            )
            .is_connection_refused(),
            "a reset is not a refusal"
        );
        let done = refused.clone().context_done();
        assert_eq!(
            (done.is_timeout(), done.is_connection_refused()),
            (true, false),
            "ctx.Err() carries no cause"
        );
        assert_eq!(
            done.do_text(),
            refused.to_string(),
            "client.Do's text stays"
        );
        let expected = RequestError::new(ErrorKind::Request, "expected")
            .sent(&Method::GET, "http://example.com/?id=one");
        assert_eq!(
            expected.to_string(),
            "Get \"http://example.com/?id=one\": expected",
            "a transport's error inside Go's url.Error"
        );
        let missing = RequestError::new(
            ErrorKind::Connect,
            DnsError::new("bidder.test".to_owned(), "no such host".to_owned(), true),
        );
        assert!(missing.is_dns_not_found(), "a name that does not exist");
        assert!(!expected.is_dns_not_found(), "no lookup failed");
    }
}
