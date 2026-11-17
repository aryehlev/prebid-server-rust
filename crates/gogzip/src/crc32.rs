//! Go's `hash/crc32` with the IEEE polynomial, the checksum `compress/gzip` writes and checks.
//!
//! Go picks a hardware implementation where it can; every implementation computes the same
//! CRC-32, so this one uses slicing-by-8 tables.
//!
//! ```
//! use pbs_gogzip::crc32;
//!
//! assert_eq!(crc32::checksum_ieee(b"hello world\n"), 0xAF08_3B2D);
//! // Update continues a running checksum, as Go's crc32.Update(crc, crc32.IEEETable, p) does.
//! let running = crc32::update(crc32::checksum_ieee(b"hello "), b"world\n");
//! assert_eq!(running, 0xAF08_3B2D);
//! ```

use core::array;
use std::sync::LazyLock;

/// Go: `crc32.IEEE`, the reversed polynomial.
pub const IEEE: u32 = 0xEDB8_8320;

/// Go: `slicing8Table` for [`IEEE`]: `TABLES[0]` is `crc32.IEEETable`, and `TABLES[k][i]`
/// advances `TABLES[k - 1][i]` by one more zero byte.
static TABLES: LazyLock<[[u32; 256]; 8]> = LazyLock::new(make_tables);

/// Go: `simpleMakeTable` followed by `slicingMakeTable`.
fn make_tables() -> [[u32; 256]; 8] {
    let first: [u32; 256] = array::from_fn(|i| {
        let mut crc = u32::try_from(i).unwrap_or(0);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ IEEE
            } else {
                crc >> 1
            };
        }
        crc
    });
    let mut tables = [first; 8];
    for k in 1..8 {
        let prev = tables.get(k - 1).copied().unwrap_or(first);
        if let Some(table) = tables.get_mut(k) {
            for (entry, &p) in table.iter_mut().zip(prev.iter()) {
                *entry = (p >> 8) ^ lookup(&first, p);
            }
        }
    }
    tables
}

/// `table[x & 0xFF]`; the mask keeps the index in range.
#[inline]
fn lookup(table: &[u32; 256], x: u32) -> u32 {
    table.get((x & 0xFF) as usize).copied().unwrap_or(0)
}

/// Go: `crc32.Update(crc, crc32.IEEETable, p)`: the checksum of the data that produced `crc`
/// followed by `p`.
#[must_use]
pub fn update(crc: u32, p: &[u8]) -> u32 {
    let tables = &*TABLES;
    let [t0, t1, t2, t3, t4, t5, t6, t7] = tables;
    let mut crc = !crc;
    let (blocks, rest) = p.as_chunks::<8>();
    for block in blocks {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = *block;
        crc ^= u32::from_le_bytes([b0, b1, b2, b3]);
        crc = lookup(t0, u32::from(b7))
            ^ lookup(t1, u32::from(b6))
            ^ lookup(t2, u32::from(b5))
            ^ lookup(t3, u32::from(b4))
            ^ lookup(t4, crc >> 24)
            ^ lookup(t5, crc >> 16)
            ^ lookup(t6, crc >> 8)
            ^ lookup(t7, crc);
    }
    for &b in rest {
        crc = lookup(t0, crc ^ u32::from(b)) ^ (crc >> 8);
    }
    !crc
}

/// Go: `crc32.ChecksumIEEE(data)`.
#[must_use]
pub fn checksum_ieee(data: &[u8]) -> u32 {
    update(0, data)
}

#[cfg(test)]
mod tests {
    use super::{checksum_ieee, update};

    /// Go: `hash/crc32` `golden` test vectors (the IEEE column).
    const GOLDEN: &[(u32, &str)] = &[
        (0x0, ""),
        (0xE8B7_BE43, "a"),
        (0x9E83_486D, "ab"),
        (0x3524_41C2, "abc"),
        (0xED82_CD11, "abcd"),
        (0x8587_D865, "abcde"),
        (0x4B8E_39EF, "abcdef"),
        (0x312A_6AA6, "abcdefg"),
        (0xAEEF_2A50, "abcdefgh"),
        (0x8DA9_88AF, "abcdefghi"),
        (0x3981_703A, "abcdefghij"),
        (0x6B9C_DFE7, "Discard medicine more than two years old."),
        (
            0xC90E_F73F,
            "He who has a shady past knows that nice guys finish last.",
        ),
        (0xB902_341F, "I wouldn't marry him with a ten foot pole."),
        (
            0x0420_80E8,
            "Free! Free!/A trip/to Mars/for 900/empty jars/Burma Shave",
        ),
        (
            0x154C_6D11,
            "The days of the digital watch are numbered.  -Tom Stoppard",
        ),
        (0x4C41_8325, "Nepal premier won't resign."),
        (
            0x3395_5150,
            "For every action there is an equal and opposite government program.",
        ),
        (
            0x2621_6A4B,
            "His money is twice tainted: 'taint yours and 'taint mine.",
        ),
        (
            0x1ABB_E45E,
            concat!(
                "There is no reason for any individual to have a computer in their home. ",
                "-Ken Olsen, 1977"
            ),
        ),
        (
            0xC89A_94F7,
            "It's a tiny change to the code and not completely disgusting. - Bob Manchek",
        ),
        (0xAB3A_BE14, "size:  a.out:  bad magic"),
        (
            0xBAB1_02B6,
            "The major problem is with sendmail.  -Mark Horton",
        ),
        (
            0x9991_49D7,
            "Give me a rock, paper and scissors and I will move the world.  CCFestoon",
        ),
        (
            0x6D52_A33C,
            "If the enemy is within range, then so are you.",
        ),
        (
            0x9063_1E8D,
            "It's well we cannot hear the screams/That we create in others' dreams.",
        ),
        (
            0x7830_9130,
            "You remind me of a TV show, but that's all right: I watch it anyway.",
        ),
        (0x7D0A_377F, "C is as portable as Stonehedge!!"),
        (
            0x8C79_FD79,
            concat!(
                "Even if I could be Shakespeare, I think I should still choose to be Faraday. ",
                "- A. Huxley"
            ),
        ),
        (
            0xA20B_7167,
            concat!(
                "The fugacity of a constituent in a mixture of gases at a given temperature is ",
                "proportional to its mole fraction.  Lewis-Randall Rule"
            ),
        ),
        (
            0x8E0B_B443,
            "How can you write a big system without C++?  -Paul Glick",
        ),
    ];

    #[test]
    fn golden_vectors() -> Result<(), String> {
        for &(want, input) in GOLDEN {
            let got = checksum_ieee(input.as_bytes());
            if got != want {
                return Err(format!(
                    "checksum_ieee({input:?}) = {got:#x}, want {want:#x}"
                ));
            }
            // Every split of the input gives the same running checksum.
            for split in 0..=input.len() {
                let (head, tail) = input.as_bytes().split_at(split);
                let running = update(checksum_ieee(head), tail);
                if running != want {
                    return Err(format!(
                        "split {split} of {input:?} = {running:#x}, want {want:#x}"
                    ));
                }
            }
        }
        Ok(())
    }
}
