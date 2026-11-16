//! A Go tokenizer.
//!
//! It follows the lexical rules of the Go specification closely enough to read the adapter test
//! files: comments, identifiers, all literal forms, operators, and automatic semicolon insertion.

use std::str::Chars;

use crate::error::{Result, ToolError};

/// The kind of a [`Token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An identifier (keywords included; compare the text to tell them apart).
    Ident,
    /// An integer literal.
    Int,
    /// A floating-point literal.
    Float,
    /// An imaginary literal.
    Imag,
    /// A rune literal.
    Char,
    /// An interpreted string literal (`"..."`).
    String,
    /// A raw string literal (`` `...` ``).
    RawString,
    /// An operator or punctuation.
    Op,
    /// A semicolon, written or inserted at a line end.
    Semi,
}

/// One token with its byte span in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The token kind.
    pub kind: Kind,
    /// The source text (empty for an inserted semicolon).
    pub text: String,
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset just past the last byte.
    pub end: usize,
}

impl Token {
    /// Whether the token is the operator or keyword `s`.
    #[must_use]
    pub fn is(&self, s: &str) -> bool {
        matches!(self.kind, Kind::Op | Kind::Ident) && self.text == s
    }

    /// Whether the token is an identifier (not a keyword).
    #[must_use]
    pub fn is_ident(&self) -> bool {
        self.kind == Kind::Ident && !KEYWORDS.contains(&self.text.as_str())
    }
}

const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

// Longest operators first so a prefix scan finds the longest match.
const OPERATORS: &[&str] = &[
    "<<=", ">>=", "&^=", "...", "&&", "||", "<-", "++", "--", "==", "!=", "<=", ">=", ":=", "+=",
    "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "&^", "+", "-", "*", "/", "%", "&", "|",
    "^", "<", ">", "=", "!", "~", "(", ")", "[", "]", "{", "}", ",", ";", ".", ":",
];

/// Splits Go source into tokens, inserting semicolons as the Go specification describes.
///
/// # Errors
///
/// Fails on an unterminated comment or literal, or a character Go does not allow.
pub fn tokenize(src: &str) -> Result<Vec<Token>> {
    Lexer {
        src,
        pos: 0,
        out: Vec::new(),
    }
    .run()
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    out: Vec<Token>,
}

impl Lexer<'_> {
    fn run(mut self) -> Result<Vec<Token>> {
        while let Some(c) = self.peek() {
            let start = self.pos;
            match c {
                '\n' => {
                    self.insert_semi(start);
                    self.pos += 1;
                }
                ' ' | '\t' | '\r' => self.pos += 1,
                '/' if self.rest().starts_with("//") => {
                    let len = self.rest().find('\n').unwrap_or_else(|| self.rest().len());
                    self.pos += len;
                }
                '/' if self.rest().starts_with("/*") => {
                    let Some(len) = self.rest().find("*/") else {
                        return Err(self.err("unterminated block comment"));
                    };
                    let body = self.rest().get(..len).unwrap_or_default();
                    if body.contains('\n') {
                        self.insert_semi(start);
                    }
                    self.pos += len + 2;
                }
                '"' => self.quoted('"', Kind::String)?,
                '\'' => self.quoted('\'', Kind::Char)?,
                '`' => {
                    let Some(len) = self.rest().get(1..).and_then(|r| r.find('`')) else {
                        return Err(self.err("unterminated raw string"));
                    };
                    self.pos += len + 2;
                    self.push(Kind::RawString, start);
                }
                c if c.is_ascii_digit() => self.number(),
                '.' if self
                    .rest()
                    .get(1..2)
                    .is_some_and(|d| d.as_bytes().iter().all(u8::is_ascii_digit)) =>
                {
                    self.number();
                }
                c if c == '_' || c.is_alphabetic() => {
                    let len = self
                        .rest()
                        .find(|ch: char| !(ch == '_' || ch.is_alphanumeric()))
                        .unwrap_or_else(|| self.rest().len());
                    self.pos += len;
                    self.push(Kind::Ident, start);
                }
                _ => {
                    let Some(op) = OPERATORS.iter().find(|op| self.rest().starts_with(**op)) else {
                        return Err(self.err(&format!("unexpected character {c:?}")));
                    };
                    self.pos += op.len();
                    let kind = if *op == ";" { Kind::Semi } else { Kind::Op };
                    self.push(kind, start);
                }
            }
        }
        self.insert_semi(self.pos);
        Ok(self.out)
    }

    fn rest(&self) -> &str {
        self.src.get(self.pos..).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn err(&self, msg: &str) -> ToolError {
        let line = self
            .src
            .get(..self.pos)
            .unwrap_or_default()
            .matches('\n')
            .count()
            + 1;
        ToolError::new(format!("line {line}: {msg}"))
    }

    fn push(&mut self, kind: Kind, start: usize) {
        let text = self.src.get(start..self.pos).unwrap_or_default().to_owned();
        self.out.push(Token {
            kind,
            text,
            start,
            end: self.pos,
        });
    }

    // Go spec "Semicolons": after a line's final token if it is an identifier, a literal, one of
    // the keywords break/continue/fallthrough/return, or one of ++ -- ) ] }.
    fn insert_semi(&mut self, at: usize) {
        let needs = self.out.last().is_some_and(|t| match t.kind {
            Kind::Ident => {
                !KEYWORDS.contains(&t.text.as_str())
                    || matches!(
                        t.text.as_str(),
                        "break" | "continue" | "fallthrough" | "return"
                    )
            }
            Kind::Int | Kind::Float | Kind::Imag | Kind::Char | Kind::String | Kind::RawString => {
                true
            }
            Kind::Op => matches!(t.text.as_str(), "++" | "--" | ")" | "]" | "}"),
            Kind::Semi => false,
        });
        if needs {
            self.out.push(Token {
                kind: Kind::Semi,
                text: String::new(),
                start: at,
                end: at,
            });
        }
    }

    fn quoted(&mut self, quote: char, kind: Kind) -> Result<()> {
        let start = self.pos;
        let mut chars = self.rest().char_indices().skip(1);
        loop {
            match chars.next() {
                Some((_, '\\')) => {
                    if chars.next().is_none() {
                        break;
                    }
                }
                Some((i, c)) if c == quote => {
                    self.pos += i + c.len_utf8();
                    self.push(kind, start);
                    return Ok(());
                }
                Some((_, '\n')) | None => break,
                Some(_) => {}
            }
        }
        Err(self.err("unterminated literal"))
    }

    fn number(&mut self) {
        let start = self.pos;
        let rest = self.rest();
        let hex = rest.starts_with("0x") || rest.starts_with("0X");
        let mut len = 0;
        let mut prev = '\0';
        for c in rest.chars() {
            let exponent_sign = matches!(c, '+' | '-')
                && if hex {
                    matches!(prev, 'p' | 'P')
                } else {
                    matches!(prev, 'e' | 'E')
                };
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || exponent_sign) {
                break;
            }
            len += c.len_utf8();
            prev = c;
        }
        let text = rest.get(..len).unwrap_or_default();
        let kind = if text.ends_with('i') {
            Kind::Imag
        } else if text.contains('.')
            || (hex && text.contains(['p', 'P']))
            || (!hex && text.contains(['e', 'E']))
        {
            Kind::Float
        } else {
            Kind::Int
        };
        self.pos += len;
        self.push(kind, start);
    }
}

/// Decodes a Go string literal (interpreted or raw) the way `strconv.Unquote` does.
///
/// # Errors
///
/// Fails on an invalid escape or when the decoded bytes are not UTF-8.
pub fn unquote(lit: &str) -> Result<String> {
    if let Some(body) = lit.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
        // Go spec: carriage returns inside raw string literals are discarded.
        return Ok(body.replace('\r', ""));
    }
    let Some(body) = lit.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Err(ToolError::new(format!("not a string literal: {lit}")));
    };
    let bytes = unescape(body, '"')?;
    String::from_utf8(bytes)
        .map_err(|e| ToolError::new(format!("string literal {lit} is not UTF-8: {e}")))
}

fn unescape(body: &str, quote: char) -> Result<Vec<u8>> {
    let bad = || ToolError::new(format!("invalid escape in literal: {body}"));
    let mut out = Vec::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let esc = chars.next().ok_or_else(bad)?;
        let simple = match esc {
            'a' => Some(0x07),
            'b' => Some(0x08),
            'f' => Some(0x0C),
            'n' => Some(b'\n'),
            'r' => Some(b'\r'),
            't' => Some(b'\t'),
            'v' => Some(0x0B),
            '\\' => Some(b'\\'),
            q if q == quote => u8::try_from(q).ok(),
            _ => None,
        };
        if let Some(b) = simple {
            out.push(b);
            continue;
        }
        match esc {
            'x' => out.push(
                read_radix(&mut chars, 2, 16)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or_else(bad)?,
            ),
            '0'..='7' => {
                let rest: String = chars.by_ref().take(2).collect();
                let digits = format!("{esc}{rest}");
                let v = u32::from_str_radix(&digits, 8)
                    .ok()
                    .filter(|_| digits.len() == 3);
                out.push(v.and_then(|v| u8::try_from(v).ok()).ok_or_else(bad)?);
            }
            'u' | 'U' => {
                let n = if esc == 'u' { 4 } else { 8 };
                let ch = read_radix(&mut chars, n, 16)
                    .and_then(char::from_u32)
                    .ok_or_else(bad)?;
                let mut buf = [0; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            _ => return Err(bad()),
        }
    }
    Ok(out)
}

fn read_radix(chars: &mut Chars<'_>, n: usize, radix: u32) -> Option<u32> {
    let digits: String = chars.by_ref().take(n).collect();
    if digits.len() != n {
        return None;
    }
    u32::from_str_radix(&digits, radix).ok()
}

/// Parses a Go integer literal (decimal, `0x`, `0o`, legacy `0` octal, `0b`, with `_`
/// separators).
#[must_use]
pub fn parse_int(lit: &str) -> Option<i64> {
    let clean: String = lit.chars().filter(|c| *c != '_').collect();
    let lower = clean.to_ascii_lowercase();
    let (digits, radix) = if let Some(d) = lower.strip_prefix("0x") {
        (d.to_owned(), 16)
    } else if let Some(d) = lower.strip_prefix("0o") {
        (d.to_owned(), 8)
    } else if let Some(d) = lower.strip_prefix("0b") {
        (d.to_owned(), 2)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (lower.get(1..)?.to_owned(), 8)
    } else {
        (lower, 10)
    };
    i64::from_str_radix(&digits, radix).ok()
}

#[cfg(test)]
mod tests {
    use super::{Kind, parse_int, tokenize, unquote};
    use crate::{
        error::Result,
        testutil::{check, check_eq},
    };

    fn kinds_and_text(src: &str) -> Result<Vec<(Kind, String)>> {
        Ok(tokenize(src)?
            .into_iter()
            .map(|t| (t.kind, t.text))
            .collect())
    }

    #[test]
    fn inserts_semicolons_like_go() -> Result<()> {
        let toks = kinds_and_text("x := f(a,\n b)\nreturn\n}\n")?;
        let texts: Vec<&str> = toks.iter().map(|(_, t)| t.as_str()).collect();
        check_eq(
            texts.as_slice(),
            &[
                "x", ":=", "f", "(", "a", ",", "b", ")", "", "return", "", "}", "",
            ][..],
            "tokens",
        )
    }

    #[test]
    fn block_comment_with_newline_acts_as_newline() -> Result<()> {
        let toks = kinds_and_text("a /* x\n */ b // c\n")?;
        let kinds: Vec<Kind> = toks.iter().map(|(k, _)| *k).collect();
        check_eq(
            kinds.as_slice(),
            &[Kind::Ident, Kind::Semi, Kind::Ident, Kind::Semi][..],
            "kinds",
        )
    }

    #[test]
    fn literals() -> Result<()> {
        let toks = kinds_and_text(r#"1 0x1e+2 1e+2 .5 3i 'a' "s\"t" `r`"#)?;
        let kinds: Vec<Kind> = toks.iter().map(|(k, _)| *k).collect();
        let want = [
            Kind::Int,
            Kind::Int,
            Kind::Op,
            Kind::Int,
            Kind::Float,
            Kind::Float,
            Kind::Imag,
            Kind::Char,
            Kind::String,
            Kind::RawString,
            Kind::Semi,
        ];
        check_eq(kinds.as_slice(), &want[..], "kinds")
    }

    // Expected values checked against Go's strconv.Unquote.
    #[test]
    fn unquote_matches_go() -> Result<()> {
        check_eq(unquote(r#""a\tb\"c\\""#)?.as_str(), "a\tb\"c\\", "escapes")?;
        let numeric = concat!(r#""\x41\101\"#, "u00e9", r#"\U0001F600""#);
        check_eq(
            unquote(numeric)?.as_str(),
            "AA\u{e9}\u{1F600}",
            "numeric escapes",
        )?;
        check_eq(
            unquote("`a\r\nb\\n`")?.as_str(),
            "a\nb\\n",
            "raw drops CR, keeps backslash",
        )?;
        check(unquote(r#""\q""#).is_err(), "bad escape")?;
        check(unquote(r#""\xff""#).is_err(), "invalid UTF-8 is rejected")
    }

    #[test]
    fn integers() -> Result<()> {
        check_eq(&parse_int("1_000"), &Some(1000), "underscores")?;
        check_eq(&parse_int("0x1F"), &Some(31), "hex")?;
        check_eq(&parse_int("017"), &Some(15), "legacy octal")?;
        check_eq(&parse_int("0o17"), &Some(15), "octal")?;
        check_eq(&parse_int("0b101"), &Some(5), "binary")?;
        check_eq(&parse_int("0"), &Some(0), "zero")
    }
}
