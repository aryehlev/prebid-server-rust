//! Go: `strings`, for the functions whose Rust counterparts behave differently.
//!
//! - [`to_lower`] / [`to_upper`]: Go maps each rune with the simple case mapping of
//!   `unicode.ToLower`/`ToUpper` (`"İ"` lowers to `"i"`, `"ß"` stays `"ß"` when upper-cased);
//!   Rust's `to_lowercase`/`to_uppercase` apply full, context-sensitive mappings.
//! - [`equal_fold`]: Unicode simple case folding (`unicode.SimpleFold` orbits).
//! - [`trim_space`] and [`fields`]: Go's `unicode.IsSpace`.
//! - [`split`], [`split_n`], [`split_after`], [`split_after_n`]: Go's `n` handling and the
//!   explode-into-runes rule for an empty separator (Rust's `split("")` yields empty strings at
//!   both ends; `split_inclusive` drops the trailing empty field that `SplitAfter` keeps).
//! - [`Replacer`]: `strings.NewReplacer` (at each position the first pair, in argument order, whose
//!   old string matches; no overlapping matches).
//!
//! Functions with identical behaviour (`Contains`, `HasPrefix`, `Index`, `Join`, `Replace`,
//! `ReplaceAll`, `TrimPrefix`, `TrimSuffix`, `Trim` with a cutset, ...) map directly onto `str`.

use crate::{unicode, utf8::lossy_string};

/// Go: `strings.ToLower`.
#[must_use]
pub fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars().map(unicode::to_lower).collect()
}

/// Go: `strings.ToUpper`.
#[must_use]
pub fn to_upper(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_uppercase();
    }
    s.chars().map(unicode::to_upper).collect()
}

/// Go: `strings.EqualFold`: equality under simple Unicode case folding.
#[must_use]
pub fn equal_fold(s: &str, t: &str) -> bool {
    let mut t_chars = t.chars();
    for sr in s.chars() {
        // If t is exhausted the strings are not equal.
        let Some(tr) = t_chars.next() else {
            return false;
        };
        if tr == sr {
            continue;
        }
        // Make sr < tr to simplify what follows.
        let (sr, tr) = if tr < sr { (tr, sr) } else { (sr, tr) };
        // Fast check for ASCII.
        if tr.is_ascii() {
            // ASCII only, sr/tr must be upper/lower case
            if sr.is_ascii_uppercase() && sr.to_ascii_lowercase() == tr {
                continue;
            }
            return false;
        }
        // General case: SimpleFold(x) returns the next equivalent rune > x or wraps around.
        let mut r = unicode::simple_fold(sr);
        while r != sr && r < tr {
            r = unicode::simple_fold(r);
        }
        if r != tr {
            return false;
        }
    }
    t_chars.next().is_none()
}

/// Go: `strings.TrimSpace`: removes leading and trailing `unicode.IsSpace` runes.
#[must_use]
pub fn trim_space(s: &str) -> &str {
    s.trim_matches(unicode::is_space)
}

/// Go: `strings.Fields`: the non-empty runs between `unicode.IsSpace` runes.
#[must_use]
pub fn fields(s: &str) -> Vec<&str> {
    s.split(unicode::is_space)
        .filter(|f| !f.is_empty())
        .collect()
}

/// Go: `explode`: at most `n` single-rune strings (`n < 0`: all), the last holding the rest.
fn explode(s: &str, n: isize) -> Vec<&str> {
    let runes = s.chars().count();
    let n = usize::try_from(n).map_or(runes, |n| n.min(runes));
    let mut out = Vec::with_capacity(n);
    let mut rest = s;
    for _ in 1..n {
        let size = rest.chars().next().map_or(0, char::len_utf8);
        let (head, tail) = rest.split_at(size);
        out.push(head);
        rest = tail;
    }
    if n > 0 {
        out.push(rest);
    }
    out
}

/// Go: `genSplit`: splits after each `sep` (keeping `save` bytes of it) into at most `n`
/// parts (`n < 0`: all, `n == 0`: none).
fn gen_split<'a>(s: &'a str, sep: &str, keep_sep: bool, n: isize) -> Vec<&'a str> {
    if n == 0 {
        return Vec::new();
    }
    if sep.is_empty() {
        return explode(s, n);
    }
    let limit = usize::try_from(n).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    let mut rest = s;
    while out.len() + 1 < limit {
        let Some(m) = rest.find(sep) else {
            break;
        };
        let end = m + sep.len();
        out.push(
            rest.get(..if keep_sep { end } else { m })
                .unwrap_or_default(),
        );
        rest = rest.get(end..).unwrap_or_default();
    }
    out.push(rest);
    out
}

/// Go: `strings.Split`.
#[must_use]
pub fn split<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    gen_split(s, sep, false, -1)
}

/// Go: `strings.SplitN`.
#[must_use]
pub fn split_n<'a>(s: &'a str, sep: &str, n: isize) -> Vec<&'a str> {
    gen_split(s, sep, false, n)
}

/// Go: `strings.SplitAfter`.
#[must_use]
pub fn split_after<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    gen_split(s, sep, true, -1)
}

/// Go: `strings.SplitAfterN`.
#[must_use]
pub fn split_after_n<'a>(s: &'a str, sep: &str, n: isize) -> Vec<&'a str> {
    gen_split(s, sep, true, n)
}

/// Go: `strings.Replacer` from `strings.NewReplacer(old1, new1, old2, new2, ...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacer {
    pairs: Vec<(String, String)>,
}

impl Replacer {
    /// Go: `strings.NewReplacer` with the `(old, new)` pairs in argument order. Go panics on an
    /// odd argument count, which a list of pairs cannot express.
    #[must_use]
    pub fn new<O, N, I>(pairs: I) -> Self
    where
        O: Into<String>,
        N: Into<String>,
        I: IntoIterator<Item = (O, N)>,
    {
        Self {
            pairs: pairs
                .into_iter()
                .map(|(o, n)| (o.into(), n.into()))
                .collect(),
        }
    }

    /// Go: `Replacer.Replace`: at each position, the first pair (in argument order) whose old
    /// string starts there is replaced and matching resumes after it. An empty old string
    /// matches between bytes, as in Go; text this splits inside a UTF-8 sequence comes back with
    /// each invalid byte as `U+FFFD`.
    #[must_use]
    pub fn replace(&self, s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        let mut prev_match_empty = false;
        while i <= bytes.len() {
            let rest = bytes.get(i..).unwrap_or_default();
            // The empty match is ignored right after an empty match.
            let found = self.pairs.iter().find(|(old, _)| {
                rest.starts_with(old.as_bytes()) && !(prev_match_empty && old.is_empty())
            });
            prev_match_empty = found.is_some_and(|(old, _)| old.is_empty());
            if let Some((old, new)) = found {
                out.extend_from_slice(new.as_bytes());
                i += old.len();
            } else {
                if let Some(&b) = bytes.get(i) {
                    out.push(b);
                }
                i += 1;
            }
        }
        String::from_utf8(out).unwrap_or_else(|e| lossy_string(e.as_bytes()))
    }
}

#[cfg(test)]
mod tests;
