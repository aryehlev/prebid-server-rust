//! Go `unicode`: the character classes and simple case mappings Go uses.
//!
//! Rust's `char` methods follow a newer Unicode version and, for case mapping, the *full*
//! mappings (`'ß'.to_uppercase()` is `"SS"`, `'İ'.to_lowercase()` is two characters). Go maps
//! each rune to exactly one rune with the simple mappings of Unicode 15.0.0. The tables in
//! `tables.rs` are generated from the Go toolchain by `testdata/go/unicodegen`, and a test checks
//! every rune against a checksum computed by Go.

mod tables;

use std::cmp::Ordering;

/// Go: `unicode.MaxRune`.
pub const MAX_RUNE: u32 = 0x0010_FFFF;
/// Go: `unicode.ReplacementChar` (also `utf8.RuneError`).
pub const REPLACEMENT_CHAR: char = '\u{FFFD}';

/// Go: `unicode.UpperLower`, the delta that marks an alternating upper/lower sequence.
const UPPER_LOWER: i32 = 1_114_112;

/// Go: `unicode.UpperCase`, `unicode.LowerCase`, `unicode.TitleCase`.
#[derive(Clone, Copy)]
enum Case {
    Upper = 0,
    Lower = 1,
    Title = 2,
}

fn in_table(table: &[(u32, u32)], r: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < r {
                Ordering::Less
            } else if lo > r {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// Go: `unicode.IsLetter`.
#[must_use]
pub fn is_letter(c: char) -> bool {
    in_table(tables::LETTER, u32::from(c))
}

/// Go: `unicode.IsDigit`.
#[must_use]
pub fn is_digit(c: char) -> bool {
    in_table(tables::DIGIT, u32::from(c))
}

/// Go: `unicode.IsPrint` (and `strconv.IsPrint`, which is defined to be identical).
#[must_use]
pub fn is_print(c: char) -> bool {
    in_table(tables::PRINT, u32::from(c))
}

/// Go: `unicode.IsGraphic` (and `strconv.IsGraphic`).
#[must_use]
pub fn is_graphic(c: char) -> bool {
    in_table(tables::GRAPHIC, u32::from(c))
}

/// Go: `unicode.IsSpace`.
#[must_use]
pub fn is_space(c: char) -> bool {
    in_table(tables::SPACE, u32::from(c))
}

/// Go: `unicode.to` over `unicode.CaseRanges`.
fn to_case(case: Case, c: char) -> char {
    let r = u32::from(c);
    let found = tables::CASE_RANGES.binary_search_by(|&(lo, hi, ..)| {
        if hi < r {
            Ordering::Less
        } else if lo > r {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    let Some(&(lo, _, upper, lower, title)) = found.ok().and_then(|i| tables::CASE_RANGES.get(i))
    else {
        return c;
    };
    let delta = match case {
        Case::Upper => upper,
        Case::Lower => lower,
        Case::Title => title,
    };
    let mapped = if delta == UPPER_LOWER {
        // Go: rune(cr.Lo) + ((r-rune(cr.Lo))&^1 | rune(_case&1)).
        lo.saturating_add((r.saturating_sub(lo) & !1) | (case as u32 & 1))
    } else {
        r.saturating_add_signed(delta)
    };
    char::from_u32(mapped).unwrap_or(c)
}

/// Go: `unicode.ToLower`.
#[must_use]
pub fn to_lower(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        to_case(Case::Lower, c)
    }
}

/// Go: `unicode.ToUpper`.
#[must_use]
pub fn to_upper(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_uppercase()
    } else {
        to_case(Case::Upper, c)
    }
}

/// Go: `unicode.ToTitle`.
#[must_use]
pub fn to_title(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_uppercase()
    } else {
        to_case(Case::Title, c)
    }
}

/// Go: `unicode.SimpleFold`: the smallest rune greater than `c` that is equivalent under simple
/// case folding, wrapping around to the smallest one.
#[must_use]
pub fn simple_fold(c: char) -> char {
    let r = u32::from(c);
    if let Ok(i) = tables::FOLD_ORBIT.binary_search_by_key(&r, |&(from, _)| from)
        && let Some(&(_, to)) = tables::FOLD_ORBIT.get(i)
    {
        return char::from_u32(to).unwrap_or(c);
    }
    let lower = to_case(Case::Lower, c);
    if lower == c {
        to_case(Case::Upper, c)
    } else {
        lower
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    /// FNV-1a 64, as Go's `hash/fnv.New64a`.
    struct Fnv64a(u64);

    impl Fnv64a {
        fn write_u32(&mut self, v: u32) {
            for byte in v.to_le_bytes() {
                self.0 ^= u64::from(byte);
                self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
    }

    // Every rune's classes and mappings hash to the value Go computed
    // (testdata/go/unicodegen/checksum.txt).
    #[test]
    fn every_rune_matches_go() -> Result<(), String> {
        let mut h = Fnv64a(0xCBF2_9CE4_8422_2325);
        for c in (0..=MAX_RUNE).filter_map(char::from_u32) {
            let preds = [is_letter, is_digit, is_print, is_graphic, is_space];
            let flags = preds
                .iter()
                .enumerate()
                .filter(|(_, pred)| pred(c))
                .fold(0_u32, |acc, (i, _)| acc | (1 << i));
            h.write_u32(u32::from(c));
            h.write_u32(flags);
            h.write_u32(u32::from(to_lower(c)));
            h.write_u32(u32::from(to_upper(c)));
            h.write_u32(u32::from(to_title(c)));
            h.write_u32(u32::from(simple_fold(c)));
        }
        let want = include_str!("../../testdata/go/unicodegen/checksum.txt").trim();
        ensure_eq(
            format!("{:016x}", h.0).as_str(),
            want,
            "checksum over all runes",
        )
    }

    #[test]
    fn go_simple_mappings_differ_from_rust_full_mappings() {
        assert_eq!(to_upper('ß'), 'ß', "no simple uppercase for sharp s");
        assert_eq!(to_lower('\u{130}'), 'i', "dotted capital I lowercases to i");
        assert_eq!(to_lower('Σ'), 'σ', "no final-sigma context");
        assert_eq!(simple_fold('k'), '\u{212A}', "k folds to the Kelvin sign");
        assert_eq!(simple_fold('\u{212A}'), 'K', "Kelvin sign folds to K");
    }
}
