//! The Go `io` behaviour the compressors depend on, over Rust's `std::io` traits.

use std::{
    io::{BufRead, ErrorKind, Read, Write},
    sync::Arc,
};

use crate::GzipError;

/// Go: `w.Write(b)` with the result Go code keeps (`_, err = w.Write(b)`): all of `b`, or the
/// error. An empty `b` is still one `Write` call, as in Go, so a writer that counts or fails
/// calls sees the same calls.
pub(super) fn write(w: &mut dyn Write, b: &[u8]) -> Result<(), GzipError> {
    if b.is_empty() {
        loop {
            match w.write(b) {
                Ok(_) => return Ok(()),
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                Err(err) => return Err(GzipError::Io(Arc::new(err))),
            }
        }
    }
    w.write_all(b).map_err(|err| GzipError::Io(Arc::new(err)))
}

/// Go: `io.ByteReader.ReadByte` on a buffered reader: the next byte, or [`GzipError::Eof`].
pub(super) fn read_byte<R: BufRead + ?Sized>(r: &mut R) -> Result<u8, GzipError> {
    loop {
        match r.fill_buf() {
            Ok(buf) => {
                let Some(&c) = buf.first() else {
                    return Err(GzipError::Eof);
                };
                r.consume(1);
                return Ok(c);
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(GzipError::Io(Arc::new(err))),
        }
    }
}

/// Go: `io.ReadFull(r, buf)`: the number of bytes read and, unless `buf` was filled, the error:
/// [`GzipError::Eof`] when nothing was read and [`GzipError::UnexpectedEof`] when the input
/// ended part way.
pub(super) fn read_full<R: Read + ?Sized>(r: &mut R, buf: &mut [u8]) -> (usize, Option<GzipError>) {
    let mut n = 0;
    while n < buf.len() {
        match r.read(buf.get_mut(n..).unwrap_or_default()) {
            Ok(0) => {
                let err = if n > 0 {
                    GzipError::UnexpectedEof
                } else {
                    GzipError::Eof
                };
                return (n, Some(err));
            }
            Ok(nn) => n += nn,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return (n, Some(GzipError::Io(Arc::new(err)))),
        }
    }
    (n, None)
}

/// Go: `noEOF`: [`GzipError::Eof`] becomes [`GzipError::UnexpectedEof`].
pub(super) fn no_eof(err: GzipError) -> GzipError {
    if err.is_eof() {
        GzipError::UnexpectedEof
    } else {
        err
    }
}

/// Go: `copy(dst, src)`: copies `min(dst.len(), src.len())` bytes and returns that count.
pub(super) fn go_copy(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
        d.copy_from_slice(s);
    }
    n
}
