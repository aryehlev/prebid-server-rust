//! jsoniter's `Iterator` (`iter.go`, `iter_int.go`, `iter_float.go`, `iter_str.go`,
//! `iter_skip*.go`, `iter_object.go`): token reading, numbers, strings, skipping, `Read`, error
//! reporting and nesting depth, in one inherent `impl` as the lint policy requires.

use core::mem;

use serde_json::{Map, Number, Value};

use crate::{
    error::JsonError,
    gotype::GoType,
    strconv::{big_parse_float_error, f64_to_f32, parse_float, parse_float_error_text},
    unicode::go_lossy,
    value::drop_value,
};

/// jsoniter's `Iterator.Error` slot. `Eof` is Go's `io.EOF`, which later errors may overwrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum IterError {
    Eof,
    Msg(Vec<u8>),
}

/// jsoniter's value type classification (`WhatIsNext`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    /// Anything else (Go prints it as `0`).
    Invalid,
    /// `"`
    String,
    /// `-` or a digit
    Number,
    /// `n`
    Nil,
    /// `t` or `f`
    Bool,
    /// `[`
    Array,
    /// `{`
    Object,
}

const fn value_type(c: u8) -> ValueType {
    match c {
        b'"' => ValueType::String,
        b'-' | b'0'..=b'9' => ValueType::Number,
        b't' | b'f' => ValueType::Bool,
        b'n' => ValueType::Nil,
        b'[' => ValueType::Array,
        b'{' => ValueType::Object,
        _ => ValueType::Invalid,
    }
}

const MAX_DEPTH: i64 = 10000;

/// A port of json-iterator's `Iterator` over an in-memory buffer.
///
/// It behaves as `ConfigCompatibleWithStandardLibrary` (case-insensitive field matching) and
/// reproduces jsoniter's consumption and error messages byte for byte, including how it keeps
/// reading after an error.
#[derive(Debug)]
pub struct Iter<'a> {
    buf: &'a [u8],
    head: usize,
    pub(super) depth: i64,
    error: Option<IterError>,
    /// Decoding for `jsonutil.MergeClone` (its frozen config's `mergeCloneExtension`).
    merge_clone: bool,
}

// Go: json-iterator/go iter_int.go

const UINT32_SAFE_TO_MULTIPLY_10: u32 = u32::MAX / 10 - 1;
const UINT64_SAFE_TO_MULTIPLY_10: u64 = u64::MAX / 10 - 1;

/// `intDigits[c]`: the digit value, or `None` for `invalidCharForNumber`.
const fn int_digit(c: u8) -> Option<u8> {
    if c.is_ascii_digit() {
        Some(c - b'0')
    } else {
        None
    }
}

/// The `unexpected character` message: Go appends `byte(invalidCharForNumber)`, i.e. `0xFF`.
const UNEXPECTED_CHARACTER: &[u8] = b"unexpected character: \xff";

// Go: json-iterator/go iter_float.go

const MAX_FLOAT64: u64 = (1 << 53) - 1;
/// `pow10` from jsoniter's `stream_float.go`.
const POW10: [u64; 7] = [1, 10, 100, 1000, 10000, 100_000, 1_000_000];

/// `floatDigits[c]`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FloatDigit {
    Digit(u8),
    Invalid,
    End,
    Dot,
}

const fn float_digit(c: u8) -> FloatDigit {
    match c {
        b'0'..=b'9' => FloatDigit::Digit(c - b'0'),
        b',' | b']' | b'}' | b' ' | b'\t' | b'\n' => FloatDigit::End,
        b'.' => FloatDigit::Dot,
        _ => FloatDigit::Invalid,
    }
}

/// Exact `float64(v)` for a `uint64` (round half to even).
fn u64_to_f64(v: u64) -> f64 {
    let hi = u32::try_from(v >> 32).unwrap_or(u32::MAX);
    let lo = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(u32::MAX);
    // `hi * 2^32` is exact, so the single rounding of the addition gives the correctly rounded
    // result.
    f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
}

/// Exact `float32(v)` for a `uint64`.
fn u64_to_f32(v: u64) -> f32 {
    v.to_string().parse::<f32>().unwrap_or(0.0)
}

/// Which float width is being read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    F32,
    F64,
}

impl Width {
    const fn op_fast(self) -> &'static str {
        match self {
            Self::F32 => "readFloat32",
            Self::F64 => "readFloat64",
        }
    }

    const fn op_slow(self) -> &'static str {
        match self {
            Self::F32 => "readFloat32SlowPath",
            Self::F64 => "readFloat64SlowPath",
        }
    }
}

/// jsoniter `validateFloat`.
fn validate_float(s: &[u8]) -> Option<&'static [u8]> {
    let Some(&first) = s.first() else {
        return Some(b"empty number");
    };
    if first == b'-' {
        return Some(b"-- is not valid");
    }
    if let Some(dot) = s.iter().position(|&c| c == b'.') {
        if dot == s.len() - 1 {
            return Some(b"dot can not be last character");
        }
        if !s.get(dot + 1).is_some_and(u8::is_ascii_digit) {
            return Some(b"missing digit after dot");
        }
    }
    None
}

// Go: json-iterator/go iter_str.go

/// jsoniter `appendRune`: surrogates and out-of-range values become U+FFFD.
fn append_rune(out: &mut Vec<u8>, r: u32) {
    let c = char::from_u32(r).unwrap_or('\u{FFFD}');
    let mut buf = [0_u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

const fn is_surrogate(r: u32) -> bool {
    r >= 0xD800 && r < 0xE000
}

/// Go's `utf16.DecodeRune`: the code point of a surrogate pair, or U+FFFD.
const fn decode_surrogates(r1: u32, r2: u32) -> u32 {
    if r1 >= 0xD800 && r1 < 0xDC00 && r2 >= 0xDC00 && r2 < 0xE000 {
        ((r1 - 0xD800) << 10 | (r2 - 0xDC00)) + 0x10000
    } else {
        0xFFFD
    }
}

// Go: json-iterator/go iter_skip.go, iter_skip_strict.go

/// A container being skipped, waiting for its next element.
#[derive(Clone, Copy)]
enum Frame {
    Array,
    Object,
}

// Go: json-iterator/go iter.go Read

/// What follows after a container's opening or after one of its elements.
enum Next {
    /// Another element follows (for objects, its key has been read).
    Element(String),
    /// The container is finished.
    Done,
}

/// A container being read, with the nesting depth recorded before its current element
/// (`ReadVal`'s mismatched-nesting check).
struct Open {
    value: Value,
    key: String,
    depth: i64,
}

impl<'a> Iter<'a> {
    /// An iterator over `buf` (Go's `ParseBytes` / `BorrowIterator`).
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            head: 0,
            depth: 0,
            error: None,
            merge_clone: false,
        }
    }

    /// An iterator over `buf` for `jsonutil.MergeClone`, whose frozen jsoniter config carries
    /// the `mergeCloneExtension`: every slice is set to nil before it is decoded (so its
    /// elements start from zero values) and every `json.RawMessage` is merged into the
    /// existing one with `jsonpatch.MergePatch` instead of being replaced.
    ///
    /// The decoders consult [`Iter::is_merge_clone`]; a `GoUnmarshaler` decodes with its own
    /// iterator, as Go's `UnmarshalJSON` methods call `jsonutil.Unmarshal`.
    #[must_use]
    pub const fn new_merge_clone(buf: &'a [u8]) -> Self {
        // Go: util/jsonutil/merge.go jsonConfigMergeClone
        let mut it = Self::new(buf);
        it.merge_clone = true;
        it
    }

    /// Whether the iterator decodes for `jsonutil.MergeClone` ([`Iter::new_merge_clone`]).
    #[must_use]
    pub const fn is_merge_clone(&self) -> bool {
        self.merge_clone
    }

    pub(crate) const fn tail(&self) -> usize {
        self.buf.len()
    }

    pub(crate) fn byte_at(&self, i: usize) -> u8 {
        self.buf.get(i).copied().unwrap_or(0)
    }

    /// Whether an error (other than end of input) has been recorded.
    #[must_use]
    pub const fn has_error(&self) -> bool {
        matches!(self.error, Some(IterError::Msg(_)))
    }

    /// Whether any error, including end of input, has been recorded (`iter.Error != nil`).
    #[must_use]
    pub const fn has_any_error(&self) -> bool {
        self.error.is_some()
    }

    /// The recorded error, if it is not end of input.
    #[must_use]
    pub fn error(&self) -> Option<JsonError> {
        match &self.error {
            Some(IterError::Msg(m)) => Some(JsonError::Message(m.clone())),
            Some(IterError::Eof) | None => None,
        }
    }

    /// Records an error the way `iter.Error = err` does (no `ReportError` decoration).
    pub fn set_error(&mut self, text: Vec<u8>) {
        self.error = Some(IterError::Msg(text));
    }

    /// Wraps a recorded non-EOF error as `fmt.Errorf("{prefix}{sep}%s", err)`.
    pub fn wrap_error(&mut self, prefix: &str, separator: &str) {
        if let Some(IterError::Msg(m)) = self.error.as_mut() {
            let mut wrapped = Vec::with_capacity(prefix.len() + separator.len() + m.len());
            wrapped.extend_from_slice(prefix.as_bytes());
            wrapped.extend_from_slice(separator.as_bytes());
            wrapped.extend_from_slice(m);
            *m = wrapped;
        }
    }

    /// Wraps a recorded non-EOF error with a Go type name, computed only when needed.
    pub fn wrap_error_with_type<T: GoType + ?Sized>(&mut self, separator: &str) {
        if self.has_error() {
            let mut name = String::new();
            T::go_type_name(&mut name);
            self.wrap_error(&name, separator);
        }
    }

    /// jsoniter `ReportError`: records `"{operation}: {msg}, error found in #N byte of ...|...|...,
    /// bigger context ...|...|..."` unless a non-EOF error is already recorded.
    pub fn report_error(&mut self, operation: &str, msg: &[u8]) {
        // Go: json-iterator/go iter.go ReportError
        if matches!(self.error, Some(IterError::Msg(_))) {
            return;
        }
        let tail = self.tail();
        let peek_start = self.head.saturating_sub(10);
        let peek_end = (self.head + 10).min(tail);
        let context_start = self.head.saturating_sub(50);
        let context_end = (self.head + 50).min(tail);
        let mut out = Vec::with_capacity(operation.len() + msg.len() + 200);
        out.extend_from_slice(operation.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(msg);
        out.extend_from_slice(b", error found in #");
        out.extend_from_slice((self.head - peek_start).to_string().as_bytes());
        out.extend_from_slice(b" byte of ...|");
        out.extend_from_slice(self.buf.get(peek_start..peek_end).unwrap_or_default());
        out.extend_from_slice(b"|..., bigger context ...|");
        out.extend_from_slice(self.buf.get(context_start..context_end).unwrap_or_default());
        out.extend_from_slice(b"|...");
        self.error = Some(IterError::Msg(out));
    }

    /// `ReportError` with a message built from a prefix and one raw byte (`string([]byte{c})`).
    pub(crate) fn report_error_byte(&mut self, operation: &str, prefix: &str, c: u8) {
        let mut msg = prefix.as_bytes().to_vec();
        msg.push(c);
        self.report_error(operation, &msg);
    }

    /// jsoniter `loadMore` for an in-memory buffer: records end of input.
    pub(crate) fn load_more(&mut self) {
        if self.error.is_none() {
            self.head = self.tail();
            self.error = Some(IterError::Eof);
        }
    }

    /// jsoniter `nextToken`: the next non-whitespace byte, or 0 at the end of input.
    pub fn next_token(&mut self) -> u8 {
        // Go: json-iterator/go iter.go nextToken
        let mut i = self.head;
        while let Some(&c) = self.buf.get(i) {
            if matches!(c, b' ' | b'\n' | b'\t' | b'\r') {
                i += 1;
                continue;
            }
            self.head = i + 1;
            return c;
        }
        self.load_more();
        0
    }

    /// jsoniter `readByte`.
    pub(crate) fn read_byte(&mut self) -> u8 {
        if let Some(&c) = self.buf.get(self.head) {
            self.head += 1;
            return c;
        }
        self.load_more();
        0
    }

    /// jsoniter `unreadByte`: a no-op once any error (including EOF) is recorded.
    pub const fn unread_byte(&mut self) {
        if self.error.is_some() {
            return;
        }
        self.head = self.head.saturating_sub(1);
    }

    /// jsoniter `WhatIsNext`.
    pub fn what_is_next(&mut self) -> ValueType {
        let t = value_type(self.next_token());
        self.unread_byte();
        t
    }

    /// jsoniter `isObjectEnd`.
    pub(crate) fn is_object_end(&mut self) -> bool {
        let c = self.next_token();
        if c == b',' {
            return false;
        }
        if c == b'}' {
            return true;
        }
        self.report_error_byte(
            "isObjectEnd",
            "object ended prematurely, unexpected char ",
            c,
        );
        true
    }

    /// jsoniter `incrementDepth`.
    pub(crate) fn increment_depth(&mut self) -> bool {
        self.depth += 1;
        if self.depth <= MAX_DEPTH {
            return true;
        }
        self.report_error("incrementDepth", b"exceeded max depth");
        false
    }

    /// jsoniter `decrementDepth`.
    pub(crate) fn decrement_depth(&mut self) {
        self.depth -= 1;
        if self.depth < 0 {
            self.report_error("decrementDepth", b"unexpected negative nesting");
        }
    }

    /// jsoniter `ReadNil`: consumes `null` and returns true, otherwise leaves the input alone.
    pub fn read_nil(&mut self) -> bool {
        let c = self.next_token();
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return true;
        }
        self.unread_byte();
        false
    }

    /// jsoniter `ReadBool`.
    pub fn read_bool(&mut self) -> bool {
        let c = self.next_token();
        if c == b't' {
            self.skip_three_bytes(b'r', b'u', b'e');
            return true;
        }
        if c == b'f' {
            self.skip_four_bytes(b'a', b'l', b's', b'e');
            return false;
        }
        self.report_error_byte("ReadBool", "expect t or f, but found ", c);
        false
    }

    /// jsoniter `skipThreeBytes`.
    pub(crate) fn skip_three_bytes(&mut self, b1: u8, b2: u8, b3: u8) {
        for expected in [b1, b2, b3] {
            if self.read_byte() != expected {
                let mut msg = b"expect ".to_vec();
                msg.extend_from_slice(&[b1, b2, b3]);
                self.report_error("skipThreeBytes", &msg);
                return;
            }
        }
    }

    /// jsoniter `skipFourBytes`.
    pub(crate) fn skip_four_bytes(&mut self, b1: u8, b2: u8, b3: u8, b4: u8) {
        for expected in [b1, b2, b3, b4] {
            if self.read_byte() != expected {
                let mut msg = b"expect ".to_vec();
                msg.extend_from_slice(&[b1, b2, b3, b4]);
                self.report_error("skipFourBytes", &msg);
                return;
            }
        }
    }

    /// jsoniter `readObjectStart`: true when a non-empty object follows.
    pub(crate) fn read_object_start(&mut self) -> bool {
        let mut c = self.next_token();
        if c == b'{' {
            c = self.next_token();
            if c == b'}' {
                return false;
            }
            self.unread_byte();
            return true;
        } else if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return false;
        }
        self.report_error_byte("readObjectStart", "expect { or n, but found ", c);
        false
    }

    /// jsoniter `SkipAndReturnBytes`: skips one value and returns its exact bytes.
    pub fn skip_and_return_bytes(&mut self) -> Vec<u8> {
        let start = self.head;
        self.skip();
        self.buf
            .get(start..self.head.max(start))
            .unwrap_or_default()
            .to_vec()
    }

    /// jsoniter `readFieldHash`: the case-folded FNV hash of an object key, consuming the `:`.
    pub(crate) fn read_field_hash(&mut self) -> i64 {
        // Go: json-iterator/go iter_object.go readFieldHash
        let mut hash: i64 = 0x811C_9DC5;
        let c = self.next_token();
        if c != b'"' {
            self.report_error_byte("readFieldHash", "expect \", but found ", c);
            return 0;
        }
        let mut i = self.head;
        while let Some(&b) = self.buf.get(i) {
            if b == b'\\' {
                self.head = i;
                for unescaped in self.read_string_slow_path() {
                    hash = super::fnv_step(hash, unescaped.to_ascii_lowercase());
                }
                return self.finish_field_hash(hash);
            }
            if b == b'"' {
                self.head = i + 1;
                return self.finish_field_hash(hash);
            }
            hash = super::fnv_step(hash, b.to_ascii_lowercase());
            i += 1;
        }
        self.load_more();
        self.report_error("readFieldHash", b"incomplete field name");
        0
    }

    fn finish_field_hash(&mut self, hash: i64) -> i64 {
        let c = self.next_token();
        if c != b':' {
            self.report_error_byte("readFieldHash", "expect :, but found ", c);
            return 0;
        }
        hash
    }

    // Go: json-iterator/go iter_int.go

    /// jsoniter `assertInteger`.
    fn assert_integer(&mut self) {
        if self.byte_at(self.head) == b'.' && self.head < self.tail() {
            self.report_error("assertInteger", b"can not decode float as int");
        }
    }

    /// The unrolled fast path shared by `readUint32` and `readUint64`: up to eight more digits
    /// when more than ten bytes remain. Returns `Ok(value)` when the number ended inside the
    /// window, or `Err(value)` to continue in the general loop.
    fn read_uint_fast_path(&mut self, mut value: u64) -> Result<u64, u64> {
        // Go: json-iterator/go iter_int.go readUint32/readUint64 (ind2 … ind9)
        if self.tail() - self.head <= 10 {
            return Err(value);
        }
        let mut i = self.head;
        for _ in 0..7 {
            let Some(d) = int_digit(self.byte_at(i)) else {
                self.head = i;
                self.assert_integer();
                return Ok(value);
            };
            value = value * 10 + u64::from(d);
            i += 1;
        }
        self.head = i;
        if int_digit(self.byte_at(i)).is_none() {
            self.assert_integer();
            return Ok(value);
        }
        Err(value)
    }

    /// jsoniter `readUint64`.
    pub(crate) fn read_uint64(&mut self, c: u8) -> u64 {
        let Some(first) = int_digit(c) else {
            self.report_error("readUint64", UNEXPECTED_CHARACTER);
            return 0;
        };
        if first == 0 {
            self.assert_integer();
            return 0;
        }
        let mut value = match self.read_uint_fast_path(u64::from(first)) {
            Ok(v) => return v,
            Err(v) => v,
        };
        let mut i = self.head;
        while let Some(&b) = self.buf.get(i) {
            let Some(d) = int_digit(b) else {
                self.head = i;
                self.assert_integer();
                return value;
            };
            let next = (value << 3)
                .wrapping_add(value << 1)
                .wrapping_add(u64::from(d));
            if value > UINT64_SAFE_TO_MULTIPLY_10 && next < value {
                self.report_error("readUint64", b"overflow");
                return 0;
            }
            value = next;
            i += 1;
        }
        self.load_more();
        self.assert_integer();
        value
    }

    /// jsoniter `readUint32`.
    pub(crate) fn read_uint32(&mut self, c: u8) -> u32 {
        let Some(first) = int_digit(c) else {
            self.report_error("readUint32", UNEXPECTED_CHARACTER);
            return 0;
        };
        if first == 0 {
            self.assert_integer();
            return 0;
        }
        let mut value = match self.read_uint_fast_path(u64::from(first)) {
            Ok(v) => return u32::try_from(v).unwrap_or(u32::MAX),
            Err(v) => u32::try_from(v).unwrap_or(u32::MAX),
        };
        let mut i = self.head;
        while let Some(&b) = self.buf.get(i) {
            let Some(d) = int_digit(b) else {
                self.head = i;
                self.assert_integer();
                return value;
            };
            let next = (value << 3)
                .wrapping_add(value << 1)
                .wrapping_add(u32::from(d));
            if value > UINT32_SAFE_TO_MULTIPLY_10 && next < value {
                self.report_error("readUint32", b"overflow");
                return 0;
            }
            value = next;
            i += 1;
        }
        self.load_more();
        self.assert_integer();
        value
    }

    /// jsoniter `ReadUint64`.
    pub fn read_uint64_value(&mut self) -> u64 {
        let c = self.next_token();
        self.read_uint64(c)
    }

    /// jsoniter `ReadUint32`.
    pub fn read_uint32_value(&mut self) -> u32 {
        let c = self.next_token();
        self.read_uint32(c)
    }

    /// jsoniter `ReadInt64`.
    pub fn read_int64(&mut self) -> i64 {
        let c = self.next_token();
        if c == b'-' {
            let next = self.read_byte();
            let val = self.read_uint64(next);
            if val > (1_u64 << 63) {
                self.report_error("ReadInt64", format!("overflow: {val}").as_bytes());
                return 0;
            }
            return 0_i64.wrapping_sub_unsigned(val);
        }
        let val = self.read_uint64(c);
        if let Ok(v) = i64::try_from(val) {
            return v;
        }
        self.report_error("ReadInt64", format!("overflow: {val}").as_bytes());
        0
    }

    /// The signed narrow reads (`ReadInt8/16/32`): `limit` is the type's maximum.
    fn read_signed_narrow(&mut self, op: &str, limit: u32) -> i64 {
        let c = self.next_token();
        if c == b'-' {
            let next = self.read_byte();
            let val = self.read_uint32(next);
            if val > limit + 1 {
                self.report_error(op, format!("overflow: {val}").as_bytes());
                return 0;
            }
            return -i64::from(val);
        }
        let val = self.read_uint32(c);
        if val > limit {
            self.report_error(op, format!("overflow: {val}").as_bytes());
            return 0;
        }
        i64::from(val)
    }

    /// The unsigned narrow reads (`ReadUint8/16`): `limit` is the type's maximum.
    fn read_unsigned_narrow(&mut self, op: &str, limit: u32) -> u32 {
        let c = self.next_token();
        let val = self.read_uint32(c);
        if val > limit {
            self.report_error(op, format!("overflow: {val}").as_bytes());
            return 0;
        }
        val
    }

    /// jsoniter `ReadInt8`.
    pub fn read_int8(&mut self) -> i8 {
        i8::try_from(self.read_signed_narrow("ReadInt8", 127)).unwrap_or_default()
    }

    /// jsoniter `ReadInt16`.
    pub fn read_int16(&mut self) -> i16 {
        i16::try_from(self.read_signed_narrow("ReadInt16", 32767)).unwrap_or_default()
    }

    /// jsoniter `ReadInt32`.
    pub fn read_int32(&mut self) -> i32 {
        i32::try_from(self.read_signed_narrow("ReadInt32", 2_147_483_647)).unwrap_or_default()
    }

    /// jsoniter `ReadUint8`.
    pub fn read_uint8(&mut self) -> u8 {
        u8::try_from(self.read_unsigned_narrow("ReadUint8", 255)).unwrap_or_default()
    }

    /// jsoniter `ReadUint16`.
    pub fn read_uint16(&mut self) -> u16 {
        u16::try_from(self.read_unsigned_narrow("ReadUint16", 65535)).unwrap_or_default()
    }

    // Go: json-iterator/go iter_float.go

    /// jsoniter `readNumberAsString`.
    pub(crate) fn read_number_as_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = self.head;
        loop {
            match self.buf.get(i) {
                Some(&c) if matches!(c, b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9') => {
                    out.push(c);
                    i += 1;
                }
                Some(_) => {
                    self.head = i;
                    break;
                }
                None => {
                    self.load_more();
                    break;
                }
            }
        }
        if self.has_error() {
            return Vec::new();
        }
        if out.is_empty() {
            self.report_error("readNumberAsString", b"invalid number");
        }
        out
    }

    fn read_float_slow_path(&mut self, width: Width) -> f64 {
        let s = self.read_number_as_string();
        if self.has_error() {
            return 0.0;
        }
        if let Some(msg) = validate_float(&s) {
            self.report_error(width.op_slow(), msg);
            return 0.0;
        }
        match parse_float(&s, width == Width::F32) {
            Ok(v) => v,
            Err(kind) => {
                self.set_error(parse_float_error_text(&s, kind));
                0.0
            }
        }
    }

    /// jsoniter `readPositiveFloat64` / `readPositiveFloat32` (the latter rounds to `f32`).
    fn read_positive_float(&mut self, width: Width) -> f64 {
        let mut i = self.head;
        let Some(&c0) = self.buf.get(i) else {
            return self.read_float_slow_path(width);
        };
        i += 1;
        let first = float_digit(c0);
        match first {
            FloatDigit::Invalid => return self.read_float_slow_path(width),
            FloatDigit::End => {
                self.report_error(width.op_fast(), b"empty number");
                return 0.0;
            }
            FloatDigit::Dot => {
                self.report_error(width.op_fast(), b"leading dot is invalid");
                return 0.0;
            }
            FloatDigit::Digit(0) => {
                let Some(&c1) = self.buf.get(i) else {
                    return self.read_float_slow_path(width);
                };
                if c1.is_ascii_digit() {
                    self.report_error(width.op_fast(), b"leading zero is invalid");
                    return 0.0;
                }
            }
            FloatDigit::Digit(_) => {}
        }
        let mut value: u64 = match first {
            FloatDigit::Digit(d) => u64::from(d),
            FloatDigit::Invalid | FloatDigit::End | FloatDigit::Dot => 0,
        };
        let mut c = c0;
        let mut saw_dot = false;
        while let Some(&b) = self.buf.get(i) {
            c = b;
            match float_digit(b) {
                FloatDigit::Invalid => return self.read_float_slow_path(width),
                FloatDigit::End => {
                    self.head = i;
                    return match width {
                        Width::F64 => u64_to_f64(value),
                        Width::F32 => f64::from(u64_to_f32(value)),
                    };
                }
                FloatDigit::Dot => {
                    saw_dot = true;
                    break;
                }
                FloatDigit::Digit(d) => {
                    if value > UINT64_SAFE_TO_MULTIPLY_10 {
                        return self.read_float_slow_path(width);
                    }
                    value = value * 10 + u64::from(d);
                }
            }
            i += 1;
        }
        if saw_dot && c == b'.' {
            return self.read_decimal_part(width, value, i + 1);
        }
        self.read_float_slow_path(width)
    }

    fn read_decimal_part(&mut self, width: Width, mut value: u64, start: usize) -> f64 {
        let mut i = start;
        if i == self.tail() {
            return self.read_float_slow_path(width);
        }
        let mut decimal_places = 0_usize;
        while let Some(&b) = self.buf.get(i) {
            match float_digit(b) {
                FloatDigit::End => {
                    if let Some(&scale) = POW10.get(decimal_places)
                        && decimal_places > 0
                    {
                        self.head = i;
                        let quotient = u64_to_f64(value) / u64_to_f64(scale);
                        return match width {
                            Width::F64 => quotient,
                            Width::F32 => f64::from(f64_to_f32(quotient)),
                        };
                    }
                    return self.read_float_slow_path(width);
                }
                FloatDigit::Invalid | FloatDigit::Dot => return self.read_float_slow_path(width),
                FloatDigit::Digit(d) => {
                    decimal_places += 1;
                    if value > UINT64_SAFE_TO_MULTIPLY_10 {
                        return self.read_float_slow_path(width);
                    }
                    value = value * 10 + u64::from(d);
                    if width == Width::F64 && value > MAX_FLOAT64 {
                        return self.read_float_slow_path(width);
                    }
                }
            }
            i += 1;
        }
        self.read_float_slow_path(width)
    }

    fn read_float(&mut self, width: Width) -> f64 {
        let c = self.next_token();
        if c == b'-' {
            return -self.read_positive_float(width);
        }
        self.unread_byte();
        self.read_positive_float(width)
    }

    /// jsoniter `ReadFloat64`.
    pub fn read_float64(&mut self) -> f64 {
        self.read_float(Width::F64)
    }

    /// jsoniter `ReadFloat32`.
    pub fn read_float32(&mut self) -> f32 {
        f64_to_f32(self.read_float(Width::F32))
    }

    /// jsoniter `ReadBigFloat`, keeping only its error behaviour (the value is unused by the
    /// skip fallback that calls it).
    pub(crate) fn read_big_float(&mut self) {
        let s = self.read_number_as_string();
        if self.has_error() {
            return;
        }
        if let Some(err) = big_parse_float_error(&s) {
            self.set_error(err);
        }
    }

    // Go: json-iterator/go iter_str.go

    /// jsoniter `ReadString`: the string's bytes (Go strings are byte strings; invalid UTF-8 is
    /// kept). `null` reads as the empty string.
    pub fn read_string(&mut self) -> Vec<u8> {
        // Go: json-iterator/go iter_str.go ReadString
        let c = self.next_token();
        if c == b'"' {
            let mut i = self.head;
            while let Some(&b) = self.buf.get(i) {
                if b == b'"' {
                    let s = self.buf.get(self.head..i).unwrap_or_default().to_vec();
                    self.head = i + 1;
                    return s;
                } else if b == b'\\' {
                    break;
                } else if b < b' ' {
                    self.report_error(
                        "ReadString",
                        format!("invalid control character found: {b}").as_bytes(),
                    );
                    return Vec::new();
                }
                i += 1;
            }
            return self.read_string_slow_path();
        } else if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return Vec::new();
        }
        self.report_error_byte("ReadString", "expects \" or n, but found ", c);
        Vec::new()
    }

    /// jsoniter `readStringSlowPath`: unescapes until the closing quote. Control characters are
    /// accepted here (only the fast path rejects them).
    pub(crate) fn read_string_slow_path(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while self.error.is_none() {
            let c = self.read_byte();
            if c == b'"' {
                return out;
            }
            if c == b'\\' {
                let e = self.read_byte();
                out = self.read_escaped_char(e, out);
            } else {
                out.push(c);
            }
        }
        self.report_error("readStringSlowPath", b"unexpected end of input");
        Vec::new()
    }

    /// jsoniter `readEscapedChar`. Returns the (possibly cleared) accumulated bytes.
    fn read_escaped_char(&mut self, mut c: u8, mut out: Vec<u8>) -> Vec<u8> {
        match c {
            b'u' => {
                let r = self.read_u4();
                if !is_surrogate(r) {
                    append_rune(&mut out, r);
                    return out;
                }
                c = self.read_byte();
                if self.error.is_some() {
                    return Vec::new();
                }
                if c != b'\\' {
                    self.unread_byte();
                    append_rune(&mut out, r);
                    return out;
                }
                c = self.read_byte();
                if self.error.is_some() {
                    return Vec::new();
                }
                if c != b'u' {
                    append_rune(&mut out, r);
                    return self.read_escaped_char(c, out);
                }
                let r2 = self.read_u4();
                if self.error.is_some() {
                    return Vec::new();
                }
                let combined = decode_surrogates(r, r2);
                if combined == 0xFFFD {
                    append_rune(&mut out, r);
                    append_rune(&mut out, r2);
                } else {
                    append_rune(&mut out, combined);
                }
            }
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'/' => out.push(b'/'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            _ => {
                self.report_error("readEscapedChar", b"invalid escape char after \\");
                return Vec::new();
            }
        }
        out
    }

    /// jsoniter `readU4`.
    fn read_u4(&mut self) -> u32 {
        let mut ret: u32 = 0;
        for _ in 0..4 {
            let c = self.read_byte();
            if self.error.is_some() {
                return ret;
            }
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => {
                    self.report_error_byte("readU4", "expects 0~9 or a~f, but found ", c);
                    return ret;
                }
            };
            ret = ret * 16 + u32::from(digit);
        }
        ret
    }

    // Go: json-iterator/go iter_skip.go, iter_skip_strict.go

    /// jsoniter `Skip`: consumes one value, reporting the errors Go reports.
    pub fn skip(&mut self) {
        // Go: json-iterator/go iter_skip.go Skip (+ skipArray/skipObject →
        // ReadArrayCB/ReadObjectCB)
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let mut c = self.next_token();
            let opened = match c {
                b'"' => {
                    self.skip_string();
                    None
                }
                b'n' => {
                    self.skip_three_bytes(b'u', b'l', b'l');
                    None
                }
                b't' => {
                    self.skip_three_bytes(b'r', b'u', b'e');
                    None
                }
                b'f' => {
                    self.skip_four_bytes(b'a', b'l', b's', b'e');
                    None
                }
                b'0' => {
                    self.unread_byte();
                    let _ = self.read_float32();
                    None
                }
                b'-' | b'1'..=b'9' => {
                    self.skip_number();
                    None
                }
                b'[' => {
                    self.unread_byte();
                    self.array_cb_begin().then_some(Frame::Array)
                }
                b'{' => {
                    self.unread_byte();
                    self.object_cb_begin().then_some(Frame::Object)
                }
                _ => {
                    self.report_error("Skip", format!("do not know how to skip: {c}").as_bytes());
                    None
                }
            };
            if let Some(frame) = opened {
                stack.push(frame);
                continue;
            }
            // The value is complete: continue the innermost container, finishing containers
            // until one has another element to skip.
            loop {
                let Some(&frame) = stack.last() else {
                    return;
                };
                c = self.next_token();
                if c == b',' {
                    if matches!(frame, Frame::Object) {
                        self.object_key();
                    }
                    break;
                }
                match frame {
                    Frame::Array => {
                        if c != b']' {
                            self.report_error_byte(
                                "ReadArrayCB",
                                "expect ] in the end, but found ",
                                c,
                            );
                        }
                    }
                    Frame::Object => {
                        if c != b'}' {
                            self.report_error("ReadObjectCB", b"object not ended with }");
                        }
                    }
                }
                self.decrement_depth();
                let _closed = stack.pop();
            }
        }
    }

    /// The opening half of `ReadArrayCB`: true when an element follows.
    fn array_cb_begin(&mut self) -> bool {
        let mut c = self.next_token();
        if c == b'[' {
            if !self.increment_depth() {
                return false;
            }
            c = self.next_token();
            if c != b']' {
                self.unread_byte();
                return true;
            }
            self.decrement_depth();
            return false;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return false;
        }
        self.report_error_byte("ReadArrayCB", "expect [ or n, but found ", c);
        false
    }

    /// The opening half of `ReadObjectCB`: true when a value follows (its key already read).
    fn object_cb_begin(&mut self) -> bool {
        let mut c = self.next_token();
        if c == b'{' {
            if !self.increment_depth() {
                return false;
            }
            c = self.next_token();
            if c == b'"' {
                self.unread_byte();
                self.object_key();
                return true;
            }
            if c == b'}' {
                self.decrement_depth();
                return false;
            }
            self.report_error_byte("ReadObjectCB", "expect \" after {, but found ", c);
            self.decrement_depth();
            return false;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return false;
        }
        self.report_error_byte("ReadObjectCB", "expect { or n, but found ", c);
        false
    }

    /// An object key and its colon, as `ReadObjectCB` reads them.
    fn object_key(&mut self) {
        let _skipped = self.read_string();
        let c = self.next_token();
        if c != b':' {
            self.report_error_byte("ReadObject", "expect : after object field, but found ", c);
        }
    }

    /// jsoniter `skipString` (strict).
    fn skip_string(&mut self) {
        if !self.try_skip_string() {
            self.unread_byte();
            let _skipped = self.read_string();
        }
    }

    fn try_skip_string(&mut self) -> bool {
        let mut i = self.head;
        while let Some(&c) = self.buf.get(i) {
            if c == b'"' {
                self.head = i + 1;
                return true;
            } else if c == b'\\' {
                return false;
            } else if c < b' ' {
                self.report_error(
                    "trySkipString",
                    format!("invalid control character found: {c}").as_bytes(),
                );
                return true;
            }
            i += 1;
        }
        false
    }

    /// jsoniter `skipNumber` (strict).
    fn skip_number(&mut self) {
        if !self.try_skip_number() {
            self.unread_byte();
            if self.has_error() {
                return;
            }
            let _ = self.read_float64();
            if self.has_error() {
                self.error = None;
                self.read_big_float();
            }
        }
    }

    fn try_skip_number(&mut self) -> bool {
        let mut dot_found = false;
        let mut i = self.head;
        while let Some(&c) = self.buf.get(i) {
            match c {
                b'0'..=b'9' => {}
                b'.' => {
                    if dot_found {
                        self.report_error("validateNumber", b"more than one dot found in number");
                        return true;
                    }
                    if i + 1 == self.tail() {
                        return false;
                    }
                    if !self.byte_at(i + 1).is_ascii_digit() {
                        self.report_error("validateNumber", b"missing digit after dot");
                        return true;
                    }
                    dot_found = true;
                }
                b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r' => {
                    if self.head == i {
                        return false;
                    }
                    self.head = i;
                    return true;
                }
                _ => return false,
            }
            i += 1;
        }
        false
    }

    // Go: json-iterator/go iter.go Read

    /// The opening half of `ReadArrayCB`.
    fn read_array_begin(&mut self) -> Next {
        let mut c = self.next_token();
        if c == b'[' {
            if !self.increment_depth() {
                return Next::Done;
            }
            c = self.next_token();
            if c != b']' {
                self.unread_byte();
                return Next::Element(String::new());
            }
            self.decrement_depth();
            return Next::Done;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return Next::Done;
        }
        self.report_error_byte("ReadArrayCB", "expect [ or n, but found ", c);
        Next::Done
    }

    /// `ReadArrayCB` after an element.
    fn read_array_after(&mut self) -> Next {
        let c = self.next_token();
        if c == b',' {
            return Next::Element(String::new());
        }
        if c != b']' {
            self.report_error_byte("ReadArrayCB", "expect ] in the end, but found ", c);
        }
        self.decrement_depth();
        Next::Done
    }

    /// A key and its colon, as `ReadMapCB` reads them; `prev` is the token Go (mistakenly)
    /// prints in the missing-colon error.
    fn read_map_key(&mut self, prev: u8) -> Next {
        let field = go_lossy(&self.read_string());
        if self.next_token() != b':' {
            self.report_error_byte("ReadMapCB", "expect : after object field, but found ", prev);
            self.decrement_depth();
            return Next::Done;
        }
        Next::Element(field)
    }

    /// The opening half of `ReadMapCB`.
    fn read_map_begin(&mut self) -> Next {
        // Go: json-iterator/go iter_object.go ReadMapCB
        let mut c = self.next_token();
        if c == b'{' {
            if !self.increment_depth() {
                return Next::Done;
            }
            c = self.next_token();
            if c == b'"' {
                self.unread_byte();
                return self.read_map_key(c);
            }
            if c == b'}' {
                self.decrement_depth();
                return Next::Done;
            }
            self.report_error_byte("ReadMapCB", "expect \" after {, but found ", c);
            self.decrement_depth();
            return Next::Done;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return Next::Done;
        }
        self.report_error_byte("ReadMapCB", "expect { or n, but found ", c);
        Next::Done
    }

    /// `ReadMapCB` after an element.
    fn read_map_after(&mut self) -> Next {
        let c = self.next_token();
        if c == b',' {
            return self.read_map_key(c);
        }
        if c != b'}' {
            self.report_error("ReadMapCB", b"object not ended with }");
        }
        self.decrement_depth();
        Next::Done
    }
}

/// A scalar `Read`, or the container kind to open.
fn read_scalar(it: &mut Iter<'_>) -> Result<Value, ValueType> {
    match it.what_is_next() {
        ValueType::String => Ok(Value::String(go_lossy(&it.read_string()))),
        ValueType::Number => {
            Ok(Number::from_f64(it.read_float64()).map_or(Value::Null, Value::Number))
        }
        ValueType::Nil => {
            it.skip_four_bytes(b'n', b'u', b'l', b'l');
            Ok(Value::Null)
        }
        ValueType::Bool => Ok(Value::Bool(it.read_bool())),
        ValueType::Array => Err(ValueType::Array),
        ValueType::Object => Err(ValueType::Object),
        ValueType::Invalid => {
            it.report_error("Read", b"unexpected value type: 0");
            Ok(Value::Null)
        }
    }
}

/// jsoniter's `Iterator.Read`: strings, `float64` numbers, booleans, `null`, `[]interface{}`
/// and `map[string]interface{}`, with jsoniter's consumption and errors.
pub fn read_value(it: &mut Iter<'_>) -> Value {
    // Go: json-iterator/go iter.go Read (+ ReadArrayCB / ReadMapCB callbacks calling ReadVal)
    let mut stack: Vec<Open> = Vec::new();
    loop {
        let mut done = match read_scalar(it) {
            Ok(v) => v,
            Err(kind) => {
                let (next, container) = if kind == ValueType::Array {
                    (it.read_array_begin(), Value::Array(Vec::new()))
                } else {
                    (it.read_map_begin(), Value::Object(Map::new()))
                };
                match next {
                    Next::Element(key) => {
                        stack.push(Open {
                            value: container,
                            key,
                            depth: it.depth,
                        });
                        continue;
                    }
                    Next::Done => container,
                }
            }
        };
        loop {
            let Some(top) = stack.last_mut() else {
                return done;
            };
            if it.depth != top.depth {
                it.report_error("ReadVal", b"unexpected mismatched nesting");
            }
            let next = match &mut top.value {
                Value::Array(items) => {
                    items.push(done);
                    it.read_array_after()
                }
                Value::Object(map) => {
                    // A duplicate key replaces a value that can be as deep as the document.
                    if let Some(replaced) = map.insert(mem::take(&mut top.key), done) {
                        drop_value(replaced);
                    }
                    it.read_map_after()
                }
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Next::Done,
            };
            match next {
                Next::Element(key) => {
                    top.key = key;
                    top.depth = it.depth;
                    break;
                }
                Next::Done => {
                    done = stack.pop().map_or(Value::Null, |open| open.value);
                }
            }
        }
    }
}
