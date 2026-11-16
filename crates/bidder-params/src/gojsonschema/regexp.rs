//! Go `regexp` (RE2 syntax with Perl flags, as `regexp.Compile` uses) on top of the `regex`
//! crate.
//!
//! The two engines share most of their syntax and leftmost-first semantics, but differ in a few
//! places that matter for parity, so patterns are translated instead of being passed through:
//!
//! - `\d`, `\s`, `\w` and `\b` are ASCII-only in Go and Unicode-aware in Rust;
//! - Go has `\Q...\E`, octal escapes and a literal `{` when it does not start a repetition;
//! - inside a class Rust treats `[`, `&&`, `--` and `~~` specially, Go does not.
//!
//! Every literal character is therefore emitted as `\x{..}`, Perl classes as explicit ASCII
//! classes, and Go's structural errors (missing brackets/parens, bad escapes, bad repetitions)
//! are rejected like `regexp/syntax` rejects them.

use std::fmt::Write as _;

use regex::{Regex, RegexBuilder};

/// A compiled Go regular expression that remembers its source (Go: `(*Regexp).String`).
#[derive(Debug, Clone)]
pub struct GoRegexp {
    source: String,
    regex: Regex,
}

impl GoRegexp {
    /// Go: `regexp.Compile(expr)`.
    ///
    /// # Errors
    ///
    /// Returns a description of the problem when Go would reject the pattern (or the
    /// translated pattern does not compile).
    pub fn compile(expr: &str) -> Result<Self, String> {
        let translated = translate(expr)?;
        let regex = RegexBuilder::new(&translated)
            .size_limit(1 << 28)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            source: expr.to_owned(),
            regex,
        })
    }

    /// Go: `(*Regexp).MatchString` (an unanchored search).
    #[must_use]
    pub fn match_string(&self, s: &str) -> bool {
        self.regex.is_match(s)
    }

    /// Go: `(*Regexp).String`: the source text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }
}

/// Go: regexp/syntax `perlGroup`, as Rust class items: `(positive, items)`.
const fn perl_class_items(c: char) -> Option<(bool, &'static str)> {
    match c {
        'd' => Some((true, r"\x{30}-\x{39}")),
        'D' => Some((false, r"\x{30}-\x{39}")),
        's' => Some((true, r"\x{9}\x{A}\x{C}\x{D}\x{20}")),
        'S' => Some((false, r"\x{9}\x{A}\x{C}\x{D}\x{20}")),
        'w' => Some((true, r"\x{30}-\x{39}\x{41}-\x{5A}\x{5F}\x{61}-\x{7A}")),
        'W' => Some((false, r"\x{30}-\x{39}\x{41}-\x{5A}\x{5F}\x{61}-\x{7A}")),
        _ => None,
    }
}

/// Go: regexp/syntax `posixGroup` names.
const POSIX_CLASSES: [&str; 14] = [
    "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
    "space", "upper", "word", "xdigit",
];

/// Go: regexp/syntax rejects repeat counts above 1000.
const MAX_REPEAT: u32 = 1000;

/// Translates a Go pattern into equivalent Rust `regex` syntax.
fn translate(expr: &str) -> Result<String, String> {
    let mut t = Translator {
        chars: expr.chars().collect(),
        pos: 0,
        out: String::new(),
    };
    t.run()?;
    Ok(t.out)
}

/// Appends `c` as a `\x{..}` escape, valid both inside and outside Rust classes.
fn push_hex(out: &mut String, c: char) {
    let _ok = write!(out, r"\x{{{:X}}}", u32::from(c));
}

/// What the last emitted item was, for Go's repetition rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    /// Start of the pattern, after `(` or `|`: a repetition has no argument.
    Nothing,
    /// An operand that can be repeated.
    Operand,
    /// A repetition operator: Go rejects stacking another one.
    Repeat,
}

/// The upper bound of a counted repetition.
#[derive(Debug, Clone, Copy)]
enum RepeatMax {
    /// `{n}`.
    Exact,
    /// `{n,}`.
    Unbounded,
    /// `{n,m}`; `None` when `m` overflows Go's `parseInt` limit.
    Bounded(Option<u32>),
}

/// A parsed `{..}` repetition; `min` is `None` when it overflows Go's `parseInt` limit.
#[derive(Debug, Clone, Copy)]
struct RepeatSpec {
    min: Option<u32>,
    max: RepeatMax,
    len: usize,
}

struct Translator {
    chars: Vec<char>,
    pos: usize,
    out: String,
}

impl Translator {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn literal(&mut self, c: char) {
        push_hex(&mut self.out, c);
    }

    fn run(&mut self) -> Result<(), String> {
        let mut depth = 0usize;
        let mut last = Last::Nothing;
        while let Some(c) = self.peek() {
            last = match c {
                '(' => {
                    if self.open_group()? {
                        depth += 1;
                        Last::Nothing
                    } else if last == Last::Repeat {
                        // Go: a flag group resets `lastRepeat` but keeps the operand.
                        Last::Operand
                    } else {
                        last
                    }
                }
                '|' => {
                    self.pos += 1;
                    self.out.push('|');
                    Last::Nothing
                }
                ')' => {
                    depth = depth.checked_sub(1).ok_or("unexpected )")?;
                    self.pos += 1;
                    self.out.push(')');
                    Last::Operand
                }
                '^' | '$' | '.' => {
                    self.pos += 1;
                    self.out.push(c);
                    Last::Operand
                }
                '[' => {
                    self.class()?;
                    Last::Operand
                }
                '*' | '+' | '?' => {
                    self.pos += 1;
                    self.repeat(last, &c.to_string())?
                }
                '{' => self.brace(last)?,
                '\\' => {
                    self.escape()?;
                    Last::Operand
                }
                _ => {
                    self.pos += 1;
                    self.literal(c);
                    Last::Operand
                }
            };
        }
        if depth != 0 {
            return Err("missing closing )".to_owned());
        }
        Ok(())
    }

    /// Go: `p.repeat`: checks the operand and the non-greedy suffix.
    fn repeat(&mut self, last: Last, op: &str) -> Result<Last, String> {
        match last {
            Last::Nothing => return Err(format!("missing argument to repetition operator: {op}")),
            Last::Repeat => return Err(format!("invalid nested repetition operator: {op}")),
            Last::Operand => {}
        }
        self.out.push_str(op);
        if self.peek() == Some('?') {
            self.pos += 1;
            self.out.push('?');
        }
        Ok(Last::Repeat)
    }

    /// Go: `case '{'`: a repetition when `parseRepeat` accepts it, a literal `{` otherwise.
    fn brace(&mut self, last: Last) -> Result<Last, String> {
        let Some(spec) = self.parse_repeat() else {
            self.pos += 1;
            self.literal('{');
            return Ok(Last::Operand);
        };
        let invalid = || "invalid repeat count".to_owned();
        let min = spec.min.filter(|&m| m <= MAX_REPEAT).ok_or_else(invalid)?;
        let op = match spec.max {
            RepeatMax::Exact => format!("{{{min}}}"),
            RepeatMax::Unbounded => format!("{{{min},}}"),
            RepeatMax::Bounded(max) => {
                let max = max
                    .filter(|&m| m <= MAX_REPEAT && m >= min)
                    .ok_or_else(invalid)?;
                format!("{{{min},{max}}}")
            }
        };
        self.pos += spec.len;
        self.repeat(last, &op)
    }

    /// Go: `parseRepeat`.
    fn parse_repeat(&self) -> Option<RepeatSpec> {
        let mut i = self.pos + 1;
        let (min, next) = self.parse_int(i)?;
        i = next;
        let max = match self.chars.get(i) {
            Some(',') => {
                i += 1;
                if self.chars.get(i) == Some(&'}') {
                    RepeatMax::Unbounded
                } else {
                    let (hi, after) = self.parse_int(i)?;
                    i = after;
                    RepeatMax::Bounded(hi)
                }
            }
            Some(_) => RepeatMax::Exact,
            None => return None,
        };
        if self.chars.get(i) != Some(&'}') {
            return None;
        }
        Some(RepeatSpec {
            min,
            max,
            len: i + 1 - self.pos,
        })
    }

    /// Go: `parseInt`: digits without a leading zero; values of 1e8 and more become `None`.
    fn parse_int(&self, start: usize) -> Option<(Option<u32>, usize)> {
        let first = self.chars.get(start)?;
        if !first.is_ascii_digit() {
            return None;
        }
        if *first == '0' && self.chars.get(start + 1).is_some_and(char::is_ascii_digit) {
            return None;
        }
        let mut end = start;
        let mut value: Option<u32> = Some(0);
        while let Some(d) = self.chars.get(end).and_then(|c| c.to_digit(10)) {
            value = value.and_then(|v| {
                if v >= 100_000_000 {
                    None
                } else {
                    Some(v * 10 + d)
                }
            });
            end += 1;
        }
        Some((value, end))
    }

    /// Go: `parsePerlFlags` and plain capture groups. Returns whether a group was opened
    /// (`false` for a bare flag change such as `(?i)`).
    fn open_group(&mut self) -> Result<bool, String> {
        if self.peek_at(1) != Some('?') {
            self.pos += 1;
            self.out.push('(');
            return Ok(true);
        }
        let named_p = self.peek_at(2) == Some('P') && self.peek_at(3) == Some('<');
        let named = self.peek_at(2) == Some('<');
        if named_p || named {
            let start = self.pos + if named_p { 4 } else { 3 };
            let rest = self.chars.get(start..).unwrap_or_default();
            let end = rest
                .iter()
                .position(|&c| c == '>')
                .ok_or("invalid named capture")?;
            let name: String = rest.get(..end).unwrap_or_default().iter().collect();
            if name.is_empty() || !name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
                return Err(format!("invalid named capture: {name}"));
            }
            self.pos = start + end + 1;
            let _ok = write!(self.out, "(?P<{name}>");
            return Ok(true);
        }
        self.flags()
    }

    /// Go: the flag loop of `parsePerlFlags`: `(?flags)` or `(?flags:`.
    fn flags(&mut self) -> Result<bool, String> {
        let mut i = self.pos + 2;
        let mut negated = false;
        let mut saw_flag = false;
        let mut text = String::new();
        while let Some(&c) = self.chars.get(i) {
            i += 1;
            match c {
                'i' | 'm' | 's' | 'U' => {
                    saw_flag = true;
                    text.push(c);
                }
                '-' if !negated => {
                    negated = true;
                    saw_flag = false;
                    text.push('-');
                }
                ':' | ')' if !negated || saw_flag => {
                    self.pos = i;
                    self.out.push_str("(?");
                    self.out.push_str(&text);
                    self.out.push(c);
                    return Ok(c == ':');
                }
                _ => break,
            }
        }
        Err("invalid or unsupported Perl syntax".to_owned())
    }

    /// Escapes outside a class.
    fn escape(&mut self) -> Result<(), String> {
        let Some(c) = self.peek_at(1) else {
            return Err("trailing backslash at end of expression".to_owned());
        };
        match c {
            'A' | 'z' => {
                self.pos += 2;
                self.out.push('\\');
                self.out.push(c);
            }
            'b' | 'B' => {
                self.pos += 2;
                let _ok = write!(self.out, r"(?-u:\{c})");
            }
            'C' => return Err(r"invalid escape sequence: \C".to_owned()),
            'Q' => {
                self.pos += 2;
                while let Some(ch) = self.peek() {
                    if ch == '\\' && self.peek_at(1) == Some('E') {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                    self.literal(ch);
                }
            }
            'p' | 'P' => {
                let class = self.unicode_class()?;
                self.out.push_str(&class);
            }
            _ => {
                if let Some((positive, items)) = perl_class_items(c) {
                    self.pos += 2;
                    let caret = if positive { "" } else { "^" };
                    let _ok = write!(self.out, "[{caret}{items}]");
                } else {
                    let r = self.parse_escape()?;
                    self.literal(r);
                }
            }
        }
        Ok(())
    }

    /// Go: `parseUnicodeClass` for `\pX`, `\p{Name}`, `\p{^Name}` and the `\P` forms.
    fn unicode_class(&mut self) -> Result<String, String> {
        let mut positive = self.peek_at(1) == Some('p');
        let Some(first) = self.peek_at(2) else {
            return Err("invalid character class range".to_owned());
        };
        let mut name: String = if first == '{' {
            let rest = self.chars.get(self.pos + 3..).unwrap_or_default();
            let end = rest
                .iter()
                .position(|&c| c == '}')
                .ok_or("invalid character class")?;
            let braced = rest.get(..end).unwrap_or_default().iter().collect();
            self.pos += 3 + end + 1;
            braced
        } else {
            self.pos += 3;
            first.to_string()
        };
        if let Some(rest) = name.strip_prefix('^') {
            positive = !positive;
            name = rest.to_owned();
        }
        if name.is_empty() || !name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
            return Err(format!("invalid character class range: {name}"));
        }
        Ok(format!(r"\{}{{{name}}}", if positive { 'p' } else { 'P' }))
    }

    /// Go: `parseEscape` for single-character escapes.
    fn parse_escape(&mut self) -> Result<char, String> {
        let Some(c) = self.peek_at(1) else {
            return Err("trailing backslash at end of expression".to_owned());
        };
        self.pos += 2;
        let invalid = || format!(r"invalid escape sequence: \{c}");
        match c {
            '1'..='7' => {
                if !self.peek().is_some_and(|d| ('0'..='7').contains(&d)) {
                    return Err(invalid());
                }
                Ok(self.octal(c))
            }
            '0' => Ok(self.octal(c)),
            'x' => self.hex_escape().ok_or_else(invalid),
            'a' => Ok('\u{7}'),
            'f' => Ok('\u{c}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'v' => Ok('\u{b}'),
            _ if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok(c),
            _ => Err(invalid()),
        }
    }

    /// Up to two more octal digits after `first`.
    fn octal(&mut self, first: char) -> char {
        let mut r = first.to_digit(8).unwrap_or(0);
        for _ in 1..3 {
            match self.peek().and_then(|d| d.to_digit(8)) {
                Some(d) => {
                    r = r * 8 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        char::from_u32(r).unwrap_or('\u{FFFD}')
    }

    /// `\xFF` or `\x{10FFFF}` (the `\x` is already consumed).
    fn hex_escape(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '{' {
            let mut r: u32 = 0;
            let mut count = 0;
            loop {
                let d = self.peek()?;
                self.pos += 1;
                if d == '}' {
                    break;
                }
                r = r * 16 + d.to_digit(16)?;
                if r > 0x0010_FFFF {
                    return None;
                }
                count += 1;
            }
            if count == 0 {
                return None;
            }
            return char::from_u32(r).or(Some('\u{FFFD}'));
        }
        let hi = c.to_digit(16)?;
        let lo = self.peek()?.to_digit(16)?;
        self.pos += 1;
        char::from_u32(hi * 16 + lo)
    }

    /// Go: `parseClass`.
    fn class(&mut self) -> Result<(), String> {
        self.pos += 1;
        let mut items = String::new();
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut first = true;
        loop {
            match self.peek() {
                None => return Err("missing closing ]".to_owned()),
                Some(']') if !first => break,
                Some(_) => {}
            }
            first = false;
            self.class_item(&mut items)?;
        }
        self.pos += 1; // ]
        let caret = if negated { "^" } else { "" };
        let _ok = write!(self.out, "[{caret}{items}]");
        Ok(())
    }

    /// One item of a class: a POSIX class, a Unicode class, a Perl class, a character or a range.
    fn class_item(&mut self, items: &mut String) -> Result<(), String> {
        if self.posix_class(items)? {
            return Ok(());
        }
        if self.peek() == Some('\\') {
            if matches!(self.peek_at(1), Some('p' | 'P')) {
                let class = self.unicode_class()?;
                items.push_str(&class);
                return Ok(());
            }
            if let Some((positive, perl)) = self.peek_at(1).and_then(perl_class_items) {
                self.pos += 2;
                if positive {
                    items.push_str(perl);
                } else {
                    let _ok = write!(items, "[^{perl}]");
                }
                return Ok(());
            }
        }
        let lo = self.class_char()?;
        let mut hi = lo;
        // Go: [a-] means (a|-), so a range needs something other than ] after the dash.
        if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
            self.pos += 1;
            hi = self.class_char()?;
            if hi < lo {
                return Err("invalid character class range".to_owned());
            }
        }
        push_hex(items, lo);
        if hi != lo {
            items.push('-');
            push_hex(items, hi);
        }
        Ok(())
    }

    /// Go: `parseNamedClass` for `[:alpha:]` and `[:^alpha:]`.
    fn posix_class(&mut self, items: &mut String) -> Result<bool, String> {
        if self.peek() != Some('[') || self.peek_at(1) != Some(':') {
            return Ok(false);
        }
        let rest = self.chars.get(self.pos + 2..).unwrap_or_default();
        let Some(end) = rest.windows(2).position(|w| w == [':', ']']) else {
            return Ok(false);
        };
        let name: String = rest.get(..end).unwrap_or_default().iter().collect();
        let bare = name.strip_prefix('^').unwrap_or(&name);
        if !POSIX_CLASSES.contains(&bare) {
            return Err(format!("invalid character class range: [:{name}:]"));
        }
        self.pos += 2 + end + 2;
        let _ok = write!(items, "[[:{name}:]]");
        Ok(true)
    }

    /// Go: `parseClassChar`.
    fn class_char(&mut self) -> Result<char, String> {
        match self.peek() {
            None => Err("missing closing ]".to_owned()),
            Some('\\') => self.parse_escape(),
            Some(c) => {
                self.pos += 1;
                Ok(c)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GoRegexp;

    fn matches(pattern: &str, input: &str) -> Result<bool, String> {
        Ok(GoRegexp::compile(pattern)?.match_string(input))
    }

    #[test]
    fn translation_keeps_go_semantics() -> Result<(), String> {
        let cases = [
            (r"^\d+$", "123", true),
            (r"^\d+$", "\u{0663}", false),
            (r"^\w+$", "\u{e9}", false),
            (r"^\s$", "\u{a0}", false),
            (r"^\S$", "\u{a0}", true),
            (r"\bfoo\b", "\u{e9}foo", true),
            (
                r"^[a-fA-F\d\-]{32,36}$",
                "0123456789abcdef0123456789abcdef",
                true,
            ),
            (
                r"^[a-fA-F\d\-]{32,36}$",
                "0123456789abcdef0123456789abcde",
                false,
            ),
            (r"(\S+-)?\d+-\d+", "x-1-2", true),
            ("[0-9|a-z|A-Z]{16}", "abcdefgh|1234567", true),
            ("a{", "a{", true),
            ("a{,2}", "a{,2}", true),
            (r"\Q.*\E", ".*", true),
            (r"\Q.*\E", "ab", false),
            ("[[a]", "[", true),
            ("[a&&b]", "&", true),
            ("[]a]", "]", true),
            ("[^]a]", "b", true),
            (r"\x41\x{42}\101", "ABA", true),
            ("(?i)k", "\u{212a}", true),
            ("[[:alpha:]]+", "abc", true),
            (r"\pL", "\u{e9}", true),
        ];
        for (pattern, input, want) in cases {
            if matches(pattern, input)? != want {
                return Err(format!("{pattern} on {input:?}: want {want}"));
            }
        }
        Ok(())
    }

    #[test]
    fn go_rejects() -> Result<(), String> {
        for bad in [
            "(", ")", "[a", "a**", "*a", r"\C", r"\1", "a{1001}", r"\y", "(?x)a", "\\",
        ] {
            if GoRegexp::compile(bad).is_ok() {
                return Err(format!("{bad} should be rejected"));
            }
        }
        Ok(())
    }
}
