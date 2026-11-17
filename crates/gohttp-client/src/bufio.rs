//! Go `bufio.Reader` over a connection, as the transport's `persistConn.br` uses it.
//!
//! The buffer has Go's default 4096 bytes, which decides where Go reports `bufio.ErrBufferFull`
//! (a chunk-size line longer than the buffer) and how much of a response Go has read from the
//! socket at any moment. Operations are poll-based and restartable: they consume buffered bytes
//! only when they complete.

use std::{
    fmt,
    task::{Context as TaskContext, Poll, ready},
};

use crate::error::HttpError;

/// Go: `bufio.defaultBufSize`, the transport's read buffer size.
pub const DEFAULT_BUF_SIZE: usize = 4096;

/// A Go `io.Reader`: `Ok(n)` has `n > 0` for a non-empty buffer; the end of input is
/// `Err(HttpError::Eof)`.
pub trait GoRead {
    /// Go: `Read(p)`.
    fn poll_go_read(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, HttpError>>;
}

/// Go: `*bufio.Reader` of [`DEFAULT_BUF_SIZE`] bytes.
pub struct BufReader {
    buf: Box<[u8]>,
    r: usize,
    w: usize,
    err: Option<HttpError>,
}

/// The outcome of [`BufReader::poll_read_slice`] when it does not find the delimiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceError {
    /// Go: `bufio.ErrBufferFull`: the buffer is full and has no delimiter.
    BufferFull,
    /// The reader's error (Go returns the buffered bytes along with it).
    Read(HttpError),
}

/// [`BufReader::poll_read_slice`]'s result: the `(start, len)` of the line, or the error with
/// the `(start, len)` of the bytes Go returns along with it.
pub type SliceResult = Result<(usize, usize), (SliceError, usize, usize)>;

impl fmt::Debug for BufReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufReader")
            .field("buffered", &self.buffered())
            .field("err", &self.err)
            .finish_non_exhaustive()
    }
}

impl Default for BufReader {
    fn default() -> Self {
        Self::new()
    }
}

impl BufReader {
    /// An empty reader with Go's default size.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: vec![0; DEFAULT_BUF_SIZE].into_boxed_slice(),
            r: 0,
            w: 0,
            err: None,
        }
    }

    /// Go: `Buffered()`.
    #[must_use]
    pub const fn buffered(&self) -> usize {
        self.w - self.r
    }

    /// The buffered bytes (Go: `Peek(Buffered())`).
    #[must_use]
    pub fn buffer(&self) -> &[u8] {
        self.buf.get(self.r..self.w).unwrap_or_default()
    }

    /// Go: `Discard(n)` for `n <= Buffered()`.
    pub fn consume(&mut self, n: usize) {
        self.r = (self.r + n).min(self.w);
    }

    /// Go: `readErr()`: returns and clears the sticky error.
    pub const fn take_err(&mut self) -> Option<HttpError> {
        self.err.take()
    }

    /// Whether a read error is pending.
    #[must_use]
    pub const fn has_err(&self) -> bool {
        self.err.is_some()
    }

    /// Go: `fill()`: slides the buffered bytes to the front and reads once.
    pub fn poll_fill<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut R,
    ) -> Poll<()> {
        if self.r > 0 {
            self.buf.copy_within(self.r..self.w, 0);
            self.w -= self.r;
            self.r = 0;
        }
        let Some(spare) = self.buf.get_mut(self.w..) else {
            return Poll::Ready(());
        };
        if spare.is_empty() {
            return Poll::Ready(());
        }
        match ready!(src.poll_go_read(cx, spare)) {
            Ok(n) => self.w += n.min(spare.len()),
            Err(e) => self.err = Some(e),
        }
        Poll::Ready(())
    }

    /// Go: `Peek(n)` for `n <= DEFAULT_BUF_SIZE`: waits until `n` bytes are buffered or the
    /// reader fails. `Ok` means at least `n` bytes are buffered.
    pub fn poll_peek<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut R,
        n: usize,
    ) -> Poll<Result<(), HttpError>> {
        while self.buffered() < n && self.buffered() < self.buf.len() && self.err.is_none() {
            ready!(self.poll_fill(cx, src));
        }
        if self.buffered() >= n {
            return Poll::Ready(Ok(()));
        }
        Poll::Ready(Err(self
            .take_err()
            .unwrap_or(HttpError::Static("bufio: buffer full"))))
    }

    /// Go: `ReadSlice(delim)`: on success, consumes and returns the range of the line
    /// (delimiter included) in [`BufReader::buffer`]'s coordinates before the call, as
    /// `(start, len)` into the internal buffer, retrieved with [`BufReader::slice`].
    pub fn poll_read_slice<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut R,
        delim: u8,
    ) -> Poll<SliceResult> {
        let mut s = 0;
        loop {
            if let Some(i) = self
                .buf
                .get(self.r + s..self.w)
                .and_then(|b| b.iter().position(|&c| c == delim))
            {
                let start = self.r;
                let len = s + i + 1;
                self.r += len;
                return Poll::Ready(Ok((start, len)));
            }
            if let Some(e) = self.err.take() {
                let (start, len) = (self.r, self.buffered());
                self.r = self.w;
                return Poll::Ready(Err((SliceError::Read(e), start, len)));
            }
            if self.buffered() >= self.buf.len() {
                let (start, len) = (self.r, self.buffered());
                self.r = self.w;
                return Poll::Ready(Err((SliceError::BufferFull, start, len)));
            }
            s = self.buffered();
            ready!(self.poll_fill(cx, src));
            // poll_fill slid the buffer to the front.
        }
    }

    /// The bytes at `(start, len)` returned by [`BufReader::poll_read_slice`] (valid until the
    /// next fill).
    #[must_use]
    pub fn slice(&self, start: usize, len: usize) -> &[u8] {
        self.buf.get(start..start + len).unwrap_or_default()
    }

    /// Go: `ReadLine()`: appends the line without its `\n` or `\r\n` to `out` and returns
    /// `isPrefix`. A final line without a newline is returned as a line.
    pub fn poll_read_line<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut R,
        out: &mut Vec<u8>,
    ) -> Poll<Result<bool, HttpError>> {
        match ready!(self.poll_read_slice(cx, src, b'\n')) {
            Ok((start, len)) => {
                let line = self.slice(start, len);
                let drop = if line.ends_with(b"\r\n") { 2 } else { 1 };
                out.extend_from_slice(line.get(..len - drop).unwrap_or_default());
                Poll::Ready(Ok(false))
            }
            Err((SliceError::BufferFull, start, len)) => {
                let line = self.slice(start, len);
                let ends_with_cr = line.ends_with(b"\r");
                let keep = if ends_with_cr { len - 1 } else { len };
                out.extend_from_slice(line.get(..keep).unwrap_or_default());
                if ends_with_cr {
                    // Put the '\r' back; the next call checks for "\r\n".
                    self.r -= 1;
                }
                Poll::Ready(Ok(true))
            }
            Err((SliceError::Read(e), start, len)) => {
                if len == 0 {
                    return Poll::Ready(Err(e));
                }
                out.extend_from_slice(self.slice(start, len));
                Poll::Ready(Ok(false))
            }
        }
    }

    /// Go: `Read(p)`.
    pub fn poll_read<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut R,
        p: &mut [u8],
    ) -> Poll<Result<usize, HttpError>> {
        if p.is_empty() {
            if self.buffered() > 0 {
                return Poll::Ready(Ok(0));
            }
            return Poll::Ready(self.take_err().map_or(Ok(0), Err));
        }
        if self.r == self.w {
            if let Some(e) = self.take_err() {
                return Poll::Ready(Err(e));
            }
            if p.len() >= self.buf.len() {
                // Large read, empty buffer: read directly into p.
                return src.poll_go_read(cx, p);
            }
            self.r = 0;
            self.w = 0;
            match ready!(src.poll_go_read(cx, &mut self.buf)) {
                Ok(n) => self.w = n.min(self.buf.len()),
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        let avail = self.buffer();
        let n = avail.len().min(p.len());
        if let (Some(dst), Some(src_bytes)) = (p.get_mut(..n), avail.get(..n)) {
            dst.copy_from_slice(src_bytes);
        }
        self.r += n;
        Poll::Ready(Ok(n))
    }
}
