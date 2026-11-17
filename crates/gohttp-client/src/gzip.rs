//! Go `compress/gzip.Reader` as the transport's `gzipReader` uses it for transparent
//! decompression: multistream, header CRC, trailer checks and Go's error values.

use std::{
    fmt,
    task::{Context as TaskContext, Poll, ready},
};

use flate2::{Crc, Decompress, FlushDecompress, Status};

use crate::{bufio::DEFAULT_BUF_SIZE, error::HttpError};

/// Go: `gzip.ErrHeader`.
pub const ERR_HEADER: HttpError = HttpError::Static("gzip: invalid header");
/// Go: `gzip.ErrChecksum`.
pub const ERR_CHECKSUM: HttpError = HttpError::Static("gzip: invalid checksum");

/// A Go `io.Reader` that may return data together with an error (the framed response body).
pub trait BodySource {
    /// Go: `Read(p)`: the bytes read and the error, if any.
    fn poll_body(
        &mut self,
        cx: &mut TaskContext<'_>,
        buf: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)>;
}

const FLAG_HCRC: u8 = 1 << 1;
const FLAG_EXTRA: u8 = 1 << 2;
const FLAG_NAME: u8 = 1 << 3;
const FLAG_COMMENT: u8 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The 10 fixed bytes.
    Fixed,
    ExtraLen,
    Extra,
    Name,
    Comment,
    HeaderCrc,
    Inflate,
    Trailer,
}

/// Go: `gzip.Reader` reading through its own 4096-byte `bufio.Reader`.
pub struct GzipReader {
    inbuf: Box<[u8]>,
    r: usize,
    w: usize,
    in_err: Option<HttpError>,
    phase: Phase,
    /// Bytes collected for the current fixed-size field.
    field: Vec<u8>,
    flags: u8,
    remaining: usize,
    header_crc: Crc,
    data_crc: Crc,
    decomp: Decompress,
    /// Go: `z.err`, sticky.
    err: Option<HttpError>,
    /// Whether at least one member was read (an empty input is `EOF`, not an error).
    member_bytes: usize,
}

impl fmt::Debug for GzipReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GzipReader")
            .field("phase", &self.phase)
            .field("err", &self.err)
            .finish_non_exhaustive()
    }
}

impl Default for GzipReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Go: `noEOF`.
fn no_eof(e: HttpError) -> HttpError {
    if e == HttpError::Eof {
        HttpError::UnexpectedEof
    } else {
        e
    }
}

impl GzipReader {
    /// A reader that will parse the first header on its first read.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inbuf: vec![0; DEFAULT_BUF_SIZE].into_boxed_slice(),
            r: 0,
            w: 0,
            in_err: None,
            phase: Phase::Fixed,
            field: Vec::with_capacity(10),
            flags: 0,
            remaining: 0,
            header_crc: Crc::new(),
            data_crc: Crc::new(),
            decomp: Decompress::new(false),
            err: None,
            member_bytes: 0,
        }
    }

    /// Go: `bufio.Reader.fill` over the body.
    fn poll_fill<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
    ) -> Poll<()> {
        if self.r > 0 {
            self.inbuf.copy_within(self.r..self.w, 0);
            self.w -= self.r;
            self.r = 0;
        }
        let Some(spare) = self.inbuf.get_mut(self.w..) else {
            return Poll::Ready(());
        };
        let (n, err) = ready!(src.poll_body(cx, spare));
        self.w += n;
        if err.is_some() {
            self.in_err = err;
        }
        Poll::Ready(())
    }

    /// Go: `bufio.Reader.ReadByte`.
    fn poll_byte<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
    ) -> Poll<Result<u8, HttpError>> {
        loop {
            if let Some(&b) = self.inbuf.get(self.r).filter(|_| self.r < self.w) {
                self.r += 1;
                return Poll::Ready(Ok(b));
            }
            if let Some(e) = self.in_err.take() {
                return Poll::Ready(Err(e));
            }
            ready!(self.poll_fill(cx, src));
        }
    }

    /// Reads the header fields after the fixed part; `Ready(Ok)` when the member's data begins.
    fn poll_header<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
    ) -> Poll<Result<(), HttpError>> {
        loop {
            match self.phase {
                Phase::Fixed => {
                    while self.field.len() < 10 {
                        match ready!(self.poll_byte(cx, src)) {
                            Ok(b) => self.field.push(b),
                            Err(e) => {
                                // Go: io.ReadFull: EOF before any byte is a clean end.
                                let e = if self.field.is_empty() { e } else { no_eof(e) };
                                return Poll::Ready(Err(e));
                            }
                        }
                    }
                    if !self.field.starts_with(&[0x1F, 0x8B, 8]) {
                        return Poll::Ready(Err(ERR_HEADER));
                    }
                    self.flags = self.field.get(3).copied().unwrap_or(0);
                    self.header_crc = Crc::new();
                    self.header_crc.update(&self.field);
                    self.field.clear();
                    self.phase = Phase::ExtraLen;
                }
                Phase::ExtraLen => {
                    if self.flags & FLAG_EXTRA == 0 {
                        self.phase = Phase::Name;
                        continue;
                    }
                    while self.field.len() < 2 {
                        let b = ready!(self.poll_byte(cx, src)).map_err(no_eof)?;
                        self.field.push(b);
                    }
                    self.header_crc.update(&self.field);
                    let lo = self.field.first().copied().unwrap_or(0);
                    let hi = self.field.get(1).copied().unwrap_or(0);
                    self.remaining = usize::from(u16::from_le_bytes([lo, hi]));
                    self.field.clear();
                    self.phase = Phase::Extra;
                }
                Phase::Extra => {
                    while self.remaining > 0 {
                        let b = ready!(self.poll_byte(cx, src)).map_err(no_eof)?;
                        self.header_crc.update(&[b]);
                        self.remaining -= 1;
                    }
                    self.phase = Phase::Name;
                }
                Phase::Name | Phase::Comment => {
                    let flag = if self.phase == Phase::Name {
                        FLAG_NAME
                    } else {
                        FLAG_COMMENT
                    };
                    if self.flags & flag != 0 {
                        loop {
                            // Go: readString reads at most 512 bytes, NUL included.
                            if self.remaining >= 512 {
                                return Poll::Ready(Err(ERR_HEADER));
                            }
                            let b = ready!(self.poll_byte(cx, src)).map_err(no_eof)?;
                            self.header_crc.update(&[b]);
                            self.remaining += 1;
                            if b == 0 {
                                break;
                            }
                        }
                        self.remaining = 0;
                    }
                    self.phase = if self.phase == Phase::Name {
                        Phase::Comment
                    } else {
                        Phase::HeaderCrc
                    };
                }
                Phase::HeaderCrc => {
                    if self.flags & FLAG_HCRC != 0 {
                        while self.field.len() < 2 {
                            let b = ready!(self.poll_byte(cx, src)).map_err(no_eof)?;
                            self.field.push(b);
                        }
                        let lo = self.field.first().copied().unwrap_or(0);
                        let hi = self.field.get(1).copied().unwrap_or(0);
                        // Go: the low 16 bits of the header's CRC-32.
                        let [c0, c1, _, _] = self.header_crc.sum().to_le_bytes();
                        if [lo, hi] != [c0, c1] {
                            return Poll::Ready(Err(ERR_HEADER));
                        }
                        self.field.clear();
                    }
                    self.decomp.reset(false);
                    self.data_crc.reset();
                    self.member_bytes = 0;
                    self.phase = Phase::Inflate;
                    return Poll::Ready(Ok(()));
                }
                Phase::Inflate | Phase::Trailer => return Poll::Ready(Ok(())),
            }
        }
    }

    /// One inflate step into `p[*n..]`: `Ready(Ok(true))` to keep going, `Ready(Ok(false))` when
    /// `p` is full.
    fn poll_inflate<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
        p: &mut [u8],
        n: &mut usize,
    ) -> Poll<Result<bool, HttpError>> {
        if *n > 0 && *n == p.len() {
            return Poll::Ready(Ok(false));
        }
        let input = self.inbuf.get(self.r..self.w).unwrap_or_default();
        let Some(out) = p.get_mut(*n..) else {
            return Poll::Ready(Ok(false));
        };
        let (in0, out0) = (self.decomp.total_in(), self.decomp.total_out());
        let status = self.decomp.decompress(input, out, FlushDecompress::None);
        let consumed = usize::try_from(self.decomp.total_in() - in0).unwrap_or(0);
        let produced = usize::try_from(self.decomp.total_out() - out0).unwrap_or(0);
        self.data_crc
            .update(out.get(..produced).unwrap_or_default());
        self.r += consumed;
        self.member_bytes += consumed;
        *n += produced;
        match status {
            Err(_corrupt) => Poll::Ready(Err(HttpError::Message(format!(
                "flate: corrupt input before offset {}",
                self.member_bytes
            )))),
            Ok(Status::StreamEnd) => {
                self.phase = Phase::Trailer;
                self.field.clear();
                Poll::Ready(Ok(true))
            }
            Ok(Status::Ok | Status::BufError) => {
                if produced > 0 || consumed > 0 {
                    return Poll::Ready(Ok(true));
                }
                if self.r < self.w {
                    // No progress with input available: the output is full.
                    return Poll::Ready(Ok(false));
                }
                if let Some(e) = self.in_err.take() {
                    return Poll::Ready(Err(no_eof(e)));
                }
                ready!(self.poll_fill(cx, src));
                Poll::Ready(Ok(true))
            }
        }
    }

    /// Go: the member trailer check (CRC-32 and size), then on to the next member.
    fn poll_trailer<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
    ) -> Poll<Result<(), HttpError>> {
        while self.field.len() < 8 {
            let b = ready!(self.poll_byte(cx, src)).map_err(no_eof)?;
            self.field.push(b);
        }
        let trailer = <[u8; 8]>::try_from(self.field.as_slice()).unwrap_or_default();
        let [d0, d1, d2, d3, s0, s1, s2, s3] = trailer;
        if u32::from_le_bytes([d0, d1, d2, d3]) != self.data_crc.sum()
            || u32::from_le_bytes([s0, s1, s2, s3]) != self.data_crc.amount()
        {
            return Poll::Ready(Err(ERR_CHECKSUM));
        }
        // The member is good; look for another.
        self.field.clear();
        self.phase = Phase::Fixed;
        Poll::Ready(Ok(()))
    }

    /// Go: `gzip.Reader.Read`: decompressed bytes and the sticky error (`EOF` at the end of
    /// the last member). `Pending` only when nothing was produced.
    pub fn poll_read<S: BodySource + ?Sized>(
        &mut self,
        cx: &mut TaskContext<'_>,
        src: &mut S,
        p: &mut [u8],
    ) -> Poll<(usize, Option<HttpError>)> {
        if let Some(e) = &self.err {
            return Poll::Ready((0, Some(e.clone())));
        }
        let mut n = 0_usize;
        loop {
            let step = match self.phase {
                Phase::Fixed
                | Phase::ExtraLen
                | Phase::Extra
                | Phase::Name
                | Phase::Comment
                | Phase::HeaderCrc => {
                    // Go: a new member's header ends a read that produced data.
                    self.poll_header(cx, src).map_ok(|()| n == 0)
                }
                Phase::Inflate => self.poll_inflate(cx, src, p, &mut n),
                Phase::Trailer => self.poll_trailer(cx, src).map_ok(|()| true),
            };
            match step {
                Poll::Pending if n == 0 => return Poll::Pending,
                Poll::Pending | Poll::Ready(Ok(false)) => return Poll::Ready((n, None)),
                Poll::Ready(Ok(true)) => {}
                Poll::Ready(Err(e)) => {
                    self.err = Some(e.clone());
                    return Poll::Ready((n, Some(e)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::{Compression, write::GzEncoder};

    use super::*;
    use crate::testutil::{ensure_eq, poll_ready};

    struct Src<'a>(&'a [u8]);

    impl BodySource for Src<'_> {
        fn poll_body(
            &mut self,
            _cx: &mut TaskContext<'_>,
            buf: &mut [u8],
        ) -> Poll<(usize, Option<HttpError>)> {
            let n = buf.len().min(self.0.len()).min(3);
            let (head, tail) = self.0.split_at(n);
            if let Some(dst) = buf.get_mut(..n) {
                dst.copy_from_slice(head);
            }
            self.0 = tail;
            Poll::Ready((n, self.0.is_empty().then_some(HttpError::Eof)))
        }
    }

    fn gunzip(data: &[u8]) -> Result<(Vec<u8>, HttpError), String> {
        let mut src = Src(data);
        let mut gz = GzipReader::new();
        let mut out = Vec::new();
        let mut buf = [0_u8; 5];
        loop {
            let (n, err) = poll_ready(|cx| gz.poll_read(cx, &mut src, &mut buf))?;
            out.extend_from_slice(buf.get(..n).unwrap_or_default());
            if let Some(e) = err {
                return Ok((out, e));
            }
        }
    }

    fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).map_err(|e| e.to_string())?;
        enc.finish().map_err(|e| e.to_string())
    }

    #[test]
    fn multistream_and_errors() -> Result<(), String> {
        let mut two = gzip(b"hello, ")?;
        two.extend(gzip(b"world")?);
        ensure_eq(
            &gunzip(&two)?,
            &(b"hello, world".to_vec(), HttpError::Eof),
            "two members",
        )?;
        ensure_eq(&gunzip(b"")?, &(Vec::new(), HttpError::Eof), "empty")?;
        ensure_eq(&gunzip(b"not gzip!!")?.1, &ERR_HEADER, "header")?;
        let mut bad = gzip(b"payload")?;
        let len = bad.len();
        if let Some(b) = bad.get_mut(len - 8) {
            *b ^= 0xFF;
        }
        ensure_eq(&gunzip(&bad)?.1, &ERR_CHECKSUM, "checksum")?;
        let good = gzip(b"payload")?;
        let cut = good.get(..good.len() - 3).unwrap_or_default();
        ensure_eq(
            &gunzip(cut)?.1,
            &HttpError::UnexpectedEof,
            "truncated trailer",
        )
    }
}
