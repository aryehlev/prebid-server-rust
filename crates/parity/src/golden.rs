//! Test support: checks and the vendored contract files.

use core::fmt::Debug;
use std::{fs, path::PathBuf};

use serde_json::Value;

/// The result of a test: the first failed check.
pub(super) type TestResult = Result<(), String>;

/// Fails with `what` unless `cond` holds.
pub(super) fn check(cond: bool, what: &str) -> TestResult {
    if cond {
        Ok(())
    } else {
        Err(format!("check failed: {what}"))
    }
}

/// Fails unless `got == want`.
pub(super) fn check_eq<T: PartialEq + Debug + ?Sized>(got: &T, want: &T, what: &str) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}:\n  got:  {got:?}\n  want: {want:?}"))
    }
}

/// A backslash-u escape of the code point `hex`, as Go's HTML-escaping encoders write `<`, `>`
/// and `&` (Go: `uEsc` in `parity/trace/session_test.go`); the backslash is built from its code.
pub(super) fn u_escape(hex: &str) -> String {
    format!("{}u{hex}", char::from(0x5C_u8))
}

/// A path under the crate directory.
pub(super) fn crate_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The bytes of a file under the crate directory.
pub(super) fn read_file(rel: &str) -> Result<Vec<u8>, String> {
    fs::read(crate_path(rel)).map_err(|err| format!("reading {rel}: {err}"))
}

/// A vendored contract file (`contract/<name>`), parsed.
pub(super) fn contract_json(name: &str) -> Result<Json, String> {
    Json::parse(&read_file(&format!("contract/{name}"))?, name)
}

/// A parsed JSON value with checked accessors.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Json(pub(super) Value);

impl Json {
    /// Parses `bytes`, naming `what` in the error.
    pub(super) fn parse(bytes: &[u8], what: &str) -> Result<Self, String> {
        serde_json::from_slice(bytes)
            .map(Self)
            .map_err(|err| format!("parsing {what}: {err}"))
    }

    /// The member `key`.
    pub(super) fn get(&self, key: &str) -> Result<Self, String> {
        self.0
            .get(key)
            .cloned()
            .map(Self)
            .ok_or_else(|| format!("missing member {key:?} in {}", self.0))
    }

    /// The elements of the array member `key`.
    pub(super) fn array(&self, key: &str) -> Result<Vec<Self>, String> {
        self.get(key)?.elements()
    }

    /// The elements of this array.
    pub(super) fn elements(&self) -> Result<Vec<Self>, String> {
        self.0
            .as_array()
            .map(|items| items.iter().cloned().map(Self).collect())
            .ok_or_else(|| format!("not an array: {}", self.0))
    }

    /// The string member `key`.
    pub(super) fn string(&self, key: &str) -> Result<String, String> {
        self.get(key)?.text()
    }

    /// This string.
    pub(super) fn text(&self) -> Result<String, String> {
        self.0
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("not a string: {}", self.0))
    }

    /// The integer member `key`.
    pub(super) fn int(&self, key: &str) -> Result<i64, String> {
        let v = self.get(key)?;
        v.0.as_i64()
            .ok_or_else(|| format!("member {key:?} is not an integer: {}", v.0))
    }
}
