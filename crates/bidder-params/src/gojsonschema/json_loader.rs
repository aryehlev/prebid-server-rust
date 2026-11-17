//! Go: gojsonschema/jsonLoader.go `decodeJSONUsingNumber`.
//!
//! gojsonschema decodes every document (schemas and instances) with an `encoding/json` `Decoder`
//! in `UseNumber` mode into `interface{}`. This module reproduces that exactly:
//!
//! - the scanner state machine of `encoding/json/scanner.go`, so syntax errors carry Go's text;
//! - `Decoder.readValue`: only the first value is read and trailing bytes are ignored, an empty
//!   input is `EOF` and a truncated one is `unexpected EOF`;
//! - string unquoting of `encoding/json/decode.go` (`unquoteBytes`), including the per-byte U+FFFD
//!   replacement of invalid UTF-8 and of unpaired surrogate escapes;
//! - numbers are kept as their literal text (`json.Number`);
//! - objects behave like `map[string]interface{}`: a repeated key keeps its first position and
//!   takes the last value.
//!
//! Decoded values live in an arena ([`Document`]) so that deeply nested input (Go allows 10000
//! levels) is neither built nor dropped recursively.

/// Go: encoding/json/scanner.go `maxNestingDepth`.
const MAX_NESTING_DEPTH: usize = 10000;

/// Index of a node inside a [`Document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(usize);

/// One decoded JSON value, as Go's `interface{}` holds it after `Decoder.UseNumber`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// `nil`.
    Null,
    /// `bool`.
    Bool(bool),
    /// `json.Number`: the literal text of the number.
    Number(String),
    /// `string`.
    String(String),
    /// `[]interface{}`.
    Array(Vec<NodeId>),
    /// `map[string]interface{}`: unique keys in first-insertion order.
    Object(Vec<(String, NodeId)>),
}

/// A decoded JSON document stored as an arena of [`Node`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    nodes: Vec<Node>,
    root: NodeId,
}

static NULL_NODE: Node = Node::Null;

impl Document {
    /// The root value.
    #[must_use]
    pub const fn root(&self) -> NodeId {
        self.root
    }

    /// The node behind `id`. Ids always come from this document; an unknown id reads as null.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes.get(id.0).unwrap_or(&NULL_NODE)
    }

    /// Looks `key` up in the object `id` (Go: `m[key]` on `map[string]interface{}`).
    #[must_use]
    pub fn get(&self, id: NodeId, key: &str) -> Option<NodeId> {
        match self.node(id) {
            Node::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|&(_, v)| v),
            Node::Null | Node::Bool(_) | Node::Number(_) | Node::String(_) | Node::Array(_) => None,
        }
    }

    /// Builds a document holding the single string `s` (used for `propertyNames`).
    pub(super) fn from_string(s: &str) -> Self {
        Self {
            nodes: vec![Node::String(s.to_owned())],
            root: NodeId(0),
        }
    }
}

/// Go: gojsonschema `decodeJSONUsingNumber` (`json.NewDecoder(r)`, `UseNumber`, `Decode`).
///
/// # Errors
///
/// Returns Go's `err.Error()` text: `EOF`, `unexpected EOF`, or a scanner syntax error such as
/// `invalid character '}' looking for beginning of object key string`.
pub fn decode_json_using_number(input: &[u8]) -> Result<Document, String> {
    let end = read_value(input)?;
    let value = input.get(..end).unwrap_or_default();
    Ok(build(value))
}

/// Go: encoding/json/stream.go `Decoder.readValue`: returns the length of the first value.
fn read_value(input: &[u8]) -> Result<usize, String> {
    let mut scan = Scanner::new();
    for (i, &c) in input.iter().enumerate() {
        match scan.step(c) {
            Scan::End => return Ok(i),
            Scan::EndObject | Scan::EndArray => {
                if scan.state_end_value(b' ') == Scan::End {
                    return Ok(i + 1);
                }
            }
            Scan::Error => return Err(scan.err),
            Scan::Continue | Scan::BeginLiteral | Scan::SkipSpace | Scan::Other => {}
        }
    }
    if scan.step(b' ') == Scan::End {
        return Ok(input.len());
    }
    if input.iter().any(|&c| !is_space(c)) {
        Err("unexpected EOF".to_owned())
    } else {
        Err("EOF".to_owned())
    }
}

/// Scanner result codes (Go: encoding/json/scanner.go `scan*` constants). Only the codes the
/// decoder distinguishes are kept apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Continue,
    BeginLiteral,
    SkipSpace,
    EndObject,
    EndArray,
    End,
    Error,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

/// Scanner step functions (Go: encoding/json/scanner.go `state*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    BeginValue,
    BeginValueOrEmpty,
    BeginStringOrEmpty,
    BeginString,
    EndValue,
    EndTop,
    InString,
    InStringEsc,
    InStringEscU(u8),
    Neg,
    One,
    Zero,
    Dot,
    Dot0,
    E,
    ESign,
    E0,
    /// Inside `true`, `false` or `null`, expecting the byte at the index.
    Literal(Word, u8),
    Error,
}

/// The JSON literal words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Word {
    True,
    False,
    Null,
}

impl Word {
    const fn text(self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::Null => "null",
        }
    }
}

/// Go: encoding/json/scanner.go `scanner`.
#[derive(Debug)]
struct Scanner {
    step: State,
    end_top: bool,
    parse_state: Vec<ParseState>,
    err: String,
}

const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

const fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

impl Scanner {
    const fn new() -> Self {
        Self {
            step: State::BeginValue,
            end_top: false,
            parse_state: Vec::new(),
            err: String::new(),
        }
    }

    fn step(&mut self, c: u8) -> Scan {
        match self.step {
            State::BeginValue => self.state_begin_value(c),
            State::BeginValueOrEmpty => {
                if is_space(c) {
                    Scan::SkipSpace
                } else if c == b']' {
                    self.state_end_value(c)
                } else {
                    self.state_begin_value(c)
                }
            }
            State::BeginStringOrEmpty => self.state_begin_string_or_empty(c),
            State::BeginString => self.state_begin_string(c),
            State::EndValue => self.state_end_value(c),
            State::EndTop => self.state_end_top(c),
            State::InString => self.state_in_string(c),
            State::InStringEsc => self.state_in_string_esc(c),
            State::InStringEscU(n) => self.state_in_string_esc_u(c, n),
            State::Neg | State::One | State::Zero | State::Dot | State::Dot0 => self.step_number(c),
            State::E | State::ESign | State::E0 => self.step_exponent(c),
            State::Literal(word, index) => self.state_literal(c, word, index),
            State::Error => Scan::Error,
        }
    }

    fn error(&mut self, c: u8, context: &str) -> Scan {
        self.step = State::Error;
        self.err = format!("invalid character {} {context}", quote_char(c));
        Scan::Error
    }

    fn push_parse_state(&mut self, c: u8, state: ParseState, success: Scan) -> Scan {
        self.parse_state.push(state);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            success
        } else {
            self.error(c, "exceeded max depth")
        }
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

    fn state_begin_value(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        match c {
            b'{' => {
                self.step = State::BeginStringOrEmpty;
                self.push_parse_state(c, ParseState::ObjectKey, Scan::Other)
            }
            b'[' => {
                self.step = State::BeginValueOrEmpty;
                self.push_parse_state(c, ParseState::ArrayValue, Scan::Other)
            }
            b'"' => self.begin_literal(State::InString),
            b'-' => self.begin_literal(State::Neg),
            b'0' => self.begin_literal(State::Zero),
            b't' => self.begin_literal(State::Literal(Word::True, 1)),
            b'f' => self.begin_literal(State::Literal(Word::False, 1)),
            b'n' => self.begin_literal(State::Literal(Word::Null, 1)),
            b'1'..=b'9' => self.begin_literal(State::One),
            _ => self.error(c, "looking for beginning of value"),
        }
    }

    const fn begin_literal(&mut self, next: State) -> Scan {
        self.step = next;
        Scan::BeginLiteral
    }

    fn state_begin_string_or_empty(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        if c == b'}' {
            if let Some(last) = self.parse_state.last_mut() {
                *last = ParseState::ObjectValue;
            }
            return self.state_end_value(c);
        }
        self.state_begin_string(c)
    }

    fn state_begin_string(&mut self, c: u8) -> Scan {
        if is_space(c) {
            return Scan::SkipSpace;
        }
        if c == b'"' {
            self.step = State::InString;
            return Scan::BeginLiteral;
        }
        self.error(c, "looking for beginning of object key string")
    }

    fn state_end_value(&mut self, c: u8) -> Scan {
        let Some(&ps) = self.parse_state.last() else {
            self.step = State::EndTop;
            self.end_top = true;
            return self.state_end_top(c);
        };
        if is_space(c) {
            self.step = State::EndValue;
            return Scan::SkipSpace;
        }
        match ps {
            ParseState::ObjectKey => {
                if c == b':' {
                    self.set_top(ParseState::ObjectValue);
                    self.step = State::BeginValue;
                    return Scan::Other;
                }
                self.error(c, "after object key")
            }
            ParseState::ObjectValue => {
                if c == b',' {
                    self.set_top(ParseState::ObjectKey);
                    self.step = State::BeginString;
                    return Scan::Other;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Scan::EndObject;
                }
                self.error(c, "after object key:value pair")
            }
            ParseState::ArrayValue => {
                if c == b',' {
                    self.step = State::BeginValue;
                    return Scan::Other;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Scan::EndArray;
                }
                self.error(c, "after array element")
            }
        }
    }

    fn set_top(&mut self, state: ParseState) {
        if let Some(last) = self.parse_state.last_mut() {
            *last = state;
        }
    }

    fn state_end_top(&mut self, c: u8) -> Scan {
        if !is_space(c) {
            // Complain about non-space byte on next call.
            let _code = self.error(c, "after top-level value");
        }
        Scan::End
    }

    fn state_in_string(&mut self, c: u8) -> Scan {
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

    fn state_in_string_esc(&mut self, c: u8) -> Scan {
        match c {
            b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => {
                self.step = State::InString;
                Scan::Continue
            }
            b'u' => {
                self.step = State::InStringEscU(0);
                Scan::Continue
            }
            _ => self.error(c, "in string escape code"),
        }
    }

    fn state_in_string_esc_u(&mut self, c: u8, seen: u8) -> Scan {
        if !is_hex(c) {
            return self.error(c, "in \\u hexadecimal character escape");
        }
        self.step = if seen >= 3 {
            State::InString
        } else {
            State::InStringEscU(seen + 1)
        };
        Scan::Continue
    }

    fn step_number(&mut self, c: u8) -> Scan {
        match self.step {
            State::Neg => match c {
                b'0' => self.cont(State::Zero),
                b'1'..=b'9' => self.cont(State::One),
                _ => self.error(c, "in numeric literal"),
            },
            State::One if c.is_ascii_digit() => self.cont(State::One),
            State::One | State::Zero => match c {
                b'.' => self.cont(State::Dot),
                b'e' | b'E' => self.cont(State::E),
                _ => self.state_end_value(c),
            },
            State::Dot => {
                if c.is_ascii_digit() {
                    self.cont(State::Dot0)
                } else {
                    self.error(c, "after decimal point in numeric literal")
                }
            }
            State::Dot0 => match c {
                b'0'..=b'9' => Scan::Continue,
                b'e' | b'E' => self.cont(State::E),
                _ => self.state_end_value(c),
            },
            State::BeginValue
            | State::BeginValueOrEmpty
            | State::BeginStringOrEmpty
            | State::BeginString
            | State::EndValue
            | State::EndTop
            | State::InString
            | State::InStringEsc
            | State::InStringEscU(_)
            | State::E
            | State::ESign
            | State::E0
            | State::Literal(..)
            | State::Error => Scan::Error,
        }
    }

    fn step_exponent(&mut self, c: u8) -> Scan {
        match self.step {
            State::E if c == b'+' || c == b'-' => self.cont(State::ESign),
            State::E | State::ESign => {
                if c.is_ascii_digit() {
                    self.cont(State::E0)
                } else {
                    self.error(c, "in exponent of numeric literal")
                }
            }
            State::E0 if c.is_ascii_digit() => Scan::Continue,
            State::E0 => self.state_end_value(c),
            State::BeginValue
            | State::BeginValueOrEmpty
            | State::BeginStringOrEmpty
            | State::BeginString
            | State::EndValue
            | State::EndTop
            | State::InString
            | State::InStringEsc
            | State::InStringEscU(_)
            | State::Neg
            | State::One
            | State::Zero
            | State::Dot
            | State::Dot0
            | State::Literal(..)
            | State::Error => Scan::Error,
        }
    }

    const fn cont(&mut self, next: State) -> Scan {
        self.step = next;
        Scan::Continue
    }

    /// Go: `stateT`, `stateTr`, ... `stateNul`.
    fn state_literal(&mut self, c: u8, word: Word, index: u8) -> Scan {
        let text = word.text();
        let Some(&want) = text.as_bytes().get(usize::from(index)) else {
            return Scan::Error;
        };
        if c != want {
            let context = format!("in literal {text} (expecting {})", quote_char(want));
            return self.error(c, &context);
        }
        let next = index.saturating_add(1);
        self.step = if usize::from(next) >= text.len() {
            State::EndValue
        } else {
            State::Literal(word, next)
        };
        Scan::Continue
    }
}

/// Go: encoding/json/scanner.go `quoteChar`.
fn quote_char(c: u8) -> String {
    if c == b'\'' {
        return r"'\''".to_owned();
    }
    if c == b'"' {
        return "'\"'".to_owned();
    }
    // strconv.Quote(string(c)) where string(c) converts the byte to the rune U+00XX.
    let quoted = go_quote_rune(char::from(c));
    format!("'{quoted}'")
}

/// The inside of Go's `strconv.Quote(string(r))` for a rune in U+0000..=U+00FF.
fn go_quote_rune(r: char) -> String {
    match r {
        '\u{7}' => r"\a".to_owned(),
        '\u{8}' => r"\b".to_owned(),
        '\u{c}' => r"\f".to_owned(),
        '\n' => r"\n".to_owned(),
        '\r' => r"\r".to_owned(),
        '\t' => r"\t".to_owned(),
        '\u{b}' => r"\v".to_owned(),
        '\\' => r"\\".to_owned(),
        _ => {
            let code = u32::from(r);
            // strconv.IsPrint for Latin-1: U+0020..=U+007E and U+00A1..=U+00FF except U+00AD.
            let printable = (0x20..0x7F).contains(&code) || (code >= 0xA1 && code != 0xAD);
            if printable {
                r.to_string()
            } else if code < 0x80 {
                format!(r"\x{code:02x}")
            } else {
                format!(r"\u{code:04x}")
            }
        }
    }
}

/// Builds the arena from a value the scanner accepted.
fn build(input: &[u8]) -> Document {
    let mut builder = Builder {
        input,
        pos: 0,
        nodes: Vec::new(),
    };
    let root = builder.value();
    Document {
        nodes: builder.nodes,
        root,
    }
}

enum Frame {
    Array(NodeId),
    Object(NodeId, String),
}

struct Builder<'a> {
    input: &'a [u8],
    pos: usize,
    nodes: Vec<Node>,
}

impl Builder<'_> {
    fn peek(&self) -> u8 {
        self.input.get(self.pos).copied().unwrap_or(0)
    }

    fn skip_space(&mut self) {
        while is_space(self.peek()) {
            self.pos += 1;
        }
    }

    fn push(&mut self, node: Node) -> NodeId {
        self.nodes.push(node);
        NodeId(self.nodes.len() - 1)
    }

    /// Parses one complete value without recursion.
    fn value(&mut self) -> NodeId {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            self.skip_space();
            let mut done = self.scalar_or_open(&mut stack);
            // Close containers and attach finished values to their parents.
            while let Some(id) = done {
                let Some(frame) = stack.pop() else {
                    return id;
                };
                done = self.attach(frame, id, &mut stack);
            }
        }
    }

    /// Reads a scalar (returned) or opens a container (pushed; returns the empty container when
    /// it closes immediately).
    fn scalar_or_open(&mut self, stack: &mut Vec<Frame>) -> Option<NodeId> {
        match self.peek() {
            b'{' => {
                self.pos += 1;
                let id = self.push(Node::Object(Vec::new()));
                self.skip_space();
                if self.peek() == b'}' {
                    self.pos += 1;
                    return Some(id);
                }
                let key = self.object_key();
                stack.push(Frame::Object(id, key));
                None
            }
            b'[' => {
                self.pos += 1;
                let id = self.push(Node::Array(Vec::new()));
                self.skip_space();
                if self.peek() == b']' {
                    self.pos += 1;
                    return Some(id);
                }
                stack.push(Frame::Array(id));
                None
            }
            b'"' => {
                let s = self.string();
                Some(self.push(Node::String(s)))
            }
            b't' => {
                self.pos += 4;
                Some(self.push(Node::Bool(true)))
            }
            b'f' => {
                self.pos += 5;
                Some(self.push(Node::Bool(false)))
            }
            b'n' => {
                self.pos += 4;
                Some(self.push(Node::Null))
            }
            _ => {
                let start = self.pos;
                while matches!(self.peek(), b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    self.pos += 1;
                }
                let literal = self.input.get(start..self.pos).unwrap_or_default();
                Some(self.push(Node::Number(String::from_utf8_lossy(literal).into_owned())))
            }
        }
    }

    /// Adds a finished child to its container. Returns the container when it is complete.
    fn attach(&mut self, frame: Frame, child: NodeId, stack: &mut Vec<Frame>) -> Option<NodeId> {
        match frame {
            Frame::Array(id) => {
                if let Some(Node::Array(items)) = self.nodes.get_mut(id.0) {
                    items.push(child);
                }
                self.skip_space();
                let close = self.peek() == b']';
                self.pos += 1;
                if close {
                    Some(id)
                } else {
                    stack.push(Frame::Array(id));
                    None
                }
            }
            Frame::Object(id, key) => {
                if let Some(Node::Object(entries)) = self.nodes.get_mut(id.0) {
                    if let Some(entry) = entries.iter_mut().find(|(k, _)| *k == key) {
                        entry.1 = child;
                    } else {
                        entries.push((key, child));
                    }
                }
                self.skip_space();
                let close = self.peek() == b'}';
                self.pos += 1;
                if close {
                    Some(id)
                } else {
                    self.skip_space();
                    let next = self.object_key();
                    stack.push(Frame::Object(id, next));
                    None
                }
            }
        }
    }

    /// Reads `"key"` and the following `:`.
    fn object_key(&mut self) -> String {
        let key = self.string();
        self.skip_space();
        self.pos += 1; // ':'
        key
    }

    /// Go: encoding/json/decode.go `unquoteBytes` on the string literal at `pos`.
    fn string(&mut self) -> String {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let c = self.peek();
            match c {
                b'"' => {
                    self.pos += 1;
                    return out;
                }
                b'\\' => self.escape(&mut out),
                0..=0x7F => {
                    out.push(char::from(c));
                    self.pos += 1;
                }
                _ => {
                    let rest = self.input.get(self.pos..).unwrap_or_default();
                    let (r, size) = decode_rune(rest);
                    out.push(r);
                    self.pos += size;
                }
            }
            if self.pos >= self.input.len() {
                return out;
            }
        }
    }

    fn escape(&mut self, out: &mut String) {
        let c = self.input.get(self.pos + 1).copied().unwrap_or(0);
        self.pos += 2;
        let simple = match c {
            b'b' => Some('\u{8}'),
            b'f' => Some('\u{c}'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'u' => None,
            other => Some(char::from(other)),
        };
        if let Some(ch) = simple {
            out.push(ch);
            return;
        }
        let r = self.getu4(self.pos);
        self.pos += 4;
        if (0xD800..0xE000).contains(&r) {
            // Go: a valid surrogate pair is consumed as one rune, anything else becomes U+FFFD.
            if self.input.get(self.pos) == Some(&b'\\')
                && self.input.get(self.pos + 1) == Some(&b'u')
            {
                let r2 = self.getu4(self.pos + 2);
                if (0xD800..0xDC00).contains(&r) && (0xDC00..0xE000).contains(&r2) {
                    let combined = 0x10000 + ((r - 0xD800) << 10) + (r2 - 0xDC00);
                    out.push(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                    self.pos += 6;
                    return;
                }
            }
            out.push('\u{FFFD}');
            return;
        }
        out.push(char::from_u32(r).unwrap_or('\u{FFFD}'));
    }

    fn getu4(&self, at: usize) -> u32 {
        let digits = self.input.get(at..at + 4).unwrap_or_default();
        let mut r = 0u32;
        for &d in digits {
            let v = match d {
                b'0'..=b'9' => d - b'0',
                b'a'..=b'f' => d - b'a' + 10,
                b'A'..=b'F' => d - b'A' + 10,
                _ => 0,
            };
            r = r * 16 + u32::from(v);
        }
        r
    }
}

/// Go: `utf8.DecodeRune`: one rune, or `(U+FFFD, 1)` for an invalid encoding.
pub(super) fn decode_rune(s: &[u8]) -> (char, usize) {
    let Some(&b0) = s.first() else {
        return ('\u{FFFD}', 0);
    };
    let width = match b0 {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return ('\u{FFFD}', 1),
    };
    // std's UTF-8 validation rejects exactly what Go's acceptRanges reject (overlong forms,
    // surrogates, values above U+10FFFF).
    s.get(..width)
        .and_then(|bytes| str::from_utf8(bytes).ok())
        .and_then(|text| text.chars().next())
        .map_or(('\u{FFFD}', 1), |c| (c, width))
}

/// Go: `utf8.RuneCount` of a Go string (always valid UTF-8 once decoded).
pub(super) fn rune_count(s: &str) -> usize {
    s.chars().count()
}
