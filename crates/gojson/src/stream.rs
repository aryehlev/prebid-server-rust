//! The streaming decoders: `encoding/json`'s `Decoder` and jsoniter's `Decoder.Decode`.
//!
//! Prebid Server decodes HTTP response bodies with `json.NewDecoder(resp.Body).Decode(&v)`
//! (`pbs/usersync.go`, `analytics/pubstack/config.go`) and
//! `jsoniter.ConfigCompatibleWithStandardLibrary.NewDecoder(resp.Body).Decode(&v)`
//! (`modules/scope3/rtd/module.go`). Unlike `Unmarshal`, both stop after the first value and
//! ignore whatever follows it, and both report `EOF` for an empty body. `jsonutil.FindElement`
//! walks a document with `Decoder.Token`.
//!
//! The Rust decoders read from a byte slice holding the whole input, which is what the Go
//! readers deliver in the end.
//!
//! | Go | Rust |
//! |---|---|
//! | `json.NewDecoder(r).Decode(&v)` | [`decode_std`] |
//! | `json.NewDecoder(r)` | [`Decoder::new`] |
//! | `dec.Decode(&v)`, `dec.Token()` | [`Decoder::decode`], [`Decoder::token`] |
//! | `dec.More()` | [`Decoder::more`] |
//! | `dec.InputOffset()`, `dec.Buffered()` | [`Decoder::input_offset`], [`Decoder::buffered`] |
//! | `jsoniter...NewDecoder(r).Decode(&v)` | [`decode_jsoniter`] |
//!
//! ```
//! use pbs_gojson::{
//!     GoJson,
//!     stream::{self, StreamError},
//! };
//!
//! // pbs/usersync.go
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "pbs.googleResponse")]
//! pub struct GoogleResponse {
//!     pub success: bool,
//!     #[gojson(json = "error-codes")]
//!     pub error_codes: Option<Vec<String>>,
//! }
//!
//! # fn main() -> Result<(), StreamError> {
//! let mut gr = GoogleResponse::default();
//! // Trailing data after the first value is never read.
//! stream::decode_std(br#"{"success": true} {"success": "#, &mut gr)?;
//! assert!(gr.success);
//! assert_eq!(stream::decode_std(b"  ", &mut gr), Err(StreamError::Eof));
//! assert_eq!(
//!     stream::decode_std(br#"{"error-codes": ["#, &mut gr).map_err(|e| e.to_string()),
//!     Err("unexpected EOF".to_owned()),
//! );
//! # Ok(())
//! # }
//! ```

use core::{error::Error, fmt};

use crate::{
    error::JsonError,
    jsoniter::{self, JsoniterDecode},
    stdjson::{
        self, StdDecode,
        scanner::{Scan, Scanner},
    },
    strconv::json_quote_char,
    unicode::go_lossy,
};

/// The errors of the streaming decoders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// `io.EOF`: there is no value left (only whitespace, or nothing).
    Eof,
    /// `io.ErrUnexpectedEOF`: the input ends inside a value.
    UnexpectedEof,
    /// A `*json.SyntaxError` or a decode error (`*json.UnmarshalTypeError`, jsoniter's
    /// message, ...).
    Json(JsonError),
}

impl StreamError {
    /// The exact bytes of Go's `err.Error()`.
    #[must_use]
    pub fn go_text(&self) -> Vec<u8> {
        match self {
            Self::Eof => b"EOF".to_vec(),
            Self::UnexpectedEof => b"unexpected EOF".to_vec(),
            Self::Json(err) => err.go_text(),
        }
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => f.write_str("EOF"),
            Self::UnexpectedEof => f.write_str("unexpected EOF"),
            Self::Json(err) => fmt::Display::fmt(err, f),
        }
    }
}

impl Error for StreamError {}

impl From<JsonError> for StreamError {
    fn from(err: JsonError) -> Self {
        Self::Json(err)
    }
}

/// A token of [`Decoder::token`] (Go's `json.Token`).
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    /// `json.Delim`: one of `[`, `]`, `{` and `}`.
    Delim(u8),
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number, as a `float64`.
    Number(f64),
    /// A JSON string (an object key or a value), unescaped as `encoding/json` does.
    String(String),
    /// `null`.
    Null,
}

/// The decoder's position in the token stream (Go's `tokenState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenState {
    TopValue,
    ArrayStart,
    ArrayValue,
    ArrayComma,
    ObjectStart,
    ObjectKey,
    ObjectColon,
    ObjectValue,
    ObjectComma,
}

const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// `encoding/json`'s `Decoder` over an input that is already complete.
#[derive(Debug)]
pub struct Decoder<'a> {
    buf: &'a [u8],
    /// Start of the unread data (Go's `scanned + scanp`).
    scanp: usize,
    /// The scanner's byte count, kept across values as Go's `dec.scan.bytes` is.
    scan_bytes: i64,
    /// The sticky error of a failed `readValue`.
    err: Option<StreamError>,
    token_state: TokenState,
    token_stack: Vec<TokenState>,
}

impl<'a> Decoder<'a> {
    /// `json.NewDecoder(bytes.NewReader(data))`.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            buf: data,
            scanp: 0,
            scan_bytes: 0,
            err: None,
            token_state: TokenState::TopValue,
            token_stack: Vec::new(),
        }
    }

    /// `dec.InputOffset()`: the offset just past the last value or token returned.
    #[must_use]
    pub fn input_offset(&self) -> i64 {
        i64::try_from(self.scanp).unwrap_or(i64::MAX)
    }

    /// `dec.Buffered()`: the input not consumed yet.
    #[must_use]
    pub fn buffered(&self) -> &'a [u8] {
        self.buf.get(self.scanp..).unwrap_or_default()
    }

    fn syntax(&self, msg: &str) -> StreamError {
        StreamError::Json(JsonError::Syntax {
            msg: msg.to_owned(),
            offset: self.input_offset(),
        })
    }

    /// `dec.Decode(&v)`: decodes the next value into `v` as `json.Unmarshal` would, without
    /// looking past it.
    ///
    /// # Errors
    ///
    /// [`StreamError::Eof`] when no value is left; [`StreamError::UnexpectedEof`] when the
    /// input ends inside one; the scanner's `SyntaxError` (this one is sticky: every later call
    /// returns it again); `not at beginning of value` and the comma and colon errors in the
    /// middle of a [`Decoder::token`] walk; or the decode error (`UnmarshalTypeError`, ...).
    pub fn decode<T: StdDecode + ?Sized>(&mut self, v: &mut T) -> Result<(), StreamError> {
        // Go: encoding/json/stream.go Decoder.Decode
        if let Some(err) = &self.err {
            return Err(err.clone());
        }
        self.token_prepare_for_decode()?;
        if !self.token_value_allowed() {
            return Err(self.syntax("not at beginning of value"));
        }
        let n = self.read_value()?;
        let start = self.scanp;
        self.scanp += n;
        // readValue has checked the value's syntax; Unmarshal checks it again and then decodes
        // exactly as dec.d.unmarshal does.
        let res = stdjson::unmarshal(self.buf.get(start..self.scanp).unwrap_or_default(), v);
        self.token_value_end();
        res.map_err(StreamError::Json)
    }

    /// `readValue`: the length of the next value, scanned from `scanp`.
    fn read_value(&mut self) -> Result<usize, StreamError> {
        // Go: encoding/json/stream.go Decoder.readValue
        let mut scan = Scanner::new();
        scan.bytes = self.scan_bytes;
        // The scanner's nesting depth: Go asks stateEndValue(' ') whether a closing bracket
        // ended the top-level value, which is whether the depth is back to zero.
        let mut depth = 0_usize;
        let mut scanp = self.scanp;
        let mut complete = false;
        while let Some(&c) = self.buf.get(scanp) {
            scan.bytes += 1;
            match scan.step(c) {
                Scan::End => {
                    // scanEnd is delayed one byte.
                    scan.bytes -= 1;
                    complete = true;
                    break;
                }
                Scan::BeginObject | Scan::BeginArray => depth += 1,
                Scan::EndObject | Scan::EndArray => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        scanp += 1;
                        complete = true;
                        break;
                    }
                }
                Scan::Error => {
                    self.scan_bytes = scan.bytes;
                    let err = StreamError::Json(scan.error_value());
                    self.err = Some(err.clone());
                    return Err(err);
                }
                Scan::Continue
                | Scan::BeginLiteral
                | Scan::ObjectKey
                | Scan::ObjectValue
                | Scan::ArrayValue
                | Scan::SkipSpace => {}
            }
            scanp += 1;
        }
        // At the end of the input a space completes a number.
        if !complete && scan.step(b' ') != Scan::End {
            self.scan_bytes = scan.bytes;
            let rest = self.buf.get(self.scanp..).unwrap_or_default();
            let err = if rest.iter().any(|&c| !is_space(c)) {
                StreamError::UnexpectedEof
            } else {
                StreamError::Eof
            };
            self.err = Some(err.clone());
            return Err(err);
        }
        self.scan_bytes = scan.bytes;
        Ok(scanp - self.scanp)
    }

    /// `tokenPrepareForDecode`: steps over the comma or colon in front of a value.
    fn token_prepare_for_decode(&mut self) -> Result<(), StreamError> {
        // Go: encoding/json/stream.go Decoder.tokenPrepareForDecode
        match self.token_state {
            TokenState::ArrayComma => {
                if self.peek()? != b',' {
                    return Err(self.syntax("expected comma after array element"));
                }
                self.scanp += 1;
                self.token_state = TokenState::ArrayValue;
            }
            TokenState::ObjectColon => {
                if self.peek()? != b':' {
                    return Err(self.syntax("expected colon after object key"));
                }
                self.scanp += 1;
                self.token_state = TokenState::ObjectValue;
            }
            TokenState::TopValue
            | TokenState::ArrayStart
            | TokenState::ArrayValue
            | TokenState::ObjectStart
            | TokenState::ObjectKey
            | TokenState::ObjectValue
            | TokenState::ObjectComma => {}
        }
        Ok(())
    }

    const fn token_value_allowed(&self) -> bool {
        matches!(
            self.token_state,
            TokenState::TopValue
                | TokenState::ArrayStart
                | TokenState::ArrayValue
                | TokenState::ObjectValue
        )
    }

    const fn token_value_end(&mut self) {
        match self.token_state {
            TokenState::ArrayStart | TokenState::ArrayValue => {
                self.token_state = TokenState::ArrayComma;
            }
            TokenState::ObjectValue => self.token_state = TokenState::ObjectComma,
            TokenState::TopValue
            | TokenState::ArrayComma
            | TokenState::ObjectStart
            | TokenState::ObjectKey
            | TokenState::ObjectColon
            | TokenState::ObjectComma => {}
        }
    }

    /// `peek`: the next non-space byte, which becomes the read position.
    fn peek(&mut self) -> Result<u8, StreamError> {
        // Go: encoding/json/stream.go Decoder.peek
        let rest = self.buf.get(self.scanp..).unwrap_or_default();
        let Some(i) = rest.iter().position(|&c| !is_space(c)) else {
            return Err(StreamError::Eof);
        };
        self.scanp += i;
        Ok(rest.get(i).copied().unwrap_or(0))
    }

    /// `More()`: whether another element follows in the current array or object.
    pub fn more(&mut self) -> bool {
        // Go: encoding/json/stream.go Decoder.More
        self.peek().is_ok_and(|c| c != b']' && c != b'}')
    }

    fn pop_state(&mut self) {
        self.token_state = self.token_stack.pop().unwrap_or(TokenState::TopValue);
        self.token_value_end();
    }

    fn token_error(&self, c: u8) -> StreamError {
        // Go: encoding/json/stream.go Decoder.tokenError
        let context = match self.token_state {
            TokenState::TopValue
            | TokenState::ArrayStart
            | TokenState::ArrayValue
            | TokenState::ObjectValue => " looking for beginning of value",
            TokenState::ArrayComma => " after array element",
            TokenState::ObjectKey => " looking for beginning of object key string",
            TokenState::ObjectColon => " after object key",
            TokenState::ObjectComma => " after object key:value pair",
            TokenState::ObjectStart => "",
        };
        let mut msg = String::from("invalid character ");
        msg.push_str(&go_lossy(&json_quote_char(c)));
        msg.push_str(context);
        self.syntax(&msg)
    }

    /// `dec.Token()`: the next token; commas and colons are checked and skipped.
    ///
    /// # Errors
    ///
    /// [`StreamError::Eof`] at the end of the input (even inside an unfinished array or
    /// object), an `invalid character ...` `SyntaxError` for a misplaced delimiter, or
    /// [`Decoder::decode`]'s error for a string, number or literal.
    pub fn token(&mut self) -> Result<Token, StreamError> {
        // Go: encoding/json/stream.go Decoder.Token
        loop {
            let c = self.peek()?;
            match c {
                b'[' | b'{' => {
                    if !self.token_value_allowed() {
                        return Err(self.token_error(c));
                    }
                    self.scanp += 1;
                    self.token_stack.push(self.token_state);
                    self.token_state = if c == b'[' {
                        TokenState::ArrayStart
                    } else {
                        TokenState::ObjectStart
                    };
                    return Ok(Token::Delim(c));
                }
                b']' => {
                    if !matches!(
                        self.token_state,
                        TokenState::ArrayStart | TokenState::ArrayComma
                    ) {
                        return Err(self.token_error(c));
                    }
                    self.scanp += 1;
                    self.pop_state();
                    return Ok(Token::Delim(c));
                }
                b'}' => {
                    if !matches!(
                        self.token_state,
                        TokenState::ObjectStart | TokenState::ObjectComma
                    ) {
                        return Err(self.token_error(c));
                    }
                    self.scanp += 1;
                    self.pop_state();
                    return Ok(Token::Delim(c));
                }
                b':' => {
                    if self.token_state != TokenState::ObjectColon {
                        return Err(self.token_error(c));
                    }
                    self.scanp += 1;
                    self.token_state = TokenState::ObjectValue;
                }
                b',' => match self.token_state {
                    TokenState::ArrayComma => {
                        self.scanp += 1;
                        self.token_state = TokenState::ArrayValue;
                    }
                    TokenState::ObjectComma => {
                        self.scanp += 1;
                        self.token_state = TokenState::ObjectKey;
                    }
                    TokenState::TopValue
                    | TokenState::ArrayStart
                    | TokenState::ArrayValue
                    | TokenState::ObjectStart
                    | TokenState::ObjectKey
                    | TokenState::ObjectColon
                    | TokenState::ObjectValue => return Err(self.token_error(c)),
                },
                b'"' if matches!(
                    self.token_state,
                    TokenState::ObjectStart | TokenState::ObjectKey
                ) =>
                {
                    let old = self.token_state;
                    self.token_state = TokenState::TopValue;
                    let mut key = String::new();
                    let res = self.decode(&mut key);
                    self.token_state = old;
                    res?;
                    self.token_state = TokenState::ObjectColon;
                    return Ok(Token::String(key));
                }
                _ => {
                    if !self.token_value_allowed() {
                        return Err(self.token_error(c));
                    }
                    let mut value = serde_json::Value::Null;
                    self.decode(&mut value)?;
                    return Ok(literal_token(value));
                }
            }
        }
    }
}

/// The token a literal decodes to as an `interface{}`.
fn literal_token(value: serde_json::Value) -> Token {
    match value {
        serde_json::Value::Bool(b) => Token::Bool(b),
        serde_json::Value::Number(n) => Token::Number(n.as_f64().unwrap_or_default()),
        serde_json::Value::String(s) => Token::String(s),
        // Only literals reach here: arrays and objects are delimiters.
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Token::Null
        }
    }
}

/// `json.NewDecoder(r).Decode(&v)` with the whole body in `data`: decodes the first value and
/// ignores the rest of the input.
///
/// # Errors
///
/// [`StreamError::Eof`] for an empty or blank body, [`StreamError::UnexpectedEof`] for a
/// truncated value, otherwise the `SyntaxError` or decode error `json.Unmarshal` would report
/// for the first value.
pub fn decode_std<T: StdDecode + ?Sized>(data: &[u8], v: &mut T) -> Result<(), StreamError> {
    Decoder::new(data).decode(v)
}

/// `jsoniter.ConfigCompatibleWithStandardLibrary.NewDecoder(r).Decode(&v)` with the whole
/// body in `data`: jsoniter's `ReadVal` without `Unmarshal`'s check for trailing bytes.
///
/// # Errors
///
/// [`StreamError::Eof`] for an empty body, otherwise jsoniter's error (a blank body is an
/// error such as `ReadVal: ...`, as in Go).
pub fn decode_jsoniter<T: JsoniterDecode + ?Sized>(
    data: &[u8],
    v: &mut T,
) -> Result<(), StreamError> {
    // Go: json-iterator/go adapter.go Decoder.Decode
    if data.is_empty() {
        return Err(StreamError::Eof);
    }
    // jsoniter.Unmarshal is ReadVal followed by the trailing-bytes check, whose error is only
    // recorded when ReadVal left none.
    match jsoniter::unmarshal(data, v) {
        Err(err) if err.go_text().starts_with(TRAILING_BYTES) => Ok(()),
        res => res.map_err(StreamError::Json),
    }
}

/// The start of the error `jsoniter.Unmarshal` adds for bytes after the value.
const TRAILING_BYTES: &[u8] = b"Unmarshal: there are bytes left after unmarshal, error found in #";

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{Decoder, StreamError, Token, decode_jsoniter, decode_std};
    use crate::{error::JsonError, stdjson};

    fn syntax(msg: &str, offset: i64) -> StreamError {
        StreamError::Json(JsonError::Syntax {
            msg: msg.to_owned(),
            offset,
        })
    }

    fn text(v: &Value) -> Vec<u8> {
        stdjson::marshal(v).unwrap_or_default()
    }

    #[test]
    fn values_then_sticky_error() -> Result<(), String> {
        let mut dec = Decoder::new(b"{} 12 x");
        let mut v = Value::Null;
        dec.decode(&mut v).map_err(|e| e.to_string())?;
        dec.decode(&mut v).map_err(|e| e.to_string())?;
        if text(&v) != b"12" || dec.input_offset() != 5 {
            return Err(format!("got {v} at {}", dec.input_offset()));
        }
        // The scanner's byte count runs on across values, as Go's does.
        let want = syntax("invalid character 'x' looking for beginning of value", 7);
        for _ in 0..2 {
            let got = dec.decode(&mut v);
            if got.as_ref().err() != Some(&want) {
                return Err(format!("got {got:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn end_of_input() -> Result<(), String> {
        let mut v = Value::Null;
        let cases: [(&[u8], StreamError); 4] = [
            (b"", StreamError::Eof),
            (b" \n", StreamError::Eof),
            (b"[1,", StreamError::UnexpectedEof),
            (b" tru", StreamError::UnexpectedEof),
        ];
        for (data, want) in cases {
            if decode_std(data, &mut v) != Err(want.clone()) {
                return Err(format!("{data:?}: want {want}"));
            }
        }
        decode_std(b"1e3", &mut v).map_err(|e| e.to_string())?;
        if text(&v) != b"1000" {
            return Err(format!("1e3: got {v}"));
        }
        Ok(())
    }

    #[test]
    fn token_errors() -> Result<(), String> {
        let mut dec = Decoder::new(b"[1 2]");
        for want in [Token::Delim(b'['), Token::Number(1.0)] {
            let got = dec.token().map_err(|e| e.to_string())?;
            if got != want {
                return Err(format!("got {got:?}, want {want:?}"));
            }
        }
        let token_err = dec.token();
        if token_err != Err(syntax("invalid character '2' after array element", 3)) {
            return Err(format!("got {token_err:?}"));
        }
        let mut v = Value::Null;
        let comma_err = dec.decode(&mut v);
        if comma_err != Err(syntax("expected comma after array element", 3)) {
            return Err(format!("decode: got {comma_err:?}"));
        }
        let mut obj = Decoder::new(br#"{"a" 1}"#);
        let open = obj.token();
        if open != Ok(Token::Delim(b'{'))
            || obj.token() != Ok(Token::String("a".to_owned()))
            || !obj.more()
        {
            return Err(format!("object start: got {open:?}"));
        }
        let colon_err = obj.decode(&mut v);
        if colon_err != Err(syntax("expected colon after object key", 5)) {
            return Err(format!("colon: got {colon_err:?}"));
        }
        Ok(())
    }

    #[test]
    fn jsoniter_decoder() -> Result<(), String> {
        let mut v = Value::Null;
        if decode_jsoniter(b"", &mut v) != Err(StreamError::Eof) {
            return Err("empty input".to_owned());
        }
        decode_jsoniter(b" 7 trailing", &mut v).map_err(|e| e.to_string())?;
        if text(&v) != b"7" {
            return Err(format!("got {v}"));
        }
        let got = decode_jsoniter(b" ", &mut v).map_err(|e| e.to_string());
        if !got
            .as_ref()
            .is_err_and(|e| e.starts_with("Read: unexpected value type: 0"))
        {
            return Err(format!("blank input: got {got:?}"));
        }
        Ok(())
    }
}
