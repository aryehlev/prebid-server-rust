//! Go: `compress/flate/huffman_code.go`, the length-limited Huffman code builder.

use core::array;
use std::sync::LazyLock;

use super::{huffman_bit_writer::OFFSET_CODE_COUNT, inflate::MAX_NUM_LIT};

/// Go: `hcode`, a Huffman code (bit-reversed, ready to write LSB first) and its length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Hcode {
    pub(super) code: u16,
    pub(super) len: u16,
}

/// Go: `literalNode`.
#[derive(Clone, Copy, Debug, Default)]
struct LiteralNode {
    literal: u16,
    freq: i32,
}

/// Go: `maxNode()`, the sentinel after the last real node.
const MAX_NODE: LiteralNode = LiteralNode {
    literal: u16::MAX,
    freq: i32::MAX,
};

/// Go: `levelInfo`, the state of the tree being built at one depth. (Go also stores the level,
/// which always equals the index into `levels`.)
#[derive(Clone, Copy, Debug, Default)]
struct LevelInfo {
    last_freq: i32,
    next_char_freq: i32,
    next_pair_freq: i32,
    needed: i32,
}

/// Go: `maxBitsLimit`.
const MAX_BITS_LIMIT: usize = 16;

/// Go: `huffmanEncoder`. `codes` keeps the codes of the last `generate` call; a symbol whose
/// frequency was zero gets length 0 and keeps its previous code bits, as in Go.
#[derive(Clone, Debug)]
pub(super) struct HuffmanEncoder {
    pub(super) codes: Vec<Hcode>,
    /// Go: `freqcache`, allocated on the first `generate` call.
    freqcache: Vec<LiteralNode>,
    bit_count: [i32; 17],
}

impl HuffmanEncoder {
    /// Go: `newHuffmanEncoder(size)`.
    pub(super) fn new(size: usize) -> Self {
        Self {
            codes: vec![Hcode::default(); size],
            freqcache: Vec::new(),
            bit_count: [0; 17],
        }
    }

    /// Go: `huffmanEncoder.generate`: the minimum code for `freq` (indexed by symbol) with no
    /// code longer than `max_bits`.
    pub(super) fn generate(&mut self, freq: &[i32], max_bits: i32) {
        if self.freqcache.is_empty() {
            // Go: the longest frequency table is maxNumLit, plus the sentinel.
            self.freqcache = vec![LiteralNode::default(); MAX_NUM_LIT + 1];
        }
        let mut count = 0;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 {
                if let Some(slot) = self.freqcache.get_mut(count) {
                    *slot = LiteralNode {
                        literal: u16::try_from(i).unwrap_or(u16::MAX),
                        freq: f,
                    };
                }
                count += 1;
            } else if let Some(code) = self.codes.get_mut(i) {
                code.len = 0;
            }
        }
        let Some(list) = self.freqcache.get_mut(..=count) else {
            return;
        };
        if count <= 2 {
            // Go: with two or fewer literals everything has bit length 1, in literal order.
            for (i, node) in (0u16..).zip(list.iter().take(count)) {
                if let Some(code) = self.codes.get_mut(usize::from(node.literal)) {
                    *code = Hcode { code: i, len: 1 };
                }
            }
            return;
        }
        let Some((sentinel, nodes)) = list.split_last_mut() else {
            return;
        };
        // Go: sort.Sort(byFreq); (freq, literal) is unique, so the order is the same.
        nodes.sort_unstable_by_key(|node| (node.freq, node.literal));
        *sentinel = MAX_NODE;
        let bit_count = bit_counts(list, max_bits, &mut self.bit_count);
        if let Some(sorted) = list.get_mut(..count) {
            assign_encoding_and_size(bit_count, sorted, &mut self.codes);
        }
    }
}

/// Go: `huffmanEncoder.bitLength`: the number of bits `freq` takes with `codes`.
pub(super) fn bit_length(codes: &[Hcode], freq: &[i32]) -> i64 {
    let mut total = 0;
    for (code, &f) in codes.iter().zip(freq) {
        if f != 0 {
            total += i64::from(f) * i64::from(code.len);
        }
    }
    total
}

/// Go: `levels`, one per depth (a bogus level 0 whose sole purpose is so that
/// `level1.prev.needed == 0`).
type Levels = [LevelInfo; MAX_BITS_LIMIT];

/// Go: `leafCounts[i][j]`, the number of literals at the left of the level `j` ancestor of the
/// rightmost node at level `i`.
type LeafCounts = [[i32; MAX_BITS_LIMIT]; MAX_BITS_LIMIT];

/// The frequency of `list[i]`, or [`MAX_NODE`]'s past the end.
fn freq_at(list: &[LiteralNode], i: i32) -> i32 {
    usize::try_from(i)
        .ok()
        .and_then(|i| list.get(i))
        .map_or(MAX_NODE.freq, |node| node.freq)
}

/// Go: `huffmanEncoder.bitCounts`: how many literals get each code length. `list` holds the
/// `n >= 3` nodes in increasing frequency order followed by the [`MAX_NODE`] sentinel; the
/// result is `bit_count[..=max_bits]`, where `bit_count[i]` counts the literals of `i` bits.
fn bit_counts<'a>(list: &[LiteralNode], max_bits: i32, bit_count: &'a mut [i32; 17]) -> &'a [i32] {
    let n = i32::try_from(list.len().saturating_sub(1)).unwrap_or(i32::MAX);
    // Go: the tree can't have greater depth than n - 1.
    let max_bits = usize::try_from(max_bits.min(n - 1)).unwrap_or(0);

    let mut levels = [LevelInfo::default(); MAX_BITS_LIMIT];
    let mut leaf_counts = [[0; MAX_BITS_LIMIT]; MAX_BITS_LIMIT];
    for level in 1..=max_bits {
        if let Some(info) = levels.get_mut(level) {
            *info = LevelInfo {
                last_freq: freq_at(list, 1),
                next_char_freq: freq_at(list, 2),
                next_pair_freq: if level == 1 {
                    i32::MAX
                } else {
                    freq_at(list, 0).wrapping_add(freq_at(list, 1))
                },
                needed: 0,
            };
        }
        if let Some(count) = leaf_counts
            .get_mut(level)
            .and_then(|row| row.get_mut(level))
        {
            *count = 2;
        }
    }
    // Go: we need a total of 2*n - 2 items at the top level and have already generated 2.
    if let Some(top) = levels.get_mut(max_bits) {
        top.needed = 2 * n - 4;
    }
    build_levels(list, max_bits, &mut levels, &mut leaf_counts);

    // Go: bitCount := h.bitCount[:maxBits+1]; bitCount[0] is never written.
    let counts = leaf_counts.get(max_bits).copied().unwrap_or_default();
    for (bits, level) in (1..).zip((1..=max_bits).rev()) {
        if let (Some(slot), Some(&hi), Some(&lo)) = (
            bit_count.get_mut(bits),
            counts.get(level),
            counts.get(level.wrapping_sub(1)),
        ) {
            *slot = hi - lo;
        }
    }
    bit_count.get(..=max_bits).unwrap_or_default()
}

/// The main loop of Go's `bitCounts`: the package-merge over the levels, from the top level
/// `max_bits` down, until the top level has all its items.
fn build_levels(
    list: &[LiteralNode],
    max_bits: usize,
    levels: &mut Levels,
    leaf_counts: &mut LeafCounts,
) {
    let mut level = max_bits;
    while let Some(&current) = levels.get(level) {
        if current.next_pair_freq == i32::MAX && current.next_char_freq == i32::MAX {
            // Go: we've run out of both leaves and pairs; never come back to this level.
            if let Some(info) = levels.get_mut(level) {
                info.needed = 0;
            }
            if let Some(up) = levels.get_mut(level + 1) {
                up.next_pair_freq = i32::MAX;
            }
            level += 1;
            continue;
        }
        let prev_freq = current.last_freq;
        let mut info = current;
        if info.next_char_freq < info.next_pair_freq {
            // Go: the next item on this row is a leaf node.
            let leaf = leaf_counts
                .get(level)
                .and_then(|row| row.get(level))
                .map_or(0, |&c| c + 1);
            info.last_freq = info.next_char_freq;
            if let Some(count) = leaf_counts
                .get_mut(level)
                .and_then(|row| row.get_mut(level))
            {
                *count = leaf;
            }
            info.next_char_freq = freq_at(list, leaf);
        } else {
            // Go: the next item on this row is a pair from the previous row; take the leaf
            // counts from the lower level, except counts[level] stays the same.
            info.last_freq = info.next_pair_freq;
            let lower = leaf_counts
                .get(level.wrapping_sub(1))
                .copied()
                .unwrap_or_default();
            if let Some(row) = leaf_counts.get_mut(level) {
                for (dst, &src) in row.iter_mut().zip(&lower).take(level) {
                    *dst = src;
                }
            }
            if let Some(below) = levels.get_mut(level.wrapping_sub(1)) {
                below.needed = 2;
            }
        }
        info.needed -= 1;
        if let Some(slot) = levels.get_mut(level) {
            *slot = info;
        }
        if info.needed == 0 {
            // Go: this level is done; fill in nextPairFreq one level up.
            if level == max_bits {
                break;
            }
            if let Some(up) = levels.get_mut(level + 1) {
                up.next_pair_freq = prev_freq.wrapping_add(info.last_freq);
            }
            level += 1;
        } else {
            // Go: if we stole from below, move down temporarily to replenish it.
            while levels
                .get(level.wrapping_sub(1))
                .is_some_and(|below| below.needed > 0)
            {
                level -= 1;
            }
        }
    }
}

/// Go: `huffmanEncoder.assignEncodingAndSize`: RFC 1951 3.2.2 codes, assigned to the literals of
/// each length in literal order. `list` is in increasing frequency order.
fn assign_encoding_and_size(bit_count: &[i32], list: &mut [LiteralNode], codes: &mut [Hcode]) {
    let mut code: u16 = 0;
    let mut list = list;
    for (n, &bits) in bit_count.iter().enumerate() {
        code <<= 1;
        if n == 0 || bits == 0 {
            continue;
        }
        // Go: the last `bits` literals of list get `n` bits.
        let split = list
            .len()
            .saturating_sub(usize::try_from(bits).unwrap_or(0));
        let Some((rest, chunk)) = list.split_at_mut_checked(split) else {
            return;
        };
        chunk.sort_unstable_by_key(|node| node.literal);
        let len = u8::try_from(n).unwrap_or(0);
        for node in chunk.iter() {
            if let Some(slot) = codes.get_mut(usize::from(node.literal)) {
                *slot = Hcode {
                    code: reverse_bits(code, len),
                    len: u16::from(len),
                };
            }
            code = code.wrapping_add(1);
        }
        list = rest;
    }
}

/// Go: `reverseBits(number, bitLength)`.
pub(super) const fn reverse_bits(number: u16, bit_length: u8) -> u16 {
    let shift = 16u8.wrapping_sub(bit_length) as u32;
    match number.checked_shl(shift) {
        Some(n) => n.reverse_bits(),
        None => 0,
    }
}

/// Go: `fixedLiteralEncoding`, the RFC 1951 3.2.6 literal/length code.
pub(super) static FIXED_LITERAL_ENCODING: LazyLock<[Hcode; MAX_NUM_LIT]> = LazyLock::new(|| {
    array::from_fn(|ch| {
        let ch = u16::try_from(ch).unwrap_or(0);
        let (bits, size) = match ch {
            // Go: size 8, 000110000 .. 10111111.
            0..144 => (ch + 48, 8),
            // Go: size 9, 110010000 .. 111111111.
            144..256 => (ch + 400 - 144, 9),
            // Go: size 7, 0000000 .. 0010111.
            256..280 => (ch - 256, 7),
            // Go: size 8, 11000000 .. 11000111.
            _ => (ch + 192 - 280, 8),
        };
        Hcode {
            code: reverse_bits(bits, size),
            len: u16::from(size),
        }
    })
});

/// Go: `fixedOffsetEncoding`.
pub(super) static FIXED_OFFSET_ENCODING: LazyLock<[Hcode; OFFSET_CODE_COUNT]> =
    LazyLock::new(|| {
        array::from_fn(|ch| Hcode {
            code: reverse_bits(u16::try_from(ch).unwrap_or(0), 5),
            len: 5,
        })
    });

#[cfg(test)]
mod tests {
    use super::reverse_bits;

    /// Go: `compress/flate` `TestReverseBits` (`reverseBitsTests`).
    #[test]
    fn reverse_bits_table() -> Result<(), String> {
        let cases: [(u16, u8, u16); 8] = [
            (1, 1, 1),
            (1, 2, 2),
            (1, 3, 4),
            (1, 4, 8),
            (1, 5, 16),
            (17, 5, 17),
            (257, 9, 257),
            (29, 5, 23),
        ];
        for (input, bit_count, want) in cases {
            let got = reverse_bits(input, bit_count);
            if got != want {
                return Err(format!(
                    "reverse_bits({input}, {bit_count}) = {got}, want {want}"
                ));
            }
        }
        Ok(())
    }
}
