//! Go: `strconv/quote.go` (`Quote`, `QuoteToASCII`, `QuoteRune`, `Unquote`, `UnquoteChar`, ...).
//!
//! Go strings are byte strings that may hold invalid UTF-8; [`quote_bytes`] and
//! [`unquote_bytes`] keep that exactness, while [`quote`] and [`unquote`] work on Rust strings.

use std::{error::Error as StdError, fmt};

use crate::{unicode, utf8::lossy_string};

const LOWERHEX: &[u8; 16] = b"0123456789abcdef";

/// Go: `strconv.ErrSyntax` as returned by `Unquote` and `UnquoteChar` (a bare error, not a
/// `NumError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnquoteError;

impl fmt::Display for UnquoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid syntax")
    }
}

impl StdError for UnquoteError {}

/// Go: `strconv.IsPrint`.
#[must_use]
pub fn is_print(c: char) -> bool {
    unicode::is_print(c)
}

/// Go: `strconv.IsGraphic`.
#[must_use]
pub fn is_graphic(c: char) -> bool {
    unicode::is_graphic(c)
}

fn hex_byte(buf: &mut Vec<u8>, b: u8) {
    buf.extend_from_slice(b"\\x");
    buf.extend(LOWERHEX.get(usize::from(b >> 4)));
    buf.extend(LOWERHEX.get(usize::from(b & 0xF)));
}

/// Go: `appendEscapedRune`.
fn append_escaped_rune(
    buf: &mut Vec<u8>,
    r: char,
    quote: u8,
    ascii_only: bool,
    graphic_only: bool,
) {
    let code = u32::from(r);
    if code == u32::from(quote) || r == '\\' {
        // always backslashed
        buf.push(b'\\');
        buf.extend(u8::try_from(code));
        return;
    }
    if ascii_only {
        if r.is_ascii() && is_print(r) {
            buf.extend(u8::try_from(code));
            return;
        }
    } else if is_print(r) || (graphic_only && is_graphic(r)) {
        let mut tmp = [0; 4];
        buf.extend_from_slice(r.encode_utf8(&mut tmp).as_bytes());
        return;
    }
    match r {
        '\u{07}' => buf.extend_from_slice(b"\\a"),
        '\u{08}' => buf.extend_from_slice(b"\\b"),
        '\u{0C}' => buf.extend_from_slice(b"\\f"),
        '\n' => buf.extend_from_slice(b"\\n"),
        '\r' => buf.extend_from_slice(b"\\r"),
        '\t' => buf.extend_from_slice(b"\\t"),
        '\u{0B}' => buf.extend_from_slice(b"\\v"),
        _ if code < u32::from(b' ') || code == 0x7F => {
            hex_byte(buf, u8::try_from(code).unwrap_or(0));
        }
        _ => {
            let (prefix, width): (&[u8], u32) = if code < 0x10000 {
                (b"\\u", 4)
            } else {
                (b"\\U", 8)
            };
            buf.extend_from_slice(prefix);
            for shift in (0..width).rev() {
                let nibble = (code >> (shift * 4)) & 0xF;
                buf.extend(LOWERHEX.get(usize::try_from(nibble).unwrap_or(0)));
            }
        }
    }
}

/// Go: `appendQuotedWith` over raw bytes: invalid UTF-8 bytes are written as `\xNN`.
fn quote_with(s: &[u8], quote: u8, ascii_only: bool, graphic_only: bool) -> String {
    let mut buf = Vec::with_capacity(s.len() * 3 / 2 + 2);
    buf.push(quote);
    for chunk in s.utf8_chunks() {
        for r in chunk.valid().chars() {
            append_escaped_rune(&mut buf, r, quote, ascii_only, graphic_only);
        }
        for &b in chunk.invalid() {
            hex_byte(&mut buf, b);
        }
    }
    buf.push(quote);
    // Every byte pushed is ASCII or comes from a valid `char`.
    String::from_utf8(buf).unwrap_or_default()
}

/// Go: `strconv.Quote`: a double-quoted Go string literal; non-printable runes (as defined by
/// [`is_print`]) are escaped with `\a \b \f \n \r \t \v \xNN \uNNNN \UNNNNNNNN`.
#[must_use]
pub fn quote(s: &str) -> String {
    quote_with(s.as_bytes(), b'"', false, false)
}

/// Go: `strconv.Quote(string(b))` for bytes that may not be valid UTF-8.
#[must_use]
pub fn quote_bytes(s: &[u8]) -> String {
    quote_with(s, b'"', false, false)
}

/// Go: `strconv.QuoteToASCII`: like [`quote`] but escapes every non-ASCII rune.
#[must_use]
pub fn quote_to_ascii(s: &str) -> String {
    quote_with(s.as_bytes(), b'"', true, false)
}

/// Go: `strconv.QuoteToASCII(string(b))` for bytes that may not be valid UTF-8.
#[must_use]
pub fn quote_to_ascii_bytes(s: &[u8]) -> String {
    quote_with(s, b'"', true, false)
}

/// Go: `strconv.QuoteToGraphic`: like [`quote`] but leaves graphic runes (such as U+00A0)
/// unescaped.
#[must_use]
pub fn quote_to_graphic(s: &str) -> String {
    quote_with(s.as_bytes(), b'"', false, true)
}

fn quote_rune_with(r: char, ascii_only: bool, graphic_only: bool) -> String {
    let mut buf = vec![b'\''];
    append_escaped_rune(&mut buf, r, b'\'', ascii_only, graphic_only);
    buf.push(b'\'');
    String::from_utf8(buf).unwrap_or_default()
}

/// Go: `strconv.QuoteRune`.
#[must_use]
pub fn quote_rune(r: char) -> String {
    quote_rune_with(r, false, false)
}

/// Go: `strconv.QuoteRuneToASCII`.
#[must_use]
pub fn quote_rune_to_ascii(r: char) -> String {
    quote_rune_with(r, true, false)
}

/// Go: `strconv.QuoteRuneToGraphic`.
#[must_use]
pub fn quote_rune_to_graphic(r: char) -> String {
    quote_rune_with(r, false, true)
}

/// Go: `strconv.CanBackquote`.
#[must_use]
pub fn can_backquote(s: &str) -> bool {
    s.chars().all(|r| {
        if r.len_utf8() > 1 {
            // BOMs are invisible and should not be quoted.
            return r != '\u{FEFF}';
        }
        !((r < ' ' && r != '\t') || r == '`' || r == '\u{7F}')
    })
}

/// Go: `strconv.CanBackquote(string(b))`: false for bytes that are not valid UTF-8.
#[must_use]
pub fn can_backquote_bytes(s: &[u8]) -> bool {
    str::from_utf8(s).is_ok_and(can_backquote)
}

const fn unhex(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

/// One decoded element of a quoted literal: a rune (with Go's `multibyte` flag) or a single raw
/// byte (`\xNN` and octal escapes, which Go appends as bytes).
enum Unit {
    Rune(char, bool),
    Byte(u8),
}

/// Go: `strconv.UnquoteChar`. Returns the decoded unit and the remaining input.
fn unquote_unit(input: &str, quote: u8) -> Result<(Unit, &str), UnquoteError> {
    let mut chars = input.chars();
    let first = chars.next().ok_or(UnquoteError)?;
    if u32::from(first) == u32::from(quote) && (quote == b'\'' || quote == b'"') {
        return Err(UnquoteError);
    }
    if !first.is_ascii() {
        return Ok((Unit::Rune(first, true), chars.as_str()));
    }
    if first != '\\' {
        return Ok((
            Unit::Byte(u8::try_from(first).ok().ok_or(UnquoteError)?),
            chars.as_str(),
        ));
    }
    let esc = chars.next().ok_or(UnquoteError)?;
    let rest = chars.as_str();
    let simple = |r: char| Ok((Unit::Rune(r, false), rest));
    match esc {
        'a' => simple('\u{07}'),
        'b' => simple('\u{08}'),
        'f' => simple('\u{0C}'),
        'n' => simple('\n'),
        'r' => simple('\r'),
        't' => simple('\t'),
        'v' => simple('\u{0B}'),
        '\\' => simple('\\'),
        'x' | 'u' | 'U' => {
            let n = match esc {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let digits = rest.as_bytes().get(..n).ok_or(UnquoteError)?;
            let mut v: u32 = 0;
            for &d in digits {
                v = (v << 4) | unhex(d).ok_or(UnquoteError)?;
            }
            // All n bytes are ASCII hex digits, so n is a character boundary.
            let tail = rest.get(n..).ok_or(UnquoteError)?;
            if esc == 'x' {
                // single-byte string, possibly not UTF-8
                return Ok((Unit::Byte(u8::try_from(v).ok().ok_or(UnquoteError)?), tail));
            }
            let r = char::from_u32(v).ok_or(UnquoteError)?;
            Ok((Unit::Rune(r, true), tail))
        }
        '0'..='7' => {
            let mut v = u32::from(esc) - u32::from('0');
            let digits = rest.as_bytes().get(..2).ok_or(UnquoteError)?;
            for &d in digits {
                if !(b'0'..=b'7').contains(&d) {
                    return Err(UnquoteError);
                }
                v = (v << 3) | u32::from(d - b'0');
            }
            let byte = u8::try_from(v).ok().ok_or(UnquoteError)?;
            Ok((Unit::Byte(byte), rest.get(2..).ok_or(UnquoteError)?))
        }
        '\'' | '"' => {
            if u32::from(esc) != u32::from(quote) {
                return Err(UnquoteError);
            }
            simple(esc)
        }
        _ => Err(UnquoteError),
    }
}

/// Go: `strconv.UnquoteChar(s, quote)`: decodes the first character or escape of `s`.
///
/// Returns the value, Go's `multibyte` flag, and the tail. `\xNN` and octal escapes return
/// the rune whose value is that byte, as in Go.
///
/// # Errors
///
/// [`UnquoteError`] for an invalid escape or an unescaped quote character.
pub fn unquote_char(s: &str, quote: u8) -> Result<(char, bool, &str), UnquoteError> {
    let (unit, tail) = unquote_unit(s, quote)?;
    Ok(match unit {
        Unit::Rune(r, multibyte) => (r, multibyte, tail),
        Unit::Byte(b) => (char::from(b), false, tail),
    })
}

/// Go: `strconv.Unquote` over bytes: the exact bytes Go produces (possibly invalid UTF-8).
///
/// # Errors
///
/// [`UnquoteError`] when `s` is not a single complete Go string, rune or raw-string literal.
pub fn unquote_bytes(s: &str) -> Result<Vec<u8>, UnquoteError> {
    let input = s.as_bytes();
    let (&quote, body) = input.split_first().ok_or(UnquoteError)?;
    if input.len() < 2 {
        return Err(UnquoteError);
    }
    match quote {
        b'`' => {
            let end = body.iter().position(|&c| c == b'`').ok_or(UnquoteError)?;
            if end + 2 != input.len() {
                return Err(UnquoteError);
            }
            Ok(body
                .get(..end)
                .unwrap_or_default()
                .iter()
                .copied()
                .filter(|&c| c != b'\r')
                .collect())
        }
        b'"' | b'\'' => {
            let mut out = Vec::with_capacity(input.len());
            let mut rest = s.get(1..).ok_or(UnquoteError)?;
            while let Some(&c) = rest.as_bytes().first() {
                if c == quote {
                    break;
                }
                if c == b'\n' {
                    return Err(UnquoteError);
                }
                let (unit, tail) = unquote_unit(rest, quote)?;
                rest = tail;
                match unit {
                    Unit::Rune(r, _) => {
                        let mut tmp = [0; 4];
                        out.extend_from_slice(r.encode_utf8(&mut tmp).as_bytes());
                    }
                    Unit::Byte(b) => out.push(b),
                }
                if quote == b'\'' {
                    break;
                }
            }
            if rest.as_bytes() != [quote] {
                return Err(UnquoteError);
            }
            Ok(out)
        }
        _ => Err(UnquoteError),
    }
}

/// Go: `strconv.Unquote`.
///
/// When Go's result is not valid UTF-8 (`"\xff"`), each invalid byte is replaced with U+FFFD
/// ([`lossy_string`]); use [`unquote_bytes`] to keep them.
///
/// # Errors
///
/// [`UnquoteError`] when `s` is not a single complete Go string, rune or raw-string literal.
pub fn unquote(s: &str) -> Result<String, UnquoteError> {
    unquote_bytes(s).map(|bytes| lossy_string(&bytes))
}
