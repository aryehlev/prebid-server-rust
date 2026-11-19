//! The fragment schema: `crates/<crate>/differences.toml`, read as written before validation.

use serde::Deserialize;

/// One fragment: the crate's entries and the restorations it brings to entries recorded
/// anywhere.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fragment {
    /// The `[[difference]]` tables.
    #[serde(default)]
    pub(super) difference: Vec<RawEntry>,
    /// The `[[restoration]]` tables.
    #[serde(default)]
    pub(super) restoration: Vec<RawRestoration>,
}

/// A `[[difference]]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawEntry {
    /// The dotted id, its first segment the category.
    pub(super) id: String,
    /// The category.
    pub(super) category: String,
    /// `ledgered`, `restored` or `decision-pending`.
    pub(super) status: String,
    /// One line on the difference.
    pub(super) title: String,
    /// What Go does.
    pub(super) go: String,
    /// What Rust does.
    pub(super) rust: String,
    /// Who could notice.
    pub(super) who: Vec<String>,
    /// Where it shows.
    pub(super) visible_in: String,
    /// How the parity harness recognises it.
    #[serde(default)]
    pub(super) harness: Vec<RawPattern>,
    /// The pin: the test that shows both behaviours.
    pub(super) pin: String,
    /// The restore option, or for a restored entry what keeps Go's behaviour.
    pub(super) restore: Option<String>,
    /// For a decision-pending entry, what applies until the user decides.
    pub(super) default: Option<String>,
    /// The decision of `docs/DECISIONS.md` the entry depends on.
    pub(super) decision: Option<String>,
    /// For a restored entry, the Rule B case or `prebid-logic`.
    pub(super) rule_b: Option<String>,
    /// For a restored entry, the units whose merge brings the restore pin.
    #[serde(default)]
    pub(super) restore_by: Vec<String>,
    /// Anything else a reader needs.
    pub(super) note: Option<String>,
}

/// A `[[difference.harness]]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPattern {
    /// The mismatch kind.
    pub(super) kind: String,
    /// The lexical class: the class of a lexical or wire mismatch (the harness record's `class`).
    pub(super) lexical_class: Option<String>,
    /// The path pattern.
    pub(super) path: String,
    /// The input predicate.
    pub(super) input: Option<String>,
}

/// A `[[restoration]]` table: the restore pin of a restored entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawRestoration {
    /// The restored entry's id.
    pub(super) id: String,
    /// The test that compares the kept behaviour with a Go golden.
    pub(super) pin: String,
    /// Anything else a reader needs.
    pub(super) note: Option<String>,
}

/// Parses a fragment's text.
///
/// # Errors
///
/// The TOML error, with its line and column, when the text is not a fragment.
pub(super) fn parse(text: &str) -> Result<Fragment, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}
