//! jsonparser's `escape.go` (`Unescape`) and `bytes.go` (`parseInt`).

use std::borrow::Cow;

use super::ParserError;

/// `h2I`: a hex digit's value.
fn hex_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'A'..=b'F' => Some(u32::from(c - b'A' + 10)),
        b'a'..=b'f' => Some(u32::from(c - b'a' + 10)),
        _ => None,
    }
}

/// `decodeSingleUnicodeEscape`: the four hex digits after a `\u` that is assumed, not checked.
fn decode_single_unicode_escape(input: &[u8]) -> Option<u32> {
    let digits = input.get(2..6)?;
    digits
        .iter()
        .try_fold(0_u32, |acc, &c| Some(acc << 4 | hex_digit(c)?))
}

/// `decodeUnicodeEscape`: the rune and the number of bytes consumed.
fn decode_unicode_escape(input: &[u8]) -> Option<(u32, usize)> {
    // Go: buger/jsonparser escape.go decodeUnicodeEscape
    let r = decode_single_unicode_escape(input)?;
    if !(0xD800..=0xDFFF).contains(&r) {
        return Some((r, 6));
    }
    // The second escape's `\u` is not checked either.
    let r2 = decode_single_unicode_escape(input.get(6..).unwrap_or_default())?;
    if r2 < 0xDC00 {
        return None;
    }
    // utf16.DecodeRune: U+FFFD unless a high surrogate is followed by a low one.
    let pair = (0xD800..0xDC00).contains(&r) && (0xDC00..0xE000).contains(&r2);
    let rune = if pair {
        ((r - 0xD800) << 10 | (r2 - 0xDC00)) + 0x10000
    } else {
        0xFFFD
    };
    Some((rune, 12))
}

/// `utf8.EncodeRune`: invalid runes become U+FFFD.
fn push_rune(out: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut buf = [0_u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// `unescapeToUTF8`: unescapes the escape sequence at the start of `input` into `out`, returning
/// the number of bytes consumed.
fn unescape_to_utf8(input: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    if input.first() != Some(&b'\\') {
        return None;
    }
    let simple = match input.get(1)? {
        b'"' => b'"',
        b'\\' => b'\\',
        b'/' => b'/',
        b'b' => 0x08,
        b'f' => 0x0C,
        b'n' => b'\n',
        b'r' => b'\r',
        b't' => b'\t',
        b'u' => {
            let (r, len) = decode_unicode_escape(input)?;
            push_rune(out, r);
            return Some(len);
        }
        _ => return None,
    };
    out.push(simple);
    Some(2)
}

/// `jsonparser.Unescape(in, out)`: the contents of a JSON string with its escape sequences
/// replaced (borrowed when there are none).
///
/// Unlike `encoding/json`, it copies other bytes as they are (invalid UTF-8 and control
/// characters included), does not check the second `\u` of a surrogate pair, and writes
/// U+FFFD for a pair that combines to no valid rune.
///
/// # Errors
///
/// [`ParserError::MalformedStringEscape`] for an unknown escape or a bad `\u` sequence.
pub fn unescape(input: &[u8]) -> Result<Cow<'_, [u8]>, ParserError> {
    // Go: buger/jsonparser escape.go Unescape
    let Some(first) = input.iter().position(|&c| c == b'\\') else {
        return Ok(Cow::Borrowed(input));
    };
    let mut out = Vec::with_capacity(input.len());
    out.extend_from_slice(input.get(..first).unwrap_or_default());
    let mut rest = input.get(first..).unwrap_or_default();
    while !rest.is_empty() {
        let consumed =
            unescape_to_utf8(rest, &mut out).ok_or(ParserError::MalformedStringEscape)?;
        rest = rest.get(consumed..).unwrap_or_default();
        match rest.iter().position(|&c| c == b'\\') {
            None => {
                out.extend_from_slice(rest);
                break;
            }
            Some(next) => {
                out.extend_from_slice(rest.get(..next).unwrap_or_default());
                rest = rest.get(next..).unwrap_or_default();
            }
        }
    }
    Ok(Cow::Owned(out))
}

/// The outcome of jsonparser's `parseInt` when it fails.
pub(super) enum IntError {
    Malformed,
    Overflow,
}

/// jsonparser's `parseInt`: base 10 with an optional `-` (a lone `-` is 0).
pub(super) fn parse_int(bytes: &[u8]) -> Result<i64, IntError> {
    // Go: buger/jsonparser bytes.go parseInt
    if bytes.is_empty() {
        return Err(IntError::Malformed);
    }
    let (neg, digits) = bytes
        .strip_prefix(b"-")
        .map_or((false, bytes), |rest| (true, rest));
    let mut n: u64 = 0;
    for &c in digits {
        if !c.is_ascii_digit() {
            return Err(IntError::Malformed);
        }
        if n > u64::MAX / 10 {
            return Err(IntError::Overflow);
        }
        n *= 10;
        n = n
            .checked_add(u64::from(c - b'0'))
            .ok_or(IntError::Overflow)?;
    }
    if neg {
        return 0_i64.checked_sub_unsigned(n).ok_or(IntError::Overflow);
    }
    i64::try_from(n).map_err(|_overflow| IntError::Overflow)
}
