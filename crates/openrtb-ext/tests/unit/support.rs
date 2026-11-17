//! Shared helpers: checks that return errors instead of panicking.

use std::{env, fmt::Debug, fs, path::PathBuf, process};

use pbs_gojson::{JsoniterDecode, jsonutil};

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

/// The repository root (the Go module).
pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
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
