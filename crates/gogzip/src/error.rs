//! The `error` values that Go's `compress/flate`, `compress/gzip` and `io` return.

use core::{error::Error, fmt};
use std::{io, sync::Arc};

/// An error from the compressors or decompressors, displayed exactly as Go's `err.Error()`.
///
/// Errors are sticky, as in Go: once a writer or reader fails, every later call returns a
/// clone of the same value. An [`Io`](GzipError::Io) error is shared through an [`Arc`] so that
/// the clones are the same error, and two of them compare equal only when they are (Go compares
/// error values with `==`).
#[derive(Clone, Debug)]
pub enum GzipError {
    /// Go: `io.EOF` (`EOF`). A reader returns it at the end of the data; `gzip.NewReader` returns
    /// it for an empty input.
    Eof,
    /// Go: `io.ErrUnexpectedEOF` (`unexpected EOF`): the input ends inside a stream.
    UnexpectedEof,
    /// Go: `flate.CorruptInputError` (`flate: corrupt input before offset N`), where `N` counts
    /// the input bytes the decompressor has read from the start of the DEFLATE stream.
    CorruptInput(i64),
    /// Go: `flate.InternalError` (`flate: internal error: ...`). Go returns it for
    /// `writeBytes with unfinished bits`; this port also returns it where Go would panic on a
    /// broken internal invariant, which cannot happen.
    Internal(&'static str),
    /// Go: the unexported `errWriterClosed` (`flate: closed writer`), returned by `Write` and
    /// `Flush` after `Close`.
    WriterClosed,
    /// Go: `flate.NewWriter`'s `flate: invalid compression level N: want value in range [-2, 9]`.
    InvalidFlateLevel(isize),
    /// Go: `gzip.NewWriterLevel`'s `gzip: invalid compression level: N`.
    InvalidGzipLevel(isize),
    /// Go: `gzip.ErrChecksum` (`gzip: invalid checksum`): the trailer's CRC-32 or size does not
    /// match the data.
    Checksum,
    /// Go: `gzip.ErrHeader` (`gzip: invalid header`).
    Header,
    /// Go: `gzip.Write: Extra data is too large` (an `Extra` field over 65535 bytes).
    ExtraTooLarge,
    /// Go: `gzip.Write: non-Latin-1 header string` (a `Name` or `Comment` with a NUL or a rune
    /// above U+00FF).
    NonLatin1,
    /// An error from the underlying reader or writer, returned unchanged (its text is the
    /// [`io::Error`]'s).
    Io(Arc<io::Error>),
}

impl GzipError {
    /// Reports whether this is [`GzipError::Eof`], Go's `err == io.EOF`.
    #[must_use]
    pub const fn is_eof(&self) -> bool {
        matches!(self, Self::Eof)
    }
}

impl fmt::Display for GzipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => f.write_str("EOF"),
            Self::UnexpectedEof => f.write_str("unexpected EOF"),
            // Go: inflate.go:35, "flate: corrupt input before offset " + strconv.FormatInt.
            Self::CorruptInput(offset) => write!(f, "flate: corrupt input before offset {offset}"),
            Self::Internal(msg) => write!(f, "flate: internal error: {msg}"),
            Self::WriterClosed => f.write_str("flate: closed writer"),
            // Go: deflate.go:597.
            Self::InvalidFlateLevel(level) => write!(
                f,
                "flate: invalid compression level {level}: want value in range [-2, 9]"
            ),
            // Go: gzip.go:62.
            Self::InvalidGzipLevel(level) => write!(f, "gzip: invalid compression level: {level}"),
            Self::Checksum => f.write_str("gzip: invalid checksum"),
            Self::Header => f.write_str("gzip: invalid header"),
            Self::ExtraTooLarge => f.write_str("gzip.Write: Extra data is too large"),
            Self::NonLatin1 => f.write_str("gzip.Write: non-Latin-1 header string"),
            Self::Io(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl Error for GzipError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(&**err),
            Self::Eof
            | Self::UnexpectedEof
            | Self::CorruptInput(_)
            | Self::Internal(_)
            | Self::WriterClosed
            | Self::InvalidFlateLevel(_)
            | Self::InvalidGzipLevel(_)
            | Self::Checksum
            | Self::Header
            | Self::ExtraTooLarge
            | Self::NonLatin1 => None,
        }
    }
}

impl PartialEq for GzipError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Eof, Self::Eof)
            | (Self::UnexpectedEof, Self::UnexpectedEof)
            | (Self::WriterClosed, Self::WriterClosed)
            | (Self::Checksum, Self::Checksum)
            | (Self::Header, Self::Header)
            | (Self::ExtraTooLarge, Self::ExtraTooLarge)
            | (Self::NonLatin1, Self::NonLatin1) => true,
            (Self::CorruptInput(a), Self::CorruptInput(b)) => a == b,
            (Self::Internal(a), Self::Internal(b)) => a == b,
            (Self::InvalidFlateLevel(a), Self::InvalidFlateLevel(b))
            | (Self::InvalidGzipLevel(a), Self::InvalidGzipLevel(b)) => a == b,
            (Self::Io(a), Self::Io(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for GzipError {}

impl From<io::Error> for GzipError {
    fn from(err: io::Error) -> Self {
        Self::Io(Arc::new(err))
    }
}
