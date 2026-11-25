//! The glue against Go's `strings.ToLower` of every rune (`testdata/golden/to_lower.txt`, which
//! `testdata/go` writes with go1.26.2).

use std::{collections::BTreeMap, error::Error, fs};

use super::to_lower;

type TestResult = Result<(), Box<dyn Error>>;

/// Go's `strings.ToLower` of every rune it changes.
fn go_table() -> Result<BTreeMap<char, char>, Box<dyn Error>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/to_lower.txt");
    let rune = |hex: &str| -> Result<char, Box<dyn Error>> {
        char::from_u32(u32::from_str_radix(hex, 16)?)
            .ok_or_else(|| format!("{hex} is not a rune").into())
    };
    let mut table = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let (from, to) = line.split_once(' ').ok_or("malformed line")?;
        if table.insert(rune(from)?, rune(to)?).is_some() {
            return Err(format!("{from} is listed twice").into());
        }
    }
    Ok(table)
}

/// Go's lower case of `c`.
fn go_lower(table: &BTreeMap<char, char>, c: char) -> char {
    table.get(&c).copied().unwrap_or(c)
}

/// std's `str::to_lowercase` maps U+0130 to two characters and a word-final sigma to `ς`; Go's
/// `strings.ToLower` maps each character by itself, and so does the glue (ledger:
/// `metrics.label-case`).
// Go: metrics/prometheus/prometheus.go:860 and the other strings.ToLower(labels.Adapter) calls
#[test]
fn std_lowercase_is_not_go() -> TestResult {
    let table = go_table()?;
    ensure_eq!(go_lower(&table, '\u{130}'), 'i', "Go lowers U+0130 to i");
    ensure_eq!(
        "\u{130}stanbul".to_lowercase(),
        "i\u{307}stanbul",
        "std lowers U+0130 to i and U+0307"
    );
    ensure_eq!(to_lower("\u{130}stanbul"), "istanbul", "the glue, as Go");
    ensure_eq!(
        go_lower(&table, '\u{3a3}'),
        '\u{3c3}',
        "Go lowers every sigma to σ"
    );
    ensure_eq!(
        "BIDDER\u{3a3}".to_lowercase(),
        "bidder\u{3c2}",
        "std lowers a word-final sigma to ς"
    );
    ensure_eq!(
        to_lower("BIDDER\u{3a3}"),
        "bidder\u{3c3}",
        "the glue, as Go"
    );
    ensure_eq!(to_lower("AppNexus"), "appnexus", "ASCII");
    Ok(())
}

/// Every rune against Go's `strings.ToLower`. The only runes that differ are letters Unicode gave
/// a lower-case mapping after 15.0.0, the version of go1.26.2's tables: Go keeps them, std's
/// later tables lower them (ledger: `metrics.label-unicode`).
#[test]
fn to_lower_matches_go_for_every_rune() -> TestResult {
    let table = go_table()?;
    ensure!(table.len() > 1400, "the table has {} runes", table.len());
    let mut lowered = 0_usize;
    let mut newer_unicode = Vec::new();
    for c in (0..=0x10_FFFF_u32).filter_map(char::from_u32) {
        let go = go_lower(&table, c);
        let rust = to_lower(c.encode_utf8(&mut [0; 4]));
        lowered += usize::from(go != c);
        if rust != go.to_string() {
            ensure_eq!(
                go,
                c,
                "Go lowers U+{:04X}, the glue lowers it otherwise",
                u32::from(c)
            );
            ensure_eq!(rust.chars().count(), 1, "std's U+{:04X}", u32::from(c));
            newer_unicode.push(c);
        }
    }
    ensure_eq!(lowered, table.len(), "the runes Go lowers");
    ensure_eq!(
        newer_unicode.len(),
        75,
        "letters with a lower case after Unicode 15.0.0"
    );
    for (letter, lower) in [
        ('\u{a7cb}', "\u{264}"),
        ('\u{10d50}', "\u{10d70}"),
        ('\u{1c89}', "\u{1c8a}"),
    ] {
        ensure!(
            newer_unicode.contains(&letter),
            "U+{:04X} is one",
            u32::from(letter)
        );
        ensure_eq!(
            to_lower(letter.encode_utf8(&mut [0; 4])),
            lower,
            "std's lower case"
        );
    }
    Ok(())
}
