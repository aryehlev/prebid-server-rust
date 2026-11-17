//! Go `net/http/request.go` for client requests: `Request`, `NewRequestWithContext` and the
//! wire form `Request.write` produces.

use std::{fmt, sync::Arc};

use bytes::Bytes;
use pbs_gocompat::{
    http::{DEFAULT_USER_AGENT, Header, REQUEST_WRITE_EXCLUDE, trim_string},
    strconv::quote,
    url::Url,
};
use tokio::io::AsyncRead;

use crate::{
    context::Context,
    error::HttpError,
    httpguts::{
        has_token, punycode_host_port, remove_empty_port, remove_zone, string_contains_ctl_byte,
        valid_host_header, valid_method,
    },
    transfer::TransferWriter,
};

/// A streaming request body: any reader whose length is not known up front.
pub type BodyReader = Box<dyn AsyncRead + Send + Unpin>;

/// Go: `Request.Body`.
#[derive(Default)]
pub enum Body {
    /// Go: a nil `Body`.
    #[default]
    Nil,
    /// Go: `http.NoBody`.
    NoBody,
    /// Go: an in-memory body (`*bytes.Buffer`, `*bytes.Reader`, `*strings.Reader`): its length
    /// is known and it is written with one `Write` call.
    Bytes(Bytes),
    /// Go: any other `io.Reader`; each read becomes one chunk when the body is chunked.
    Reader(BodyReader),
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("Nil"),
            Self::NoBody => f.write_str("NoBody"),
            Self::Bytes(b) => f.debug_tuple("Bytes").field(&b.len()).finish(),
            Self::Reader(_) => f.write_str("Reader"),
        }
    }
}

/// Go: `Request.GetBody`: returns a fresh copy of the body for retries and redirects.
#[derive(Clone)]
pub struct GetBody(pub Arc<dyn Fn() -> Result<Body, HttpError> + Send + Sync>);

impl fmt::Debug for GetBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GetBody")
    }
}

impl GetBody {
    /// A `GetBody` that returns `body` (cheaply cloned) each time.
    #[must_use]
    pub fn bytes(body: Bytes) -> Self {
        Self(Arc::new(move || Ok(Body::Bytes(body.clone()))))
    }

    /// Calls the function.
    ///
    /// # Errors
    ///
    /// Whatever the function returns.
    pub fn call(&self) -> Result<Body, HttpError> {
        (self.0)()
    }
}

/// Go: `*http.Request` as a client sends it.
#[derive(Debug)]
pub struct Request {
    /// Go: `Method` (`""` means `GET`).
    pub method: String,
    /// Go: `URL`.
    pub url: Url,
    /// Go: `Header`.
    pub header: Header,
    /// Go: `Body`.
    pub body: Body,
    /// Go: `GetBody`.
    pub get_body: Option<GetBody>,
    /// Go: `ContentLength` (`0` with a non-nil body means unknown).
    pub content_length: i64,
    /// Go: `TransferEncoding`.
    pub transfer_encoding: Vec<String>,
    /// Go: `Close`.
    pub close: bool,
    /// Go: `Host` (overrides the URL's host on the wire).
    pub host: String,
    /// Go: `Trailer`.
    pub trailer: Option<Header>,
    /// Go: the request context (`Request.Context()` / `WithContext`).
    pub ctx: Context,
}

impl Request {
    /// Go: `http.NewRequestWithContext(ctx, method, url, body)`.
    ///
    /// An in-memory body sets `ContentLength` and `GetBody`, and an empty one becomes
    /// [`Body::NoBody`], as Go does for `*bytes.Buffer`, `*bytes.Reader` and `*strings.Reader`.
    ///
    /// # Errors
    ///
    /// `net/http: invalid method "BAD METHOD"`, or the `*url.Error` of `url.Parse`.
    pub fn new_with_context(
        ctx: Context,
        method: &str,
        url: &str,
        body: Body,
    ) -> Result<Self, HttpError> {
        let method = if method.is_empty() { "GET" } else { method };
        if !valid_method(method) {
            return Err(HttpError::Message(format!(
                "net/http: invalid method {}",
                quote(method)
            )));
        }
        let mut u = Url::parse(url).map_err(|e| HttpError::Message(e.to_string()))?;
        // Go: removeEmptyPort only drops a trailing colon.
        let kept = remove_empty_port(&u.host).len();
        u.host.truncate(kept);
        let mut req = Self {
            method: method.to_owned(),
            host: u.host.clone(),
            url: u,
            header: Header::new(),
            body: Body::Nil,
            get_body: None,
            content_length: 0,
            transfer_encoding: Vec::new(),
            close: false,
            trailer: None,
            ctx,
        };
        match body {
            Body::Bytes(b) => {
                req.content_length = i64::try_from(b.len()).unwrap_or(i64::MAX);
                if b.is_empty() {
                    req.body = Body::NoBody;
                    req.get_body = Some(GetBody(Arc::new(|| Ok(Body::NoBody))));
                } else {
                    req.get_body = Some(GetBody::bytes(b.clone()));
                    req.body = Body::Bytes(b);
                }
            }
            body @ (Body::Nil | Body::NoBody | Body::Reader(_)) => req.body = body,
        }
        Ok(req)
    }

    /// Go: `http.NewRequest(method, url, body)` (the background context).
    ///
    /// # Errors
    ///
    /// As [`Request::new_with_context`].
    pub fn new(method: &str, url: &str, body: Body) -> Result<Self, HttpError> {
        Self::new_with_context(Context::background(), method, url, body)
    }

    /// Go: `req.WithContext(ctx)` (the request itself, with the new context).
    #[must_use]
    pub fn with_context(mut self, ctx: Context) -> Self {
        self.ctx = ctx;
        self
    }

    /// Go: `outgoingLength`.
    #[must_use]
    pub const fn outgoing_length(&self) -> i64 {
        match self.body {
            Body::Nil | Body::NoBody => 0,
            Body::Bytes(_) | Body::Reader(_) => {
                if self.content_length != 0 {
                    self.content_length
                } else {
                    -1
                }
            }
        }
    }

    /// Go: `isReplayable`.
    #[must_use]
    pub fn is_replayable(&self) -> bool {
        if matches!(self.body, Body::Nil | Body::NoBody) || self.get_body.is_some() {
            let method = if self.method.is_empty() {
                "GET"
            } else {
                self.method.as_str()
            };
            if matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE") {
                return true;
            }
            if self.header.get_raw("Idempotency-Key").is_some()
                || self.header.get_raw("X-Idempotency-Key").is_some()
            {
                return true;
            }
        }
        false
    }

    /// Go: `expectsContinue`.
    #[must_use]
    pub fn expects_continue(&self) -> bool {
        has_token(
            self.header
                .get_raw("Expect")
                .and_then(|v| v.first())
                .map_or("", String::as_str),
            "100-continue",
        )
    }

    /// Go: `wantsClose`.
    #[must_use]
    pub fn wants_close(&self) -> bool {
        if self.close {
            return true;
        }
        has_token(
            self.header
                .get_raw("Connection")
                .and_then(|v| v.first())
                .map_or("", String::as_str),
            "close",
        )
    }

    /// Go: the `Request.write` steps before the body: the request line, `Host`, `User-Agent`,
    /// the framing headers, the header map and `extra` (the transport's headers), then the
    /// blank line.
    ///
    /// # Errors
    ///
    /// Go's `http: invalid Host header`, `net/http: can't write control character in
    /// Request.URL`, punycode and trailer-key errors.
    pub fn write_head(
        &self,
        using_proxy: bool,
        extra: &Header,
        tw: &TransferWriter,
        out: &mut Vec<u8>,
    ) -> Result<(), HttpError> {
        self.write_start(using_proxy, out)?;
        self.write_fields(extra, tw, out)
    }

    /// Go: the first `Request.write` steps, which run before the transfer writer is built: the
    /// request line, `Host` and `User-Agent`.
    ///
    /// # Errors
    ///
    /// As [`Request::write_head`], except the trailer-key errors.
    pub(crate) fn write_start(
        &self,
        using_proxy: bool,
        out: &mut Vec<u8>,
    ) -> Result<(), HttpError> {
        let host = if self.host.is_empty() {
            self.url.host.as_str()
        } else {
            self.host.as_str()
        };
        let mut host = punycode_host_port(host)?;
        if !valid_host_header(&host) {
            if using_proxy {
                return Err(HttpError::Static("http: invalid Host header"));
            }
            host.clear();
        }
        let host = remove_zone(&host);
        let mut ruri = self.url.request_uri();
        if using_proxy && !self.url.scheme.is_empty() && self.url.opaque.is_empty() {
            ruri = format!("{}://{host}{ruri}", self.url.scheme);
        } else if self.method == "CONNECT" && self.url.path.is_empty() {
            ruri.clone_from(&host);
            if !self.url.opaque.is_empty() {
                ruri.clone_from(&self.url.opaque);
            }
        }
        if string_contains_ctl_byte(&ruri) {
            return Err(HttpError::Static(
                "net/http: can't write control character in Request.URL",
            ));
        }
        let method = if self.method.is_empty() {
            "GET"
        } else {
            self.method.as_str()
        };
        out.extend_from_slice(method.as_bytes());
        out.push(b' ');
        out.extend_from_slice(ruri.as_bytes());
        out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
        out.extend_from_slice(host.as_bytes());
        out.extend_from_slice(b"\r\n");
        // Use the default User-Agent unless the header has one, which may be blank to send none.
        let user_agent = if self.header.get_raw("User-Agent").is_some() {
            self.header.get("User-Agent")
        } else {
            DEFAULT_USER_AGENT
        };
        if !user_agent.is_empty() {
            let ua = user_agent.replace(['\n', '\r'], " ");
            out.extend_from_slice(b"User-Agent: ");
            out.extend_from_slice(trim_string(&ua).as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        Ok(())
    }

    /// Go: the `Request.write` steps after [`Request::write_start`]: the framing headers, the
    /// header map and `extra`, then the blank line.
    ///
    /// # Errors
    ///
    /// Go's `invalid Trailer key` errors.
    pub(crate) fn write_fields(
        &self,
        extra: &Header,
        tw: &TransferWriter,
        out: &mut Vec<u8>,
    ) -> Result<(), HttpError> {
        tw.write_header(out)?;
        out.extend_from_slice(self.header.write_subset(&REQUEST_WRITE_EXCLUDE).as_bytes());
        out.extend_from_slice(extra.write().as_bytes());
        out.extend_from_slice(b"\r\n");
        Ok(())
    }
}
