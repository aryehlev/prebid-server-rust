//! Go: `compress/flate/deflate.go`: the compressor and the public [`Writer`].

use core::{fmt, mem};
use std::io::Write;

use super::{
    super::goio::go_copy,
    deflatefast::{DeflateFast, common_prefix},
    huffman_bit_writer::{HuffmanBitWriter, MAX_STORE_BLOCK_SIZE},
    token::Token,
};
use crate::GzipError;

/// Go: `flate.NoCompression`.
pub const NO_COMPRESSION: isize = 0;
/// Go: `flate.BestSpeed`.
pub const BEST_SPEED: isize = 1;
/// Go: `flate.BestCompression`.
pub const BEST_COMPRESSION: isize = 9;
/// Go: `flate.DefaultCompression` (level 6).
pub const DEFAULT_COMPRESSION: isize = -1;
/// Go: `flate.HuffmanOnly`: Huffman entropy coding only, no LZ77 match search.
pub const HUFFMAN_ONLY: isize = -2;

/// Go: `logWindowSize`.
const LOG_WINDOW_SIZE: u32 = 15;
/// Go: `windowSize`.
pub(super) const WINDOW_SIZE: usize = 1 << LOG_WINDOW_SIZE;
/// Go: `windowMask`.
const WINDOW_MASK: usize = WINDOW_SIZE - 1;
/// Go: `minMatchLength`, the smallest match length the compressor emits.
const MIN_MATCH_LENGTH: usize = 4;
/// Go: `maxMatchLength`.
pub(super) const MAX_MATCH_LENGTH: usize = 258;
/// Go: `baseMatchLength`, the smallest match length per RFC 1951 3.2.5.
const BASE_MATCH_LENGTH: usize = 3;
/// Go: `baseMatchOffset`.
const BASE_MATCH_OFFSET: usize = 1;
/// Go: `maxMatchOffset`.
pub(super) const MAX_MATCH_OFFSET: usize = 1 << 15;
/// Go: `maxFlateBlockTokens`, the maximum number of tokens in one block.
const MAX_FLATE_BLOCK_TOKENS: usize = 1 << 14;
/// Go: `hashBits`.
const HASH_BITS: u32 = 17;
/// Go: `hashSize`.
const HASH_SIZE: usize = 1 << HASH_BITS;
/// Go: `hashMask`.
const HASH_MASK: u32 = (1 << HASH_BITS) - 1;
/// Go: `maxHashOffset`.
const MAX_HASH_OFFSET: usize = 1 << 24;
/// Go: `skipNever` (`math.MaxInt32`).
const SKIP_NEVER: usize = 0x7FFF_FFFF;
/// Go: `math.MaxInt32`, the `blockStart` of a block whose start left the window.
const BLOCK_START_GONE: usize = 0x7FFF_FFFF;
/// Go: `hashmul`.
const HASH_MUL: u32 = 0x1E35_A7BD;

/// Go: `compressionLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompressionLevel {
    level: usize,
    good: usize,
    lazy: usize,
    nice: usize,
    chain: usize,
    fast_skip_hashing: usize,
}

impl CompressionLevel {
    const fn new(
        level: usize,
        good: usize,
        lazy: usize,
        nice: usize,
        chain: usize,
        fast_skip_hashing: usize,
    ) -> Self {
        Self {
            level,
            good,
            lazy,
            nice,
            chain,
            fast_skip_hashing,
        }
    }
}

/// Go: `levels`.
static LEVELS: [CompressionLevel; 10] = [
    // Go: NoCompression.
    CompressionLevel::new(0, 0, 0, 0, 0, 0),
    // Go: BestSpeed uses a custom algorithm; see deflatefast.go.
    CompressionLevel::new(1, 0, 0, 0, 0, 0),
    // Go: for levels 2-3 we don't bother trying with lazy matches.
    CompressionLevel::new(2, 4, 0, 16, 8, 5),
    CompressionLevel::new(3, 4, 0, 32, 32, 6),
    // Go: levels 4-9 use increasingly more lazy matching and increasingly stringent conditions
    // for "good enough".
    CompressionLevel::new(4, 4, 4, 16, 16, SKIP_NEVER),
    CompressionLevel::new(5, 8, 16, 32, 32, SKIP_NEVER),
    CompressionLevel::new(6, 8, 16, 128, 128, SKIP_NEVER),
    CompressionLevel::new(7, 8, 32, 128, 256, SKIP_NEVER),
    CompressionLevel::new(8, 32, 128, 258, 1024, SKIP_NEVER),
    CompressionLevel::new(9, 32, 258, 258, 4096, SKIP_NEVER),
];

/// Go: the zero `compressionLevel` that levels 0 and -2 keep.
const ZERO_LEVEL: CompressionLevel = CompressionLevel::new(0, 0, 0, 0, 0, 0);

/// Go: the `fill` and `step` functions `compressor.init` picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Go: `fillStore` and `store` (level 0).
    Store,
    /// Go: `fillStore` and `storeHuff` (level -2).
    StoreHuff,
    /// Go: `fillStore` and `encSpeed` (level 1).
    EncSpeed,
    /// Go: `fillDeflate` and `deflate` (levels 2 to 9).
    Deflate,
}

/// Go: `compressor`. The output goes to the writer each call is given (the `huffmanBitWriter`'s
/// `writer` in Go).
pub(in crate::compress) struct Compressor {
    level: CompressionLevel,
    w: HuffmanBitWriter,
    step: Step,
    /// Go: `bestSpeed`, the encoder of level 1.
    best_speed: Option<DeflateFast>,

    // Go: input hash chains. hashHead[hashValue] contains the largest inputIndex with the
    // specified hash value. If hashHead[hashValue] is within the current window, then
    // hashPrev[hashHead[hashValue] & windowMask] contains the previous index with the same hash
    // value. Allocated for levels 2 to 9 only.
    chain_head: isize,
    hash_head: Box<[u32]>,
    hash_prev: Box<[u32]>,
    hash_offset: usize,

    // Go: input window: unprocessed data is window[index:windowEnd].
    index: usize,
    window: Vec<u8>,
    window_end: usize,
    /// Go: window index where the current tokens start.
    block_start: usize,
    /// Go: if true, still need to process `window[index-1]`.
    byte_available: bool,

    /// Go: requesting flush.
    sync: bool,

    /// Go: queued output tokens.
    tokens: Vec<Token>,

    // Go: deflate state.
    length: usize,
    offset: usize,
    max_insert_index: isize,
    err: Option<GzipError>,
}

impl fmt::Debug for Compressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Compressor")
            .field("level", &self.level.level)
            .field("step", &self.step)
            .field("err", &self.err)
            .finish_non_exhaustive()
    }
}

/// Go: `hash4`: the hash of the first 4 bytes of `b`.
fn hash4(b: &[u8]) -> u32 {
    match b.get(..4) {
        Some(&[b0, b1, b2, b3]) => {
            u32::from_be_bytes([b0, b1, b2, b3]).wrapping_mul(HASH_MUL) >> (32 - HASH_BITS)
        }
        _ => 0,
    }
}

/// Go: `matchLen`: the number of bytes `a` and `b` have in common, up to `max`. Both are at
/// least `max` long.
fn match_len(a: &[u8], b: &[u8], max: usize) -> usize {
    match (a.get(..max), b.get(..max)) {
        (Some(a), Some(b)) => common_prefix(a, b),
        _ => 0,
    }
}

/// `x` as a Go `uint32` hash chain entry.
fn chain_entry(x: usize) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

impl Compressor {
    /// Go: `compressor.init` (and `NewWriter`'s level check).
    pub(in crate::compress) fn new(level: isize) -> Result<Self, GzipError> {
        let mut d = Self {
            level: ZERO_LEVEL,
            w: HuffmanBitWriter::new(),
            step: Step::Store,
            best_speed: None,
            chain_head: 0,
            hash_head: Box::default(),
            hash_prev: Box::default(),
            hash_offset: 0,
            index: 0,
            window: Vec::new(),
            window_end: 0,
            block_start: 0,
            byte_available: false,
            sync: false,
            tokens: Vec::new(),
            length: 0,
            offset: 0,
            max_insert_index: 0,
            err: None,
        };
        match level {
            NO_COMPRESSION => {
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.step = Step::Store;
            }
            HUFFMAN_ONLY => {
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.step = Step::StoreHuff;
            }
            BEST_SPEED => {
                d.level = LEVELS[1];
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.step = Step::EncSpeed;
                d.best_speed = Some(DeflateFast::new());
                d.tokens = Vec::with_capacity(MAX_STORE_BLOCK_SIZE + 1);
            }
            DEFAULT_COMPRESSION | 2..=9 => {
                let index = if level == DEFAULT_COMPRESSION {
                    6
                } else {
                    level.unsigned_abs()
                };
                d.level = LEVELS.get(index).copied().unwrap_or(ZERO_LEVEL);
                d.init_deflate();
                d.step = Step::Deflate;
            }
            _ => return Err(GzipError::InvalidFlateLevel(level)),
        }
        Ok(d)
    }

    /// Go: `compressor.initDeflate`.
    fn init_deflate(&mut self) {
        self.window = vec![0; 2 * WINDOW_SIZE];
        self.hash_head = vec![0; HASH_SIZE].into_boxed_slice();
        self.hash_prev = vec![0; WINDOW_SIZE].into_boxed_slice();
        self.hash_offset = 1;
        self.tokens = Vec::with_capacity(MAX_FLATE_BLOCK_TOKENS + 1);
        self.length = MIN_MATCH_LENGTH - 1;
        self.offset = 0;
        self.byte_available = false;
        self.index = 0;
        self.chain_head = -1;
    }

    /// Go: `d.fill(d, b)`.
    fn fill(&mut self, b: &[u8]) -> usize {
        if self.step == Step::Deflate {
            self.fill_deflate(b)
        } else {
            self.fill_store(b)
        }
    }

    /// Go: `d.step(d)`.
    fn step(&mut self, w: &mut dyn Write) {
        match self.step {
            Step::Store => self.store(w),
            Step::StoreHuff => self.store_huff(w),
            Step::EncSpeed => self.enc_speed(w),
            Step::Deflate => self.deflate(w),
        }
    }

    /// Go: `hashHead[h & hashMask]`.
    fn head(&self, h: u32) -> u32 {
        self.hash_head
            .get((h & HASH_MASK) as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Go: inserts window position `index` into the hash chains and returns the previous head
    /// of its chain.
    fn insert(&mut self, index: usize) -> u32 {
        let h = hash4(self.window.get(index..).unwrap_or_default());
        let prev = self.head(h);
        if let Some(slot) = self.hash_prev.get_mut(index & WINDOW_MASK) {
            *slot = prev;
        }
        if let Some(slot) = self.hash_head.get_mut((h & HASH_MASK) as usize) {
            *slot = chain_entry(index + self.hash_offset);
        }
        prev
    }

    /// Go: `compressor.fillDeflate`.
    fn fill_deflate(&mut self, b: &[u8]) -> usize {
        if self.index >= 2 * WINDOW_SIZE - (MIN_MATCH_LENGTH + MAX_MATCH_LENGTH)
            && self.window.len() >= 2 * WINDOW_SIZE
        {
            // Go: shift the window by windowSize.
            self.window.copy_within(WINDOW_SIZE..2 * WINDOW_SIZE, 0);
            self.index -= WINDOW_SIZE;
            self.window_end -= WINDOW_SIZE;
            if self.block_start >= WINDOW_SIZE {
                self.block_start -= WINDOW_SIZE;
            } else {
                self.block_start = BLOCK_START_GONE;
            }
            self.hash_offset += WINDOW_SIZE;
            if self.hash_offset > MAX_HASH_OFFSET {
                let delta = self.hash_offset - 1;
                self.hash_offset -= delta;
                self.chain_head -= isize::try_from(delta).unwrap_or(isize::MAX);
                let rebase = |v: &mut u32| {
                    let x = *v as usize;
                    *v = if x > delta { chain_entry(x - delta) } else { 0 };
                };
                self.hash_prev.iter_mut().for_each(rebase);
                self.hash_head.iter_mut().for_each(rebase);
            }
        }
        let n = go_copy(
            self.window.get_mut(self.window_end..).unwrap_or_default(),
            b,
        );
        self.window_end += n;
        n
    }

    /// Go: `compressor.writeBlock`: writes the queued tokens as a block ending at window
    /// position `index`.
    fn write_block(&mut self, w: &mut dyn Write, index: usize) -> Option<GzipError> {
        if index > 0 {
            let window = if self.block_start <= index {
                self.window.get(self.block_start..index)
            } else {
                None
            };
            self.block_start = index;
            self.w.write_block(w, &self.tokens, false, window);
            return self.w.err();
        }
        None
    }

    /// Go: `compressor.fillWindow`: fills the window with the dictionary `b` and its hashes.
    /// Only used right after a reset.
    pub(in crate::compress) fn fill_window(&mut self, b: &[u8]) {
        // Go: do not fill window if we are in store-only mode.
        if self.level.level < 2 {
            return;
        }
        if self.index != 0 || self.window_end != 0 {
            // Go: panic("internal error: fillWindow called with stale data"); unreachable.
            return;
        }

        // Go: if we are given too much, cut it.
        let b = b
            .get(b.len().saturating_sub(WINDOW_SIZE)..)
            .unwrap_or_default();
        // Go: add all to window.
        let n = go_copy(&mut self.window, b);

        // Go: bulkHash4 over 256-byte chunks inserts every position with 4 bytes of data, in
        // order.
        for di in 0..n.saturating_sub(MIN_MATCH_LENGTH - 1) {
            let _prev = self.insert(di);
        }
        // Go: update window information.
        self.window_end = n;
        self.index = n;
    }

    /// Go: `compressor.findMatch`: the longest match at `pos` longer than `prev_length`,
    /// following at most `chain` entries of the hash chain from `prev_head`.
    fn find_match(
        &self,
        pos: usize,
        prev_head: isize,
        prev_length: usize,
        lookahead: usize,
    ) -> Option<(usize, usize)> {
        let min_match_look = MAX_MATCH_LENGTH.min(lookahead);
        let win = self.window.get(..pos + min_match_look)?;

        // Go: we quit when we get a match that's at least nice long.
        let nice = (win.len() - pos).min(self.level.nice);

        // Go: if we've got a match that's good enough, only look in 1/4 the chain.
        let mut tries = self.level.chain;
        let mut length = prev_length;
        if length >= self.level.good {
            tries >>= 2;
        }

        let mut w_end = win.get(pos + length).copied();
        let w_pos = win.get(pos..).unwrap_or_default();
        let min_index = pos.cast_signed() - WINDOW_SIZE.cast_signed();
        let mut found = None;

        let mut i = prev_head;
        while tries > 0 {
            let iu = i.cast_unsigned();
            if w_end == win.get(iu + length).copied() {
                let n = match_len(win.get(iu..).unwrap_or_default(), w_pos, min_match_look);
                if n > length && (n > MIN_MATCH_LENGTH || pos - iu <= 4096) {
                    length = n;
                    found = Some((n, pos - iu));
                    if n >= nice {
                        // Go: the match is good enough that we don't try to find a better one.
                        break;
                    }
                    w_end = win.get(pos + n).copied();
                }
            }
            if i == min_index {
                // Go: hashPrev[i & windowMask] has already been overwritten, so stop now.
                break;
            }
            let prev = self.hash_prev.get(iu & WINDOW_MASK).copied().unwrap_or(0);
            i = (prev as usize).cast_signed() - self.hash_offset.cast_signed();
            if i < min_index || i < 0 {
                break;
            }
            tries -= 1;
        }
        found
    }

    /// Go: `compressor.writeStoredBlock`.
    fn write_stored_block(
        bw: &mut HuffmanBitWriter,
        w: &mut dyn Write,
        buf: &[u8],
    ) -> Option<GzipError> {
        bw.write_stored_header(w, buf.len(), false);
        if let Some(err) = bw.err() {
            return Some(err);
        }
        bw.write_bytes(w, buf);
        bw.err()
    }

    /// Go: `compressor.encSpeed`: compresses and writes the window once it holds
    /// `maxStoreBlockSize` bytes or a flush is requested.
    fn enc_speed(&mut self, w: &mut dyn Write) {
        let Some(best_speed) = self.best_speed.as_mut() else {
            return;
        };
        let window = self.window.get(..self.window_end).unwrap_or_default();
        // Go: we only compress if we have maxStoreBlockSize.
        if self.window_end < MAX_STORE_BLOCK_SIZE {
            if !self.sync {
                return;
            }

            // Go: handle small sizes.
            if self.window_end < 128 {
                match self.window_end {
                    0 => return,
                    1..=16 => self.err = Self::write_stored_block(&mut self.w, w, window),
                    _ => {
                        self.w.write_block_huff(w, false, window);
                        self.err = self.w.err();
                    }
                }
                self.window_end = 0;
                best_speed.reset();
                return;
            }
        }
        // Go: encode the block.
        self.tokens.clear();
        best_speed.encode(&mut self.tokens, window);

        // Go: if we removed less than 1/16th, Huffman compress the block.
        if self.tokens.len() > self.window_end - (self.window_end >> 4) {
            self.w.write_block_huff(w, false, window);
        } else {
            self.w
                .write_block_dynamic(w, &self.tokens, false, Some(window));
        }
        self.err = self.w.err();
        self.window_end = 0;
    }

    /// Go: `compressor.deflate`: the LZ77 matcher of levels 2 to 9 (greedy for 2 and 3, lazy
    /// above), run over the window while enough lookahead remains or a flush is requested.
    fn deflate(&mut self, w: &mut dyn Write) {
        if self.window_end - self.index < MIN_MATCH_LENGTH + MAX_MATCH_LENGTH && !self.sync {
            return;
        }

        self.max_insert_index =
            self.window_end.cast_signed() - (MIN_MATCH_LENGTH - 1).cast_signed();
        let fast = self.level.fast_skip_hashing != SKIP_NEVER;

        loop {
            let lookahead = self.window_end - self.index;
            if lookahead < MIN_MATCH_LENGTH + MAX_MATCH_LENGTH {
                if !self.sync {
                    break;
                }
                if lookahead == 0 {
                    // Go: flush current output block if any.
                    if self.byte_available {
                        // Go: there is still one pending token that needs to be flushed.
                        self.push_literal(self.index - 1);
                        self.byte_available = false;
                    }
                    if !self.tokens.is_empty() {
                        self.err = self.write_block(w, self.index);
                        if self.err.is_some() {
                            return;
                        }
                        self.tokens.clear();
                    }
                    break;
                }
            }
            if self.index.cast_signed() < self.max_insert_index {
                // Go: update the hash.
                self.chain_head = (self.insert(self.index) as usize).cast_signed();
            }
            let prev_length = self.length;
            let prev_offset = self.offset;
            self.length = MIN_MATCH_LENGTH - 1;
            self.offset = 0;
            let min_index = self.index.saturating_sub(WINDOW_SIZE);

            let head = self.chain_head - self.hash_offset.cast_signed();
            if head >= min_index.cast_signed()
                && (fast && lookahead > MIN_MATCH_LENGTH - 1
                    || !fast && lookahead > prev_length && prev_length < self.level.lazy)
                && let Some((new_length, new_offset)) =
                    self.find_match(self.index, head, MIN_MATCH_LENGTH - 1, lookahead)
            {
                self.length = new_length;
                self.offset = new_offset;
            }
            let emit_match = if fast {
                self.length >= MIN_MATCH_LENGTH
            } else {
                prev_length >= MIN_MATCH_LENGTH && self.length <= prev_length
            };
            if emit_match {
                // Go: there was a match at the previous step, and the current match is not
                // better. Output the previous match.
                let (length, offset) = if fast {
                    (self.length, self.offset)
                } else {
                    (prev_length, prev_offset)
                };
                self.tokens.push(Token::match_token(
                    chain_entry(length - BASE_MATCH_LENGTH),
                    chain_entry(offset - BASE_MATCH_OFFSET),
                ));
                self.skip_match(prev_length, fast);
                if self.tokens.len() == MAX_FLATE_BLOCK_TOKENS {
                    // Go: the block includes the current character.
                    self.err = self.write_block(w, self.index);
                    if self.err.is_some() {
                        return;
                    }
                    self.tokens.clear();
                }
            } else {
                if fast || self.byte_available {
                    let i = if fast { self.index } else { self.index - 1 };
                    self.push_literal(i);
                    if self.tokens.len() == MAX_FLATE_BLOCK_TOKENS {
                        self.err = self.write_block(w, i + 1);
                        if self.err.is_some() {
                            return;
                        }
                        self.tokens.clear();
                    }
                }
                self.index += 1;
                if !fast {
                    self.byte_available = true;
                }
            }
        }
    }

    /// Go: queues `literalToken(window[i])`.
    fn push_literal(&mut self, i: usize) {
        let literal = self.window.get(i).copied().unwrap_or(0);
        self.tokens.push(Token::literal_token(u32::from(literal)));
    }

    /// The part of Go's `deflate` after a match is queued: inserts the strings up to the end of
    /// the match into the hash table (unless the match is too long for the level) and moves
    /// `index` past it.
    fn skip_match(&mut self, prev_length: usize, fast: bool) {
        // Go: index and index-1 are already inserted. If there is not enough lookahead, the
        // last two strings are not inserted into the hash table.
        if self.length <= self.level.fast_skip_hashing {
            let new_index = if fast {
                self.index + self.length
            } else {
                self.index + prev_length - 1
            };
            let mut index = self.index + 1;
            while index < new_index {
                if index.cast_signed() < self.max_insert_index {
                    let _prev = self.insert(index);
                }
                index += 1;
            }
            self.index = index;

            if !fast {
                self.byte_available = false;
                self.length = MIN_MATCH_LENGTH - 1;
            }
        } else {
            // Go: for matches this long, we don't bother inserting each individual item into
            // the table.
            self.index += self.length;
        }
    }

    /// Go: `compressor.fillStore`.
    fn fill_store(&mut self, b: &[u8]) -> usize {
        let n = go_copy(
            self.window.get_mut(self.window_end..).unwrap_or_default(),
            b,
        );
        self.window_end += n;
        n
    }

    /// Go: `compressor.store` (level 0).
    fn store(&mut self, w: &mut dyn Write) {
        if self.window_end > 0 && (self.window_end == MAX_STORE_BLOCK_SIZE || self.sync) {
            let window = self.window.get(..self.window_end).unwrap_or_default();
            self.err = Self::write_stored_block(&mut self.w, w, window);
            self.window_end = 0;
        }
    }

    /// Go: `compressor.storeHuff` (level -2): Huffman codes the window once it is full or a
    /// flush is requested.
    fn store_huff(&mut self, w: &mut dyn Write) {
        if self.window_end < self.window.len() && !self.sync || self.window_end == 0 {
            return;
        }
        let window = self.window.get(..self.window_end).unwrap_or_default();
        self.w.write_block_huff(w, false, window);
        self.err = self.w.err();
        self.window_end = 0;
    }

    /// Go: `compressor.write`.
    pub(in crate::compress) fn write(
        &mut self,
        w: &mut dyn Write,
        mut b: &[u8],
    ) -> Result<usize, GzipError> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        let n = b.len();
        while !b.is_empty() {
            self.step(w);
            let filled = self.fill(b);
            b = b.get(filled..).unwrap_or_default();
            if let Some(err) = &self.err {
                return Err(err.clone());
            }
        }
        Ok(n)
    }

    /// Go: `compressor.syncFlush`: everything written so far, then an empty stored block.
    pub(in crate::compress) fn sync_flush(&mut self, w: &mut dyn Write) -> Result<(), GzipError> {
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        self.sync = true;
        self.step(w);
        if self.err.is_none() {
            self.w.write_stored_header(w, 0, false);
            self.w.flush(w);
            self.err = self.w.err();
        }
        self.sync = false;
        self.err.clone().map_or(Ok(()), Err)
    }

    /// Go: `compressor.reset` (the new writer is the one later calls are given).
    pub(in crate::compress) fn reset(&mut self) {
        self.w.reset();
        self.sync = false;
        self.err = None;
        match self.level.level {
            0 => self.window_end = 0,
            1 => {
                self.window_end = 0;
                self.tokens.clear();
                if let Some(best_speed) = self.best_speed.as_mut() {
                    best_speed.reset();
                }
            }
            _ => {
                self.chain_head = -1;
                self.hash_head.fill(0);
                self.hash_prev.fill(0);
                self.hash_offset = 1;
                self.index = 0;
                self.window_end = 0;
                self.block_start = 0;
                self.byte_available = false;
                self.tokens.clear();
                self.length = MIN_MATCH_LENGTH - 1;
                self.offset = 0;
                self.max_insert_index = 0;
            }
        }
    }

    /// Go: `compressor.close`: everything written so far, then a final empty stored block.
    /// Later writes fail with [`GzipError::WriterClosed`].
    pub(in crate::compress) fn close(&mut self, w: &mut dyn Write) -> Result<(), GzipError> {
        if self.err == Some(GzipError::WriterClosed) {
            return Ok(());
        }
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        self.sync = true;
        self.step(w);
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        // Go: a failure from here on leaves d.err nil and d.sync true.
        self.w.write_stored_header(w, 0, true);
        if let Some(err) = self.w.err() {
            return Err(err);
        }
        self.w.flush(w);
        if let Some(err) = self.w.err() {
            return Err(err);
        }
        self.err = Some(GzipError::WriterClosed);
        Ok(())
    }
}

/// Go: `flate.Writer`: writes the DEFLATE (RFC 1951) compressed form of the data written to it
/// to an underlying writer, byte for byte as Go does.
///
/// The bytes depend on the level and on how the data is split into [`Writer::write`] calls, as
/// in Go; [`Writer::flush`] emits a sync marker and [`Writer::close`] the final block.
pub struct Writer<W> {
    w: W,
    d: Box<Compressor>,
    /// Go: `dict`, kept for [`Writer::reset`].
    dict: Vec<u8>,
}

impl<W> fmt::Debug for Writer<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("d", &self.d)
            .finish_non_exhaustive()
    }
}

impl<W: Write> Writer<W> {
    /// Go: `flate.NewWriter(w, level)`. Levels range from [`BEST_SPEED`] (1) to
    /// [`BEST_COMPRESSION`] (9); [`NO_COMPRESSION`] (0) only adds the DEFLATE framing,
    /// [`DEFAULT_COMPRESSION`] (-1) is level 6 and [`HUFFMAN_ONLY`] (-2) does no match search.
    ///
    /// # Errors
    ///
    /// [`GzipError::InvalidFlateLevel`] when `level` is outside -2 to 9.
    pub fn new(w: W, level: isize) -> Result<Self, GzipError> {
        Ok(Self {
            w,
            d: Box::new(Compressor::new(level)?),
            dict: Vec::new(),
        })
    }

    /// Go: `flate.NewWriterDict(w, level, dict)`: as [`Writer::new`], as if `dict` had been
    /// written first without producing output. Only levels 2 to 9 use the dictionary.
    ///
    /// # Errors
    ///
    /// [`GzipError::InvalidFlateLevel`] when `level` is outside -2 to 9.
    pub fn new_dict(w: W, level: isize, dict: &[u8]) -> Result<Self, GzipError> {
        let mut zw = Self::new(w, level)?;
        zw.d.fill_window(dict);
        // Go: duplicate dictionary for Reset method.
        zw.dict = dict.to_vec();
        Ok(zw)
    }

    /// Go: `Writer.Write`: compresses `data`; returns `data.len()`.
    ///
    /// # Errors
    ///
    /// The underlying writer's error, which is sticky, or [`GzipError::WriterClosed`] after
    /// [`Writer::close`].
    pub fn write(&mut self, data: &[u8]) -> Result<usize, GzipError> {
        self.d.write(&mut self.w, data)
    }

    /// Go: `Writer.Flush`: writes all pending data followed by a sync marker (an empty stored
    /// block, at least 4 bytes), zlib's `Z_SYNC_FLUSH`. The underlying writer is not flushed.
    ///
    /// # Errors
    ///
    /// The underlying writer's error, or [`GzipError::WriterClosed`] after [`Writer::close`].
    pub fn flush(&mut self) -> Result<(), GzipError> {
        self.d.sync_flush(&mut self.w)
    }

    /// Go: `Writer.Close`: writes all pending data and the final block. Closing again does
    /// nothing.
    ///
    /// # Errors
    ///
    /// The underlying writer's error.
    pub fn close(&mut self) -> Result<(), GzipError> {
        self.d.close(&mut self.w)
    }

    /// Go: `Writer.Reset(dst)`: discards the state, as if newly created by [`Writer::new`] or
    /// [`Writer::new_dict`] with `dst` and the same level and dictionary, keeping the
    /// allocations. Returns the previous underlying writer.
    pub fn reset(&mut self, dst: W) -> W {
        let old = mem::replace(&mut self.w, dst);
        self.d.reset();
        self.d.fill_window(&self.dict);
        old
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
    //! Ports of Go's `deflate_test.go` and `writer_test.go` (the golden files cover the bytes
    //! of every level; these are Go's own checks). Go's `TestVeryLongSparseChunk` (2.3 GB) and
    //! `TestRegression2508` (128 MiB) only run without `-short` and are not ported, and
    //! `TestMaxStackSize` checks goroutine stacks. `TestWriterReset` compares the reset state
    //! with `reflect.DeepEqual`; the port compares the tokens and the output instead.

    use std::{cell::RefCell, io, rc::Rc};

    use super::{
        BEST_SPEED, DEFAULT_COMPRESSION, HASH_BITS, HASH_MUL, MAX_MATCH_OFFSET,
        MAX_STORE_BLOCK_SIZE, MIN_MATCH_LENGTH, Writer, hash4,
    };
    use crate::{
        GzipError,
        compress::{
            flate::Reader,
            testutil::{Target, gen_input},
        },
    };

    /// Go: `deflateTests`.
    const DEFLATE_TESTS: [(&[u8], isize, &[u8]); 15] = [
        (&[], 0, &[1, 0, 0, 255, 255]),
        (&[0x11], -1, &[18, 4, 4, 0, 0, 255, 255]),
        (&[0x11], DEFAULT_COMPRESSION, &[18, 4, 4, 0, 0, 255, 255]),
        (&[0x11], 4, &[18, 4, 4, 0, 0, 255, 255]),
        (&[0x11], 0, &[0, 1, 0, 254, 255, 17, 1, 0, 0, 255, 255]),
        (
            &[0x11, 0x12],
            0,
            &[0, 2, 0, 253, 255, 17, 18, 1, 0, 0, 255, 255],
        ),
        (
            &[0x11; 8],
            0,
            &[
                0, 8, 0, 247, 255, 17, 17, 17, 17, 17, 17, 17, 17, 1, 0, 0, 255, 255,
            ],
        ),
        (&[], 2, &[1, 0, 0, 255, 255]),
        (&[0x11], 2, &[18, 4, 4, 0, 0, 255, 255]),
        (&[0x11, 0x12], 2, &[18, 20, 2, 4, 0, 0, 255, 255]),
        (&[0x11; 8], 2, &[18, 132, 2, 64, 0, 0, 0, 255, 255]),
        (&[], 9, &[1, 0, 0, 255, 255]),
        (&[0x11], 9, &[18, 4, 4, 0, 0, 255, 255]),
        (&[0x11, 0x12], 9, &[18, 20, 2, 4, 0, 0, 255, 255]),
        (&[0x11; 8], 9, &[18, 132, 2, 64, 0, 0, 0, 255, 255]),
    ];

    /// The text of Go's `io.ErrClosedPipe`, which a failing [`Target`] returns.
    const CLOSED_PIPE: &str = "io: read/write on closed pipe";

    /// Go: `w.Write(b)` in Go's tests, which check the error and not the count.
    fn put<W: io::Write>(w: &mut Writer<W>, b: &[u8]) -> Result<(), String> {
        w.write(b).map(|_| ()).map_err(|err| err.to_string())
    }

    /// Compresses `input` with one write and a close.
    fn deflate(level: isize, input: &[u8]) -> Result<Vec<u8>, String> {
        let mut w = Writer::new(Vec::new(), level).map_err(|err| err.to_string())?;
        put(&mut w, input)?;
        w.close().map_err(|err| err.to_string())?;
        Ok(w.into_inner())
    }

    /// Go: `io.ReadAll(NewReader(...))`, the data and the error that ended it (`None` at EOF).
    fn inflate(stream: &[u8], dict: &[u8]) -> (Vec<u8>, Option<GzipError>) {
        let mut r = Reader::new_dict(stream, dict);
        let mut out = Vec::new();
        let mut buf = [0; 512];
        loop {
            let (n, err) = r.read(&mut buf);
            out.extend_from_slice(buf.get(..n).unwrap_or_default());
            match err {
                None => {}
                Some(GzipError::Eof) => return (out, None),
                Some(err) => return (out, Some(err)),
            }
        }
    }

    /// Go: `bulkHash4`, the rolling form of `hash4` that Go's `fillWindow` uses.
    fn bulk_hash4(b: &[u8]) -> Vec<u32> {
        let mut dst = Vec::new();
        let Some(&[b0, b1, b2, b3]) = b.get(..4) else {
            return dst;
        };
        let mut hb = u32::from_be_bytes([b0, b1, b2, b3]);
        dst.push(hb.wrapping_mul(HASH_MUL) >> (32 - HASH_BITS));
        for &next in b.get(4..).unwrap_or_default() {
            hb = (hb << 8) | u32::from(next);
            dst.push(hb.wrapping_mul(HASH_MUL) >> (32 - HASH_BITS));
        }
        dst
    }

    /// Go: `TestBulkHash4`: the per-position `hash4` the port's window filling uses gives Go's
    /// rolling `bulkHash4`.
    #[test]
    fn bulk_hash4_matches_hash4() -> Result<(), String> {
        for (_, _, out) in DEFLATE_TESTS {
            if out.len() < MIN_MATCH_LENGTH {
                continue;
            }
            let doubled = [out, out].concat();
            for j in 4..doubled.len() {
                let y = doubled.get(..j).unwrap_or_default();
                let want: Vec<u32> = (0..=j - MIN_MATCH_LENGTH)
                    .map(|i| hash4(y.get(i..).unwrap_or_default()))
                    .collect();
                if bulk_hash4(y) != want {
                    return Err(format!("Len:{j}: bulkHash4 differs from hash4"));
                }
            }
        }
        Ok(())
    }

    /// Go: `TestDeflate`.
    #[test]
    fn deflate_vectors() -> Result<(), String> {
        for (input, level, want) in DEFLATE_TESTS {
            let got = deflate(level, input)?;
            if got != want {
                return Err(format!(
                    "Deflate({level}, {input:x?}) = {got:?}, want {want:?}"
                ));
            }
        }
        Ok(())
    }

    /// Go: `TestWriterClose`.
    #[test]
    fn writer_close() -> Result<(), String> {
        let mut zw = Writer::new(Vec::new(), 6).map_err(|err| err.to_string())?;
        let open = zw.write(b"Test");
        if open != Ok(4) {
            return Err(format!("Write to not closed writer: {open:?}"));
        }
        zw.close().map_err(|err| format!("Close: {err}"))?;
        let after_close = zw.get_ref().len();
        let closed = zw.write(b"Test");
        if closed != Err(GzipError::WriterClosed) {
            return Err(format!("Write to closed writer: {closed:?}"));
        }
        if zw.flush().is_ok() {
            return Err("Flush to closed writer succeeded".to_owned());
        }
        zw.close().map_err(|err| format!("Close: {err}"))?;
        if after_close != zw.get_ref().len() {
            return Err(format!(
                "Writer wrote data after close. After close: {after_close}. After writes on \
                 closed stream: {}",
                zw.get_ref().len()
            ));
        }
        Ok(())
    }

    /// The writer half of Go's `syncBuffer`: appends to a buffer the reader half reads.
    #[derive(Debug)]
    struct SyncWriter(Rc<RefCell<Vec<u8>>>);

    impl io::Write for SyncWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The reader half of Go's `syncBuffer`: reads what has been written so far, and reports
    /// EOF past it (where Go's reader would block until more is written).
    #[derive(Debug)]
    struct SyncReader {
        shared: Rc<RefCell<Vec<u8>>>,
        pending: Vec<u8>,
        pos: usize,
        taken: usize,
    }

    impl SyncReader {
        /// The written bytes not yet read (Go: `buf.buf.Len()`).
        fn unread(&self) -> usize {
            self.shared.borrow().len() - self.taken + self.pending.len() - self.pos
        }
    }

    impl io::Read for SyncReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = io::BufRead::fill_buf(self)?.len().min(buf.len());
            if let (Some(dst), Some(src)) =
                (buf.get_mut(..n), self.pending.get(self.pos..self.pos + n))
            {
                dst.copy_from_slice(src);
            }
            io::BufRead::consume(self, n);
            Ok(n)
        }
    }

    impl io::BufRead for SyncReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.pos == self.pending.len() {
                let shared = self.shared.borrow();
                self.pending.clear();
                self.pending
                    .extend_from_slice(shared.get(self.taken..).unwrap_or_default());
                self.taken = shared.len();
                self.pos = 0;
            }
            Ok(self.pending.get(self.pos..).unwrap_or_default())
        }

        fn consume(&mut self, amount: usize) {
            self.pos = (self.pos + amount).min(self.pending.len());
        }
    }

    /// Go: `testSync`: after a flush, the reader returns everything written so far, reading
    /// the whole flushed stream and not past it; after a close, it returns `(0, EOF)` and has
    /// consumed the whole stream.
    fn sync(level: isize, input: &[u8]) -> Result<(), String> {
        if input.is_empty() {
            return Ok(());
        }
        let shared = Rc::new(RefCell::new(Vec::new()));
        let mut w =
            Writer::new(SyncWriter(Rc::clone(&shared)), level).map_err(|err| err.to_string())?;
        let mut r = Reader::new(SyncReader {
            shared: Rc::clone(&shared),
            pending: Vec::new(),
            pos: 0,
            taken: 0,
        });
        let half = input.len().div_ceil(2);
        for (i, (lo, hi)) in [(0, half), (half, input.len())].into_iter().enumerate() {
            let part = input.get(lo..hi).unwrap_or_default();
            put(&mut w, part).map_err(|err| format!("testSync: write: {err}"))?;
            if i == 0 {
                w.flush().map_err(|err| format!("testSync: flush: {err}"))?;
            } else {
                w.close().map_err(|err| format!("testSync: close: {err}"))?;
            }
            // Go: io.ReadAtLeast(r, out, hi-lo) with a buffer one byte larger.
            let mut out = vec![0; hi - lo + 1];
            let mut m = 0;
            while m < hi - lo {
                let (n, err) = r.read(out.get_mut(m..).unwrap_or_default());
                m += n;
                if let Some(err) = err
                    && m < hi - lo
                {
                    return Err(format!(
                        "testSync/{i} ({level}, {}): read {m}: {err}",
                        input.len()
                    ));
                }
            }
            if m != hi - lo || out.get(..m) != Some(part) {
                return Err(format!(
                    "testSync/{i} ({level}, {}): read {m} wrong bytes",
                    input.len()
                ));
            }
            if i == 0 && r.get_ref().unread() != 0 {
                return Err(format!(
                    "testSync/{i} ({level}, {}): extra data after {}",
                    input.len(),
                    hi - lo
                ));
            }
        }
        let (n, last) = r.read(&mut [0; 10]);
        if n > 0 || last != Some(GzipError::Eof) {
            return Err(format!(
                "testSync ({level}, {}): final Read: {n}, {last:?}",
                input.len()
            ));
        }
        if r.get_ref().unread() != 0 {
            return Err(format!(
                "testSync ({level}, {}): extra data at end",
                input.len()
            ));
        }
        // Go: the stream should work for an ordinary reader too.
        let stream = shared.borrow().clone();
        let (data, read_err) = inflate(&stream, &[]);
        if let Some(err) = read_err {
            return Err(format!("testSync: read: {err}"));
        }
        if data != input {
            return Err(format!(
                "testSync: decompress(compress(data)) != data: level={level}"
            ));
        }
        Ok(())
    }

    /// Go: `testToFromWithLevelAndLimit` without a limit.
    fn to_from(level: isize, input: &[u8]) -> Result<(), String> {
        let stream = deflate(level, input)?;
        let (out, err) = inflate(&stream, &[]);
        if let Some(err) = err {
            return Err(format!("read: {err}"));
        }
        if out != input {
            return Err(format!(
                "decompress(compress(data)) != data: level={level} input={} bytes",
                input.len()
            ));
        }
        sync(level, input)
    }

    /// Go: `TestDeflateInflate` (`deflateInflateTests` at levels 0 to 9 and -2).
    #[test]
    fn deflate_inflate() -> Result<(), String> {
        // Go: largeDataChunk.
        let large = gen_input("square:100000")?;
        let inputs: [&[u8]; 6] = [
            &[],
            &[0x11],
            &[0x11, 0x12],
            &[0x11; 8],
            &[
                0x11, 0x10, 0x13, 0x41, 0x21, 0x21, 0x41, 0x13, 0x87, 0x78, 0x13,
            ],
            &large,
        ];
        for input in inputs {
            for level in (0..=9).chain([-2]) {
                to_from(level, input)?;
            }
        }
        Ok(())
    }

    /// Go: `TestDeflateInflateString`, over the Gettysburg Address (Go uses `e.txt` and
    /// `Isaac.Newton-Opticks.txt` and checks compressed sizes, which the golden files cover).
    #[test]
    fn deflate_inflate_string() -> Result<(), String> {
        let input = gen_input("file:gettysburg.txt")?;
        for level in (0..=9).chain([-2]) {
            to_from(level, &input)?;
        }
        Ok(())
    }

    /// The stream of `body` after writing and flushing `dict` at level 5.
    fn after_dict(dict: &[u8], body: &[u8]) -> Result<Vec<u8>, String> {
        let mut w = Writer::new(Vec::new(), 5).map_err(|err| err.to_string())?;
        put(&mut w, dict)?;
        w.flush().map_err(|err| err.to_string())?;
        w.get_mut().clear();
        put(&mut w, body)?;
        w.close().map_err(|err| err.to_string())?;
        Ok(w.into_inner())
    }

    /// Go: `TestReaderDict`.
    #[test]
    fn reader_dict() -> Result<(), String> {
        let (dict, body) = (b"hello world", b"hello again world");
        let stream = after_dict(dict, body)?;
        let (data, err) = inflate(&stream, dict);
        if let Some(err) = err {
            return Err(err.to_string());
        }
        if data != body {
            return Err(format!("read returned {data:?} want {body:?}"));
        }
        Ok(())
    }

    /// Go: `TestWriterDict`.
    #[test]
    fn writer_dict() -> Result<(), String> {
        let (dict, body) = (b"hello world", b"hello again world");
        let want = after_dict(dict, body)?;
        let mut w = Writer::new_dict(Vec::new(), 5, dict).map_err(|err| err.to_string())?;
        put(&mut w, body)?;
        w.close().map_err(|err| err.to_string())?;
        let got = w.into_inner();
        if got != want {
            return Err(format!("writer wrote {got:?} want {want:?}"));
        }
        Ok(())
    }

    /// Go: `TestWriterReset`: a reset writer holds no tokens and writes what a new writer does.
    #[test]
    fn writer_reset() -> Result<(), String> {
        for level in 0..=9 {
            let mut w = Writer::new(Vec::new(), level).map_err(|err| err.to_string())?;
            for _ in 0..1024 {
                put(&mut w, b"hello world")?;
            }
            let _old = w.reset(Vec::new());
            if !w.d.tokens.is_empty() {
                return Err(format!(
                    "level {level} Writer not reset after Reset. {} tokens were present",
                    w.d.tokens.len()
                ));
            }
            let mut wref = Writer::new(Vec::new(), level).map_err(|err| err.to_string())?;
            for z in [&mut w, &mut wref] {
                put(z, b"hello world")?;
                z.close().map_err(|err| err.to_string())?;
            }
            if w.get_ref() != wref.get_ref() {
                return Err(format!("level {level} Writer not reset after Reset"));
            }
        }
        Ok(())
    }

    /// Go: `testResetOutput`'s `writeData`.
    fn write_data<W: io::Write>(w: &mut Writer<W>) -> Result<(), String> {
        put(w, b"now is the time for all good gophers")?;
        w.flush().map_err(|err| err.to_string())?;
        for _ in 0..1024 {
            put(w, b"hello world")?;
        }
        put(w, &b"x".repeat(65000))
    }

    /// Go: `TestWriterReset`'s `testResetOutput`: a reset writer writes what it wrote before
    /// the reset. (Go's "dict" subtests pass a nil dictionary too.)
    #[test]
    fn reset_output() -> Result<(), String> {
        for level in [0, 1, 2, 5, 9] {
            let mut w = Writer::new(Vec::new(), level).map_err(|err| err.to_string())?;
            write_data(&mut w)?;
            w.close().map_err(|err| err.to_string())?;
            let out1 = w.reset(Vec::new());
            write_data(&mut w)?;
            w.close().map_err(|err| err.to_string())?;
            let out2 = w.into_inner();
            if out1 != out2 {
                return Err(format!(
                    "level {level}: got {} bytes, expected {}",
                    out2.len(),
                    out1.len()
                ));
            }
        }
        Ok(())
    }

    /// Go: `TestBestSpeed`: pieces of `abcabc`, with and without a flush after each, round
    /// trip through `BestSpeed`.
    #[test]
    fn best_speed() -> Result<(), String> {
        let abcabc = gen_input("abc:131072")?;
        let test_cases: [&[usize]; 16] = [
            &[65536, 0],
            &[65536, 1],
            &[65536, 1, 256],
            &[65536, 1, 65536],
            &[65536, 14],
            &[65536, 15],
            &[65536, 16],
            &[65536, 16, 256],
            &[65536, 16, 65536],
            &[65536, 127],
            &[65536, 128],
            &[65536, 128, 256],
            &[65536, 128, 65536],
            &[65536, 129],
            &[65536, 65536, 256],
            &[65536, 65536, 65536],
        ];
        for (i, tc) in test_cases.iter().enumerate() {
            for first_n in [1, 65534, 65535, 65536, 65537, 131_072] {
                for flush in [false, true] {
                    let mut want = Vec::new();
                    let mut w =
                        Writer::new(Vec::new(), BEST_SPEED).map_err(|err| err.to_string())?;
                    for (j, &n) in tc.iter().enumerate() {
                        let piece = abcabc
                            .get(..if j == 0 { first_n } else { n })
                            .unwrap_or_default();
                        want.extend_from_slice(piece);
                        put(&mut w, piece)?;
                        if flush {
                            w.flush().map_err(|err| err.to_string())?;
                        }
                    }
                    w.close().map_err(|err| err.to_string())?;
                    let (got, err) = inflate(w.get_ref(), &[]);
                    if err.is_some() || got != want {
                        return Err(format!(
                            "i={i}, firstN={first_n}, flush={flush}: corruption during \
                             deflate-then-inflate ({err:?})"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether `r` failed with a failing [`Target`]'s error (Go's `errIO`).
    fn is_err_io(r: &Result<(), GzipError>) -> bool {
        r.as_ref()
            .err()
            .is_some_and(|err| err.to_string() == CLOSED_PIPE)
    }

    /// Go: `TestWriterPersistentWriteError`, over the first 10000 bytes of the fixture corpus
    /// (Go uses `Isaac.Newton-Opticks.txt`).
    #[test]
    fn writer_persistent_write_error() -> Result<(), String> {
        let d = gen_input("file:fixtures.json:10000")?;
        let mut zw =
            Writer::new(Target::default(), DEFAULT_COMPRESSION).map_err(|err| err.to_string())?;
        // Go: sweep over the threshold at which an error is returned: the ith call to the
        // writer fails and the later ones succeed, so the flate.Writer must keep the error.
        for i in 0..1000 {
            let _old = zw.reset(Target::failing(i, false));
            let werr = zw.write(&d).map(|_| ());
            let cerr = zw.close();
            let ferr = zw.flush();
            let failed = zw.get_ref().calls() > i;
            if werr.is_err() && !is_err_io(&werr) {
                return Err(format!(
                    "test {i}, mismatching Write error: got {werr:?}, want {CLOSED_PIPE}"
                ));
            }
            if failed && !is_err_io(&cerr) {
                return Err(format!(
                    "test {i}, mismatching Close error: got {cerr:?}, want {CLOSED_PIPE}"
                ));
            }
            if failed && !is_err_io(&ferr) {
                return Err(format!(
                    "test {i}, mismatching Flush error: got {ferr:?}, want {CLOSED_PIPE}"
                ));
            }
            if !failed {
                // Go: the failure threshold was high enough that the whole stream was written
                // without any errors.
                return Ok(());
            }
        }
        Ok(())
    }

    /// Go: `checkErrors`: every result is the same error, the failing call's.
    fn check_errors(got: &[Result<(), GzipError>]) -> Result<(), String> {
        let first = got.first().and_then(|r| r.as_ref().err());
        for r in got {
            match r {
                Err(err) if err.to_string() == CLOSED_PIPE && Some(err) == first => {}
                Ok(()) | Err(_) => {
                    return Err(format!(
                        "Error doesn't match\nWant: {CLOSED_PIPE}\nGot: {got:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Go: `TestWriterPersistentFlushError`.
    #[test]
    fn writer_persistent_flush_error() -> Result<(), String> {
        let mut zw = Writer::new(Target::failing(0, false), DEFAULT_COMPRESSION)
            .map_err(|err| err.to_string())?;
        let flush_err = zw.flush();
        let close_err = zw.close();
        let write_err = zw.write(b"Test").map(|_| ());
        check_errors(&[close_err, flush_err, write_err])
    }

    /// Go: `TestWriterPersistentCloseError`: an error from the underlying writer while closing
    /// persists across all writer calls.
    #[test]
    fn writer_persistent_close_error() -> Result<(), String> {
        let mut zw = Writer::new(Target::failing(0, false), DEFAULT_COMPRESSION)
            .map_err(|err| err.to_string())?;
        let close_err = zw.close();
        let flush_err = zw.flush();
        let write_err = zw.write(b"Test").map(|_| ());
        check_errors(&[close_err, flush_err, write_err])?;

        // Go: after closing the writer, "write after close" persists across Flush and Write
        // calls, while later Close calls return nil.
        let _old = zw.reset(Target::default());
        zw.close()
            .map_err(|err| format!("First call to close returned error: {err}"))?;
        zw.close()
            .map_err(|err| format!("Second call to close returned error: {err}"))?;
        let closed = [zw.flush(), zw.write(b"Test").map(|_| ())];
        if closed.iter().any(|r| *r != Err(GzipError::WriterClosed)) {
            return Err(format!(
                "Error doesn't match\nWant: flate: closed writer\nGot: {closed:?}"
            ));
        }
        Ok(())
    }

    /// Go: `writer_test.go` `TestWriteError`: the underlying writer's error is passed upwards
    /// and kept until a reset.
    #[test]
    fn write_error() -> Result<(), String> {
        let mut input = Vec::new();
        for i in 0..65536 * 4 {
            input.extend_from_slice(format!("asdasfasf{i}{i}fghfgujyut{i}yutyu\n").as_bytes());
        }
        for l in 0..10 {
            for fail in (0..9).map(|k| 1 << k) {
                // Go: errorWriter fails every call after `fail` writes.
                let mut w =
                    Writer::new(Target::failing(fail, true), l).map_err(|err| err.to_string())?;
                // Go: io.CopyBuffer with a 128-byte buffer stops at the first error.
                if input
                    .chunks(128)
                    .try_for_each(|chunk| put(&mut w, chunk))
                    .is_ok()
                {
                    return Err(format!("Level {l}: Expected an error"));
                }
                if w.write(&[1, 2, 2, 3, 4, 5]).is_ok() {
                    return Err(format!("Level {l}: Expected an error"));
                }
                if w.flush().is_ok() {
                    return Err(format!("Level {l}: Expected an error on flush"));
                }
                if w.close().is_ok() {
                    return Err(format!("Level {l}: Expected an error on close"));
                }
                let _old = w.reset(Target::default());
                match w.write(&[1, 2, 3, 4, 5, 6]) {
                    Ok(0) => return Err(format!("Level {l}: Got 0 length write, expected > 0")),
                    Ok(_) => {}
                    Err(err) => {
                        return Err(format!(
                            "Level {l}: Got unexpected error after reset: {err}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Go: `writer_test.go` `TestDeflateFast_Reset` (with `-short`'s single offset of 256): the
    /// output of level 1 is the same right before the match offsets wrap around.
    #[test]
    fn deflate_fast_reset() -> Result<(), String> {
        // Go: bufferReset.
        const BUFFER_RESET: usize = 0x7FFF_FFFF - MAX_STORE_BLOCK_SIZE * 2;
        let mut input = Vec::new();
        for i in 0..65536 {
            input.extend_from_slice(
                format!("asdfasdfasdfasdf{i}{i}fghfgujyut{i}yutyu\n").as_bytes(),
            );
        }
        // Go: an encode with a clean writer to compare, the input written 3 times.
        let mut clean = Writer::new(Vec::new(), BEST_SPEED).map_err(|err| err.to_string())?;
        for _ in 0..3 {
            put(&mut clean, &input)?;
        }
        clean.close().map_err(|err| err.to_string())?;
        let want = clean.into_inner();

        let offset = 256;
        let mut w = Writer::new(Vec::new(), BEST_SPEED).map_err(|err| err.to_string())?;
        // Go: reset until right before the wraparound; each reset adds maxMatchOffset.
        for _ in 0..(BUFFER_RESET - input.len() - offset - MAX_MATCH_OFFSET) / MAX_MATCH_OFFSET {
            w.d.reset();
        }
        let _old = w.reset(Vec::new());
        for _ in 0..3 {
            put(&mut w, &input)?;
        }
        w.close().map_err(|err| err.to_string())?;
        if *w.get_ref() != want {
            return Err(format!(
                "output did not match at wraparound, len(want) = {}, len(got) = {}",
                want.len(),
                w.get_ref().len()
            ));
        }
        Ok(())
    }
}
