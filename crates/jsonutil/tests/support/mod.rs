//! What every test file uses: the goldens and a check that returns an error instead of panicking.

use std::{error::Error, fmt::Debug, fs, path::Path};

use serde_json::Value;

/// A test's result; the error is the failure message.
pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The golden `name` from `testdata/golden`, which `testdata/go/gen` writes.
pub(super) fn golden(name: &str) -> TestResult<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/golden")
        .join(name);
    let data = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(serde_json::from_slice(&data)?)
}

/// The array at `pointer` in `value`.
pub(super) fn list<'a>(value: &'a Value, pointer: &str) -> TestResult<&'a [Value]> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("no array at {pointer}").into())
}

/// The string at `pointer` in `value`.
pub(super) fn text<'a>(value: &'a Value, pointer: &str) -> TestResult<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no string at {pointer} in {value}").into())
}

/// Fails, showing both values, unless `got` equals `want`.
pub(super) fn ensure_eq<T: PartialEq + Debug + ?Sized>(
    got: &T,
    want: &T,
    what: &str,
) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}").into())
    }
}
