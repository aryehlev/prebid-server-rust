//! UTF-8 decoding and case mapping that reproduce Go's `unicode/utf8`, `unicode` and `strings`
//! behaviour where the JSON libraries depend on it.
//!
//! Case mapping and rune classes come from Go's own Unicode tables ([`tables`]), not from the
//! Rust standard library, whose Unicode version is newer than Go's.

mod tables;

use core::str::from_utf8;
use std::borrow::Cow;

use tables::{CASE_RANGES, LETTER_OR_DIGIT, PRINT, UPPER_LOWER};

/// Go's `utf8.RuneError`.
pub(crate) const RUNE_ERROR: char = '\u{FFFD}';

/// Width of the UTF-8 sequence introduced by `first`, or 1 for a byte that cannot start one.
const fn sequence_width(first: u8) -> usize {
    match first {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 1,
    }
}

/// Go's `utf8.DecodeRune`: the rune at the start of `b` and its width. An invalid or truncated
/// encoding yields `(None, 1)` (Go returns `RuneError, 1`); an empty slice yields `(None, 0)`.
pub(crate) fn decode_rune(b: &[u8]) -> (Option<char>, usize) {
    // Go: unicode/utf8/utf8.go DecodeRune
    let Some(&first) = b.first() else {
        return (None, 0);
    };
    if first < 0x80 {
        return (Some(char::from(first)), 1);
    }
    let width = sequence_width(first);
    if width == 1 {
        return (None, 1);
    }
    match b.get(..width).map(from_utf8) {
        Some(Ok(s)) => (s.chars().next(), width),
        Some(Err(_)) | None => (None, 1),
    }
}

/// Converts Go string bytes to a Rust `String`, replacing every byte that is not part of a valid
/// UTF-8 sequence with U+FFFD, the way Go's `for range` and `utf8.DecodeRune` see it.
pub(crate) fn go_lossy(b: &[u8]) -> String {
    if let Ok(s) = from_utf8(b) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(b.len() + 8);
    let mut rest = b;
    while !rest.is_empty() {
        let (r, width) = decode_rune(rest);
        out.push(r.unwrap_or(RUNE_ERROR));
        rest = rest.get(width.max(1)..).unwrap_or_default();
    }
    out
}

/// [`go_lossy`] as bytes, borrowed when `b` is already valid UTF-8.
pub(crate) fn go_lossy_bytes(b: &[u8]) -> Cow<'_, [u8]> {
    if from_utf8(b).is_ok() {
        Cow::Borrowed(b)
    } else {
        Cow::Owned(go_lossy(b).into_bytes())
    }
}

/// Go's `unicode.to`: `c` mapped through `unicode.CaseRanges` to lower case when `lower`,
/// otherwise to upper case; `c` itself when no range covers it.
fn to_case(c: char, lower: bool) -> char {
    // Go: unicode/letter.go to
    let code = u32::from(c);
    let index = CASE_RANGES.partition_point(|&(_, hi, ..)| hi < code);
    let Some(&(lo, _, to_upper, to_lower)) = CASE_RANGES.get(index).filter(|r| r.0 <= code) else {
        return c;
    };
    let delta = if lower { to_lower } else { to_upper };
    let mapped = if delta == UPPER_LOWER {
        // An alternating range starts with an upper-case rune: even offsets are upper case and
        // odd offsets lower case.
        Some(lo + (((code - lo) & !1) | u32::from(lower)))
    } else {
        code.checked_add_signed(delta)
    };
    mapped.and_then(char::from_u32).unwrap_or(c)
}

/// Go's `unicode.ToLower` (simple case mapping).
pub(crate) fn to_lower_rune(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    to_case(c, true)
}

/// Go's `unicode.ToUpper` (simple case mapping).
pub(crate) fn to_upper_rune(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    to_case(c, false)
}

/// Go's `strings.ToLower` over string bytes.
pub(crate) fn strings_to_lower(s: &[u8]) -> Vec<u8> {
    // Go: strings/strings.go ToLower
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    // strings.Map(unicode.ToLower, s): invalid bytes become U+FFFD.
    let mut out = Vec::with_capacity(s.len() + 4);
    let mut rest = s;
    while !rest.is_empty() {
        let (r, width) = decode_rune(rest);
        let mapped = r.map_or(RUNE_ERROR, to_lower_rune);
        let mut buf = [0_u8; 4];
        out.extend_from_slice(mapped.encode_utf8(&mut buf).as_bytes());
        rest = rest.get(width.max(1)..).unwrap_or_default();
    }
    out
}

/// Go's `unicode.caseOrbit` (Unicode 15.0.0): the folding orbits with more than two members,
/// and the dotted/dotless i that fold only to themselves.
const CASE_ORBIT: [(u32, u32); 88] = [
    (0x004B, 0x006B),
    (0x0053, 0x0073),
    (0x006B, 0x212A),
    (0x0073, 0x017F),
    (0x00B5, 0x039C),
    (0x00C5, 0x00E5),
    (0x00DF, 0x1E9E),
    (0x00E5, 0x212B),
    (0x0130, 0x0130),
    (0x0131, 0x0131),
    (0x017F, 0x0053),
    (0x01C4, 0x01C5),
    (0x01C5, 0x01C6),
    (0x01C6, 0x01C4),
    (0x01C7, 0x01C8),
    (0x01C8, 0x01C9),
    (0x01C9, 0x01C7),
    (0x01CA, 0x01CB),
    (0x01CB, 0x01CC),
    (0x01CC, 0x01CA),
    (0x01F1, 0x01F2),
    (0x01F2, 0x01F3),
    (0x01F3, 0x01F1),
    (0x0345, 0x0399),
    (0x0392, 0x03B2),
    (0x0395, 0x03B5),
    (0x0398, 0x03B8),
    (0x0399, 0x03B9),
    (0x039A, 0x03BA),
    (0x039C, 0x03BC),
    (0x03A0, 0x03C0),
    (0x03A1, 0x03C1),
    (0x03A3, 0x03C2),
    (0x03A6, 0x03C6),
    (0x03A9, 0x03C9),
    (0x03B2, 0x03D0),
    (0x03B5, 0x03F5),
    (0x03B8, 0x03D1),
    (0x03B9, 0x1FBE),
    (0x03BA, 0x03F0),
    (0x03BC, 0x00B5),
    (0x03C0, 0x03D6),
    (0x03C1, 0x03F1),
    (0x03C2, 0x03C3),
    (0x03C3, 0x03A3),
    (0x03C6, 0x03D5),
    (0x03C9, 0x2126),
    (0x03D0, 0x0392),
    (0x03D1, 0x03F4),
    (0x03D5, 0x03A6),
    (0x03D6, 0x03A0),
    (0x03F0, 0x039A),
    (0x03F1, 0x03A1),
    (0x03F4, 0x0398),
    (0x03F5, 0x0395),
    (0x0412, 0x0432),
    (0x0414, 0x0434),
    (0x041E, 0x043E),
    (0x0421, 0x0441),
    (0x0422, 0x0442),
    (0x042A, 0x044A),
    (0x0432, 0x1C80),
    (0x0434, 0x1C81),
    (0x043E, 0x1C82),
    (0x0441, 0x1C83),
    (0x0442, 0x1C84),
    (0x044A, 0x1C86),
    (0x0462, 0x0463),
    (0x0463, 0x1C87),
    (0x1C80, 0x0412),
    (0x1C81, 0x0414),
    (0x1C82, 0x041E),
    (0x1C83, 0x0421),
    (0x1C84, 0x1C85),
    (0x1C85, 0x0422),
    (0x1C86, 0x042A),
    (0x1C87, 0x0462),
    (0x1C88, 0xA64A),
    (0x1E60, 0x1E61),
    (0x1E61, 0x1E9B),
    (0x1E9B, 0x1E60),
    (0x1E9E, 0x00DF),
    (0x1FBE, 0x0345),
    (0x2126, 0x03A9),
    (0x212A, 0x004B),
    (0x212B, 0x00C5),
    (0xA64A, 0xA64B),
    (0xA64B, 0x1C88),
];

/// Go's `unicode.SimpleFold`: the next rune of `c`'s case-folding orbit.
fn simple_fold(c: char) -> char {
    // Go: unicode/letter.go SimpleFold (asciiFold is the caseOrbit entries plus the ASCII
    // letter pairs)
    let code = u32::from(c);
    if let Ok(index) = CASE_ORBIT.binary_search_by_key(&code, |&(from, _)| from) {
        return CASE_ORBIT
            .get(index)
            .and_then(|&(_, to)| char::from_u32(to))
            .unwrap_or(c);
    }
    if c.is_ascii() {
        return if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c.to_ascii_uppercase()
        };
    }
    let lower = to_lower_rune(c);
    if lower == c { to_upper_rune(c) } else { lower }
}

/// `encoding/json`'s `foldRune`: the smallest rune of `c`'s folding orbit.
fn fold_rune(mut c: char) -> char {
    // Go: encoding/json/fold.go foldRune
    loop {
        let next = simple_fold(c);
        if next <= c {
            return next;
        }
        c = next;
    }
}

/// `encoding/json`'s `foldName`: ASCII letters upper-cased, other runes mapped to the smallest
/// rune of their folding orbit, invalid bytes to U+FFFD.
pub(crate) fn fold_name(name: &[u8]) -> Vec<u8> {
    // Go: encoding/json/fold.go appendFoldedName
    let mut out = Vec::with_capacity(name.len());
    let mut rest = name;
    while let Some(&c) = rest.first() {
        if c < 0x80 {
            out.push(c.to_ascii_uppercase());
            rest = rest.get(1..).unwrap_or_default();
            continue;
        }
        let (r, width) = decode_rune(rest);
        let mapped = fold_rune(r.unwrap_or(RUNE_ERROR));
        let mut buf = [0_u8; 4];
        out.extend_from_slice(mapped.encode_utf8(&mut buf).as_bytes());
        rest = rest.get(width.max(1)..).unwrap_or_default();
    }
    out
}

/// Whether `c` lies in one of the sorted, disjoint inclusive `ranges`.
fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let code = u32::from(c);
    let index = ranges.partition_point(|&(_, hi)| hi < code);
    ranges.get(index).is_some_and(|&(lo, _)| lo <= code)
}

/// Go's `strconv.IsPrint` for a rune.
pub(crate) fn is_print(c: char) -> bool {
    // Go: strconv/quote.go IsPrint
    in_ranges(&PRINT, c)
}

/// Go's `unicode.IsLetter(c) || unicode.IsDigit(c)`.
pub(crate) fn is_letter_or_digit(c: char) -> bool {
    in_ranges(&LETTER_OR_DIGIT, c)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        decode_rune, fold_name, fold_rune, go_lossy, is_letter_or_digit, is_print,
        strings_to_lower, to_lower_rune, to_upper_rune,
    };

    /// `Ok` when the values are equal, otherwise an error naming the case.
    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let (got, want) = ($got, $want);
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    // Expected values below were produced by Go 1.26.2 (utf8.DecodeRune, strings.ToLower,
    // unicode.ToUpper and encoding/json's foldRune over unicode.SimpleFold).

    #[test]
    fn decode_rune_matches_go() -> Result<(), String> {
        check!(decode_rune(b""), (None, 0), "empty input")?;
        check!(decode_rune(b"a"), (Some('a'), 1), "ascii")?;
        check!(decode_rune(b"\xC3\xA9"), (Some('\u{e9}'), 2), "two bytes")?;
        check!(decode_rune(b"\xE2\x80"), (None, 1), "truncated")?;
        check!(decode_rune(b"\xED\xA0\x80"), (None, 1), "surrogate")?;
        check!(decode_rune(b"\xC0\x80"), (None, 1), "overlong")?;
        check!(decode_rune(b"\xFF"), (None, 1), "invalid start")
    }

    #[test]
    fn lossy_replaces_each_invalid_byte() -> Result<(), String> {
        check!(go_lossy(b"a\xE2\x80b"), "a\u{FFFD}\u{FFFD}b", "per byte")
    }

    #[test]
    fn lower_and_upper() -> Result<(), String> {
        check!(strings_to_lower(b"ABc"), b"abc".to_vec(), "ascii lower")?;
        check!(
            strings_to_lower("\u{212A}".as_bytes()),
            b"k".to_vec(),
            "kelvin sign"
        )?;
        check!(to_upper_rune('\u{1F80}'), '\u{1F88}', "ypogegrammeni")?;
        check!(
            to_upper_rune('\u{DF}'),
            '\u{DF}',
            "sharp s has no simple upper"
        )
    }

    #[test]
    fn fold_matches_go() -> Result<(), String> {
        let cases: [(char, char); 18] = [
            ('\u{130}', '\u{130}'),
            ('\u{131}', '\u{131}'),
            ('\u{B5}', '\u{B5}'),
            ('\u{3A3}', '\u{3A3}'),
            ('\u{3C2}', '\u{3A3}'),
            ('\u{3C3}', '\u{3A3}'),
            ('\u{17F}', 'S'),
            ('\u{212A}', 'K'),
            ('\u{1E9E}', '\u{DF}'),
            ('\u{DF}', '\u{DF}'),
            ('\u{345}', '\u{345}'),
            ('\u{1C5}', '\u{1C4}'),
            ('\u{E9}', '\u{C9}'),
            ('\u{13A0}', '\u{13A0}'),
            ('\u{AB70}', '\u{13A0}'),
            ('\u{FFFD}', '\u{FFFD}'),
            ('\u{10400}', '\u{10400}'),
            ('\u{10428}', '\u{10400}'),
        ];
        for (c, want) in cases {
            check!(fold_rune(c), want, &format!("foldRune({c:?})"))?;
        }
        check!(fold_name(b"aBc"), b"ABC".to_vec(), "ascii fold")?;
        check!(
            fold_name("\u{17F}".as_bytes()),
            b"S".to_vec(),
            "long s folds to S"
        )?;
        check!(
            fold_name("\u{130}d".as_bytes()),
            "\u{130}D".as_bytes().to_vec(),
            "dotted capital I folds only to itself",
        )?;
        check!(
            fold_name(b"a\xFFb"),
            "A\u{FFFD}B".as_bytes().to_vec(),
            "invalid byte",
        )
    }

    #[test]
    fn unicode_15_not_the_standard_library() -> Result<(), String> {
        // Go 1.26.2 (Unicode 15.0.0) against what Rust's newer char methods give.
        check!(
            to_upper_rune('\u{19B}'),
            '\u{19B}',
            "U+019B has no upper case"
        )?;
        check!(
            to_lower_rune('\u{A7DC}'),
            '\u{A7DC}',
            "U+A7DC is unassigned"
        )?;
        check!(fold_rune('\u{A7DC}'), '\u{A7DC}', "U+A7DC folds to itself")?;
        check!(is_print('\u{378}'), false, "unassigned U+0378")?;
        check!(is_print('\u{A7DC}'), false, "unassigned U+A7DC")?;
        check!(is_print('\u{E000}'), false, "private use")?;
        check!(is_print('\u{3000}'), false, "ideographic space")?;
        check!(is_print('\u{1F600}'), true, "emoji")?;
        check!(is_letter_or_digit('\u{B2}'), false, "superscript two (No)")?;
        check!(
            is_letter_or_digit('\u{2163}'),
            false,
            "roman numeral four (Nl)"
        )?;
        check!(
            is_letter_or_digit('\u{93E}'),
            false,
            "devanagari vowel sign (Mc)"
        )?;
        check!(
            is_letter_or_digit('\u{663}'),
            true,
            "arabic-indic digit three (Nd)"
        )?;
        check!(is_letter_or_digit('\u{3C3}'), true, "greek small sigma")
    }

    /// Go's answers from `testdata/golden/unicode.txt` (written by `testdata/go` with Go
    /// 1.26.2), indexed by rune.
    struct GoTables {
        lower: Vec<u32>,
        upper: Vec<u32>,
        fold: Vec<u32>,
        print: Vec<bool>,
        letter_or_digit: Vec<bool>,
    }

    fn go_tables() -> Result<GoTables, String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/unicode.txt");
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let identity: Vec<u32> = (0..=0x0010_FFFF).collect();
        let mut tables = GoTables {
            lower: identity.clone(),
            upper: identity.clone(),
            fold: identity,
            print: vec![false; 0x0011_0000],
            letter_or_digit: vec![false; 0x0011_0000],
        };
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<&str> = line.split('\t').map(|f| f.trim_matches('"')).collect();
            let bad = || format!("bad row {line:?}");
            let [kind, a, b] = fields.as_slice() else {
                return Err(bad());
            };
            let a = u32::from_str_radix(a, 16).map_err(|e| e.to_string())?;
            let b = u32::from_str_radix(b, 16).map_err(|e| e.to_string())?;
            let (first, last) = (a as usize, b as usize);
            match *kind {
                "lower" | "upper" | "fold" => {
                    let table = match *kind {
                        "lower" => &mut tables.lower,
                        "upper" => &mut tables.upper,
                        _ => &mut tables.fold,
                    };
                    *table.get_mut(first).ok_or_else(bad)? = b;
                }
                "print" | "letterdigit" => {
                    let table = if *kind == "print" {
                        &mut tables.print
                    } else {
                        &mut tables.letter_or_digit
                    };
                    table.get_mut(first..=last).ok_or_else(bad)?.fill(true);
                }
                _ => return Err(bad()),
            }
        }
        Ok(tables)
    }

    #[test]
    fn tables_match_go_for_every_rune() -> Result<(), String> {
        // Go: unicode.ToLower, unicode.ToUpper, encoding/json foldRune, strconv.IsPrint and
        // unicode.IsLetter || unicode.IsDigit over every rune.
        let go = go_tables()?;
        let mut mismatches = Vec::new();
        for c in (0..=0x0010_FFFF_u32).filter_map(char::from_u32) {
            let code = u32::from(c);
            let at = |table: &[u32]| table.get(code as usize).copied();
            let flag = |table: &[bool]| table.get(code as usize).copied().map(u32::from);
            let got = [
                ("ToLower", u32::from(to_lower_rune(c)), at(&go.lower)),
                ("ToUpper", u32::from(to_upper_rune(c)), at(&go.upper)),
                ("foldRune", u32::from(fold_rune(c)), at(&go.fold)),
                ("IsPrint", u32::from(is_print(c)), flag(&go.print)),
                (
                    "IsLetter||IsDigit",
                    u32::from(is_letter_or_digit(c)),
                    flag(&go.letter_or_digit),
                ),
            ];
            for (name, rust, go_value) in got {
                if Some(rust) != go_value {
                    mismatches.push(format!(
                        "{name}(U+{code:04X}): go {go_value:X?}, rust {rust:X}"
                    ));
                }
            }
        }
        if mismatches.is_empty() {
            return Ok(());
        }
        let total = mismatches.len();
        mismatches.truncate(40);
        Err(format!("{total} mismatches:\n{}", mismatches.join("\n")))
    }
}
