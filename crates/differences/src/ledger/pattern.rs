//! Path patterns: the parity harness's JSON paths, with wildcards.
//!
//! A path starts at `$`, the compared value. `.name` is a member whose name is an identifier
//! (`[A-Za-z_][A-Za-z0-9_]*`), `['name']` any other member, with `\` and `'` escaped as `\\` and
//! `\'` (meta members such as `['@status']` and `['@headers']` are always bracketed), and `[*]`
//! any array index: the harness writes every index of a mismatch's path as `[*]` before it
//! matches. A pattern may also hold `.*`, any one member, and `.**`, any run of members and
//! indices, the empty run included. A pattern is written in the harness's own spelling, so it
//! matches the paths the harness writes character for character.

use std::{collections::BTreeSet, fmt};

/// One step of a path pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// The member with this name.
    Member(String),
    /// Any one member: `.*`.
    AnyMember,
    /// Any array index: `[*]`.
    AnyIndex,
    /// Any run of members and indices, the empty run included: `.**`.
    AnyDepth,
}

impl Segment {
    /// Whether the segment steps into an object member.
    const fn is_member(&self) -> bool {
        matches!(self, Self::Member(_) | Self::AnyMember)
    }

    /// Whether some single step of a concrete path is matched by both segments.
    fn meets(&self, other: &Self) -> bool {
        if *self == Self::AnyDepth || *other == Self::AnyDepth {
            return true;
        }
        if let (Self::Member(a), Self::Member(b)) = (self, other) {
            return a == b;
        }
        self.is_member() == other.is_member()
    }
}

/// A path pattern, parsed from its canonical spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PathPattern {
    /// The steps after `$`.
    segments: Vec<Segment>,
}

impl PathPattern {
    /// Parses `text`, which must be in the harness's spelling.
    ///
    /// # Errors
    ///
    /// When `text` is not a path pattern, or names a member in another spelling than the
    /// harness's (`['id']` for `.id`).
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let mut rest = text
            .strip_prefix('$')
            .ok_or_else(|| "a path starts with $".to_owned())?;
        let mut segments = Vec::new();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix(".**") {
                if segments.last() == Some(&Segment::AnyDepth) {
                    return Err(".** follows .**".to_owned());
                }
                segments.push(Segment::AnyDepth);
                rest = after;
            } else if let Some(after) = rest.strip_prefix(".*") {
                segments.push(Segment::AnyMember);
                rest = after;
            } else if let Some(after) = rest.strip_prefix('.') {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                let (name, tail) = after
                    .split_at_checked(end)
                    .ok_or_else(|| "a member name that is not valid text".to_owned())?;
                if !is_identifier(name) {
                    return Err(format!(
                        "member {name:?} after a dot is not an identifier: write it ['...']"
                    ));
                }
                segments.push(Segment::Member(name.to_owned()));
                rest = tail;
            } else if let Some(after) = rest.strip_prefix("[*]") {
                segments.push(Segment::AnyIndex);
                rest = after;
            } else if let Some(after) = rest.strip_prefix("['") {
                let (name, tail) = bracketed(after)?;
                segments.push(Segment::Member(name));
                rest = tail;
            } else if rest.starts_with('[') {
                return Err("an index is written [*]: the harness matches every index".to_owned());
            } else {
                return Err(format!("unexpected {rest:?}"));
            }
        }
        let pattern = Self { segments };
        let canonical = pattern.to_string();
        if canonical == text {
            Ok(pattern)
        } else {
            Err(format!("write it {canonical}, as the harness spells it"))
        }
    }

    /// Whether the pattern matches exactly one path once indices are written `[*]`: it has no
    /// `.*` and no `.**`.
    pub(super) fn is_concrete(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| matches!(segment, Segment::Member(_) | Segment::AnyIndex))
    }

    /// Whether some concrete path matches both patterns.
    pub(super) fn overlaps(&self, other: &Self) -> bool {
        // A walk over pairs of positions: `.**` may match nothing (move past it) or one more
        // step (stay on it); any other segment matches exactly one step.
        let mut seen = BTreeSet::new();
        let mut todo = vec![(0_usize, 0_usize)];
        while let Some((i, j)) = todo.pop() {
            if !seen.insert((i, j)) {
                continue;
            }
            let (left, right) = (self.segments.get(i), other.segments.get(j));
            let (Some(a), Some(b)) = (left, right) else {
                if left.is_none() && right.is_none() {
                    return true;
                }
                if left == Some(&Segment::AnyDepth) {
                    todo.push((i + 1, j));
                }
                if right == Some(&Segment::AnyDepth) {
                    todo.push((i, j + 1));
                }
                continue;
            };
            if *a == Segment::AnyDepth {
                todo.push((i + 1, j));
            }
            if *b == Segment::AnyDepth {
                todo.push((i, j + 1));
            }
            if a.meets(b) {
                let next_i = if *a == Segment::AnyDepth { i } else { i + 1 };
                let next_j = if *b == Segment::AnyDepth { j } else { j + 1 };
                todo.push((next_i, next_j));
            }
        }
        false
    }
}

impl fmt::Display for PathPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("$")?;
        for segment in &self.segments {
            match segment {
                Segment::Member(name) if is_identifier(name) => write!(f, ".{name}")?,
                Segment::Member(name) => {
                    write!(f, "['{}']", name.replace('\\', "\\\\").replace('\'', "\\'"))?;
                }
                Segment::AnyMember => f.write_str(".*")?,
                Segment::AnyIndex => f.write_str("[*]")?,
                Segment::AnyDepth => f.write_str(".**")?,
            }
        }
        Ok(())
    }
}

/// Whether `name` is written after a dot: `[A-Za-z_][A-Za-z0-9_]*`, as the harness's `child`
/// decides.
pub(super) fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Reads a bracketed member name after `['`, up to its closing `']`, and returns it with the
/// rest of the path.
fn bracketed(text: &str) -> Result<(String, &str), String> {
    let mut name = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some((_, escaped @ ('\\' | '\''))) => name.push(escaped),
                Some(_) | None => {
                    return Err("in ['...'] only \\\\ and \\' are escapes".to_owned());
                }
            },
            '\'' => {
                let after = text.get(at + 1..).unwrap_or_default();
                return match after.strip_prefix(']') {
                    Some(tail) => Ok((name, tail)),
                    None if after.is_empty() => Err("['... has no closing ']".to_owned()),
                    None => Err("a ' inside ['...'] is written \\'".to_owned()),
                };
            }
            other => name.push(other),
        }
    }
    Err("['... has no closing ']".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{PathPattern, is_identifier};
    use crate::{BoxError, TestResult, check, check_eq};

    fn pattern(text: &str) -> Result<PathPattern, BoxError> {
        PathPattern::parse(text).map_err(|e| format!("{text}: {e}").into())
    }

    #[test]
    fn spellings_of_the_harness_parse() -> TestResult {
        for text in [
            "$",
            "$['@status']",
            "$['@headers']['Content-Type']",
            "$['@headers'].Vary",
            "$[*].request.headers.Accept",
            "$.seatbid[*].bid[*].adm",
            "$['a b']['it\\'s']['back\\\\slash']",
            "$['']",
            "$['*']",
            "$.**",
            "$.ext.*.price",
            "$.**.ext",
        ] {
            check_eq(pattern(text)?.to_string().as_str(), text, "round trip")?;
        }
        Ok(())
    }

    #[test]
    fn other_spellings_fail() -> TestResult {
        for (text, why) in [
            ("", "starts with $"),
            ("seatbid", "starts with $"),
            ("$['id']", "write it $.id"),
            ("$.Content-Type", "not an identifier"),
            ("$.imp[0]", "is written [*]"),
            ("$['@status'", "no closing"),
            ("$['it's']", "is written \\'"),
            ("$['a\\b']", "only"),
            ("$.**.**", "follows"),
            ("$.***", "unexpected"),
            ("$.", "not an identifier"),
            ("$..x", "not an identifier"),
            ("$x", "unexpected"),
        ] {
            let error = PathPattern::parse(text)
                .err()
                .ok_or_else(|| format!("{text:?} parsed"))?;
            check(
                error.contains(why),
                &format!("{text:?}: {error:?} should say {why:?}"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn concrete_paths() -> TestResult {
        check(pattern("$['@status']")?.is_concrete(), "a meta member")?;
        check(
            pattern("$[*].request.headers.Accept")?.is_concrete(),
            "indices",
        )?;
        check(!pattern("$.**")?.is_concrete(), "any depth")?;
        check(!pattern("$.ext.*")?.is_concrete(), "any member")?;
        check(pattern("$['*']")?.is_concrete(), "a member named *")
    }

    #[test]
    fn overlaps() -> TestResult {
        for (a, b, both) in [
            ("$.**", "$['@status']", true),
            ("$.**", "$", true),
            ("$['@status']", "$['@status']", true),
            ("$['@status']", "$['@headers']", false),
            ("$.a.*", "$.a.b", true),
            ("$.a.*", "$.a[*]", false),
            ("$.a[*]", "$.a[*]", true),
            ("$.**.price", "$.seatbid[*].bid[*].price", true),
            ("$.**.price", "$.seatbid[*].bid[*].adm", false),
            ("$.a.**.b", "$.**.c", false),
            ("$.a.**.b", "$.**.b", true),
            ("$.a.**", "$.b.**", false),
            ("$.**.a.**", "$.**.b.**", true),
            ("$.*", "$", false),
            ("$['*']", "$.x", false),
        ] {
            let (left, right) = (pattern(a)?, pattern(b)?);
            check_eq(&left.overlaps(&right), &both, &format!("{a} and {b}"))?;
            check_eq(&right.overlaps(&left), &both, &format!("{b} and {a}"))?;
        }
        Ok(())
    }

    #[test]
    fn identifiers() -> TestResult {
        check(is_identifier("Accept"), "a header name")?;
        check(is_identifier("_x9"), "underscore and digits")?;
        check(!is_identifier("9x"), "a leading digit")?;
        check(!is_identifier("Content-Type"), "a hyphen")?;
        check(!is_identifier(""), "empty")?;
        check(!is_identifier("@status"), "a meta member")
    }
}
