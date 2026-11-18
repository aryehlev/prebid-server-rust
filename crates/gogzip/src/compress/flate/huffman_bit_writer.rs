//! Go: `compress/flate/huffman_bit_writer.go`: block encoding (stored, fixed or dynamic Huffman)
//! and the bit layout of the output.

use core::mem;
use std::{io::Write, iter, sync::LazyLock};

use super::{
    super::goio,
    huffman_code::{
        FIXED_LITERAL_ENCODING, FIXED_OFFSET_ENCODING, Hcode, HuffmanEncoder, bit_length,
    },
    inflate::MAX_NUM_LIT,
    token::{Token, length_code, offset_code},
};
use crate::GzipError;

/// Go: `offsetCodeCount`, the number of offset codes.
pub(super) const OFFSET_CODE_COUNT: usize = 30;
/// Go: `endBlockMarker`.
pub(super) const END_BLOCK_MARKER: usize = 256;
/// Go: `lengthCodesStart`.
const LENGTH_CODES_START: usize = 257;
/// Go: `codegenCodeCount`.
const CODEGEN_CODE_COUNT: usize = 19;
/// Go: `badCode`.
const BAD_CODE: u8 = 255;
/// Go: `bufferFlushSize`: the buffered bytes are written once they reach this many.
const BUFFER_FLUSH_SIZE: usize = 240;
/// Go: `bufferSize`, with room for one more 8-byte flush.
const BUFFER_SIZE: usize = BUFFER_FLUSH_SIZE + 8;
/// Go: `maxStoreBlockSize`.
pub(super) const MAX_STORE_BLOCK_SIZE: usize = 65535;

/// Go: `lengthExtraBits`, indexed by length code minus 257.
static LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, // 257
    0, 0, 0, 0, 0, 1, 1, 1, 1, 2, // 260
    2, 2, 2, 3, 3, 3, 3, 4, 4, 4, // 270
    4, 5, 5, 5, 5, 0, // 280
];

/// Go: `lengthBase`.
static LENGTH_BASE: [u32; 29] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128,
    160, 192, 224, 255,
];

/// Go: `offsetExtraBits`.
static OFFSET_EXTRA_BITS: [u8; OFFSET_CODE_COUNT] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Go: `offsetBase`.
static OFFSET_BASE: [u32; OFFSET_CODE_COUNT] = [
    0x00_0000, 0x00_0001, 0x00_0002, 0x00_0003, 0x00_0004, 0x00_0006, 0x00_0008, 0x00_000C,
    0x00_0010, 0x00_0018, 0x00_0020, 0x00_0030, 0x00_0040, 0x00_0060, 0x00_0080, 0x00_00C0,
    0x00_0100, 0x00_0180, 0x00_0200, 0x00_0300, 0x00_0400, 0x00_0600, 0x00_0800, 0x00_0C00,
    0x00_1000, 0x00_1800, 0x00_2000, 0x00_3000, 0x00_4000, 0x00_6000,
];

/// Go: `codegenOrder`, the order in which the code length code lengths are written.
static CODEGEN_ORDER: [usize; CODEGEN_CODE_COUNT] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Go: `huffOffset`, the offset encoder used by `writeBlockHuff` (one offset code of length 1).
static HUFF_OFFSET: LazyLock<HuffmanEncoder> = LazyLock::new(|| {
    let mut offset_freq = [0; OFFSET_CODE_COUNT];
    offset_freq[0] = 1;
    let mut huff_offset = HuffmanEncoder::new(OFFSET_CODE_COUNT);
    huff_offset.generate(&offset_freq, 15);
    huff_offset
});

/// The output half of Go's `huffmanBitWriter`: the bit accumulator, the byte buffer and the
/// sticky write error.
#[derive(Clone, Debug)]
struct Output {
    /// Go: data waiting to be written is `bytes[..nbytes]` and then the low `nbits` of `bits`.
    bits: u64,
    nbits: u32,
    bytes: [u8; BUFFER_SIZE],
    nbytes: usize,
    err: Option<GzipError>,
}

impl Output {
    /// Go: `huffmanBitWriter.write`: writes `bytes[..n]` unless an error is pending.
    fn write_buffered(&mut self, w: &mut dyn Write, n: usize) {
        if self.err.is_some() {
            return;
        }
        self.err = goio::write(w, self.bytes.get(..n).unwrap_or_default()).err();
    }

    /// Go: `huffmanBitWriter.write`.
    fn write(&mut self, w: &mut dyn Write, b: &[u8]) {
        if self.err.is_some() {
            return;
        }
        self.err = goio::write(w, b).err();
    }

    /// Moves 48 accumulated bits into the byte buffer (Go: the tail of `writeBits` and
    /// `writeCode`).
    fn store48(&mut self, w: &mut dyn Write) {
        let bits = self.bits;
        self.bits >>= 48;
        self.nbits -= 48;
        let mut n = self.nbytes;
        let [b0, b1, b2, b3, b4, b5, _, _] = bits.to_le_bytes();
        if let Some(dst) = self.bytes.get_mut(n..n + 6) {
            dst.copy_from_slice(&[b0, b1, b2, b3, b4, b5]);
        }
        n += 6;
        if n >= BUFFER_FLUSH_SIZE {
            self.write_buffered(w, n);
            n = 0;
        }
        self.nbytes = n;
    }

    /// Go: `huffmanBitWriter.writeBits`. `b` is sign extended, as Go's `uint64(b)` is.
    fn write_bits(&mut self, w: &mut dyn Write, b: i32, nb: u32) {
        if self.err.is_some() {
            return;
        }
        self.bits |= i64::from(b).cast_unsigned() << self.nbits;
        self.nbits += nb;
        if self.nbits >= 48 {
            self.store48(w);
        }
    }

    /// Go: `huffmanBitWriter.writeCode`.
    fn write_code(&mut self, w: &mut dyn Write, c: Hcode) {
        if self.err.is_some() {
            return;
        }
        self.bits |= u64::from(c.code) << self.nbits;
        self.nbits += u32::from(c.len);
        if self.nbits >= 48 {
            self.store48(w);
        }
    }

    /// Go: `huffmanBitWriter.flush`: writes everything, padding the last byte with zero bits.
    fn flush(&mut self, w: &mut dyn Write) {
        if self.err.is_some() {
            self.nbits = 0;
            return;
        }
        let mut n = self.nbytes;
        while self.nbits != 0 {
            if let Some(slot) = self.bytes.get_mut(n) {
                *slot = self.bits.to_le_bytes()[0];
            }
            self.bits >>= 8;
            // Go: avoid underflow.
            self.nbits = self.nbits.saturating_sub(8);
            n += 1;
        }
        self.bits = 0;
        self.write_buffered(w, n);
        self.nbytes = 0;
    }

    /// Go: `huffmanBitWriter.writeBytes`: writes `bytes` after the pending whole bytes.
    fn write_bytes(&mut self, w: &mut dyn Write, bytes: &[u8]) {
        if self.err.is_some() {
            return;
        }
        let mut n = self.nbytes;
        if self.nbits & 7 != 0 {
            self.err = Some(GzipError::Internal("writeBytes with unfinished bits"));
            return;
        }
        while self.nbits != 0 {
            if let Some(slot) = self.bytes.get_mut(n) {
                *slot = self.bits.to_le_bytes()[0];
            }
            self.bits >>= 8;
            self.nbits -= 8;
            n += 1;
        }
        if n != 0 {
            self.write_buffered(w, n);
        }
        self.nbytes = 0;
        self.write(w, bytes);
    }

    /// Go: `huffmanBitWriter.writeStoredHeader`.
    fn write_stored_header(&mut self, w: &mut dyn Write, length: usize, is_eof: bool) {
        if self.err.is_some() {
            return;
        }
        self.write_bits(w, i32::from(is_eof), 3);
        self.flush(w);
        let length = u16::try_from(length & 0xFFFF).unwrap_or(0);
        self.write_bits(w, i32::from(length), 16);
        self.write_bits(w, i32::from(!length), 16);
    }

    /// Go: `huffmanBitWriter.writeFixedHeader`.
    fn write_fixed_header(&mut self, w: &mut dyn Write, is_eof: bool) {
        if self.err.is_some() {
            return;
        }
        // Go: indicate that we are a fixed Huffman block.
        self.write_bits(w, if is_eof { 3 } else { 2 }, 3);
    }

    /// Go: `huffmanBitWriter.writeTokens`: the tokens and the end-of-block marker, with the
    /// literal/length codes `le` and the offset codes `oe`.
    fn write_tokens(&mut self, w: &mut dyn Write, tokens: &[Token], le: &[Hcode], oe: &[Hcode]) {
        if self.err.is_some() {
            return;
        }
        let code = |codes: &[Hcode], i: u32| codes.get(i as usize).copied().unwrap_or_default();
        for t in tokens.iter().copied().chain(iter::once(END_BLOCK_TOKEN)) {
            if t.is_literal() {
                self.write_code(w, code(le, t.literal()));
                continue;
            }
            // Go: write the length.
            let length = t.length();
            let length_code = length_code(length);
            self.write_code(w, code(le, length_code + 257));
            let length_index = length_code as usize;
            let extra_length_bits = LENGTH_EXTRA_BITS.get(length_index).copied().unwrap_or(0);
            if extra_length_bits > 0 {
                let base = LENGTH_BASE.get(length_index).copied().unwrap_or(0);
                let extra_length = length.wrapping_sub(base).cast_signed();
                self.write_bits(w, extra_length, u32::from(extra_length_bits));
            }
            // Go: write the offset.
            let offset = t.offset();
            let offset_code = offset_code(offset);
            self.write_code(w, code(oe, offset_code));
            let offset_index = offset_code as usize;
            let extra_offset_bits = OFFSET_EXTRA_BITS.get(offset_index).copied().unwrap_or(0);
            if extra_offset_bits > 0 {
                let base = OFFSET_BASE.get(offset_index).copied().unwrap_or(0);
                let extra_offset = offset.wrapping_sub(base).cast_signed();
                self.write_bits(w, extra_offset, u32::from(extra_offset_bits));
            }
        }
    }
}

/// Go: `token(endBlockMarker)`.
const END_BLOCK_TOKEN: Token = Token::literal_token(256);
/// Go: `writeBlockHuff`'s `numLiterals`.
const HUFF_NUM_LITERALS: usize = END_BLOCK_MARKER + 1;
/// Go: `writeBlockHuff`'s `numOffsets`.
const HUFF_NUM_OFFSETS: usize = 1;

/// Go: `huffmanBitWriter.storedSize`: the size in bits of `input` as a stored block, and
/// whether it fits in one. A nil `input` is never stored.
fn stored_size(input: Option<&[u8]>) -> (i64, bool) {
    match input {
        Some(input) if input.len() <= MAX_STORE_BLOCK_SIZE => {
            (i64::try_from(input.len() + 5).unwrap_or(i64::MAX) * 8, true)
        }
        Some(_) | None => (0, false),
    }
}

/// Go: `huffmanBitWriter`.
#[derive(Clone, Debug)]
pub(super) struct HuffmanBitWriter {
    out: Output,
    codegen_freq: [i32; CODEGEN_CODE_COUNT],
    literal_freq: [i32; MAX_NUM_LIT],
    offset_freq: [i32; OFFSET_CODE_COUNT],
    codegen: [u8; MAX_NUM_LIT + OFFSET_CODE_COUNT + 1],
    literal_encoding: HuffmanEncoder,
    offset_encoding: HuffmanEncoder,
    codegen_encoding: HuffmanEncoder,
}

/// Which literal and offset encoders a block uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    /// Go: `fixedLiteralEncoding` and `fixedOffsetEncoding`.
    Fixed,
    /// Go: `w.literalEncoding` and `w.offsetEncoding`.
    Dynamic,
}

impl HuffmanBitWriter {
    /// Go: `newHuffmanBitWriter`.
    pub(super) fn new() -> Self {
        Self {
            out: Output {
                bits: 0,
                nbits: 0,
                bytes: [0; BUFFER_SIZE],
                nbytes: 0,
                err: None,
            },
            codegen_freq: [0; CODEGEN_CODE_COUNT],
            literal_freq: [0; MAX_NUM_LIT],
            offset_freq: [0; OFFSET_CODE_COUNT],
            codegen: [0; MAX_NUM_LIT + OFFSET_CODE_COUNT + 1],
            literal_encoding: HuffmanEncoder::new(MAX_NUM_LIT),
            offset_encoding: HuffmanEncoder::new(OFFSET_CODE_COUNT),
            codegen_encoding: HuffmanEncoder::new(CODEGEN_CODE_COUNT),
        }
    }

    /// Go: `huffmanBitWriter.err`.
    pub(super) fn err(&self) -> Option<GzipError> {
        self.out.err.clone()
    }

    /// Go: `huffmanBitWriter.reset`.
    pub(super) fn reset(&mut self) {
        self.out.bits = 0;
        self.out.nbits = 0;
        self.out.nbytes = 0;
        self.out.err = None;
    }

    /// Go: `huffmanBitWriter.flush`.
    pub(super) fn flush(&mut self, w: &mut dyn Write) {
        self.out.flush(w);
    }

    /// Go: `huffmanBitWriter.writeBytes`.
    pub(super) fn write_bytes(&mut self, w: &mut dyn Write, bytes: &[u8]) {
        self.out.write_bytes(w, bytes);
    }

    /// Go: `huffmanBitWriter.writeStoredHeader`.
    pub(super) fn write_stored_header(&mut self, w: &mut dyn Write, length: usize, is_eof: bool) {
        self.out.write_stored_header(w, length, is_eof);
    }

    /// Go: `huffmanBitWriter.generateCodegen`: the RFC 1951 3.2.7 run-length encoding of the
    /// literal and offset code lengths, into `codegen` (ended by [`BAD_CODE`]) and
    /// `codegen_freq`.
    fn generate_codegen(
        &mut self,
        num_literals: usize,
        num_offsets: usize,
        lit_codes: &[Hcode],
        off_codes: &[Hcode],
    ) {
        self.codegen_freq = [0; CODEGEN_CODE_COUNT];
        // Go: codegen holds a copy of the code sizes and then, in place, the result; the output
        // is always shorter than the input used so far.
        let codegen = &mut self.codegen;
        let freq = &mut self.codegen_freq;
        let lens = lit_codes
            .iter()
            .take(num_literals)
            .chain(off_codes.iter().take(num_offsets));
        for (slot, code) in codegen.iter_mut().zip(lens) {
            *slot = u8::try_from(code.len).unwrap_or(u8::MAX);
        }
        if let Some(slot) = codegen.get_mut(num_literals + num_offsets) {
            *slot = BAD_CODE;
        }

        let mut out_index = 0;
        let mut put = |buf: &mut [u8], v: u8| {
            if let Some(slot) = buf.get_mut(out_index) {
                *slot = v;
            }
            out_index += 1;
        };
        let count_freq = |counts: &mut [i32; CODEGEN_CODE_COUNT], v: u8| {
            if let Some(f) = counts.get_mut(usize::from(v)) {
                *f += 1;
            }
        };
        let mut size = codegen.first().copied().unwrap_or(BAD_CODE);
        let mut count: i32 = 1;
        let mut in_index = 1;
        while size != BAD_CODE {
            // Go: INVARIANT: we have seen "count" copies of size that have not yet had output
            // generated for them.
            let next_size = codegen.get(in_index).copied().unwrap_or(BAD_CODE);
            in_index += 1;
            if next_size == size {
                count += 1;
                continue;
            }
            // Go: we need to generate codegen indicating "count" of size.
            if size != 0 {
                put(codegen, size);
                count_freq(freq, size);
                count -= 1;
                while count >= 3 {
                    let n = count.min(6);
                    put(codegen, 16);
                    put(codegen, u8::try_from(n - 3).unwrap_or(0));
                    count_freq(freq, 16);
                    count -= n;
                }
            } else {
                while count >= 11 {
                    let n = count.min(138);
                    put(codegen, 18);
                    put(codegen, u8::try_from(n - 11).unwrap_or(0));
                    count_freq(freq, 18);
                    count -= n;
                }
                if count >= 3 {
                    // Go: count >= 3 && count <= 10.
                    put(codegen, 17);
                    put(codegen, u8::try_from(count - 3).unwrap_or(0));
                    count_freq(freq, 17);
                    count = 0;
                }
            }
            count -= 1;
            while count >= 0 {
                put(codegen, size);
                count_freq(freq, size);
                count -= 1;
            }
            // Go: set up invariant for next time through the loop.
            size = next_size;
            count = 1;
        }
        // Go: marker indicating the end of the codegen.
        put(codegen, BAD_CODE);
    }

    /// Go: `huffmanBitWriter.dynamicSize`: the size in bits of a dynamic block, and the number
    /// of code length codes its header lists.
    fn dynamic_size(
        &self,
        lit_codes: &[Hcode],
        off_codes: &[Hcode],
        extra_bits: i64,
    ) -> (i64, usize) {
        let freq = |i: usize| self.codegen_freq.get(i).copied().map_or(0, i64::from);
        let mut num_codegens = CODEGEN_CODE_COUNT;
        while num_codegens > 4
            && CODEGEN_ORDER
                .get(num_codegens - 1)
                .is_some_and(|&i| freq(i) == 0)
        {
            num_codegens -= 1;
        }
        let header = 3
            + 5
            + 5
            + 4
            + 3 * i64::try_from(num_codegens).unwrap_or(0)
            + bit_length(&self.codegen_encoding.codes, &self.codegen_freq)
            + freq(16) * 2
            + freq(17) * 3
            + freq(18) * 7;
        let size = header
            + bit_length(lit_codes, &self.literal_freq)
            + bit_length(off_codes, &self.offset_freq)
            + extra_bits;
        (size, num_codegens)
    }

    /// Go: `huffmanBitWriter.fixedSize`.
    fn fixed_size(&self, extra_bits: i64) -> i64 {
        3 + bit_length(&*FIXED_LITERAL_ENCODING, &self.literal_freq)
            + bit_length(&*FIXED_OFFSET_ENCODING, &self.offset_freq)
            + extra_bits
    }

    /// Go: `huffmanBitWriter.writeDynamicHeader`.
    fn write_dynamic_header(
        &mut self,
        w: &mut dyn Write,
        num_literals: usize,
        num_offsets: usize,
        num_codegens: usize,
        is_eof: bool,
    ) {
        if self.out.err.is_some() {
            return;
        }
        let as_i32 = |x: usize| i32::try_from(x).unwrap_or(i32::MAX);
        let first_bits = if is_eof { 5 } else { 4 };
        self.out.write_bits(w, first_bits, 3);
        self.out.write_bits(w, as_i32(num_literals) - 257, 5);
        self.out.write_bits(w, as_i32(num_offsets) - 1, 5);
        self.out.write_bits(w, as_i32(num_codegens) - 4, 4);

        let codes = &self.codegen_encoding.codes;
        let code = |i: usize| codes.get(i).copied().unwrap_or_default();
        for &order in CODEGEN_ORDER.iter().take(num_codegens) {
            self.out.write_bits(w, i32::from(code(order).len), 3);
        }

        let mut words = self.codegen.iter().copied();
        while let Some(code_word) = words.next() {
            if code_word == BAD_CODE {
                break;
            }
            self.out.write_code(w, code(usize::from(code_word)));
            let extra_bits = match code_word {
                16 => 2,
                17 => 3,
                18 => 7,
                _ => continue,
            };
            let extra = words.next().unwrap_or(0);
            self.out.write_bits(w, i32::from(extra), extra_bits);
        }
    }

    /// Go: `huffmanBitWriter.indexTokens`: the literal and offset frequencies of `tokens` and
    /// the end-of-block marker, the Huffman codes for them, and the number of literal and
    /// offset codes in use.
    fn index_tokens(&mut self, tokens: &[Token]) -> (usize, usize) {
        self.literal_freq = [0; MAX_NUM_LIT];
        self.offset_freq = [0; OFFSET_CODE_COUNT];
        for t in tokens.iter().copied().chain(iter::once(END_BLOCK_TOKEN)) {
            if t.is_literal() {
                if let Some(f) = self.literal_freq.get_mut(t.literal() as usize) {
                    *f += 1;
                }
                continue;
            }
            let length = t.length();
            let offset = t.offset();
            let lc = LENGTH_CODES_START + length_code(length) as usize;
            if let Some(f) = self.literal_freq.get_mut(lc) {
                *f += 1;
            }
            if let Some(f) = self.offset_freq.get_mut(offset_code(offset) as usize) {
                *f += 1;
            }
        }

        // Go: get the number of literals.
        let num_literals = self
            .literal_freq
            .iter()
            .rposition(|&f| f != 0)
            .map_or(0, |i| i + 1);
        // Go: get the number of offsets.
        let mut num_offsets = self
            .offset_freq
            .iter()
            .rposition(|&f| f != 0)
            .map_or(0, |i| i + 1);
        if num_offsets == 0 {
            // Go: we haven't found a single match. If we want to go with the dynamic encoding,
            // we should count at least one offset to be sure that the offset huffman tree could
            // be encoded.
            self.offset_freq[0] = 1;
            num_offsets = 1;
        }
        self.literal_encoding.generate(&self.literal_freq, 15);
        self.offset_encoding.generate(&self.offset_freq, 15);
        (num_literals, num_offsets)
    }

    /// Go: `huffmanBitWriter.writeBlock`: `tokens` with the smallest of the fixed, dynamic and
    /// (when `input` is given and fits) stored encodings.
    pub(super) fn write_block(
        &mut self,
        w: &mut dyn Write,
        tokens: &[Token],
        eof: bool,
        input: Option<&[u8]>,
    ) {
        if self.out.err.is_some() {
            return;
        }
        let (num_literals, num_offsets) = self.index_tokens(tokens);

        let mut extra_bits: i64 = 0;
        let (stored_size, storable) = stored_size(input);
        if storable {
            // Go: only bother calculating the costs of the extra bits required by the length of
            // offset fields (which will be the same for both fixed and dynamic encoding), if we
            // need to compare those two encodings against stored encoding.
            for length_code in LENGTH_CODES_START + 8..num_literals {
                // Go: first eight length codes have extra size = 0.
                let freq = self.literal_freq.get(length_code).copied().unwrap_or(0);
                let extra = LENGTH_EXTRA_BITS
                    .get(length_code - LENGTH_CODES_START)
                    .copied()
                    .unwrap_or(0);
                extra_bits += i64::from(freq) * i64::from(extra);
            }
            for offset_code in 4..num_offsets {
                // Go: first four offset codes have extra size = 0.
                let freq = self.offset_freq.get(offset_code).copied().unwrap_or(0);
                let extra = OFFSET_EXTRA_BITS.get(offset_code).copied().unwrap_or(0);
                extra_bits += i64::from(freq) * i64::from(extra);
            }
        }

        // Go: figure out smallest code. Fixed Huffman baseline.
        let mut encoding = Encoding::Fixed;
        let mut size = self.fixed_size(extra_bits);

        // Go: generate codegen and codegenFrequencies, which indicates how to encode the
        // literalEncoding and the offsetEncoding.
        let lit = mem::take(&mut self.literal_encoding.codes);
        let off = mem::take(&mut self.offset_encoding.codes);
        self.generate_codegen(num_literals, num_offsets, &lit, &off);
        self.codegen_encoding.generate(&self.codegen_freq, 7);
        let (dynamic_size, num_codegens) = self.dynamic_size(&lit, &off, extra_bits);
        if dynamic_size < size {
            size = dynamic_size;
            encoding = Encoding::Dynamic;
        }

        // Go: stored bytes?
        match input {
            Some(input) if storable && stored_size < size => {
                self.out.write_stored_header(w, input.len(), eof);
                self.out.write_bytes(w, input);
            }
            Some(_) | None => {
                // Go: Huffman.
                if encoding == Encoding::Fixed {
                    self.out.write_fixed_header(w, eof);
                    self.out.write_tokens(
                        w,
                        tokens,
                        &*FIXED_LITERAL_ENCODING,
                        &*FIXED_OFFSET_ENCODING,
                    );
                } else {
                    self.write_dynamic_header(w, num_literals, num_offsets, num_codegens, eof);
                    self.out.write_tokens(w, tokens, &lit, &off);
                }
            }
        }
        self.literal_encoding.codes = lit;
        self.offset_encoding.codes = off;
    }

    /// Go: `huffmanBitWriter.writeBlockDynamic`: `tokens` with a dynamic Huffman table, or
    /// `input` stored when the table saves less than 1/16 of it.
    pub(super) fn write_block_dynamic(
        &mut self,
        w: &mut dyn Write,
        tokens: &[Token],
        eof: bool,
        input: Option<&[u8]>,
    ) {
        if self.out.err.is_some() {
            return;
        }
        let (num_literals, num_offsets) = self.index_tokens(tokens);

        let lit = mem::take(&mut self.literal_encoding.codes);
        let off = mem::take(&mut self.offset_encoding.codes);
        self.generate_codegen(num_literals, num_offsets, &lit, &off);
        self.codegen_encoding.generate(&self.codegen_freq, 7);
        let (size, num_codegens) = self.dynamic_size(&lit, &off, 0);

        // Go: store bytes, if we don't get a reasonable improvement.
        let (stored, storable) = stored_size(input);
        match input {
            Some(input) if storable && stored < size + (size >> 4) => {
                self.out.write_stored_header(w, input.len(), eof);
                self.out.write_bytes(w, input);
            }
            Some(_) | None => {
                // Go: write Huffman table, then the tokens.
                self.write_dynamic_header(w, num_literals, num_offsets, num_codegens, eof);
                self.out.write_tokens(w, tokens, &lit, &off);
            }
        }
        self.literal_encoding.codes = lit;
        self.offset_encoding.codes = off;
    }

    /// Go: `huffmanBitWriter.writeBlockHuff`: `input` as literals with a dynamic Huffman table,
    /// or stored when the table saves less than 1/16 of it.
    pub(super) fn write_block_huff(&mut self, w: &mut dyn Write, eof: bool, input: &[u8]) {
        if self.out.err.is_some() {
            return;
        }
        // Go: clear histogram, add everything as literals.
        self.literal_freq = [0; MAX_NUM_LIT];
        histogram(input, &mut self.literal_freq);
        self.literal_freq[END_BLOCK_MARKER] = 1;
        // Go: offsetFreq is not cleared; only HUFF_OFFSET's single code is counted.
        self.offset_freq[0] = 1;

        self.literal_encoding.generate(&self.literal_freq, 15);

        // Go: figure out smallest code; always use dynamic Huffman or Store.
        let lit = mem::take(&mut self.literal_encoding.codes);
        self.generate_codegen(
            HUFF_NUM_LITERALS,
            HUFF_NUM_OFFSETS,
            &lit,
            &HUFF_OFFSET.codes,
        );
        self.codegen_encoding.generate(&self.codegen_freq, 7);
        let (size, num_codegens) = self.dynamic_size(&lit, &HUFF_OFFSET.codes, 0);

        // Go: store bytes, if we don't get a reasonable improvement.
        let (stored, storable) = stored_size(Some(input));
        if storable && stored < size + (size >> 4) {
            self.out.write_stored_header(w, input.len(), eof);
            self.out.write_bytes(w, input);
            self.literal_encoding.codes = lit;
            return;
        }

        // Go: Huffman.
        self.write_dynamic_header(w, HUFF_NUM_LITERALS, HUFF_NUM_OFFSETS, num_codegens, eof);
        self.write_huff_literals(w, &lit, input);
        self.literal_encoding.codes = lit;
    }

    /// The literal loop of Go's `writeBlockHuff`, with the bit writing inlined as in Go.
    fn write_huff_literals(&mut self, w: &mut dyn Write, lit: &[Hcode], input: &[u8]) {
        let encoding = lit.get(..=END_BLOCK_MARKER).unwrap_or_default();
        let out = &mut self.out;
        let mut n = out.nbytes;
        for &t in input {
            let c = encoding.get(usize::from(t)).copied().unwrap_or_default();
            out.bits |= u64::from(c.code) << out.nbits;
            out.nbits += u32::from(c.len);
            if out.nbits < 48 {
                continue;
            }
            // Go: store 6 bytes.
            let bits = out.bits;
            out.bits >>= 48;
            out.nbits -= 48;
            let [b0, b1, b2, b3, b4, b5, _, _] = bits.to_le_bytes();
            if let Some(dst) = out.bytes.get_mut(n..n + 6) {
                dst.copy_from_slice(&[b0, b1, b2, b3, b4, b5]);
            }
            n += 6;
            if n < BUFFER_FLUSH_SIZE {
                continue;
            }
            out.write_buffered(w, n);
            if out.err.is_some() {
                // Go: return early in the event of write failures.
                return;
            }
            n = 0;
        }
        out.nbytes = n;
        let eob = encoding.get(END_BLOCK_MARKER).copied().unwrap_or_default();
        out.write_code(w, eob);
    }
}

/// Go: `histogram`: adds the byte counts of `b` to `h`.
fn histogram(b: &[u8], h: &mut [i32; MAX_NUM_LIT]) {
    for &t in b {
        if let Some(count) = h.get_mut(usize::from(t)) {
            *count += 1;
        }
    }
}

#[cfg(test)]
pub(super) mod test_support {
    //! Access to the block writers for the ports of Go's `huffman_bit_writer_test.go`.

    use super::{HuffmanBitWriter, Token};

    /// Which block writer a golden case exercises.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(in crate::compress) enum Kind {
        /// Go: `writeBlock` (`wb`).
        Block,
        /// Go: `writeBlockDynamic` (`dyn`).
        Dynamic,
        /// Go: `writeBlockHuff` (`huff`).
        Huff,
    }

    /// Go: `writeToType` followed by `flush`, on a fresh writer and on the same writer after
    /// `reset`. Returns both outputs.
    pub(in crate::compress) fn write_to_type(
        kind: Kind,
        tokens: &[u32],
        eof: bool,
        input: Option<&[u8]>,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let tokens: Vec<Token> = tokens.iter().map(|&t| Token(t)).collect();
        let mut bw = HuffmanBitWriter::new();
        let mut first = Vec::new();
        run(&mut bw, &mut first, kind, &tokens, eof, input)?;
        bw.reset();
        let mut second = Vec::new();
        run(&mut bw, &mut second, kind, &tokens, eof, input)?;
        Ok((first, second))
    }

    fn run(
        bw: &mut HuffmanBitWriter,
        out: &mut Vec<u8>,
        kind: Kind,
        tokens: &[Token],
        eof: bool,
        input: Option<&[u8]>,
    ) -> Result<(), String> {
        match kind {
            Kind::Block => bw.write_block(out, tokens, eof, input),
            Kind::Dynamic => bw.write_block_dynamic(out, tokens, eof, input),
            Kind::Huff => bw.write_block_huff(out, eof, input.unwrap_or_default()),
        }
        if let Some(err) = bw.err() {
            return Err(err.to_string());
        }
        bw.flush(out);
        bw.err().map_or(Ok(()), |err| Err(err.to_string()))
    }
}
