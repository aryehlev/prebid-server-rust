//! The read side of a server connection: Go's `connReader` (read limit, background byte,
//! read-error cancellation) under a 4096-byte `bufio.Reader`, with the `net/textproto` line and
//! MIME header readers the request parser uses.

use core::ops::Range;
use std::sync::Arc;

use pbs_gocompat::{http::canonical_mime_header_key, strconv::quote_bytes};
use tokio::{sync::Mutex, time::Instant};

use super::{
    context::RequestContext,
    lex::{trim_left_ows, trim_ows_bytes, valid_header_value_byte},
    netconn::{NetError, NetReader},
};

/// Go: `bufio.defaultBufSize`.
pub(super) const BUF_SIZE: usize = 4096;

/// An error from the buffered reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadErr {
    /// An error from the connection (EOF included).
    Net(NetError),
    /// Go: `bufio.ErrBufferFull`.
    BufferFull,
    /// Go: `textproto.ProtocolError` and other parse errors; the text is only logged.
    Protocol(String),
    /// Go: `textproto.errMessageTooLarge`.
    MessageTooLarge,
}

impl ReadErr {
    pub(super) const fn is_eof(&self) -> bool {
        matches!(self, Self::Net(NetError::Eof))
    }
}

/// Go: `connReader` plus the conn's `bufio.Reader`.
#[derive(Debug)]
pub(super) struct BufReader {
    net: NetReader,
    buf: Box<[u8]>,
    r: usize,
    w: usize,
    err: Option<ReadErr>,
    last_byte: Option<u8>,
    /// Go: `connReader.remain`.
    pub(super) remain: i64,
    /// Go: `connReader.hasByte` / `byteBuf`.
    has_byte: Option<u8>,
    /// The context `handleReadErrorLocked` cancels (the conn's, which every request's derives
    /// from).
    pub(super) cancel_on_error: Option<RequestContext>,
    /// Set once a read failed (Go cancels the conn context, so later request contexts start
    /// cancelled).
    pub(super) conn_cancelled: bool,
}

impl BufReader {
    pub(super) fn new(net: NetReader) -> Self {
        Self {
            net,
            buf: vec![0; BUF_SIZE].into_boxed_slice(),
            r: 0,
            w: 0,
            err: None,
            last_byte: None,
            remain: i64::MAX,
            has_byte: None,
            cancel_on_error: None,
            conn_cancelled: false,
        }
    }

    /// Go: `connReader.handleReadErrorLocked`.
    fn handle_read_error(&mut self) {
        self.conn_cancelled = true;
        if let Some(ctx) = &self.cancel_on_error {
            ctx.cancel();
        }
    }

    /// Go: `connReader.Read`.
    async fn conn_read(&mut self, start: usize) -> Result<usize, NetError> {
        if self.remain <= 0 {
            return Err(NetError::Eof);
        }
        let room = self.buf.len().saturating_sub(start);
        if room == 0 {
            return Ok(0);
        }
        let lim = usize::try_from(self.remain).map_or(room, |r| room.min(r));
        if let Some(b) = self.has_byte.take() {
            if let Some(slot) = self.buf.get_mut(start) {
                *slot = b;
            }
            return Ok(1);
        }
        let Some(dst) = self.buf.get_mut(start..start + lim) else {
            return Ok(0);
        };
        let result = self.net.read(dst).await;
        match result {
            Ok(n) => {
                self.remain -= i64::try_from(n).unwrap_or(i64::MAX);
                Ok(n)
            }
            Err(e) => {
                self.handle_read_error();
                Err(e)
            }
        }
    }

    /// Go: `connReader.Read` into a caller's buffer (a large `bufio.Reader.Read`).
    async fn conn_read_into(&mut self, p: &mut [u8]) -> Result<usize, NetError> {
        if self.remain <= 0 {
            return Err(NetError::Eof);
        }
        if p.is_empty() {
            return Ok(0);
        }
        let lim = usize::try_from(self.remain).map_or(p.len(), |r| p.len().min(r));
        if let Some(b) = self.has_byte.take() {
            if let Some(slot) = p.first_mut() {
                *slot = b;
            }
            return Ok(1);
        }
        let Some(dst) = p.get_mut(..lim) else {
            return Ok(0);
        };
        match self.net.read(dst).await {
            Ok(n) => {
                self.remain -= i64::try_from(n).unwrap_or(i64::MAX);
                Ok(n)
            }
            Err(e) => {
                self.handle_read_error();
                Err(e)
            }
        }
    }

    /// Go: `connReader.backgroundRead` (started by `startBackgroundRead`, which clears the read
    /// deadline). Dropping the future is `abortPendingRead`.
    pub(super) async fn background_read(&mut self) {
        if self.has_byte.is_some() {
            return;
        }
        self.net.deadline = None;
        let mut byte = [0_u8; 1];
        match self.net.read(&mut byte).await {
            Ok(n) => {
                if n == 1 {
                    self.has_byte = Some(byte[0]);
                }
            }
            Err(_) => self.handle_read_error(),
        }
    }

    /// Go: `bufio.Reader.readErr`.
    const fn read_err(&mut self) -> Option<ReadErr> {
        self.err.take()
    }

    /// Go: `bufio.Reader.Buffered`.
    pub(super) const fn buffered(&self) -> usize {
        self.w - self.r
    }

    /// Go: `bufio.Reader.fill`.
    async fn fill(&mut self) {
        if self.r > 0 {
            self.buf.copy_within(self.r..self.w, 0);
            self.w -= self.r;
            self.r = 0;
        }
        if self.w >= self.buf.len() {
            return;
        }
        // Go retries empty reads; a connection read never returns 0 bytes without an error.
        match self.conn_read(self.w).await {
            Ok(n) => {
                self.w += n;
                if n == 0 {
                    self.err = Some(ReadErr::Protocol(
                        "multiple Read calls return no data or error".to_owned(),
                    ));
                }
            }
            Err(e) => self.err = Some(ReadErr::Net(e)),
        }
    }

    /// Go: `bufio.Reader.Peek`, returning the available bytes and the error.
    pub(super) async fn peek(&mut self, n: usize) -> (&[u8], Option<ReadErr>) {
        self.last_byte = None;
        while self.w - self.r < n && self.w - self.r < self.buf.len() && self.err.is_none() {
            self.fill().await;
        }
        if n > self.buf.len() {
            return (
                self.buf.get(self.r..self.w).unwrap_or_default(),
                Some(ReadErr::BufferFull),
            );
        }
        let avail = self.w - self.r;
        let (n, err) = if avail < n {
            (avail, Some(self.read_err().unwrap_or(ReadErr::BufferFull)))
        } else {
            (n, None)
        };
        (self.buf.get(self.r..self.r + n).unwrap_or_default(), err)
    }

    /// Go: `bufio.Reader.Discard`.
    pub(super) async fn discard(&mut self, n: usize) -> (usize, Option<ReadErr>) {
        if n == 0 {
            return (0, None);
        }
        self.last_byte = None;
        let mut remain = n;
        loop {
            let mut skip = self.buffered();
            if skip == 0 {
                self.fill().await;
                skip = self.buffered();
            }
            skip = skip.min(remain);
            self.r += skip;
            remain -= skip;
            if remain == 0 {
                return (n, None);
            }
            if self.err.is_some() {
                return (n - remain, self.read_err());
            }
        }
    }

    /// Go: `bufio.Reader.ReadByte`.
    async fn read_byte(&mut self) -> Result<u8, ReadErr> {
        while self.r == self.w {
            if let Some(e) = self.read_err() {
                return Err(e);
            }
            self.fill().await;
        }
        let c = self.buf.get(self.r).copied().unwrap_or(0);
        self.r += 1;
        self.last_byte = Some(c);
        Ok(c)
    }

    /// Go: `bufio.Reader.UnreadByte` right after a `ReadByte`.
    fn unread_byte(&mut self) {
        if let Some(b) = self.last_byte.take() {
            if self.r > 0 {
                self.r -= 1;
            } else {
                self.w = 1;
            }
            if let Some(slot) = self.buf.get_mut(self.r) {
                *slot = b;
            }
        }
    }

    /// Go: `bufio.Reader.Read`.
    pub(super) async fn read(&mut self, p: &mut [u8]) -> (usize, Option<ReadErr>) {
        if p.is_empty() {
            if self.buffered() > 0 {
                return (0, None);
            }
            return (0, self.read_err());
        }
        if self.r == self.w {
            if let Some(e) = self.read_err() {
                return (0, Some(e));
            }
            if p.len() >= self.buf.len() {
                let result = self.conn_read_into(p).await;
                return match result {
                    Ok(n) => {
                        if n > 0 {
                            self.last_byte = p.get(n - 1).copied();
                        }
                        (n, None)
                    }
                    Err(e) => (0, Some(ReadErr::Net(e))),
                };
            }
            self.r = 0;
            self.w = 0;
            match self.conn_read(0).await {
                Ok(0) => return (0, self.read_err()),
                Ok(n) => self.w += n,
                Err(e) => return (0, Some(ReadErr::Net(e))),
            }
        }
        let n = p.len().min(self.w - self.r);
        if let (Some(dst), Some(src)) = (p.get_mut(..n), self.buf.get(self.r..self.r + n)) {
            dst.copy_from_slice(src);
        }
        self.r += n;
        self.last_byte = self.buf.get(self.r.wrapping_sub(1)).copied();
        (n, None)
    }

    /// Go: `bufio.Reader.ReadSlice`: the line (with `delim`) as a range of the buffer.
    async fn read_slice(&mut self, delim: u8) -> (Range<usize>, Option<ReadErr>) {
        let mut s = 0;
        let (range, err) = loop {
            if let Some(i) = self
                .buf
                .get(self.r + s..self.w)
                .and_then(|b| b.iter().position(|&c| c == delim))
            {
                let i = i + s;
                let range = self.r..self.r + i + 1;
                self.r += i + 1;
                break (range, None);
            }
            if self.err.is_some() {
                let range = self.r..self.w;
                self.r = self.w;
                break (range, self.read_err());
            }
            if self.buffered() >= self.buf.len() {
                self.r = self.w;
                break (0..self.buf.len(), Some(ReadErr::BufferFull));
            }
            s = self.w - self.r;
            self.fill().await;
        };
        if range.end > range.start {
            self.last_byte = self.buf.get(range.end - 1).copied();
        }
        (range, err)
    }

    /// Go: `bufio.Reader.ReadSlice`, copied out of the buffer.
    pub(super) async fn read_slice_owned(&mut self, delim: u8) -> (Vec<u8>, Option<ReadErr>) {
        let (range, err) = self.read_slice(delim).await;
        (self.buf.get(range).unwrap_or_default().to_vec(), err)
    }

    /// Go: `io.ReadFull(r, buf[:n])`.
    pub(super) async fn read_full(&mut self, n: usize) -> Result<Vec<u8>, ReadErr> {
        let mut out = vec![0_u8; n];
        let mut got = 0;
        while got < n {
            let (nn, err) = self.read(out.get_mut(got..).unwrap_or_default()).await;
            got += nn;
            if let Some(e) = err {
                if got >= n {
                    break;
                }
                return Err(if got > 0 && e.is_eof() {
                    ReadErr::Protocol("unexpected EOF".to_owned())
                } else {
                    e
                });
            }
        }
        Ok(out)
    }

    /// Go: `bytes.IndexByte(peek(Buffered()), b) >= 0` (`chunkHeaderAvailable`).
    pub(super) fn buffered_contains(&self, b: u8) -> bool {
        self.buf.get(self.r..self.w).is_some_and(|s| s.contains(&b))
    }

    /// Go: `bufio.Reader.ReadLine`: a line fragment without its end of line, whether the line
    /// continues, or an error (never both a line and an error).
    async fn read_line_fragment(&mut self) -> Result<(Vec<u8>, bool), ReadErr> {
        let (mut range, err) = self.read_slice(b'\n').await;
        if err == Some(ReadErr::BufferFull) {
            if range.end > range.start && self.buf.get(range.end - 1) == Some(&b'\r') {
                self.r -= 1;
                range.end -= 1;
            }
            return Ok((self.buf.get(range).unwrap_or_default().to_vec(), true));
        }
        if range.is_empty() {
            return err.map_or_else(|| Ok((Vec::new(), false)), Err);
        }
        let line = self.buf.get(range).unwrap_or_default();
        let mut end = line.len();
        if line.last() == Some(&b'\n') {
            end -= 1;
            if end > 0 && line.get(end - 1) == Some(&b'\r') {
                end -= 1;
            }
        }
        Ok((line.get(..end).unwrap_or_default().to_vec(), false))
    }

    /// Go: `textproto.Reader.readLineSlice(lim)` (`lim < 0` is unlimited).
    pub(super) async fn read_line_slice(&mut self, lim: i64) -> Result<Vec<u8>, ReadErr> {
        let mut line: Vec<u8> = Vec::new();
        loop {
            let (l, more) = self.read_line_fragment().await?;
            let total = i64::try_from(line.len() + l.len()).unwrap_or(i64::MAX);
            if lim >= 0 && total > lim {
                return Err(ReadErr::MessageTooLarge);
            }
            if line.is_empty() && !more {
                return Ok(l);
            }
            line.extend_from_slice(&l);
            if !more {
                return Ok(line);
            }
        }
    }

    /// Go: `textproto.Reader.skipSpace`.
    async fn skip_space(&mut self) -> usize {
        let mut n = 0;
        loop {
            match self.read_byte().await {
                // `ReadByte` consumed the error; the next read meets it again.
                Err(_) => return n,
                Ok(c) if c != b' ' && c != b'\t' => {
                    self.unread_byte();
                    return n;
                }
                Ok(_) => n += 1,
            }
        }
    }

    /// Go: `textproto.Reader.readContinuedLineSlice(lim, mustHaveFieldNameColon)`.
    async fn read_continued_line_slice(&mut self) -> Result<Vec<u8>, ReadErr> {
        let line = self.read_line_slice(-1).await?;
        if line.is_empty() {
            return Ok(line);
        }
        if !line.contains(&b':') {
            return Err(ReadErr::Protocol(format!(
                "malformed MIME header: missing colon: {}",
                quote_bytes(&line)
            )));
        }
        let mut buf = trim_ows_bytes(&line).to_vec();
        while self.skip_space().await > 0 {
            buf.push(b' ');
            match self.read_line_slice(-1).await {
                Ok(l) => buf.extend_from_slice(trim_ows_bytes(&l)),
                Err(_) => break,
            }
        }
        Ok(buf)
    }

    /// Go: `textproto.Reader.ReadMIMEHeader`: keys canonicalized as Go does (keys holding a
    /// space are kept as they are), values in order. Values that are not UTF-8 are converted
    /// lossily.
    pub(super) async fn read_mime_header(&mut self) -> (Vec<(String, String)>, Option<ReadErr>) {
        let mut out: Vec<(String, String)> = Vec::new();
        let (peeked, peek_err) = self.peek(1).await;
        let starts_with_space = peeked.first().is_some_and(|&b| b == b' ' || b == b'\t');
        if peek_err.is_none() && starts_with_space {
            return match self.read_line_slice(80).await {
                Err(e) => (out, Some(e)),
                Ok(line) => (
                    out,
                    Some(ReadErr::Protocol(format!(
                        "malformed MIME header initial line: {}",
                        String::from_utf8_lossy(&line)
                    ))),
                ),
            };
        }
        // Go ignores the Peek error (consumed by `readErr`); the next read meets it again.
        loop {
            let (kv, err) = match self.read_continued_line_slice().await {
                Ok(kv) => (kv, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            if kv.is_empty() {
                return (out, err);
            }
            let malformed = || {
                ReadErr::Protocol(format!(
                    "malformed MIME header line: {}",
                    String::from_utf8_lossy(&kv)
                ))
            };
            let Some(colon) = kv.iter().position(|&b| b == b':') else {
                return (out, Some(malformed()));
            };
            let (k, v) = (
                kv.get(..colon).unwrap_or_default(),
                kv.get(colon + 1..).unwrap_or_default(),
            );
            let Some(key) = canonical_mime_header_key_bytes(k) else {
                return (out, Some(malformed()));
            };
            if !v.iter().all(|&c| valid_header_value_byte(c)) {
                return (out, Some(malformed()));
            }
            let value = String::from_utf8_lossy(trim_left_ows(v)).into_owned();
            out.push((key, value));
            if err.is_some() {
                return (out, err);
            }
        }
    }

    /// Go: `rwc.SetReadDeadline`.
    pub(super) const fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.net.deadline = deadline;
    }
}

/// Go: `textproto.canonicalMIMEHeaderKey`: `None` for invalid keys; keys containing a space are
/// returned unchanged.
pub(super) fn canonical_mime_header_key_bytes(a: &[u8]) -> Option<String> {
    if a.is_empty() {
        return None;
    }
    let mut no_canon = false;
    for &c in a {
        if super::lex::is_token_byte(c) {
            continue;
        }
        if c == b' ' {
            no_canon = true;
            continue;
        }
        return None;
    }
    let text = String::from_utf8_lossy(a).into_owned();
    if no_canon {
        return Some(text);
    }
    Some(canonical_mime_header_key(&text))
}

/// Shared ownership of the read side.
pub(super) type SharedReader = Arc<Mutex<BufReader>>;
