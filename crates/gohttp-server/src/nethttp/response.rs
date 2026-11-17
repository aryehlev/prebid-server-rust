//! Go: the [`ResponseWriter`] and [`Handler`] interfaces, and the server's `*response` with
//! its `chunkWriter` (`server.go`): header snapshots, the 2048-byte buffer that decides between
//! `Content-Length` and chunking, `Content-Type` sniffing, `Date`, `Connection` handling and
//! trailers.

use core::{
    fmt, mem,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{io, sync::Arc};

use pbs_gocompat::{
    http::{Header, canonical_header_key},
    strconv::{parse_int, quote},
    time::Time,
};

use super::{
    BoxFuture,
    body::{BodyLimitNotifier, MAX_POST_HANDLER_READ_BYTES, ServerBody},
    bufio::ReadErr,
    lex::{
        has_token, header_elements, header_values_contains_token, push_lower_hex,
        valid_trailer_header,
    },
    request::{Request, first_raw},
    server::ConnShared,
    sniff::detect_content_type,
    status::{STATUS_NO_CONTENT, body_allowed_for_status, status_text},
};

/// Go: `http.TimeFormat`.
pub const TIME_FORMAT: &str = "Mon, 02 Jan 2006 15:04:05 GMT";

/// Go: `http.TrailerPrefix`.
pub const TRAILER_PREFIX: &str = "Trailer:";

/// Go: `http.ErrBodyNotAllowed`.
pub const ERR_BODY_NOT_ALLOWED: &str =
    "http: request method or response status code does not allow body";

/// Go: `http.ErrContentLength`.
pub const ERR_CONTENT_LENGTH: &str = "http: wrote more than the declared Content-Length";

/// Go: `bufferBeforeChunkingSize`.
const BUFFER_BEFORE_CHUNKING_SIZE: usize = 2048;

/// Go: `http.ResponseWriter` together with `http.Flusher`.
///
/// [`ResponseWriter::write_header`] does no I/O (Go's writes the header to the connection's
/// buffer only once the body is flushed), so it is synchronous; writing and flushing are not.
/// A middleware such as a gzip handler wraps a `&mut dyn ResponseWriter` and forwards to it.
pub trait ResponseWriter: Send {
    /// Go: `Header()`: the header map the response will send. Changing it after
    /// [`ResponseWriter::write_header`] has no effect, except for trailers.
    fn header(&mut self) -> &mut Header;

    /// Go: `WriteHeader(code)`.
    fn write_header(&mut self, code: u16);

    /// Go: `Write(data)`.
    fn write<'a>(&'a mut self, data: &'a [u8]) -> BoxFuture<'a, io::Result<usize>>;

    /// Go: `Flush()` (`FlushError()`).
    fn flush(&mut self) -> BoxFuture<'_, io::Result<()>>;

    /// The hook `http.MaxBytesReader` finds on the server's own response writer (Go's type
    /// assertion for `requestTooLarge`). Wrappers return `None`, as Go's wrappers do not have the
    /// method.
    fn body_limit_notifier(&self) -> Option<BodyLimitNotifier> {
        None
    }
}

/// Go: `http.Handler`.
pub trait Handler: Send + Sync {
    /// Go: `ServeHTTP(w, r)`.
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()>;
}

impl<H: Handler + ?Sized> Handler for Arc<H> {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        (**self).serve_http(w, r)
    }
}

impl<H: Handler + ?Sized> Handler for Box<H> {
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        (**self).serve_http(w, r)
    }
}

/// Go: `http.HandlerFunc`: a function used as a [`Handler`].
///
/// ```
/// use pbs_gohttp_server::{BoxFuture, HandlerFunc, Request, ResponseWriter};
///
/// fn hello<'a>(w: &'a mut dyn ResponseWriter, _r: &'a mut Request) -> BoxFuture<'a, ()> {
///     Box::pin(async move {
///         let _written = w.write(b"hello").await;
///     })
/// }
/// let _handler = HandlerFunc(hello);
/// ```
#[derive(Clone, Copy)]
pub struct HandlerFunc<F>(pub F);

impl<F> fmt::Debug for HandlerFunc<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HandlerFunc")
    }
}

impl<F> Handler for HandlerFunc<F>
where
    F: for<'a> Fn(&'a mut dyn ResponseWriter, &'a mut Request) -> BoxFuture<'a, ()> + Send + Sync,
{
    fn serve_http<'a>(
        &'a self,
        w: &'a mut dyn ResponseWriter,
        r: &'a mut Request,
    ) -> BoxFuture<'a, ()> {
        (self.0)(w, r)
    }
}

/// The request's protocol version as the response cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ProtoLevel {
    /// Below HTTP/1.0.
    Below10,
    /// HTTP/1.0.
    Http10,
    /// HTTP/1.1 or later.
    Http11,
}

/// What the request's `Connection` header asked for.
#[derive(Debug, Clone, Copy)]
pub(super) struct ConnHints {
    /// Go: `req.wantsHttp10KeepAlive()`.
    pub(super) wants10_keep_alive: bool,
    /// Go: `req.wantsClose()`.
    pub(super) wants_close: bool,
}

/// The request facts the response consults.
#[derive(Debug)]
pub(super) struct ReqFacts {
    pub(super) is_head: bool,
    pub(super) proto: ProtoLevel,
    pub(super) content_length: i64,
    pub(super) conn: ConnHints,
    /// Whether `req.Body` is an `expectContinueReader`.
    pub(super) ecr: bool,
}

impl ReqFacts {
    pub(super) fn new(req: &Request, ecr: bool) -> Self {
        let proto = if req.proto_at_least(1, 1) {
            ProtoLevel::Http11
        } else if req.proto_at_least(1, 0) {
            ProtoLevel::Http10
        } else {
            ProtoLevel::Below10
        };
        Self {
            is_head: req.method == "HEAD",
            proto,
            content_length: req.content_length,
            conn: ConnHints {
                wants10_keep_alive: req.wants_http10_keep_alive(),
                wants_close: req.wants_close(),
            },
            ecr,
        }
    }

    /// Go: `req.ProtoAtLeast(1, 1)`.
    pub(super) fn at_least_11(&self) -> bool {
        self.proto >= ProtoLevel::Http11
    }

    /// Go: `req.ProtoAtLeast(1, 0)`.
    pub(super) fn at_least_10(&self) -> bool {
        self.proto >= ProtoLevel::Http10
    }
}

/// Go: `extraHeader`.
#[derive(Debug, Default)]
struct ExtraHeader {
    content_type: &'static str,
    connection: &'static str,
    transfer_encoding: &'static str,
    date: Option<String>,
    content_length: Option<String>,
}

impl ExtraHeader {
    fn write(&self, out: &mut Vec<u8>) {
        if let Some(date) = &self.date {
            out.extend_from_slice(b"Date: ");
            out.extend_from_slice(date.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        if let Some(cl) = &self.content_length {
            out.extend_from_slice(b"Content-Length: ");
            out.extend_from_slice(cl.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        for (key, v) in [
            ("Content-Type", self.content_type),
            ("Connection", self.connection),
            ("Transfer-Encoding", self.transfer_encoding),
        ] {
            if !v.is_empty() {
                out.extend_from_slice(key.as_bytes());
                out.extend_from_slice(b": ");
                out.extend_from_slice(v.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
    }
}

/// Go: `writeStatusLine`.
pub(super) fn write_status_line(out: &mut Vec<u8>, is11: bool, code: u16) {
    out.extend_from_slice(if is11 { b"HTTP/1.1 " } else { b"HTTP/1.0 " });
    let text = status_text(code);
    if text.is_empty() {
        out.extend_from_slice(format!("{code:03} status code {code}\r\n").as_bytes());
    } else {
        out.extend_from_slice(code.to_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
}

/// Go: `suppressedHeaders`.
const fn suppressed_headers(status: u16) -> &'static [&'static str] {
    if status == 304 {
        &["Content-Type", "Content-Length", "Transfer-Encoding"]
    } else if !body_allowed_for_status(status) {
        &["Content-Length", "Transfer-Encoding"]
    } else {
        &[]
    }
}

/// A sticky write error (Go keeps the `error` value).
#[derive(Debug, Clone)]
struct StickyErr(String);

impl StickyErr {
    fn to_io(&self) -> io::Error {
        io::Error::other(self.0.clone())
    }
}

/// Go: the `response` fields deciding how the exchange ends.
#[derive(Debug, Default)]
pub(super) struct ReplyState {
    /// Go: `closeAfterReply`.
    pub(super) close_after_reply: bool,
    /// Go: `requestBodyLimitHit`.
    pub(super) request_body_limit_hit: bool,
    /// Go: `handlerDone`.
    handler_done: bool,
}

/// Go: `chunkWriter`.
#[derive(Debug, Default)]
struct ChunkWriterState {
    /// Go: `cw.header`.
    header: Option<Header>,
    /// Go: `cw.wroteHeader`.
    wrote_header: bool,
    /// Go: `cw.chunking`.
    chunking: bool,
}

/// Go: the server's `*response` (with its `chunkWriter` and buffered writer).
pub(super) struct ServerResponse {
    conn: Arc<ConnShared>,
    facts: ReqFacts,
    /// Go: `w.reqBody` (`None` for `http.NoBody`).
    req_body: Option<Arc<ServerBody>>,
    /// Set when the handler replaced `req.Body` (Go's type switches then see another type).
    body_swapped: Arc<AtomicBool>,
    handler_header: Header,
    called_header: bool,
    wrote_header: bool,
    status: u16,
    written: i64,
    pub(super) content_length: i64,
    pub(super) reply: ReplyState,
    trailers: Vec<String>,
    cw: ChunkWriterState,
    // w.w: bufio.Writer(2048) over the chunkWriter
    buf: Vec<u8>,
    buf_err: Option<StickyErr>,
    can_write_continue: Arc<AtomicBool>,
    limit: BodyLimitNotifier,
    /// 1xx responses written by `WriteHeader`, sent at the next write, flush or finish.
    informational: Vec<u8>,
    /// Set when the handler did what makes Go's handler panic (an invalid status code).
    pub(super) panic: Option<String>,
}

impl fmt::Debug for ServerResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerResponse")
            .field("status", &self.status)
            .field("wrote_header", &self.wrote_header)
            .finish_non_exhaustive()
    }
}

impl ServerResponse {
    pub(super) fn new(
        conn: Arc<ConnShared>,
        facts: ReqFacts,
        req_body: Option<Arc<ServerBody>>,
        body_swapped: Arc<AtomicBool>,
        can_write_continue: Arc<AtomicBool>,
    ) -> Self {
        Self {
            conn,
            facts,
            req_body,
            body_swapped,
            handler_header: Header::new(),
            called_header: false,
            wrote_header: false,
            status: 0,
            written: 0,
            content_length: -1,
            reply: ReplyState::default(),
            trailers: Vec::new(),
            cw: ChunkWriterState::default(),
            buf: Vec::with_capacity(BUFFER_BEFORE_CHUNKING_SIZE),
            buf_err: None,
            can_write_continue,
            limit: BodyLimitNotifier::default(),
            informational: Vec::new(),
            panic: None,
        }
    }

    fn swapped(&self) -> bool {
        self.body_swapped.load(Ordering::SeqCst)
    }

    /// Go: `requestTooLarge`, applied when `MaxBytesReader` hit its limit.
    fn sync_limit(&mut self) {
        if self.limit.hit.swap(false, Ordering::SeqCst) {
            self.request_too_large();
        }
    }

    /// Go: `requestTooLarge`.
    fn request_too_large(&mut self) {
        self.reply.close_after_reply = true;
        self.reply.request_body_limit_hit = true;
        if !self.wrote_header {
            self.header_inner().set("Connection", "close");
        }
    }

    /// Go: `disableWriteContinue`.
    pub(super) fn disable_write_continue(&self) {
        self.can_write_continue.store(false, Ordering::SeqCst);
    }

    /// Go: `Header()`.
    fn header_inner(&mut self) -> &mut Header {
        if self.cw.header.is_none() && self.wrote_header && !self.cw.wrote_header {
            self.cw.header = Some(self.handler_header.clone());
        }
        self.called_header = true;
        &mut self.handler_header
    }

    /// Go: `WriteHeader(code)`.
    fn write_header_inner(&mut self, code: u16) {
        if self.panic.is_some() {
            return;
        }
        if self.wrote_header {
            self.conn
                .logf("http: superfluous response.WriteHeader call from unknown (unknown:0)");
            return;
        }
        if !(100..=999).contains(&code) {
            self.panic = Some(format!("invalid WriteHeader code {code}"));
            return;
        }
        if !(101..=199).contains(&code) {
            self.disable_write_continue();
        }
        if (100..=199).contains(&code) && code != 101 {
            if code == 100 {
                self.can_write_continue.store(false, Ordering::SeqCst);
            }
            write_status_line(&mut self.informational, self.facts.at_least_11(), code);
            self.informational.extend_from_slice(
                self.handler_header
                    .write_subset(&["Content-Length", "Transfer-Encoding"])
                    .as_bytes(),
            );
            self.informational.extend_from_slice(b"\r\n");
            return;
        }
        self.wrote_header = true;
        self.status = code;
        if self.called_header && self.cw.header.is_none() {
            self.cw.header = Some(self.handler_header.clone());
        }
        let cl = first_raw(&self.handler_header, "Content-Length").to_owned();
        if !cl.is_empty() {
            match parse_int(&cl, 10, 64) {
                Ok(v) if v >= 0 => self.content_length = v,
                Ok(_) | Err(_) => {
                    self.conn
                        .logf(&format!("http: invalid Content-Length of {}", quote(&cl)));
                    self.handler_header.del("Content-Length");
                }
            }
        }
    }

    /// Sends queued 1xx responses (Go writes and flushes them in `WriteHeader`).
    async fn send_informational(&mut self) {
        if self.informational.is_empty() {
            return;
        }
        let bytes = mem::take(&mut self.informational);
        let mut w = self.conn.writer.lock().await;
        let _written = w.write(&bytes).await;
        let _flushed = w.flush().await;
        drop(w);
    }

    /// Go: `declareTrailer`.
    fn declare_trailer(&mut self, k: &str) {
        let k = canonical_header_key(k);
        if !valid_trailer_header(&k) {
            return;
        }
        self.trailers.push(k);
    }

    /// Go: `finalTrailers`.
    fn final_trailers(&self) -> Option<Header> {
        let mut t: Option<Header> = None;
        for (k, vv) in self.handler_header.iter() {
            if let Some(kk) = k.strip_prefix(TRAILER_PREFIX) {
                t.get_or_insert_with(Header::new)
                    .insert_raw(kk.to_owned(), vv.to_vec());
            }
        }
        for k in &self.trailers {
            let t = t.get_or_insert_with(Header::new);
            for v in self.handler_header.get_raw(k).unwrap_or_default() {
                t.add(k, v.clone());
            }
        }
        t
    }

    /// The header the chunk writer reads (Go: `cw.header`, or `w.handlerHeader` when not
    /// owned; the latter is then empty).
    fn view(&self) -> &Header {
        self.cw.header.as_ref().unwrap_or(&self.handler_header)
    }

    /// Go: `delHeader` inside `writeHeader` (an unowned header is empty, so only an owned one
    /// is changed).
    fn del_header(&mut self, key: &str) {
        if let Some(h) = self.cw.header.as_mut() {
            h.del(key);
        }
    }

    /// Go: the trailer scan of `chunkWriter.writeHeader`: the `Trailer:`-prefixed keys (left
    /// out of the header block) and whether the response has trailers.
    fn cw_scan_trailers(&mut self) -> (Vec<String>, bool) {
        let mut exclude: Vec<String> = Vec::new();
        let mut trailers = false;
        let mut declared: Vec<String> = Vec::new();
        if let Some(h) = &self.cw.header {
            for (k, _) in h.iter() {
                if k.starts_with(TRAILER_PREFIX) {
                    exclude.push(k.to_owned());
                    trailers = true;
                }
            }
            for v in h.get_raw("Trailer").unwrap_or_default() {
                trailers = true;
                declared.extend(header_elements(v).map(str::to_owned));
            }
        }
        for k in &declared {
            self.declare_trailer(k);
        }
        (exclude, trailers)
    }

    /// Go: `chunkWriter.writeHeader(p)`.
    async fn cw_write_header(&mut self, p: &[u8]) {
        if self.cw.wrote_header {
            return;
        }
        self.cw.wrote_header = true;
        let keep_alives = self.conn.keep_alives_enabled();
        let is_head = self.facts.is_head;
        let mut set = ExtraHeader::default();
        let (exclude, trailers) = self.cw_scan_trailers();

        let te = first_raw(self.view(), "Transfer-Encoding").to_owned();
        let has_te = !te.is_empty();

        if self.reply.handler_done
            && !trailers
            && !has_te
            && body_allowed_for_status(self.status)
            && self.view().get_raw("Content-Length").is_none()
            && (!is_head || !p.is_empty())
        {
            self.content_length = i64::try_from(p.len()).unwrap_or(i64::MAX);
            set.content_length = Some(p.len().to_string());
        }

        if self.facts.conn.wants10_keep_alive && keep_alives {
            let sent_length = !first_raw(self.view(), "Content-Length").is_empty();
            if sent_length && first_raw(self.view(), "Connection") == "keep-alive" {
                self.reply.close_after_reply = false;
            }
        }

        let has_cl = self.content_length != -1;
        self.cw_decide_close(&mut set, is_head, has_cl, keep_alives);
        if self.facts.content_length != 0 && !self.reply.close_after_reply {
            self.discard_unread_body(&mut set).await;
        }

        let code = self.status;
        if body_allowed_for_status(code) {
            let have_type = self.view().get_raw("Content-Type").is_some();
            let content_encoded = !self.view().get("Content-Encoding").is_empty();
            if !content_encoded && !have_type && !has_te && !p.is_empty() {
                set.content_type = detect_content_type(p);
            }
        } else {
            for k in suppressed_headers(code) {
                self.del_header(k);
            }
        }

        if self.view().get_raw("Date").is_none() {
            set.date = Some(Time::now().utc().format(TIME_FORMAT));
        }

        self.cw_framing(&mut set, &te, has_cl, is_head);
        if !self.facts.at_least_10() {
            return;
        }

        let cw_connection = self
            .cw
            .header
            .as_ref()
            .map_or("", |h| first_raw(h, "Connection"));
        let del_connection_header = self.reply.close_after_reply
            && (!keep_alives || !has_token(cw_connection, "close"))
            && !is_protocol_switch_response(self.status, self.view());
        if del_connection_header {
            self.del_header("Connection");
            if self.facts.at_least_11() {
                set.connection = "close";
            }
        }

        let mut out = Vec::with_capacity(256);
        write_status_line(&mut out, self.facts.at_least_11(), code);
        if let Some(h) = &self.cw.header {
            let exclude: Vec<&str> = exclude.iter().map(String::as_str).collect();
            out.extend_from_slice(h.write_subset(&exclude).as_bytes());
        }
        set.write(&mut out);
        out.extend_from_slice(b"\r\n");
        let _written = self.conn.writer.lock().await.write(&out).await;
    }

    /// Go: the keep-alive decisions of `chunkWriter.writeHeader` before the body discard.
    fn cw_decide_close(
        &mut self,
        set: &mut ExtraHeader,
        is_head: bool,
        has_cl: bool,
        keep_alives: bool,
    ) {
        if self.facts.conn.wants10_keep_alive
            && (is_head || has_cl || !body_allowed_for_status(self.status))
        {
            if self.view().get_raw("Connection").is_none() {
                set.connection = "keep-alive";
            }
        } else if !self.facts.at_least_11() || self.facts.conn.wants_close {
            self.reply.close_after_reply = true;
        }

        if first_raw(self.view(), "Connection") == "close" || !keep_alives {
            self.reply.close_after_reply = true;
        }

        let swapped = self.swapped();
        if self.facts.ecr
            && !swapped
            && let Some(body) = &self.req_body
            && body
                .expect
                .as_ref()
                .is_some_and(|e| !e.saw_eof.load(Ordering::SeqCst))
        {
            self.reply.close_after_reply = true;
        }
    }

    /// Go: the `Content-Length` / `Transfer-Encoding` decisions of `chunkWriter.writeHeader`.
    fn cw_framing(&mut self, set: &mut ExtraHeader, te: &str, has_cl: bool, is_head: bool) {
        let has_te = !te.is_empty();
        let mut has_cl = has_cl;
        let code = self.status;
        if has_cl && has_te && te != "identity" {
            self.conn.logf(&format!(
                "http: WriteHeader called with both Transfer-Encoding of {} and a \
                 Content-Length of {}",
                quote(te),
                self.content_length
            ));
            self.del_header("Content-Length");
            has_cl = false;
        }

        if is_head || !body_allowed_for_status(code) || code == STATUS_NO_CONTENT || has_cl {
            self.del_header("Transfer-Encoding");
        } else if self.facts.at_least_11() {
            if has_te && te == "identity" {
                self.cw.chunking = false;
                self.reply.close_after_reply = true;
                self.del_header("Transfer-Encoding");
            } else {
                self.cw.chunking = true;
                set.transfer_encoding = "chunked";
                if has_te && te == "chunked" {
                    self.del_header("Transfer-Encoding");
                }
            }
        } else {
            self.reply.close_after_reply = true;
            self.del_header("Transfer-Encoding");
        }

        if self.cw.chunking {
            self.del_header("Content-Length");
        }
    }

    /// The part of `writeHeader` that reads what the handler left of the request body.
    async fn discard_unread_body(&mut self, set: &mut ExtraHeader) {
        let Some(body) = self.req_body.clone() else {
            return;
        };
        let mut discard = false;
        let mut too_big = false;
        if self.swapped() {
            discard = true;
        } else if !self.facts.ecr {
            let (closed, saw_eof, unread) = body.write_header_view().await;
            if closed {
                if !saw_eof {
                    self.reply.close_after_reply = true;
                }
            } else if unread >= MAX_POST_HANDLER_READ_BYTES {
                too_big = true;
            } else {
                discard = true;
            }
        }
        if discard {
            match body.copy_n_discard(MAX_POST_HANDLER_READ_BYTES + 1).await {
                Ok(_) => too_big = true,
                Err(ReadErr::Protocol(t)) if t == super::body::ERR_BODY_READ_AFTER_CLOSE => {}
                Err(e) if e.is_eof() => {
                    if body.close().await.is_some() {
                        self.reply.close_after_reply = true;
                    }
                }
                Err(_) => self.reply.close_after_reply = true,
            }
        }
        if too_big {
            self.request_too_large();
            self.del_header("Connection");
            set.connection = "close";
        }
    }

    /// Go: `chunkWriter.Write(p)`.
    async fn cw_write(&mut self, p: &[u8]) -> (usize, Option<StickyErr>) {
        if !self.cw.wrote_header {
            self.cw_write_header(p).await;
        }
        if self.facts.is_head {
            return (p.len(), None);
        }
        let conn = Arc::clone(&self.conn);
        let mut w = conn.writer.lock().await;
        if self.cw.chunking {
            let mut line = String::with_capacity(10);
            push_lower_hex(&mut line, u64::try_from(p.len()).unwrap_or(u64::MAX));
            line.push_str("\r\n");
            if let Err(e) = w.write(line.as_bytes()).await {
                conn.close_rwc(&mut w);
                return (0, Some(StickyErr(e)));
            }
        }
        let (n, mut err) = match w.write(p).await {
            Ok(n) => (n, None),
            Err(e) => (0, Some(StickyErr(e))),
        };
        if self.cw.chunking
            && err.is_none()
            && let Err(e) = w.write(b"\r\n").await
        {
            err = Some(StickyErr(e));
        }
        if err.is_some() {
            conn.close_rwc(&mut w);
        }
        drop(w);
        (n, err)
    }

    /// Go: `bufio.Writer.Write` of `w.w`.
    async fn buf_write(&mut self, mut p: &[u8]) -> io::Result<usize> {
        let mut nn = 0;
        while p.len() > BUFFER_BEFORE_CHUNKING_SIZE - self.buf.len() && self.buf_err.is_none() {
            let n = if self.buf.is_empty() {
                let (n, err) = self.cw_write(p).await;
                self.buf_err = err;
                n
            } else {
                let n = BUFFER_BEFORE_CHUNKING_SIZE - self.buf.len();
                self.buf.extend_from_slice(p.get(..n).unwrap_or_default());
                let _flushed = self.buf_flush().await;
                n
            };
            nn += n;
            p = p.get(n..).unwrap_or_default();
        }
        if let Some(e) = &self.buf_err {
            return Err(e.to_io());
        }
        self.buf.extend_from_slice(p);
        Ok(nn + p.len())
    }

    /// Go: `bufio.Writer.Flush` of `w.w`.
    async fn buf_flush(&mut self) -> Result<(), StickyErr> {
        if let Some(e) = &self.buf_err {
            return Err(e.clone());
        }
        if self.buf.is_empty() {
            return Ok(());
        }
        let buf = mem::take(&mut self.buf);
        let (n, err) = self.cw_write(&buf).await;
        self.buf = buf;
        match err {
            None if n >= self.buf.len() => {
                self.buf.clear();
                Ok(())
            }
            None => {
                let e = StickyErr("short write".to_owned());
                let _shifted = self.buf.drain(..n);
                self.buf_err = Some(e.clone());
                Err(e)
            }
            Some(e) => {
                let _shifted = self.buf.drain(..n.min(self.buf.len()));
                self.buf_err = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Go: `chunkWriter.flush`.
    async fn cw_flush(&mut self) -> Result<(), StickyErr> {
        if !self.cw.wrote_header {
            self.cw_write_header(&[]).await;
        }
        self.conn
            .writer
            .lock()
            .await
            .flush()
            .await
            .map_err(StickyErr)
    }

    /// Go: `chunkWriter.close`.
    async fn cw_close(&mut self) {
        if !self.cw.wrote_header {
            self.cw_write_header(&[]).await;
        }
        if self.cw.chunking {
            let mut out = b"0\r\n".to_vec();
            if let Some(trailers) = self.final_trailers() {
                out.extend_from_slice(trailers.write().as_bytes());
            }
            out.extend_from_slice(b"\r\n");
            let _written = self.conn.writer.lock().await.write(&out).await;
        }
    }

    /// Go: `response.write`.
    async fn write_inner(&mut self, data: &[u8]) -> io::Result<usize> {
        self.sync_limit();
        if let Some(p) = &self.panic {
            return Err(io::Error::other(p.clone()));
        }
        self.send_informational().await;
        if self.can_write_continue.load(Ordering::SeqCst) {
            self.disable_write_continue();
        }
        if !self.wrote_header {
            self.write_header_inner(200);
        }
        if data.is_empty() {
            return Ok(0);
        }
        if !body_allowed_for_status(self.status) {
            return Err(io::Error::other(ERR_BODY_NOT_ALLOWED));
        }
        self.written += i64::try_from(data.len()).unwrap_or(i64::MAX);
        if self.content_length != -1 && self.written > self.content_length {
            return Err(io::Error::other(ERR_CONTENT_LENGTH));
        }
        self.buf_write(data).await
    }

    /// Go: `FlushError`.
    async fn flush_inner(&mut self) -> io::Result<()> {
        self.sync_limit();
        if let Some(p) = &self.panic {
            return Err(io::Error::other(p.clone()));
        }
        self.send_informational().await;
        if !self.wrote_header {
            self.write_header_inner(200);
        }
        let err = self.buf_flush().await;
        let e2 = self.cw_flush().await;
        err.and(e2).map_err(|e| e.to_io())
    }

    /// Go: `finishRequest` (the caller has already stopped the background read).
    pub(super) async fn finish_request(&mut self) {
        self.sync_limit();
        self.reply.handler_done = true;
        self.send_informational().await;
        if !self.wrote_header {
            self.write_header_inner(200);
        }
        let _flushed = self.buf_flush().await;
        self.cw_close().await;
        let _conn_flushed = self.conn.writer.lock().await.flush().await;
        if let Some(body) = &self.req_body {
            let _closed = body.close().await;
        }
        self.sync_limit();
    }

    /// Go: `closedRequestBodyEarly`.
    pub(super) async fn closed_request_body_early(&self) -> bool {
        if self.facts.ecr || self.swapped() {
            return false;
        }
        match &self.req_body {
            Some(body) => body.did_early_close().await,
            None => false,
        }
    }

    /// Go: `shouldReuseConnection`.
    pub(super) async fn should_reuse_connection(&self, werr: bool) -> bool {
        if self.reply.close_after_reply {
            return false;
        }
        if !self.facts.is_head
            && self.content_length != -1
            && body_allowed_for_status(self.status)
            && self.content_length != self.written
        {
            return false;
        }
        if werr {
            return false;
        }
        !self.closed_request_body_early().await
    }

    /// Go: `sendExpectationFailed`.
    pub(super) async fn send_expectation_failed(&mut self) {
        self.header_inner().set("Connection", "close");
        self.write_header_inner(417);
        self.finish_request().await;
    }
}

/// Go: `isProtocolSwitchResponse`.
fn is_protocol_switch_response(code: u16, h: &Header) -> bool {
    code == 101
        && !h.get("Upgrade").is_empty()
        && header_values_contains_token(h.get_raw("Connection").unwrap_or_default(), "Upgrade")
}

impl ResponseWriter for ServerResponse {
    fn header(&mut self) -> &mut Header {
        self.sync_limit();
        self.header_inner()
    }

    fn write_header(&mut self, code: u16) {
        self.sync_limit();
        self.write_header_inner(code);
    }

    fn write<'a>(&'a mut self, data: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(self.write_inner(data))
    }

    fn flush(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(self.flush_inner())
    }

    fn body_limit_notifier(&self) -> Option<BodyLimitNotifier> {
        Some(self.limit.clone())
    }
}
