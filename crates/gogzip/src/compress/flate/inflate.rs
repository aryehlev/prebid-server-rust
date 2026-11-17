//! Go: `compress/flate/inflate.go`: the decompressor and the public [`Reader`].

use core::{fmt, mem, ops::Range};
use std::{io::BufRead, sync::LazyLock};

use super::{
    super::goio::{self, no_eof},
    deflate::MAX_MATCH_OFFSET,
    dict_decoder::DictDecoder,
    huffman_bit_writer::END_BLOCK_MARKER,
};
use crate::GzipError;

/// Go: `maxCodeLen`, the maximum length of a Huffman code.
const MAX_CODE_LEN: usize = 16;
/// Go: `maxNumLit`, the number of literal/length codes (RFC 1951 3.2.7).
pub(super) const MAX_NUM_LIT: usize = 286;
/// Go: `maxNumDist`, the number of distance codes.
const MAX_NUM_DIST: usize = 30;
/// Go: `numCodes`, the number of codes in the Huffman meta-code.
const NUM_CODES: usize = 19;

/// Go: `huffmanChunkBits`, the width of the first-level lookup table.
const HUFFMAN_CHUNK_BITS: u32 = 9;
/// Go: `huffmanNumChunks`.
const HUFFMAN_NUM_CHUNKS: usize = 1 << HUFFMAN_CHUNK_BITS;
/// Go: `huffmanCountMask`: `chunk & 15` is the number of bits.
const HUFFMAN_COUNT_MASK: u32 = 15;
/// Go: `huffmanValueShift`: `chunk >> 4` is the value, including a table link.
const HUFFMAN_VALUE_SHIFT: u32 = 4;

/// Go: `codeOrder`, the order of the code length code lengths (RFC 1951 3.2.7).
static CODE_ORDER: [usize; NUM_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Go: `huffmanDecoder`: a zlib-style table of 9-bit chunks, with overflow link tables for
/// longer codes.
#[derive(Clone, Debug)]
struct HuffmanDecoder {
    /// Go: the minimum code length.
    min: usize,
    chunks: [u32; HUFFMAN_NUM_CHUNKS],
    /// Go: `links`, the overflow tables of `link_len` entries each, back to back.
    links: Vec<u32>,
    link_len: usize,
    /// Go: mask the width of the link table.
    link_mask: u32,
}

impl HuffmanDecoder {
    /// Go: the zero `huffmanDecoder`.
    const fn new() -> Self {
        Self {
            min: 0,
            chunks: [0; HUFFMAN_NUM_CHUNKS],
            links: Vec::new(),
            link_len: 0,
            link_mask: 0,
        }
    }

    /// Go: `huffmanDecoder.init`: the decoding tables for the code `lengths`. Reports whether
    /// the code is complete (or the degenerate single code of length 1, or empty).
    fn init(&mut self, lengths: &[u8]) -> bool {
        if self.min != 0 {
            *self = Self::new();
        }

        // Go: count number of codes of each length, compute min and max length.
        let mut count = [0usize; MAX_CODE_LEN];
        let mut min = 0;
        let mut max = 0;
        for &n in lengths {
            let n = usize::from(n);
            if n == 0 {
                continue;
            }
            if min == 0 || n < min {
                min = n;
            }
            if n > max {
                max = n;
            }
            if let Some(c) = count.get_mut(n) {
                *c += 1;
            }
        }

        // Go: empty tree. The decompressor.huffSym function will fail later if the tree is
        // used.
        if max == 0 {
            return true;
        }

        let mut code = 0;
        let mut nextcode = [0usize; MAX_CODE_LEN];
        for i in min..=max {
            code <<= 1;
            if let Some(slot) = nextcode.get_mut(i) {
                *slot = code;
            }
            code += count.get(i).copied().unwrap_or(0);
        }

        // Go: check that the coding is complete. Exception: to be compatible with zlib, we
        // also need to accept degenerate single-code codings.
        if code != 1 << max && !(code == 1 && max == 1) {
            return false;
        }

        self.min = min;
        let chunk_bits = HUFFMAN_CHUNK_BITS as usize;
        if max > chunk_bits {
            let num_links = 1usize << (max - chunk_bits);
            self.link_mask = u32::try_from(num_links - 1).unwrap_or(0);

            // Go: create link tables.
            let link = nextcode.get(chunk_bits + 1).copied().unwrap_or(0) >> 1;
            self.link_len = num_links;
            self.links = vec![0; HUFFMAN_NUM_CHUNKS.saturating_sub(link) * num_links];
            for j in link..HUFFMAN_NUM_CHUNKS {
                let reverse = reverse_low(j, chunk_bits);
                let off = j - link;
                if let Some(slot) = self.chunks.get_mut(reverse) {
                    *slot = chunk_value(off, chunk_bits + 1);
                }
            }
        }

        for (i, &n) in lengths.iter().enumerate() {
            let n = usize::from(n);
            if n == 0 {
                continue;
            }
            let Some(next) = nextcode.get_mut(n) else {
                continue;
            };
            let symbol_code = *next;
            *next += 1;
            let chunk = chunk_value(i, n);
            let reverse = reverse_low(symbol_code, n);
            if n <= chunk_bits {
                for off in (reverse..HUFFMAN_NUM_CHUNKS).step_by(1 << n) {
                    if let Some(slot) = self.chunks.get_mut(off) {
                        *slot = chunk;
                    }
                }
            } else {
                let j = reverse & (HUFFMAN_NUM_CHUNKS - 1);
                let value = self.chunks.get(j).map_or(0, |&c| c >> HUFFMAN_VALUE_SHIFT) as usize;
                let start = value * self.link_len;
                let Some(linktab) = self.links.get_mut(start..start + self.link_len) else {
                    continue;
                };
                let reverse = reverse >> chunk_bits;
                for off in (reverse..linktab.len()).step_by(1 << (n - chunk_bits)) {
                    if let Some(slot) = linktab.get_mut(off) {
                        *slot = chunk;
                    }
                }
            }
        }
        true
    }
}

/// Go: `int(bits.Reverse16(uint16(code))) >> (16 - n)`: the low `n` bits of `code`, reversed.
fn reverse_low(code: usize, n: usize) -> usize {
    let reversed = usize::from(u16::try_from(code & 0xFFFF).unwrap_or(0).reverse_bits());
    reversed >> (16 - n)
}

/// Go: `uint32(value<<huffmanValueShift | count)`.
fn chunk_value(value: usize, count: usize) -> u32 {
    u32::try_from(value << HUFFMAN_VALUE_SHIFT | count).unwrap_or(0)
}

/// Go: `fixedHuffmanDecoder`, the RFC 1951 3.2.6 literal/length code.
static FIXED_HUFFMAN_DECODER: LazyLock<HuffmanDecoder> = LazyLock::new(|| {
    let bits: Vec<u8> = (0..288)
        .map(|i| match i {
            0..144 | 280.. => 8,
            144..256 => 9,
            _ => 7,
        })
        .collect();
    let mut h = HuffmanDecoder::new();
    let _complete = h.init(&bits);
    h
});

/// The input half of Go's `decompressor`: the bit buffer and the input offset.
#[derive(Clone, Copy, Debug, Default)]
struct Bits {
    /// Go: `roffset`, the number of input bytes read.
    roffset: i64,
    /// Go: input bits, in the low end of `b`.
    b: u32,
    nb: u32,
}

impl Bits {
    /// Go: `decompressor.moreBits`.
    fn more_bits<R: BufRead + ?Sized>(&mut self, input: &mut R) -> Result<(), GzipError> {
        let c = goio::read_byte(input).map_err(no_eof)?;
        self.roffset += 1;
        self.b |= u32::from(c).checked_shl(self.nb).unwrap_or(0);
        self.nb += 8;
        Ok(())
    }

    /// Reads until `n` bits are buffered (Go: `for f.nb < n { f.moreBits() }`).
    fn need<R: BufRead + ?Sized>(&mut self, input: &mut R, n: u32) -> Result<(), GzipError> {
        while self.nb < n {
            self.more_bits(input)?;
        }
        Ok(())
    }

    /// Takes the low `n` buffered bits.
    const fn take(&mut self, n: u32) -> u32 {
        let v = self.b & ((1 << n) - 1);
        self.b >>= n;
        self.nb -= n;
        v
    }

    /// Go: `decompressor.huffSym`: the next symbol of `decoder`.
    fn huff_sym<R: BufRead + ?Sized>(
        &mut self,
        input: &mut R,
        decoder: &HuffmanDecoder,
    ) -> Result<usize, GzipError> {
        // Go: since a huffmanDecoder can be empty or be composed of a degenerate tree with
        // single element, huffSym must error on these two edge cases. In both cases, the chunks
        // slice will be 0 for the invalid sequence, leading it satisfy the n == 0 check below.
        let mut n = u32::try_from(decoder.min).unwrap_or(u32::MAX);
        let mut nb = self.nb;
        let mut b = self.b;
        loop {
            while nb < n {
                match goio::read_byte(input) {
                    Ok(next) => {
                        self.roffset += 1;
                        b |= u32::from(next) << (nb & 31);
                        nb += 8;
                    }
                    Err(err) => {
                        self.b = b;
                        self.nb = nb;
                        return Err(no_eof(err));
                    }
                }
            }
            let mut chunk = decoder
                .chunks
                .get((b as usize) & (HUFFMAN_NUM_CHUNKS - 1))
                .copied()
                .unwrap_or(0);
            n = chunk & HUFFMAN_COUNT_MASK;
            if n > HUFFMAN_CHUNK_BITS {
                let table = (chunk >> HUFFMAN_VALUE_SHIFT) as usize;
                let index = ((b >> HUFFMAN_CHUNK_BITS) & decoder.link_mask) as usize;
                chunk = decoder
                    .links
                    .get(table * decoder.link_len + index)
                    .copied()
                    .unwrap_or(0);
                n = chunk & HUFFMAN_COUNT_MASK;
            }
            if n <= nb {
                if n == 0 {
                    self.b = b;
                    self.nb = nb;
                    return Err(GzipError::CorruptInput(self.roffset));
                }
                self.b = b >> (n & 31);
                self.nb = nb - n;
                return Ok((chunk >> HUFFMAN_VALUE_SHIFT) as usize);
            }
        }
    }
}

/// Go: the `step` functions of `decompressor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Go: `nextBlock`.
    NextBlock,
    /// Go: `huffmanBlock`.
    HuffmanBlock,
    /// Go: `copyData`.
    CopyData,
}

/// Go: `huffmanBlock`'s `stepState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepState {
    /// Go: `stateInit`, read the next literal.
    Init,
    /// Go: `stateDict`, continue a copy from the history.
    Dict,
}

/// Go: the `hl` and `hd` decoders of the current block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    /// Go: `hl = &fixedHuffmanDecoder` and `hd = nil`.
    Fixed,
    /// Go: `hl = &f.h1` and `hd = &f.h2`.
    Dynamic,
}

/// Go: `decompressor`. The input comes from the reader each call is given (Go's `r`).
pub(in crate::compress) struct Decompressor {
    bits: Bits,
    /// Go: Huffman decoders for literal/length, distance.
    h1: HuffmanDecoder,
    h2: HuffmanDecoder,
    /// Go: `bits`, the code lengths of a dynamic block.
    lens: Box<[u8; MAX_NUM_LIT + MAX_NUM_DIST]>,
    /// Go: `codebits`, the code length code lengths.
    codebits: [u8; NUM_CODES],
    /// Go: output history, buffer.
    dict: DictDecoder,
    /// Go: temporary buffer.
    buf: [u8; 4],
    step: Step,
    step_state: StepState,
    /// Go: `final`.
    final_block: bool,
    err: Option<GzipError>,
    /// Go: `toRead`, as a range of the history.
    to_read: Range<usize>,
    block: Block,
    copy_len: usize,
    copy_dist: usize,
}

impl fmt::Debug for Decompressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decompressor")
            .field("roffset", &self.bits.roffset)
            .field("step", &self.step)
            .field("err", &self.err)
            .finish_non_exhaustive()
    }
}

impl Decompressor {
    /// Go: `flate.NewReaderDict` (`NewReader` with an empty `dict`).
    pub(in crate::compress) fn new(dict: &[u8]) -> Self {
        let mut f = Self {
            bits: Bits::default(),
            h1: HuffmanDecoder::new(),
            h2: HuffmanDecoder::new(),
            lens: Box::new([0; MAX_NUM_LIT + MAX_NUM_DIST]),
            codebits: [0; NUM_CODES],
            dict: DictDecoder::default(),
            buf: [0; 4],
            step: Step::NextBlock,
            step_state: StepState::Init,
            final_block: false,
            err: None,
            to_read: 0..0,
            block: Block::Fixed,
            copy_len: 0,
            copy_dist: 0,
        };
        f.dict.init(MAX_MATCH_OFFSET, dict);
        f
    }

    /// Go: `decompressor.Reset(r, dict)`: the state of a new reader, keeping the allocations
    /// (Go keeps `bits`, `codebits`, `h1`, `h2` and `dict`, which every block initializes
    /// before use).
    pub(in crate::compress) fn reset(&mut self, dict: &[u8]) {
        self.bits = Bits::default();
        self.buf = [0; 4];
        self.step = Step::NextBlock;
        self.step_state = StepState::Init;
        self.final_block = false;
        self.err = None;
        self.to_read = 0..0;
        self.block = Block::Fixed;
        self.copy_len = 0;
        self.copy_dist = 0;
        self.dict.init(MAX_MATCH_OFFSET, dict);
    }

    /// Go: `decompressor.Read`.
    pub(in crate::compress) fn read<R: BufRead + ?Sized>(
        &mut self,
        input: &mut R,
        b: &mut [u8],
    ) -> (usize, Option<GzipError>) {
        loop {
            if !self.to_read.is_empty() {
                let src = self.dict.bytes(self.to_read.clone());
                let n = src.len().min(b.len());
                if let (Some(dst), Some(src)) = (b.get_mut(..n), src.get(..n)) {
                    dst.copy_from_slice(src);
                }
                self.to_read.start += n;
                if self.to_read.is_empty() {
                    return (n, self.err.clone());
                }
                return (n, None);
            }
            if let Some(err) = &self.err {
                return (0, Some(err.clone()));
            }
            self.step(input);
            if self.err.is_some() && self.to_read.is_empty() {
                // Go: flush what's left in case of error.
                self.to_read = self.dict.read_flush();
            }
        }
    }

    /// Go: `decompressor.Close`.
    pub(in crate::compress) fn close(&self) -> Result<(), GzipError> {
        match &self.err {
            None | Some(GzipError::Eof) => Ok(()),
            Some(err) => Err(err.clone()),
        }
    }

    /// Go: `f.step(f)`.
    fn step<R: BufRead + ?Sized>(&mut self, input: &mut R) {
        match self.step {
            Step::NextBlock => self.next_block(input),
            Step::HuffmanBlock => self.huffman_block(input),
            Step::CopyData => self.copy_data(input),
        }
    }

    /// Go: `decompressor.nextBlock`.
    fn next_block<R: BufRead + ?Sized>(&mut self, input: &mut R) {
        if let Err(err) = self.bits.need(input, 1 + 2) {
            self.err = Some(err);
            return;
        }
        self.final_block = self.bits.take(1) == 1;
        match self.bits.take(2) {
            0 => self.data_block(input),
            1 => {
                // Go: compressed, fixed Huffman tables.
                self.block = Block::Fixed;
                self.huffman_block(input);
            }
            2 => {
                // Go: compressed, dynamic Huffman tables.
                self.err = self.read_huffman(input).err();
                if self.err.is_some() {
                    return;
                }
                self.block = Block::Dynamic;
                self.huffman_block(input);
            }
            _ => {
                // Go: 3 is reserved.
                self.err = Some(GzipError::CorruptInput(self.bits.roffset));
            }
        }
    }

    /// Go: `decompressor.readHuffman`: the code tables of a dynamic block (RFC 1951 3.2.7).
    fn read_huffman<R: BufRead + ?Sized>(&mut self, input: &mut R) -> Result<(), GzipError> {
        // Go: HLIT[5], HDIST[5], HCLEN[4].
        self.bits.need(input, 5 + 5 + 4)?;
        let nlit = (self.bits.b & 0x1F) as usize + 257;
        if nlit > MAX_NUM_LIT {
            return Err(GzipError::CorruptInput(self.bits.roffset));
        }
        self.bits.b >>= 5;
        let ndist = (self.bits.b & 0x1F) as usize + 1;
        if ndist > MAX_NUM_DIST {
            return Err(GzipError::CorruptInput(self.bits.roffset));
        }
        self.bits.b >>= 5;
        // Go: numCodes is 19, so nclen is always valid.
        let nclen = (self.bits.b & 0xF) as usize + 4;
        self.bits.b >>= 4;
        self.bits.nb -= 5 + 5 + 4;

        // Go: (HCLEN+4)*3 bits: code lengths in the magic codeOrder order.
        for (i, &order) in CODE_ORDER.iter().enumerate() {
            let len = if i < nclen {
                self.bits.need(input, 3)?;
                self.bits.take(3)
            } else {
                0
            };
            if let Some(slot) = self.codebits.get_mut(order) {
                *slot = u8::try_from(len).unwrap_or(0);
            }
        }
        if !self.h1.init(&self.codebits) {
            return Err(GzipError::CorruptInput(self.bits.roffset));
        }

        // Go: HLIT + 257 code lengths, HDIST + 1 code lengths, using the code length Huffman
        // code.
        let n = nlit + ndist;
        let mut i = 0;
        while i < n {
            let x = self.bits.huff_sym(input, &self.h1)?;
            if x < 16 {
                // Go: actual length.
                if let Some(slot) = self.lens.get_mut(i) {
                    *slot = u8::try_from(x).unwrap_or(0);
                }
                i += 1;
                continue;
            }
            // Go: repeat previous length or zero.
            let (rep, nb, b) = match x {
                16 => {
                    if i == 0 {
                        return Err(GzipError::CorruptInput(self.bits.roffset));
                    }
                    (3, 2, self.lens.get(i - 1).copied().unwrap_or(0))
                }
                17 => (3, 3, 0),
                18 => (11, 7, 0),
                _ => return Err(GzipError::Internal("unexpected length code")),
            };
            self.bits.need(input, nb)?;
            let rep = rep + self.bits.take(nb) as usize;
            if i + rep > n {
                return Err(GzipError::CorruptInput(self.bits.roffset));
            }
            for slot in self.lens.iter_mut().skip(i).take(rep) {
                *slot = b;
            }
            i += rep;
        }

        let lit = self.lens.get(..nlit).unwrap_or_default();
        let dist = self.lens.get(nlit..nlit + ndist).unwrap_or_default();
        if !self.h1.init(lit) || !self.h2.init(dist) {
            return Err(GzipError::CorruptInput(self.bits.roffset));
        }

        // Go: as an optimization, we can initialize the min bits to read at a time for the
        // HLIT tree to the length of the EOB marker since we know that every block must
        // terminate with one. This preserves the property that we never read any extra bytes
        // after the end of the DEFLATE stream.
        let eob = usize::from(self.lens.get(END_BLOCK_MARKER).copied().unwrap_or(0));
        if self.h1.min < eob {
            self.h1.min = eob;
        }
        Ok(())
    }

    /// Go: `decompressor.huffmanBlock`: decodes a Huffman block (RFC 1951 3.2.3), pausing
    /// whenever the history window is full.
    fn huffman_block<R: BufRead + ?Sized>(&mut self, input: &mut R) {
        // Go: switch f.stepState { case stateInit: goto readLiteral; case stateDict: goto
        // copyHistory }.
        let mut copy_history = self.step_state == StepState::Dict;
        loop {
            if !copy_history {
                match self.read_literal(input) {
                    Literal::Byte => continue,
                    Literal::Copy => {}
                    Literal::Pause => return,
                }
            }
            copy_history = false;

            // Go: perform a backwards copy according to RFC section 3.2.3.
            let mut cnt = self.dict.try_write_copy(self.copy_dist, self.copy_len);
            if cnt == 0 {
                cnt = self.dict.write_copy(self.copy_dist, self.copy_len);
            }
            self.copy_len -= cnt;

            if self.dict.avail_write() == 0 || self.copy_len > 0 {
                self.to_read = self.dict.read_flush();
                // Go: we need to continue this work.
                self.step = Step::HuffmanBlock;
                self.step_state = StepState::Dict;
                return;
            }
        }
    }

    /// Go: `huffmanBlock`'s `readLiteral`: decodes one literal into the history, or one
    /// length and distance into `copy_len` and `copy_dist`.
    fn read_literal<R: BufRead + ?Sized>(&mut self, input: &mut R) -> Literal {
        let decoded = match self.block {
            Block::Fixed => self.bits.huff_sym(input, &FIXED_HUFFMAN_DECODER),
            Block::Dynamic => self.bits.huff_sym(input, &self.h1),
        };
        let v = match decoded {
            Ok(v) => v,
            Err(err) => return self.fail(err),
        };
        // Go: n is the number of extra bits.
        let (length, n) = match v {
            0..256 => {
                self.dict.write_byte(u8::try_from(v).unwrap_or(0));
                if self.dict.avail_write() == 0 {
                    self.to_read = self.dict.read_flush();
                    self.step = Step::HuffmanBlock;
                    self.step_state = StepState::Init;
                    return Literal::Pause;
                }
                return Literal::Byte;
            }
            256 => {
                self.finish_block();
                return Literal::Pause;
            }
            // Go: otherwise, reference to older data.
            257..265 => (v - (257 - 3), 0),
            265..269 => (v * 2 - (265 * 2 - 11), 1),
            269..273 => (v * 4 - (269 * 4 - 19), 2),
            273..277 => (v * 8 - (273 * 8 - 35), 3),
            277..281 => (v * 16 - (277 * 16 - 67), 4),
            281..285 => (v * 32 - (281 * 32 - 131), 5),
            285..MAX_NUM_LIT => (258, 0),
            _ => return self.fail(GzipError::CorruptInput(self.bits.roffset)),
        };
        let mut length = length;
        if n > 0 {
            if let Err(err) = self.bits.need(input, n) {
                return self.fail(err);
            }
            length += self.bits.take(n) as usize;
        }

        let dist = match self.block {
            Block::Fixed => {
                if let Err(err) = self.bits.need(input, 5) {
                    return self.fail(err);
                }
                // Go: bits.Reverse8(uint8(f.b & 0x1F << 3)).
                let code = u8::try_from(self.bits.take(5) << 3).unwrap_or(0);
                usize::from(code.reverse_bits())
            }
            Block::Dynamic => match self.bits.huff_sym(input, &self.h2) {
                Ok(dist) => dist,
                Err(err) => return self.fail(err),
            },
        };

        let dist = match dist {
            0..4 => dist + 1,
            4..MAX_NUM_DIST => {
                let nb = u32::try_from((dist - 2) >> 1).unwrap_or(0);
                // Go: have 1 bit in bottom of dist, need nb more.
                let extra = (dist & 1) << nb;
                if let Err(err) = self.bits.need(input, nb) {
                    return self.fail(err);
                }
                let extra = extra | self.bits.take(nb) as usize;
                (1 << (nb + 1)) + 1 + extra
            }
            _ => return self.fail(GzipError::CorruptInput(self.bits.roffset)),
        };

        // Go: no check on length; encoding can be prescient.
        if dist > self.dict.hist_size() {
            return self.fail(GzipError::CorruptInput(self.bits.roffset));
        }

        self.copy_len = length;
        self.copy_dist = dist;
        Literal::Copy
    }

    /// Go: `f.err = err; return` inside `huffmanBlock`.
    fn fail(&mut self, err: GzipError) -> Literal {
        self.err = Some(err);
        Literal::Pause
    }

    /// Go: `decompressor.dataBlock`: starts a stored block.
    fn data_block<R: BufRead + ?Sized>(&mut self, input: &mut R) {
        // Go: uncompressed. Discard current half-byte.
        self.bits.nb = 0;
        self.bits.b = 0;

        // Go: length then ones-complement of length.
        let (nr, err) = goio::read_full(input, &mut self.buf);
        self.bits.roffset += i64::try_from(nr).unwrap_or(0);
        if let Some(err) = err {
            self.err = Some(no_eof(err));
            return;
        }
        let [n0, n1, nn0, nn1] = self.buf;
        let n = u16::from_le_bytes([n0, n1]);
        let nn = u16::from_le_bytes([nn0, nn1]);
        if nn != !n {
            self.err = Some(GzipError::CorruptInput(self.bits.roffset));
            return;
        }

        if n == 0 {
            self.to_read = self.dict.read_flush();
            self.finish_block();
            return;
        }

        self.copy_len = usize::from(n);
        self.copy_data(input);
    }

    /// Go: `decompressor.copyData`: copies `copy_len` bytes of a stored block from the input
    /// into the history, pausing when the history is full.
    fn copy_data<R: BufRead + ?Sized>(&mut self, input: &mut R) {
        let buf = self.dict.write_slice();
        let len = buf.len().min(self.copy_len);
        let (cnt, err) = goio::read_full(input, buf.get_mut(..len).unwrap_or_default());
        self.bits.roffset += i64::try_from(cnt).unwrap_or(0);
        self.copy_len -= cnt;
        self.dict.write_mark(cnt);
        if let Some(err) = err {
            self.err = Some(no_eof(err));
            return;
        }

        if self.dict.avail_write() == 0 || self.copy_len > 0 {
            self.to_read = self.dict.read_flush();
            self.step = Step::CopyData;
            return;
        }
        self.finish_block();
    }

    /// Go: `decompressor.finishBlock`.
    fn finish_block(&mut self) {
        if self.final_block {
            if self.dict.avail_read() > 0 {
                self.to_read = self.dict.read_flush();
            }
            self.err = Some(GzipError::Eof);
        }
        self.step = Step::NextBlock;
    }
}

/// What `read_literal` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Literal {
    /// Go: `goto readLiteral`.
    Byte,
    /// Go: `goto copyHistory`.
    Copy,
    /// Go: `return` from `huffmanBlock`.
    Pause,
}

/// Go: the `io.ReadCloser` that `flate.NewReader` returns: reads the decompressed form of a
/// DEFLATE (RFC 1951) stream, with Go's results and error texts.
///
/// Go reads the input through `io.ByteReader`, wrapping a reader that has no `ReadByte` in a
/// `bufio.Reader`; this port reads a [`BufRead`], so the caller picks the buffering (for
/// example `BufReader::with_capacity(4096, r)`, Go's `bufio` default, or a byte slice). The
/// decompressor never reads past the end of the DEFLATE stream from a [`BufRead`].
pub struct Reader<R> {
    r: R,
    f: Box<Decompressor>,
}

impl<R> fmt::Debug for Reader<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("f", &self.f)
            .finish_non_exhaustive()
    }
}

impl<R: BufRead> Reader<R> {
    /// Go: `flate.NewReader(r)`.
    pub fn new(r: R) -> Self {
        Self {
            r,
            f: Box::new(Decompressor::new(&[])),
        }
    }

    /// Go: `flate.NewReaderDict(r, dict)`: as if the decompressed data started with `dict`
    /// (which is not returned).
    pub fn new_dict(r: R, dict: &[u8]) -> Self {
        Self {
            r,
            f: Box::new(Decompressor::new(dict)),
        }
    }

    /// Go: `Read(b)`: decompresses into `b` and returns Go's `(n, err)` pair. `err` is
    /// [`GzipError::Eof`] once the final block has been read, possibly together with the last
    /// bytes, and is sticky.
    pub fn read(&mut self, b: &mut [u8]) -> (usize, Option<GzipError>) {
        self.f.read(&mut self.r, b)
    }

    /// Go: `Close`: the decompression error, if any (end of stream is not an error). The
    /// underlying reader is not closed.
    ///
    /// # Errors
    ///
    /// The error that stopped decompression.
    pub fn close(&mut self) -> Result<(), GzipError> {
        self.f.close()
    }

    /// Go: `Resetter.Reset(r, dict)`: the state of [`Reader::new_dict`] with `r` and `dict`,
    /// keeping the allocations. Returns the previous reader.
    pub fn reset(&mut self, r: R, dict: &[u8]) -> R {
        let old = mem::replace(&mut self.r, r);
        self.f.reset(dict);
        old
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
    //! Ports of Go's `flate_test.go` and `inflate_test.go` tests of the decoder tables and of
    //! `Reset`. The streams of `TestStreams`, `TestTruncatedStreams`, `TestReaderTruncated` and
    //! `TestNlitOutOfRange` are replayed from the golden files with Go's results. Go's
    //! `TestReaderReusesReaderBuffer` checks the `bufio.Reader` Go wraps around readers without
    //! `ReadByte`, which this port leaves to the caller.

    use std::io::BufRead;

    use super::{Bits, HuffmanDecoder, Reader};
    use crate::{
        GzipError,
        compress::flate::{DEFAULT_COMPRESSION, Writer},
    };

    /// Go: `TestIssue5915`.
    #[test]
    fn issue5915() -> Result<(), String> {
        let bits = [
            4, 0, 0, 6, 4, 3, 2, 3, 3, 4, 4, 5, 0, 0, 0, 0, 5, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 8, 6, 0, 11, 0,
            8, 0, 6, 6, 10, 8,
        ];
        let mut h = HuffmanDecoder::new();
        if h.init(&bits) {
            return Err("Given sequence of bits is bad, and should not succeed.".to_owned());
        }
        Ok(())
    }

    /// Go: `TestIssue5962`.
    #[test]
    fn issue5962() -> Result<(), String> {
        let bits = [
            4, 0, 0, 6, 4, 3, 2, 3, 3, 4, 4, 5, 0, 0, 0, 0, 5, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            11,
        ];
        let mut h = HuffmanDecoder::new();
        if h.init(&bits) {
            return Err("Given sequence of bits is bad, and should not succeed.".to_owned());
        }
        Ok(())
    }

    /// Go: `TestIssue6255`.
    #[test]
    fn issue6255() -> Result<(), String> {
        let mut h = HuffmanDecoder::new();
        if !h.init(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 11]) {
            return Err("Given sequence of bits is good and should succeed.".to_owned());
        }
        if h.init(&[11, 13]) {
            return Err("Given sequence of bits is bad and should not succeed.".to_owned());
        }
        Ok(())
    }

    /// Go: `TestInvalidEncoding`.
    #[test]
    fn invalid_encoding() -> Result<(), String> {
        // Go: initialize the Huffman decoder to recognize "0".
        let mut h = HuffmanDecoder::new();
        if !h.init(&[1]) {
            return Err("Failed to initialize Huffman decoder".to_owned());
        }
        // Go: decode an invalid Huffman coding.
        let mut bits = Bits::default();
        let mut input: &[u8] = &[0xFF];
        if bits.huff_sym(&mut input, &h).is_ok() {
            return Err("Should have rejected invalid bit sequence".to_owned());
        }
        Ok(())
    }

    /// Go: `TestInvalidBits`.
    #[test]
    fn invalid_bits() -> Result<(), String> {
        let mut h = HuffmanDecoder::new();
        if h.init(&[1, 2, 3, 4, 4, 5]) {
            return Err("Should reject oversubscribed bit-length set".to_owned());
        }
        if h.init(&[1, 2, 4, 4]) {
            return Err("Should reject incomplete bit-length set".to_owned());
        }
        Ok(())
    }

    /// Compresses each of `ss`.
    fn deflated(ss: &[&str], level: isize, dict: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        ss.iter()
            .map(|s| {
                let mut w = Writer::new_dict(Vec::new(), level, dict).map_err(|e| e.to_string())?;
                let _n = w.write(s.as_bytes()).map_err(|e| e.to_string())?;
                w.close().map_err(|e| e.to_string())?;
                Ok(w.into_inner())
            })
            .collect()
    }

    /// Go: `io.Copy` of a reader into a buffer.
    fn copy<R: BufRead>(f: &mut Reader<R>) -> Result<String, String> {
        let mut out = Vec::new();
        let mut buf = [0; 7];
        loop {
            let (n, err) = f.read(&mut buf);
            out.extend_from_slice(buf.get(..n).unwrap_or_default());
            match err {
                None => {}
                Some(GzipError::Eof) => break,
                Some(err) => return Err(err.to_string()),
            }
        }
        String::from_utf8(out).map_err(|err| err.to_string())
    }

    /// Go: `TestReset`.
    #[test]
    fn reset() -> Result<(), String> {
        let ss = [
            "lorem ipsum izzle fo rizzle",
            "the quick brown fox jumped over",
        ];
        let deflated = deflated(&ss, 1, &[])?;
        let mut streams = deflated.iter().map(Vec::as_slice);
        let mut f = Reader::new(streams.next().unwrap_or_default());
        let mut inflated = vec![copy(&mut f)?];
        let _old = f.reset(streams.next().unwrap_or_default(), &[]);
        inflated.push(copy(&mut f)?);
        f.close().map_err(|err| err.to_string())?;
        if inflated != ss {
            return Err(format!("inflated {inflated:?}, want {ss:?}"));
        }
        Ok(())
    }

    /// Go: `TestResetDict`.
    #[test]
    fn reset_dict() -> Result<(), String> {
        let dict = b"the lorem fox";
        let ss = [
            "lorem ipsum izzle fo rizzle",
            "the quick brown fox jumped over",
        ];
        let deflated = deflated(&ss, DEFAULT_COMPRESSION, dict)?;
        let mut f = Reader::new(&[][..]);
        let mut inflated = Vec::new();
        for stream in &deflated {
            let _old = f.reset(stream.as_slice(), dict);
            inflated.push(copy(&mut f)?);
        }
        f.close().map_err(|err| err.to_string())?;
        if inflated != ss {
            return Err(format!("inflated {inflated:?}, want {ss:?}"));
        }
        Ok(())
    }
}
