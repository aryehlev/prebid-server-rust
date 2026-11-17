//! Port of freecache `ringbuf.go`: the fixed-size ring buffer that stores a segment's entries.

use std::{borrow::Cow, cmp::Ordering, fmt};

use super::{
    convert::{copy, copy_within, index, span, span_mut, wide},
    segment::CacheError,
};

/// Go: `RingBuf`.
///
/// A ring buffer of fixed size over a data stream. When the stream outgrows the buffer, new data
/// overwrites the oldest; the buffer then holds the stream from [`RingBuf::begin`] to
/// [`RingBuf::end`].
#[derive(Clone, PartialEq, Eq)]
pub struct RingBuf {
    /// Go: `begin`, the beginning offset of the data stream.
    begin: i64,
    /// Go: `end`, the ending offset of the data stream.
    end: i64,
    /// Go: `data`.
    data: Vec<u8>,
    /// Go: `index`, the write position in `data`, from `0` to `len(data)-1`.
    index: i64,
}

impl RingBuf {
    /// Go: `NewRingBuf(size, begin)`.
    #[must_use]
    pub fn new(size: usize, begin: i64) -> Self {
        Self {
            begin,
            end: begin,
            data: vec![0; size],
            index: 0,
        }
    }

    /// Go: `Reset`. Restarts the stream at `begin`; the buffer bytes are left as they are.
    pub const fn reset(&mut self, begin: i64) {
        self.begin = begin;
        self.end = begin;
        self.index = 0;
    }

    /// Go: `Dump`, a copy of the whole buffer.
    #[must_use]
    pub fn dump(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// Go: `Size`, the buffer length.
    #[must_use]
    pub fn size(&self) -> i64 {
        wide(self.data.len())
    }

    /// Go: `Begin`.
    #[must_use]
    pub const fn begin(&self) -> i64 {
        self.begin
    }

    /// Go: `End`.
    #[must_use]
    pub const fn end(&self) -> i64 {
        self.end
    }

    /// Go: `ReadAt`, reading up to `p.len()` bytes at stream offset `off`.
    ///
    /// Returns Go's `(n, err)`: [`CacheError::OutOfRange`] when `off` is outside the stream, and
    /// [`CacheError::Eof`] when fewer than `p.len()` bytes remain before the end.
    pub fn read_at(&self, p: &mut [u8], off: i64) -> (usize, Option<CacheError>) {
        if off > self.end || off < self.begin {
            return (0, Some(CacheError::OutOfRange));
        }
        let size = self.size();
        let read_off = self.get_data_off(off);
        let read_end = read_off + (self.end - off);
        let mut n;
        if read_end <= size {
            n = copy(p, span(&self.data, read_off, read_end));
        } else {
            n = copy(p, span(&self.data, read_off, size));
            if n < p.len() {
                n += copy(
                    p.get_mut(n..).unwrap_or_default(),
                    span(&self.data, 0, read_end - size),
                );
            }
        }
        if n < p.len() {
            return (n, Some(CacheError::Eof));
        }
        (n, None)
    }

    /// Go: `getDataOff`, the buffer position of stream offset `off`.
    fn get_data_off(&self, off: i64) -> i64 {
        let size = self.size();
        let mut data_off = if self.end - self.begin < size {
            off - self.begin
        } else {
            self.index + (off - self.begin)
        };
        if data_off >= size {
            data_off -= size;
        }
        data_off
    }

    /// Go: `Slice`, `length` bytes at stream offset `off`, borrowed unless they wrap around the
    /// end of the buffer.
    ///
    /// Like Go, it only checks that `off` is inside the stream, not that `off + length` is.
    ///
    /// # Errors
    ///
    /// [`CacheError::OutOfRange`] when `off` is outside the stream, [`CacheError::Eof`] when the
    /// wrapped range is longer than the buffer. A negative `length`, on which Go panics, is
    /// [`CacheError::OutOfRange`].
    pub fn slice(&self, off: i64, length: i64) -> Result<Cow<'_, [u8]>, CacheError> {
        if off > self.end || off < self.begin {
            return Err(CacheError::OutOfRange);
        }
        let Some(len) = index(length) else {
            return Err(CacheError::OutOfRange);
        };
        let size = self.size();
        let read_off = self.get_data_off(off);
        let read_end = read_off + length;
        if read_end <= size {
            return Ok(Cow::Borrowed(span(&self.data, read_off, read_end)));
        }
        let mut buf = vec![0; len];
        let mut n = copy(&mut buf, span(&self.data, read_off, size));
        if n < len {
            n += copy(
                buf.get_mut(n..).unwrap_or_default(),
                span(&self.data, 0, read_end - size),
            );
        }
        if n < len {
            return Err(CacheError::Eof);
        }
        Ok(Cow::Owned(buf))
    }

    /// Go: `Write`, appending `p` to the stream.
    ///
    /// Returns Go's `(n, err)`: [`CacheError::OutOfRange`] when `p` is longer than the buffer.
    pub fn write(&mut self, p: &[u8]) -> (usize, Option<CacheError>) {
        if p.len() > self.data.len() {
            return (0, Some(CacheError::OutOfRange));
        }
        let size = self.size();
        let mut n = 0;
        while n < p.len() {
            let written = copy(
                span_mut(&mut self.data, self.index, size),
                p.get(n..).unwrap_or_default(),
            );
            if written == 0 {
                // Unreachable: `index` is always inside the buffer. Go would spin forever.
                break;
            }
            self.end += wide(written);
            n += written;
            self.index += wide(written);
            if self.index >= size {
                self.index -= size;
            }
        }
        if self.end - self.begin > size {
            self.begin = self.end - size;
        }
        (n, None)
    }

    /// Go: `WriteAt`, overwriting stream bytes at `off`.
    ///
    /// Returns Go's `(n, err)`: [`CacheError::OutOfRange`] when `p` does not fit between `off`
    /// and the end of the stream.
    pub fn write_at(&mut self, p: &[u8], off: i64) -> (usize, Option<CacheError>) {
        if off + wide(p.len()) > self.end || off < self.begin {
            return (0, Some(CacheError::OutOfRange));
        }
        let size = self.size();
        let write_off = self.get_data_off(off);
        let write_end = write_off + (self.end - off);
        let mut n;
        if write_end <= size {
            n = copy(span_mut(&mut self.data, write_off, write_end), p);
        } else {
            n = copy(span_mut(&mut self.data, write_off, size), p);
            if n < p.len() {
                n += copy(
                    span_mut(&mut self.data, 0, write_end - size),
                    p.get(n..).unwrap_or_default(),
                );
            }
        }
        (n, None)
    }

    /// Go: `EqualAt`, whether the stream holds `p` at `off`.
    #[must_use]
    pub fn equal_at(&self, p: &[u8], off: i64) -> bool {
        if off + wide(p.len()) > self.end || off < self.begin {
            return false;
        }
        let size = self.size();
        let read_off = self.get_data_off(off);
        let read_end = read_off + wide(p.len());
        if read_end <= size {
            return p == span(&self.data, read_off, read_end);
        }
        let Some((first, second)) = index(size - read_off).and_then(|n| p.split_at_checked(n))
        else {
            return false;
        };
        first == span(&self.data, read_off, size)
            && second == span(&self.data, 0, wide(second.len()))
    }

    /// Go: `Evacuate`, which copies the `length` bytes at `off` to the end of the stream so that
    /// new data does not overwrite them, and returns their new offset, or `-1` when the range
    /// is outside the stream.
    pub fn evacuate(&mut self, off: i64, length: i64) -> i64 {
        if off + length > self.end || off < self.begin {
            return -1;
        }
        let size = self.size();
        let read_off = self.get_data_off(off);
        match read_off.cmp(&self.index) {
            Ordering::Equal => {
                // no copy evacuate
                self.index += length;
                if self.index >= size {
                    self.index -= size;
                }
            }
            Ordering::Less => {
                let n = copy_within(&mut self.data, read_off, read_off + length, self.index);
                self.index += n;
                if self.index == size {
                    self.index = copy_within(&mut self.data, read_off + n, read_off + length, 0);
                }
            }
            Ordering::Greater => {
                let read_end = read_off + length;
                if read_end <= size {
                    let n = copy_within(&mut self.data, read_off, read_end, self.index);
                    self.index += n;
                } else {
                    let mut n = copy_within(&mut self.data, read_off, size, self.index);
                    self.index += n;
                    let tail = length - n;
                    n = copy_within(&mut self.data, 0, tail, self.index);
                    self.index += n;
                    if self.index == size {
                        self.index = copy_within(&mut self.data, n, tail, 0);
                    }
                }
            }
        }
        let new_off = self.end;
        self.end += length;
        if self.begin < self.end - size {
            self.begin = self.end - size;
        }
        new_off
    }

    /// Go: `Resize`, which reallocates the buffer, keeping the newest bytes of the stream.
    pub fn resize(&mut self, new_size: usize) {
        if self.data.len() == new_size {
            return;
        }
        let new_size_wide = wide(new_size);
        let mut new_data = vec![0; new_size];
        let mut offset = if self.end - self.begin == self.size() {
            self.index
        } else {
            0
        };
        if self.end - self.begin > new_size_wide {
            let discard = self.end - self.begin - new_size_wide;
            offset = (offset + discard).checked_rem(self.size()).unwrap_or(0);
            self.begin = self.end - new_size_wide;
        }
        let n = copy(&mut new_data, span(&self.data, offset, self.size()));
        if n < new_size {
            let _ = copy(
                new_data.get_mut(n..).unwrap_or_default(),
                span(&self.data, 0, offset),
            );
        }
        self.data = new_data;
        self.index = 0;
    }

    /// Go: `Skip`, advancing the end of the stream by `length` bytes without writing them.
    pub fn skip(&mut self, length: i64) {
        let size = self.size();
        self.end += length;
        self.index += length;
        // Go loops forever on an empty buffer; the guard only differs there.
        while size > 0 && self.index >= size {
            self.index -= size;
        }
        if self.end - self.begin > size {
            self.begin = self.end - size;
        }
    }

    /// The whole buffer, for the white-box parity digests.
    #[cfg(test)]
    pub(super) fn data(&self) -> &[u8] {
        &self.data
    }

    /// Go: the `index` field, for the white-box parity digests.
    #[cfg(test)]
    pub(super) const fn write_index(&self) -> i64 {
        self.index
    }
}

/// Go: `String`, `[size:%v, start:%v, end:%v, index:%v]`.
impl fmt::Display for RingBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[size:{}, start:{}, end:{}, index:{}]",
            self.data.len(),
            self.begin,
            self.end,
            self.index
        )
    }
}

impl fmt::Debug for RingBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RingBuf")
            .field("size", &self.data.len())
            .field("begin", &self.begin)
            .field("end", &self.end)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}
