//! Go: `compress/flate/deflatefast.go`, the Snappy-style encoder of level 1 (`BestSpeed`).

use super::{
    deflate::{MAX_MATCH_LENGTH, MAX_MATCH_OFFSET},
    huffman_bit_writer::MAX_STORE_BLOCK_SIZE,
    token::Token,
};

/// Go: `tableBits`, the bits used in the table.
const TABLE_BITS: u32 = 14;
/// Go: `tableSize`.
const TABLE_SIZE: usize = 1 << TABLE_BITS;
/// Go: `tableMask`.
const TABLE_MASK: usize = TABLE_SIZE - 1;
/// Go: `tableShift`.
const TABLE_SHIFT: u32 = 32 - TABLE_BITS;
/// Go: `maxStoreBlockSize` as the `int32` offsets use it.
const MAX_STORE_BLOCK_SIZE_32: i32 = 65535;
/// Go: `maxMatchOffset` as an `int32`.
const MAX_MATCH_OFFSET_32: i32 = 1 << 15;
/// Go: `bufferReset`: `e.cur` is reset before it reaches this, so that offsets stored as `int32`
/// never overflow.
const BUFFER_RESET: i32 = i32::MAX - MAX_STORE_BLOCK_SIZE_32 * 2;
/// Go: `inputMargin`.
const INPUT_MARGIN: usize = 16 - 1;
/// Go: `minNonLiteralBlockSize`.
const MIN_NON_LITERAL_BLOCK_SIZE: usize = 1 + 1 + INPUT_MARGIN;

const _: () = assert!(
    MAX_STORE_BLOCK_SIZE == 65535 && MAX_MATCH_OFFSET == 1 << 15,
    "the int32 copies of the window constants match them"
);

/// Go: `load32`.
fn load32(b: &[u8], i: usize) -> u32 {
    match b.get(i..i + 4) {
        Some(&[b0, b1, b2, b3]) => u32::from_le_bytes([b0, b1, b2, b3]),
        _ => 0,
    }
}

/// Go: `load64`.
fn load64(b: &[u8], i: usize) -> u64 {
    match b.get(i..i + 8) {
        Some(&[b0, b1, b2, b3, b4, b5, b6, b7]) => {
            u64::from_le_bytes([b0, b1, b2, b3, b4, b5, b6, b7])
        }
        _ => 0,
    }
}

/// Go: `uint32(x)`, the low 32 bits.
const fn low32(x: u64) -> u32 {
    let [b0, b1, b2, b3, _, _, _, _] = x.to_le_bytes();
    u32::from_le_bytes([b0, b1, b2, b3])
}

/// Go: `hash`.
const fn hash(u: u32) -> u32 {
    u.wrapping_mul(0x1E35_A7BD) >> TABLE_SHIFT
}

/// `x` as Go's `int32`, for positions in a block (below 65536).
fn int32(x: usize) -> i32 {
    i32::try_from(x).unwrap_or(i32::MAX)
}

/// Go: `tableEntry`.
#[derive(Clone, Copy, Debug, Default)]
struct TableEntry {
    /// Go: value at destination.
    val: u32,
    offset: i32,
}

/// Go: `deflateFast`: the match table and the previous block, for cross-block matching.
#[derive(Clone, Debug)]
pub(super) struct DeflateFast {
    table: Box<[TableEntry]>,
    /// Go: the previous block, zero length if unknown.
    prev: Vec<u8>,
    /// Go: the current match offset.
    cur: i32,
}

impl DeflateFast {
    /// Go: `newDeflateFast`.
    pub(super) fn new() -> Self {
        Self {
            table: vec![TableEntry::default(); TABLE_SIZE].into_boxed_slice(),
            prev: Vec::with_capacity(MAX_STORE_BLOCK_SIZE),
            cur: MAX_STORE_BLOCK_SIZE_32,
        }
    }

    fn entry(&self, h: u32) -> TableEntry {
        self.table
            .get(h as usize & TABLE_MASK)
            .copied()
            .unwrap_or_default()
    }

    fn set_entry(&mut self, h: u32, entry: TableEntry) {
        if let Some(slot) = self.table.get_mut(h as usize & TABLE_MASK) {
            *slot = entry;
        }
    }

    /// Go: `deflateFast.encode`: appends the tokens of the block `src` to `dst`.
    pub(super) fn encode(&mut self, dst: &mut Vec<Token>, src: &[u8]) {
        // Go: ensure that e.cur doesn't wrap.
        if self.cur >= BUFFER_RESET {
            self.shift_offsets();
        }

        // Go: this check isn't in the Snappy implementation, but there, the caller instead of
        // the callee handles this case.
        if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
            self.cur += MAX_STORE_BLOCK_SIZE_32;
            self.prev.clear();
            emit_literal(dst, src);
            return;
        }

        let next_emit = self.encode_matches(dst, src);
        if next_emit < src.len() {
            emit_literal(dst, src.get(next_emit..).unwrap_or_default());
        }
        self.cur += int32(src.len());
        self.prev.clear();
        self.prev.extend_from_slice(src);
    }

    /// The main loop of Go's `deflateFast.encode`, up to `emitRemainder`. Returns `nextEmit`.
    fn encode_matches(&mut self, dst: &mut Vec<Token>, src: &[u8]) -> usize {
        // Go: sLimit is when to stop looking for offset/length copies.
        let s_limit = src.len() - INPUT_MARGIN;
        // Go: nextEmit is where in src the next emitLiteral should start from.
        let mut next_emit = 0;
        let mut s = 0;
        let mut cv = load32(src, s);
        let mut next_hash = hash(cv);

        loop {
            // Go: heuristic match skipping: if 32 bytes are scanned with no matches found,
            // start looking only at every other byte, and so on.
            let mut skip = 32;
            let mut next_s = s;
            let mut candidate;
            loop {
                s = next_s;
                let bytes_between_hash_lookups = skip >> 5;
                next_s = s + bytes_between_hash_lookups;
                skip += bytes_between_hash_lookups;
                if next_s > s_limit {
                    return next_emit;
                }
                candidate = self.entry(next_hash);
                let now = load32(src, next_s);
                self.set_entry(
                    next_hash,
                    TableEntry {
                        offset: int32(s) + self.cur,
                        val: cv,
                    },
                );
                next_hash = hash(now);

                let distance = int32(s) - (candidate.offset - self.cur);
                if distance > MAX_MATCH_OFFSET_32 || cv != candidate.val {
                    // Go: out of range or not matched.
                    cv = now;
                    continue;
                }
                break;
            }

            // Go: a 4-byte match has been found; src[nextEmit:s] are unmatched.
            emit_literal(dst, src.get(next_emit..s).unwrap_or_default());

            // Go: call emitCopy, and then see if another emitCopy could be our next move.
            loop {
                // Go: invariant: we have a 4-byte match at s, and no need to emit any literal
                // bytes prior to s. Extend the 4-byte match as long as possible.
                s += 4;
                let t = candidate.offset - self.cur + 4;
                let l = self.match_len(s, t, src);

                // Go: matchToken is flate's equivalent of Snappy's emitCopy.
                let length = int32(l) + 4 - 3;
                let match_offset = int32(s) - t - 1;
                dst.push(Token::match_token(
                    length.cast_unsigned(),
                    match_offset.cast_unsigned(),
                ));
                s += l;
                next_emit = s;
                if s >= s_limit {
                    return next_emit;
                }

                // Go: update the hash table at s-1 and at s; if another emitCopy is not our next
                // move, also calculate nextHash at s+1.
                let mut x = load64(src, s - 1);
                let prev_hash = hash(low32(x));
                self.set_entry(
                    prev_hash,
                    TableEntry {
                        offset: self.cur + int32(s) - 1,
                        val: low32(x),
                    },
                );
                x >>= 8;
                let curr_hash = hash(low32(x));
                candidate = self.entry(curr_hash);
                self.set_entry(
                    curr_hash,
                    TableEntry {
                        offset: self.cur + int32(s),
                        val: low32(x),
                    },
                );

                let distance = int32(s) - (candidate.offset - self.cur);
                if distance > MAX_MATCH_OFFSET_32 || low32(x) != candidate.val {
                    cv = low32(x >> 8);
                    next_hash = hash(cv);
                    s += 1;
                    break;
                }
            }
        }
    }

    /// Go: `deflateFast.matchLen`: the match length between `src[s..]` and `src[t..]`, where a
    /// negative `t` starts the match in the previous block. `src[s-4..s]` and `src[t-4..t]`
    /// already match.
    pub(super) fn match_len(&self, s: usize, t: i32, src: &[u8]) -> usize {
        let s1 = (s + MAX_MATCH_LENGTH - 4).min(src.len());
        let current = src.get(s..s1).unwrap_or_default();

        // Go: if we are inside the current block.
        if let Ok(start) = usize::try_from(t) {
            return common_prefix(current, src.get(start..).unwrap_or_default());
        }

        // Go: we found a match in the previous block.
        let Ok(tp) = usize::try_from(int32(self.prev.len()) + t) else {
            return 0;
        };

        // Go: extend the match to be as long as possible.
        let mut previous = self.prev.get(tp..).unwrap_or_default();
        if previous.len() > current.len() {
            previous = previous.get(..current.len()).unwrap_or_default();
        }
        let head = current.get(..previous.len()).unwrap_or_default();
        let matched = common_prefix(head, previous);
        if matched < previous.len() {
            return matched;
        }

        // Go: if we reached our limit, we matched everything we are allowed to in the previous
        // block and we return.
        if s + matched == s1 {
            return matched;
        }

        // Go: continue looking for more matches in the current block.
        let rest = src.get(s + matched..s1).unwrap_or_default();
        common_prefix(rest, src) + matched
    }

    /// Go: `deflateFast.reset`: no match may refer to data before this point.
    pub(super) fn reset(&mut self) {
        self.prev.clear();
        // Go: bump the offset, so all matches will fail distance check.
        self.cur += MAX_MATCH_OFFSET_32;

        // Go: protect against e.cur wraparound.
        if self.cur >= BUFFER_RESET {
            self.shift_offsets();
        }
    }

    /// Go: `deflateFast.shiftOffsets`: shifts down all match offsets, to prevent integer
    /// overflow.
    fn shift_offsets(&mut self) {
        if self.prev.is_empty() {
            // Go: we have no history; just clear the table.
            self.table.fill(TableEntry::default());
            self.cur = MAX_MATCH_OFFSET_32 + 1;
            return;
        }

        // Go: shift down everything in the table that isn't already too far away.
        for entry in &mut self.table {
            let v = entry.offset - self.cur + MAX_MATCH_OFFSET_32 + 1;
            // Go: we ignore matches > maxMatchOffset, so negative offsets are capped at 0.
            entry.offset = v.max(0);
        }
        self.cur = MAX_MATCH_OFFSET_32 + 1;
    }
}

/// The number of leading bytes `a` and `b` have in common, at most `a.len()`, compared 8 bytes
/// at a time (Go compares byte by byte, with the same result).
pub(super) fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let (a_chunks, _) = a.as_chunks::<8>();
    let (b_chunks, _) = b.as_chunks::<8>();
    let mut n = 0;
    for (x, y) in a_chunks.iter().zip(b_chunks) {
        let diff = u64::from_le_bytes(*x) ^ u64::from_le_bytes(*y);
        if diff != 0 {
            return n + (diff.trailing_zeros() / 8) as usize;
        }
        n += 8;
    }
    let tail_a = a.get(n..).unwrap_or_default();
    let tail_b = b.get(n..).unwrap_or_default();
    n + tail_a
        .iter()
        .zip(tail_b)
        .take_while(|(x, y)| x == y)
        .count()
}

/// Go: `emitLiteral`.
fn emit_literal(dst: &mut Vec<Token>, lit: &[u8]) {
    dst.extend(lit.iter().map(|&v| Token::literal_token(u32::from(v))));
}

#[cfg(test)]
mod tests {
    //! Ports of Go's `deflate_test.go` tests of `deflateFast`.

    use super::{BUFFER_RESET, DeflateFast, INPUT_MARGIN, MAX_MATCH_LENGTH, MAX_MATCH_OFFSET};
    use crate::{
        GzipError,
        compress::{
            flate::{BEST_SPEED, Reader, Writer},
            testutil::gen_input,
        },
    };

    /// Go: `TestBestSpeedMaxMatchOffset`: matches about `maxMatchOffset` back, at the end of
    /// the input or not, round trip through `BestSpeed`.
    #[test]
    fn best_speed_max_match_offset() -> Result<(), String> {
        const ABC: &[u8] = b"abcdefgh";
        const XYZ: &[u8] = b"stuvwxyz";
        for match_before in [false, true] {
            for extra in [
                0,
                INPUT_MARGIN - 1,
                INPUT_MARGIN,
                INPUT_MARGIN + 1,
                2 * INPUT_MARGIN,
            ] {
                for offset in MAX_MATCH_OFFSET - 5..=MAX_MATCH_OFFSET + 5 {
                    // Go: src is abc, about maxMatchOffset zeros, xyz unless matchBefore, abc
                    // and `extra` zeros; the two abc are `offset` apart.
                    let mut src = vec![0; offset + ABC.len() + extra];
                    let mut put = |at: usize, b: &[u8]| {
                        if let Some(dst) = src.get_mut(at..at + b.len()) {
                            dst.copy_from_slice(b);
                        }
                    };
                    put(0, ABC);
                    if !match_before {
                        put(offset - XYZ.len(), XYZ);
                    }
                    put(offset, ABC);
                    let report = |desc: &str| {
                        format!(
                            "matchBefore={match_before}, extra={extra}, offsetAdj={}: {desc}",
                            offset.cast_signed() - MAX_MATCH_OFFSET.cast_signed()
                        )
                    };

                    let mut w = Writer::new(Vec::new(), BEST_SPEED)
                        .map_err(|err| report(&format!("NewWriter: {err}")))?;
                    let _n = w
                        .write(&src)
                        .map_err(|err| report(&format!("Write: {err}")))?;
                    w.close()
                        .map_err(|err| report(&format!("Writer.Close: {err}")))?;
                    let stream = w.into_inner();

                    let mut r = Reader::new(stream.as_slice());
                    let mut dst = Vec::new();
                    let mut buf = [0; 4096];
                    loop {
                        let (n, err) = r.read(&mut buf);
                        dst.extend_from_slice(buf.get(..n).unwrap_or_default());
                        match err {
                            None => {}
                            Some(GzipError::Eof) => break,
                            Some(err) => return Err(report(&format!("ReadAll: {err}"))),
                        }
                    }
                    if dst != src {
                        return Err(report("bytes differ after round-tripping"));
                    }
                }
            }
        }
        Ok(())
    }

    /// A case of Go's `TestBestSpeedMatch`: `(previous, current, t, s, want)`.
    type MatchCase = (Vec<u8>, Vec<u8>, i32, usize, usize);

    /// Go: `TestBestSpeedMatch`.
    #[test]
    fn best_speed_match() -> Result<(), String> {
        let longest = MAX_MATCH_LENGTH - 4;
        let cases: [MatchCase; 17] = [
            (
                vec![0, 0, 0, 1, 2],
                vec![3, 4, 5, 0, 1, 2, 3, 4, 5],
                -3,
                3,
                6,
            ),
            (
                vec![0, 0, 0, 1, 2],
                vec![2, 4, 5, 0, 1, 2, 3, 4, 5],
                -3,
                3,
                3,
            ),
            (
                vec![0, 0, 0, 1, 1],
                vec![3, 4, 5, 0, 1, 2, 3, 4, 5],
                -3,
                3,
                2,
            ),
            (
                vec![0, 0, 0, 1, 2],
                vec![2, 2, 2, 2, 1, 2, 3, 4, 5],
                -1,
                0,
                4,
            ),
            (
                vec![0, 0, 0, 1, 2, 3, 4, 5, 2, 2],
                vec![2, 2, 2, 2, 1, 2, 3, 4, 5],
                -7,
                4,
                5,
            ),
            (
                vec![9, 9, 9, 9, 9],
                vec![2, 2, 2, 2, 1, 2, 3, 4, 5],
                -1,
                0,
                0,
            ),
            (
                vec![9, 9, 9, 9, 9],
                vec![9, 2, 2, 2, 1, 2, 3, 4, 5],
                0,
                1,
                0,
            ),
            (vec![], vec![9, 2, 2, 2, 1, 2, 3, 4, 5], -5, 1, 0),
            (vec![], vec![9, 2, 2, 2, 1, 2, 3, 4, 5], -1, 1, 0),
            (vec![], vec![2, 2, 2, 2, 1, 2, 3, 4, 5], 0, 1, 3),
            (vec![3, 4, 5], vec![3, 4, 5], -3, 0, 3),
            (vec![0; 1000], vec![0; 1000], -1000, 0, longest),
            (vec![0; 200], vec![0; 500], -200, 0, longest),
            (vec![0; 200], vec![0; 500], 0, 1, longest),
            (vec![0; longest], vec![0; 500], -254, 0, longest),
            (vec![0; 200], vec![0; 500], -200, 400, 100),
            (vec![0; 10], vec![0; 500], 200, 400, 100),
        ];
        for (i, (previous, current, t, s, want)) in cases.into_iter().enumerate() {
            let mut e = DeflateFast::new();
            e.prev = previous;
            let got = e.match_len(s, t, &current);
            if got != want {
                return Err(format!("Test {i}: match length, want {want}, got {got}"));
            }
        }
        Ok(())
    }

    /// The number of tokens `enc` encodes `src` to.
    fn tokens(enc: &mut DeflateFast, src: &[u8]) -> usize {
        let mut dst = Vec::new();
        enc.encode(&mut dst, src);
        dst.len()
    }

    /// Go: `TestBestSpeedShiftOffsets`: shifting the offsets keeps the matches in range and drops
    /// the others. The 32 bytes come from splitmix64 (Go draws them from `math/rand`); like Go's,
    /// they have no internal matches.
    #[test]
    fn best_speed_shift_offsets() -> Result<(), String> {
        let mut enc = DeflateFast::new();
        let test_data = gen_input("rand:32:0")?;
        let len = i32::try_from(test_data.len()).map_err(|err| err.to_string())?;

        // Go: encode the test data with clean state; the second part should pick up matches
        // from the first block.
        let want_first_tokens = tokens(&mut enc, &test_data);
        let want_second_tokens = tokens(&mut enc, &test_data);
        if want_first_tokens <= want_second_tokens {
            return Err("test needs matches between inputs to be generated".to_owned());
        }

        // Go: forward the current indicator to before wraparound; part 1 before the wrap should
        // match the clean state.
        enc.cur = BUFFER_RESET - len;
        let before_wrap = tokens(&mut enc, &test_data);
        if want_first_tokens != before_wrap {
            return Err(format!(
                "got {before_wrap}, want {want_first_tokens} tokens"
            ));
        }
        if enc.cur != BUFFER_RESET {
            return Err(format!(
                "got {}, want e.cur to be at bufferReset ({BUFFER_RESET})",
                enc.cur
            ));
        }

        // Go: part 2 should match the clean state as well even if wrapped.
        let wrapped = tokens(&mut enc, &test_data);
        if want_second_tokens != wrapped {
            return Err(format!("got {wrapped}, want {want_second_tokens} token"));
        }
        if enc.cur >= BUFFER_RESET {
            return Err(format!(
                "want e.cur to be < bufferReset ({BUFFER_RESET}), got {}",
                enc.cur
            ));
        }

        // Go: forward the current buffer, leaving the matches at the bottom; no match may be
        // picked up.
        enc.cur = BUFFER_RESET;
        enc.shift_offsets();
        let shifted = tokens(&mut enc, &test_data);
        if want_first_tokens != shifted {
            return Err(format!("got {shifted}, want {want_first_tokens} tokens"));
        }
        Ok(())
    }
}
