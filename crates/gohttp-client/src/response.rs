//! Go `net/http.Response`: `ReadResponse`'s head parsing and the response body readers.

use std::{
    borrow::Cow,
    fmt,
    future::poll_fn,
    io,
    pin::Pin,
    str,
    task::{Context as TaskContext, Poll, ready},
    time::Instant,
};

use bytes::Bytes;
use pbs_gocompat::{
    http::Header,
    strconv::{atoi, quote_bytes},
    url::Url,
};
use tokio::io::{AsyncRead, ReadBuf};

use crate::{
    bufio::{BufReader, GoRead},
    client::parse_url_reference,
    context::CancelFunc,
    error::HttpError,
    httptrace::ConnectionState,
    request::Request,
    textproto::{LineReader, MimeHeaderReader},
    transport::ConnBody,
};

/// Go: `*http.Response`.
#[derive(Debug)]
pub struct Response {
    /// Go: `Status` (`"200 OK"`).
    pub status: String,
    /// Go: `StatusCode`.
    pub status_code: u16,
    /// Go: `Proto` (`"HTTP/1.1"`).
    pub proto: String,
    /// Go: `ProtoMajor`.
    pub proto_major: u8,
    /// Go: `ProtoMinor`.
    pub proto_minor: u8,
    /// Go: `Header`.
    pub header: Header,
    /// Go: `Body`.
    pub body: ResponseBody,
    /// Go: `ContentLength` (`-1`: unknown).
    pub content_length: i64,
    /// Go: `TransferEncoding`.
    pub transfer_encoding: Vec<String>,
    /// Go: `Close`.
    pub close: bool,
    /// Go: `Uncompressed`: the transport removed a gzip encoding.
    pub uncompressed: bool,
    /// Go: `Trailer` as announced before the body is read; the values arrive in
    /// [`ResponseBody::trailer`] once the body is read to the end.
    pub trailer: Option<Header>,
    /// Go: `Request`: the request that produced this response.
    pub request: Option<Box<Request>>,
    /// Go: `TLS`.
    pub tls: Option<ConnectionState>,
}

/// Go: `http.ErrNoLocation`.
pub const ERR_NO_LOCATION: HttpError = HttpError::Static("http: no Location header in response");

impl Response {
    /// Go: `Response.Location()`: the `Location` header resolved against the request URL.
    ///
    /// # Errors
    ///
    /// [`ERR_NO_LOCATION`], or the `*url.Error` of parsing the header.
    pub fn location(&self) -> Result<Url, HttpError> {
        let lv = self.header.get("Location");
        if lv.is_empty() {
            return Err(ERR_NO_LOCATION);
        }
        self.request
            .as_ref()
            .map_or_else(|| Url::parse(lv), |req| parse_url_reference(&req.url, lv))
            .map_err(|e| HttpError::Message(e.to_string()))
    }
}

/// The parsed status line and header of a response (Go: `ReadResponse` before
/// `readTransfer`).
#[derive(Debug)]
pub(crate) struct ResponseHead {
    pub(crate) status: String,
    pub(crate) status_code: u16,
    pub(crate) proto: String,
    pub(crate) proto_major: u8,
    pub(crate) proto_minor: u8,
    pub(crate) header: Header,
}

/// Go: `badStringError(what, val)`.
fn bad_string_error(what: &str, val: &[u8]) -> HttpError {
    HttpError::Message(format!("{what} {}", quote_bytes(val)))
}

/// Go: `http.ParseHTTPVersion`.
#[must_use]
pub fn parse_http_version(vers: &[u8]) -> Option<(u8, u8)> {
    match vers {
        b"HTTP/1.1" => return Some((1, 1)),
        b"HTTP/1.0" => return Some((1, 0)),
        _ => {}
    }
    let rest = vers.strip_prefix(b"HTTP/")?;
    let [major, b'.', minor] = rest else {
        return None;
    };
    if !major.is_ascii_digit() || !minor.is_ascii_digit() {
        return None;
    }
    Some((major - b'0', minor - b'0'))
}

/// Splits `s` at the first space (Go: `strings.Cut(s, " ")`).
fn cut_space(s: &[u8]) -> (&[u8], Option<&[u8]>) {
    s.iter().position(|&b| b == b' ').map_or((s, None), |i| {
        let (a, b) = s.split_at(i);
        (a, b.get(1..))
    })
}

/// Go: `ReadResponse` up to the header, restartable across `Pending`.
#[derive(Debug, Default)]
pub(crate) struct ResponseHeadReader {
    line: LineReader,
    status: Option<(String, u16, String, u8, u8)>,
    mime: MimeHeaderReader,
}

impl ResponseHeadReader {
    /// Go: the status line and `ReadMIMEHeader` of `ReadResponse`.
    pub(crate) fn poll_read<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        br: &mut BufReader,
        src: &mut R,
    ) -> Poll<Result<ResponseHead, HttpError>> {
        if self.status.is_none() {
            let line = match ready!(self.line.poll_read(cx, br, src, None)) {
                Ok(line) => line,
                Err(HttpError::Eof) => return Poll::Ready(Err(HttpError::UnexpectedEof)),
                Err(e) => return Poll::Ready(Err(e)),
            };
            let (proto, Some(status)) = cut_space(&line) else {
                return Poll::Ready(Err(bad_string_error("malformed HTTP response", &line)));
            };
            let start = status
                .iter()
                .position(|&b| b != b' ')
                .unwrap_or(status.len());
            let status = status.get(start..).unwrap_or_default();
            let (code, _) = cut_space(status);
            if code.len() != 3 {
                return Poll::Ready(Err(bad_string_error("malformed HTTP status code", code)));
            }
            let parsed = str::from_utf8(code)
                .ok()
                .and_then(|c| atoi(c).ok())
                .and_then(|c| u16::try_from(c).ok());
            let Some(status_code) = parsed else {
                return Poll::Ready(Err(bad_string_error("malformed HTTP status code", code)));
            };
            let Some((major, minor)) = parse_http_version(proto) else {
                return Poll::Ready(Err(bad_string_error("malformed HTTP version", proto)));
            };
            self.status = Some((
                String::from_utf8_lossy(status).into_owned(),
                status_code,
                String::from_utf8_lossy(proto).into_owned(),
                major,
                minor,
            ));
        }
        let mime = match ready!(self.mime.poll_read(cx, br, src)) {
            Ok(m) => m,
            Err(HttpError::Eof) => return Poll::Ready(Err(HttpError::UnexpectedEof)),
            Err(e) => return Poll::Ready(Err(e)),
        };
        let Some((status, status_code, proto, proto_major, proto_minor)) = self.status.take()
        else {
            return Poll::Ready(Err(HttpError::UnexpectedEof));
        };
        let mut header = Header::new();
        for (k, v) in mime {
            header.insert_raw(k, v);
        }
        Poll::Ready(Ok(ResponseHead {
            status,
            status_code,
            proto,
            proto_major,
            proto_minor,
            header,
        }))
    }
}

/// Go: `cancelTimerBody`: the `Client.Timeout` wrapper around a response body.
#[derive(Debug)]
pub(crate) struct ClientTimer {
    /// Go: `reqDidTimeout`: true once this instant has passed (`None`: never).
    pub(crate) deadline: Option<Instant>,
    /// Go: `stop`: cancels the client deadline's context.
    pub(crate) stop: Option<CancelFunc>,
}

enum BodyKind {
    /// Go: `http.NoBody`.
    Empty,
    /// An in-memory body.
    Bytes(Box<Bytes>),
    /// A body read from a transport connection.
    Conn(Box<ConnBody>),
}

/// Go: `Response.Body`.
///
/// [`ResponseBody::read_go`] has Go's `Read` semantics (bytes together with an error, `EOF` at
/// the end); the [`AsyncRead`] implementation reports the same data with `Ok(())` and no bytes
/// at the end. Dropping a body without reading it to the end closes its connection, as
/// [`ResponseBody::close`] does.
pub struct ResponseBody {
    kind: BodyKind,
    timer: Option<ClientTimer>,
    /// An error that arrived with data, held for the next [`AsyncRead`] poll.
    pending_err: Option<HttpError>,
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.kind {
            BodyKind::Empty => "NoBody",
            BodyKind::Bytes(_) => "Bytes",
            BodyKind::Conn(_) => "Conn",
        };
        f.debug_struct("ResponseBody")
            .field("kind", &kind)
            .field("timer", &self.timer)
            .finish_non_exhaustive()
    }
}

impl Default for ResponseBody {
    fn default() -> Self {
        Self::empty()
    }
}

impl ResponseBody {
    /// Go: `http.NoBody`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            kind: BodyKind::Empty,
            timer: None,
            pending_err: None,
        }
    }

    /// An in-memory body (Go: `io.NopCloser(bytes.NewReader(b))`).
    #[must_use]
    pub fn from_bytes(b: Bytes) -> Self {
        Self {
            kind: BodyKind::Bytes(Box::new(b)),
            timer: None,
            pending_err: None,
        }
    }

    pub(crate) fn from_conn(c: ConnBody) -> Self {
        Self {
            kind: BodyKind::Conn(Box::new(c)),
            timer: None,
            pending_err: None,
        }
    }

    pub(crate) fn set_timer(&mut self, timer: ClientTimer) {
        self.timer = Some(timer);
    }

    /// The trailer received after a chunked body (Go: `Response.Trailer` after `EOF`).
    #[must_use]
    pub fn trailer(&self) -> Option<&Header> {
        match &self.kind {
            BodyKind::Conn(c) => c.trailer(),
            BodyKind::Empty | BodyKind::Bytes(_) => None,
        }
    }

    fn poll_inner(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        match &mut self.kind {
            BodyKind::Empty => Poll::Ready((0, Some(HttpError::Eof))),
            BodyKind::Bytes(b) => {
                if b.is_empty() {
                    return Poll::Ready((0, Some(HttpError::Eof)));
                }
                if buf.is_empty() {
                    return Poll::Ready((0, None));
                }
                let n = b.len().min(buf.len());
                let chunk = b.split_to(n);
                if let Some(dst) = buf.get_mut(..n) {
                    dst.copy_from_slice(&chunk);
                }
                Poll::Ready((n, None))
            }
            BodyKind::Conn(c) => c.poll_read(cx, buf),
        }
    }

    /// Go: `Body.Read(p)`: the bytes read and the error (`EOF` at the end).
    pub fn poll_read_go(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        let (n, err) = ready!(self.poll_inner(cx, buf));
        let err = match err {
            Some(e)
                if e != HttpError::Eof
                    && self
                        .timer
                        .as_ref()
                        .and_then(|t| t.deadline)
                        .is_some_and(|d| Instant::now() > d) =>
            {
                Some(HttpError::Timeout(Cow::Owned(format!(
                    "{e} (Client.Timeout or context cancellation while reading body)"
                ))))
            }
            other => other,
        };
        Poll::Ready((n, err))
    }

    /// Go: `Body.Read(p)`.
    pub async fn read_go(&mut self, buf: &mut [u8]) -> (usize, Option<HttpError>) {
        poll_fn(|cx| self.poll_read_go(cx, buf)).await
    }

    /// Go: `io.ReadAll(Body)`: the bytes read and the error other than `EOF`, if any.
    pub async fn read_all_go(&mut self) -> (Vec<u8>, Option<HttpError>) {
        let mut b: Vec<u8> = Vec::with_capacity(512);
        loop {
            if b.len() == b.capacity() {
                b.reserve(b.capacity().max(512));
            }
            let len = b.len();
            b.resize(b.capacity(), 0);
            let (n, err) = {
                let spare = b.get_mut(len..).unwrap_or_default();
                poll_fn(|cx| self.poll_read_go(cx, spare)).await
            };
            b.truncate(len + n);
            match err {
                None => {}
                Some(HttpError::Eof) => return (b, None),
                Some(e) => return (b, Some(e)),
            }
        }
    }

    /// Go: `io.ReadAll(Body)`, dropping the partial data on error.
    ///
    /// # Errors
    ///
    /// The first read error other than `EOF`.
    pub async fn read_all(&mut self) -> Result<Vec<u8>, HttpError> {
        match self.read_all_go().await {
            (b, None) => Ok(b),
            (_, Some(e)) => Err(e),
        }
    }

    /// Go: `Body.Close()`: before the end of the body the connection is closed, not drained.
    pub fn close(&mut self) {
        if let BodyKind::Conn(c) = &mut self.kind {
            c.close();
        }
        if let Some(stop) = self.timer.as_mut().and_then(|t| t.stop.take()) {
            stop.cancel();
        }
    }
}

impl AsyncRead for ResponseBody {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(e) = this.pending_err.take() {
            return Poll::Ready(Err(io::Error::other(e)));
        }
        let dst = buf.initialize_unfilled();
        if dst.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let (n, err) = ready!(this.poll_read_go(cx, dst));
        buf.advance(n);
        match err {
            None if n == 0 => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            None | Some(HttpError::Eof) => Poll::Ready(Ok(())),
            Some(e) if n > 0 => {
                this.pending_err = Some(e);
                Poll::Ready(Ok(()))
            }
            Some(e) => Poll::Ready(Err(io::Error::other(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{SliceSource, ensure_eq, poll_ready};

    fn head(raw: &[u8]) -> Result<Result<ResponseHead, String>, String> {
        let mut br = BufReader::new();
        let mut src = SliceSource::new(raw, 7);
        let mut r = ResponseHeadReader::default();
        Ok(poll_ready(|cx| r.poll_read(cx, &mut br, &mut src))?.map_err(|e| e.to_string()))
    }

    #[test]
    fn status_line_errors() -> Result<(), String> {
        let ok = head(b"HTTP/1.1 200 OK\r\nA: b\r\nA: c\r\n\r\n")?;
        let h = ok?;
        ensure_eq(&h.status, &"200 OK".to_owned(), "status")?;
        ensure_eq(&h.status_code, &200, "code")?;
        ensure_eq(
            &h.header.values("A").to_vec(),
            &vec!["b".to_owned(), "c".to_owned()],
            "a",
        )?;
        let cases: [(&[u8], &str); 6] = [
            (b"HTTP/1.1\r\n\r\n", r#"malformed HTTP response "HTTP/1.1""#),
            (
                b"HTTP/1.1 20 OK\r\n\r\n",
                r#"malformed HTTP status code "20""#,
            ),
            (
                b"HTTP/1.1 abc OK\r\n\r\n",
                r#"malformed HTTP status code "abc""#,
            ),
            (
                b"HTTP/1.1 -12 OK\r\n\r\n",
                r#"malformed HTTP status code "-12""#,
            ),
            (
                b"HTTX/1.1 200 OK\r\n\r\n",
                r#"malformed HTTP version "HTTX/1.1""#,
            ),
            (b"HTTP/1.1 200 OK\r\nX: y", "unexpected EOF"),
        ];
        for (raw, want) in cases {
            ensure_eq(&head(raw)?.err(), &Some(want.to_owned()), want)?;
        }
        ensure_eq(
            &head(b"")?.err(),
            &Some("unexpected EOF".to_owned()),
            "empty",
        )?;
        ensure_eq(&parse_http_version(b"HTTP/2.0"), &Some((2, 0)), "2.0")?;
        ensure_eq(&parse_http_version(b"HTTP/1.10"), &None, "two digits")
    }
}
