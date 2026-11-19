//! Glue (Rule B (a)): Prebid.js and other clients send `JSON.stringify` output, which writes an
//! unpaired surrogate (a truncated emoji in a title or keyword) as a lone `\uD83D` escape. jsoniter
//! and `encoding/json` decode it to U+FFFD, while `serde_json` rejects the whole document, which
//! would turn Go's 200 into a 400. Every decoder in this crate therefore rewrites each lone
//! surrogate escape to the escape of U+FFFD before `serde_json` reads the bytes
//! (`json.lone-surrogates`, restored).

use std::{borrow::Cow, str};

/// The escape a lone surrogate becomes: U+FFFD, the same six bytes long.
const REPLACEMENT: &[u8; 6] = b"\\uFFFD";

/// Returns `data` with every lone surrogate escape inside a string rewritten to the escape of
/// U+FFFD, and a valid surrogate pair left as it is.
///
/// The bytes are scanned only when they hold a `\u` escape in the surrogate range; string and
/// escape state are tracked, so a `\\` followed by `ud800` is text, not an escape. Each rewrite
/// keeps the length, so offsets into the result are offsets into `data`.
///
/// Escapes pair the way jsoniter reads them (`readEscapedChar` in jsoniter's `iter_str.go`), as
/// `jsonutil.Unmarshal` decodes outside input: a surrogate followed by another `\u` escape is read
/// with it, and when the two do not form a pair both stand alone (each a U+FFFD if it is a
/// surrogate). `encoding/json` reads the second escape again instead, so a high surrogate followed
/// by a valid pair gives U+FFFD and the pair's character there, and three U+FFFD here; gjson
/// (`IntString`, `ParseIntoString`) turns the two escapes into one U+FFFD
/// (`json.surrogate-pairing`).
#[must_use]
pub fn replace_lone_surrogates(data: &[u8]) -> Cow<'_, [u8]> {
    if !data.windows(4).any(|w| {
        matches!(
            w,
            [
                b'\\',
                b'u',
                b'd' | b'D',
                b'8'..=b'9' | b'a'..=b'f' | b'A'..=b'F',
            ]
        )
    }) {
        return Cow::Borrowed(data);
    }
    let mut out = data.to_vec();
    let mut in_string = false;
    let mut at = 0;
    while let Some(&byte) = out.get(at) {
        if !in_string {
            in_string = byte == b'"';
            at += 1;
        } else if byte == b'"' {
            in_string = false;
            at += 1;
        } else if byte == b'\\' {
            at = rewrite_escape(&mut out, at);
        } else {
            at += 1;
        }
    }
    Cow::Owned(out)
}

/// Rewrites the escape that starts at `at` (a backslash inside a string) where it is a lone
/// surrogate, and returns the index after the escapes read.
fn rewrite_escape(buf: &mut [u8], at: usize) -> usize {
    let Some(unit) = code_unit(buf, at) else {
        // A two-byte escape such as \" or \\, or a malformed one serde_json rejects.
        return at + 2;
    };
    if !is_surrogate(unit) {
        return at + 6;
    }
    match code_unit(buf, at + 6) {
        Some(next) if is_high(unit) && is_low(next) => at + 12,
        Some(next) => {
            replace(buf, at);
            if is_surrogate(next) {
                replace(buf, at + 6);
            }
            at + 12
        }
        None => {
            replace(buf, at);
            at + 6
        }
    }
}

/// The UTF-16 code unit of the `\uXXXX` escape at `at`, if one is there.
fn code_unit(buf: &[u8], at: usize) -> Option<u16> {
    match buf.get(at..at + 6)? {
        [b'\\', b'u', hex @ ..] if hex.iter().all(u8::is_ascii_hexdigit) => {
            u16::from_str_radix(str::from_utf8(hex).ok()?, 16).ok()
        }
        _ => None,
    }
}

/// Overwrites the six-byte escape at `at` with [`REPLACEMENT`].
fn replace(buf: &mut [u8], at: usize) {
    if let Some(slot) = buf.get_mut(at..at + 6) {
        slot.copy_from_slice(REPLACEMENT);
    }
}

/// Go: `utf16.IsSurrogate`.
const fn is_surrogate(unit: u16) -> bool {
    matches!(unit, 0xD800..=0xDFFF)
}

/// A high (leading) surrogate.
const fn is_high(unit: u16) -> bool {
    matches!(unit, 0xD800..=0xDBFF)
}

/// A low (trailing) surrogate.
const fn is_low(unit: u16) -> bool {
    matches!(unit, 0xDC00..=0xDFFF)
}
