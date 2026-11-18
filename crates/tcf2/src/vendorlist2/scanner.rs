//! Go `encoding/json/scanner.go`: the validity check (`checkValid`) that `json.Unmarshal` runs
//! over the whole input before decoding anything, so a syntax error anywhere wins over every
//! decoding error.
//!
//! The state machine is a one-to-one port of Go's `step` functions; only the opcodes that
//! `checkValid` looks at are kept.

/// Go: `maxNestingDepth`.
const MAX_NESTING_DEPTH: usize = 10_000;

/// Go: `parseObjectKey`, `parseObjectValue` and `parseArrayValue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    ObjectKey,
    ObjectValue,
    ArrayValue,
}

/// Go: the `scanner.step` function value, one variant per `state*` function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
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

/// The scanner opcodes `checkValid` distinguishes: Go's `scanEnd`, `scanError`, and every other
/// opcode as `Continue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Continue,
    End,
    Error,
}

/// Go: `scanner`.
#[derive(Debug)]
struct Scanner {
    step: Step,
    parse_state: Vec<ParseState>,
    end_top: bool,
    err: Option<String>,
}

/// Go: `checkValid`: `Err` holds the `*json.SyntaxError` message.
pub(super) fn check_valid(data: &[u8]) -> Result<(), String> {
    let mut scan = Scanner {
        step: Step::BeginValue,
        parse_state: Vec::new(),
        end_top: false,
        err: None,
    };
    for &c in data {
        if scan.step(c) == Op::Error {
            return Err(scan.err.unwrap_or_default());
        }
    }
    scan.eof()
}

/// Go: `isSpace`.
const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

impl Scanner {
    /// Go: `scanner.eof`.
    fn eof(mut self) -> Result<(), String> {
        if let Some(err) = self.err {
            return Err(err);
        }
        if self.end_top {
            return Ok(());
        }
        let _op = self.step(b' ');
        if self.end_top {
            return Ok(());
        }
        Err(self
            .err
            .unwrap_or_else(|| "unexpected end of JSON input".to_owned()))
    }

    fn step(&mut self, c: u8) -> Op {
        match self.step {
            Step::BeginValueOrEmpty => self.begin_value_or_empty(c),
            Step::BeginValue => self.begin_value(c),
            Step::BeginStringOrEmpty => self.begin_string_or_empty(c),
            Step::BeginString => self.begin_string(c),
            Step::EndValue => self.end_value(c),
            Step::EndTop => self.end_top(c),
            Step::InString => self.in_string(c),
            Step::InStringEsc => self.in_string_esc(c),
            Step::InStringEscU => self.hex_digit(c, Step::InStringEscU1),
            Step::InStringEscU1 => self.hex_digit(c, Step::InStringEscU12),
            Step::InStringEscU12 => self.hex_digit(c, Step::InStringEscU123),
            Step::InStringEscU123 => self.hex_digit(c, Step::InString),
            Step::Neg => self.neg(c),
            Step::One => self.one(c),
            Step::Zero => self.zero(c),
            Step::Dot => self.dot(c),
            Step::Dot0 => self.dot0(c),
            Step::E => self.e(c),
            Step::ESign => self.e_sign(c),
            Step::E0 => self.e0(c),
            Step::T => self.literal(c, b'r', Step::Tr, "in literal true (expecting 'r')"),
            Step::Tr => self.literal(c, b'u', Step::Tru, "in literal true (expecting 'u')"),
            Step::Tru => self.literal(c, b'e', Step::EndValue, "in literal true (expecting 'e')"),
            Step::F => self.literal(c, b'a', Step::Fa, "in literal false (expecting 'a')"),
            Step::Fa => self.literal(c, b'l', Step::Fal, "in literal false (expecting 'l')"),
            Step::Fal => self.literal(c, b's', Step::Fals, "in literal false (expecting 's')"),
            Step::Fals => self.literal(c, b'e', Step::EndValue, "in literal false (expecting 'e')"),
            Step::N => self.literal(c, b'u', Step::Nu, "in literal null (expecting 'u')"),
            Step::Nu => self.literal(c, b'l', Step::Nul, "in literal null (expecting 'l')"),
            Step::Nul => self.literal(c, b'l', Step::EndValue, "in literal null (expecting 'l')"),
            Step::Error => Op::Error,
        }
    }

    /// Go: `scanner.error`.
    fn error(&mut self, c: u8, context: &str) -> Op {
        self.step = Step::Error;
        self.err = Some(format!("invalid character {} {context}", quote_char(c)));
        Op::Error
    }

    /// Go: `scanner.pushParseState`.
    fn push_parse_state(&mut self, c: u8, state: ParseState) -> Op {
        self.parse_state.push(state);
        if self.parse_state.len() <= MAX_NESTING_DEPTH {
            Op::Continue
        } else {
            self.error(c, "exceeded max depth")
        }
    }

    /// Go: `scanner.popParseState`.
    fn pop_parse_state(&mut self) {
        let _popped = self.parse_state.pop();
        if self.parse_state.is_empty() {
            self.step = Step::EndTop;
            self.end_top = true;
        } else {
            self.step = Step::EndValue;
        }
    }

    fn set_last_parse_state(&mut self, state: ParseState) {
        if let Some(last) = self.parse_state.last_mut() {
            *last = state;
        }
    }

    /// Go: `stateBeginValueOrEmpty`.
    fn begin_value_or_empty(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        if c == b']' {
            return self.end_value(c);
        }
        self.begin_value(c)
    }

    /// Go: `stateBeginValue`.
    fn begin_value(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        let next = match c {
            b'{' => {
                self.step = Step::BeginStringOrEmpty;
                return self.push_parse_state(c, ParseState::ObjectKey);
            }
            b'[' => {
                self.step = Step::BeginValueOrEmpty;
                return self.push_parse_state(c, ParseState::ArrayValue);
            }
            b'"' => Step::InString,
            b'-' => Step::Neg,
            b'0' => Step::Zero,
            b't' => Step::T,
            b'f' => Step::F,
            b'n' => Step::N,
            b'1'..=b'9' => Step::One,
            _ => return self.error(c, "looking for beginning of value"),
        };
        self.step = next;
        Op::Continue
    }

    /// Go: `stateBeginStringOrEmpty`.
    fn begin_string_or_empty(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        if c == b'}' {
            self.set_last_parse_state(ParseState::ObjectValue);
            return self.end_value(c);
        }
        self.begin_string(c)
    }

    /// Go: `stateBeginString`.
    fn begin_string(&mut self, c: u8) -> Op {
        if is_space(c) {
            return Op::Continue;
        }
        if c == b'"' {
            self.step = Step::InString;
            return Op::Continue;
        }
        self.error(c, "looking for beginning of object key string")
    }

    /// Go: `stateEndValue`.
    fn end_value(&mut self, c: u8) -> Op {
        let Some(&state) = self.parse_state.last() else {
            self.step = Step::EndTop;
            self.end_top = true;
            return self.end_top(c);
        };
        if is_space(c) {
            self.step = Step::EndValue;
            return Op::Continue;
        }
        match state {
            ParseState::ObjectKey => {
                if c == b':' {
                    self.set_last_parse_state(ParseState::ObjectValue);
                    self.step = Step::BeginValue;
                    return Op::Continue;
                }
                self.error(c, "after object key")
            }
            ParseState::ObjectValue => {
                if c == b',' {
                    self.set_last_parse_state(ParseState::ObjectKey);
                    self.step = Step::BeginString;
                    return Op::Continue;
                }
                if c == b'}' {
                    self.pop_parse_state();
                    return Op::Continue;
                }
                self.error(c, "after object key:value pair")
            }
            ParseState::ArrayValue => {
                if c == b',' {
                    self.step = Step::BeginValue;
                    return Op::Continue;
                }
                if c == b']' {
                    self.pop_parse_state();
                    return Op::Continue;
                }
                self.error(c, "after array element")
            }
        }
    }

    /// Go: `stateEndTop`. The error is recorded but reported by the next step (or `eof`).
    fn end_top(&mut self, c: u8) -> Op {
        if !is_space(c) {
            let _op = self.error(c, "after top-level value");
        }
        Op::End
    }

    /// Go: `stateInString`.
    fn in_string(&mut self, c: u8) -> Op {
        match c {
            b'"' => self.step = Step::EndValue,
            b'\\' => self.step = Step::InStringEsc,
            0..0x20 => return self.error(c, "in string literal"),
            _ => {}
        }
        Op::Continue
    }

    /// Go: `stateInStringEsc`.
    fn in_string_esc(&mut self, c: u8) -> Op {
        match c {
            b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => self.step = Step::InString,
            b'u' => self.step = Step::InStringEscU,
            _ => return self.error(c, "in string escape code"),
        }
        Op::Continue
    }

    /// Go: `stateInStringEscU`, `stateInStringEscU1`, `stateInStringEscU12` and
    /// `stateInStringEscU123`.
    fn hex_digit(&mut self, c: u8, next: Step) -> Op {
        if c.is_ascii_hexdigit() {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, "in \\u hexadecimal character escape")
    }

    /// Go: `stateNeg`.
    fn neg(&mut self, c: u8) -> Op {
        match c {
            b'0' => self.step = Step::Zero,
            b'1'..=b'9' => self.step = Step::One,
            _ => return self.error(c, "in numeric literal"),
        }
        Op::Continue
    }

    /// Go: `state1`.
    fn one(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = Step::One;
            return Op::Continue;
        }
        self.zero(c)
    }

    /// Go: `state0`.
    fn zero(&mut self, c: u8) -> Op {
        match c {
            b'.' => self.step = Step::Dot,
            b'e' | b'E' => self.step = Step::E,
            _ => return self.end_value(c),
        }
        Op::Continue
    }

    /// Go: `stateDot`.
    fn dot(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = Step::Dot0;
            return Op::Continue;
        }
        self.error(c, "after decimal point in numeric literal")
    }

    /// Go: `stateDot0`.
    fn dot0(&mut self, c: u8) -> Op {
        match c {
            b'0'..=b'9' => {}
            b'e' | b'E' => self.step = Step::E,
            _ => return self.end_value(c),
        }
        Op::Continue
    }

    /// Go: `stateE`.
    fn e(&mut self, c: u8) -> Op {
        if c == b'+' || c == b'-' {
            self.step = Step::ESign;
            return Op::Continue;
        }
        self.e_sign(c)
    }

    /// Go: `stateESign`.
    fn e_sign(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            self.step = Step::E0;
            return Op::Continue;
        }
        self.error(c, "in exponent of numeric literal")
    }

    /// Go: `stateE0`.
    fn e0(&mut self, c: u8) -> Op {
        if c.is_ascii_digit() {
            return Op::Continue;
        }
        self.end_value(c)
    }

    /// Go: `stateT`, `stateTr`, `stateTru`, `stateF`, ..., `stateNul`: one expected byte of a
    /// `true`, `false` or `null` literal.
    fn literal(&mut self, c: u8, expected: u8, next: Step, context: &str) -> Op {
        if c == expected {
            self.step = next;
            return Op::Continue;
        }
        self.error(c, context)
    }
}

/// Go: `quoteChar`: formats a byte for an error message, using `strconv.Quote` on the rune with
/// that value.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => r"'\''".to_owned(),
        b'"' => r#"'"'"#.to_owned(),
        _ => format!("'{}'", quote_rune(c)),
    }
}

/// Go `strconv.Quote(string(rune(c)))` without the surrounding quotes, for a rune in
/// `U+0000..=U+00FF` other than `"`.
fn quote_rune(c: u8) -> String {
    match c {
        b'\\' => r"\\".to_owned(),
        0x07 => r"\a".to_owned(),
        0x08 => r"\b".to_owned(),
        0x0C => r"\f".to_owned(),
        b'\n' => r"\n".to_owned(),
        b'\r' => r"\r".to_owned(),
        b'\t' => r"\t".to_owned(),
        0x0B => r"\v".to_owned(),
        0..0x20 | 0x7F => format!(r"\x{c:02x}"),
        // Not printable per strconv.IsPrint: the C1 controls, no-break space and soft hyphen.
        0x80..=0xA0 | 0xAD => format!(r"\u{c:04x}"),
        _ => char::from(c).to_string(),
    }
}
