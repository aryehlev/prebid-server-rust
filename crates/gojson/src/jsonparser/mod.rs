//! `github.com/buger/jsonparser` v1.1.2, the byte-scanning JSON accessor.
//!
//! Prebid Server uses it on request and response bytes (`imp.ext`, stored requests, cache
//! responses, adapter bodies). jsonparser does not validate: it scans for quotes and brackets,
//! so it accepts malformed documents, returns raw (still escaped) string contents, and can find
//! keys that a real parser would not. The port keeps every such behaviour, including the
//! offsets it returns and the bytes [`set`] and [`delete`] produce (whitespace is kept as it is
//! around the edit).
//!
//! | Go | Rust |
//! |---|---|
//! | `Get(data, keys...)` | [`get`] (a [`Got`] with all four results) |
//! | `GetString`, `GetInt`, `GetBoolean` | [`get_string`], [`get_int`], [`get_boolean`] |
//! | `ParseString`, `ParseInt` | [`parse_string`], [`parse_int`] |
//! | `ParseBoolean`, `Unescape` | [`parse_boolean`], [`unescape`] |
//! | `Set(data, value, keys...)` | [`set`] |
//! | `Delete(data, keys...)` | [`delete`] |
//! | `ArrayEach(data, cb, keys...)` | [`array_each`] |
//! | `ObjectEach(data, cb, keys...)` | [`object_each`] |
//! | `EachKey(data, cb, paths...)` | [`each_key`] |
//! | `KeyPathNotFoundError`, ... | [`ParserError`] |
//! | `ValueType`, `NotExist`, ... | [`ValueType`] |
//!
//! `err == jsonparser.KeyPathNotFoundError` is `err == Some(ParserError::KeyPathNotFound)`.
//!
//! Keys are byte strings (`&str`, `String` and `&[u8]` all work, and [`NO_KEYS`] is the empty
//! path); an array element is addressed as `"[0]"`.
//!
//! ```
//! use pbs_gojson::jsonparser::{self, ParserError, ValueType};
//!
//! let ext = br#"{"bidder": {"placementId": "a\"b", "sizes": [[300, 250]]}, "prebid": {}}"#;
//! let got = jsonparser::get(ext, &["bidder", "placementId"]);
//! // The string's raw contents: escapes are kept.
//! assert_eq!(
//!     (got.value, got.data_type, got.err),
//!     (&br#"a\"b"#[..], ValueType::String, None)
//! );
//! assert_eq!(
//!     jsonparser::get_string(ext, &["bidder", "placementId"]).map(|s| s.into_bytes()),
//!     Ok(br#"a"b"#.to_vec())
//! );
//! assert_eq!(
//!     jsonparser::get(ext, &["bidder", "sizes", "[0]", "[1]"]).value,
//!     b"250"
//! );
//! assert_eq!(
//!     jsonparser::get(ext, &["missing"]).err,
//!     Some(ParserError::KeyPathNotFound)
//! );
//! assert_eq!(
//!     jsonparser::get_int(ext, &["bidder", "placementId"]).map_err(|e| e.to_string()),
//!     Err(r#"Value is not a number: a\"b"#.to_owned()),
//! );
//! assert_eq!(
//!     &*jsonparser::delete(ext, &["prebid"]),
//!     br#"{"bidder": {"placementId": "a\"b", "sizes": [[300, 250]]}}"#
//! );
//! ```
//!
//! Where Go's jsonparser panics (an index out of range on malformed input, or an empty key),
//! the port returns the function's not-found result instead: [`get`] and the iterators stop or
//! report [`ParserError::KeyPathNotFound`], [`delete`] returns the input unchanged, an empty key
//! never addresses an array element, and [`set`] writes it as an object key. Go's `Set` can
//! also write into the spare capacity of its input slice; the Rust input is never modified.

mod escape;

use core::{error, fmt};
use std::borrow::Cow;

pub use escape::unescape;
use escape::{InStack, IntError, STACK_BUF_SIZE, parse_int as raw_parse_int, unescape_in_stack};

use crate::gostring::GoString;

/// The empty key path (Go's call without keys).
pub const NO_KEYS: &[&[u8]] = &[];

/// jsonparser's `ValueType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// `jsonparser.NotExist`
    NotExist,
    /// `jsonparser.String`
    String,
    /// `jsonparser.Number`
    Number,
    /// `jsonparser.Object`
    Object,
    /// `jsonparser.Array`
    Array,
    /// `jsonparser.Boolean`
    Boolean,
    /// `jsonparser.Null`
    Null,
    /// `jsonparser.Unknown`
    Unknown,
}

impl ValueType {
    /// `ValueType.String()`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotExist => "non-existent",
            Self::String => "string",
            Self::Number => "number",
            Self::Object => "object",
            Self::Array => "array",
            Self::Boolean => "boolean",
            Self::Null => "null",
            Self::Unknown => "unknown",
        }
    }

    /// The Go constant's integer value (what `%d` prints).
    #[must_use]
    pub const fn go_value(self) -> u8 {
        match self {
            Self::NotExist => 0,
            Self::String => 1,
            Self::Number => 2,
            Self::Object => 3,
            Self::Array => 4,
            Self::Boolean => 5,
            Self::Null => 6,
            Self::Unknown => 7,
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// jsonparser's errors: its sentinel values and the `fmt.Errorf` type errors of the `Get*`
/// helpers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ParserError {
    /// `KeyPathNotFoundError`: `Key path not found`.
    KeyPathNotFound,
    /// `UnknownValueTypeError`: `Unknown value type`.
    UnknownValueType,
    /// `MalformedJsonError`: `Malformed JSON error`.
    MalformedJson,
    /// `MalformedStringError`.
    MalformedString,
    /// `MalformedArrayError`.
    MalformedArray,
    /// `MalformedObjectError`.
    MalformedObject,
    /// `MalformedValueError`.
    MalformedValue,
    /// `OverflowIntegerError`.
    OverflowInteger,
    /// `MalformedStringEscapeError`.
    MalformedStringEscape,
    /// `NullValueError`: `Value is null`.
    NullValue,
    /// `Value is not a string: <raw value>` ([`get_string`]).
    NotAString(Vec<u8>),
    /// `Value is not a number: <raw value>` ([`get_int`]).
    NotANumber(Vec<u8>),
    /// `Value is not a boolean: <raw value>` ([`get_boolean`]).
    NotABoolean(Vec<u8>),
}

impl ParserError {
    /// The exact bytes of Go's `err.ParserError()` (the type errors quote raw input bytes).
    #[must_use]
    pub fn go_text(&self) -> Vec<u8> {
        let (prefix, value): (&str, &[u8]) = match self {
            Self::KeyPathNotFound => ("Key path not found", b""),
            Self::UnknownValueType => ("Unknown value type", b""),
            Self::MalformedJson => ("Malformed JSON error", b""),
            Self::MalformedString => ("Value is string, but can't find closing '\"' symbol", b""),
            Self::MalformedArray => ("Value is array, but can't find closing ']' symbol", b""),
            Self::MalformedObject => (
                "Value looks like object, but can't find closing '}' symbol",
                b"",
            ),
            Self::MalformedValue => (
                "Value looks like Number/Boolean/None, but can't find its end: ',' or '}' symbol",
                b"",
            ),
            Self::OverflowInteger => ("Value is number, but overflowed while parsing", b""),
            Self::MalformedStringEscape => {
                ("Encountered an invalid escape sequence in a string", b"")
            }
            Self::NullValue => ("Value is null", b""),
            Self::NotAString(v) => ("Value is not a string: ", v),
            Self::NotANumber(v) => ("Value is not a number: ", v),
            Self::NotABoolean(v) => ("Value is not a boolean: ", v),
        };
        let mut out = Vec::with_capacity(prefix.len() + value.len());
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(value);
        out
    }
}

impl fmt::Display for ParserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.go_text()))
    }
}

impl error::Error for ParserError {}

/// The four results of [`get`] (Go's `value, dataType, offset, err`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Got<'a> {
    /// The value's bytes: a string's raw contents without the quotes; empty on error.
    pub value: &'a [u8],
    /// The value's type ([`ValueType::NotExist`] when the path is not found).
    pub data_type: ValueType,
    /// The offset in `data` just past the value (`-1` when not found).
    pub offset: isize,
    /// The error, if any.
    pub err: Option<ParserError>,
}

/// `internalGet`'s results.
struct Lookup<'a> {
    value: &'a [u8],
    data_type: ValueType,
    /// The value's start (`-1` is `None`).
    offset: Option<usize>,
    /// The value's end (`-1` is `None`).
    end: Option<usize>,
    err: Option<ParserError>,
}

impl Lookup<'_> {
    const fn not_found() -> Self {
        Self {
            value: &[],
            data_type: ValueType::NotExist,
            offset: None,
            end: None,
            err: Some(ParserError::KeyPathNotFound),
        }
    }
}

/// A Go `int` from an offset (`-1` for `None`).
fn go_int(offset: Option<usize>) -> isize {
    offset.map_or(-1, |o| isize::try_from(o).unwrap_or(isize::MAX))
}

const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t')
}

/// `data[from:]`, empty past the end.
fn tail(data: &[u8], from: usize) -> &[u8] {
    data.get(from..).unwrap_or_default()
}

/// `tokenEnd`.
fn token_end(data: &[u8]) -> usize {
    data.iter()
        .position(|&c| is_space(c) || matches!(c, b',' | b'}' | b']'))
        .unwrap_or(data.len())
}

/// `findTokenStart`: the last `token` before an opening bracket, else 0.
fn find_token_start(data: &[u8], token: u8) -> usize {
    for (i, &c) in data.iter().enumerate().rev() {
        if c == token {
            return i;
        }
        if matches!(c, b'[' | b'{') {
            return 0;
        }
    }
    0
}

/// `nextToken`: the first non-whitespace byte (`-1` is `None`).
fn next_token(data: &[u8]) -> Option<usize> {
    data.iter().position(|&c| !is_space(c))
}

/// `lastToken`: the last non-whitespace byte.
fn last_token(data: &[u8]) -> Option<usize> {
    data.iter().rposition(|&c| !is_space(c))
}

/// `stringEnd`: the index after the closing quote of a string whose opening quote was already
/// consumed, and whether a backslash was seen.
fn string_end(data: &[u8]) -> (Option<usize>, bool) {
    // Go: buger/jsonparser parser.go stringEnd
    let mut escaped = false;
    for (i, &c) in data.iter().enumerate() {
        if c == b'"' {
            if !escaped {
                return (Some(i + 1), false);
            }
            // `j` is one past Go's `j`, so that `j == 0` is Go's `j < 0`.
            let mut j = i;
            loop {
                if j == 0 || data.get(j - 1) != Some(&b'\\') {
                    return (Some(i + 1), true);
                }
                j -= 1;
                if j == 0 || data.get(j - 1) != Some(&b'\\') {
                    break;
                }
                j -= 1;
            }
        } else if c == b'\\' {
            escaped = true;
        }
    }
    (None, escaped)
}

/// `blockEnd`: the index after the bracket closing the block at the start of `data`.
fn block_end(data: &[u8], open: u8, close: u8) -> Option<usize> {
    // Go: buger/jsonparser parser.go blockEnd
    let mut level: i64 = 0;
    let mut i = 0;
    while let Some(&c) = data.get(i) {
        if c == b'"' {
            i += string_end(tail(data, i + 1)).0?;
        } else if c == open {
            level += 1;
        } else if c == close {
            level -= 1;
            if level == 0 {
                return Some(i + 1);
            }
        }
        i += 1;
    }
    None
}

/// Go's `strconv.Atoi`: the value and whether it parsed. A syntax error gives 0 and a range
/// error the clamped value; like `ParseUint`, the first overflow wins over a later bad digit.
fn atoi(s: &[u8]) -> (i64, bool) {
    let (neg, digits) = match s.first() {
        Some(b'-') => (true, tail(s, 1)),
        Some(b'+') => (false, tail(s, 1)),
        Some(_) | None => (false, s),
    };
    if digits.is_empty() {
        return (0, false);
    }
    let mut n: u64 = 0;
    for &d in digits {
        if !d.is_ascii_digit() {
            return (0, false);
        }
        match n
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(d - b'0')))
        {
            Some(v) => n = v,
            None => return (if neg { i64::MIN } else { i64::MAX }, false),
        }
    }
    if neg {
        0_i64
            .checked_sub_unsigned(n)
            .map_or((i64::MIN, false), |v| (v, true))
    } else {
        i64::try_from(n).map_or((i64::MAX, false), |v| (v, true))
    }
}

fn key_at<K: AsRef<[u8]>>(keys: &[K], index: usize) -> Option<&[u8]> {
    keys.get(index).map(AsRef::as_ref)
}

/// The `[n]` index of a key path element, without its brackets (`p[1:len(p)-1]`).
fn index_digits(step: &[u8]) -> &[u8] {
    step.get(1..step.len().saturating_sub(1))
        .unwrap_or_default()
}

/// The key `findKeyStart` compares against.
enum SearchKey<'a> {
    /// A string of its own.
    Fixed(Cow<'a, [u8]>),
    /// The first `n` bytes of the shared stack buffer, which later unescapes overwrite.
    Stack(usize),
}

/// `findKeyStart`: the offset of the opening quote of `key` among the top-level keys of the
/// object (or array) at the start of `data`.
///
/// Go unescapes `key` into a 64-byte stack buffer and every escaped key of `data` into the same
/// buffer, and an unescaped `key` that lives there is a view of it, so it changes as keys are
/// read: once a short escaped key was unescaped, the next escaped key of the same unescaped
/// length matches. Only a `key` with a backslash is affected.
fn find_key_start(data: &[u8], key: &[u8]) -> Option<usize> {
    // Go: buger/jsonparser parser.go findKeyStart
    let mut i = next_token(data)?;
    let ln = data.len();
    if ln > 0 && matches!(data.get(i), Some(b'{' | b'[')) {
        i += 1;
    }
    let mut stack = [0_u8; STACK_BUF_SIZE];
    let key = match unescape_in_stack(key, &mut stack) {
        Ok(InStack::Stack(len)) => SearchKey::Stack(len),
        Ok(InStack::Heap(owned)) => SearchKey::Fixed(Cow::Owned(owned)),
        Ok(InStack::Input(_)) | Err(_) => SearchKey::Fixed(Cow::Borrowed(key)),
    };
    while let Some(&c) = data.get(i) {
        match c {
            b'"' => 'string: {
                i += 1;
                let key_begin = i;
                let (str_end, key_escaped) = string_end(tail(data, i));
                let Some(str_end) = str_end else {
                    break 'string;
                };
                i += str_end;
                let key_end = i - 1;
                let Some(value_offset) = next_token(tail(data, i)) else {
                    break 'string;
                };
                i += value_offset;
                let raw = data.get(key_begin..key_end).unwrap_or_default();
                let heap: Vec<u8>;
                let k: &[u8] = if key_escaped {
                    match unescape_in_stack(raw, &mut stack) {
                        Ok(InStack::Stack(len)) => stack.get(..len).unwrap_or_default(),
                        Ok(InStack::Heap(owned)) => {
                            heap = owned;
                            &heap
                        }
                        Ok(InStack::Input(raw)) => raw,
                        Err(_) => break 'string,
                    }
                } else {
                    raw
                };
                let key_now: &[u8] = match &key {
                    SearchKey::Fixed(fixed) => fixed,
                    SearchKey::Stack(len) => stack.get(..*len).unwrap_or_default(),
                };
                if data.get(i) == Some(&b':') && k == key_now {
                    return Some(key_begin - 1);
                }
            }
            b'[' => {
                if let Some(end) = block_end(tail(data, i), b'[', b']') {
                    i += end;
                }
            }
            b'{' => {
                if let Some(end) = block_end(tail(data, i), b'{', b'}') {
                    i += end;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `searchKeys`: the offset just past the colon of the last key of the path (or the start of
/// the array element it ends with); 0 for an empty path.
fn search_keys<K: AsRef<[u8]>>(data: &[u8], keys: &[K]) -> Option<usize> {
    // Go: buger/jsonparser parser.go searchKeys
    let mut key_level: i64 = 0;
    let mut level: i64 = 0;
    let mut i = 0;
    let lk = i64::try_from(keys.len()).unwrap_or(i64::MAX);
    let mut last_matched = true;
    if keys.is_empty() {
        return Some(0);
    }
    while let Some(&c) = data.get(i) {
        match c {
            b'"' => {
                i += 1;
                let key_begin = i;
                let (str_end, key_escaped) = string_end(tail(data, i));
                i += str_end?;
                let key_end = i - 1;
                i += next_token(tail(data, i))?;
                if data.get(i) == Some(&b':') {
                    if level < 1 {
                        return None;
                    }
                    let raw = data.get(key_begin..key_end).unwrap_or_default();
                    let key = if key_escaped {
                        unescape(raw).ok()?
                    } else {
                        Cow::Borrowed(raw)
                    };
                    if level > lk {
                        return None;
                    }
                    let index = usize::try_from(level - 1).ok()?;
                    if key_at(keys, index) == Some(&*key) {
                        last_matched = true;
                        if key_level == level - 1 {
                            key_level += 1;
                            if key_level == lk {
                                return Some(i + 1);
                            }
                        }
                    } else {
                        last_matched = false;
                    }
                } else {
                    i -= 1;
                }
            }
            b'{' => {
                if last_matched {
                    level += 1;
                } else {
                    let end = block_end(tail(data, i), b'{', b'}')?;
                    i += end - 1;
                }
            }
            b'}' => {
                level -= 1;
                if level == key_level {
                    key_level -= 1;
                }
            }
            b'[' => {
                // Go indexes keys[level] here (level is never negative when it equals
                // keyLevel) and panics on an empty key.
                let wanted = if key_level == level {
                    usize::try_from(level)
                        .ok()
                        .filter(|&l| key_at(keys, l).is_some_and(|k| k.first() == Some(&b'[')))
                } else {
                    None
                };
                if let Some(index) = wanted {
                    return search_array_index(data, i, keys, index);
                }
                let skip = block_end(tail(data, i), b'[', b']')?;
                i += skip - 1;
            }
            b':' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The array-index step of `searchKeys`: `keys[level]` is `[n]` and `data[i]` opens the array.
fn search_array_index<K: AsRef<[u8]>>(
    data: &[u8],
    i: usize,
    keys: &[K],
    level: usize,
) -> Option<usize> {
    let key = key_at(keys, level)?;
    if key.len() < 3 || key.last() != Some(&b']') {
        return None;
    }
    let (a_idx, ok) = atoi(index_digits(key));
    if !ok {
        return None;
    }
    let array = tail(data, i);
    let mut cur_idx: i64 = 0;
    let mut found: Option<(&[u8], usize)> = None;
    let _scan = array_each_inner(array, NO_KEYS, |value, data_type, offset| {
        if cur_idx == a_idx {
            found = if data_type == ValueType::String {
                // A string element is re-sliced with its quotes.
                let start = offset.saturating_sub(2);
                array
                    .get(start..start + value.len() + 2)
                    .map(|v| (v, start))
            } else {
                Some((value, offset))
            };
        }
        cur_idx += 1;
    });
    let (value, value_offset) = found?;
    let sub = search_keys(value, keys.get(level + 1..).unwrap_or_default())?;
    Some(i + value_offset + sub)
}

/// `getType`: the value at `offset` (strings with their quotes) and its type and end; on error
/// the type Go reports with it.
fn get_type(
    data: &[u8],
    offset: usize,
) -> Result<(&[u8], ValueType, usize), (ValueType, ParserError)> {
    // Go: buger/jsonparser parser.go getType
    let Some(&first) = data.get(offset) else {
        // Go indexes out of range here; callers always pass a valid offset.
        return Err((ValueType::NotExist, ParserError::MalformedJson));
    };
    let (data_type, end) = match first {
        b'"' => match string_end(tail(data, offset + 1)).0 {
            Some(idx) => (ValueType::String, offset + idx + 1),
            None => return Err((ValueType::String, ParserError::MalformedString)),
        },
        b'[' => match block_end(tail(data, offset), b'[', b']') {
            Some(end) => (ValueType::Array, offset + end),
            None => return Err((ValueType::Array, ParserError::MalformedArray)),
        },
        b'{' => match block_end(tail(data, offset), b'{', b'}') {
            Some(end) => (ValueType::Object, offset + end),
            None => return Err((ValueType::Object, ParserError::MalformedObject)),
        },
        c => {
            let end = offset + token_end(tail(data, offset));
            let value = data.get(offset..end).unwrap_or_default();
            let data_type = match c {
                b't' | b'f' if value == b"true" || value == b"false" => ValueType::Boolean,
                b'u' | b'n' if value == b"null" => ValueType::Null,
                b'0'..=b'9' | b'-' => ValueType::Number,
                _ => return Err((ValueType::Unknown, ParserError::UnknownValueType)),
            };
            (data_type, end)
        }
    };
    Ok((data.get(offset..end).unwrap_or_default(), data_type, end))
}

/// `internalGet`.
fn internal_get<'a, K: AsRef<[u8]>>(data: &'a [u8], keys: &[K]) -> Lookup<'a> {
    // Go: buger/jsonparser parser.go internalGet
    let found = if keys.is_empty() {
        Some(0)
    } else {
        search_keys(data, keys)
    };
    let Some(mut offset) = found else {
        return Lookup::not_found();
    };
    let Some(n_o) = next_token(tail(data, offset)) else {
        return Lookup {
            value: &[],
            data_type: ValueType::NotExist,
            offset: Some(offset),
            end: None,
            err: Some(ParserError::MalformedJson),
        };
    };
    offset += n_o;
    match get_type(data, offset) {
        Err((data_type, err)) => Lookup {
            value: &[],
            data_type,
            offset: Some(offset),
            end: Some(offset),
            err: Some(err),
        },
        Ok((value, data_type, end)) => {
            let value = if data_type == ValueType::String {
                value
                    .get(1..value.len().saturating_sub(1))
                    .unwrap_or_default()
            } else {
                value
            };
            Lookup {
                value,
                data_type,
                offset: Some(offset),
                end: Some(end),
                err: None,
            }
        }
    }
}

/// `jsonparser.Get(data, keys...)`: the value at the key path.
///
/// With no keys it reads the first value in `data`. A string's value is its raw contents
/// (escapes kept, quotes dropped); `offset` is just past the value. A path that is not found
/// gives [`ValueType::NotExist`], offset `-1` and [`ParserError::KeyPathNotFound`].
#[must_use]
pub fn get<'a, K: AsRef<[u8]>>(data: &'a [u8], keys: &[K]) -> Got<'a> {
    let found = internal_get(data, keys);
    Got {
        value: found.value,
        data_type: found.data_type,
        offset: go_int(found.end),
        err: found.err,
    }
}

/// `jsonparser.GetString`: the string at the path, unescaped.
///
/// # Errors
///
/// [`get`]'s error, [`ParserError::NullValue`] for `null`, [`ParserError::NotAString`] for any
/// other type, and [`ParserError::MalformedValue`] for a bad escape.
pub fn get_string<K: AsRef<[u8]>>(data: &[u8], keys: &[K]) -> Result<GoString, ParserError> {
    let got = internal_get(data, keys);
    if let Some(err) = got.err {
        return Err(err);
    }
    match got.data_type {
        ValueType::String => {}
        ValueType::Null => return Err(ParserError::NullValue),
        ValueType::NotExist
        | ValueType::Number
        | ValueType::Object
        | ValueType::Array
        | ValueType::Boolean
        | ValueType::Unknown => return Err(ParserError::NotAString(got.value.to_vec())),
    }
    if !got.value.contains(&b'\\') {
        return Ok(GoString::from_bytes(got.value.to_vec()));
    }
    parse_string(got.value)
}

/// `jsonparser.GetInt`.
///
/// # Errors
///
/// [`get`]'s error, [`ParserError::NullValue`], [`ParserError::NotANumber`], or [`parse_int`]'s
/// error.
pub fn get_int<K: AsRef<[u8]>>(data: &[u8], keys: &[K]) -> Result<i64, ParserError> {
    let got = internal_get(data, keys);
    if let Some(err) = got.err {
        return Err(err);
    }
    match got.data_type {
        ValueType::Number => parse_int(got.value),
        ValueType::Null => Err(ParserError::NullValue),
        ValueType::NotExist
        | ValueType::String
        | ValueType::Object
        | ValueType::Array
        | ValueType::Boolean
        | ValueType::Unknown => Err(ParserError::NotANumber(got.value.to_vec())),
    }
}

/// `jsonparser.GetBoolean`.
///
/// # Errors
///
/// [`get`]'s error, [`ParserError::NullValue`] or [`ParserError::NotABoolean`].
pub fn get_boolean<K: AsRef<[u8]>>(data: &[u8], keys: &[K]) -> Result<bool, ParserError> {
    let got = internal_get(data, keys);
    if let Some(err) = got.err {
        return Err(err);
    }
    match got.data_type {
        ValueType::Boolean => parse_boolean(got.value),
        ValueType::Null => Err(ParserError::NullValue),
        ValueType::NotExist
        | ValueType::String
        | ValueType::Number
        | ValueType::Object
        | ValueType::Array
        | ValueType::Unknown => Err(ParserError::NotABoolean(got.value.to_vec())),
    }
}

/// `jsonparser.ParseBoolean`.
///
/// # Errors
///
/// [`ParserError::MalformedValue`] unless `b` is `true` or `false`.
pub const fn parse_boolean(b: &[u8]) -> Result<bool, ParserError> {
    match b {
        b"true" => Ok(true),
        b"false" => Ok(false),
        _ => Err(ParserError::MalformedValue),
    }
}

/// `jsonparser.ParseString`: [`unescape`] as a Go string.
///
/// # Errors
///
/// [`ParserError::MalformedValue`] for a bad escape.
pub fn parse_string(b: &[u8]) -> Result<GoString, ParserError> {
    unescape(b)
        .map(|s| GoString::from_bytes(s.into_owned()))
        .map_err(|_bad_escape| ParserError::MalformedValue)
}

/// `jsonparser.ParseInt`: base-10 digits with an optional `-` (a lone `-` parses as 0).
///
/// # Errors
///
/// [`ParserError::OverflowInteger`] outside the `int64` range, [`ParserError::MalformedValue`]
/// otherwise.
pub fn parse_int(b: &[u8]) -> Result<i64, ParserError> {
    raw_parse_int(b).map_err(|e| match e {
        IntError::Malformed => ParserError::MalformedValue,
        IntError::Overflow => ParserError::OverflowInteger,
    })
}

/// `ArrayEach` with unsigned offsets (`-1` is `None`).
fn array_each_inner<'a, K, F>(
    data: &'a [u8],
    keys: &[K],
    mut cb: F,
) -> (Option<usize>, Option<ParserError>)
where
    K: AsRef<[u8]>,
    F: FnMut(&'a [u8], ValueType, usize),
{
    // Go: buger/jsonparser parser.go ArrayEach
    if data.is_empty() {
        return (None, Some(ParserError::MalformedObject));
    }
    let Some(n_t) = next_token(data) else {
        return (None, Some(ParserError::MalformedJson));
    };
    let mut offset = n_t + 1;
    if !keys.is_empty() {
        let Some(found) = search_keys(data, keys) else {
            return (None, Some(ParserError::KeyPathNotFound));
        };
        offset = found;
        let Some(n_o) = next_token(tail(data, offset)) else {
            return (Some(offset), Some(ParserError::MalformedJson));
        };
        offset += n_o;
        if data.get(offset) != Some(&b'[') {
            return (Some(offset), Some(ParserError::MalformedArray));
        }
        offset += 1;
    }
    let Some(n_o) = next_token(tail(data, offset)) else {
        return (Some(offset), Some(ParserError::MalformedJson));
    };
    offset += n_o;
    if data.get(offset) == Some(&b']') {
        return (Some(offset), None);
    }
    loop {
        let got = internal_get(tail(data, offset), NO_KEYS);
        if let Some(err) = got.err {
            return (Some(offset), Some(err));
        }
        let Some(o) = got.end.filter(|&o| o != 0) else {
            break;
        };
        if got.data_type != ValueType::NotExist {
            cb(got.value, got.data_type, offset + o - got.value.len());
        }
        offset += o;
        let Some(skip) = next_token(tail(data, offset)) else {
            return (Some(offset), Some(ParserError::MalformedArray));
        };
        offset += skip;
        match data.get(offset) {
            Some(b']') => break,
            Some(b',') => offset += 1,
            Some(_) | None => return (Some(offset), Some(ParserError::MalformedArray)),
        }
    }
    (Some(offset), None)
}

/// `jsonparser.ArrayEach(data, cb, keys...)`: calls `cb(value, type, offset)` for each element.
///
/// The elements are those of the array at the path or, with no keys, of whatever follows the
/// first byte of `data`, which is not checked to be `[`. Go also passes an error to the
/// callback, but it is always nil. `offset` is the element's end minus its value's length, so a
/// string element's offset is two past its opening quote.
///
/// Returns Go's `offset, err`: where scanning stopped (the closing `]` on success, `-1` when
/// nothing was scanned) and the error, if any.
pub fn array_each<'a, K, F>(data: &'a [u8], keys: &[K], mut cb: F) -> (isize, Option<ParserError>)
where
    K: AsRef<[u8]>,
    F: FnMut(&'a [u8], ValueType, isize),
{
    let (offset, err) = array_each_inner(data, keys, |value, data_type, offset| {
        cb(value, data_type, go_int(Some(offset)));
    });
    (go_int(offset), err)
}

/// `jsonparser.ObjectEach(data, cb, keys...)`: calls `cb(key, value, type, offset)` for each
/// member of the object at the path, with the key unescaped and `offset` just past the value.
///
/// # Errors
///
/// The first error `cb` returns, or the parse error converted with `E::from`
/// ([`ParserError::KeyPathNotFound`], [`ParserError::MalformedObject`],
/// [`ParserError::MalformedJson`], [`ParserError::MalformedArray`],
/// [`ParserError::MalformedStringEscape`] or [`get`]'s error).
pub fn object_each<'a, K, E, F>(data: &'a [u8], keys: &[K], mut cb: F) -> Result<(), E>
where
    K: AsRef<[u8]>,
    E: From<ParserError>,
    F: FnMut(&[u8], &'a [u8], ValueType, isize) -> Result<(), E>,
{
    // Go: buger/jsonparser parser.go ObjectEach
    let mut offset = if keys.is_empty() {
        0
    } else {
        search_keys(data, keys).ok_or(ParserError::KeyPathNotFound)?
    };
    offset += next_token(tail(data, offset)).ok_or(ParserError::MalformedObject)?;
    if data.get(offset) != Some(&b'{') {
        return Err(ParserError::MalformedObject.into());
    }
    offset += 1;
    offset += next_token(tail(data, offset)).ok_or(ParserError::MalformedJson)?;
    if data.get(offset) == Some(&b'}') {
        return Ok(());
    }
    while let Some(&c) = data.get(offset) {
        match c {
            b'"' => offset += 1,
            b'}' => return Ok(()),
            _ => return Err(ParserError::MalformedObject.into()),
        }
        let (end, escaped) = string_end(tail(data, offset));
        let end = end.ok_or(ParserError::MalformedJson)?;
        let raw = data.get(offset..offset + end - 1).unwrap_or_default();
        offset += end;
        let key = if escaped {
            unescape(raw).map_err(|_bad_escape| ParserError::MalformedStringEscape)?
        } else {
            Cow::Borrowed(raw)
        };
        offset += next_token(tail(data, offset)).ok_or(ParserError::MalformedJson)?;
        if data.get(offset) != Some(&b':') {
            return Err(ParserError::MalformedJson.into());
        }
        offset += 1;
        let got = internal_get(tail(data, offset), NO_KEYS);
        if let Some(err) = got.err {
            return Err(err.into());
        }
        let value_end = got.end.unwrap_or_default();
        cb(
            &key,
            got.value,
            got.data_type,
            go_int(Some(offset + value_end)),
        )?;
        offset += value_end;
        offset += next_token(tail(data, offset)).ok_or(ParserError::MalformedArray)?;
        match data.get(offset) {
            Some(b'}') => return Ok(()),
            Some(b',') => offset += 1,
            Some(_) | None => return Err(ParserError::MalformedObject.into()),
        }
        offset += next_token(tail(data, offset)).ok_or(ParserError::MalformedArray)?;
    }
    Err(ParserError::MalformedObject.into())
}

/// `sameTree`: whether the shorter of two paths is a prefix of the other.
fn same_tree<K: AsRef<[u8]>>(p1: &[K], p2: &[Cow<'_, [u8]>]) -> bool {
    p1.iter().zip(p2).all(|(a, b)| a.as_ref() == b.as_ref())
}

/// The scan state `EachKey` shares between its key and array steps.
struct EachKeyState<'a> {
    /// `pathFlags`: which paths were found.
    found: Vec<bool>,
    /// `pathsMatched`.
    matched: usize,
    /// `pathsBuf`: the unescaped keys of the current path.
    keys: Vec<Cow<'a, [u8]>>,
    /// `maxPath`: the longest path's length.
    max_level: i64,
}

/// `jsonparser.EachKey(data, cb, paths...)`: finds several paths in one scan.
///
/// Calls `cb(path_index, value, type, err)` for each path found, and `cb(-1, b"", Unknown,
/// Some(MalformedJson))` on some malformed input. Returns where scanning stopped once every
/// path was found, or `-1`.
pub fn each_key<'a, P, K, F>(data: &'a [u8], paths: &[P], mut cb: F) -> isize
where
    P: AsRef<[K]>,
    K: AsRef<[u8]>,
    F: FnMut(isize, &'a [u8], ValueType, Option<ParserError>),
{
    // Go: buger/jsonparser parser.go EachKey
    let mut level: i64 = 0;
    let mut i = 0_usize;
    let max_path = paths.iter().map(|p| p.as_ref().len()).max().unwrap_or(0);
    let mut state = EachKeyState {
        found: vec![false; paths.len()],
        matched: 0,
        keys: vec![Cow::Borrowed(&[]); max_path],
        max_level: i64::try_from(max_path).unwrap_or(i64::MAX),
    };
    while let Some(&c) = data.get(i) {
        match c {
            b'"' => {
                i += 1;
                let key_begin = i;
                let (str_end, key_escaped) = string_end(tail(data, i));
                let Some(str_end) = str_end else {
                    return -1;
                };
                i += str_end;
                let key_end = i - 1;
                let Some(value_offset) = next_token(tail(data, i)) else {
                    return -1;
                };
                i += value_offset;
                if data.get(i) != Some(&b':') {
                    // Go: i-- here and i++ below, so the scan resumes at this byte.
                    continue;
                }
                let raw = data.get(key_begin..key_end).unwrap_or_default();
                let key: Cow<'a, [u8]> = if key_escaped {
                    match unescape(raw) {
                        Ok(k) => Cow::Owned(k.into_owned()),
                        Err(_) => return -1,
                    }
                } else {
                    Cow::Borrowed(raw)
                };
                let matched = match each_key_match(data, i, level, key, paths, &mut state, &mut cb)
                {
                    Ok(matched) => matched,
                    Err(end) => return end,
                };
                if !matched {
                    // Go adds the value's offset relative to i+1 to i, landing just before the
                    // value, so the block skip below never fires on valid input.
                    match next_token(tail(data, i + 1)) {
                        Some(token_offset) => i += token_offset,
                        None => i -= 1,
                    }
                    if data.get(i) == Some(&b'{')
                        && let Some(skip) = block_end(tail(data, i), b'{', b'}')
                    {
                        i += skip + 1;
                    }
                }
                if matches!(data.get(i), Some(b'{' | b'}' | b'[' | b'"')) {
                    i -= 1;
                }
            }
            b'{' => level += 1,
            b'}' | b']' => level -= 1,
            b'[' => {
                let Some(depth) = usize::try_from(level).ok() else {
                    cb(
                        -1,
                        &[],
                        ValueType::Unknown,
                        Some(ParserError::MalformedJson),
                    );
                    return -1;
                };
                match each_key_array(data, i, depth, paths, &mut state, &mut cb) {
                    ArrayStep::Done => return go_int(Some(i)),
                    ArrayStep::Continue(next) => {
                        level += 1;
                        i = next;
                    }
                    ArrayStep::Skip => {
                        let Some(skip) = block_end(tail(data, i), b'[', b']') else {
                            return -1;
                        };
                        i += skip - 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    -1
}

/// The key step of `EachKey`: `key` is followed by the colon at `data[i]`. Returns whether a
/// path matched it, or `EachKey`'s result when the scan ends here.
fn each_key_match<'a, P, K, F>(
    data: &'a [u8],
    i: usize,
    level: i64,
    key: Cow<'a, [u8]>,
    paths: &[P],
    state: &mut EachKeyState<'a>,
    cb: &mut F,
) -> Result<bool, isize>
where
    P: AsRef<[K]>,
    K: AsRef<[u8]>,
    F: FnMut(isize, &'a [u8], ValueType, Option<ParserError>),
{
    if state.max_level < level {
        return Ok(false);
    }
    let Some(depth) = usize::try_from(level).ok().filter(|&d| d >= 1) else {
        cb(
            -1,
            &[],
            ValueType::Unknown,
            Some(ParserError::MalformedJson),
        );
        return Err(-1);
    };
    // depth <= maxPath, so the slot exists.
    if let Some(slot) = state.keys.get_mut(depth - 1) {
        *slot = key;
    }
    let mut matched = false;
    for (pi, p) in paths.iter().enumerate() {
        let p = p.as_ref();
        if p.len() != depth
            || state.found.get(pi).copied().unwrap_or(true)
            || !same_tree(p, state.keys.get(..depth).unwrap_or_default())
        {
            continue;
        }
        matched = true;
        state.matched += 1;
        if let Some(flag) = state.found.get_mut(pi) {
            *flag = true;
        }
        let got = get(tail(data, i + 1), NO_KEYS);
        cb(go_int(Some(pi)), got.value, got.data_type, got.err);
        if state.matched == paths.len() {
            break;
        }
    }
    if state.matched == paths.len() {
        return Err(go_int(Some(i)));
    }
    Ok(matched)
}

/// What the `[` step of `EachKey` does next.
enum ArrayStep {
    /// Every path has been found.
    Done,
    /// Some path wanted an element of the array: continue from this index, one level deeper.
    Continue(usize),
    /// No path wants an element here: skip the whole array.
    Skip,
}

/// The `[` step of `EachKey` at `data[i]`, at nesting `depth`.
fn each_key_array<'a, P, K, F>(
    data: &'a [u8],
    i: usize,
    depth: usize,
    paths: &[P],
    state: &mut EachKeyState<'a>,
    cb: &mut F,
) -> ArrayStep
where
    P: AsRef<[K]>,
    K: AsRef<[u8]>,
    F: FnMut(isize, &'a [u8], ValueType, Option<ParserError>),
{
    let mut wanted: Vec<i64> = Vec::new();
    let mut indexed = vec![false; paths.len()];
    for (pi, p) in paths.iter().enumerate() {
        let p = p.as_ref();
        // Go panics on an empty key here.
        let Some(step) = key_at(p, depth) else {
            continue;
        };
        if state.found.get(pi).copied().unwrap_or(true)
            || step.first() != Some(&b'[')
            || !same_tree(p, state.keys.get(..depth).unwrap_or_default())
        {
            continue;
        }
        if step.len() >= 2 {
            wanted.push(atoi(index_digits(step)).0);
            if let Some(flag) = indexed.get_mut(pi) {
                *flag = true;
            }
        }
    }
    if wanted.is_empty() {
        return ArrayStep::Skip;
    }
    let mut cur_idx: i64 = 0;
    let (arr_off, _err) = array_each_inner(tail(data, i), NO_KEYS, |value, _, _| {
        if wanted.contains(&cur_idx) {
            for (pi, p) in paths.iter().enumerate() {
                let p = p.as_ref();
                if !indexed.get(pi).copied().unwrap_or(false)
                    || atoi(index_digits(key_at(p, depth).unwrap_or_default())).0 != cur_idx
                {
                    continue;
                }
                let of = search_keys(value, p.get(depth + 1..).unwrap_or_default());
                state.matched += 1;
                if let Some(flag) = state.found.get_mut(pi) {
                    *flag = true;
                }
                if let Some(of) = of {
                    let got = get(tail(value, of), NO_KEYS);
                    cb(go_int(Some(pi)), got.value, got.data_type, got.err);
                }
            }
        }
        cur_idx += 1;
    });
    if state.matched == paths.len() {
        return ArrayStep::Done;
    }
    // Go: i += arrOff - 1; arrOff is at least 1 here, since data[i] is '['.
    ArrayStep::Continue((i + arr_off.unwrap_or(1)).saturating_sub(1))
}

/// `createInsertComponent`: the bytes `Set` inserts for the missing part of a path.
fn create_insert_component<K: AsRef<[u8]>>(
    keys: &[K],
    set_value: &[u8],
    comma: bool,
    object: bool,
) -> Vec<u8> {
    // Go: buger/jsonparser parser.go createInsertComponent
    let is_index = |k: &K| k.as_ref().first() == Some(&b'[');
    let first_is_index = keys.first().is_some_and(is_index);
    let mut buffer = Vec::with_capacity(set_value.len() + 16 * keys.len());
    if comma {
        buffer.push(b',');
    }
    if first_is_index && !comma {
        buffer.push(b'[');
    } else {
        if object {
            buffer.push(b'{');
        }
        if !first_is_index {
            buffer.push(b'"');
            buffer.extend_from_slice(key_at(keys, 0).unwrap_or_default());
            buffer.extend_from_slice(b"\":");
        }
    }
    for key in keys.iter().skip(1) {
        if is_index(key) {
            buffer.push(b'[');
        } else {
            buffer.extend_from_slice(b"{\"");
            buffer.extend_from_slice(key.as_ref());
            buffer.extend_from_slice(b"\":");
        }
    }
    buffer.extend_from_slice(set_value);
    for key in keys.iter().skip(1).rev() {
        buffer.push(if is_index(key) { b']' } else { b'}' });
    }
    if first_is_index && !comma {
        buffer.push(b']');
    }
    if object && !first_is_index {
        buffer.push(b'}');
    }
    buffer
}

/// `head + insert + rest` in one allocation.
fn splice(head: &[u8], insert: &[u8], rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(head.len() + insert.len() + rest.len());
    out.extend_from_slice(head);
    out.extend_from_slice(insert);
    out.extend_from_slice(rest);
    out
}

/// `jsonparser.Set(data, setValue, keys...)`: `data` with the value at the path replaced by
/// `set_value` (written as it is: a string needs its quotes), or the missing part of the path
/// inserted.
///
/// A missing key is appended to the deepest existing object on the path (keys are written
/// without escaping); an existing empty object or a non-object on the path is overwritten.
///
/// # Errors
///
/// [`ParserError::KeyPathNotFound`] with no keys, or when a top-level key is missing and `data` is
/// not an object; [`get`]'s parse error otherwise.
pub fn set<K: AsRef<[u8]>>(
    data: &[u8],
    set_value: &[u8],
    keys: &[K],
) -> Result<Vec<u8>, ParserError> {
    // Go: buger/jsonparser parser.go Set
    if keys.is_empty() {
        return Err(ParserError::KeyPathNotFound);
    }
    let found = internal_get(data, keys);
    match found.err {
        None => Ok(splice(
            data.get(..found.offset.unwrap_or_default())
                .unwrap_or_default(),
            set_value,
            tail(data, found.end.unwrap_or_default()),
        )),
        Some(ParserError::KeyPathNotFound) => set_missing(data, set_value, keys),
        Some(err) => Err(err),
    }
}

/// `Set` when the full path does not exist.
fn set_missing<K: AsRef<[u8]>>(
    data: &[u8],
    set_value: &[u8],
    keys: &[K],
) -> Result<Vec<u8>, ParserError> {
    // Does any subpath exist?
    let mut start_offset = 0;
    let mut end_offset: Option<usize> = None;
    let mut depth = 0;
    for i in 0..keys.len() {
        let sub = internal_get(data, keys.get(..=i).unwrap_or_default());
        if sub.err.is_some() {
            break;
        }
        end_offset = sub.end;
        start_offset = sub.offset.unwrap_or_default();
        depth += 1;
    }
    let mut comma = true;
    let mut object = false;
    let end_offset = if let Some(end) = end_offset {
        end
    } else {
        // A top-level key can only be added to an object.
        let first = next_token(data)
            .filter(|&f| data.get(f) == Some(&b'{'))
            .ok_or(ParserError::KeyPathNotFound)?;
        // Go: firstToken + 1 + nextToken(...), where nextToken may be -1.
        let second = next_token(tail(data, first + 1)).map_or(first, |n| first + 1 + n);
        if data.get(second) == Some(&b'}') {
            comma = false;
        }
        last_token(data).unwrap_or(first)
    };
    let mut depth_offset = end_offset;
    if depth == 0 {
        start_offset = depth_offset;
    } else {
        let s = start_offset;
        let after = next_token(tail(data, s + 1)).map_or(s, |n| s + 1 + n);
        let into_object = data.get(s) == Some(&b'{') && data.get(after) != Some(&b'}');
        let into_array = data.get(s) == Some(&b'[')
            && data.get(after) == Some(&b'{')
            && key_at(keys, depth).is_some_and(|k| k.first() == Some(&b'['));
        if into_object || into_array {
            depth_offset = depth_offset.saturating_sub(1);
            start_offset = depth_offset;
        } else {
            comma = false;
            object = true;
        }
    }
    let insert = create_insert_component(
        keys.get(depth..).unwrap_or_default(),
        set_value,
        comma,
        object,
    );
    Ok(splice(
        data.get(..start_offset).unwrap_or_default(),
        &insert,
        tail(data, depth_offset),
    ))
}

/// `jsonparser.Delete(data, keys...)`: `data` without the key (or array element) at the path.
///
/// The comma that separated it goes too. `data` is returned unchanged when the path is not
/// found; with no keys the result is empty.
#[must_use]
pub fn delete<'a, K: AsRef<[u8]>>(data: &'a [u8], keys: &[K]) -> Cow<'a, [u8]> {
    // Go: buger/jsonparser parser.go Delete
    if keys.is_empty() {
        return Cow::Borrowed(&[]);
    }
    match delete_range(data, keys) {
        Some((new_offset, end_offset)) => Cow::Owned(splice(
            data.get(..new_offset).unwrap_or_default(),
            &[],
            tail(data, end_offset),
        )),
        None => Cow::Borrowed(data),
    }
}

/// The bytes `Delete` keeps (`data[:newOffset]` and `data[endOffset:]`), or `None` when it
/// returns `data` (or Go would panic).
fn delete_range<K: AsRef<[u8]>>(data: &[u8], keys: &[K]) -> Option<(usize, usize)> {
    let last = keys.last()?.as_ref();
    let mut key_offset;
    let mut end_offset;
    if last.first() == Some(&b'[') {
        let found = internal_get(data, keys);
        if found.err.is_some() {
            return None;
        }
        key_offset = found.offset?;
        end_offset = found.end?;
        let tok_end = token_end(tail(data, end_offset));
        let tok_start = find_token_start(data.get(..key_offset)?, b',');
        let at_end = *data.get(end_offset + tok_end)?;
        if at_end == b',' {
            end_offset += tok_end + 1;
        } else if at_end == b']' && data.get(tok_start) == Some(&b',') {
            key_offset = tok_start;
        }
    } else {
        let mut start_offset = 0;
        end_offset = data.len();
        if keys.len() > 1 {
            let parent = internal_get(data, keys.get(..keys.len() - 1)?);
            if parent.err.is_some() {
                return None;
            }
            start_offset = parent.offset?;
            end_offset = parent.end?;
        }
        let scope = data.get(start_offset..end_offset)?;
        key_offset = start_offset + find_key_start(scope, last)?;
        let sub = internal_get(scope, &[last]);
        if sub.err.is_some() {
            return None;
        }
        end_offset = start_offset + sub.end?;
        let tok_end = token_end(tail(data, end_offset));
        let tok_start = find_token_start(data.get(..key_offset)?, b',');
        let at_end = *data.get(end_offset + tok_end)?;
        if at_end == b',' {
            end_offset += tok_end + 1;
        } else if at_end == b' ' && data.get(end_offset + tok_end + 1) == Some(&b',') {
            end_offset += tok_end + 2;
        } else if at_end == b'}' && data.get(tok_start) == Some(&b',') {
            key_offset = tok_start;
        }
    }
    // Remove the comma left before a deleted last member.
    let prev_tok = last_token(data.get(..key_offset)?);
    let remained = tail(data, end_offset);
    let closes = next_token(remained).is_some_and(|n| remained.get(n) == Some(&b'}'));
    let new_offset = match prev_tok {
        Some(p) if closes && data.get(p) == Some(&b',') => p,
        Some(p) => p + 1,
        // Go reads data[-1] (and panics) when the rest starts with '}'.
        None if closes => return None,
        None => 0,
    };
    Some((new_offset, end_offset))
}

#[cfg(test)]
mod tests;
