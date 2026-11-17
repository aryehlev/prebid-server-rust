//! The hand-maintained hooks overlay, `rust/testdata/go_fixture_hooks.json`.
//!
//! Some Go adapter tests change a bidder after `Builder` returns it (a fake clock, a fixed random
//! generator, a testing flag) or build it as a struct literal. The manifest generator cannot
//! evaluate that Go code, so it detects those statements and requires an overlay entry that names
//! the equivalent hook values in a fixed vocabulary ([`Hooks`]). Each entry pins the exact
//! statements it accounts for and the SHA-256 of the Go test file, so any change on the Go side
//! fails generation until someone re-reviews the entry.

use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, ToolError},
    manifest::Totals,
};

/// Repository-relative path of the overlay.
pub const HOOKS_PATH: &str = "rust/testdata/go_fixture_hooks.json";

/// The overlay file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksFile {
    /// Free-form notes for maintainers.
    pub notes: Vec<String>,
    /// Totals the generated manifest must reproduce; update them after reviewing new adapters.
    pub expected_totals: Totals,
    /// Hooks that apply to every run.
    pub global_hooks: Hooks,
    /// Per-run entries keyed by `"<go_pkg>/<go_test>"`.
    pub runs: BTreeMap<String, RunHooks>,
}

/// One run's overlay entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunHooks {
    /// The hook values to apply.
    pub hooks: Hooks,
    /// The setup statements (as the manifest prints them) this entry accounts for.
    #[serde(default)]
    pub setup: Vec<String>,
    /// For struct-literal construction: the fields whose values the hooks supply.
    #[serde(default)]
    pub opaque_fields: Vec<String>,
    /// SHA-256 of the Go test file the run is declared in, at review time.
    pub go_test_file_sha256: String,
    /// Where the Go code sets the hooked state, and anything a porter needs to know.
    pub notes: String,
}

/// The hook vocabulary. Every field is optional; absent means "not hooked".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// A fixed clock, RFC 3339 in UTC (`2016-01-01T12:30:15Z`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_utc: Option<String>,
    /// The value returned by the adapter's random `GenerateInt63`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_int63: Option<i64>,
    /// The value returned by the adapter's random `Intn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_intn: Option<i64>,
    /// The value returned by the adapter's UUID generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// The adapter's `testing` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testing_flag: Option<bool>,
    /// The value returned by the adapter's cache buster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_buster: Option<String>,
    /// The value returned by the adapter's week generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<String>,
    /// Package-level variables the test assigns, by Go name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_vars: Option<BTreeMap<String, String>>,
    /// The value of `version.Ver` (empty under `go test`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_ver: Option<String>,
}

impl HooksFile {
    /// Reads and validates the overlay.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be read or parsed, or a hook value is malformed.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|e| ToolError::io(path, &e))?;
        let file: Self = serde_json::from_str(&text)
            .map_err(|e| ToolError::new(format!("{}: {e}", path.display())))?;
        file.global_hooks.validate("global_hooks")?;
        for (key, run) in &file.runs {
            run.hooks.validate(key)?;
            if run.go_test_file_sha256.len() != 64
                || !run
                    .go_test_file_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
            {
                return Err(ToolError::new(format!(
                    "{key}: go_test_file_sha256 is not a SHA-256 hex digest"
                )));
            }
        }
        Ok(file)
    }
}

impl Hooks {
    /// Checks the formats the vocabulary prescribes.
    ///
    /// # Errors
    ///
    /// Fails when `clock_utc` is not `YYYY-MM-DDTHH:MM:SSZ` or `uuid` is not a UUID.
    pub fn validate(&self, context: &str) -> Result<()> {
        if let Some(clock) = &self.clock_utc
            && !is_rfc3339_utc(clock)
        {
            return Err(ToolError::new(format!(
                "{context}: clock_utc {clock:?} is not YYYY-MM-DDTHH:MM:SSZ"
            )));
        }
        if let Some(uuid) = &self.uuid
            && !is_uuid(uuid)
        {
            return Err(ToolError::new(format!(
                "{context}: uuid {uuid:?} is not a UUID"
            )));
        }
        Ok(())
    }
}

fn is_rfc3339_utc(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::{HOOKS_PATH, Hooks, HooksFile, is_rfc3339_utc, is_uuid};
    use crate::{
        error::Result,
        repo::Repo,
        testutil::{check, check_eq},
    };

    #[test]
    fn formats() -> Result<()> {
        check(is_rfc3339_utc("2016-01-01T12:30:15Z"), "valid clock")?;
        check(!is_rfc3339_utc("2016-01-01 12:30:15Z"), "space")?;
        check(
            is_uuid("30470a14-2949-4110-abce-b62d57304ad5"),
            "valid uuid",
        )?;
        check(!is_uuid("30470a14294941100abce0b62d57304ad5"), "no dashes")
    }

    #[test]
    fn validate_rejects_bad_clock() -> Result<()> {
        let hooks = Hooks {
            clock_utc: Some("yesterday".to_owned()),
            ..Hooks::default()
        };
        check(hooks.validate("x").is_err(), "bad clock")
    }

    #[test]
    fn committed_overlay_loads() -> Result<()> {
        let repo = Repo::locate(None)?;
        let file = HooksFile::load(&repo.path(HOOKS_PATH))?;
        check_eq(
            &file.global_hooks.version_ver.as_deref(),
            &Some(""),
            "version.Ver is empty under go test",
        )?;
        check_eq(&file.runs.len(), &8, "hooked runs")?;
        check(
            file.runs.values().all(|r| !r.notes.is_empty()),
            "every entry cites the Go code",
        )
    }
}
