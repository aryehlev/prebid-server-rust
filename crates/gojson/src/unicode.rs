//! UTF-8 decoding and case mapping that reproduce Go's `unicode/utf8`, `unicode` and `strings`
//! behaviour where the JSON libraries depend on it.

use core::str::from_utf8;
use std::borrow::Cow;

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

/// Go's `unicode.ToLower` (simple case mapping).
pub(crate) fn to_lower_rune(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        // Only U+0130 has a multi-character full lowercase mapping; its simple mapping is 'i',
        // which is the first character of the full mapping.
        (Some(l), _) => l,
        (None, _) => c,
    }
}

/// Go's `unicode.ToUpper` (simple case mapping).
pub(crate) fn to_upper_rune(c: char) -> char {
    let code = u32::from(c);
    // Greek letters with ypogegrammeni have a single-character simple uppercase mapping (their
    // titlecase form) but a two-character full mapping.
    let simple = match code {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => Some(code + 8),
        0x1FB3 => Some(0x1FBC),
        0x1FC3 => Some(0x1FCC),
        0x1FF3 => Some(0x1FFC),
        _ => None,
    };
    if let Some(mapped) = simple.and_then(char::from_u32) {
        return mapped;
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        // A multi-character full mapping means there is no simple mapping.
        (Some(_) | None, _) => c,
    }
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

/// Go's `strconv.IsPrint` for a rune. Exact for Latin-1; an approximation (control, format,
/// separator, private-use and surrogate code points are unprintable) elsewhere.
pub(crate) fn is_print(c: char) -> bool {
    let code = u32::from(c);
    if code < 0x100 {
        return matches!(code, 0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF);
    }
    !(c.is_control()
        || c.is_whitespace()
        || matches!(
            code,
            0x061C
                | 0x070F
                | 0x180E
                | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x206F
                | 0xFEFF
                | 0xFFF9..=0xFFFB
                | 0xE000..=0xF8FF
                | 0x000F_0000..=0x0010_FFFF
        ))
}

#[cfg(test)]
mod tests {
    use super::{decode_rune, fold_name, fold_rune, go_lossy, strings_to_lower, to_upper_rune};

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
}
