//! Glue (Rule B (c)): the error a request ends with, classified as bidder.go classifies Go's
//! (exchange/bidder.go:616-647), which Prebid's error codes and timeout metrics follow.
//!
//! bidder.go sends through `ctxhttp.Do`, which returns `ctx.Err()` once the request's context is
//! done, so a request that runs past its deadline fails with Go's bare `context deadline
//! exceeded`, whatever the Transport was doing; bidder.go then turns every error that is
//! `context.DeadlineExceeded` under `errors.Is` into `errortypes.Timeout`.
//! [`RequestError::is_timeout`] is that test, and [`ErrorKind::DeadlineExceeded`] keeps Go's text
//! (`http-client.timeout-classification`, restored; pin
//! `crates/http-client/tests/errors.rs::deadline_is_timeout` against
//! `testdata/golden/errors.json`). Other errors read `<Method> "<url>": <reqwest's text>`, Go's
//! `url.Error` shape around reqwest's words (`http-client.error-texts`).

use std::{error::Error, fmt};

use reqwest::Method;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request's deadline passed, while it waited for a connection, for the response head or
    /// for the body: Go's `context.DeadlineExceeded`.
    DeadlineExceeded,
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

/// A failed request, or a client that could not be built.
#[derive(Debug)]
pub struct RequestError {
    kind: ErrorKind,
    /// Go's `url.Error` prefix, `<Method> "<url>"`, for the errors Go wraps in one.
    prefix: Option<String>,
    /// The words after the prefix.
    message: String,
    source: Option<reqwest::Error>,
}

/// Go's `context.DeadlineExceeded` text.
const DEADLINE_EXCEEDED: &str = "context deadline exceeded";

/// Go's refusal of `HTTP_PROXY` in a CGI environment (golang.org/x/net/http/httpproxy).
pub(crate) const CGI_REFUSAL: &str =
    "refusing to use HTTP_PROXY value in CGI environment; see golang.org/s/cgihttpproxy";

impl RequestError {
    /// What went wrong.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Whether Go's `errors.Is(err, context.DeadlineExceeded)` holds: the request ran past its
    /// deadline or its dial timed out. bidder.go reports such an error as `errortypes.Timeout`.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::DeadlineExceeded | ErrorKind::ConnectTimeout
        )
    }

    /// The request ran past its deadline.
    pub(crate) const fn deadline() -> Self {
        Self {
            kind: ErrorKind::DeadlineExceeded,
            prefix: None,
            message: String::new(),
            source: None,
        }
    }

    /// An error of the crate's own about the request `method url`.
    pub(crate) fn own(kind: ErrorKind, method: &Method, url: &str, message: String) -> Self {
        Self {
            kind,
            prefix: Some(prefix(method, url)),
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
            source: Some(error),
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
            source: Some(error),
        }
    }

    /// reqwest's error sending the request `method url`.
    pub(crate) fn reqwest(error: reqwest::Error, method: &Method, url: &str) -> Self {
        if let Some(refusal) = redirect_refusal(&error) {
            return Self {
                kind: refusal.kind,
                prefix: Some(prefix(method, &refusal.url)),
                message: refusal.message.clone(),
                source: Some(error),
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
            prefix: Some(prefix(method, url)),
            message: error_chain(&error),
            source: Some(error),
        }
    }
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
        let source: &(dyn Error + 'static) = self.source.as_ref()?;
        Some(source)
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Method;

    use super::{ErrorKind, RequestError, prefix};

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
        let own = RequestError::own(
            ErrorKind::Proxy,
            &Method::GET,
            "http://a.test/",
            "no".to_owned(),
        );
        assert!(!own.is_timeout(), "a proxy refusal is not a timeout");
        assert_eq!(own.to_string(), "Get \"http://a.test/\": no", "prefixed");
    }
}
