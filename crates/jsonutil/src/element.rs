//! Glue (Rule B (d)): `DropElement`'s output is bytes Prebid writes (the Audience Network adapter
//! sends the request body with `consented_providers_settings` cut out), a cut of the input at the
//! offsets `FindElement` computes from the token walk of `encoding/json`'s `Decoder`. The walk's
//! states, its errors, its index panics and its offsets are ported here
//! (`json.find-element-tokens`, restored); `serde_json` reads each scalar and the matched value,
//! so a malformed value fails with `serde_json`'s message where Go's decoder fails with its own
//! (`json.find-element-invalid`).
//!
//! Go: `util/jsonutil/jsonutil.go` (`FindElement`, `DropElement`) and `encoding/json/stream.go`
//! (`Decoder.Token`, `Decode`, `More`, `InputOffset`).

use std::{
    error::Error,
    fmt::{self, Display},
};

use serde::de::DeserializeOwned;
use serde_json::{Deserializer, Value, error::Category};

use crate::replace_lone_surrogates;

/// What [`find_element`] found: Go's `found`, `startIndex` and `endIndex`, the span
/// [`drop_element`] cuts out. When nothing is found the two offsets are where the walk stopped, as
/// Go leaves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementSpan {
    /// Whether the element was found.
    pub found: bool,
    /// The first byte of the span.
    pub start: i64,
    /// The byte after the span.
    pub end: i64,
}

/// Why [`find_element`] or [`drop_element`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementError {
    /// The error Go returns: the decoder's message (`EOF` and `unexpected EOF` among them), or
    /// `serde_json`'s where it reads a value Go's decoder rejects with another.
    Decode(String),
    /// Where Go panics (an index or slice out of range): the runtime error's text, such as
    /// `runtime error: index out of range [7] with length 7`.
    Panic(String),
}

impl Display for ElementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(text) | Self::Panic(text) => f.write_str(text),
        }
    }
}

impl Error for ElementError {}

/// Go: `jsonutil.FindElement(extension, elementNames...)`: the span of the first member named.
///
/// The walk goes token by token for the first key (or string value) equal to `names[0]`, and the
/// span covers that member with one neighbouring comma, so cutting it leaves the rest of the
/// document; with more names, the found value is searched for the next one. Keys in the path may
/// skip levels.
///
/// Lone surrogate escapes are read as U+FFFD ([`replace_lone_surrogates`]); the offsets are
/// offsets into `extension`.
///
/// # Errors
///
/// [`ElementError::Decode`] where Go returns an error, and [`ElementError::Panic`] where it
/// panics, both with Go's text (`serde_json`'s where it reads a malformed value).
pub fn find_element(extension: &[u8], names: &[&str]) -> Result<ElementSpan, ElementError> {
    let text = replace_lone_surrogates(extension);
    find(&text, names)
}

/// Go: `jsonutil.DropElement(extension, elementNames...)`: the span [`find_element`] finds cut out.
///
/// `extension` comes back unchanged when nothing is found. Lists are not searched for elements to
/// drop, and only the first match goes.
///
/// # Errors
///
/// As [`find_element`].
pub fn drop_element(extension: &[u8], names: &[&str]) -> Result<Vec<u8>, ElementError> {
    let span = find_element(extension, names)?;
    if !span.found {
        return Ok(extension.to_vec());
    }
    let head = extension
        .get(..index(span.start))
        .ok_or_else(|| slice_panic(span.start, span.end))?;
    let tail = extension
        .get(index(span.end)..)
        .ok_or_else(|| slice_panic(span.start, span.end))?;
    Ok([head, tail].concat())
}

/// `FindElement` on `text` (the bytes with lone surrogates rewritten, structurally identical to
/// the caller's).
fn find(text: &[u8], names: &[&str]) -> Result<ElementSpan, ElementError> {
    let Some((&name, rest)) = names.split_first() else {
        return Err(ElementError::Panic(
            "runtime error: index out of range [0] with length 0".to_owned(),
        ));
    };
    let mut walker = Walker::new(text);
    let mut found = false;
    let mut start: i64 = 0;
    let mut end: i64 = 0;
    while let Some(token) = walker.token()? {
        if token.is(name) {
            walker.decode()?;
            end = walker.offset();
            if walker.more() {
                // If there were other members before, the comma before this one goes.
                if byte_at(text, start)? == b',' {
                    start += 1;
                }
                // The member has others after it: the comma after it goes.
                loop {
                    let byte = byte_at(text, end)?;
                    end += 1;
                    if byte == b',' {
                        break;
                    }
                }
            }
            found = true;
            break;
        }
        start = walker.offset();
    }
    if !found || rest.is_empty() {
        return Ok(ElementSpan { found, start, end });
    }
    // The start of the nested value.
    loop {
        let byte = byte_at(text, start)?;
        start += 1;
        if byte == b':' {
            break;
        }
    }
    // The end of the nested value, before the comma after it if the member had others after it.
    loop {
        if end == length(text) {
            end -= 1;
        }
        let byte = byte_at(text, end)?;
        if byte == b']' || byte == b'}' {
            break;
        }
        end -= 1;
        if byte == b',' {
            break;
        }
    }
    let inner = text
        .get(index(start)..index(end))
        .ok_or_else(|| slice_panic(start, end))?;
    let span = find(inner, rest)?;
    Ok(ElementSpan {
        found: span.found,
        start: start + span.start,
        end: start + span.end,
    })
}

/// `len(text)` as Go's `int64`.
fn length(text: &[u8]) -> i64 {
    i64::try_from(text.len()).unwrap_or(i64::MAX)
}

/// An `int64` offset as an index; a negative one is past every slice.
fn index(offset: i64) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// Go's `text[at]`, with its panic text when `at` is out of range.
fn byte_at(text: &[u8], at: i64) -> Result<u8, ElementError> {
    if at < 0 {
        return Err(ElementError::Panic(format!(
            "runtime error: index out of range [{at}]"
        )));
    }
    text.get(index(at)).copied().ok_or_else(|| {
        ElementError::Panic(format!(
            "runtime error: index out of range [{at}] with length {}",
            text.len()
        ))
    })
}

/// Go's panic text for `text[start:end]` out of range.
fn slice_panic(start: i64, end: i64) -> ElementError {
    ElementError::Panic(if end < 0 {
        format!("runtime error: slice bounds out of range [:{end}]")
    } else {
        format!("runtime error: slice bounds out of range [{start}:{end}]")
    })
}

/// Go: the decoder's `tokenState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
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

/// A token Go's `Decoder.Token` returns, as far as `FindElement` looks at it.
enum Token {
    /// A delimiter: `[`, `]`, `{` or `}`.
    Delim,
    /// An object key.
    Key(String),
    /// A string, number, boolean or null.
    Scalar(Value),
}

impl Token {
    /// Go: `token == elementName`, which holds for a key or a string value.
    fn is(&self, name: &str) -> bool {
        match self {
            Self::Key(text) | Self::Scalar(Value::String(text)) => text == name,
            Self::Delim | Self::Scalar(_) => false,
        }
    }
}

/// Go's `json.Decoder` over the whole input, as `FindElement` drives it.
struct Walker<'a> {
    text: &'a [u8],
    /// Go: `scanp`, which `InputOffset` reports.
    pos: usize,
    state: State,
    stack: Vec<State>,
}

impl<'a> Walker<'a> {
    const fn new(text: &'a [u8]) -> Self {
        Self {
            text,
            pos: 0,
            state: State::TopValue,
            stack: Vec::new(),
        }
    }

    /// Go: `InputOffset`.
    fn offset(&self) -> i64 {
        i64::try_from(self.pos).unwrap_or(i64::MAX)
    }

    /// Go: `peek`: the next byte that is not a space, which it moves to; at the end of the input
    /// it stays where it was.
    fn peek(&mut self) -> Option<u8> {
        let rest = self.text.get(self.pos..)?;
        let skipped = rest.iter().position(|&b| !is_space(b))?;
        self.pos += skipped;
        rest.get(skipped).copied()
    }

    /// Go: `tokenValueAllowed`.
    const fn value_allowed(&self) -> bool {
        matches!(
            self.state,
            State::TopValue | State::ArrayStart | State::ArrayValue | State::ObjectValue
        )
    }

    /// Go: `tokenValueEnd`.
    const fn value_end(&mut self) {
        self.state = match self.state {
            State::ArrayStart | State::ArrayValue => State::ArrayComma,
            State::ObjectValue => State::ObjectComma,
            other @ (State::TopValue
            | State::ArrayComma
            | State::ObjectStart
            | State::ObjectKey
            | State::ObjectColon
            | State::ObjectComma) => other,
        };
    }

    /// Go: `tokenError`.
    fn token_error(&self, byte: u8) -> ElementError {
        let context = match self.state {
            State::TopValue | State::ArrayStart | State::ArrayValue | State::ObjectValue => {
                " looking for beginning of value"
            }
            State::ArrayComma => " after array element",
            State::ObjectKey => " looking for beginning of object key string",
            State::ObjectColon => " after object key",
            State::ObjectComma => " after object key:value pair",
            State::ObjectStart => "",
        };
        ElementError::Decode(format!("invalid character {}{context}", quote_char(byte)))
    }

    /// Go: `Token`; `None` is `io.EOF`, which `FindElement` takes as the end of a successful walk.
    fn token(&mut self) -> Result<Option<Token>, ElementError> {
        loop {
            let Some(byte) = self.peek() else {
                return Ok(None);
            };
            match byte {
                b'[' | b'{' => {
                    if !self.value_allowed() {
                        return Err(self.token_error(byte));
                    }
                    self.pos += 1;
                    self.stack.push(self.state);
                    self.state = if byte == b'[' {
                        State::ArrayStart
                    } else {
                        State::ObjectStart
                    };
                    return Ok(Some(Token::Delim));
                }
                b']' | b'}' => {
                    let open = if byte == b']' {
                        [State::ArrayStart, State::ArrayComma]
                    } else {
                        [State::ObjectStart, State::ObjectComma]
                    };
                    if !open.contains(&self.state) {
                        return Err(self.token_error(byte));
                    }
                    self.pos += 1;
                    self.state = self.stack.pop().unwrap_or(State::TopValue);
                    self.value_end();
                    return Ok(Some(Token::Delim));
                }
                b':' => {
                    if self.state != State::ObjectColon {
                        return Err(self.token_error(byte));
                    }
                    self.pos += 1;
                    self.state = State::ObjectValue;
                }
                b',' => {
                    if self.state == State::ArrayComma {
                        self.state = State::ArrayValue;
                    } else if self.state == State::ObjectComma {
                        self.state = State::ObjectKey;
                    } else {
                        return Err(self.token_error(byte));
                    }
                    self.pos += 1;
                }
                b'"' if matches!(self.state, State::ObjectStart | State::ObjectKey) => {
                    let key = self.read::<String>()?;
                    self.state = State::ObjectColon;
                    return Ok(Some(Token::Key(key)));
                }
                _ => {
                    if !self.value_allowed() {
                        return Err(self.token_error(byte));
                    }
                    return self.read_value().map(|value| Some(Token::Scalar(value)));
                }
            }
        }
    }

    /// Go: `Decode(&i)` as `FindElement` calls it on the matched element's value.
    fn decode(&mut self) -> Result<(), ElementError> {
        // Go: tokenPrepareForDecode.
        if self.state == State::ArrayComma {
            if self.peek().ok_or_else(eof)? != b',' {
                return Err(ElementError::Decode(
                    "expected comma after array element".to_owned(),
                ));
            }
            self.pos += 1;
            self.state = State::ArrayValue;
        } else if self.state == State::ObjectColon {
            if self.peek().ok_or_else(eof)? != b':' {
                return Err(ElementError::Decode(
                    "expected colon after object key".to_owned(),
                ));
            }
            self.pos += 1;
            self.state = State::ObjectValue;
        }
        if !self.value_allowed() {
            return Err(ElementError::Decode("not at beginning of value".to_owned()));
        }
        let _value = self.read_value()?;
        Ok(())
    }

    /// Go: `More`.
    fn more(&mut self) -> bool {
        self.peek().is_some_and(|b| b != b']' && b != b'}')
    }

    /// Go: `Decode` of one value into an `interface{}`: `readValue`, then the value's end.
    fn read_value(&mut self) -> Result<Value, ElementError> {
        let first = self
            .text
            .get(self.pos..)
            .and_then(|rest| rest.iter().find(|&&b| !is_space(b)))
            .copied()
            .ok_or_else(eof)?;
        if !matches!(
            first,
            b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n' | b'[' | b'{'
        ) {
            return Err(ElementError::Decode(format!(
                "invalid character {} looking for beginning of value",
                quote_char(first)
            )));
        }
        let value = self.read::<Value>()?;
        self.value_end();
        Ok(value)
    }

    /// Reads one value at `pos` with `serde_json` and moves past it.
    fn read<T: DeserializeOwned>(&mut self) -> Result<T, ElementError> {
        let rest = self.text.get(self.pos..).unwrap_or_default();
        let mut stream = Deserializer::from_slice(rest).into_iter::<T>();
        match stream.next() {
            Some(Ok(value)) => {
                self.pos += stream.byte_offset();
                Ok(value)
            }
            Some(Err(error)) if error.classify() == Category::Eof => {
                Err(ElementError::Decode("unexpected EOF".to_owned()))
            }
            Some(Err(error)) => Err(ElementError::Decode(error.to_string())),
            None => Err(eof()),
        }
    }
}

/// Go: `io.EOF`'s text.
fn eof() -> ElementError {
    ElementError::Decode("EOF".to_owned())
}

/// Go: `isSpace` in `encoding/json`.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Go: `quoteChar` in `encoding/json`: the byte in single quotes, as `strconv.Quote` writes the
/// code point of that value.
fn quote_char(byte: u8) -> String {
    match byte {
        b'\'' => r"'\''".to_owned(),
        b'"' => r#"'"'"#.to_owned(),
        b'\\' => r"'\\'".to_owned(),
        0x07 => r"'\a'".to_owned(),
        0x08 => r"'\b'".to_owned(),
        0x0C => r"'\f'".to_owned(),
        b'\n' => r"'\n'".to_owned(),
        b'\r' => r"'\r'".to_owned(),
        b'\t' => r"'\t'".to_owned(),
        0x0B => r"'\v'".to_owned(),
        0x20..=0x7E | 0xA1..=0xAC | 0xAE..=0xFF => format!("'{}'", char::from(byte)),
        0x00..=0x1F | 0x7F => format!("'\\x{byte:02x}'"),
        0x80..=0xA0 | 0xAD => format!("'\\u{:04x}'", u32::from(byte)),
    }
}
