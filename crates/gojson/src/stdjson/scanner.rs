//! `encoding/json`'s scanner state machine (`scanner.go`): syntax checking with Go's exact
//! `SyntaxError` texts, plus `Compact` and `Valid` (`indent.go`).

use crate::{error::JsonError, strconv::json_quote_char, unicode::go_lossy};

/// The scanner's result codes, in Go's order (`compact` relies on `>= SkipSpace`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Scan {
    Continue,
    BeginLiteral,
    BeginObject,
    ObjectKey,
    ObjectValue,
    EndObject,
    BeginArray,
    ArrayValue,
    EndArray,
    SkipSpace,
    End,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parse {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    BeginValueOrEmpty,
    BeginValue,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU,
    InStringEscU1,
    InStringEscU12,
    InStringEscU123,
    Neg,
    One,
    Zero,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    T,
    Tr,
    Tru,
    F,
    Fa,
    Fal,
    Fals,
    N,
    Nu,
    Nul,
    Error,
}

const MAX_NESTING_DEPTH: usize = 10000;

const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// Go's `scanner`.
#[derive(Debug)]
pub(crate) struct Scanner {
    step: State,
    end_top: bool,
    parse_state: Vec<Parse>,
    err: Option<(String, i64)>,
    /// Bytes consumed so far (Go's `scanner.bytes`, the `SyntaxError.Offset`).
    pub(crate) bytes: i64,
}

impl Scanner {
    pub(crate) const fn new() -> Self {
        Self {
            step: State::BeginValue,
            end_top: false,
            parse_state: Vec::new(),
            err: None,
            bytes: 0,
        }
    }

    pub(crate) fn error_value(&self) -> JsonError {
        match &self.err {
            Some((msg, offset)) => JsonError::Syntax {
                msg: msg.clone(),
                offset: *offset,
            },
            None => JsonError::Syntax {
                msg: "unexpected end of JSON input".to_owned(),
                offset: self.bytes,
            },
        }
    }

    fn error(&mut self, c: u8, context: &str) -> Scan {
        self.step = State::Error;
        let mut msg = String::from("invalid character ");
        msg.push_str(&go_lossy(&json_quote_char(c)));
        msg.push(' ');
        msg.push_str(context);
        self.err = Some((msg, self.bytes));
        Scan::Error
    }

    /// Go's `scanner.eof`.
    pub(crate) fn eof(&mut self) -> Scan {
        if self.err.is_some() {
            return Scan::Error;
        }
        if self.end_top {
            return Scan::End;
        }
        let _scan = self.step(b' ');
        if self.end_top {
            return Scan::End;
        }
        if self.err.is_none() {
            self.err = Some(("unexpected end of JSON input".to_owned(), self.bytes));
        }
        Scan::Error
    }

    fn push_parse_state(&mut self, c: u8, state: Parse, success: Scan) -> Scan {
        self.parse_state.push(state);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            return success;
        }
        self.error(c, "exceeded max depth")
    }

    fn pop_parse_state(&mut self) {
        let _popped = self.parse_state.pop();
        if self.parse_state.is_empty() {
            self.step = State::EndTop;
            self.end_top = true;
        } else {
            self.step = State::EndValue;
        }
    }

    /// Feeds one byte.
    pub(crate) fn step(&mut self, c: u8) -> Scan {
        // Go: encoding/json/scanner.go state* functions
        match self.step {
            State::BeginValueOrEmpty => {
                if is_space(c) {
                    return Scan::SkipSpace;
                }
                if c == b']' {
                    return self.end_value(c);
                }
                self.begin_value(c)
            }
            State::BeginValue => self.begin_value(c),
            State::BeginStringOrEmpty => self.begin_string_or_empty(c),
            State::BeginString => self.begin_string(c),
            State::EndValue => self.end_value(c),
            State::EndTop => {
                if !is_space(c) {
                    let _scan = self.error(c, "after top-level value");
                }
                Scan::End
            }
            State::InString => self.in_string(c),
            State::InStringEsc => self.in_string_esc(c),
            State::InStringEscU => self.hex(c, State::InStringEscU1),
            State::InStringEscU1 => self.hex(c, State::InStringEscU12),
            State::InStringEscU12 => self.hex(c, State::InStringEscU123),
            State::InStringEscU123 => self.hex(c, State::InString),
            State::Neg => self.neg(c),
            State::One => {
                if c.is_ascii_digit() {
                    return Scan::Continue;
                }
                self.zero(c)
            }
            State::Zero => self.zero(c),
            State::Dot => {
                if c.is_ascii_digit() {
                    self.step = State::Dot0;
                    return Scan::Continue;
                }
                self.error(c, "after decimal point in numeric literal")
            }
            State::Dot0 => self.dot0(c),
            State::E => {
                if c == b'+' || c == b'-' {
                    self.step = State::ESign;
                    return Scan::Continue;
                }
                self.e_sign(c)
            }
            State::ESign => self.e_sign(c),
            State::E0 => {
                if c.is_ascii_digit() {
                    return Scan::Continue;
                }
                self.end_value(c)
            }
            State::T => self.literal(c, b'r', State::Tr, "in literal true (expecting 'r')"),
            State::Tr => self.literal(c, b'u', State::Tru, "in literal true (expecting 'u')"),
            State::Tru => self.literal(c, b'e', State::EndValue, "in literal true (expecting 'e')"),
            State::F => self.literal(c, b'a', State::Fa, "in literal false (expecting 'a')"),
            State::Fa => self.literal(c, b'l', State::Fal, "in literal false (expecting 'l')"),
            State::Fal => self.literal(c, b's', State::Fals, "in literal false (expecting 's')"),
            State::Fals => {
                self.literal(c, b'e', State::EndValue, "in literal false (expecting 'e')")
            }
            State::N => self.literal(c, b'u', State::Nu, "in literal null (expecting 'u')"),
            State::Nu => self.literal(c, b'l', State::Nul, "in literal null (expecting 'l')"),
            State::Nul => self.literal(c, b'l', State::EndValue, "in literal null (expecting 'l')"),
            State::Error => Scan::Error,
        }
    }

    /// Go's `stateBeginStringOrEmpty`.
    fn begin_string_or_empty(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        if c == b'}' {
            if let Some(last) = self.parse_state.last_mut() {
                *last = Parse::ObjectValue;
            }
            return self.end_value(c);
        }
        self.begin_string(c)
    }

    /// Go's `stateInString`.
    fn in_string(&mut self, c: u8) -> Scan {
        if c == b'"' {
            self.step = State::EndValue;
            return Scan::Continue;
        }
        if c == b'\\' {
            self.step = State::InStringEsc;
            return Scan::Continue;
        }
        if c < 0x20 {
            return self.error(c, "in string literal");
        }
        Scan::Continue
    }

    /// Go's `stateInStringEsc`.
    fn in_string_esc(&mut self, c: u8) -> Scan {
        if matches!(c, b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"') {
            self.step = State::InString;
            return Scan::Continue;
        }
        if c == b'u' {
            self.step = State::InStringEscU;
            return Scan::Continue;
        }
        self.error(c, "in string escape code")
    }

    /// Go's `stateNeg`.
    fn neg(&mut self, c: u8) -> Scan {
        match c {
            b'0' => {
                self.step = State::Zero;
                Scan::Continue
            }
            b'1'..=b'9' => {
                self.step = State::One;
                Scan::Continue
            }
            _ => self.error(c, "in numeric literal"),
        }
    }

    /// Go's `stateDot0`.
    fn dot0(&mut self, c: u8) -> Scan {
        if c.is_ascii_digit() {
            return Scan::Continue;
        }
        if c == b'e' || c == b'E' {
            self.step = State::E;
            return Scan::Continue;
        }
        self.end_value(c)
    }

    fn literal(&mut self, c: u8, want: u8, next: State, context: &str) -> Scan {
        if c == want {
            self.step = next;
            return Scan::Continue;
        }
        self.error(c, context)
    }

    fn hex(&mut self, c: u8, next: State) -> Scan {
        if c.is_ascii_hexdigit() {
            self.step = next;
            return Scan::Continue;
        }
        self.error(c, "in \\u hexadecimal character escape")
    }

    fn zero(&mut self, c: u8) -> Scan {
        if c == b'.' {
            self.step = State::Dot;
            return Scan::Continue;
        }
        if c == b'e' || c == b'E' {
            self.step = State::E;
            return Scan::Continue;
        }
        self.end_value(c)
    }

    fn e_sign(&mut self, c: u8) -> Scan {
        if c.is_ascii_digit() {
            self.step = State::E0;
            return Scan::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }

    fn begin_value(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        match c {
            b'{' => {
                self.step = State::BeginStringOrEmpty;
                self.push_parse_state(c, Parse::ObjectKey, Scan::BeginObject)
            }
            b'[' => {
                self.step = State::BeginValueOrEmpty;
                self.push_parse_state(c, Parse::ArrayValue, Scan::BeginArray)
            }
            b'"' => {
                self.step = State::InString;
                Scan::BeginLiteral
            }
            b'-' => {
                self.step = State::Neg;
                Scan::BeginLiteral
            }
            b'0' => {
                self.step = State::Zero;
                Scan::BeginLiteral
            }
            b't' => {
                self.step = State::T;
                Scan::BeginLiteral
            }
            b'f' => {
                self.step = State::F;
                Scan::BeginLiteral
            }
            b'n' => {
                self.step = State::N;
                Scan::BeginLiteral
            }
            b'1'..=b'9' => {
                self.step = State::One;
                Scan::BeginLiteral
            }
            _ => self.error(c, "looking for beginning of value"),
        }
    }

    fn begin_string(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        if c == b'"' {
            self.step = State::InString;
            return Scan::BeginLiteral;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn end_value(&mut self, c: u8) -> Scan {
        let Some(&ps) = self.parse_state.last() else {
            self.step = State::EndTop;
            self.end_top = true;
            return self.step(c);
        };
        if is_space(c) {
            self.step = State::EndValue;
            return Scan::SkipSpace;
        }
        match ps {
            Parse::ObjectKey => {
                if c == b':' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = Parse::ObjectValue;
                    }
                    self.step = State::BeginValue;
                    return Scan::ObjectKey;
                }
                self.error(c, "after object key")
            }
            Parse::ObjectValue => {
                if c == b',' {
                    if let Some(last) = self.parse_state.last_mut() {
                        *last = Parse::ObjectKey;
                    }
                    self.step = State::BeginString;
                    return Scan::ObjectValue;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Scan::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            Parse::ArrayValue => {
                if c == b',' {
                    self.step = State::BeginValue;
                    return Scan::ArrayValue;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Scan::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }
}

/// Go's `checkValid`: the `SyntaxError` for the first syntax problem, if any.
pub(crate) fn check_valid(data: &[u8]) -> Result<(), JsonError> {
    let mut scan = Scanner::new();
    for &c in data {
        scan.bytes += 1;
        if scan.step(c) == Scan::Error {
            return Err(scan.error_value());
        }
    }
    if scan.eof() == Scan::Error {
        return Err(scan.error_value());
    }
    Ok(())
}

/// Go's `json.Valid`.
#[must_use]
pub fn valid(data: &[u8]) -> bool {
    check_valid(data).is_ok()
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_digit(v: u8) -> u8 {
    HEX.get(usize::from(v & 0xF)).copied().unwrap_or(b'0')
}

/// Go's `appendCompact`: `src` without insignificant whitespace (with `escape`, also
/// HTML-escaping `<`, `>`, `&`, U+2028 and U+2029), or the scanner's error.
pub(crate) fn append_compact(dst: &mut Vec<u8>, src: &[u8], escape: bool) -> Result<(), JsonError> {
    // Go: encoding/json/indent.go appendCompact
    let orig_len = dst.len();
    let mut scan = Scanner::new();
    let mut start = 0;
    for (i, &c) in src.iter().enumerate() {
        if escape && (c == b'<' || c == b'>' || c == b'&') {
            dst.extend_from_slice(src.get(start..i).unwrap_or_default());
            dst.extend_from_slice(&[b'\\', b'u', b'0', b'0', hex_digit(c >> 4), hex_digit(c)]);
            start = i + 1;
        }
        if escape
            && c == 0xE2
            && i + 2 < src.len()
            && src.get(i + 1) == Some(&0x80)
            && src.get(i + 2).is_some_and(|&b| b & !1 == 0xA8)
        {
            dst.extend_from_slice(src.get(start..i).unwrap_or_default());
            let last = src.get(i + 2).copied().unwrap_or(0);
            dst.extend_from_slice(&[b'\\', b'u', b'2', b'0', b'2', hex_digit(last)]);
            start = i + 3;
        }
        let v = scan.step(c);
        if v >= Scan::SkipSpace {
            if v == Scan::Error {
                break;
            }
            dst.extend_from_slice(src.get(start..i).unwrap_or_default());
            start = i + 1;
        }
    }
    if scan.eof() == Scan::Error {
        dst.truncate(orig_len);
        return Err(scan.error_value());
    }
    dst.extend_from_slice(src.get(start..).unwrap_or_default());
    Ok(())
}

/// Go's `json.Compact` (no HTML escaping): the compacted bytes, or the syntax error.
///
/// # Errors
///
/// Returns the `SyntaxError` Go reports for malformed input.
pub fn compact(src: &[u8]) -> Result<Vec<u8>, JsonError> {
    let mut out = Vec::with_capacity(src.len());
    append_compact(&mut out, src, false)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{check_valid, compact};

    fn syntax(input: &[u8]) -> String {
        match check_valid(input) {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn syntax_messages() -> Result<(), String> {
        // Go: encoding/json checkValid / SyntaxError texts
        let cases: [(&[u8], &str); 7] = [
            (b"{\"a\":1}", ""),
            (b"", "unexpected end of JSON input"),
            (b"{\"a\" 1}", "invalid character '1' after object key"),
            (
                b"[1,]",
                "invalid character ']' looking for beginning of value",
            ),
            (b"{} x", "invalid character 'x' after top-level value"),
            (b"\"\\q\"", "invalid character 'q' in string escape code"),
            (
                b"tru",
                "invalid character ' ' in literal true (expecting 'e')",
            ),
        ];
        for (input, want) in cases {
            let got = syntax(input);
            if got != want {
                return Err(format!(
                    "{:?}: got {got:?}, want {want:?}",
                    String::from_utf8_lossy(input)
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn compacts() -> Result<(), String> {
        // Go: json.Compact
        let out = compact(b" { \"a\" : [ 1 , 2 ] } ").map_err(|e| e.to_string())?;
        if out != b"{\"a\":[1,2]}" {
            return Err(format!("compact: {:?}", String::from_utf8_lossy(&out)));
        }
        if compact(b"{").is_ok() {
            return Err("compact accepted invalid input".to_owned());
        }
        Ok(())
    }
}
