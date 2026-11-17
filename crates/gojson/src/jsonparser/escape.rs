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
    // Go: combineUTF16Surrogates is plain arithmetic: neither unit is checked to be the right
    // surrogate, so `\uD800\uE000` is U+10400. utf8.EncodeRune later writes U+FFFD for a result
    // past U+10FFFF (a leading low surrogate, or a large second unit).
    Some((0x10000 + ((r - 0xD800) << 10) + (r2 - 0xDC00), 12))
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
/// characters included). A `\u` escape of a surrogate (U+D800 to U+DFFF) must be followed by
/// four hex digits of at least `DC00` (their `\u` is not checked) and is combined with them
/// arithmetically, whether or not they are a low surrogate: `\uD800\uE000` is U+10400, and a
/// result past U+10FFFF is written as U+FFFD.
///
/// # Errors
///
/// [`ParserError::MalformedStringEscape`] for an unknown escape or a bad `\u` sequence.
pub fn unescape(input: &[u8]) -> Result<Cow<'_, [u8]>, ParserError> {
    if !input.contains(&b'\\') {
        return Ok(Cow::Borrowed(input));
    }
    let mut out = Vec::with_capacity(input.len());
    unescape_into(input, &mut out)?;
    Ok(Cow::Owned(out))
}

/// `Unescape`'s loop, appending to `out` as Go writes its output buffer: on a bad escape,
/// what was written before it stays.
fn unescape_into(input: &[u8], out: &mut Vec<u8>) -> Result<(), ParserError> {
    // Go: buger/jsonparser escape.go Unescape
    let Some(first) = input.iter().position(|&c| c == b'\\') else {
        out.extend_from_slice(input);
        return Ok(());
    };
    out.extend_from_slice(input.get(..first).unwrap_or_default());
    let mut rest = input.get(first..).unwrap_or_default();
    while !rest.is_empty() {
        let consumed = unescape_to_utf8(rest, out).ok_or(ParserError::MalformedStringEscape)?;
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
    Ok(())
}

/// Go's `unescapeStackBufSize`: the size of the stack buffer jsonparser unescapes keys into.
pub(super) const STACK_BUF_SIZE: usize = 64;

/// Where `Unescape(in, stackbuf[:])` put its result.
pub(super) enum InStack<'a> {
    /// `in` itself: it has no escapes.
    Input(&'a [u8]),
    /// The first `n` bytes of the stack buffer.
    Stack(usize),
    /// A new allocation: `in` is longer than the buffer.
    Heap(Vec<u8>),
}

/// `Unescape(in, stackbuf[:])` with Go's side effect on the caller's buffer: when `in` has an
/// escape and fits, the result is built in `stack`, and a bad escape leaves the bytes written
/// before it there.
pub(super) fn unescape_in_stack<'a>(
    input: &'a [u8],
    stack: &mut [u8; STACK_BUF_SIZE],
) -> Result<InStack<'a>, ParserError> {
    // Go: buger/jsonparser escape.go Unescape (`if cap(out) < len(in) { out = make(...) }`)
    if !input.contains(&b'\\') {
        return Ok(InStack::Input(input));
    }
    let mut out = Vec::with_capacity(input.len());
    let res = unescape_into(input, &mut out);
    if input.len() > STACK_BUF_SIZE {
        return res.map(|()| InStack::Heap(out));
    }
    // The output is never longer than the input.
    if let Some(dst) = stack.get_mut(..out.len()) {
        dst.copy_from_slice(&out);
    }
    res.map(|()| InStack::Stack(out.len()))
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
