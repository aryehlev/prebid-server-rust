//! Go doc comments to rustdoc that rustfmt leaves as written and clippy accepts.
//!
//! A comment whose lines are plain prose becomes Markdown, with identifiers that clippy's
//! `doc_markdown` would flag put in backticks. Anything else (indented lines, lists, Markdown
//! syntax, URLs, lines too long for the width) keeps Go's exact text in a `text` code block,
//! hard-wrapped to the width because rustfmt does not wrap code blocks and refuses long lines.

use core::mem;

/// The line width rustfmt enforces (`max_width` and `comment_width` in `rustfmt.toml`).
pub const WIDTH: usize = 100;

/// Characters that make a line more than plain prose in Markdown (`_` is allowed inside words,
/// which then go in backticks).
const MARKUP: &[char] = &[
    '`', '*', '[', ']', '<', '>', '|', '\\', '~', '#', '&', '{', '}', '$', '^', '=', '@',
];

/// Renders Go doc comment `text` as the `///` lines of an item indented by `indent` spaces.
/// Empty text renders as no lines.
#[must_use]
pub fn render(text: &str, indent: usize) -> Vec<String> {
    let lines = normalize(text);
    if lines.is_empty() {
        return Vec::new();
    }
    let pad = " ".repeat(indent);
    let width = WIDTH.saturating_sub(indent + 4).max(20);
    let body = markdown(&lines, width).unwrap_or_else(|| text_block(&lines, width));
    body.into_iter()
        .map(|line| {
            if line.is_empty() {
                format!("{pad}///")
            } else {
                format!("{pad}/// {line}")
            }
        })
        .collect()
}

/// Joins doc comment parts (a declaration's doc and its trailing comment) into one text.
#[must_use]
pub fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| p.trim_end())
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Lines with tabs expanded, trailing white space removed, and blank lines collapsed and trimmed.
fn normalize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.replace('\t', "    ").trim_end().to_owned();
        if line.is_empty() && out.last().is_none_or(String::is_empty) {
            continue;
        }
        out.push(line);
    }
    let keep = out.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    out.truncate(keep);
    out
}

/// The longest first paragraph kept as Markdown: clippy's `too_long_first_doc_paragraph` flags
/// those over 200 characters.
const FIRST_PARAGRAPH: usize = 180;

/// The lines as Markdown prose, or `None` when they are not plain prose that fits.
fn markdown(lines: &[String], width: usize) -> Option<Vec<String>> {
    let first: usize = lines
        .iter()
        .take_while(|l| !l.is_empty())
        .map(|l| l.len() + 1)
        .sum();
    if first > FIRST_PARAGRAPH {
        return None;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let rendered = prose_line(line)?;
        if rendered.chars().count() > width {
            return None;
        }
        out.push(rendered);
    }
    Some(out)
}

/// One line of plain prose with clippy-flagged words in backticks, or `None` when the line has
/// Markdown meaning.
fn prose_line(line: &str) -> Option<String> {
    if !line.is_ascii()
        || line.starts_with(' ')
        || line.contains(MARKUP)
        || line.contains("://")
        || line.contains("  ")
        || starts_block(line)
    {
        return None;
    }
    let mut out = String::with_capacity(line.len() + 8);
    for (i, word) in line.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        for (j, piece) in word.split('\'').enumerate() {
            if j > 0 {
                out.push('\'');
            }
            out.push_str(&code_span(piece)?);
        }
    }
    Some(out)
}

/// Whether a line would start a Markdown block other than a paragraph (list item, quote,
/// heading, rule, numbered item).
fn starts_block(line: &str) -> bool {
    if line.starts_with(['-', '+', '>', '#', '*', '=', '_']) {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && line
            .get(digits..)
            .is_some_and(|rest| rest.starts_with(['.', ')']))
}

/// A word (ASCII) with the part clippy's `doc_markdown` checks put in backticks when it would be
/// flagged; `None` when the word cannot be made safe.
fn code_span(piece: &str) -> Option<String> {
    let flagged = |c: char| !c.is_ascii_alphanumeric() && c != ':';
    let mut end = piece.trim_end_matches(flagged).len();
    if piece.get(end..end + 2) == Some("()") {
        end += 2;
    }
    let head = piece.get(..end)?;
    let mut start = end - head.trim_start_matches(flagged).len();
    let lead = piece.get(start..end)?;
    if lead.starts_with(':') && !lead.starts_with("::") {
        start += lead.len() - lead.trim_start_matches(':').len();
    }
    let trail = piece.get(start..end)?;
    if trail.ends_with(':') && !trail.ends_with("::") {
        end -= trail.len() - trail.trim_end_matches(':').len();
    }
    let (before, core, after) = (
        piece.get(..start)?,
        piece.get(start..end)?,
        piece.get(end..)?,
    );
    if before.contains('_') || after.contains('_') {
        return None;
    }
    let needs =
        core.contains('_') || core.contains("::") || core.ends_with("()") || is_camel_case(core);
    Some(if needs {
        format!("{before}`{core}`{after}")
    } else {
        piece.to_owned()
    })
}

/// clippy's `doc_markdown` camel-case test: starts upper-case, has at least two upper-case and
/// one lower-case letter (a trailing `s` aside), and nothing but letters and digits.
fn is_camel_case(s: &str) -> bool {
    if s.starts_with(|c: char| c.is_ascii_digit() || c.is_ascii_lowercase()) {
        return false;
    }
    let s = s.strip_suffix('s').unwrap_or(s);
    s.chars().all(char::is_alphanumeric)
        && s.chars().filter(|c| c.is_uppercase()).count() > 1
        && s.chars().any(char::is_lowercase)
}

/// The lines verbatim in a `text` code block, wrapped to the width.
fn text_block(lines: &[String], width: usize) -> Vec<String> {
    let longest_run = lines
        .iter()
        .map(|l| l.trim_start().chars().take_while(|&c| c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    let mut out = vec![format!("{fence}text")];
    for line in lines {
        out.extend(wrap(line, width));
    }
    out.push(fence);
    out
}

/// Wraps one line at spaces to the width (continuation lines keep its indentation), splitting
/// words longer than the width.
fn wrap(line: &str, width: usize) -> Vec<String> {
    if line.chars().count() <= width {
        return vec![line.to_owned()];
    }
    let indent_len = line.len() - line.trim_start().len();
    let indent = " ".repeat(indent_len.min(width / 2));
    let room = width - indent.len();
    let mut out = Vec::new();
    let mut current = String::new();
    for word in line.split(' ').filter(|w| !w.is_empty()) {
        let mut word: Vec<char> = word.chars().collect();
        while word.len() > room {
            if !current.is_empty() {
                out.push(mem::take(&mut current));
            }
            let rest = word.split_off(room);
            out.push(format!("{indent}{}", word.iter().collect::<String>()));
            word = rest;
        }
        let word: String = word.into_iter().collect();
        if current.is_empty() {
            current = format!("{indent}{word}");
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(&word);
        } else {
            out.push(mem::replace(&mut current, format!("{indent}{word}")));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{WIDTH, join, render};
    use crate::{
        error::Result,
        testutil::{check, check_eq},
    };

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn renders_prose_as_markdown() -> Result<()> {
        check_eq(
            &render("Unknown\n", 0),
            &lines(&["/// Unknown"]),
            "one word",
        )?;
        check_eq(
            &render(
                "PlacementPosition represents positions.\n\nSee the IAB's spec (OpenRTB 2.6).\n",
                4,
            ),
            &lines(&[
                "    /// `PlacementPosition` represents positions.",
                "    ///",
                "    /// See the IAB's spec (`OpenRTB` 2.6).",
            ]),
            "camel case and paragraphs",
        )?;
        check_eq(
            &render("set us_privacy (see foo::bar) or call Ptr().", 0),
            &lines(&["/// set `us_privacy` (see `foo::bar`) or call `Ptr()`."]),
            "underscores, paths and calls",
        )?;
        check_eq(
            &render("Note: IDs are 10.", 0),
            &lines(&["/// Note: IDs are 10."]),
            "plural",
        )?;
        check_eq(&render("", 0), &Vec::new(), "empty")
    }

    #[test]
    fn keeps_everything_else_in_a_text_block() -> Result<()> {
        check_eq(
            &render("Attribute:\n  id\nType:\n  string; required\n", 4),
            &lines(&[
                "    /// ```text",
                "    /// Attribute:",
                "    ///   id",
                "    /// Type:",
                "    ///   string; required",
                "    /// ```",
            ]),
            "indented lines",
        )?;
        check_eq(
            &render("1. first\n\n\n- see https://x.org/a_b\n", 0),
            &lines(&[
                "/// ```text",
                "/// 1. first",
                "///",
                "/// - see https://x.org/a_b",
                "/// ```",
            ]),
            "lists and URLs",
        )?;
        let long = format!("  {}", "word ".repeat(40));
        let out = render(&long, 4);
        check(out.iter().all(|l| l.len() <= WIDTH), "wrapped to the width")?;
        check_eq(
            &out.get(1).map(|l| l.starts_with("    ///   word")),
            &Some(true),
            "indent kept",
        )?;
        let token = "x".repeat(150);
        let split = render(&token, 0);
        check(split.iter().all(|l| l.len() <= WIDTH), "long word split")?;
        check_eq(&split.len(), &4, "split in two, between the text fences")
    }

    #[test]
    fn joins_parts() -> Result<()> {
        check_eq(
            join(&["Doc.\n", "", "trailing\n"]).as_str(),
            "Doc.\n\ntrailing",
            "join",
        )
    }
}
