//! Shared helpers: checks that return errors, and reading the goldens and the Go corpora.

use std::{
    fmt::Debug,
    fs,
    path::{Path, PathBuf},
};

use pbs_gocompat::strconv::unquote_bytes;

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

/// The crate directory.
pub(super) fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The repository root: the Go module whose corpora the goldens were made from.
pub(super) fn repo_root() -> PathBuf {
    crate_dir().join("../../..")
}

/// The file's bytes.
pub(super) fn read(path: &Path) -> TestResult<Vec<u8>> {
    fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// The rows of a golden file of `testdata/golden`: tab-separated `strconv.QuoteToASCII`
/// fields, unquoted to their bytes.
pub(super) fn golden_rows(file: &str) -> TestResult<Vec<Vec<Vec<u8>>>> {
    let path = crate_dir().join("testdata/golden").join(file);
    let text = String::from_utf8(read(&path)?)
        .map_err(|e| format!("{}: not ASCII: {e}", path.display()))?;
    text.lines()
        .enumerate()
        .map(|(n, line)| {
            line.split('\t')
                .map(|field| {
                    unquote_bytes(field).map_err(|e| format!("{file}:{}: {e}: {field}", n + 1))
                })
                .collect()
        })
        .collect()
}

/// FNV-1a 64 (Go's `hash/fnv` `New64a`).
pub(super) fn fnv64(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// Text for a failure message: the bytes, lossily, cut at 300 bytes.
pub(super) fn show(bytes: &[u8]) -> String {
    let cut = bytes.get(..300).unwrap_or(bytes);
    let mut s = String::from_utf8_lossy(cut).into_owned();
    if cut.len() < bytes.len() {
        s.push_str("...");
    }
    s
}

/// The first `n` failures, one per line, and how many there were in all.
pub(super) fn report(what: &str, failures: &[String], n: usize) -> TestResult {
    if failures.is_empty() {
        return Ok(());
    }
    let shown: Vec<&str> = failures.iter().take(n).map(String::as_str).collect();
    Err(format!(
        "{what}: {} failures; the first {}:\n{}",
        failures.len(),
        shown.len(),
        shown.join("\n")
    ))
}
