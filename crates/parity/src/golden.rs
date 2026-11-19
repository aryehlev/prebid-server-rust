//! Test support: checks, the vendored contract files, the Go goldens, the comparison of JSON
//! documents as values, and the lock that serializes the tests touching process-wide state.

use core::fmt::Debug;
use std::{collections::BTreeMap, fs, path::PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(feature = "trace")]
use serde::Serialize;
use serde_json::{Value, value::RawValue};
use tokio::sync::{Mutex, MutexGuard};

/// The result of a test: the first failed check.
pub(super) type TestResult = Result<(), String>;

/// The `regs` struct of Go's `TestAddAnalyticsKeepsTraceEncodable` (and of the golden scripts):
/// `GDPR int` as `gdpr` and `Ext json.RawMessage` as `ext,omitempty`.
#[cfg(feature = "trace")]
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct Regs {
    /// `gdpr`
    pub(super) gdpr: isize,
    /// `ext`, left out when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ext: Option<Box<RawValue>>,
}

/// The `record` struct of the same test: `{"regs": *regs}`.
#[cfg(feature = "trace")]
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct RegsRecord {
    /// `regs`
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

/// `bytes` as text, invalid UTF-8 replaced.
pub(super) fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Standard base64 with padding, decoded (the goldens' `*_b64` members).
pub(super) fn decode_b64(text: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(text).map_err(|e| format!("{text:?}: {e}"))
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

    /// The members of this object.
    pub(super) fn members(&self) -> Result<Vec<(String, Self)>, String> {
        self.0
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), Self(v.clone())))
                    .collect()
            })
            .ok_or_else(|| format!("not an object: {}", self.0))
    }

    /// The string member `key`.
    pub(super) fn string(&self, key: &str) -> Result<String, String> {
        self.get(key)?.text()
    }

    /// The string member `key`, or `""` when it is absent or null (the golden program's
    /// `omitempty` members).
    #[cfg(feature = "trace")]
    pub(super) fn opt_string(&self, key: &str) -> Result<String, String> {
        Ok(self
            .opt(key)
            .map(|v| v.text())
            .transpose()?
            .unwrap_or_default())
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

/// A JSON text read as a value, which is how the runner compares the documents of the two
/// servers: `serde_json` and `encoding/json` spell the same document differently
/// (`json.parity-documents`).
///
/// Numbers compare by value, so Go's `1` and `1e-07` equal `serde_json`'s `1.0` and `1e-7`, and a
/// number past the range of `f64` (`1e400`, which both keep inside raw members) still reads.
/// Strings compare after their escapes; objects by member name, the last of repeated members
/// winning (only raw members, which both sides keep as they came, can repeat one).
#[derive(Clone, Debug)]
pub(super) enum Doc {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// A number, as written.
    Number(String),
    /// A string.
    String(String),
    /// A string `serde_json` does not decode (a lone surrogate escape), as written.
    Escaped(String),
    /// An array.
    Array(Vec<Self>),
    /// An object.
    Object(BTreeMap<String, Self>),
}

impl Doc {
    /// Reads one JSON text.
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let raw: Box<RawValue> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        Self::of(&raw).map_err(|e| e.to_string())
    }

    /// Reads one value `serde_json` has checked.
    fn of(raw: &RawValue) -> Result<Self, serde_json::Error> {
        let text = raw.get();
        Ok(match text.as_bytes().first() {
            Some(b'{') => Self::Object(
                serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(text)?
                    .into_iter()
                    .map(|(k, v)| Ok((k, Self::of(&v)?)))
                    .collect::<Result<_, serde_json::Error>>()?,
            ),
            Some(b'[') => Self::Array(
                serde_json::from_str::<Vec<Box<RawValue>>>(text)?
                    .iter()
                    .map(|v| Self::of(v))
                    .collect::<Result<_, _>>()?,
            ),
            Some(b'"') => serde_json::from_str(text)
                .map_or_else(|_| Self::Escaped(text.to_owned()), Self::String),
            Some(b't') => Self::Bool(true),
            Some(b'f') => Self::Bool(false),
            Some(b'n') => Self::Null,
            _ => Self::Number(text.to_owned()),
        })
    }
}

impl PartialEq for Doc {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => same_number(a, b),
            (Self::String(a), Self::String(b)) | (Self::Escaped(a), Self::Escaped(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Object(a), Self::Object(b)) => a == b,
            (
                Self::Null
                | Self::Bool(_)
                | Self::Number(_)
                | Self::String(_)
                | Self::Escaped(_)
                | Self::Array(_)
                | Self::Object(_),
                _,
            ) => false,
        }
    }
}

/// Whether two JSON numbers have the same value: integers exactly, anything else as the `f64`
/// both read it as (the sign of zero included).
fn same_number(a: &str, b: &str) -> bool {
    let integer = |s: &str| {
        if s.contains(['.', 'e', 'E']) {
            None
        } else {
            s.parse::<i128>().ok()
        }
    };
    match (integer(a), integer(b)) {
        (Some(x), Some(y)) => x == y,
        _ => match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => x.total_cmp(&y).is_eq(),
            _ => a == b,
        },
    }
}

/// Fails unless `got` and `want` are JSON texts of the same value ([`Doc`]).
pub(super) fn check_same_json(got: &[u8], want: &[u8], what: &str) -> TestResult {
    let read = |bytes: &[u8], side: &str| {
        Doc::parse(bytes).map_err(|e| format!("{what}: {side} is not JSON ({e}): {}", lossy(bytes)))
    };
    if read(got, "got")? == read(want, "want")? {
        Ok(())
    } else {
        Err(format!(
            "{what}: not the same JSON value\n  got:  {}\n  want: {}",
            lossy(got),
            lossy(want)
        ))
    }
}
