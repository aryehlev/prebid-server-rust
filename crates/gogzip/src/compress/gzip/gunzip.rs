//! Go: `compress/gzip/gunzip.go`: the [`Reader`] and the [`Header`].

use core::fmt;
use std::io::BufRead;

use pbs_gocompat::time::Time;

use super::{
    super::{
        flate::Decompressor,
        goio::{self, no_eof},
    },
    low32,
};
use crate::{GzipError, crc32};

/// Go: `gzipID1`.
pub(super) const GZIP_ID1: u8 = 0x1F;
/// Go: `gzipID2`.
pub(super) const GZIP_ID2: u8 = 0x8B;
/// Go: `gzipDeflate`, the DEFLATE compression method.
pub(super) const GZIP_DEFLATE: u8 = 8;
/// Go: `flagHdrCrc`.
const FLAG_HDR_CRC: u8 = 1 << 1;
/// Go: `flagExtra`.
pub(super) const FLAG_EXTRA: u8 = 1 << 2;
/// Go: `flagName`.
pub(super) const FLAG_NAME: u8 = 1 << 3;
/// Go: `flagComment`.
pub(super) const FLAG_COMMENT: u8 = 1 << 4;

/// Go: `gzip.Header`, the metadata of a gzip member (RFC 1952 2.3.1).
///
/// Strings are UTF-8 in Rust and Latin-1 in the file: the writer converts and rejects runes
/// above U+00FF and NUL ([`GzipError::NonLatin1`]), and the reader converts Latin-1 to UTF-8,
/// as Go does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    /// Go: `Comment`.
    pub comment: String,
    /// Go: `Extra`, the "extra data". `Some(vec![])` is written as an empty extra field, while
    /// `None` (Go's nil) writes none.
    pub extra: Option<Vec<u8>>,
    /// Go: `ModTime`, the modification time; written only when after the Unix epoch, and
    /// written as the low 32 bits of its Unix seconds.
    pub mod_time: Time,
    /// Go: `Name`, the file name.
    pub name: String,
    /// Go: `OS`, the operating system type (a writer starts at 255, unknown).
    pub os: u8,
}

/// Go: `gzip.Reader`: reads the decompressed data of a gzip stream, with Go's results and error
/// texts.
///
/// By default the reader reads a series of concatenated gzip members as one stream (Go's
/// multistream mode), checking each member's CRC-32 and size; see [`Reader::multistream`].
/// Go wraps a reader that has no `ReadByte` in a `bufio.Reader`; this port reads a
/// [`BufRead`], so the caller picks the buffering.
pub struct Reader<R> {
    /// Go: the embedded `Header`, valid after [`Reader::new`] or [`Reader::reset`].
    header: Header,
    r: R,
    decompressor: Option<Box<Decompressor>>,
    /// Go: CRC-32, IEEE polynomial (section 8).
    digest: u32,
    /// Go: uncompressed size (section 2.3.1).
    size: u32,
    buf: [u8; 512],
    err: Option<GzipError>,
    multistream: bool,
}

impl<R> fmt::Debug for Reader<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("header", &self.header)
            .field("decompressor", &self.decompressor)
            .field("err", &self.err)
            .field("multistream", &self.multistream)
            .finish_non_exhaustive()
    }
}

impl<R: BufRead> Reader<R> {
    /// Go: `gzip.NewReader(r)`: reads the header of the first member.
    ///
    /// # Errors
    ///
    /// [`GzipError::Eof`] for an empty input, [`GzipError::UnexpectedEof`] for a truncated
    /// header, [`GzipError::Header`] for one that is not gzip, or the reader's error.
    pub fn new(r: R) -> Result<Self, GzipError> {
        let mut z = Self {
            header: Header::default(),
            r,
            decompressor: None,
            digest: 0,
            size: 0,
            buf: [0; 512],
            err: None,
            multistream: true,
        };
        let (header, err) = z.read_header();
        z.header = header;
        err.map_or(Ok(z), Err)
    }

    /// Go: `Reader.Reset(r)`: the state of [`Reader::new`] with `r`, keeping the
    /// decompressor. On an error the header holds the fields read before it, as in Go.
    ///
    /// # Errors
    ///
    /// As [`Reader::new`]; the error is also returned by later reads.
    pub fn reset(&mut self, r: R) -> Result<(), GzipError> {
        self.r = r;
        self.digest = 0;
        self.size = 0;
        self.buf = [0; 512];
        self.multistream = true;
        let (header, err) = self.read_header();
        self.header = header;
        self.err.clone_from(&err);
        err.map_or(Ok(()), Err)
    }

    /// Go: `Reader.Multistream(ok)`: with `false`, the reader stops at the end of the first
    /// member and returns [`GzipError::Eof`], leaving the input after it unread (with a
    /// [`BufRead`] input, which the decompressor never reads past the member).
    pub const fn multistream(&mut self, ok: bool) {
        self.multistream = ok;
    }

    /// Go: the embedded `Header` fields of the first member.
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Go: `Reader.readString`: a NUL-terminated Latin-1 string of at most 511 bytes, as
    /// UTF-8. Updates the digest.
    fn read_string(&mut self) -> Result<String, GzipError> {
        for i in 0..self.buf.len() {
            let c = goio::read_byte(&mut self.r)?;
            if let Some(slot) = self.buf.get_mut(i) {
                *slot = c;
            }
            if c == 0 {
                // Go: digest covers the NUL terminator.
                self.digest = crc32::update(self.digest, self.buf.get(..=i).unwrap_or_default());
                // Go: strings are ISO 8859-1, Latin-1 (RFC 1952, section 2.3.1); each byte is
                // the rune of the same value (Go converts only when a byte is above 0x7f, with
                // the same result).
                let s = self.buf.get(..i).unwrap_or_default();
                return Ok(s.iter().copied().map(char::from).collect());
            }
        }
        Err(GzipError::Header)
    }

    /// Go: `Reader.readHeader`: reads a member header (RFC 1952 2.3.1) and resets the
    /// decompressor for its data. Returns the header read so far and the error, if any.
    fn read_header(&mut self) -> (Header, Option<GzipError>) {
        let mut hdr = Header::default();
        let err = self.read_header_into(&mut hdr).err();
        (hdr, err)
    }

    /// The body of Go's `readHeader`, filling `hdr`.
    fn read_header_into(&mut self, hdr: &mut Header) -> Result<(), GzipError> {
        let (_, head_err) =
            goio::read_full(&mut self.r, self.buf.get_mut(..10).unwrap_or_default());
        if let Some(err) = head_err {
            // Go: a gzip file is "zero or more" members, so returning io.EOF here is okay.
            return Err(err);
        }
        let [id1, id2, cm, flg, m0, m1, m2, m3, _xfl, os] = self.head10();
        if id1 != GZIP_ID1 || id2 != GZIP_ID2 || cm != GZIP_DEFLATE {
            return Err(GzipError::Header);
        }
        let t = u32::from_le_bytes([m0, m1, m2, m3]);
        if t > 0 {
            // Go: section 2.3.1, the zero value for MTIME means that the modified time is not
            // set.
            hdr.mod_time = Time::from_unix(i64::from(t), 0);
        }
        // Go: z.buf[8] is XFL and is currently ignored.
        hdr.os = os;
        self.digest = crc32::checksum_ieee(self.buf.get(..10).unwrap_or_default());

        if flg & FLAG_EXTRA != 0 {
            let mut len = [0; 2];
            if let (_, Some(err)) = goio::read_full(&mut self.r, &mut len) {
                return Err(no_eof(err));
            }
            self.digest = crc32::update(self.digest, &len);
            let mut data = vec![0; usize::from(u16::from_le_bytes(len))];
            if let (_, Some(err)) = goio::read_full(&mut self.r, &mut data) {
                return Err(no_eof(err));
            }
            self.digest = crc32::update(self.digest, &data);
            hdr.extra = Some(data);
        }

        if flg & FLAG_NAME != 0 {
            hdr.name = self.read_string().map_err(no_eof)?;
        }

        if flg & FLAG_COMMENT != 0 {
            hdr.comment = self.read_string().map_err(no_eof)?;
        }

        if flg & FLAG_HDR_CRC != 0 {
            let mut crc = [0; 2];
            if let (_, Some(err)) = goio::read_full(&mut self.r, &mut crc) {
                return Err(no_eof(err));
            }
            let [d0, d1, ..] = self.digest.to_le_bytes();
            if crc != [d0, d1] {
                return Err(GzipError::Header);
            }
        }

        self.digest = 0;
        match self.decompressor.as_mut() {
            Some(decompressor) => decompressor.reset(&[]),
            None => self.decompressor = Some(Box::new(Decompressor::new(&[]))),
        }
        Ok(())
    }

    /// The first 10 bytes of the buffer.
    fn head10(&self) -> [u8; 10] {
        let mut head = [0; 10];
        if let Some(src) = self.buf.get(..10) {
            head.copy_from_slice(src);
        }
        head
    }

    /// Go: `Reader.Read`: decompresses into `p` and returns Go's `(n, err)` pair.
    ///
    /// `err` is [`GzipError::Eof`] once the last member's trailer has been checked, possibly
    /// together with the last bytes; [`GzipError::Checksum`] when a trailer does not match.
    /// Errors are sticky.
    pub fn read(&mut self, p: &mut [u8]) -> (usize, Option<GzipError>) {
        if let Some(err) = &self.err {
            return (0, Some(err.clone()));
        }

        let mut n = 0;
        while n == 0 {
            let Some(decompressor) = self.decompressor.as_mut() else {
                // Go: a Reader always has a decompressor once NewReader succeeded.
                return (0, None);
            };
            let (nn, read_err) = decompressor.read(&mut self.r, p);
            n = nn;
            self.err = read_err;
            self.digest = crc32::update(self.digest, p.get(..n).unwrap_or_default());
            self.size = self.size.wrapping_add(low32(n));
            if !matches!(self.err, Some(GzipError::Eof)) {
                // Go: in the normal case we return here.
                return (n, self.err.clone());
            }

            // Go: finished file; check checksum and size.
            let mut trailer = [0; 8];
            if let (_, Some(err)) = goio::read_full(&mut self.r, &mut trailer) {
                self.err = Some(no_eof(err));
                return (n, self.err.clone());
            }
            let [d0, d1, d2, d3, s0, s1, s2, s3] = trailer;
            let digest = u32::from_le_bytes([d0, d1, d2, d3]);
            let size = u32::from_le_bytes([s0, s1, s2, s3]);
            if digest != self.digest || size != self.size {
                self.err = Some(GzipError::Checksum);
                return (n, self.err.clone());
            }
            self.digest = 0;
            self.size = 0;

            // Go: file is ok; check if there is another.
            if !self.multistream {
                return (n, Some(GzipError::Eof));
            }
            // Go: remove io.EOF.
            self.err = None;

            let (_, header_err) = self.read_header();
            if header_err.is_some() {
                self.err = header_err;
                return (n, self.err.clone());
            }
        }
        (n, None)
    }

    /// Go: `Reader.Close`: the decompression error of the current member, if any (end of
    /// stream is not an error). The underlying reader is not closed. The checksum is only
    /// verified by reading to [`GzipError::Eof`].
    ///
    /// # Errors
    ///
    /// The error that stopped decompression.
    pub fn close(&mut self) -> Result<(), GzipError> {
        self.decompressor
            .as_ref()
            .map_or(Ok(()), |decompressor| decompressor.close())
    }

    /// The underlying reader.
    pub const fn get_ref(&self) -> &R {
        &self.r
    }

    /// The underlying reader, mutably.
    pub const fn get_mut(&mut self) -> &mut R {
        &mut self.r
    }

    /// The underlying reader, discarding the decompressor.
    pub fn into_inner(self) -> R {
        self.r
    }
}

#[cfg(test)]
mod tests {
    //! Ports of Go's `gunzip_test.go` and of `gzip_test.go`'s `readString` check. The vectors of
    //! `TestDecompressor` (`gunzipTests`, through `NewReader`, `Reset` and `Multistream(false)`),
    //! `TestIssue6550`, `TestMultistreamFalse`, `TestNilStream` and `TestTruncatedStreams` are
    //! replayed with Go's results from the golden files.

    use super::{Header, Reader};
    use crate::GzipError;

    /// Go: `TestLatin1`'s `readString`: Latin-1 strings are read as UTF-8.
    #[test]
    fn latin1() -> Result<(), String> {
        let latin1: &[u8] = &[0xC4, b'u', 0xDF, b'e', b'r', b'u', b'n', b'g', 0];
        let mut z = Reader {
            header: Header::default(),
            r: latin1,
            decompressor: None,
            digest: 0,
            size: 0,
            buf: [0; 512],
            err: None,
            multistream: true,
        };
        let s = z
            .read_string()
            .map_err(|err| format!("readString: {err}"))?;
        if s != "Äußerung" {
            return Err(format!("read latin-1: got {s:?}, want \"Äußerung\""));
        }
        Ok(())
    }

    /// Go: `TestCVE202230631`: a long run of empty members is read in a loop rather than by
    /// recursion (Go's 4e6 members).
    #[test]
    fn cve_2022_30631() -> Result<(), String> {
        const EMPTY: [u8; 20] = [
            0x1F, 0x8B, 0x08, 0x00, 0xA7, 0x8F, 0x43, 0x62, 0x00, 0x03, 0x03, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let input = EMPTY.repeat(4_000_000);
        let mut z = Reader::new(input.as_slice())
            .map_err(|err| format!("NewReader: got {err}, want nil"))?;
        let (_, err) = z.read(&mut [0; 10]);
        if err != Some(GzipError::Eof) {
            return Err(format!("Reader.Read: got {err:?}, want EOF"));
        }
        Ok(())
    }
}
