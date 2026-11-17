//! Decoding of the currency file exactly as Go's `jsonutil.UnmarshalValid(body, &Rates{})`.
//!
//! Go decodes the fetched body with json-iterator v1.1.12 in its
//! `ConfigCompatibleWithStandardLibrary` mode. That decoder is not a strict JSON parser: it
//! matches the `conversions` key case-insensitively through a hash, accepts `+5` and `null`
//! map keys, keeps decoding after the first error, and lets a later `conversions` key merge
//! into the first one. Which documents are accepted, the decoded rates, and the error text are
//! all reproduced here by porting the iterator functions that this one target type reaches
//! (`oneFieldStructDecoder`, `mapDecoder`, `stringCodec`, `float64Codec` and the strict
//! `Skip`), with the same buffer positions, so the messages, including their context excerpts,
//! are byte-identical. Then `jsonutil.tryExtractErrorMessage` shortens the message as Go does.
//!
//! Map keys are Go strings, which may hold any bytes; a key that is not valid UTF-8 is stored
//! with each invalid byte replaced by U+FFFD (what Go's JSON encoder prints for it). Two such
//! keys that only differ in their invalid bytes therefore collapse into one entry here.

use std::collections::BTreeMap;

use crate::rates::{ConversionRates, Rates};

/// Go: `reflect2.Type.String()` of the decoded struct, used to prefix errors.
const STRUCT_TYPE_NAME: &[u8] = b"currency.Rates";
/// Go: the struct field name, used to prefix errors.
const FIELD_NAME: &[u8] = b"Conversions";
/// Go: the JSON tag of the only field.
const FIELD_TAG: &[u8] = b"conversions";
/// Go: jsoniter `maxDepth`.
const MAX_DEPTH: i64 = 10_000;
/// Go: jsoniter `pow10`, whose length also bounds the fast float path.
const POW10: [u32; 7] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000];
/// Go: jsoniter `uint64SafeToMultiple10`.
const UINT64_SAFE_TO_MULTIPLE10: u64 = u64::MAX / 10 - 1;
/// Go: jsoniter `maxFloat64`.
const MAX_FLOAT64: u64 = (1 << 53) - 1;
/// Go: `utf8.RuneError`.
const RUNE_ERROR: u32 = 0xFFFD;

/// Decodes a currency file body (Go: `jsonutil.UnmarshalValid(body, &currency.Rates{})`).
///
/// The error is the `FailedToUnmarshal` message, as raw bytes because Go error strings may hold
/// any bytes of the input.
pub(super) fn unmarshal_valid(data: &[u8]) -> Result<Rates, Vec<u8>> {
    unmarshal(data).map_err(|raw| try_extract_error_message(&raw))
}

/// Go: `jsoniter.ConfigCompatibleWithStandardLibrary.Unmarshal(data, &Rates{})`.
fn unmarshal(data: &[u8]) -> Result<Rates, Vec<u8>> {
    let mut iter = Iter {
        buf: data,
        head: 0,
        depth: 0,
        error: None,
    };
    let mut conversions = None;
    let depth = iter.depth;
    iter.decode_rates(&mut conversions);
    if iter.depth != depth {
        iter.report_error("ReadVal", b"unexpected mismatched nesting");
    }
    let c = iter.next_token();
    if c != 0 {
        iter.report_error("Unmarshal", b"there are bytes left after unmarshal");
    }
    // Go returns iter.Error when the next token is 0 (a NUL byte or the end of input) and the
    // error is not io.EOF, so a NUL byte after the object ends decoding successfully.
    match iter.error {
        Some(IterError::Message(message)) => Err(message),
        Some(IterError::Eof) | None => Ok(Rates::new(conversions)),
    }
}

/// Go: `iter.Error`, either `io.EOF` or a formatted error.
#[derive(Debug)]
enum IterError {
    Eof,
    Message(Vec<u8>),
}

/// Go: jsoniter `floatDigits` classes.
#[derive(Debug, Clone, Copy)]
enum FloatChar {
    Digit(u8),
    End,
    Dot,
    Invalid,
}

const fn float_char(c: u8) -> FloatChar {
    match c {
        b'0'..=b'9' => FloatChar::Digit(c - b'0'),
        b',' | b']' | b'}' | b' ' | b'\t' | b'\n' => FloatChar::End,
        b'.' => FloatChar::Dot,
        _ => FloatChar::Invalid,
    }
}

/// A container opened by `Skip` whose remaining elements are still to be skipped.
#[derive(Debug, Clone, Copy)]
enum SkipFrame {
    Array,
    Object,
}

/// Go: `jsoniter.Iterator` over a byte slice (no reader, so `loadMore` only reports EOF).
#[derive(Debug)]
struct Iter<'a> {
    buf: &'a [u8],
    head: usize,
    depth: i64,
    error: Option<IterError>,
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

impl Iter<'_> {
    fn byte_at(&self, index: usize) -> u8 {
        self.buf.get(index).copied().unwrap_or(0)
    }

    const fn tail(&self) -> usize {
        self.buf.len()
    }

    /// Go: `iter.Error != nil && iter.Error != io.EOF`.
    const fn has_error(&self) -> bool {
        matches!(self.error, Some(IterError::Message(_)))
    }

    /// Go: `loadMore` without a reader, which never loads anything (so it always returns false).
    fn load_more(&mut self) {
        if self.error.is_none() {
            self.head = self.tail();
            self.error = Some(IterError::Eof);
        }
    }

    /// Go: `nextToken`.
    fn next_token(&mut self) -> u8 {
        let start = self.head;
        let skipped = self
            .buf
            .get(start..)
            .unwrap_or_default()
            .iter()
            .position(|c| !matches!(c, b' ' | b'\n' | b'\t' | b'\r'));
        if let Some(offset) = skipped {
            let index = start + offset;
            self.head = index + 1;
            return self.byte_at(index);
        }
        self.load_more();
        0
    }

    /// Go: `readByte`.
    fn read_byte(&mut self) -> u8 {
        if self.head == self.tail() {
            self.load_more();
            return 0;
        }
        let c = self.byte_at(self.head);
        self.head += 1;
        c
    }

    /// Go: `unreadByte`, a no-op once an error (even io.EOF) is recorded.
    const fn unread_byte(&mut self) {
        if self.error.is_some() {
            return;
        }
        self.head = self.head.saturating_sub(1);
    }

    /// Go: `ReportError`: the first non-EOF error wins.
    fn report_error(&mut self, operation: &str, message: &[u8]) {
        if self.has_error() {
            return;
        }
        let head = self.head;
        let tail = self.tail();
        let peek_start = head.saturating_sub(10);
        let peek_end = head.saturating_add(10).min(tail);
        let context_start = head.saturating_sub(50);
        let context_end = head.saturating_add(50).min(tail);
        let parsing = self.buf.get(peek_start..peek_end).unwrap_or_default();
        let context = self.buf.get(context_start..context_end).unwrap_or_default();
        let position = (head - peek_start).to_string();
        self.error = Some(IterError::Message(concat(&[
            operation.as_bytes(),
            b": ",
            message,
            b", error found in #",
            position.as_bytes(),
            b" byte of ...|",
            parsing,
            b"|..., bigger context ...|",
            context,
            b"|...",
        ])));
    }

    /// Prefixes the recorded error, as Go's `fmt.Errorf` wrappers do.
    fn wrap_error(&mut self, prefix: &[u8], separator: &[u8]) {
        if let Some(IterError::Message(message)) = &self.error {
            self.error = Some(IterError::Message(concat(&[prefix, separator, message])));
        }
    }

    // ---- decoders reached from `currency.Rates` ----

    /// Go: `oneFieldStructDecoder.Decode` for `Rates`.
    fn decode_rates(&mut self, conversions: &mut Option<ConversionRates>) {
        if !self.read_object_start() {
            return;
        }
        if !self.increment_depth() {
            return;
        }
        let field_hash = calc_hash(FIELD_TAG);
        loop {
            if self.read_field_hash() == field_hash {
                // Go: structFieldDecoder.Decode wraps any error present after the field.
                self.decode_conversions(conversions);
                self.wrap_error(FIELD_NAME, b": ");
            } else {
                self.skip();
            }
            if self.is_object_end() {
                break;
            }
        }
        self.wrap_error(STRUCT_TYPE_NAME, b".");
        self.decrement_depth();
    }

    /// Go: `mapDecoder.Decode` for `map[string]map[string]float64`.
    fn decode_conversions(&mut self, map: &mut Option<ConversionRates>) {
        let c = self.next_token();
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            *map = None;
            return;
        }
        let entries = map.get_or_insert_with(BTreeMap::new);
        if c != b'{' {
            self.report_error("ReadMapCB", &concat(&[b"expect { or n, but found ", &[c]]));
            return;
        }
        if self.next_token() == b'}' {
            return;
        }
        self.unread_byte();
        loop {
            let key = self.read_string();
            let separator = self.next_token();
            if separator != b':' {
                self.report_error(
                    "ReadMapCB",
                    &concat(&[b"expect : after object field, but found ", &[separator]]),
                );
                return;
            }
            // Go: every element is decoded into a fresh (nil) inner map.
            let mut inner = None;
            self.decode_inner_map(&mut inner);
            drop(entries.insert(go_string(&key), inner));
            let next = self.next_token();
            if next != b',' {
                if next != b'}' {
                    self.report_error("ReadMapCB", &concat(&[b"expect }, but found ", &[next]]));
                }
                return;
            }
        }
    }

    /// Go: `mapDecoder.Decode` for `map[string]float64`.
    fn decode_inner_map(&mut self, map: &mut Option<BTreeMap<String, f64>>) {
        let c = self.next_token();
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            *map = None;
            return;
        }
        let entries = map.get_or_insert_with(BTreeMap::new);
        if c != b'{' {
            self.report_error("ReadMapCB", &concat(&[b"expect { or n, but found ", &[c]]));
            return;
        }
        if self.next_token() == b'}' {
            return;
        }
        self.unread_byte();
        loop {
            let key = self.read_string();
            let separator = self.next_token();
            if separator != b':' {
                self.report_error(
                    "ReadMapCB",
                    &concat(&[b"expect : after object field, but found ", &[separator]]),
                );
                return;
            }
            // Go: float64Codec.Decode leaves the fresh zero value for null.
            let value = if self.read_nil() {
                0.0
            } else {
                self.read_float64()
            };
            let _previous = entries.insert(go_string(&key), value);
            let next = self.next_token();
            if next != b',' {
                if next != b'}' {
                    self.report_error("ReadMapCB", &concat(&[b"expect }, but found ", &[next]]));
                }
                return;
            }
        }
    }

    // ---- objects ----

    /// Go: `readObjectStart`.
    fn read_object_start(&mut self) -> bool {
        let c = self.next_token();
        if c == b'{' {
            if self.next_token() == b'}' {
                return false;
            }
            self.unread_byte();
            return true;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return false;
        }
        self.report_error(
            "readObjectStart",
            &concat(&[b"expect { or n, but found ", &[c]]),
        );
        false
    }

    /// Go: `readFieldHash` with `caseSensitive` false.
    fn read_field_hash(&mut self) -> i64 {
        let quote = self.next_token();
        if quote != b'"' {
            self.report_error(
                "readFieldHash",
                &concat(&[b"expect \", but found ", &[quote]]),
            );
            return 0;
        }
        let mut hash = FNV_OFFSET;
        let mut index = self.head;
        while index < self.tail() {
            let b = self.byte_at(index);
            if b == b'\\' {
                self.head = index;
                // Go ranges over the decoded string, so the rest is hashed rune by rune.
                let rest = self.read_string_slow_path();
                hash = GoRunes(&rest).fold(hash, |hash, rune| fnv_step(hash, fold_rune(rune)));
                return self.finish_field_hash(hash);
            }
            if b == b'"' {
                self.head = index + 1;
                return self.finish_field_hash(hash);
            }
            hash = fnv_step(hash, u32::from(b.to_ascii_lowercase()));
            index += 1;
        }
        self.load_more();
        self.report_error("readFieldHash", b"incomplete field name");
        0
    }

    fn finish_field_hash(&mut self, hash: i64) -> i64 {
        let colon = self.next_token();
        if colon != b':' {
            self.report_error(
                "readFieldHash",
                &concat(&[b"expect :, but found ", &[colon]]),
            );
            return 0;
        }
        hash
    }

    /// Go: `isObjectEnd`.
    fn is_object_end(&mut self) -> bool {
        let c = self.next_token();
        if c == b',' {
            return false;
        }
        if c == b'}' {
            return true;
        }
        self.report_error(
            "isObjectEnd",
            &concat(&[b"object ended prematurely, unexpected char ", &[c]]),
        );
        true
    }

    /// Go: `ReadNil`.
    fn read_nil(&mut self) -> bool {
        if self.next_token() == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return true;
        }
        self.unread_byte();
        false
    }

    fn increment_depth(&mut self) -> bool {
        self.depth += 1;
        if self.depth <= MAX_DEPTH {
            return true;
        }
        self.report_error("incrementDepth", b"exceeded max depth");
        false
    }

    fn decrement_depth(&mut self) {
        self.depth -= 1;
        if self.depth < 0 {
            self.report_error("decrementDepth", b"unexpected negative nesting");
        }
    }

    // ---- strings ----

    /// Go: `ReadString` (also used by `stringCodec` for map keys, so `null` is the key "").
    fn read_string(&mut self) -> Vec<u8> {
        let c = self.next_token();
        if c == b'"' {
            let start = self.head;
            let mut index = start;
            while index < self.tail() {
                let b = self.byte_at(index);
                if b == b'"' {
                    self.head = index + 1;
                    return self.buf.get(start..index).unwrap_or_default().to_vec();
                }
                if b == b'\\' {
                    break;
                }
                if b < b' ' {
                    let message = format!("invalid control character found: {b}");
                    self.report_error("ReadString", message.as_bytes());
                    return Vec::new();
                }
                index += 1;
            }
            return self.read_string_slow_path();
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return Vec::new();
        }
        self.report_error(
            "ReadString",
            &concat(&[b"expects \" or n, but found ", &[c]]),
        );
        Vec::new()
    }

    /// Go: `readStringSlowPath`, which does not reject control characters.
    fn read_string_slow_path(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while self.error.is_none() {
            let c = self.read_byte();
            if c == b'"' {
                return out;
            }
            if c == b'\\' {
                let escaped = self.read_byte();
                out = self.read_escaped_char(escaped, out);
            } else {
                out.push(c);
            }
        }
        self.report_error("readStringSlowPath", b"unexpected end of input");
        Vec::new()
    }

    /// Go: `readEscapedChar`.
    fn read_escaped_char(&mut self, c: u8, mut out: Vec<u8>) -> Vec<u8> {
        let simple = match c {
            b'u' => return self.read_unicode_escape(out),
            b'"' => b'"',
            b'\\' => b'\\',
            b'/' => b'/',
            b'b' => 0x08,
            b'f' => 0x0C,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            _ => {
                self.report_error("readEscapedChar", b"invalid escape char after \\");
                return Vec::new();
            }
        };
        out.push(simple);
        out
    }

    /// The `\u` branch of Go's `readEscapedChar`, including surrogate pairs.
    fn read_unicode_escape(&mut self, mut out: Vec<u8>) -> Vec<u8> {
        let first = self.read_u4();
        if !(0xD800..0xE000).contains(&first) {
            append_rune(&mut out, first);
            return out;
        }
        let backslash = self.read_byte();
        if self.error.is_some() {
            return Vec::new();
        }
        if backslash != b'\\' {
            self.unread_byte();
            append_rune(&mut out, first);
            return out;
        }
        let u = self.read_byte();
        if self.error.is_some() {
            return Vec::new();
        }
        if u != b'u' {
            append_rune(&mut out, first);
            return self.read_escaped_char(u, out);
        }
        let second = self.read_u4();
        if self.error.is_some() {
            return Vec::new();
        }
        // Go: utf16.DecodeRune
        if (0xD800..0xDC00).contains(&first) && (0xDC00..0xE000).contains(&second) {
            append_rune(
                &mut out,
                (((first - 0xD800) << 10) | (second - 0xDC00)) + 0x1_0000,
            );
        } else {
            append_rune(&mut out, first);
            append_rune(&mut out, second);
        }
        out
    }

    /// Go: `readU4`.
    fn read_u4(&mut self) -> u32 {
        let mut value = 0_u32;
        for _ in 0..4 {
            let c = self.read_byte();
            if self.error.is_some() {
                return value;
            }
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => {
                    self.report_error(
                        "readU4",
                        &concat(&[b"expects 0~9 or a~f, but found ", &[c]]),
                    );
                    return value;
                }
            };
            value = value * 16 + u32::from(digit);
        }
        value
    }

    // ---- skipping (Go: iter_skip.go and iter_skip_strict.go) ----

    /// Go: `Skip`, recursive in Go, iterative here so deep nesting cannot overflow the stack.
    fn skip(&mut self) {
        let mut stack = Vec::new();
        loop {
            if let Some(frame) = self.skip_value_start() {
                stack.push(frame);
                continue;
            }
            // The value is complete: close the containers it completed.
            loop {
                let Some(frame) = stack.last().copied() else {
                    return;
                };
                if self.skip_container_continues(frame) {
                    break;
                }
                let _closed = stack.pop();
            }
        }
    }

    /// Starts skipping one value; returns the container frame when its first element follows.
    fn skip_value_start(&mut self) -> Option<SkipFrame> {
        let c = self.next_token();
        match c {
            b'"' => self.skip_string(),
            b'n' => self.skip_three_bytes(b'u', b'l', b'l'),
            b't' => self.skip_three_bytes(b'r', b'u', b'e'),
            b'f' => self.skip_four_bytes(b'a', b'l', b's', b'e'),
            b'0' => {
                self.unread_byte();
                self.read_float32();
            }
            b'-' | b'1'..=b'9' => self.skip_number(),
            b'[' => {
                self.unread_byte();
                return self.open_array().then_some(SkipFrame::Array);
            }
            b'{' => {
                self.unread_byte();
                return self.open_object().then_some(SkipFrame::Object);
            }
            _ => self.report_error("Skip", format!("do not know how to skip: {c}").as_bytes()),
        }
        None
    }

    /// After an element of `frame`: true when another element follows, false when the
    /// container is closed (Go: the loop tails of `ReadArrayCB` and `ReadObjectCB`).
    fn skip_container_continues(&mut self, frame: SkipFrame) -> bool {
        let c = self.next_token();
        match frame {
            SkipFrame::Array => {
                if c == b',' {
                    return true;
                }
                if c != b']' {
                    self.report_error(
                        "ReadArrayCB",
                        &concat(&[b"expect ] in the end, but found ", &[c]]),
                    );
                }
            }
            SkipFrame::Object => {
                if c == b',' {
                    let _field = self.read_string();
                    self.expect_object_colon();
                    return true;
                }
                if c != b'}' {
                    self.report_error("ReadObjectCB", b"object not ended with }");
                }
            }
        }
        self.decrement_depth();
        false
    }

    fn expect_object_colon(&mut self) {
        let colon = self.next_token();
        if colon != b':' {
            self.report_error(
                "ReadObject",
                &concat(&[b"expect : after object field, but found ", &[colon]]),
            );
        }
    }

    /// The start of Go's `ReadArrayCB`; true when an element follows.
    fn open_array(&mut self) -> bool {
        let c = self.next_token();
        if c == b'[' {
            if !self.increment_depth() {
                return false;
            }
            if self.next_token() != b']' {
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
        self.report_error(
            "ReadArrayCB",
            &concat(&[b"expect [ or n, but found ", &[c]]),
        );
        false
    }

    /// The start of Go's `ReadObjectCB`; true when the first field's value follows.
    fn open_object(&mut self) -> bool {
        let c = self.next_token();
        if c == b'{' {
            if !self.increment_depth() {
                return false;
            }
            let first = self.next_token();
            if first == b'"' {
                self.unread_byte();
                let _field = self.read_string();
                self.expect_object_colon();
                return true;
            }
            if first == b'}' {
                self.decrement_depth();
                return false;
            }
            self.report_error(
                "ReadObjectCB",
                &concat(&[b"expect \" after {, but found ", &[first]]),
            );
            self.decrement_depth();
            return false;
        }
        if c == b'n' {
            self.skip_three_bytes(b'u', b'l', b'l');
            return false;
        }
        self.report_error(
            "ReadObjectCB",
            &concat(&[b"expect { or n, but found ", &[c]]),
        );
        false
    }

    fn skip_three_bytes(&mut self, b1: u8, b2: u8, b3: u8) {
        for expected in [b1, b2, b3] {
            if self.read_byte() != expected {
                let message = concat(&[b"expect ", &[b1, b2, b3]]);
                self.report_error("skipThreeBytes", &message);
                return;
            }
        }
    }

    fn skip_four_bytes(&mut self, b1: u8, b2: u8, b3: u8, b4: u8) {
        for expected in [b1, b2, b3, b4] {
            if self.read_byte() != expected {
                let message = concat(&[b"expect ", &[b1, b2, b3, b4]]);
                self.report_error("skipFourBytes", &message);
                return;
            }
        }
    }

    /// Go: `skipString`.
    fn skip_string(&mut self) {
        if !self.try_skip_string() {
            self.unread_byte();
            let _value = self.read_string();
        }
    }

    /// Go: `trySkipString`.
    fn try_skip_string(&mut self) -> bool {
        let mut index = self.head;
        while index < self.tail() {
            let c = self.byte_at(index);
            if c == b'"' {
                self.head = index + 1;
                return true;
            }
            if c == b'\\' {
                return false;
            }
            if c < b' ' {
                let message = format!("invalid control character found: {c}");
                self.report_error("trySkipString", message.as_bytes());
                return true;
            }
            index += 1;
        }
        false
    }

    /// Go: `skipNumber`, which retries a failed float parse as a `big.Float`.
    fn skip_number(&mut self) {
        if self.try_skip_number() {
            return;
        }
        self.unread_byte();
        if self.has_error() {
            return;
        }
        let _value = self.read_float64();
        if self.has_error() {
            self.error = None;
            self.read_big_float();
        }
    }

    /// Go: `trySkipNumber`.
    fn try_skip_number(&mut self) -> bool {
        let mut dot_found = false;
        let mut index = self.head;
        while index < self.tail() {
            let c = self.byte_at(index);
            match c {
                b'0'..=b'9' => {}
                b'.' => {
                    if dot_found {
                        self.report_error("validateNumber", b"more than one dot found in number");
                        return true;
                    }
                    if index + 1 == self.tail() {
                        return false;
                    }
                    if !self.byte_at(index + 1).is_ascii_digit() {
                        self.report_error("validateNumber", b"missing digit after dot");
                        return true;
                    }
                    dot_found = true;
                }
                b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r' => {
                    if self.head == index {
                        return false;
                    }
                    self.head = index;
                    return true;
                }
                _ => return false,
            }
            index += 1;
        }
        false
    }

    // ---- numbers (Go: iter_float.go) ----

    /// Go: `ReadFloat64`.
    fn read_float64(&mut self) -> f64 {
        if self.next_token() == b'-' {
            return -self.read_positive_float(FloatWidth::F64);
        }
        self.unread_byte();
        self.read_positive_float(FloatWidth::F64)
    }

    /// Go: `ReadFloat32`; only reached while skipping, so the value is not needed.
    fn read_float32(&mut self) {
        if self.next_token() != b'-' {
            self.unread_byte();
        }
        let _value = self.read_positive_float(FloatWidth::F32);
    }

    /// Go: `readPositiveFloat64` / `readPositiveFloat32` (their fast paths only differ in the
    /// 2^53 guard and the final rounding).
    fn read_positive_float(&mut self, width: FloatWidth) -> f64 {
        let tail = self.tail();
        let mut index = self.head;
        if index == tail {
            return self.read_float_slow_path(width);
        }
        let first = self.byte_at(index);
        index += 1;
        let mut value = match float_char(first) {
            FloatChar::Invalid => return self.read_float_slow_path(width),
            FloatChar::End => {
                self.report_error(width.fast_operation(), b"empty number");
                return 0.0;
            }
            FloatChar::Dot => {
                self.report_error(width.fast_operation(), b"leading dot is invalid");
                return 0.0;
            }
            FloatChar::Digit(digit) => {
                if digit == 0 {
                    if index == tail {
                        return self.read_float_slow_path(width);
                    }
                    if self.byte_at(index).is_ascii_digit() {
                        self.report_error(width.fast_operation(), b"leading zero is invalid");
                        return 0.0;
                    }
                }
                u64::from(digit)
            }
        };
        // Integer digits.
        let mut last = first;
        while index < tail {
            last = self.byte_at(index);
            match float_char(last) {
                FloatChar::Invalid => return self.read_float_slow_path(width),
                FloatChar::End => {
                    self.head = index;
                    return width.round(u64_to_f64(value));
                }
                FloatChar::Dot => break,
                FloatChar::Digit(digit) => {
                    if value > UINT64_SAFE_TO_MULTIPLE10 {
                        return self.read_float_slow_path(width);
                    }
                    value = value * 10 + u64::from(digit);
                }
            }
            index += 1;
        }
        if last == b'.' {
            return self.read_fraction(width, value, index + 1);
        }
        self.read_float_slow_path(width)
    }

    /// The digits after the dot in Go's fast float path.
    fn read_fraction(&mut self, width: FloatWidth, mut value: u64, mut index: usize) -> f64 {
        let tail = self.tail();
        if index == tail {
            return self.read_float_slow_path(width);
        }
        let mut decimal_places = 0_usize;
        while index < tail {
            match float_char(self.byte_at(index)) {
                FloatChar::End => {
                    if let Some(divisor) = POW10.get(decimal_places).filter(|_| decimal_places > 0)
                    {
                        self.head = index;
                        return width.round(u64_to_f64(value) / f64::from(*divisor));
                    }
                    return self.read_float_slow_path(width);
                }
                FloatChar::Invalid | FloatChar::Dot => return self.read_float_slow_path(width),
                FloatChar::Digit(digit) => {
                    decimal_places += 1;
                    if value > UINT64_SAFE_TO_MULTIPLE10 {
                        return self.read_float_slow_path(width);
                    }
                    value = value * 10 + u64::from(digit);
                    if width == FloatWidth::F64 && value > MAX_FLOAT64 {
                        return self.read_float_slow_path(width);
                    }
                }
            }
            index += 1;
        }
        self.read_float_slow_path(width)
    }

    /// Go: `readFloat64SlowPath` / `readFloat32SlowPath`.
    fn read_float_slow_path(&mut self, width: FloatWidth) -> f64 {
        let text = self.read_number_as_string();
        if self.has_error() {
            return 0.0;
        }
        if let Some(message) = validate_float(&text) {
            self.report_error(width.slow_operation(), message.as_bytes());
            return 0.0;
        }
        match parse_float(&text, width) {
            Ok(value) => value,
            Err(message) => {
                // Go assigns iter.Error directly, bypassing ReportError.
                self.error = Some(IterError::Message(message));
                0.0
            }
        }
    }

    /// Go: `readNumberAsString`.
    fn read_number_as_string(&mut self) -> Vec<u8> {
        let start = self.head;
        let rest = self.buf.get(start..).unwrap_or_default();
        let length = rest
            .iter()
            .position(|c| !matches!(c, b'+' | b'-' | b'.' | b'e' | b'E' | b'0'..=b'9'));
        let text = rest
            .get(..length.unwrap_or(rest.len()))
            .unwrap_or_default()
            .to_vec();
        match length {
            Some(length) => self.head = start + length,
            None => {
                self.load_more();
            }
        }
        if self.has_error() {
            return Vec::new();
        }
        if text.is_empty() {
            self.report_error("readNumberAsString", b"invalid number");
        }
        text
    }

    /// Go: `ReadBigFloat`, whose value is discarded by `skipNumber`.
    fn read_big_float(&mut self) {
        let text = self.read_number_as_string();
        if self.has_error() {
            return;
        }
        if let Err(message) = big_float_parse(&text) {
            self.error = Some(IterError::Message(message));
        }
    }
}

/// Which of jsoniter's two float readers is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatWidth {
    F32,
    F64,
}

impl FloatWidth {
    const fn fast_operation(self) -> &'static str {
        match self {
            Self::F32 => "readFloat32",
            Self::F64 => "readFloat64",
        }
    }

    const fn slow_operation(self) -> &'static str {
        match self {
            Self::F32 => "readFloat32SlowPath",
            Self::F64 => "readFloat64SlowPath",
        }
    }

    /// The float32 reader's results are never used, so only the float64 rounding matters.
    const fn round(self, value: f64) -> f64 {
        match self {
            Self::F32 | Self::F64 => value,
        }
    }
}

/// Go: jsoniter `validateFloat`.
fn validate_float(text: &[u8]) -> Option<&'static str> {
    let first = text.first()?;
    if *first == b'-' {
        return Some("-- is not valid");
    }
    let dot = text.iter().position(|c| *c == b'.')?;
    match text.get(dot + 1) {
        None => Some("dot can not be last character"),
        Some(next) if next.is_ascii_digit() => None,
        Some(_) => Some("missing digit after dot"),
    }
}

/// Go: `strconv.ParseFloat(text, 64 or 32)` for the `[+-.eE0-9]` strings jsoniter collects.
///
/// On that alphabet Go and Rust accept the same grammar and both round correctly; Go reports
/// overflow to infinity as a range error, underflow is not an error in either.
fn parse_float(text: &[u8], width: FloatWidth) -> Result<f64, Vec<u8>> {
    let syntax = || num_error("ParseFloat", text, "invalid syntax");
    let range = || num_error("ParseFloat", text, "value out of range");
    let string = str::from_utf8(text).map_err(|_utf8| syntax())?;
    let value = match width {
        FloatWidth::F64 => string.parse::<f64>().map_err(|_syntax| syntax())?,
        FloatWidth::F32 => f64::from(string.parse::<f32>().map_err(|_syntax| syntax())?),
    };
    if value.is_infinite() {
        Err(range())
    } else {
        Ok(value)
    }
}

/// Go: `strconv.NumError.Error()` for an ASCII printable input.
fn num_error(function: &str, input: &[u8], reason: &str) -> Vec<u8> {
    concat(&[
        b"strconv.",
        function.as_bytes(),
        b": parsing \"",
        input,
        b"\": ",
        reason.as_bytes(),
    ])
}

/// Go: `big.ParseFloat(text, 10, prec, big.ToZero)` reduced to its error, for strings over
/// `[+-.eE0-9]` (`Float.Parse` → `scan` → `nat.scan` / `scanExponent`).
fn big_float_parse(text: &[u8]) -> Result<(), Vec<u8>> {
    if text.is_empty() {
        // Go: scanSign reads io.EOF.
        return Err(b"EOF".to_vec());
    }
    let mut position = usize::from(matches!(text.first(), Some(b'+' | b'-')));
    // Mantissa: digits with at most one '.'.
    let mut digits = Vec::new();
    let mut seen_dot = false;
    let mut fraction_digits = 0_i64;
    while let Some(&c) = text.get(position) {
        if c == b'.' && !seen_dot {
            seen_dot = true;
        } else if c.is_ascii_digit() {
            digits.push(c);
            if seen_dot {
                fraction_digits += 1;
            }
        } else {
            break;
        }
        position += 1;
    }
    if digits.is_empty() {
        return Err(b"number has no digits".to_vec());
    }
    let exponent = big_float_exponent(text, &mut position)?;
    let bit_length = decimal_bit_length(&digits);
    if bit_length > 0 {
        let exp2 = i128::from(bit_length) - i128::from(fraction_digits) + i128::from(exponent);
        if exp2 < i128::from(i32::MIN) || exp2 > i128::from(i32::MAX) {
            return Err(b"exponent overflow".to_vec());
        }
    }
    match text.get(position) {
        Some(&c) => Err(concat(&[b"expected end of string, found '", &[c], b"'"])),
        None => Ok(()),
    }
}

/// Go: `scanExponent(r, true, false)` for base 10.
fn big_float_exponent(text: &[u8], position: &mut usize) -> Result<i64, Vec<u8>> {
    if !matches!(text.get(*position), Some(b'e' | b'E')) {
        return Ok(0);
    }
    *position += 1;
    let mut literal = Vec::new();
    if let Some(&sign @ (b'+' | b'-')) = text.get(*position) {
        if sign == b'-' {
            literal.push(b'-');
        }
        *position += 1;
    }
    let digits_start = literal.len();
    while let Some(&c) = text.get(*position).filter(|c| c.is_ascii_digit()) {
        literal.push(c);
        *position += 1;
    }
    if literal.len() == digits_start {
        return Err(b"number has no digits".to_vec());
    }
    parse_int64(&literal).ok_or_else(|| num_error("ParseInt", &literal, "value out of range"))
}

/// Go: `strconv.ParseInt(s, 10, 64)` for an optional '-' followed by digits; `None` on range.
fn parse_int64(literal: &[u8]) -> Option<i64> {
    let (negative, digits) = match literal.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, literal),
    };
    let magnitude = digits.iter().try_fold(0_i128, |value, digit| {
        let next = value * 10 + i128::from(digit - b'0');
        (next <= i128::from(i64::MAX) + 1).then_some(next)
    })?;
    i64::try_from(if negative { -magnitude } else { magnitude }).ok()
}

/// The bit length of the decimal integer `digits` (0 for zero), as `big.nat` computes it.
fn decimal_bit_length(digits: &[u8]) -> u64 {
    let significant = digits
        .iter()
        .position(|d| *d != b'0')
        .map_or(&[][..], |first| digits.get(first..).unwrap_or_default());
    // Little-endian base-2^64 limbs, fed 19 decimal digits at a time like nat.scan.
    let mut limbs: Vec<u64> = Vec::new();
    for chunk in significant.chunks(19) {
        let mut multiplier = 1_u128;
        let mut addend = 0_u128;
        for digit in chunk {
            multiplier *= 10;
            addend = addend * 10 + u128::from(digit - b'0');
        }
        let mut carry = addend;
        for limb in &mut limbs {
            let product = u128::from(*limb) * multiplier + carry;
            *limb = low_u64(product);
            carry = product >> 64;
        }
        if carry > 0 {
            limbs.push(low_u64(carry));
        }
    }
    let Some(top) = limbs.last() else {
        return 0;
    };
    let full_limbs = u64::try_from(limbs.len() - 1).unwrap_or(u64::MAX);
    full_limbs.saturating_mul(64) + u64::from(64 - top.leading_zeros())
}

fn low_u64(value: u128) -> u64 {
    u64::try_from(value & u128::from(u64::MAX)).unwrap_or(u64::MAX)
}

/// Go: `float64(value)` for a `uint64` (round to nearest, ties to even) without a lossy cast.
fn u64_to_f64(value: u64) -> f64 {
    let high = u32::try_from(value >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & 0xFFFF_FFFF).unwrap_or(u32::MAX);
    // high * 2^32 is exact, so the single rounding of the fused add is the correct one.
    f64::from(high).mul_add(4_294_967_296.0, f64::from(low))
}

/// Go: `appendRune` from jsoniter (surrogates and out-of-range values become U+FFFD).
fn append_rune(out: &mut Vec<u8>, rune: u32) {
    let ch = char::from_u32(rune).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut buffer = [0_u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

const FNV_OFFSET: i64 = 0x811C_9DC5;

/// One step of jsoniter's case-folding field hash (`hash ^= b; hash *= 0x1000193` on int64).
fn fnv_step(hash: i64, value: u32) -> i64 {
    (hash ^ i64::from(value)).wrapping_mul(0x0100_0193)
}

/// Go: `calcHash(name, false)`.
fn calc_hash(name: &[u8]) -> i64 {
    name.iter().fold(FNV_OFFSET, |hash, byte| {
        fnv_step(hash, u32::from(byte.to_ascii_lowercase()))
    })
}

/// Case folding of `readFieldHash`'s escape path, applied to runes.
fn fold_rune(rune: u32) -> u32 {
    if (u32::from(b'A')..=u32::from(b'Z')).contains(&rune) {
        rune + 32
    } else {
        rune
    }
}

/// Go's `for _, r := range s` over bytes: an invalid byte yields U+FFFD and advances by one.
#[derive(Debug)]
struct GoRunes<'a>(&'a [u8]);

impl Iterator for GoRunes<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let (rune, size) = decode_rune(self.0)?;
        self.0 = self.0.get(size..).unwrap_or_default();
        Some(rune)
    }
}

/// Go: `utf8.DecodeRune`, `None` for empty input.
fn decode_rune(bytes: &[u8]) -> Option<(u32, usize)> {
    let first = *bytes.first()?;
    let width = match first {
        0x00..=0x7F => return Some((u32::from(first), 1)),
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return Some((RUNE_ERROR, 1)),
    };
    let decoded = bytes
        .get(..width)
        .and_then(|sequence| str::from_utf8(sequence).ok())
        .and_then(|text| text.chars().next());
    Some(decoded.map_or((RUNE_ERROR, 1), |ch| (u32::from(ch), width)))
}

/// Converts a Go string to a Rust `String`, replacing each invalid byte with U+FFFD.
fn go_string(bytes: &[u8]) -> String {
    match str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_invalid) => GoRunes(bytes)
            .map(|rune| char::from_u32(rune).unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect(),
    }
}

/// Go: `jsonutil.tryExtractErrorMessage`.
fn try_extract_error_message(message: &[u8]) -> Vec<u8> {
    let Some(end) = rfind(message, b", error found in #") else {
        return message.to_vec();
    };
    let Some(mut start) = find(message, b": ") else {
        return message.to_vec();
    };
    let mut operations = vec![message.get(..start).unwrap_or_default()];
    loop {
        let rest = message.get(start + 2..).unwrap_or_default();
        let Some(next) = find(rest, b": ") else {
            break;
        };
        if start + next >= end {
            break;
        }
        let operation = rest.get(..next).unwrap_or_default();
        if operation.contains(&b' ') {
            break;
        }
        operations.push(operation);
        start += next + 2;
    }
    let detail = message.get(start + 2..end).unwrap_or_default();
    let remainder = message.get(start + 2..).unwrap_or_default();
    if operations.len() > 1 && !remainder.starts_with(b"request.") {
        let operation = operations
            .len()
            .checked_sub(2)
            .and_then(|index| operations.get(index));
        return concat(&[
            b"cannot unmarshal ",
            operation.copied().unwrap_or_default(),
            b": ",
            detail,
        ]);
    }
    detail.to_vec()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fmt::{self, Write as _},
    };

    use super::{try_extract_error_message, unmarshal, unmarshal_valid};
    use crate::{
        golden::{
            TestResult, check, check_eq, conversion_rates, fnv64a_hex, hex_decode, hex_encode,
            read_golden,
        },
        rates::{ConversionRates, Rates},
    };

    /// Renders a decoded conversions map like the Go generator's `dumpConversions`.
    fn dump(conversions: Option<&ConversionRates>) -> Result<String, fmt::Error> {
        let Some(conversions) = conversions else {
            return Ok("nil".to_owned());
        };
        let mut out = String::from("{");
        for (key, inner) in conversions {
            out.push_str(&hex_encode(key.as_bytes()));
            out.push(':');
            match inner {
                None => out.push_str("nil;"),
                Some(inner) => {
                    out.push('{');
                    for (to, rate) in inner {
                        out.push_str(&hex_encode(to.as_bytes()));
                        out.push('=');
                        write!(out, "{:016x},", rate.to_bits())?;
                    }
                    out.push_str("};");
                }
            }
        }
        out.push('}');
        Ok(out)
    }

    /// Renders a decode outcome like the Go generator's `decodeResult`.
    fn render(data: &[u8]) -> Result<Vec<u8>, fmt::Error> {
        Ok(match unmarshal(data) {
            Ok(rates) => format!("ok:{}", dump(rates.conversions.as_ref())?).into_bytes(),
            Err(raw) => {
                let mut out = b"err:".to_vec();
                out.extend_from_slice(&raw);
                out.push(0x1F);
                out.extend_from_slice(&try_extract_error_message(&raw));
                out
            }
        })
    }

    #[test]
    fn unmarshal_rates() -> TestResult {
        // Go: TestUnMarshallRates
        let err = unmarshal_valid(b"malformed").err().unwrap_or_default();
        check_eq(
            &err,
            &b"expect { or n, but found m".to_vec(),
            "malformed JSON object",
        )?;

        let valid = br#"{
				"conversions":{
					"USD":{
						"GBP":0.7662523901
					},
					"GBP":{
						"USD":1.3050530256
					}
				}
			}"#;
        let expected = conversion_rates(&[
            ("USD", &[("GBP", 0.766_252_390_1)]),
            ("GBP", &[("USD", 1.305_053_025_6)]),
        ]);
        check_eq(
            &unmarshal_valid(valid),
            &Ok(Rates::new(Some(expected))),
            "valid conversions",
        )?;

        let repeated = br#"{
				"conversions":{
					"USD":{
						"GBP":0.7662523901,
						"MXN":20.00
					},
					"USD":{
						"GBP":0.4815162342
					}
				}
			}"#;
        let last = conversion_rates(&[("USD", &[("GBP", 0.481_516_234_2)])]);
        check_eq(
            &unmarshal_valid(repeated),
            &Ok(Rates::new(Some(last))),
            "last one wins",
        )
    }

    #[test]
    fn go_golden_explicit_documents() -> TestResult {
        let mut checked = 0_usize;
        for fields in read_golden("rates_json.golden")? {
            if let [kind, input, expected] = fields.as_slice()
                && kind == "json"
            {
                let input = hex_decode(input)?;
                let actual = render(&input)?;
                let expected = hex_decode(expected)?;
                check(
                    actual == expected,
                    &format!(
                        "decoding {:?}: got {:?}, want {:?}",
                        String::from_utf8_lossy(&input),
                        String::from_utf8_lossy(&actual),
                        String::from_utf8_lossy(&expected),
                    ),
                )?;
                checked += 1;
            }
        }
        check(checked > 150, "the golden must hold the explicit documents")
    }

    #[test]
    fn go_golden_mutations() -> TestResult {
        let mut alphabet = Vec::new();
        let mut bases: Vec<Vec<u8>> = Vec::new();
        let mut checked = 0_usize;
        for fields in read_golden("rates_json.golden")? {
            match fields.as_slice() {
                [kind, hex] if kind == "alphabet" => alphabet = hex_decode(hex)?,
                [kind, _index, hex] if kind == "base" => bases.push(hex_decode(hex)?),
                [kind, base, mutation, position, count, digest] if kind == "mut" => {
                    let base = bases.get(base.parse::<usize>()?).ok_or("unknown base")?;
                    let position = position.parse::<usize>()?;
                    let inputs = mutations(base, &alphabet, mutation, position)?;
                    let mut rendered = Vec::new();
                    for input in &inputs {
                        rendered.extend_from_slice(&render(input)?);
                        rendered.push(b'\n');
                    }
                    let context = format!(
                        "{mutation} at {position} of {:?}",
                        String::from_utf8_lossy(base)
                    );
                    check_eq(&inputs.len().to_string(), count, &context)?;
                    check_eq(&fnv64a_hex(&rendered), digest, &context)?;
                    checked += 1;
                }
                _ => {}
            }
        }
        check(checked > 600, "the golden must hold the mutation digests")
    }

    fn mutations(
        base: &[u8],
        alphabet: &[u8],
        mutation: &str,
        position: usize,
    ) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        let before = base.get(..position).ok_or("position out of range")?;
        let at_and_after = base.get(position..).ok_or("position out of range")?;
        let after = base.get(position + 1..).unwrap_or_default();
        Ok(match mutation {
            "trunc" => vec![before.to_vec()],
            "repl" => alphabet
                .iter()
                .map(|c| [before, &[*c], after].concat())
                .collect(),
            "ins" => alphabet
                .iter()
                .map(|c| [before, &[*c], at_and_after].concat())
                .collect(),
            "del" => vec![[before, after].concat()],
            _ => return Err(format!("unknown mutation {mutation}").into()),
        })
    }

    #[test]
    fn go_golden_deep_nesting() -> TestResult {
        let mut checked = 0_usize;
        for fields in read_golden("rates_json.golden")? {
            if let [kind, shape, depth, closed, expected] = fields.as_slice()
                && kind == "deep"
            {
                let (open, close): (&[u8], &[u8]) = if shape == "array" {
                    (b"[", b"]")
                } else {
                    (b"{\"a\":", b"}")
                };
                let depth = depth.parse::<usize>()?;
                let mut document = b"{\"x\":".to_vec();
                document.extend(open.repeat(depth));
                if closed == "true" {
                    document.push(b'1');
                    document.extend(close.repeat(depth));
                    document.push(b'}');
                }
                check_eq(
                    &hex_encode(&render(&document)?),
                    expected,
                    &format!("{shape} nested {depth} deep, closed: {closed}"),
                )?;
                checked += 1;
            }
        }
        check(checked == 16, "the golden must hold the nesting cases")
    }
}
