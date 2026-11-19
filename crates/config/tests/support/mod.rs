//! Helpers the loader's and the YAML reader's tests share: the goldens `testdata/go` writes, and
//! checks that return errors instead of panicking.

use std::{fmt::Debug, fs, path::Path};

use serde_json::Value as Json;

/// A test result whose error is the failure message.
pub type TestResult<T = ()> = Result<T, String>;

/// The golden `name` of `testdata/golden`.
///
/// # Errors
///
/// When the file cannot be read or is not JSON.
pub fn golden(name: &str) -> TestResult<Json> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("golden")
        .join(name);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The value at the JSON pointer `pointer` of `doc`.
///
/// # Errors
///
/// When `doc` has nothing there.
pub fn at<'a>(doc: &'a Json, pointer: &str) -> TestResult<&'a Json> {
    doc.pointer(pointer)
        .ok_or_else(|| format!("the golden has no {pointer}"))
}

/// The string at `pointer`.
///
/// # Errors
///
/// When `doc` has no string there.
pub fn text<'a>(doc: &'a Json, pointer: &str) -> TestResult<&'a str> {
    at(doc, pointer)?
        .as_str()
        .ok_or_else(|| format!("the golden's {pointer} is not a string"))
}

/// The list at `pointer`.
///
/// # Errors
///
/// When `doc` has no list there.
pub fn list<'a>(doc: &'a Json, pointer: &str) -> TestResult<&'a [Json]> {
    at(doc, pointer)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("the golden's {pointer} is not a list"))
}

/// Fails, showing both values, unless they are equal.
///
/// # Errors
///
/// When the values differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{what}: got {actual:?}, want {expected:?}"))
    }
}

/// Fails with every failure, one per line, unless there are none.
///
/// # Errors
///
/// When `failures` is not empty.
pub fn report(what: &str, failures: &[String]) -> TestResult {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{what}: {} failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        ))
    }
}

/// Whether `value` is a tagged value of the goldens' compact form (`{"$int64": 5}`), which is a
/// leaf.
fn is_tagged(value: &Json) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == 1 && map.keys().all(|k| k.starts_with('$')))
}

/// The differences between `got` and `want`, one per leaf: objects are entered (a tagged value is
/// a leaf), anything else is compared whole.
#[must_use]
pub fn diff(got: &Json, want: &Json) -> Vec<String> {
    let mut out = Vec::new();
    diff_at(got, want, "", &mut out);
    out
}

/// [`diff`] under `path`.
fn diff_at(got: &Json, want: &Json, path: &str, out: &mut Vec<String>) {
    match (got.as_object(), want.as_object()) {
        (Some(g), Some(w)) if !is_tagged(got) && !is_tagged(want) => {
            for (key, value) in w {
                let at = format!("{path}/{key}");
                match g.get(key) {
                    Some(mine) => diff_at(mine, value, &at, out),
                    None => out.push(format!("{at}: missing, want {value}")),
                }
            }
            for (key, value) in g {
                if !w.contains_key(key) {
                    out.push(format!("{path}/{key}: got {value}, not in Go's"));
                }
            }
        }
        _ if got == want => {}
        _ => out.push(format!("{path}: got {got}, want {want}")),
    }
}
