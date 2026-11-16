//! Replays `strings.txt` and `strings_sweeps.txt` from `testdata/go/golden/strings.go`.

use super::*;
use crate::{
    golden::{self, Sweep},
    strconv::quote,
    testutil::ensure_eq,
    unicode::{self, MAX_RUNE},
};

/// The `(old, new)` pairs of Go's `replacerCases`, by index.
const REPLACER_CASES: [&[(&str, &str)]; 9] = [
    &[(".", "_"), ("-", "_")],
    &[("a", "1"), ("aa", "2"), ("aaa", "3")],
    &[("aaa", "3"), ("aa", "2"), ("a", "1")],
    &[("a", "x"), ("a", "y")],
    &[("", "X")],
    &[("", "X"), ("b", "B")],
    &[("${AUCTION_PRICE}", "1.23"), ("${AUCTION_ID}", "id")],
    &[("\"", "\\\""), ("\\", "\\\\")],
    &[("\u{e9}", "e"), ("\u{20ac}", "EUR")],
];

/// Go: `quoteAll`.
fn quote_all(parts: &[&str]) -> String {
    let quoted: Vec<String> = parts.iter().map(|p| quote(p)).collect();
    format!("[{}]", quoted.join(","))
}

fn check_split(row: &[String]) -> Result<(), String> {
    let [_, s, sep, n, plain, after] = golden::fields::<6>(row)?;
    let (got_plain, got_after) = if n == "all" {
        (split(&s, &sep), split_after(&s, &sep))
    } else {
        let n: isize = n.parse().map_err(|e| format!("{e}"))?;
        (split_n(&s, &sep, n), split_after_n(&s, &sep, n))
    };
    ensure_eq(
        quote_all(&got_plain).as_str(),
        plain.as_str(),
        &format!("Split {s:?} {sep:?}"),
    )?;
    ensure_eq(
        quote_all(&got_after).as_str(),
        after.as_str(),
        &format!("SplitAfter {s:?} {sep:?}"),
    )
}

#[test]
fn rows_match_go() -> Result<(), String> {
    let rows = golden::rows(include_str!("../../testdata/go/golden/strings.txt"))?;
    for row in &rows {
        match row.first().map(String::as_str) {
            Some("case") => {
                let [_, s, lower, upper] = golden::fields::<4>(row)?;
                ensure_eq(
                    to_lower(&s).as_str(),
                    lower.as_str(),
                    &format!("ToLower {s:?}"),
                )?;
                ensure_eq(
                    to_upper(&s).as_str(),
                    upper.as_str(),
                    &format!("ToUpper {s:?}"),
                )?;
            }
            Some("fold") => {
                let [_, s, t, want] = golden::fields::<4>(row)?;
                let got = equal_fold(&s, &t).to_string();
                ensure_eq(
                    got.as_str(),
                    want.as_str(),
                    &format!("EqualFold {s:?} {t:?}"),
                )?;
            }
            Some("space") => {
                let [_, s, trimmed, joined, count] = golden::fields::<5>(row)?;
                ensure_eq(
                    trim_space(&s),
                    trimmed.as_str(),
                    &format!("TrimSpace {s:?}"),
                )?;
                let f = fields(&s);
                ensure_eq(
                    f.join("|").as_str(),
                    joined.as_str(),
                    &format!("Fields {s:?}"),
                )?;
                ensure_eq(f.len().to_string().as_str(), count.as_str(), "Fields count")?;
            }
            Some("split") => check_split(row)?,
            Some("replace") => {
                let [_, case, s, want] = golden::fields::<4>(row)?;
                let i: usize = case.parse().map_err(|e| format!("{e}"))?;
                let pairs = REPLACER_CASES.get(i).ok_or("replacer case")?;
                let got = Replacer::new(pairs.iter().copied()).replace(&s);
                ensure_eq(got.as_str(), want.as_str(), &format!("Replace #{i} {s:?}"))?;
            }
            other => return Err(format!("unknown row kind {other:?}")),
        }
    }
    ensure_eq(&rows.len(), &(23 + 23 + 13 + 12 * 6 + 9 * 9), "row count")
}

#[test]
fn every_rune_matches_go() -> Result<(), String> {
    let mut sweep = Sweep::default();
    for c in (0..=MAX_RUNE).filter_map(char::from_u32) {
        let s = c.to_string();
        let f = unicode::simple_fold(c).to_string();
        let upper = to_upper(&s);
        sweep.add(&format!(
            "{}\t{upper}\t{}{}{}",
            to_lower(&s),
            equal_fold(&s, &f),
            equal_fold(&format!("{s}a"), &format!("A{f}")),
            equal_fold(&s, &upper),
        ));
    }
    let sums = include_str!("../../testdata/go/golden/strings_sweeps.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "strings_runes")?.as_str(),
        "runes",
    )
}
