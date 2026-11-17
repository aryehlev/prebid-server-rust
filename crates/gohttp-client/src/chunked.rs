//! Go `net/http/internal/chunked.go`: the chunked transfer-coding reader and writer framing.

use std::{
    fmt::Write as _,
    task::{Context as TaskContext, Poll},
};

use crate::{
    bufio::{BufReader, GoRead, SliceError},
    error::HttpError,
};

/// Go: `internal.maxLineLength`.
const MAX_LINE_LENGTH: usize = 4096;

/// Go: `internal.ErrLineTooLong`.
pub const ERR_LINE_TOO_LONG: HttpError = HttpError::Static("header line too long");

/// Go: `chunkedReader`: decodes the chunked body read from a [`BufReader`].
#[derive(Debug, Default)]
pub struct ChunkedReader {
    /// Unread bytes in the current chunk.
    n: u64,
    /// Whether the `\r\n` after the current chunk is still to be read.
    check_end: bool,
    /// The sticky error (`EOF` after the last chunk).
    err: Option<HttpError>,
    /// Go: `excess`, the non-data bytes beyond the allowance.
    excess: i64,
}

/// Go: `parseHexUint`.
fn parse_hex_uint(v: &[u8]) -> Result<u64, HttpError> {
    if v.is_empty() {
        return Err(HttpError::Static("empty hex number for chunk length"));
    }
    let mut n: u64 = 0;
    for (i, &b) in v.iter().enumerate() {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(HttpError::Static("invalid byte in chunk length")),
        };
        if i == 16 {
            return Err(HttpError::Static("http chunk length too large"));
        }
        n = (n << 4) | u64::from(d);
    }
    Ok(n)
}

impl ChunkedReader {
    /// Go: `chunkHeaderAvailable`.
    fn chunk_header_available(br: &BufReader) -> bool {
        br.buffer().contains(&b'\n')
    }

    /// Go: `beginChunk`. `Pending` consumes nothing.
    fn poll_begin_chunk<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        br: &mut BufReader,
        src: &mut R,
    ) -> Poll<()> {
        let (start, len) = match br.poll_read_slice(cx, src, b'\n') {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(found)) => found,
            Poll::Ready(Err((SliceError::Read(HttpError::Eof), ..))) => {
                self.err = Some(HttpError::UnexpectedEof);
                return Poll::Ready(());
            }
            Poll::Ready(Err((SliceError::Read(e), ..))) => {
                self.err = Some(e);
                return Poll::Ready(());
            }
            Poll::Ready(Err((SliceError::BufferFull, ..))) => {
                self.err = Some(ERR_LINE_TOO_LONG);
                return Poll::Ready(());
            }
        };
        self.err = self.chunk_line(br.slice(start, len)).err();
        Poll::Ready(())
    }

    /// Go: the part of `beginChunk` after `readChunkLine` read `p` (newline included).
    fn chunk_line(&mut self, p: &[u8]) -> Result<(), HttpError> {
        match p.iter().position(|&b| b == b'\r') {
            None => return Err(HttpError::Static("chunked line ends with bare LF")),
            Some(idx) if idx + 2 != p.len() => {
                return Err(HttpError::Static("invalid CR in chunked line"));
            }
            Some(_) => {}
        }
        let line = p.get(..p.len() - 2).unwrap_or_default();
        if line.len() >= MAX_LINE_LENGTH {
            return Err(ERR_LINE_TOO_LONG);
        }
        // Header, plus the \r\n after the chunk data.
        self.excess = self
            .excess
            .wrapping_add(i64::try_from(line.len()).unwrap_or(i64::MAX))
            .wrapping_add(2);
        let end = line
            .iter()
            .rposition(|&b| b != b' ' && b != b'\t')
            .map_or(0, |i| i + 1);
        let trimmed = line.get(..end).unwrap_or_default();
        let size = trimmed.split(|&b| b == b';').next().unwrap_or_default();
        self.n = parse_hex_uint(size)?;
        self.excess = self
            .excess
            .wrapping_sub(16_i64.wrapping_add(2_i64.wrapping_mul(self.n.cast_signed())))
            .max(0);
        if self.excess > 16 * 1024 {
            return Err(HttpError::Static(
                "chunked encoding contains too much non-data",
            ));
        }
        if self.n == 0 {
            return Err(HttpError::Eof);
        }
        Ok(())
    }

    /// Go: `chunkedReader.Read`: the bytes read and the sticky error (`EOF` after the last
    /// chunk, possibly together with data). `Pending` only when nothing was read.
    pub fn poll_read<R: GoRead + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        br: &mut BufReader,
        src: &mut R,
        b: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        let mut n = 0_usize;
        while self.err.is_none() {
            if self.check_end {
                if n > 0 && br.buffered() < 2 {
                    break;
                }
                match br.poll_peek(cx, src, 2) {
                    Poll::Pending if n == 0 => return Poll::Pending,
                    Poll::Pending => break,
                    Poll::Ready(Ok(())) => {
                        let crlf = br.buffer().starts_with(b"\r\n");
                        br.consume(2);
                        if !crlf {
                            self.err = Some(HttpError::Static("malformed chunked encoding"));
                            break;
                        }
                    }
                    Poll::Ready(Err(e)) => {
                        self.err = Some(if e == HttpError::Eof {
                            HttpError::UnexpectedEof
                        } else {
                            e
                        });
                        break;
                    }
                }
                self.check_end = false;
            }
            if self.n == 0 {
                if n > 0 && !Self::chunk_header_available(br) {
                    break;
                }
                match self.poll_begin_chunk(cx, br, src) {
                    Poll::Pending if n == 0 => return Poll::Pending,
                    Poll::Pending => break,
                    Poll::Ready(()) => continue,
                }
            }
            let Some(rest) = b.get_mut(n..) else {
                break;
            };
            if rest.is_empty() {
                break;
            }
            let want = usize::try_from(self.n)
                .unwrap_or(usize::MAX)
                .min(rest.len());
            let Some(rbuf) = rest.get_mut(..want) else {
                break;
            };
            match br.poll_read(cx, src, rbuf) {
                Poll::Pending if n == 0 => return Poll::Pending,
                Poll::Pending => break,
                Poll::Ready(Ok(n0)) => {
                    n += n0;
                    self.n -= u64::try_from(n0).unwrap_or(0);
                    if self.n == 0 {
                        self.check_end = true;
                    }
                }
                Poll::Ready(Err(e)) => {
                    self.err = Some(if e == HttpError::Eof {
                        HttpError::UnexpectedEof
                    } else {
                        e
                    });
                }
            }
        }
        Poll::Ready((n, self.err.clone()))
    }
}

/// Go: `chunkedWriter.Write` framing for one non-empty write: `<hex len>\r\n<data>\r\n`.
pub fn chunk_header(len: usize, out: &mut Vec<u8>) {
    let mut s = String::with_capacity(8);
    let _infallible = write!(s, "{len:x}\r\n");
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{SliceSource, ensure_eq, poll_ready};

    fn decode(wire: &[u8], chunk: usize) -> Result<(Vec<u8>, Option<HttpError>), String> {
        let mut src = SliceSource::new(wire, chunk);
        let mut br = BufReader::new();
        let mut cr = ChunkedReader::default();
        let mut out = Vec::new();
        let mut buf = [0_u8; 7];
        loop {
            let (n, err) = poll_ready(|cx| cr.poll_read(cx, &mut br, &mut src, &mut buf))?;
            out.extend_from_slice(buf.get(..n).unwrap_or_default());
            if let Some(e) = err {
                return Ok((out, Some(e)));
            }
        }
    }

    #[test]
    fn chunked_decoding_and_errors() -> Result<(), String> {
        let (body, end) = decode(b"3\r\nabc\r\n10;ext=1\r\n0123456789abcdef\r\n0\r\n\r\n", 5)?;
        ensure_eq(&body, &b"abc0123456789abcdef".to_vec(), "body")?;
        ensure_eq(&end, &Some(HttpError::Eof), "eof")?;
        let cases: [(&[u8], &str); 6] = [
            (b"3\nabc", "chunked line ends with bare LF"),
            (b"3\r\rx\r\n", "invalid CR in chunked line"),
            (b"\r\n", "empty hex number for chunk length"),
            (b"zz\r\n", "invalid byte in chunk length"),
            (b"11111111111111111\r\n", "http chunk length too large"),
            (b"3\r\nabcXY", "malformed chunked encoding"),
        ];
        for (wire, want) in cases {
            let (_, failure) = decode(wire, 64)?;
            ensure_eq(
                &failure.map(|e| e.to_string()),
                &Some(want.to_owned()),
                want,
            )?;
        }
        let (_, truncated) = decode(b"5\r\nab", 64)?;
        ensure_eq(&truncated, &Some(HttpError::UnexpectedEof), "truncated")
    }
}
