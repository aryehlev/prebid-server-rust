//! Shared helpers: checks that return errors instead of panicking.

use std::{env, fmt::Debug, fs, path::PathBuf, process};

use pbs_devtools::repo;
use pbs_gojson::{JsoniterDecode, jsonutil};
use serde::de::DeserializeOwned;

/// A test result whose error is the failure message.
pub(super) type TestResult<T = ()> = Result<T, String>;

/// Fails, showing both values, unless they are equal.
pub(super) fn ensure_eq<T: PartialEq + Debug + ?Sized>(
    actual: &T,
    expected: &T,
    what: &str,
) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}: got {actual:?}, want {expected:?}"))
    }
}

/// `jsonutil.UnmarshalValid(doc, v)`'s error text, `None` on success.
pub(super) fn unmarshal_error<T: JsoniterDecode + ?Sized>(doc: &str, v: &mut T) -> Option<String> {
    jsonutil::unmarshal_valid(doc.as_bytes(), v)
        .err()
        .map(|e| e.message.to_string_lossy())
}

/// `jsonutil.UnmarshalValid(doc, &v)` through the serde code (`pbs_jsonutil::unmarshal_valid`):
/// the value, or the error's text.
pub(super) fn serde_unmarshal<T: DeserializeOwned>(doc: &str) -> TestResult<T> {
    pbs_jsonutil::unmarshal_valid(doc.as_bytes()).map_err(|e| e.to_string())
}

/// `text` without the position `serde_json` puts after an error inside the document
/// (` at line 1 column 52`), which Go's text does not have (`json.prebid-message-context`).
pub(super) fn without_position(text: &str) -> &str {
    text.rsplit_once(" at line ")
        .filter(|(_, position)| {
            position
                .split_once(" column ")
                .is_some_and(|(line, column)| is_digits(line) && is_digits(column))
        })
        .map_or(text, |(message, _)| message)
}

/// Whether `text` is a run of decimal digits.
fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The Go root: the upstream checkout whose static files the tests read (`$PBS_GO_ROOT`, else
/// `.upstream` at the workspace root; `scripts/fetch-upstream.sh` makes it).
pub(super) fn go_root() -> TestResult<PathBuf> {
    repo::go_root().map_err(|e| e.to_string())
}

/// A path under the system's temporary directory for this process, named `name`.
pub(super) fn temp_path(name: &str) -> String {
    format!(
        "{}/pbs-openrtb-ext-unit-{}-{name}",
        env::temp_dir().display(),
        process::id()
    )
}

/// A directory of files under [`temp_path`], removed when dropped.
pub(super) struct TempDir(pub(super) String);

impl TempDir {
    /// Creates the directory `name` holding `files` (name, contents).
    pub(super) fn new(name: &str, files: &[(&str, &str)]) -> TestResult<Self> {
        let dir = temp_path(name);
        let _cleared = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| format!("creating {dir}: {e}"))?;
        for (file, contents) in files {
            fs::write(format!("{dir}/{file}"), contents)
                .map_err(|e| format!("writing {dir}/{file}: {e}"))?;
        }
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

/// Every case name of a table-driven test, with `Some(failure)` for the failed ones: fails with
/// all the failures.
pub(super) fn all_ok<I: IntoIterator<Item = (String, TestResult)>>(cases: I) -> TestResult {
    let failures: Vec<String> = cases
        .into_iter()
        .filter_map(|(name, result)| result.err().map(|e| format!("{name}: {e}")))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
