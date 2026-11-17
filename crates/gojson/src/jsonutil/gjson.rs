//! The part of `github.com/tidwall/gjson` (v1.17.1) that `jsonutil` uses: `Parse` of a whole
//! value and `Result.String()`.

use core::str::from_utf8;

use crate::strconv::{FloatFmt, append_float, non_finite_text, parse_float_go};

/// gjson's value types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Null,
    False,
    Number,
    String,
    True,
    Json,
}

/// A gjson `Result` (the fields `jsonutil` reads).
#[derive(Debug)]
pub(crate) struct GResult<'a> {
    pub(crate) kind: Kind,
    pub(crate) raw: &'a [u8],
    num: f64,
    s: Vec<u8>,
}

impl GResult<'_> {
    /// `Result.Exists()`.
    pub(crate) const fn exists(&self) -> bool {
        !matches!(self.kind, Kind::Null) || !self.raw.is_empty()
    }

    /// `Result.String()`.
    pub(crate) fn string(&self) -> Vec<u8> {
        match self.kind {
            Kind::Null => Vec::new(),
            Kind::False => b"false".to_vec(),
            Kind::True => b"true".to_vec(),
            Kind::Json => self.raw.to_vec(),
            Kind::String => self.s.clone(),
            Kind::Number => {
                let digits = self.raw.strip_prefix(b"-").unwrap_or(self.raw);
                if !self.raw.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                    return self.raw.to_vec();
                }
                let mut out = Vec::new();
                if self.num.is_finite() {
                    append_float(&mut out, self.num, FloatFmt::F, false);
                } else {
                    out.extend_from_slice(non_finite_text(self.num).as_bytes());
                }
                out
            }
        }
    }
}

fn tonum(json: &[u8]) -> (&[u8], f64) {
    let mut end = json.len();
    for (i, &c) in json.iter().enumerate().skip(1) {
        if (c <= b'-' && (c <= b' ' || c == b',')) || c == b']' || c == b'}' {
            end = i;
            break;
        }
    }
    let raw = json.get(..end).unwrap_or_default();
    (raw, parse_float_go(raw, false).0)
}

fn tolit(json: &[u8]) -> &[u8] {
    let end = json
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, c)| !c.is_ascii_lowercase())
        .map_or(json.len(), |(i, _)| i);
    json.get(..end).unwrap_or_default()
}

fn runeit(json: &[u8]) -> u32 {
    let hex = json.get(..4).unwrap_or_default();
    from_utf8(hex)
        .ok()
        .and_then(|s| u32::from_str_radix(s, 16).ok())
        .filter(|_| hex.iter().all(u8::is_ascii_hexdigit))
        .unwrap_or(0)
}

fn push_rune(out: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut buf = [0_u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

/// gjson `unescape`.
fn unescape(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(json.len());
    let mut i = 0;
    while let Some(&c) = json.get(i) {
        if c < b' ' {
            return out;
        }
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&e) = json.get(i) else {
            return out;
        };
        match e {
            b'\\' | b'/' | b'"' => out.push(e),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                if i + 5 > json.len() {
                    return out;
                }
                let mut r = runeit(json.get(i + 1..).unwrap_or_default());
                i += 5;
                if (0xD800..0xE000).contains(&r)
                    && json.len() - i >= 6
                    && json.get(i) == Some(&b'\\')
                    && json.get(i + 1) == Some(&b'u')
                {
                    let r2 = runeit(json.get(i + 2..).unwrap_or_default());
                    r = if (0xD800..0xDC00).contains(&r) && (0xDC00..0xE000).contains(&r2) {
                        ((r - 0xD800) << 10 | (r2 - 0xDC00)) + 0x10000
                    } else {
                        0xFFFD
                    };
                    i += 6;
                }
                push_rune(&mut out, r);
                continue;
            }
            _ => return out,
        }
        i += 1;
    }
    out
}

/// gjson `tostr`: the raw string token and its unescaped value.
fn tostr(json: &[u8]) -> (&[u8], Vec<u8>) {
    let mut i = 1;
    while let Some(&c) = json.get(i) {
        if c > b'\\' {
            i += 1;
            continue;
        }
        if c == b'"' {
            return (
                json.get(..=i).unwrap_or_default(),
                json.get(1..i).unwrap_or_default().to_vec(),
            );
        }
        if c == b'\\' {
            i += 1;
            while let Some(&byte) = json.get(i) {
                if byte > b'\\' {
                    i += 1;
                    continue;
                }
                if byte == b'"' {
                    if json.get(i - 1) == Some(&b'\\') {
                        let n = (1..i - 1)
                            .rev()
                            .take_while(|&j| json.get(j) == Some(&b'\\'))
                            .count();
                        if n % 2 == 0 {
                            i += 1;
                            continue;
                        }
                    }
                    return (
                        json.get(..=i).unwrap_or_default(),
                        unescape(json.get(1..i).unwrap_or_default()),
                    );
                }
                i += 1;
            }
            let raw = if i + 1 < json.len() {
                json.get(..=i).unwrap_or_default()
            } else {
                json.get(..i).unwrap_or_default()
            };
            return (raw, unescape(json.get(1..i).unwrap_or_default()));
        }
        i += 1;
    }
    (json, json.get(1..).unwrap_or_default().to_vec())
}

/// gjson `Parse`.
pub(crate) fn parse(json: &[u8]) -> GResult<'_> {
    let mut result = GResult {
        kind: Kind::Null,
        raw: &[],
        num: 0.0,
        s: Vec::new(),
    };
    for (i, &c) in json.iter().enumerate() {
        let rest = json.get(i..).unwrap_or_default();
        if c == b'{' || c == b'[' {
            result.kind = Kind::Json;
            result.raw = rest;
            return result;
        }
        if c <= b' ' {
            continue;
        }
        match c {
            b'+' | b'-' | b'0'..=b'9' | b'i' | b'I' | b'N' => {
                result.kind = Kind::Number;
                (result.raw, result.num) = tonum(rest);
            }
            b'n' => {
                if json.get(i + 1).is_some_and(|&n| n != b'u') {
                    result.kind = Kind::Number;
                    (result.raw, result.num) = tonum(rest);
                } else {
                    result.kind = Kind::Null;
                    result.raw = tolit(rest);
                }
            }
            b't' => {
                result.kind = Kind::True;
                result.raw = tolit(rest);
            }
            b'f' => {
                result.kind = Kind::False;
                result.raw = tolit(rest);
            }
            b'"' => {
                result.kind = Kind::String;
                (result.raw, result.s) = tostr(rest);
            }
            _ => {
                return GResult {
                    kind: Kind::Null,
                    raw: &[],
                    num: 0.0,
                    s: Vec::new(),
                };
            }
        }
        return result;
    }
    result
}
