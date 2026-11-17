//! Go: `compress/flate/dict_decoder.go`, the LZ77 sliding window of the decompressor.

use core::{mem, ops::Range};

/// Go: `dictDecoder`. Invariant: `rd_pos <= wr_pos <= hist.len()`.
#[derive(Clone, Debug, Default)]
pub(super) struct DictDecoder {
    /// Go: sliding window history.
    hist: Vec<u8>,
    /// Go: current output position in buffer.
    wr_pos: usize,
    /// Go: have emitted `hist[:rdPos]` already.
    rd_pos: usize,
    /// Go: has a full window length been written yet?
    full: bool,
}

impl DictDecoder {
    /// Go: `dictDecoder.init`: a window of `size` bytes holding the end of `dict`.
    pub(super) fn init(&mut self, size: usize, dict: &[u8]) {
        // Go: keeps the buffer, and its stale bytes, which are never read back.
        let mut hist = mem::take(&mut self.hist);
        hist.resize(size, 0);
        *self = Self {
            hist,
            ..Self::default()
        };

        let dict = dict
            .get(dict.len().saturating_sub(self.hist.len())..)
            .unwrap_or_default();
        if let Some(dst) = self.hist.get_mut(..dict.len()) {
            dst.copy_from_slice(dict);
        }
        self.wr_pos = dict.len();
        if self.wr_pos == self.hist.len() {
            self.wr_pos = 0;
            self.full = true;
        }
        self.rd_pos = self.wr_pos;
    }

    /// Go: `dictDecoder.histSize`: the amount of history in the window.
    pub(super) const fn hist_size(&self) -> usize {
        if self.full {
            self.hist.len()
        } else {
            self.wr_pos
        }
    }

    /// Go: `dictDecoder.availRead`: the bytes `read_flush` would return.
    pub(super) const fn avail_read(&self) -> usize {
        self.wr_pos - self.rd_pos
    }

    /// Go: `dictDecoder.availWrite`: the free space in the window.
    pub(super) const fn avail_write(&self) -> usize {
        self.hist.len() - self.wr_pos
    }

    /// Go: `dictDecoder.writeSlice`: the free space to write data to.
    pub(super) fn write_slice(&mut self) -> &mut [u8] {
        self.hist.get_mut(self.wr_pos..).unwrap_or_default()
    }

    /// Go: `dictDecoder.writeMark`: advances the write position by `cnt <= avail_write()`.
    pub(super) const fn write_mark(&mut self, cnt: usize) {
        self.wr_pos += cnt;
    }

    /// Go: `dictDecoder.writeByte`; `avail_write()` must be positive.
    pub(super) fn write_byte(&mut self, c: u8) {
        if let Some(slot) = self.hist.get_mut(self.wr_pos) {
            *slot = c;
        }
        self.wr_pos += 1;
    }

    /// Go: `dictDecoder.writeCopy`: copies `length` bytes from `dist` back (`0 < dist <=
    /// hist_size()`), as far as the window has room. Returns the number of bytes copied.
    pub(super) fn write_copy(&mut self, dist: usize, length: usize) -> usize {
        let dst_base = self.wr_pos;
        let mut dst_pos = dst_base;
        let end_pos = (dst_pos + length).min(self.hist.len());

        // Go: copy the non-overlapping section after the destination position, a distance
        // that wraps around the buffer.
        let src_pos = if dst_pos < dist {
            let src_pos = dst_pos + self.hist.len() - dist;
            let n = (end_pos - dst_pos).min(self.hist.len() - src_pos);
            self.hist.copy_within(src_pos..src_pos + n, dst_pos);
            dst_pos += n;
            0
        } else {
            dst_pos - dist
        };

        // Go: copy the possibly overlapping section before the destination position, a forward
        // copy that may repeat the bytes it has just written.
        dst_pos = self.forward_copy(src_pos, dst_pos, end_pos);
        self.wr_pos = dst_pos;
        dst_pos - dst_base
    }

    /// Go: `for dstPos < endPos { dstPos += copy(hist[dstPos:endPos], hist[srcPos:dstPos]) }`.
    fn forward_copy(&mut self, src_pos: usize, mut dst_pos: usize, end_pos: usize) -> usize {
        while dst_pos < end_pos {
            let n = (end_pos - dst_pos).min(dst_pos - src_pos);
            if n == 0 {
                // Go would loop forever; dist > 0 makes this unreachable.
                break;
            }
            self.hist.copy_within(src_pos..src_pos + n, dst_pos);
            dst_pos += n;
        }
        dst_pos
    }

    /// Go: `dictDecoder.tryWriteCopy`: [`DictDecoder::write_copy`] for the common case where
    /// the source does not wrap and the window has room; returns 0 otherwise.
    pub(super) fn try_write_copy(&mut self, dist: usize, length: usize) -> usize {
        let dst_pos = self.wr_pos;
        let end_pos = dst_pos + length;
        if dst_pos < dist || end_pos > self.hist.len() {
            return 0;
        }
        let dst_base = dst_pos;
        let src_pos = dst_pos - dist;

        // Go: copy possibly overlapping section before destination position.
        let dst_pos = self.forward_copy(src_pos, dst_pos, end_pos);
        self.wr_pos = dst_pos;
        dst_pos - dst_base
    }

    /// Go: `dictDecoder.readFlush`: the range of `hist` ready to be emitted, which must be
    /// consumed before the next call of any other method.
    pub(super) const fn read_flush(&mut self) -> Range<usize> {
        let to_read = self.rd_pos..self.wr_pos;
        self.rd_pos = self.wr_pos;
        if self.wr_pos == self.hist.len() {
            self.wr_pos = 0;
            self.rd_pos = 0;
            self.full = true;
        }
        to_read
    }

    /// The window bytes in `range` (a [`DictDecoder::read_flush`] result).
    pub(super) fn bytes(&self, range: Range<usize>) -> &[u8] {
        self.hist.get(range).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::DictDecoder;

    /// Go: `poem`.
    const POEM: &str = concat!(
        "The Road Not Taken\nRobert Frost\n",
        "\n",
        "Two roads diverged in a yellow wood,\n",
        "And sorry I could not travel both\n",
        "And be one traveler, long I stood\n",
        "And looked down one as far as I could\n",
        "To where it bent in the undergrowth;\n",
        "\n",
        "Then took the other, as just as fair,\n",
        "And having perhaps the better claim,\n",
        "Because it was grassy and wanted wear;\n",
        "Though as for that the passing there\n",
        "Had worn them really about the same,\n",
        "\n",
        "And both that morning equally lay\n",
        "In leaves no step had trodden black.\n",
        "Oh, I kept the first for another day!\n",
        "Yet knowing how way leads on to way,\n",
        "I doubted if I should ever come back.\n",
        "\n",
        "I shall be telling this with a sigh\n",
        "Somewhere ages and ages hence:\n",
        "Two roads diverged in a wood, and I-\n",
        "I took the one less traveled by,\n",
        "And that has made all the difference.\n",
    );

    /// Go: `poemRefs`: (backward distance, or 0 for an insertion; length).
    const POEM_REFS: &[(usize, usize)] = &[
        (0, 38),
        (33, 3),
        (0, 48),
        (79, 3),
        (0, 11),
        (34, 5),
        (0, 6),
        (23, 7),
        (0, 8),
        (50, 3),
        (0, 2),
        (69, 3),
        (34, 5),
        (0, 4),
        (97, 3),
        (0, 4),
        (43, 5),
        (0, 6),
        (7, 4),
        (88, 7),
        (0, 12),
        (80, 3),
        (0, 2),
        (141, 4),
        (0, 1),
        (196, 3),
        (0, 3),
        (157, 3),
        (0, 6),
        (181, 3),
        (0, 2),
        (23, 3),
        (77, 3),
        (28, 5),
        (128, 3),
        (110, 4),
        (70, 3),
        (0, 4),
        (85, 6),
        (0, 2),
        (182, 6),
        (0, 4),
        (133, 3),
        (0, 7),
        (47, 5),
        (0, 20),
        (112, 5),
        (0, 1),
        (58, 3),
        (0, 8),
        (59, 3),
        (0, 4),
        (173, 3),
        (0, 5),
        (114, 3),
        (0, 4),
        (92, 5),
        (0, 2),
        (71, 3),
        (0, 2),
        (76, 5),
        (0, 1),
        (46, 3),
        (96, 4),
        (130, 4),
        (0, 3),
        (360, 3),
        (0, 3),
        (178, 5),
        (0, 7),
        (75, 3),
        (0, 3),
        (45, 6),
        (0, 6),
        (299, 6),
        (180, 3),
        (70, 6),
        (0, 1),
        (48, 3),
        (66, 4),
        (0, 3),
        (47, 5),
        (0, 9),
        (325, 3),
        (0, 1),
        (359, 3),
        (318, 3),
        (0, 2),
        (199, 3),
        (0, 1),
        (344, 3),
        (0, 3),
        (248, 3),
        (0, 10),
        (310, 3),
        (0, 3),
        (93, 6),
        (0, 3),
        (252, 3),
        (157, 4),
        (0, 2),
        (273, 5),
        (0, 14),
        (99, 4),
        (0, 1),
        (464, 4),
        (0, 2),
        (92, 4),
        (495, 3),
        (0, 1),
        (322, 4),
        (16, 4),
        (0, 3),
        (402, 3),
        (0, 2),
        (237, 4),
        (0, 2),
        (432, 4),
        (0, 1),
        (483, 5),
        (0, 2),
        (294, 4),
        (0, 2),
        (306, 3),
        (113, 5),
        (0, 1),
        (26, 4),
        (164, 3),
        (488, 4),
        (0, 1),
        (542, 3),
        (248, 6),
        (0, 5),
        (205, 3),
        (0, 8),
        (48, 3),
        (449, 6),
        (0, 2),
        (192, 3),
        (328, 4),
        (9, 5),
        (433, 3),
        (0, 3),
        (622, 25),
        (615, 5),
        (46, 5),
        (0, 2),
        (104, 3),
        (475, 10),
        (549, 3),
        (0, 4),
        (597, 8),
        (314, 3),
        (0, 1),
        (473, 6),
        (317, 5),
        (0, 1),
        (400, 3),
        (0, 3),
        (109, 3),
        (151, 3),
        (48, 4),
        (0, 4),
        (125, 3),
        (108, 3),
        (0, 2),
    ];

    /// Go: `TestDictDecoder`'s `writeCopy` and `writeString` helpers.
    struct Harness {
        dd: DictDecoder,
        got: Vec<u8>,
    }

    impl Harness {
        fn flush_if_full(&mut self) {
            if self.dd.avail_write() == 0 {
                let range = self.dd.read_flush();
                self.got.extend_from_slice(self.dd.bytes(range));
            }
        }

        fn write_copy(&mut self, dist: usize, mut length: usize) -> Result<(), String> {
            while length > 0 {
                let mut cnt = self.dd.try_write_copy(dist, length);
                if cnt == 0 {
                    cnt = self.dd.write_copy(dist, length);
                }
                if cnt == 0 {
                    return Err(format!("no progress copying {length} bytes at {dist}"));
                }
                length -= cnt;
                self.flush_if_full();
            }
            Ok(())
        }

        fn write_string(&mut self, mut s: &[u8]) {
            while !s.is_empty() {
                let slice = self.dd.write_slice();
                let cnt = slice.len().min(s.len());
                if let (Some(dst), Some(src)) = (slice.get_mut(..cnt), s.get(..cnt)) {
                    dst.copy_from_slice(src);
                }
                s = s.get(cnt..).unwrap_or_default();
                self.dd.write_mark(cnt);
                self.flush_if_full();
            }
        }
    }

    /// Go: `compress/flate` `TestDictDecoder`.
    #[test]
    fn dict_decoder() -> Result<(), String> {
        const ABC: &str = "ABC\n";
        const FOX: &str = "The quick brown fox jumped over the lazy dog!\n";
        let mut h = Harness {
            dd: DictDecoder::default(),
            got: Vec::new(),
        };
        h.dd.init(1 << 11, &[]);
        let mut want: Vec<u8> = Vec::new();

        h.write_string(b".");
        want.push(b'.');

        let mut s = POEM.as_bytes();
        for &(dist, length) in POEM_REFS {
            if dist == 0 {
                h.write_string(s.get(..length).ok_or("poem too short")?);
            } else {
                h.write_copy(dist, length)?;
            }
            s = s.get(length..).ok_or("poem too short")?;
        }
        want.extend_from_slice(POEM.as_bytes());

        h.write_copy(h.dd.hist_size(), 33)?;
        want.extend_from_within(..33);

        h.write_string(ABC.as_bytes());
        h.write_copy(ABC.len(), 59 * ABC.len())?;
        want.extend_from_slice(ABC.repeat(60).as_bytes());

        h.write_string(FOX.as_bytes());
        h.write_copy(FOX.len(), 9 * FOX.len())?;
        want.extend_from_slice(FOX.repeat(10).as_bytes());

        h.write_string(b".");
        h.write_copy(1, 9)?;
        want.extend_from_slice(".".repeat(10).as_bytes());

        let upper = POEM.to_uppercase();
        h.write_string(upper.as_bytes());
        h.write_copy(POEM.len(), 7 * POEM.len())?;
        want.extend_from_slice(upper.repeat(8).as_bytes());

        let hist_size = h.dd.hist_size();
        h.write_copy(hist_size, 10)?;
        let start = want.len() - hist_size;
        want.extend_from_within(start..start + 10);

        let range = h.dd.read_flush();
        h.got.extend_from_slice(h.dd.bytes(range));
        if h.got != want {
            return Err(format!(
                "final string mismatch:\ngot  {:?}\nwant {:?}",
                String::from_utf8_lossy(&h.got),
                String::from_utf8_lossy(&want)
            ));
        }
        Ok(())
    }
}
