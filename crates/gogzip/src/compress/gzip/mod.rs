//! Go: `compress/gzip`: the gzip format (RFC 1952). This file is Go's `gzip.go` (the
//! [`Writer`]); `gunzip.rs` is Go's `gunzip.go` (the [`Reader`] and the [`Header`]).

#[cfg(test)]
mod golden_test;
mod gunzip;

use core::{fmt, mem};
use std::io::Write;

use gunzip::{FLAG_COMMENT, FLAG_EXTRA, FLAG_NAME, GZIP_DEFLATE, GZIP_ID1, GZIP_ID2};
pub use gunzip::{Header, Reader};
use pbs_gocompat::time::Time;

pub use super::flate::{
    BEST_COMPRESSION, BEST_SPEED, DEFAULT_COMPRESSION, HUFFMAN_ONLY, NO_COMPRESSION,
};
use super::{flate::Compressor, goio};
use crate::{GzipError, crc32};

/// Go: the low 32 bits of a length, as `uint32(len(p))`.
fn low32(n: usize) -> u32 {
    u32::try_from(n & 0xFFFF_FFFF).unwrap_or(0)
}

/// Go: `gzip.Writer`: writes the gzip form of the data written to it to an underlying writer,
/// byte for byte as Go does.
///
/// The [`Header`] is written at the first [`Writer::write`], [`Writer::flush`] or
/// [`Writer::close`]; set it before that through [`Writer::header_mut`]. The compressor is
/// created at the first write and kept by [`Writer::reset`], so a reset writer compresses
/// without allocating (Go's `sync.Pool` of writers).
pub struct Writer<W> {
    /// Go: the embedded `Header`.
    header: Header,
    w: W,
    level: isize,
    wrote_header: bool,
    closed: bool,
    buf: [u8; 10],
    compressor: Option<Box<Compressor>>,
    /// Go: CRC-32, IEEE polynomial (section 8).
    digest: u32,
    /// Go: uncompressed size (section 2.3.1).
    size: u32,
    err: Option<GzipError>,
}

impl<W> fmt::Debug for Writer<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("header", &self.header)
            .field("level", &self.level)
            .field("wrote_header", &self.wrote_header)
            .field("closed", &self.closed)
            .field("err", &self.err)
            .finish_non_exhaustive()
    }
}

/// Go: the `Header` a new or reset writer starts with.
fn initial_header() -> Header {
    Header {
        // Go: unknown.
        os: 255,
        ..Header::default()
    }
}

/// Go: `Writer.writeBytes`: a length-prefixed byte slice.
fn write_bytes(w: &mut dyn Write, b: &[u8]) -> Result<(), GzipError> {
    let Ok(len) = u16::try_from(b.len()) else {
        return Err(GzipError::ExtraTooLarge);
    };
    goio::write(w, &len.to_le_bytes())?;
    goio::write(w, b)
}

/// Go: `Writer.writeString`: `s` as a NUL-terminated Latin-1 string.
fn write_string(w: &mut dyn Write, s: &str) -> Result<(), GzipError> {
    // Go: GZIP stores Latin-1 strings; error if non-Latin-1; convert if non-ASCII.
    let mut needconv = false;
    for v in s.chars() {
        if v == '\0' || u32::from(v) > 0xFF {
            return Err(GzipError::NonLatin1);
        }
        if u32::from(v) > 0x7F {
            needconv = true;
        }
    }
    if needconv {
        let b: Vec<u8> = s.chars().filter_map(|v| u8::try_from(v).ok()).collect();
        goio::write(w, &b)?;
    } else {
        goio::write(w, s.as_bytes())?;
    }
    // Go: GZIP strings are NUL-terminated.
    goio::write(w, &[0])
}

impl<W: Write> Writer<W> {
    /// Go: `gzip.NewWriter(w)`: a writer at [`DEFAULT_COMPRESSION`].
    pub fn new(w: W) -> Self {
        Self {
            header: initial_header(),
            w,
            level: DEFAULT_COMPRESSION,
            wrote_header: false,
            closed: false,
            buf: [0; 10],
            compressor: None,
            digest: 0,
            size: 0,
            err: None,
        }
    }

    /// Go: `gzip.NewWriterLevel(w, level)`: levels as in [`crate::flate::Writer::new`].
    ///
    /// # Errors
    ///
    /// [`GzipError::InvalidGzipLevel`] when `level` is outside -2 to 9.
    pub fn new_level(w: W, level: isize) -> Result<Self, GzipError> {
        if !(HUFFMAN_ONLY..=BEST_COMPRESSION).contains(&level) {
            return Err(GzipError::InvalidGzipLevel(level));
        }
        let mut z = Self::new(w);
        z.level = level;
        Ok(z)
    }

    /// Go: the embedded `Header` fields.
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Go: the embedded `Header` fields, to set before the first write. Later changes are
    /// not written.
    pub const fn header_mut(&mut self) -> &mut Header {
        &mut self.header
    }

    /// Go: `Writer.Reset(w)`: discards the state, as if newly created with `w` and the same
    /// level (the header too), keeping the compressor. Returns the previous underlying writer.
    pub fn reset(&mut self, w: W) -> W {
        if let Some(compressor) = self.compressor.as_mut() {
            compressor.reset();
        }
        self.header = initial_header();
        self.wrote_header = false;
        self.closed = false;
        self.buf = [0; 10];
        self.digest = 0;
        self.size = 0;
        self.err = None;
        mem::replace(&mut self.w, w)
    }

    /// The header part of Go's `Writer.Write`.
    fn write_header(&mut self) -> Result<(), GzipError> {
        let h = &self.header;
        let mut flags = 0;
        if h.extra.is_some() {
            flags |= FLAG_EXTRA;
        }
        if !h.name.is_empty() {
            flags |= FLAG_NAME;
        }
        if !h.comment.is_empty() {
            flags |= FLAG_COMMENT;
        }
        // Go: section 2.3.1, the zero value for MTIME means that the modified time is not set.
        let mtime = if h.mod_time.after(&Time::from_unix(0, 0)) {
            h.mod_time.unix()
        } else {
            0
        };
        let [m0, m1, m2, m3, ..] = mtime.to_le_bytes();
        let xfl = match self.level {
            BEST_COMPRESSION => 2,
            BEST_SPEED => 4,
            _ => 0,
        };
        self.buf = [
            GZIP_ID1,
            GZIP_ID2,
            GZIP_DEFLATE,
            flags,
            m0,
            m1,
            m2,
            m3,
            xfl,
            h.os,
        ];
        goio::write(&mut self.w, &self.buf)?;
        if let Some(extra) = &h.extra {
            write_bytes(&mut self.w, extra)?;
        }
        if !h.name.is_empty() {
            write_string(&mut self.w, &h.name)?;
        }
        if !h.comment.is_empty() {
            write_string(&mut self.w, &h.comment)?;
        }
        Ok(())
    }

    /// Go: `Writer.Write`: compresses `p`, writing the header first if it has not been
    /// written; returns `p.len()`. The compressed bytes are not necessarily written to the
    /// underlying writer until [`Writer::flush`] or [`Writer::close`].
    ///
    /// # Errors
    ///
    /// The underlying writer's error (sticky), a header error ([`GzipError::ExtraTooLarge`],
    /// [`GzipError::NonLatin1`]), or [`GzipError::WriterClosed`] after [`Writer::close`].
    pub fn write(&mut self, p: &[u8]) -> Result<usize, GzipError> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        // Go: write the GZIP header lazily.
        if !self.wrote_header {
            self.wrote_header = true;
            if let Err(err) = self.write_header() {
                self.err = Some(err.clone());
                return Err(err);
            }
            if self.compressor.is_none() {
                self.compressor = Compressor::new(self.level).ok().map(Box::new);
            }
        }
        self.size = self.size.wrapping_add(low32(p.len()));
        self.digest = crc32::update(self.digest, p);
        let result = match self.compressor.as_mut() {
            Some(compressor) => compressor.write(&mut self.w, p),
            None => Ok(0),
        };
        self.err = result.as_ref().err().cloned();
        result
    }

    /// Go: `Writer.Flush`: writes the pending compressed data followed by a DEFLATE sync
    /// marker, the header first if it has not been written. Does nothing after
    /// [`Writer::close`]. The underlying writer is not flushed.
    ///
    /// # Errors
    ///
    /// The underlying writer's error (sticky) or a header error.
    pub fn flush(&mut self) -> Result<(), GzipError> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if self.closed {
            return Ok(());
        }
        if !self.wrote_header {
            let _written = self.write(&[])?;
        }
        let result = match self.compressor.as_mut() {
            Some(compressor) => compressor.sync_flush(&mut self.w),
            None => Ok(()),
        };
        self.err = result.as_ref().err().cloned();
        result
    }

    /// Go: `Writer.Close`: writes the pending data, the final block and the trailer (CRC-32
    /// and size), the header first if it has not been written. Closing again does nothing;
    /// the underlying writer is not closed.
    ///
    /// # Errors
    ///
    /// The underlying writer's error (sticky) or a header error.
    pub fn close(&mut self) -> Result<(), GzipError> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if !self.wrote_header {
            let _written = self.write(&[])?;
        }
        if let Some(compressor) = self.compressor.as_mut()
            && let Err(err) = compressor.close(&mut self.w)
        {
            self.err = Some(err.clone());
            return Err(err);
        }
        let [d0, d1, d2, d3] = self.digest.to_le_bytes();
        let [s0, s1, s2, s3] = self.size.to_le_bytes();
        self.err = goio::write(&mut self.w, &[d0, d1, d2, d3, s0, s1, s2, s3]).err();
        self.err.clone().map_or(Ok(()), Err)
    }

    /// The underlying writer.
    pub const fn get_ref(&self) -> &W {
        &self.w
    }

    /// The underlying writer, mutably. Writing to it directly corrupts the stream.
    pub const fn get_mut(&mut self) -> &mut W {
        &mut self.w
    }

    /// The underlying writer, discarding the compressor.
    pub fn into_inner(self) -> W {
        self.w
    }
}

#[cfg(test)]
mod tests {
    //! Ports of Go's `gzip_test.go`. The golden files cover the bytes of every level and header;
    //! these are Go's own checks.

    use std::io::{self, BufRead, Write};

    use pbs_gocompat::time::Time;

    use super::{Header, Reader, Writer, write_string};
    use crate::GzipError;

    /// Go: `w.Write(b)` in Go's tests, which check the error and not the count.
    fn put<W: Write>(w: &mut Writer<W>, b: &[u8]) -> Result<(), String> {
        w.write(b).map(|_| ()).map_err(|err| err.to_string())
    }

    /// Go: `io.ReadAll(r)`.
    fn read_all<R: BufRead>(r: &mut Reader<R>) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut buf = [0; 512];
        loop {
            let (n, err) = r.read(&mut buf);
            out.extend_from_slice(buf.get(..n).unwrap_or_default());
            match err {
                None => {}
                Some(GzipError::Eof) => return Ok(out),
                Some(err) => return Err(format!("ReadAll: {err}")),
            }
        }
    }

    /// The header of Go's `TestRoundTrip` and `TestWriterFlush`.
    fn set_header(h: &mut Header) {
        h.comment = "comment".to_owned();
        h.extra = Some(b"extra".to_vec());
        h.mod_time = Time::from_unix(100_000_000, 0);
        h.name = "name".to_owned();
    }

    /// Go: `TestEmpty`: an empty payload still forms a valid gzip stream.
    #[test]
    fn empty() -> Result<(), String> {
        let mut w = Writer::new(Vec::new());
        w.close().map_err(|err| format!("Writer.Close: {err}"))?;
        let buf = w.into_inner();
        let mut r = Reader::new(buf.as_slice()).map_err(|err| format!("NewReader: {err}"))?;
        let want = Header {
            os: 255,
            ..Header::default()
        };
        if *r.header() != want {
            return Err(format!(
                "Header mismatch:\ngot  {:?}\nwant {want:?}",
                r.header()
            ));
        }
        let b = read_all(&mut r)?;
        if !b.is_empty() {
            return Err(format!("got {} bytes, want 0", b.len()));
        }
        r.close().map_err(|err| format!("Reader.Close: {err}"))
    }

    /// Go: `TestRoundTrip`: gzipping and then gunzipping is the identity function.
    #[test]
    fn round_trip() -> Result<(), String> {
        let mut w = Writer::new(Vec::new());
        set_header(w.header_mut());
        put(&mut w, b"payload").map_err(|err| format!("Write: {err}"))?;
        w.close().map_err(|err| format!("Writer.Close: {err}"))?;
        let buf = w.into_inner();
        let mut r = Reader::new(buf.as_slice()).map_err(|err| format!("NewReader: {err}"))?;
        let b = read_all(&mut r)?;
        if b != b"payload" {
            return Err(format!("payload is {b:?}, want \"payload\""));
        }
        let h = r.header();
        if h.comment != "comment" {
            return Err(format!("comment is {:?}, want \"comment\"", h.comment));
        }
        if h.extra.as_deref() != Some(b"extra".as_slice()) {
            return Err(format!("extra is {:?}, want \"extra\"", h.extra));
        }
        if h.mod_time.unix() != 100_000_000 {
            return Err(format!("mtime is {}, want 100000000", h.mod_time.unix()));
        }
        if h.name != "name" {
            return Err(format!("name is {:?}, want \"name\"", h.name));
        }
        r.close().map_err(|err| format!("Reader.Close: {err}"))
    }

    /// Go: `TestLatin1`'s `writeString` (the `readString` half is in `gunzip.rs`).
    #[test]
    fn latin1() -> Result<(), String> {
        let latin1 = [0xC4, b'u', 0xDF, b'e', b'r', b'u', b'n', b'g', 0];
        let mut buf = Vec::with_capacity(latin1.len());
        write_string(&mut buf, "Äußerung").map_err(|err| format!("writeString: {err}"))?;
        if buf != latin1 {
            return Err(format!("write utf-8: got {buf:?}, want {latin1:?}"));
        }
        Ok(())
    }

    /// Go: `TestLatin1RoundTrip`: metadata that is representable in Latin-1 survives a round
    /// trip.
    #[test]
    fn latin1_round_trip() -> Result<(), String> {
        let test_cases = [
            ("", true),
            ("ASCII is OK", true),
            ("unless it contains a NUL\x00", false),
            ("no matter where \x00 occurs", false),
            ("\x00\x00\x00", false),
            ("Látin-1 also passes (U+00E1)", true),
            ("but LĀtin Extended-A (U+0100) does not", false),
            ("neither does 日本語", false),
            // Go: "invalid UTF-8 also \xffails"; Go reads the invalid byte as U+FFFD, which is
            // what a Rust string holds in its place.
            ("invalid UTF-8 also \u{FFFD}ails", false),
            ("\x00 as does Látin-1 with NUL", false),
        ];
        for (name, ok) in test_cases {
            let mut w = Writer::new(Vec::new());
            name.clone_into(&mut w.header_mut().name);
            let closed = w.close();
            if closed.is_ok() != ok {
                return Err(format!("Writer.Close: name = {name:?}, err = {closed:?}"));
            }
            if !ok {
                continue;
            }
            let buf = w.into_inner();
            let mut r = Reader::new(buf.as_slice()).map_err(|err| format!("NewReader: {err}"))?;
            let _data = read_all(&mut r)?;
            if r.header().name != name {
                return Err(format!("name is {:?}, want {name:?}", r.header().name));
            }
            r.close().map_err(|err| format!("Reader.Close: {err}"))?;
        }
        Ok(())
    }

    /// Go: `TestWriterFlush`.
    #[test]
    fn writer_flush() -> Result<(), String> {
        let mut w = Writer::new(Vec::new());
        set_header(w.header_mut());
        let n0 = w.get_ref().len();
        if n0 != 0 {
            return Err(format!("buffer size = {n0} before writes; want 0"));
        }
        w.flush().map_err(|err| err.to_string())?;
        let n1 = w.get_ref().len();
        if n1 == 0 {
            return Err("no data after first flush".to_owned());
        }
        put(&mut w, b"x")?;
        let n2 = w.get_ref().len();
        if n1 != n2 {
            return Err(format!(
                "after writing a single byte, size changed from {n1} to {n2}; want no change"
            ));
        }
        w.flush().map_err(|err| err.to_string())?;
        let n3 = w.get_ref().len();
        if n2 == n3 {
            return Err("Flush didn't flush any data".to_owned());
        }
        w.close().map_err(|err| err.to_string())
    }

    /// Go: `TestConcat`: multiple gzip files concatenated form a valid gzip file.
    #[test]
    fn concat() -> Result<(), String> {
        let mut w = Writer::new(Vec::new());
        put(&mut w, b"hello ")?;
        w.close().map_err(|err| err.to_string())?;
        let mut w = Writer::new(w.into_inner());
        put(&mut w, b"world\n")?;
        w.close().map_err(|err| err.to_string())?;
        let buf = w.into_inner();
        let mut r = Reader::new(buf.as_slice()).map_err(|err| err.to_string())?;
        let data = read_all(&mut r)?;
        if data != b"hello world\n" {
            return Err(format!("ReadAll = {data:?}, want \"hello world\\n\""));
        }
        Ok(())
    }

    /// Go: `TestWriterReset`.
    #[test]
    fn writer_reset() -> Result<(), String> {
        let msg = b"hello world";
        let mut z = Writer::new(Vec::new());
        put(&mut z, msg)?;
        z.close().map_err(|err| err.to_string())?;
        let buf = z.reset(Vec::new());
        put(&mut z, msg)?;
        z.close().map_err(|err| err.to_string())?;
        if *z.get_ref() != buf {
            return Err(format!("buf2 {:?} != original buf of {buf:?}", z.get_ref()));
        }
        Ok(())
    }

    /// Go: `limitedWriter`: accepts `N` more bytes, then fails with `io.ErrShortWrite`. Go
    /// returns the short count with the error, which the gzip writer drops; a Rust writer
    /// returns the error alone.
    #[derive(Debug)]
    struct LimitedWriter(usize);

    impl Write for LimitedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.0 < buf.len() {
                self.0 = 0;
                return Err(io::Error::new(io::ErrorKind::WriteZero, "short write"));
            }
            self.0 -= buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Go: `TestLimitedWrite`: `Write` never returns more bytes than the input slice. (Go's
    /// `time.Now()` modification time is a fixed time here.)
    #[test]
    fn limited_write() -> Result<(), String> {
        let msg = b"a";
        for lim in 2..20 {
            let mut z = Writer::new(LimitedWriter(lim));
            if let Ok(n) = z.write(msg)
                && n > msg.len()
            {
                return Err(format!("Write() = {n}, want {} or less", msg.len()));
            }
            let _old = z.reset(LimitedWriter(lim));
            *z.header_mut() = Header {
                comment: "comment".to_owned(),
                extra: Some(b"extra".to_vec()),
                mod_time: Time::from_unix(1_700_000_000, 0),
                name: "name".to_owned(),
                os: 1,
            };
            if let Ok(n) = z.write(msg)
                && n > msg.len()
            {
                return Err(format!("Write() = {n}, want {} or less", msg.len()));
            }
        }
        Ok(())
    }
}
