//! Glue (Rule B (d)): Prebid's own patterns keep Go `regexp`'s meaning on the regex crate.
//!
//! Prebid's regular expressions are Go `regexp` patterns, in code and in the `pattern` keywords
//! of static/bidder-params, so each is translated from Go's RE2 syntax (`regexp/syntax` with the
//! `Perl` flags that `regexp.Compile` uses) to the regex crate's before it is compiled.
//!
//! The translation keeps Go's syntax and its meaning where the two differ:
//!
//! - `\d`, `\w`, `\s` and `\b` (and `\D`, `\W`, `\S`, `\B`) are ASCII in Go and Unicode in the
//!   regex crate: outside classes they become explicit ASCII sets and `(?-u:\b)`; inside classes,
//!   the ASCII ranges or a nested negated set. Go's `\s` is `[\t\n\f\r ]`, without the vertical tab
//!   of the crate's ASCII `\s`.
//! - `\Q...\E` (Go: literal text) becomes escaped literals; the regex crate rejects it.
//! - A `{` that does not start a valid repetition is a literal in Go and an error in the regex
//!   crate (`{{\s*\.ExternalURL\s*}}`, `^{h(?::(\d+))?}$`); a count with a leading zero (`a{01}`)
//!   is not a repetition in Go.
//! - Every literal is written as an escape of its code point, so `\<` stays Go's `<` (a word
//!   boundary in the regex crate), and `[`, `&&`, `--` and `~~` inside a class stay literals
//!   (nested classes and set operations in the regex crate).
//! - POSIX classes are written as their ASCII ranges, octal and `\x` escapes as code points, and
//!   `\p{Name}` as `\p{gc=Name}` or `\p{sc=Name}` (the crate reads a bare script name as
//!   `Script_Extensions`, Go as `Script`); `\p{^Name}` is `\P{Name}`.
//! - Go's errors are kept: stacked repetitions (`a**`), counts above 1000 alone or nested
//!   (`(a{100}){20}`), a repetition with nothing to repeat, flags other than `i`, `m`, `s` and `U`,
//!   backreferences, `\C`, `\E` without `\Q`, and unknown escapes all fail to compile, as they do
//!   in Go, where the regex crate accepts some of them.
//!
//! Matching is then the regex crate's: leftmost-first like Go's, with Go's capture numbering,
//! simple case folding and `.`, `^` and `$` under the same flags. The forms the translation
//! does not carry over, which no Prebid pattern uses, are listed on [`CompileError::Unsupported`].

use std::{error, fmt};

use regex::Regex;

/// The largest repetition count Go accepts, alone or as the product of nested counts.
const MAX_REPEAT: i64 = 1000;

/// Why a pattern does not compile.
#[derive(Clone, Debug)]
pub enum CompileError {
    /// A pattern Go's `regexp/syntax` rejects: Go's error code text (`missing closing )`) and
    /// the part of the pattern it names.
    Syntax {
        /// Go's `syntax.ErrorCode` text.
        code: &'static str,
        /// The part of the pattern the error names.
        expr: String,
    },
    /// A pattern Go accepts that the translation does not carry over: a repetition right
    /// after a flag group (`a(?i)*`) or an escape of a surrogate code point (`\x{D800}`).
    Unsupported {
        /// The part of the pattern the translation stops at.
        expr: String,
        /// Why.
        reason: &'static str,
    },
    /// The regex crate refuses the translated pattern: its size and nesting limits, or a
    /// Unicode name it does not know.
    Crate(regex::Error),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { code, expr } => write!(f, "error parsing regexp: {code}: `{expr}`"),
            Self::Unsupported { expr, reason } => write!(
                f,
                "error parsing regexp: not translated to the regex crate ({reason}): `{expr}`"
            ),
            Self::Crate(e) => write!(f, "error parsing regexp: {e}"),
        }
    }
}

impl error::Error for CompileError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Crate(e) => Some(e),
            Self::Syntax { .. } | Self::Unsupported { .. } => None,
        }
    }
}

/// Go: `regexp.Compile(pattern)`: the pattern translated to the regex crate's syntax and
/// compiled.
///
/// # Errors
///
/// [`CompileError::Syntax`] where Go's `regexp.Compile` fails, [`CompileError::Unsupported`] for
/// the forms the translation does not carry over, and [`CompileError::Crate`] when the regex crate
/// refuses the translation.
pub fn compile(pattern: &str) -> Result<Regex, CompileError> {
    let translated = translate(pattern)?;
    Regex::new(&translated).map_err(CompileError::Crate)
}

/// The regex crate's spelling of a Go `regexp` pattern.
///
/// # Errors
///
/// [`CompileError::Syntax`] where Go's parser fails and [`CompileError::Unsupported`] for the forms
/// the translation does not carry over.
pub fn translate(pattern: &str) -> Result<String, CompileError> {
    Translator {
        whole: pattern,
        pos: 0,
        out: String::with_capacity(pattern.len() * 2),
        weights: vec![0],
        last: Last::Nothing,
        last_repeat: 0,
    }
    .run()
}

/// Go's `syntax.ErrorCode` texts.
mod code {
    pub(super) const INVALID_CHAR_RANGE: &str = "invalid character class range";
    pub(super) const INVALID_ESCAPE: &str = "invalid escape sequence";
    pub(super) const INVALID_NAMED_CAPTURE: &str = "invalid named capture";
    pub(super) const INVALID_PERL_OP: &str = "invalid or unsupported Perl syntax";
    pub(super) const INVALID_REPEAT_OP: &str = "invalid nested repetition operator";
    pub(super) const INVALID_REPEAT_SIZE: &str = "invalid repeat count";
    pub(super) const MISSING_BRACKET: &str = "missing closing ]";
    pub(super) const MISSING_PAREN: &str = "missing closing )";
    pub(super) const MISSING_REPEAT_ARGUMENT: &str = "missing argument to repetition operator";
    pub(super) const TRAILING_BACKSLASH: &str = "trailing backslash at end of expression";
    pub(super) const UNEXPECTED_PAREN: &str = "unexpected )";
}

fn syntax(code: &'static str, expr: &str) -> CompileError {
    CompileError::Syntax {
        code,
        expr: expr.to_owned(),
    }
}

/// What a repetition operator would apply to (Go: the top of the parse stack and
/// `lastRepeat`).
#[derive(Clone, Copy, Debug)]
enum Last {
    /// Nothing: the start of the pattern or of a group, or after `|` (Go:
    /// `ErrMissingRepeatArgument`).
    Nothing,
    /// An operand, with the product of the counts of its nested `{n,m}` repetitions (Go:
    /// `repeatIsValid`).
    Operand(i64),
    /// A repetition, which Go does not let another follow (Go: `ErrInvalidRepeatOp`), with the
    /// count product of the repeated operand.
    Repeat(i64),
    /// A flag group such as `(?i)` after an operand, which Go repeats when a repetition follows.
    Flags,
}

struct Translator<'a> {
    whole: &'a str,
    /// The byte offset of the rest of the pattern.
    pos: usize,
    out: String,
    /// The largest count product of the finished operands of each open group, the whole
    /// pattern first.
    weights: Vec<i64>,
    last: Last,
    /// The offset of the last repetition operator, for Go's error text.
    last_repeat: usize,
}

impl<'a> Translator<'a> {
    fn rest(&self) -> &'a str {
        self.whole.get(self.pos..).unwrap_or_default()
    }

    fn run(mut self) -> Result<String, CompileError> {
        while let Some(c) = self.rest().chars().next() {
            match c {
                '(' => self.group()?,
                ')' => self.close()?,
                '|' => {
                    self.settle();
                    self.out.push('|');
                    self.last = Last::Nothing;
                    self.pos += 1;
                }
                '^' | '$' | '.' => {
                    self.out.push(c);
                    self.operand(0);
                    self.pos += 1;
                }
                '[' => self.class()?,
                '*' | '+' | '?' => self.repeat_op()?,
                '{' => self.brace()?,
                '\\' => self.escape()?,
                _ => {
                    self.literal(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if self.weights.len() > 1 {
            return Err(syntax(code::MISSING_PAREN, self.whole));
        }
        Ok(self.out)
    }

    /// Records an operand of count product `weight`, once the one before it is finished.
    fn operand(&mut self, weight: i64) {
        self.settle();
        self.last = Last::Operand(weight);
    }

    /// Adds the count product of the last operand, with the repetition applied to it, to its
    /// group's. Go's `repeatIsValid` walks finished operands, so a `{0}` that follows an operand
    /// stops the check of the counts inside it (`(?:(?:a{10}){0}){1000}` compiles).
    fn settle(&mut self) {
        if let Last::Operand(weight) | Last::Repeat(weight) = self.last
            && let Some(w) = self.weights.last_mut()
        {
            *w = (*w).max(weight);
        }
    }

    /// Writes a literal character (Go: `p.literal`).
    fn literal(&mut self, c: char) {
        push_char(&mut self.out, c);
        self.operand(0);
    }

    /// `(`: a capture, a named capture, a non-capturing group or a flag group (Go:
    /// `parsePerlFlags`).
    fn group(&mut self) -> Result<(), CompileError> {
        let t = self.rest();
        let Some(after) = t.strip_prefix("(?") else {
            self.out.push('(');
            self.open(1);
            return Ok(());
        };
        let named = if after.starts_with("P<") && t.len() > 4 {
            Some(4)
        } else if after.starts_with('<') && t.len() > 3 {
            Some(3)
        } else {
            None
        };
        if let Some(begin) = named {
            let Some(end) = t.find('>') else {
                return Err(syntax(code::INVALID_NAMED_CAPTURE, t));
            };
            let name = t.get(begin..end).unwrap_or_default();
            let capture = t.get(..=end).unwrap_or_default();
            if name.is_empty() || !name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric()) {
                return Err(syntax(code::INVALID_NAMED_CAPTURE, capture));
            }
            self.out.push_str("(?P<");
            self.out.push_str(name);
            self.out.push('>');
            self.open(end + 1);
            return Ok(());
        }
        let mut negated = false;
        let mut saw_flag = false;
        let mut flags = String::new();
        for (i, c) in after.char_indices() {
            match c {
                'i' | 'm' | 's' | 'U' => {
                    saw_flag = true;
                    flags.push(c);
                }
                '-' if !negated => {
                    negated = true;
                    saw_flag = false;
                    flags.push(c);
                }
                ':' | ')' if !negated || saw_flag => {
                    let len = 2 + i + 1;
                    if c == ':' {
                        self.out.push_str("(?");
                        self.out.push_str(&flags);
                        self.out.push(':');
                        self.open(len);
                    } else {
                        if !flags.is_empty() {
                            self.out.push_str("(?");
                            self.out.push_str(&flags);
                            self.out.push(')');
                        }
                        self.settle();
                        // At the start of a group or an alternative Go has nothing to repeat
                        // (`(?i)*` is ErrMissingRepeatArgument); after an operand it repeats it.
                        if !matches!(self.last, Last::Nothing) {
                            self.last = Last::Flags;
                        }
                        self.pos += len;
                    }
                    return Ok(());
                }
                _ => {
                    let len = 2 + i + c.len_utf8();
                    return Err(syntax(code::INVALID_PERL_OP, t.get(..len).unwrap_or(t)));
                }
            }
        }
        Err(syntax(code::INVALID_PERL_OP, t))
    }

    /// Opens a group whose opening text is `len` bytes long.
    fn open(&mut self, len: usize) {
        self.settle();
        self.weights.push(0);
        self.last = Last::Nothing;
        self.pos += len;
    }

    /// `)`.
    fn close(&mut self) -> Result<(), CompileError> {
        if self.weights.len() < 2 {
            return Err(syntax(code::UNEXPECTED_PAREN, self.whole));
        }
        self.settle();
        let weight = self.weights.pop().unwrap_or_default();
        self.out.push(')');
        self.pos += 1;
        self.last = Last::Nothing;
        self.operand(weight);
        Ok(())
    }

    /// Checks that a repetition may follow, as Go's `p.repeat` does, given the text of the
    /// operator (with its `?`).
    fn check_repeat(&self, op: &str) -> Result<i64, CompileError> {
        match self.last {
            Last::Operand(weight) => Ok(weight),
            Last::Repeat(_) => {
                let end = self.pos + op.len();
                let expr = self.whole.get(self.last_repeat..end).unwrap_or(op);
                Err(syntax(code::INVALID_REPEAT_OP, expr))
            }
            Last::Nothing => Err(syntax(code::MISSING_REPEAT_ARGUMENT, op)),
            Last::Flags => Err(CompileError::Unsupported {
                expr: op.to_owned(),
                reason: "a repetition right after a flag group",
            }),
        }
    }

    /// `*`, `+` or `?`, with an optional `?` for the non-greedy form.
    fn repeat_op(&mut self) -> Result<(), CompileError> {
        let t = self.rest();
        let len = if t.get(1..2) == Some("?") { 2 } else { 1 };
        let op = t.get(..len).unwrap_or_default();
        // `*`, `+` and `?` pass the counts of their operand through (Go: repeatIsValid).
        let weight = self.check_repeat(op)?;
        self.out.push_str(op);
        self.last_repeat = self.pos;
        self.last = Last::Repeat(weight);
        self.pos += len;
        Ok(())
    }

    /// `{`: a counted repetition when it parses as one (Go: `parseRepeat`), else a literal.
    fn brace(&mut self) -> Result<(), CompileError> {
        let t = self.rest();
        let Some((min, max, len)) = parse_repeat(t) else {
            self.literal('{');
            self.pos += 1;
            return Ok(());
        };
        let counts = t.get(..len).unwrap_or_default();
        if !(0..=MAX_REPEAT).contains(&min) || max > MAX_REPEAT || (max >= 0 && min > max) {
            return Err(syntax(code::INVALID_REPEAT_SIZE, counts));
        }
        let lazy = t.get(len..=len) == Some("?");
        let op = t.get(..len + usize::from(lazy)).unwrap_or_default();
        let inner = self.check_repeat(op)?;
        // Go's repeatIsValid: `{0}` stops the check, `{0,}` passes it through, and a count
        // multiplies the counts nested inside it.
        let bound = if max < 0 { min } else { max };
        let weight = if max == 0 {
            0
        } else if bound == 0 {
            inner
        } else {
            bound.saturating_mul(inner.max(1))
        };
        if (min >= 2 || max >= 2) && weight > MAX_REPEAT {
            return Err(syntax(code::INVALID_REPEAT_SIZE, op));
        }
        self.out.push('{');
        push_decimal(&mut self.out, min);
        if max != min {
            self.out.push(',');
            if max >= 0 {
                push_decimal(&mut self.out, max);
            }
        }
        self.out.push('}');
        if lazy {
            self.out.push('?');
        }
        self.last_repeat = self.pos;
        self.last = Last::Repeat(weight);
        self.pos += op.len();
        Ok(())
    }

    /// `\`: an assertion, `\Q...\E`, a Unicode or Perl class, or an escaped character.
    fn escape(&mut self) -> Result<(), CompileError> {
        let t = self.rest();
        let Some(second) = t.get(1..).and_then(|s| s.chars().next()) else {
            return Err(syntax(code::TRAILING_BACKSLASH, ""));
        };
        let text = match second {
            'A' => Some(r"\A"),
            'z' => Some(r"\z"),
            'b' => Some(r"(?-u:\b)"),
            'B' => Some(r"(?-u:\B)"),
            _ => None,
        };
        if let Some(text) = text {
            self.out.push_str(text);
            self.operand(0);
            self.pos += 2;
            return Ok(());
        }
        match second {
            'C' => Err(syntax(code::INVALID_ESCAPE, r"\C")),
            'Q' => {
                let body = t.get(2..).unwrap_or_default();
                let (literal, len) = body.find(r"\E").map_or((body, body.len()), |end| {
                    (body.get(..end).unwrap_or_default(), end + 2)
                });
                for c in literal.chars() {
                    self.literal(c);
                }
                self.pos += 2 + len;
                Ok(())
            }
            'p' | 'P' => {
                let (class, len) = unicode_class(t)?;
                self.out.push_str(&class);
                self.operand(0);
                self.pos += len;
                Ok(())
            }
            _ => {
                if let Some(set) = perl_class(second) {
                    self.out.push_str(set.outside);
                    self.operand(0);
                    self.pos += 2;
                    return Ok(());
                }
                let (c, len) = parse_escape(t)?;
                self.literal(c);
                self.pos += len;
                Ok(())
            }
        }
    }

    /// `[...]` (Go: `parseClass`).
    fn class(&mut self) -> Result<(), CompileError> {
        let whole_class = self.rest();
        let mut t = whole_class.get(1..).unwrap_or_default();
        let mut out = String::from("[");
        if let Some(rest) = t.strip_prefix('^') {
            out.push('^');
            t = rest;
        }
        let mut first = true;
        loop {
            if t.starts_with(']') && !first {
                break;
            }
            first = false;
            if t.len() > 2
                && t.starts_with("[:")
                && let Some(i) = t.get(2..).and_then(|s| s.find(":]"))
            {
                let name = t.get(..i + 4).unwrap_or_default();
                let Some(posix) = posix_class(name) else {
                    return Err(syntax(code::INVALID_CHAR_RANGE, name));
                };
                out.push_str(&posix);
                t = t.get(i + 4..).unwrap_or_default();
                continue;
            }
            if t.starts_with(r"\p") || t.starts_with(r"\P") {
                let (class, len) = unicode_class(t)?;
                out.push_str(&class);
                t = t.get(len..).unwrap_or_default();
                continue;
            }
            if let Some(set) = t
                .strip_prefix('\\')
                .and_then(|s| s.chars().next())
                .and_then(perl_class)
            {
                out.push_str(set.inside);
                t = t.get(2..).unwrap_or_default();
                continue;
            }
            let range = t;
            let (lo, rest) = class_char(t, whole_class)?;
            t = rest;
            let mut hi = lo;
            if t.len() >= 2 && t.starts_with('-') && !t.get(1..).is_some_and(|s| s.starts_with(']'))
            {
                let (h, after_hi) = class_char(t.get(1..).unwrap_or_default(), whole_class)?;
                if h < lo {
                    let expr = range.get(..range.len() - after_hi.len()).unwrap_or(range);
                    return Err(syntax(code::INVALID_CHAR_RANGE, expr));
                }
                hi = h;
                t = after_hi;
            }
            push_range(&mut out, lo, hi);
        }
        out.push(']');
        self.out.push_str(&out);
        self.operand(0);
        self.pos = self.whole.len() - t.len() + 1;
        Ok(())
    }
}

/// One character of a class, or an escape (Go: `parseClassChar`), and the rest of the text.
fn class_char<'a>(t: &'a str, whole_class: &str) -> Result<(char, &'a str), CompileError> {
    let Some(c) = t.chars().next() else {
        return Err(syntax(code::MISSING_BRACKET, whole_class));
    };
    if c == '\\' {
        let (escaped, len) = parse_escape(t)?;
        return Ok((escaped, t.get(len..).unwrap_or_default()));
    }
    Ok((c, t.get(c.len_utf8()..).unwrap_or_default()))
}

/// Go: `parseEscape`: the character an escape at the start of `t` stands for, and the
/// escape's length.
fn parse_escape(t: &str) -> Result<(char, usize), CompileError> {
    let body = t.get(1..).unwrap_or_default();
    let Some(c) = body.chars().next() else {
        return Err(syntax(code::TRAILING_BACKSLASH, ""));
    };
    let invalid = || {
        let len = 1 + c.len_utf8();
        syntax(code::INVALID_ESCAPE, t.get(..len).unwrap_or(t))
    };
    let digits = body.as_bytes();
    match c {
        '1'..='7' if !digits.get(1).is_some_and(|d| (b'0'..=b'7').contains(d)) => Err(invalid()),
        '0'..='7' => {
            // Up to three octal digits.
            let n = digits
                .iter()
                .take(3)
                .take_while(|d| (b'0'..=b'7').contains(*d))
                .count();
            let value = digits
                .get(..n)
                .unwrap_or_default()
                .iter()
                .fold(0_u32, |v, d| v * 8 + u32::from(d - b'0'));
            Ok((char::from_u32(value).unwrap_or('\0'), 1 + n))
        }
        'x' => hex_escape(t),
        'a' => Ok(('\u{7}', 2)),
        'f' => Ok(('\u{c}', 2)),
        'n' => Ok(('\n', 2)),
        'r' => Ok(('\r', 2)),
        't' => Ok(('\t', 2)),
        'v' => Ok(('\u{b}', 2)),
        _ if c.is_ascii() && !c.is_ascii_alphanumeric() => Ok((c, 2)),
        _ => Err(invalid()),
    }
}

/// Go's `\x` escapes: `\xHH` or `\x{H...}`, at most U+10FFFF. As in Go, an error names the
/// escape up to the character that ends it.
fn hex_escape(t: &str) -> Result<(char, usize), CompileError> {
    let invalid = |len: usize| syntax(code::INVALID_ESCAPE, t.get(..len).unwrap_or(t));
    let mut chars = t.get(2..).unwrap_or_default().chars();
    let mut len = 2;
    let Some(c) = chars.next() else {
        return Err(invalid(len));
    };
    len += c.len_utf8();
    if c != '{' {
        let y = chars.next();
        len += y.map_or(0, char::len_utf8);
        return match (c.to_digit(16), y.and_then(|y| y.to_digit(16))) {
            (Some(hi), Some(lo)) => Ok((char::from_u32(hi * 16 + lo).unwrap_or('\0'), len)),
            _ => Err(invalid(len)),
        };
    }
    let mut value = 0_u32;
    let mut digits = 0_usize;
    loop {
        let Some(d) = chars.next() else {
            return Err(invalid(len));
        };
        len += d.len_utf8();
        if d == '}' {
            break;
        }
        let Some(v) = d.to_digit(16) else {
            return Err(invalid(len));
        };
        value = value * 16 + v;
        if value > u32::from(char::MAX) {
            return Err(invalid(len));
        }
        digits += 1;
    }
    if digits == 0 {
        return Err(invalid(len));
    }
    char::from_u32(value)
        .map(|escaped| (escaped, len))
        .ok_or_else(|| CompileError::Unsupported {
            expr: t.get(..len).unwrap_or(t).to_owned(),
            reason: "an escape of a surrogate code point",
        })
}

/// A Perl class escape (Go: `perlGroup`), outside and inside a class.
struct PerlClass {
    outside: &'static str,
    inside: &'static str,
}

const fn perl_class(c: char) -> Option<PerlClass> {
    let (outside, inside) = match c {
        'd' => ("[0-9]", "0-9"),
        'D' => ("[^0-9]", "[^0-9]"),
        'w' => ("[0-9A-Z_a-z]", "0-9A-Z_a-z"),
        'W' => ("[^0-9A-Z_a-z]", "[^0-9A-Z_a-z]"),
        's' => (r"[\t\n\x0C\r\x20]", r"\t\n\x0C\r\x20"),
        'S' => (r"[^\t\n\x0C\r\x20]", r"[^\t\n\x0C\r\x20]"),
        _ => return None,
    };
    Some(PerlClass { outside, inside })
}

/// A POSIX class such as `[:alpha:]` or `[:^alpha:]` (Go: `posixGroup`), as class items.
fn posix_class(name: &str) -> Option<String> {
    let (negated, inner) = name
        .strip_prefix("[:^")
        .map_or((false, name.strip_prefix("[:")?), |inner| (true, inner));
    let ranges: &[(char, char)] = match inner.strip_suffix(":]")? {
        "alnum" => &[('0', '9'), ('A', 'Z'), ('a', 'z')],
        "alpha" => &[('A', 'Z'), ('a', 'z')],
        "ascii" => &[('\0', '\u{7f}')],
        "blank" => &[('\t', '\t'), (' ', ' ')],
        "cntrl" => &[('\0', '\u{1f}'), ('\u{7f}', '\u{7f}')],
        "digit" => &[('0', '9')],
        "graph" => &[('!', '~')],
        "lower" => &[('a', 'z')],
        "print" => &[(' ', '~')],
        "punct" => &[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')],
        "space" => &[('\t', '\r'), (' ', ' ')],
        "upper" => &[('A', 'Z')],
        "word" => &[('0', '9'), ('A', 'Z'), ('a', 'z'), ('_', '_')],
        "xdigit" => &[('0', '9'), ('A', 'F'), ('a', 'f')],
        _ => return None,
    };
    let mut out = String::from(if negated { "[^" } else { "" });
    for &(lo, hi) in ranges {
        push_range(&mut out, lo, hi);
    }
    if negated {
        out.push(']');
    }
    Some(out)
}

/// Go's general category names (`unicode.Categories` but `LC`, which Go looks up as `Lc`).
const CATEGORIES: [&str; 37] = [
    "C", "Cc", "Cf", "Cn", "Co", "Cs", "L", "Ll", "Lm", "Lo", "Lt", "Lu", "M", "Mc", "Me", "Mn",
    "N", "Nd", "Nl", "No", "P", "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "S", "Sc", "Sk", "Sm",
    "So", "Z", "Zl", "Zp", "Zs",
];

/// The scripts of Go's `unicode.Scripts` (Unicode 15.0.0) that a name reaches: Go looks a
/// name up in its canonical form, which no name of two words (`Old_Italic`) or with an inner
/// capital (`SignWriting`) has, so Go rejects those.
const SCRIPTS: [&str; 117] = [
    "Adlam",
    "Ahom",
    "Arabic",
    "Armenian",
    "Avestan",
    "Balinese",
    "Bamum",
    "Batak",
    "Bengali",
    "Bhaiksuki",
    "Bopomofo",
    "Brahmi",
    "Braille",
    "Buginese",
    "Buhid",
    "Carian",
    "Chakma",
    "Cham",
    "Cherokee",
    "Chorasmian",
    "Common",
    "Coptic",
    "Cuneiform",
    "Cypriot",
    "Cyrillic",
    "Deseret",
    "Devanagari",
    "Dogra",
    "Duployan",
    "Elbasan",
    "Elymaic",
    "Ethiopic",
    "Georgian",
    "Glagolitic",
    "Gothic",
    "Grantha",
    "Greek",
    "Gujarati",
    "Gurmukhi",
    "Han",
    "Hangul",
    "Hanunoo",
    "Hatran",
    "Hebrew",
    "Hiragana",
    "Inherited",
    "Javanese",
    "Kaithi",
    "Kannada",
    "Katakana",
    "Kawi",
    "Kharoshthi",
    "Khmer",
    "Khojki",
    "Khudawadi",
    "Lao",
    "Latin",
    "Lepcha",
    "Limbu",
    "Lisu",
    "Lycian",
    "Lydian",
    "Mahajani",
    "Makasar",
    "Malayalam",
    "Mandaic",
    "Manichaean",
    "Marchen",
    "Medefaidrin",
    "Miao",
    "Modi",
    "Mongolian",
    "Mro",
    "Multani",
    "Myanmar",
    "Nabataean",
    "Nandinagari",
    "Newa",
    "Nko",
    "Nushu",
    "Ogham",
    "Oriya",
    "Osage",
    "Osmanya",
    "Palmyrene",
    "Phoenician",
    "Rejang",
    "Runic",
    "Samaritan",
    "Saurashtra",
    "Sharada",
    "Shavian",
    "Siddham",
    "Sinhala",
    "Sogdian",
    "Soyombo",
    "Sundanese",
    "Syriac",
    "Tagalog",
    "Tagbanwa",
    "Takri",
    "Tamil",
    "Tangsa",
    "Tangut",
    "Telugu",
    "Thaana",
    "Thai",
    "Tibetan",
    "Tifinagh",
    "Tirhuta",
    "Toto",
    "Ugaritic",
    "Vai",
    "Vithkuqi",
    "Wancho",
    "Yezidi",
    "Yi",
];

/// Go's `unicode.CategoryAliases` in canonical form, with the category each names.
const CATEGORY_ALIASES: [(&str, &str); 42] = [
    ("Casedletter", "LC"),
    ("Closepunctuation", "Pe"),
    ("Cntrl", "Cc"),
    ("Combiningmark", "M"),
    ("Connectorpunctuation", "Pc"),
    ("Control", "Cc"),
    ("Currencysymbol", "Sc"),
    ("Dashpunctuation", "Pd"),
    ("Decimalnumber", "Nd"),
    ("Digit", "Nd"),
    ("Enclosingmark", "Me"),
    ("Finalpunctuation", "Pf"),
    ("Format", "Cf"),
    ("Initialpunctuation", "Pi"),
    ("Letter", "L"),
    ("Letternumber", "Nl"),
    ("Lineseparator", "Zl"),
    ("Lowercaseletter", "Ll"),
    ("Mark", "M"),
    ("Mathsymbol", "Sm"),
    ("Modifierletter", "Lm"),
    ("Modifiersymbol", "Sk"),
    ("Nonspacingmark", "Mn"),
    ("Number", "N"),
    ("Openpunctuation", "Ps"),
    ("Other", "C"),
    ("Otherletter", "Lo"),
    ("Othernumber", "No"),
    ("Otherpunctuation", "Po"),
    ("Othersymbol", "So"),
    ("Paragraphseparator", "Zp"),
    ("Privateuse", "Co"),
    ("Punct", "P"),
    ("Punctuation", "P"),
    ("Separator", "Z"),
    ("Spaceseparator", "Zs"),
    ("Spacingmark", "Mc"),
    ("Surrogate", "Cs"),
    ("Symbol", "S"),
    ("Titlecaseletter", "Lt"),
    ("Unassigned", "Cn"),
    ("Uppercaseletter", "Lu"),
];

/// Go: `canonicalName`: the first letter upper-case, the others lower-case, without `_`, `-`
/// and spaces.
fn canonical_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars().filter(|c| !matches!(c, '_' | '-' | ' ')) {
        if out.is_empty() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// `\pN`, `\p{Name}`, `\PN` or `\P{Name}` at the start of `t` (Go: `parseUnicodeClass`), as
/// the regex crate's class, and its length.
fn unicode_class(t: &str) -> Result<(String, usize), CompileError> {
    let mut negated = t.starts_with(r"\P");
    let body = t.get(2..).unwrap_or_default();
    let (seq_len, mut name) = if let Some(braced) = body.strip_prefix('{') {
        let Some(end) = braced.find('}') else {
            return Err(syntax(code::INVALID_CHAR_RANGE, t));
        };
        (2 + 1 + end + 1, braced.get(..end).unwrap_or_default())
    } else {
        let len = body.chars().next().map_or(0, char::len_utf8);
        (2 + len, body.get(..len).unwrap_or_default())
    };
    let seq = t.get(..seq_len).unwrap_or(t);
    if let Some(rest) = name.strip_prefix('^') {
        negated = !negated;
        name = rest;
    }
    // Go: unicodeTable, which looks the canonical name up among a few special names, then
    // the categories, the scripts and the category aliases.
    let canonical = canonical_name(name);
    let c = canonical.as_str();
    let property = match c {
        "Any" | "Assigned" => c,
        "Ascii" => "ASCII",
        "Lc" => "gc=LC",
        _ if CATEGORIES.contains(&c) => {
            return Ok((class_escape(negated, "gc=", c), seq_len));
        }
        _ if SCRIPTS.contains(&c) => return Ok((class_escape(negated, "sc=", c), seq_len)),
        _ => match CATEGORY_ALIASES.iter().find(|(alias, _)| *alias == c) {
            Some((_, category)) => {
                return Ok((class_escape(negated, "gc=", category), seq_len));
            }
            None => return Err(syntax(code::INVALID_CHAR_RANGE, seq)),
        },
    };
    Ok((class_escape(negated, "", property), seq_len))
}

/// `\p{<kind><name>}`, or `\P{...}` for a negated class.
fn class_escape(negated: bool, kind: &str, name: &str) -> String {
    let sign = if negated { 'P' } else { 'p' };
    format!("\\{sign}{{{kind}{name}}}")
}

/// Go: `parseRepeat`: the minimum and maximum of `{n}`, `{n,}` or `{n,m}` at the start of `t`
/// and its length, or `None` when it is not a repetition. As in Go, the maximum is -1 for
/// `{n,}`, and the minimum -1 when a count reaches 10^8.
fn parse_repeat(t: &str) -> Option<(i64, i64, usize)> {
    let s = t.strip_prefix('{')?;
    let (mut min, s) = parse_int(s)?;
    let (max, s) = if let Some(rest) = s.strip_prefix(',') {
        if rest.starts_with('}') {
            (-1, rest)
        } else {
            let (max, rest) = parse_int(rest)?;
            if max < 0 {
                min = -1;
            }
            (max, rest)
        }
    } else {
        (min, s)
    };
    let rest = s.strip_prefix('}')?;
    Some((min, max, t.len() - rest.len()))
}

/// Go: `parseInt`: decimal digits without a leading zero, and the rest; -1 once the value
/// reaches 10^8.
fn parse_int(s: &str) -> Option<(i64, &str)> {
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || (digits >= 2 && s.starts_with('0')) {
        return None;
    }
    let (number, rest) = s.split_at_checked(digits)?;
    let mut value = 0_i64;
    for d in number.bytes() {
        if value >= 100_000_000 {
            value = -1;
            break;
        }
        value = value * 10 + i64::from(d - b'0');
    }
    Some((value, rest))
}

/// Writes a non-negative count in decimal.
fn push_decimal(out: &mut String, n: i64) {
    out.push_str(&n.to_string());
}

/// Writes a character so that the regex crate reads it as itself, inside or outside a class:
/// ASCII letters, digits and `_` as they are, every other character as `\x{...}`.
fn push_char(out: &mut String, c: char) {
    if c.is_ascii_alphanumeric() || c == '_' {
        out.push(c);
        return;
    }
    out.push_str(r"\x{");
    let code = u32::from(c);
    let mut started = false;
    for shift in (0..6).rev() {
        let digit = (code >> (shift * 4)) & 0xF;
        if digit != 0 || started || shift == 0 {
            started = true;
            out.push(char::from_digit(digit, 16).map_or('0', |d| d.to_ascii_uppercase()));
        }
    }
    out.push('}');
}

/// Writes a class range.
fn push_range(out: &mut String, lo: char, hi: char) {
    push_char(out, lo);
    if hi != lo {
        out.push('-');
        push_char(out, hi);
    }
}
