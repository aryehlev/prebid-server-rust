//! Request bodies: Go's server `*body` (`transfer.go`) over a `LimitReader` or the chunked
//! reader (`internal/chunked.go`), the `expectContinueReader` that sends `100 Continue`,
//! `http.NoBody`, in-memory bodies and `MaxBytesReader`.

use core::{
    error::Error,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};
use std::{io, sync::Arc};

use pbs_gocompat::{
    http::{Header, canonical_header_key},
    strconv::quote,
};
use tokio::sync::{Mutex, Notify};

use super::{
    BoxFuture,
    bufio::{BufReader, ReadErr, SharedReader},
    netconn::NetError,
    writer::SharedWriter,
};

/// Go: `maxPostHandlerReadBytes`.
pub(super) const MAX_POST_HANDLER_READ_BYTES: i64 = 256 << 10;

/// Go: `internal.maxLineLength`.
const MAX_LINE_LENGTH: usize = 4096;

/// Go: `http.ErrBodyReadAfterClose`.
pub(super) const ERR_BODY_READ_AFTER_CLOSE: &str = "http: invalid Read on closed Body";

/// Go: the error texts of body reads.
fn read_err_to_io(err: ReadErr) -> io::Error {
    match err {
        ReadErr::Net(e) => e.into_io(),
        ReadErr::BufferFull => io::Error::other("bufio: buffer full"),
        ReadErr::Protocol(t) => io::Error::new(io::ErrorKind::InvalidData, t),
        ReadErr::MessageTooLarge => io::Error::new(io::ErrorKind::InvalidData, "message too large"),
    }
}

fn unexpected_eof() -> ReadErr {
    ReadErr::Protocol(UNEXPECTED_EOF.to_owned())
}

/// Go: `io.ErrUnexpectedEOF`.
const UNEXPECTED_EOF: &str = "unexpected EOF";

fn is_unexpected_eof(err: &ReadErr) -> bool {
    matches!(err, ReadErr::Protocol(t) if t == UNEXPECTED_EOF)
}

fn to_io(err: ReadErr) -> io::Error {
    if is_unexpected_eof(&err) {
        return io::Error::new(io::ErrorKind::UnexpectedEof, UNEXPECTED_EOF);
    }
    read_err_to_io(err)
}

/// Go: `internal.chunkedReader`.
#[derive(Debug, Default)]
struct Chunked {
    n: u64,
    err: Option<ReadErr>,
    check_end: bool,
    excess: i64,
}

/// Go: `beginChunk`'s overhead accounting: `excess` (which held the overhead before this chunk)
/// plus the chunk-size line of `line_len` bytes and its two CRLFs, less the 16 bytes and twice
/// the `n` data bytes the chunk is allowed, and at least 0.
///
/// The arithmetic is Go's `int64`, which wraps: for a size of 2^62 or more, `2 * int64(n)`
/// overflows, and some such sizes leave a huge excess (so Go reports too much non-data) where
/// others leave none.
fn chunk_excess(excess: i64, line_len: usize, n: u64) -> i64 {
    let line = i64::try_from(line_len).unwrap_or(i64::MAX);
    let allowed = 16_i64.wrapping_add(n.cast_signed().wrapping_mul(2));
    excess
        .wrapping_add(line.wrapping_add(2))
        .wrapping_sub(allowed)
        .max(0)
}

/// Go: `internal.parseHexUint`.
fn parse_hex_uint(v: &[u8]) -> Result<u64, ReadErr> {
    if v.is_empty() {
        return Err(ReadErr::Protocol(
            "empty hex number for chunk length".to_owned(),
        ));
    }
    let mut n: u64 = 0;
    for (i, &b) in v.iter().enumerate() {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(ReadErr::Protocol("invalid byte in chunk length".to_owned())),
        };
        if i == 16 {
            return Err(ReadErr::Protocol("http chunk length too large".to_owned()));
        }
        n = (n << 4) | u64::from(d);
    }
    Ok(n)
}

impl Chunked {
    /// Go: `readChunkLine`.
    async fn read_chunk_line(br: &mut BufReader) -> Result<Vec<u8>, ReadErr> {
        let (line, err) = br.read_slice_owned(b'\n').await;
        if let Some(e) = err {
            return Err(match e {
                ReadErr::Net(NetError::Eof) => unexpected_eof(),
                ReadErr::BufferFull => ReadErr::Protocol("header line too long".to_owned()),
                other @ (ReadErr::Net(_) | ReadErr::Protocol(_) | ReadErr::MessageTooLarge) => {
                    other
                }
            });
        }
        match line.iter().position(|&b| b == b'\r') {
            None => {
                return Err(ReadErr::Protocol(
                    "chunked line ends with bare LF".to_owned(),
                ));
            }
            Some(idx) if idx + 2 != line.len() => {
                return Err(ReadErr::Protocol("invalid CR in chunked line".to_owned()));
            }
            Some(_) => {}
        }
        let p = line.get(..line.len() - 2).unwrap_or_default();
        if p.len() >= MAX_LINE_LENGTH {
            return Err(ReadErr::Protocol("header line too long".to_owned()));
        }
        Ok(p.to_vec())
    }

    /// Go: `chunkedReader.beginChunk`.
    async fn begin_chunk(&mut self, br: &mut BufReader) {
        let line = match Self::read_chunk_line(br).await {
            Ok(l) => l,
            Err(e) => {
                self.err = Some(e);
                return;
            }
        };
        let mut end = line.len();
        while end > 0 && matches!(line.get(end - 1), Some(b' ' | b'\t')) {
            end -= 1;
        }
        let trimmed = line.get(..end).unwrap_or_default();
        let digits = trimmed
            .iter()
            .position(|&b| b == b';')
            .map_or(trimmed, |i| trimmed.get(..i).unwrap_or_default());
        match parse_hex_uint(digits) {
            Ok(n) => self.n = n,
            Err(e) => {
                self.err = Some(e);
                return;
            }
        }
        self.excess = chunk_excess(self.excess, line.len(), self.n);
        if self.excess > 16 * 1024 {
            self.err = Some(ReadErr::Protocol(
                "chunked encoding contains too much non-data".to_owned(),
            ));
        }
        if self.n == 0 {
            self.err = Some(ReadErr::Net(NetError::Eof));
        }
    }

    /// Go: `chunkedReader.Read`.
    async fn read(&mut self, br: &mut BufReader, b: &mut [u8]) -> (usize, Option<ReadErr>) {
        let mut n = 0;
        while self.err.is_none() {
            if self.check_end {
                if n > 0 && br.buffered() < 2 {
                    break;
                }
                match br.read_full(2).await {
                    Ok(two) => {
                        if two != b"\r\n" {
                            self.err =
                                Some(ReadErr::Protocol("malformed chunked encoding".to_owned()));
                            break;
                        }
                    }
                    Err(e) => {
                        self.err = Some(if e.is_eof() { unexpected_eof() } else { e });
                        break;
                    }
                }
                self.check_end = false;
            }
            if self.n == 0 {
                if n > 0 && !br.buffered_contains(b'\n') {
                    break;
                }
                self.begin_chunk(br).await;
                continue;
            }
            let rest = b.len() - n;
            if rest == 0 {
                break;
            }
            let want = usize::try_from(self.n).map_or(rest, |c| rest.min(c));
            let Some(rbuf) = b.get_mut(n..n + want) else {
                break;
            };
            let (n0, err) = br.read(rbuf).await;
            n += n0;
            self.n -= u64::try_from(n0).unwrap_or(0);
            self.err = err;
            if self.n == 0 && self.err.is_none() {
                self.check_end = true;
            } else if self.err.as_ref().is_some_and(ReadErr::is_eof) {
                self.err = Some(unexpected_eof());
            }
        }
        (n, self.err.clone())
    }
}

#[derive(Debug)]
enum Src {
    /// Go: `io.LimitReader(r, n)`.
    Limited(i64),
    Chunked(Chunked),
}

/// Go: `*body`'s mutable state (guarded by `body.mu`).
#[derive(Debug)]
pub(super) struct BodyState {
    src: Src,
    /// Go: `hdr != nil`: a trailer remains to be read.
    hdr: bool,
    saw_eof: bool,
    closed: bool,
    close: CloseMode,
    trailer: Option<Header>,
}

/// Go: `body.closing`, `body.doEarlyClose` and `body.earlyClose`.
#[derive(Debug)]
struct CloseMode {
    closing: bool,
    do_early_close: bool,
    early_close: bool,
}

/// The pieces of the connection a server body needs.
#[derive(Debug)]
pub(super) struct ServerBody {
    state: Mutex<BodyState>,
    reader: SharedReader,
    writer: SharedWriter,
    /// Go: `onHitEOF`: starts the connection's background read.
    pub(super) hit_eof: Arc<HitEof>,
    /// Go: `expectContinueReader` state, when the request asked for `100-continue`.
    pub(super) expect: Option<ExpectContinue>,
    /// Set once `req.Body` no longer holds this body (the handler dropped it or wrapped it in
    /// `MaxBytesReader`), which changes the type switches of Go's `writeHeader`.
    pub(super) swapped: Arc<AtomicBool>,
}

/// Go: `registerOnHitEOF` / `startBackgroundRead` signalling.
#[derive(Debug, Default)]
pub(super) struct HitEof {
    hit: AtomicBool,
    notify: Notify,
}

impl HitEof {
    pub(super) fn signal(&self) {
        self.hit.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(super) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.hit.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// Go: `expectContinueReader` and the response's `canWriteContinue`.
#[derive(Debug)]
pub(super) struct ExpectContinue {
    pub(super) can_write_continue: Arc<AtomicBool>,
    closed: AtomicBool,
    pub(super) saw_eof: AtomicBool,
}

impl ExpectContinue {
    pub(super) const fn new(can_write_continue: Arc<AtomicBool>) -> Self {
        Self {
            can_write_continue,
            closed: AtomicBool::new(false),
            saw_eof: AtomicBool::new(false),
        }
    }
}

/// How `readTransfer` framed a request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Framing {
    Length(i64),
    Chunked,
}

impl ServerBody {
    pub(super) fn new(
        framing: Framing,
        closing: bool,
        trailer: Option<Header>,
        reader: SharedReader,
        writer: SharedWriter,
    ) -> Self {
        let (src, hdr) = match framing {
            Framing::Length(n) => (Src::Limited(n), false),
            Framing::Chunked => (Src::Chunked(Chunked::default()), true),
        };
        Self {
            state: Mutex::new(BodyState {
                src,
                hdr,
                saw_eof: false,
                closed: false,
                close: CloseMode {
                    closing,
                    do_early_close: true,
                    early_close: false,
                },
                trailer,
            }),
            reader,
            writer,
            hit_eof: Arc::new(HitEof::default()),
            expect: None,
            swapped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Wraps the body in Go's `expectContinueReader`.
    pub(super) fn with_expect(mut self, expect: ExpectContinue) -> Self {
        self.expect = Some(expect);
        self
    }

    /// Go: `body.readLocked`.
    async fn read_locked(&self, st: &mut BodyState, p: &mut [u8]) -> (usize, Option<ReadErr>) {
        if st.saw_eof {
            return (0, Some(ReadErr::Net(NetError::Eof)));
        }
        let mut br = self.reader.lock().await;
        let (n, mut err) = match &mut st.src {
            Src::Limited(remaining) => {
                if *remaining <= 0 {
                    (0, Some(ReadErr::Net(NetError::Eof)))
                } else {
                    let len = usize::try_from(*remaining).map_or(p.len(), |r| p.len().min(r));
                    let (n, err) = br.read(p.get_mut(..len).unwrap_or_default()).await;
                    *remaining -= i64::try_from(n).unwrap_or(0);
                    (n, err)
                }
            }
            Src::Chunked(c) => c.read(&mut br, p).await,
        };
        if err.as_ref().is_some_and(ReadErr::is_eof) {
            st.saw_eof = true;
            if st.hdr {
                if let Err(e) = read_trailer(&mut br, st).await {
                    err = Some(e);
                    st.saw_eof = false;
                    st.closed = true;
                }
                st.hdr = false;
            } else if matches!(st.src, Src::Limited(r) if r > 0) {
                err = Some(unexpected_eof());
            }
        }
        drop(br);
        if err.is_none() && n > 0 && matches!(st.src, Src::Limited(0)) {
            err = Some(ReadErr::Net(NetError::Eof));
            st.saw_eof = true;
        }
        if st.saw_eof {
            self.hit_eof.signal();
        }
        (n, err)
    }

    /// Go: `body.Read`.
    pub(super) async fn read(&self, p: &mut [u8]) -> (usize, Option<ReadErr>) {
        let mut st = self.state.lock().await;
        if st.closed {
            return (
                0,
                Some(ReadErr::Protocol(ERR_BODY_READ_AFTER_CLOSE.to_owned())),
            );
        }
        let out = self.read_locked(&mut st, p).await;
        drop(st);
        out
    }

    /// Go: `io.Copy(io.Discard, bodyLocked{b})` up to `limit` bytes (`None` for no limit),
    /// returning the bytes read and the first error (`None` at a clean end of body).
    async fn discard_locked(
        &self,
        st: &mut BodyState,
        limit: Option<i64>,
    ) -> (i64, Option<ReadErr>) {
        let mut buf = vec![0_u8; 8192];
        let mut total: i64 = 0;
        loop {
            let want = limit.map_or(buf.len(), |l| {
                usize::try_from(l - total).map_or(buf.len(), |r| buf.len().min(r))
            });
            if want == 0 {
                return (total, None);
            }
            if st.closed {
                return (
                    total,
                    Some(ReadErr::Protocol(ERR_BODY_READ_AFTER_CLOSE.to_owned())),
                );
            }
            let (n, err) = self
                .read_locked(st, buf.get_mut(..want).unwrap_or_default())
                .await;
            total += i64::try_from(n).unwrap_or(0);
            if err.is_some() {
                return (total, err);
            }
        }
    }

    /// Go: `io.CopyN(io.Discard, body, n)`: `Ok(n)` when `n` bytes were read, `Err(EOF)` when
    /// the body ended first, other errors as they are.
    pub(super) async fn copy_n_discard(&self, n: i64) -> Result<i64, ReadErr> {
        let mut st = self.state.lock().await;
        if st.closed {
            return Err(ReadErr::Protocol(ERR_BODY_READ_AFTER_CLOSE.to_owned()));
        }
        let (written, err) = self.discard_locked(&mut st, Some(n)).await;
        drop(st);
        match err {
            Some(e) if !e.is_eof() => Err(e),
            _ if written == n => Ok(n),
            _ => Err(ReadErr::Net(NetError::Eof)),
        }
    }

    /// Go: `body.Close` (with `doEarlyClose` for server requests).
    pub(super) async fn close(&self) -> Option<ReadErr> {
        let mut st = self.state.lock().await;
        if st.closed {
            return None;
        }
        let mut err = None;
        if st.saw_eof || (!st.hdr && st.close.closing) {
            // Already at EOF, or the connection closes next.
        } else if st.close.do_early_close {
            if matches!(st.src, Src::Limited(r) if r > MAX_POST_HANDLER_READ_BYTES) {
                st.close.early_close = true;
            } else {
                let (n, e) = self
                    .discard_locked(&mut st, Some(MAX_POST_HANDLER_READ_BYTES))
                    .await;
                err = e.filter(|e| !e.is_eof());
                if n == MAX_POST_HANDLER_READ_BYTES {
                    st.close.early_close = true;
                }
            }
        } else {
            let (_, e) = self.discard_locked(&mut st, None).await;
            err = e.filter(|e| !e.is_eof());
        }
        st.closed = true;
        err
    }

    /// Go: `body.didEarlyClose`.
    pub(super) async fn did_early_close(&self) -> bool {
        self.state.lock().await.close.early_close
    }

    /// The state the response's `writeHeader` inspects: `(closed, saw_eof, unread)` where
    /// `unread` is `unreadDataSizeLocked` (-1 when unknown).
    pub(super) async fn write_header_view(&self) -> (bool, bool, i64) {
        let st = self.state.lock().await;
        let unread = match st.src {
            Src::Limited(n) => n,
            Src::Chunked(_) => -1,
        };
        (st.closed, st.saw_eof, unread)
    }

    async fn trailer(&self) -> Option<Header> {
        self.state.lock().await.trailer.clone()
    }

    /// Go: `expectContinueReader.Read`.
    async fn read_expect_continue(
        &self,
        ecr: &ExpectContinue,
        p: &mut [u8],
    ) -> (usize, Option<ReadErr>) {
        if ecr.closed.load(Ordering::SeqCst) {
            return (
                0,
                Some(ReadErr::Protocol(ERR_BODY_READ_AFTER_CLOSE.to_owned())),
            );
        }
        if ecr.can_write_continue.load(Ordering::SeqCst) {
            let mut w = self.writer.lock().await;
            if ecr.can_write_continue.load(Ordering::SeqCst) {
                let _written = w.write(b"HTTP/1.1 100 Continue\r\n\r\n").await;
                let _flushed = w.flush().await;
                ecr.can_write_continue.store(false, Ordering::SeqCst);
            }
            drop(w);
        }
        let (n, err) = self.read(p).await;
        if err.as_ref().is_some_and(ReadErr::is_eof) {
            ecr.saw_eof.store(true, Ordering::SeqCst);
        }
        (n, err)
    }
}

/// Go: `body.readTrailer`.
async fn read_trailer(br: &mut BufReader, st: &mut BodyState) -> Result<(), ReadErr> {
    const TRAILER_EOF: &str = "http: unexpected EOF reading trailer";
    let (buf, err) = br.peek(2).await;
    if buf == b"\r\n" {
        let _discarded = br.discard(2).await;
        return Ok(());
    }
    if buf.len() < 2 {
        return Err(ReadErr::Protocol(TRAILER_EOF.to_owned()));
    }
    if let Some(e) = err {
        return Err(e);
    }
    if !see_upcoming_double_crlf(br).await {
        return Err(ReadErr::Protocol(
            "http: suspiciously long trailer after chunked body".to_owned(),
        ));
    }
    let (fields, hdr_err) = br.read_mime_header().await;
    if let Some(e) = hdr_err {
        return Err(if e.is_eof() {
            ReadErr::Protocol(TRAILER_EOF.to_owned())
        } else {
            e
        });
    }
    let trailer = st.trailer.get_or_insert_with(Header::new);
    let mut parsed = Header::new();
    for (k, v) in fields {
        let mut values = parsed.remove_raw(&k).unwrap_or_default();
        values.push(v);
        parsed.insert_raw(k, values);
    }
    for (k, v) in parsed.into_map() {
        trailer.insert_raw(k, v);
    }
    Ok(())
}

/// Go: `seeUpcomingDoubleCRLF`.
async fn see_upcoming_double_crlf(br: &mut BufReader) -> bool {
    let mut peek_size = 4;
    loop {
        let (buf, err) = br.peek(peek_size).await;
        if buf.ends_with(b"\r\n\r\n") {
            return true;
        }
        if err.is_some() {
            return false;
        }
        peek_size += 1;
    }
}

/// A user-supplied request body (Go: any `io.ReadCloser` assigned to `Request.Body`).
pub trait BodyReader: Send + Sync {
    /// Go: `Read`: `Ok(0)` at the end of the body.
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<usize>>;

    /// Go: `Close`.
    fn close(&mut self) -> BoxFuture<'_, io::Result<()>>;
}

enum Kind {
    /// Go: `http.NoBody` (and a nil `Body`).
    Empty,
    /// Go: the server's `*body`, possibly behind an `expectContinueReader`.
    Server {
        body: Arc<ServerBody>,
        ecr: bool,
    },
    /// Go: `io.NopCloser(bytes.NewReader(b))`.
    Bytes {
        data: Vec<u8>,
        pos: usize,
    },
    /// Go: `*maxBytesReader`.
    MaxBytes(Box<MaxBytesReader>),
    Custom(Box<dyn BodyReader>),
}

/// Go: `Request.Body`: the request body as the handler reads it.
///
/// [`Body::read`] returns `Ok(0)` at the end of the body. An error that Go returns together
/// with data is returned by the following call, so [`Body::read_to_end`] sees the same bytes
/// and error as Go's `io.ReadAll`.
pub struct Body {
    kind: Kind,
    pending: Option<io::Error>,
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.kind {
            Kind::Empty => "NoBody",
            Kind::Server { .. } => "server",
            Kind::Bytes { .. } => "bytes",
            Kind::MaxBytes(_) => "MaxBytesReader",
            Kind::Custom(_) => "custom",
        };
        f.debug_struct("Body")
            .field("kind", &kind)
            .finish_non_exhaustive()
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        self.mark_swapped();
    }
}

impl Body {
    /// Go: `http.NoBody`.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            kind: Kind::Empty,
            pending: None,
        }
    }

    /// An in-memory body (Go: `io.NopCloser(bytes.NewReader(data))`).
    #[must_use]
    pub fn from_bytes<B: Into<Vec<u8>>>(data: B) -> Self {
        Self {
            kind: Kind::Bytes {
                data: data.into(),
                pos: 0,
            },
            pending: None,
        }
    }

    /// A body read from `reader`.
    #[must_use]
    pub fn from_reader<R: BodyReader + 'static>(reader: R) -> Self {
        Self {
            kind: Kind::Custom(Box::new(reader)),
            pending: None,
        }
    }

    pub(super) const fn server(body: Arc<ServerBody>, ecr: bool) -> Self {
        Self {
            kind: Kind::Server { body, ecr },
            pending: None,
        }
    }

    /// Whether this is `http.NoBody`.
    #[must_use]
    pub const fn is_no_body(&self) -> bool {
        matches!(self.kind, Kind::Empty)
    }

    /// Whether this body is a [`max_bytes_reader`] (Go: `r.Body.(*maxBytesReader)`).
    #[must_use]
    pub const fn is_max_bytes(&self) -> bool {
        matches!(self.kind, Kind::MaxBytes(_))
    }

    /// Records that `req.Body` no longer holds the server's body.
    fn mark_swapped(&self) {
        if let Kind::Server { body, .. } = &self.kind {
            body.swapped.store(true, Ordering::SeqCst);
        }
    }

    /// Go: `io.Copy(io.Discard, http.MaxBytesReader(w, body, n))` for a reader that is not
    /// stored back into the request (`globalOptionsHandler`).
    pub(super) async fn discard_limited(&mut self, notifier: Option<&BodyLimitNotifier>, n: i64) {
        let mut left = n;
        let mut buf = vec![0_u8; 8192];
        loop {
            let cap =
                usize::try_from(left.saturating_add(1)).map_or(buf.len(), |c| buf.len().min(c));
            match self.read(buf.get_mut(..cap).unwrap_or_default()).await {
                Ok(0) | Err(_) => return,
                Ok(got) => {
                    let got = i64::try_from(got).unwrap_or(i64::MAX);
                    if got <= left {
                        left -= got;
                        continue;
                    }
                    if let Some(notifier) = notifier {
                        notifier.notify();
                    }
                    return;
                }
            }
        }
    }

    /// Go: `Read`. `Ok(0)` is the end of the body (Go's `io.EOF`).
    ///
    /// # Errors
    ///
    /// The Go error, with Go's text: `unexpected EOF`, `http: invalid Read on closed Body`,
    /// `malformed chunked encoding`, `read tcp a->b: i/o timeout`, ...
    pub async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(e) = self.pending.take() {
            return Err(e);
        }
        let (n, err) = match &mut self.kind {
            Kind::Empty => return Ok(0),
            Kind::Bytes { data, pos } => {
                let rest = data.get(*pos..).unwrap_or_default();
                let n = rest.len().min(buf.len());
                if let (Some(dst), Some(src)) = (buf.get_mut(..n), rest.get(..n)) {
                    dst.copy_from_slice(src);
                }
                *pos += n;
                return Ok(n);
            }
            Kind::Custom(r) => return r.read(buf).await,
            Kind::MaxBytes(r) => return r.read(buf).await,
            Kind::Server { body, ecr } => {
                if buf.is_empty() {
                    return Ok(0);
                }
                match (&body.expect, *ecr) {
                    (Some(e), true) => body.read_expect_continue(e, buf).await,
                    _ => body.read(buf).await,
                }
            }
        };
        match err {
            None => Ok(n),
            Some(e) if e.is_eof() => Ok(n),
            Some(e) if n > 0 => {
                self.pending = Some(to_io(e));
                Ok(n)
            }
            Some(e) => Err(to_io(e)),
        }
    }

    /// Go: `io.ReadAll(body)`: appends the body to `out` and returns the number of bytes read.
    ///
    /// # Errors
    ///
    /// The first read error; the bytes read before it are already in `out`.
    pub async fn read_to_end(&mut self, out: &mut Vec<u8>) -> io::Result<usize> {
        let start = out.len();
        let mut chunk = vec![0_u8; 512];
        loop {
            let n = self.read(&mut chunk).await?;
            if n == 0 {
                return Ok(out.len() - start);
            }
            out.extend_from_slice(chunk.get(..n).unwrap_or_default());
            if chunk.len() < 64 << 10 && n == chunk.len() {
                chunk.resize(chunk.len() * 2, 0);
            }
        }
    }

    /// Go: `Close`.
    ///
    /// # Errors
    ///
    /// The error of reading the rest of the body.
    pub async fn close(&mut self) -> io::Result<()> {
        match &mut self.kind {
            Kind::Empty | Kind::Bytes { .. } => Ok(()),
            Kind::Custom(r) => r.close().await,
            Kind::MaxBytes(r) => r.close().await,
            Kind::Server { body, ecr } => {
                if *ecr && let Some(e) = &body.expect {
                    e.closed.store(true, Ordering::SeqCst);
                }
                body.close().await.map_or(Ok(()), |e| Err(to_io(e)))
            }
        }
    }

    /// Go: `Request.Trailer` after the body was read to its end: the trailer fields the client
    /// sent after a chunked body (declared keys without values before that).
    pub fn trailer(&self) -> BoxFuture<'_, Option<Header>> {
        Box::pin(async move {
            match &self.kind {
                Kind::Server { body, .. } => body.trailer().await,
                Kind::MaxBytes(r) => r.r.trailer().await,
                Kind::Empty | Kind::Bytes { .. } | Kind::Custom(_) => None,
            }
        })
    }
}

/// Go: `*http.MaxBytesError` (`http: request body too large`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxBytesError {
    /// Go: `Limit`.
    pub limit: i64,
}

impl fmt::Display for MaxBytesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("http: request body too large")
    }
}

impl Error for MaxBytesError {}

/// Go: the `requestTooLarge` hook `MaxBytesReader` calls on the server's response writer.
#[derive(Debug, Clone, Default)]
pub struct BodyLimitNotifier {
    pub(super) hit: Arc<AtomicBool>,
}

impl BodyLimitNotifier {
    fn notify(&self) {
        self.hit.store(true, Ordering::SeqCst);
    }
}

/// Go: `maxBytesReader`.
struct MaxBytesReader {
    notifier: Option<BodyLimitNotifier>,
    r: Body,
    i: i64,
    n: i64,
    /// Go: `l.err` (sticky), as kind and text.
    err: Option<(io::ErrorKind, String)>,
    err_is_max: bool,
}

impl BodyReader for MaxBytesReader {
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<usize>> {
        let p = buf;
        Box::pin(async move {
            if self.err_is_max {
                return Err(io::Error::other(MaxBytesError { limit: self.i }));
            }
            if let Some((kind, text)) = &self.err {
                return Err(io::Error::new(*kind, text.clone()));
            }
            if p.is_empty() {
                return Ok(0);
            }
            let cap = usize::try_from(self.n.saturating_add(1)).map_or(p.len(), |c| p.len().min(c));
            let result = self.r.read(p.get_mut(..cap).unwrap_or_default()).await;
            let n = match result {
                Ok(n) => n,
                Err(e) => {
                    self.err = Some((e.kind(), e.to_string()));
                    return Err(e);
                }
            };
            let n64 = i64::try_from(n).unwrap_or(i64::MAX);
            if n64 <= self.n {
                self.n -= n64;
                return Ok(n);
            }
            let rest = usize::try_from(self.n).unwrap_or(0);
            self.n = 0;
            if let Some(notifier) = &self.notifier {
                notifier.notify();
            }
            self.err_is_max = true;
            if rest > 0 {
                return Ok(rest);
            }
            Err(io::Error::other(MaxBytesError { limit: self.i }))
        })
    }

    fn close(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(self.r.close())
    }
}

/// Go: `http.MaxBytesReader(w, r, n)`: reads of `body` fail with [`MaxBytesError`] past `n`
/// bytes, and (on the server's own response writer) the connection closes after the reply.
///
/// `notifier` is [`super::ResponseWriter::body_limit_notifier`] of the response writer; wrappers
/// that do not forward it (as Go's do not forward `requestTooLarge`) pass `None`.
#[must_use]
pub fn max_bytes_reader(notifier: Option<BodyLimitNotifier>, body: Body, n: i64) -> Body {
    body.mark_swapped();
    let n = n.max(0);
    Body {
        kind: Kind::MaxBytes(Box::new(MaxBytesReader {
            notifier,
            r: body,
            i: n,
            n,
            err: None,
            err_is_max: false,
        })),
        pending: None,
    }
}

/// Go: `fixTrailer`'s declared keys.
pub(super) fn declared_trailer(values: &[String]) -> Result<Option<Header>, String> {
    let mut trailer = Header::new();
    for v in values {
        for key in super::lex::header_elements(v) {
            let key = canonical_header_key(key);
            if matches!(
                key.as_str(),
                "Transfer-Encoding" | "Trailer" | "Content-Length"
            ) {
                return Err(format!("bad trailer key {}", quote(&key)));
            }
            trailer.insert_raw(key, Vec::new());
        }
    }
    Ok((!trailer.is_empty()).then_some(trailer))
}

#[cfg(test)]
mod tests {
    use super::chunk_excess;

    /// Go's `excess` after one chunk-size line, from the overhead before it, the line's length
    /// and the chunk size, computed by Go 1.26.2's `beginChunk` arithmetic.
    #[test]
    fn chunk_excess_wraps_like_go() -> Result<(), String> {
        for (before, line, n, want) in [
            (0, 1, 0x5, 0),
            (0, 16, 0x1000_0000_0000_0000, 0),
            (0, 16, 0x3FFF_FFFF_FFFF_FFFF, 0),
            (0, 16, 0x4000_0000_0000_0000, 0),
            (0, 16, 0x4000_0000_0000_0001, 0),
            (0, 16, 0x4000_0000_0000_0002, 9_223_372_036_854_775_806),
            (0, 16, 0x6000_0000_0000_0000, 4_611_686_018_427_387_906),
            (0, 16, 0x7FFF_FFFF_FFFF_E000, 16386),
            (0, 16, 0x7FFF_FFFF_FFFF_FFFF, 4),
            (0, 16, 0x8000_0000_0000_0000, 2),
            (0, 16, 0xC000_0000_0000_0001, 0),
            (0, 16, 0xC000_0000_0000_0002, 9_223_372_036_854_775_806),
            (0, 16, 0xFFFF_FFFF_FFFF_E000, 16386),
            (0, 16, 0xFFFF_FFFF_FFFF_EFFF, 8196),
            (0, 16, 0xFFFF_FFFF_FFFF_FFFF, 4),
            (15944, 4002, 0x1, 19930),
            (12000, 4002, 0x1, 15986),
            (16000, 402, 0x0, 16388),
        ] {
            let got = chunk_excess(before, line, n);
            if got != want {
                return Err(format!(
                    "chunk_excess({before}, {line}, {n:#x}) = {got}, Go: {want}"
                ));
            }
        }
        Ok(())
    }
}
