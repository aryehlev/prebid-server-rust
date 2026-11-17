//! Go: `text/template/parse/lex.go`, with the default `{{ }}` delimiters, comments skipped and
//! `break`/`continue` lexed as keywords (no function of those names is defined).

use crate::{
    fmt::{Value, sprintf},
    strconv::quote,
    unicode,
};

/// Go: `itemType`; the order matters (`typ > Keyword` marks keywords).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(super) enum ItemType {
    #[default]
    Error,
    Bool,
    Char,
    CharConstant,
    Comment,
    Complex,
    Assign,
    Declare,
    Eof,
    Field,
    Identifier,
    LeftDelim,
    LeftParen,
    Number,
    Pipe,
    RawString,
    RightDelim,
    RightParen,
    Space,
    String,
    Text,
    Variable,
    Keyword,
    Block,
    Break,
    Continue,
    Dot,
    Define,
    Else,
    End,
    If,
    Nil,
    Range,
    Template,
    With,
}

/// Go: `item`.
#[derive(Debug, Clone, Default)]
pub(super) struct Item {
    pub(super) typ: ItemType,
    pub(super) pos: usize,
    pub(super) val: String,
    pub(super) line: usize,
}

impl Item {
    /// Go: `item.String`.
    pub(super) fn describe(&self) -> String {
        if self.typ == ItemType::Eof {
            return "EOF".to_owned();
        }
        if self.typ == ItemType::Error {
            return self.val.clone();
        }
        if self.typ > ItemType::Keyword {
            return format!("<{}>", self.val);
        }
        if self.val.len() > 10 {
            return sprintf("%.10q...", &[Value::from(self.val.as_str())]);
        }
        quote(&self.val)
    }
}

/// Go: the `key` map of keywords.
fn keyword(word: &str) -> Option<ItemType> {
    Some(match word {
        "." => ItemType::Dot,
        "block" => ItemType::Block,
        "break" => ItemType::Break,
        "continue" => ItemType::Continue,
        "define" => ItemType::Define,
        "else" => ItemType::Else,
        "end" => ItemType::End,
        "if" => ItemType::If,
        "range" => ItemType::Range,
        "nil" => ItemType::Nil,
        "template" => ItemType::Template,
        "with" => ItemType::With,
        _ => return None,
    })
}

const LEFT_DELIM: &str = "{{";
const RIGHT_DELIM: &str = "}}";
const LEFT_COMMENT: &str = "/*";
const RIGHT_COMMENT: &str = "*/";
/// The trim marker plus the space before or after it.
const TRIM_MARKER_LEN: usize = 2;
const SPACE_CHARS: &[char] = &[' ', '\t', '\r', '\n'];

/// Go: the lexer's state functions.
#[derive(Debug, Clone, Copy)]
enum State {
    Text,
    LeftDelim,
    Comment,
    RightDelim,
    InsideAction,
    Space,
    Identifier,
    Field,
    Variable,
    Char,
    Number,
    Quote,
    RawQuote,
}

const fn is_space(r: char) -> bool {
    matches!(r, ' ' | '\t' | '\r' | '\n')
}

fn is_alpha_numeric(r: char) -> bool {
    r == '_' || unicode::is_letter(r) || unicode::is_digit(r)
}

fn has_left_trim_marker(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b, [b'-', c, ..] if is_space(char::from(*c)))
}

fn has_right_trim_marker(s: &str) -> bool {
    let b = s.as_bytes();
    matches!(b, [c, b'-', ..] if is_space(char::from(*c)))
}

fn count_newlines(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

/// Go's `%#U`: `U+005D ']'`.
fn unicode_text(r: Option<char>) -> String {
    let code = r.map_or(-1, |c| i32::try_from(u32::from(c)).unwrap_or(-1));
    sprintf("%#U", &[Value::Int32(code)])
}

/// Go: `lexer`.
#[derive(Debug)]
pub(super) struct Lexer<'a> {
    input: &'a str,
    pos: usize,
    start: usize,
    at_eof: bool,
    paren_depth: i64,
    line: usize,
    start_line: usize,
    item: Item,
    inside_action: bool,
}

impl<'a> Lexer<'a> {
    pub(super) fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            start: 0,
            at_eof: false,
            paren_depth: 0,
            line: 1,
            start_line: 1,
            item: Item::default(),
            inside_action: false,
        }
    }

    fn rest(&self, from: usize) -> &'a str {
        self.input.get(from..).unwrap_or_default()
    }

    fn slice(&self, from: usize, to: usize) -> &'a str {
        self.input.get(from..to).unwrap_or_default()
    }

    /// Go: `next`; `None` is `eof`.
    fn next(&mut self) -> Option<char> {
        let Some(r) = self.rest(self.pos).chars().next() else {
            self.at_eof = true;
            return None;
        };
        self.pos += r.len_utf8();
        if r == '\n' {
            self.line += 1;
        }
        Some(r)
    }

    fn peek(&mut self) -> Option<char> {
        let r = self.next();
        self.backup();
        r
    }

    /// Go: `backup`: steps back one rune; a no-op once `eof` has been seen.
    fn backup(&mut self) {
        if !self.at_eof
            && self.pos > 0
            && let Some(r) = self.slice(0, self.pos).chars().next_back()
        {
            self.pos -= r.len_utf8();
            if r == '\n' {
                self.line -= 1;
            }
        }
    }

    fn this_item(&mut self, typ: ItemType) -> Item {
        let item = Item {
            typ,
            pos: self.start,
            val: self.slice(self.start, self.pos).to_owned(),
            line: self.start_line,
        };
        self.start = self.pos;
        self.start_line = self.line;
        item
    }

    fn emit(&mut self, typ: ItemType) -> Option<State> {
        self.item = self.this_item(typ);
        None
    }

    fn ignore(&mut self) {
        self.line += count_newlines(self.slice(self.start, self.pos));
        self.start = self.pos;
        self.start_line = self.line;
    }

    fn accept(&mut self, valid: &str) -> bool {
        if self.next().is_some_and(|r| valid.contains(r)) {
            return true;
        }
        self.backup();
        false
    }

    fn accept_run(&mut self, valid: &str) {
        while self.next().is_some_and(|r| valid.contains(r)) {}
        self.backup();
    }

    /// Go: `errorf`: an error item, and the input is dropped so that `EOF` follows.
    fn errorf(&mut self, message: String) -> Option<State> {
        self.item = Item {
            typ: ItemType::Error,
            pos: self.start,
            val: message,
            line: self.start_line,
        };
        self.start = 0;
        self.pos = 0;
        self.input = "";
        None
    }

    /// Go: `nextItem`.
    pub(super) fn next_item(&mut self) -> Item {
        self.item = Item {
            typ: ItemType::Eof,
            pos: self.pos,
            val: "EOF".to_owned(),
            line: self.start_line,
        };
        let mut state = if self.inside_action {
            State::InsideAction
        } else {
            State::Text
        };
        while let Some(next) = self.step(state) {
            state = next;
        }
        self.item.clone()
    }

    fn step(&mut self, state: State) -> Option<State> {
        match state {
            State::Text => self.lex_text(),
            State::LeftDelim => self.lex_left_delim(),
            State::Comment => self.lex_comment(),
            State::RightDelim => self.lex_right_delim(),
            State::InsideAction => self.lex_inside_action(),
            State::Space => self.lex_space(),
            State::Identifier => self.lex_identifier(),
            State::Field => self.lex_field_or_variable(ItemType::Field),
            State::Variable => self.lex_variable(),
            State::Char => self.lex_quoted('\'', "unterminated character constant"),
            State::Number => self.lex_number(),
            State::Quote => self.lex_quoted('"', "unterminated quoted string"),
            State::RawQuote => self.lex_raw_quote(),
        }
    }

    /// Go: `lexText`.
    fn lex_text(&mut self) -> Option<State> {
        if let Some(x) = self.rest(self.pos).find(LEFT_DELIM) {
            if x > 0 {
                self.pos += x;
                let mut trim_length = 0;
                let delim_end = self.pos + LEFT_DELIM.len();
                if has_left_trim_marker(self.rest(delim_end)) {
                    let text = self.slice(self.start, self.pos);
                    trim_length = text.len() - text.trim_end_matches(SPACE_CHARS).len();
                }
                self.pos -= trim_length;
                self.line += count_newlines(self.slice(self.start, self.pos));
                let item = self.this_item(ItemType::Text);
                self.pos += trim_length;
                self.ignore();
                if !item.val.is_empty() {
                    self.item = item;
                    return None;
                }
            }
            return Some(State::LeftDelim);
        }
        self.pos = self.input.len();
        if self.pos > self.start {
            self.line += count_newlines(self.slice(self.start, self.pos));
            return self.emit(ItemType::Text);
        }
        self.emit(ItemType::Eof)
    }

    /// Go: `atRightDelim`: whether the input is at a right delimiter, and whether it is
    /// preceded by a trim marker.
    fn at_right_delim(&self) -> (bool, bool) {
        let rest = self.rest(self.pos);
        if has_right_trim_marker(rest)
            && self
                .rest(self.pos + TRIM_MARKER_LEN)
                .starts_with(RIGHT_DELIM)
        {
            return (true, true);
        }
        (rest.starts_with(RIGHT_DELIM), false)
    }

    fn left_trim_length(&self) -> usize {
        let rest = self.rest(self.pos);
        rest.len() - rest.trim_start_matches(SPACE_CHARS).len()
    }

    /// Go: `lexLeftDelim`.
    fn lex_left_delim(&mut self) -> Option<State> {
        self.pos += LEFT_DELIM.len();
        let trim_space = has_left_trim_marker(self.rest(self.pos));
        let after_marker = if trim_space { TRIM_MARKER_LEN } else { 0 };
        if self.rest(self.pos + after_marker).starts_with(LEFT_COMMENT) {
            self.pos += after_marker;
            self.ignore();
            return Some(State::Comment);
        }
        let item = self.this_item(ItemType::LeftDelim);
        self.inside_action = true;
        self.pos += after_marker;
        self.ignore();
        self.paren_depth = 0;
        self.item = item;
        None
    }

    /// Go: `lexComment` (comments are not emitted).
    fn lex_comment(&mut self) -> Option<State> {
        self.pos += LEFT_COMMENT.len();
        let Some(x) = self.rest(self.pos).find(RIGHT_COMMENT) else {
            return self.errorf("unclosed comment".to_owned());
        };
        self.pos += x + RIGHT_COMMENT.len();
        let (delim, trim_space) = self.at_right_delim();
        if !delim {
            return self.errorf("comment ends before closing delimiter".to_owned());
        }
        self.line += count_newlines(self.slice(self.start, self.pos));
        let _comment = self.this_item(ItemType::Comment);
        if trim_space {
            self.pos += TRIM_MARKER_LEN;
        }
        self.pos += RIGHT_DELIM.len();
        if trim_space {
            self.pos += self.left_trim_length();
        }
        self.ignore();
        Some(State::Text)
    }

    /// Go: `lexRightDelim`.
    fn lex_right_delim(&mut self) -> Option<State> {
        let (_, trim_space) = self.at_right_delim();
        if trim_space {
            self.pos += TRIM_MARKER_LEN;
            self.ignore();
        }
        self.pos += RIGHT_DELIM.len();
        let item = self.this_item(ItemType::RightDelim);
        if trim_space {
            self.pos += self.left_trim_length();
            self.ignore();
        }
        self.inside_action = false;
        self.item = item;
        None
    }

    /// Go: `lexInsideAction`.
    fn lex_inside_action(&mut self) -> Option<State> {
        let (delim, _) = self.at_right_delim();
        if delim {
            if self.paren_depth == 0 {
                return Some(State::RightDelim);
            }
            return self.errorf("unclosed left paren".to_owned());
        }
        let Some(r) = self.next() else {
            return self.errorf("unclosed action".to_owned());
        };
        match r {
            _ if is_space(r) => {
                self.backup(); // Put space back in case we have " -}}".
                Some(State::Space)
            }
            '=' => self.emit(ItemType::Assign),
            ':' => {
                if self.next() != Some('=') {
                    return self.errorf("expected :=".to_owned());
                }
                self.emit(ItemType::Declare)
            }
            '|' => self.emit(ItemType::Pipe),
            '"' => Some(State::Quote),
            '`' => Some(State::RawQuote),
            '$' => Some(State::Variable),
            '\'' => Some(State::Char),
            '.' if self
                .rest(self.pos)
                .bytes()
                .next()
                .is_some_and(|c| !c.is_ascii_digit()) =>
            {
                Some(State::Field)
            }
            // '.' can start a number.
            '.' | '+' | '-' | '0'..='9' => {
                self.backup();
                Some(State::Number)
            }
            _ if is_alpha_numeric(r) => {
                self.backup();
                Some(State::Identifier)
            }
            '(' => {
                self.paren_depth += 1;
                self.emit(ItemType::LeftParen)
            }
            ')' => {
                self.paren_depth -= 1;
                if self.paren_depth < 0 {
                    return self.errorf("unexpected right paren".to_owned());
                }
                self.emit(ItemType::RightParen)
            }
            _ if r.is_ascii() && unicode::is_print(r) => self.emit(ItemType::Char),
            _ => self.errorf(format!(
                "unrecognized character in action: {}",
                unicode_text(Some(r))
            )),
        }
    }

    /// Go: `lexSpace`: a run of spaces, or the right delimiter when a single space precedes a
    /// trim marker.
    fn lex_space(&mut self) -> Option<State> {
        let mut num_spaces = 0;
        while self.peek().is_some_and(is_space) {
            let _space = self.next();
            num_spaces += 1;
        }
        let before = self.pos.saturating_sub(1);
        if has_right_trim_marker(self.rest(before))
            && self.rest(before + TRIM_MARKER_LEN).starts_with(RIGHT_DELIM)
        {
            self.backup(); // Before the space.
            if num_spaces == 1 {
                return Some(State::RightDelim); // On the delim, so go right to that.
            }
        }
        self.emit(ItemType::Space)
    }

    /// Go: `lexIdentifier`.
    fn lex_identifier(&mut self) -> Option<State> {
        let r = loop {
            match self.next() {
                Some(r) if is_alpha_numeric(r) => {}
                r => break r,
            }
        };
        self.backup();
        let word = self.slice(self.start, self.pos);
        if !self.at_terminator() {
            return self.errorf(format!("bad character {}", unicode_text(r)));
        }
        let typ = match keyword(word) {
            Some(key) if key > ItemType::Keyword => key,
            _ if word.starts_with('.') => ItemType::Field,
            _ if word == "true" || word == "false" => ItemType::Bool,
            _ => ItemType::Identifier,
        };
        self.emit(typ)
    }

    /// Go: `lexVariable`.
    fn lex_variable(&mut self) -> Option<State> {
        if self.at_terminator() {
            // Nothing interesting follows -> "$".
            return self.emit(ItemType::Variable);
        }
        self.lex_field_or_variable(ItemType::Variable)
    }

    /// Go: `lexFieldOrVariable`.
    fn lex_field_or_variable(&mut self, typ: ItemType) -> Option<State> {
        if self.at_terminator() {
            // Nothing interesting follows -> "." or "$".
            if typ == ItemType::Variable {
                return self.emit(ItemType::Variable);
            }
            return self.emit(ItemType::Dot);
        }
        let r = loop {
            match self.next() {
                Some(r) if is_alpha_numeric(r) => {}
                r => break r,
            }
        };
        self.backup();
        if !self.at_terminator() {
            return self.errorf(format!("bad character {}", unicode_text(r)));
        }
        self.emit(typ)
    }

    /// Go: `atTerminator`: whether the next rune can follow a word.
    fn at_terminator(&mut self) -> bool {
        match self.peek() {
            None | Some('.' | ',' | '|' | ':' | ')' | '(') => true,
            Some(r) if is_space(r) => true,
            Some(_) => self.rest(self.pos).starts_with(RIGHT_DELIM),
        }
    }

    /// Go: `lexChar` and `lexQuote`: a quoted constant with backslash escapes.
    fn lex_quoted(&mut self, quote: char, unterminated: &str) -> Option<State> {
        loop {
            match self.next() {
                Some('\\') => {
                    if matches!(self.next(), Some(r) if r != '\n') {
                        continue;
                    }
                    return self.errorf(unterminated.to_owned());
                }
                None | Some('\n') => return self.errorf(unterminated.to_owned()),
                Some(r) if r == quote => break,
                Some(_) => {}
            }
        }
        if quote == '"' {
            self.emit(ItemType::String)
        } else {
            self.emit(ItemType::CharConstant)
        }
    }

    /// Go: `lexRawQuote`.
    fn lex_raw_quote(&mut self) -> Option<State> {
        loop {
            match self.next() {
                None => return self.errorf("unterminated raw quoted string".to_owned()),
                Some('`') => break,
                Some(_) => {}
            }
        }
        self.emit(ItemType::RawString)
    }

    /// Go: `lexNumber`.
    fn lex_number(&mut self) -> Option<State> {
        if !self.scan_number() {
            let text = quote(self.slice(self.start, self.pos));
            return self.errorf(format!("bad number syntax: {text}"));
        }
        if matches!(self.peek(), Some('+' | '-')) {
            // Complex: 1+2i. No spaces, must end in 'i'.
            if !self.scan_number() || !self.slice(0, self.pos).ends_with('i') {
                let text = quote(self.slice(self.start, self.pos));
                return self.errorf(format!("bad number syntax: {text}"));
            }
            return self.emit(ItemType::Complex);
        }
        self.emit(ItemType::Number)
    }

    /// Go: `scanNumber`.
    fn scan_number(&mut self) -> bool {
        // Optional leading sign.
        let _sign = self.accept("+-");
        // Is it hex?
        let mut digits = "0123456789_";
        if self.accept("0") {
            // Note: Leading 0 does not mean octal in floats.
            if self.accept("xX") {
                digits = "0123456789abcdefABCDEF_";
            } else if self.accept("oO") {
                digits = "01234567_";
            } else if self.accept("bB") {
                digits = "01_";
            }
        }
        self.accept_run(digits);
        if self.accept(".") {
            self.accept_run(digits);
        }
        if digits.len() == 10 + 1 && self.accept("eE") {
            let _exponent_sign = self.accept("+-");
            self.accept_run("0123456789_");
        }
        if digits.len() == 16 + 6 + 1 && self.accept("pP") {
            let _binary_exponent_sign = self.accept("+-");
            self.accept_run("0123456789_");
        }
        // Is it imaginary?
        let _imaginary = self.accept("i");
        // Next thing mustn't be alphanumeric.
        if self.peek().is_some_and(is_alpha_numeric) {
            let _bad = self.next();
            return false;
        }
        true
    }
}
