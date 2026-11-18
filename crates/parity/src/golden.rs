//! Test support: checks, the vendored contract files, the Go goldens, and the lock that
//! serializes the tests touching process-wide state.

use core::fmt::Debug;
use std::{fs, path::PathBuf};

#[cfg(feature = "trace")]
use pbs_gojson::{GoJson, RawJson};
use serde_json::Value;
use tokio::sync::{Mutex, MutexGuard};

/// The result of a test: the first failed check.
pub(super) type TestResult = Result<(), String>;

/// The `regs` struct of Go's `TestAddAnalyticsKeepsTraceEncodable` (and of the golden scripts):
/// a raw member that `jsonutil.Marshal` writes nothing for when it does not compact.
#[cfg(feature = "trace")]
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "main.regs")]
pub(super) struct Regs {
    /// `gdpr`
    #[gojson(go = "GDPR", json = "gdpr")]
    pub(super) gdpr: isize,
    /// `ext`
    #[gojson(json = "ext,omitempty")]
    pub(super) ext: RawJson,
}

/// The `record` struct of the same test: `{"regs": *regs}`.
#[cfg(feature = "trace")]
#[derive(Clone, Debug, Default, PartialEq, Eq, GoJson)]
#[gojson(go_type = "main.regsRecord")]
pub(super) struct RegsRecord {
    /// `regs`
    #[gojson(json = "regs")]
    pub(super) regs: Option<Regs>,
}

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

/// A golden file the Go program wrote (`testdata/golden/<name>`), parsed.
pub(super) fn golden_json(name: &str) -> Result<Json, String> {
    Json::parse(&read_file(&format!("testdata/golden/{name}"))?, name)
}

/// The lock that serializes the tests touching process-wide state (the fakes, the published
/// session).
static GLOBAL: Mutex<()> = Mutex::const_new(());

/// Takes [`GLOBAL`] from an async test.
#[cfg(feature = "trace")]
pub(super) async fn global_lock() -> MutexGuard<'static, ()> {
    GLOBAL.lock().await
}

/// Takes [`GLOBAL`] from a test that runs outside a runtime.
pub(super) fn global_lock_blocking() -> MutexGuard<'static, ()> {
    GLOBAL.blocking_lock()
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

    /// The member `key`, or `None` when it is absent or null.
    pub(super) fn opt(&self, key: &str) -> Option<Self> {
        self.0.get(key).filter(|v| !v.is_null()).cloned().map(Self)
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

    /// The boolean member `key`.
    pub(super) fn boolean(&self, key: &str) -> Result<bool, String> {
        let v = self.get(key)?;
        v.0.as_bool()
            .ok_or_else(|| format!("member {key:?} is not a boolean: {}", v.0))
    }

    /// The value as compact JSON bytes.
    pub(super) fn to_bytes(&self) -> Vec<u8> {
        self.0.to_string().into_bytes()
    }
}
