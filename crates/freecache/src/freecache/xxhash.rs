//! XXH64 with a zero seed: `xxhash.Sum64` from `github.com/cespare/xxhash/v2`, the hash freecache
//! uses to pick a key's segment, slot and 16-bit tag.
//!
//! Prebid's `go.mod` resolves xxhash to v2.3.0; this is a port of its portable `Sum64`
//! (`xxhash_other.go`), which the amd64/arm64 assembly versions match bit for bit.

const PRIME1: u64 = 11_400_714_785_074_694_791;
const PRIME2: u64 = 14_029_467_366_897_019_727;
const PRIME3: u64 = 1_609_587_929_392_839_161;
const PRIME4: u64 = 9_650_029_242_287_828_579;
const PRIME5: u64 = 2_870_177_450_012_600_261;

/// Go: `xxhash.Sum64(b)`.
pub(super) fn sum64(b: &[u8]) -> u64 {
    let n = b.len();
    let mut rest = b;
    let mut h = if n >= 32 {
        let mut v = [
            PRIME1.wrapping_add(PRIME2),
            PRIME2,
            0,
            PRIME1.wrapping_neg(),
        ];
        let (blocks, tail) = b.as_chunks::<32>();
        for block in blocks {
            let (lanes, _) = block.as_chunks::<8>();
            for (acc, lane) in v.iter_mut().zip(lanes) {
                *acc = round(*acc, u64::from_le_bytes(*lane));
            }
        }
        rest = tail;
        let [v1, v2, v3, v4] = v;
        let mut h = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        h = merge_round(h, v1);
        h = merge_round(h, v2);
        h = merge_round(h, v3);
        merge_round(h, v4)
    } else {
        PRIME5
    };

    h = h.wrapping_add(u64::try_from(n).unwrap_or(u64::MAX));

    while let Some((lane, tail)) = rest.split_first_chunk::<8>() {
        let k1 = round(0, u64::from_le_bytes(*lane));
        h ^= k1;
        h = h.rotate_left(27).wrapping_mul(PRIME1).wrapping_add(PRIME4);
        rest = tail;
    }
    if let Some((lane, tail)) = rest.split_first_chunk::<4>() {
        h ^= u64::from(u32::from_le_bytes(*lane)).wrapping_mul(PRIME1);
        h = h.rotate_left(23).wrapping_mul(PRIME2).wrapping_add(PRIME3);
        rest = tail;
    }
    for &byte in rest {
        h ^= u64::from(byte).wrapping_mul(PRIME5);
        h = h.rotate_left(11).wrapping_mul(PRIME1);
    }

    h ^= h >> 33;
    h = h.wrapping_mul(PRIME2);
    h ^= h >> 29;
    h = h.wrapping_mul(PRIME3);
    h ^= h >> 32;
    h
}

/// Go: `round`.
const fn round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(PRIME2))
        .rotate_left(31)
        .wrapping_mul(PRIME1)
}

/// Go: `mergeRound`.
const fn merge_round(acc: u64, val: u64) -> u64 {
    (acc ^ round(0, val))
        .wrapping_mul(PRIME1)
        .wrapping_add(PRIME4)
}

#[cfg(test)]
mod tests {
    use super::{
        super::golden::{TestResult, check_eq},
        sum64,
    };

    /// The zero-seed vectors from cespare/xxhash's `TestAll` (`xxhash_test.go`).
    #[test]
    fn reference_vectors() -> TestResult {
        let cases: [(&[u8], u64); 6] = [
            (b"", 0xEF46_DB37_51D8_E999),
            (b"a", 0xD24E_C4F1_A98C_6E5B),
            (b"as", 0x1C33_0FB2_D66B_E179),
            (b"asd", 0x631C_37CE_72A9_7393),
            (b"asdf", 0x4158_72F5_99CE_A71E),
            (
                b"Call me Ishmael. Some years ago--never mind how long precisely-",
                0x02A2_E854_70D6_FD96,
            ),
        ];
        for (input, want) in cases {
            check_eq(
                &sum64(input),
                &want,
                &format!("Sum64({:?})", String::from_utf8_lossy(input)),
            )?;
        }
        Ok(())
    }
}
