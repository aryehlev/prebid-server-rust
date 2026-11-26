//! What the crate's unit tests share.
//!
//! Their result type, the go-metrics goldens that `testdata/go` writes, and Go's FNV-1a hash the
//! goldens digest with.

use std::{error::Error, fs};

/// The result of a test: its error is the failure.
pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The go-metrics goldens (`testdata/go/gometrics.go` and `testdata/go/arbiter`).
const GOMETRICS_GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/golden/gometrics");

/// The go-metrics golden file `name`.
///
/// # Errors
///
/// When the file cannot be read.
pub fn gometrics_golden(name: &str) -> TestResult<String> {
    let path = format!("{GOMETRICS_GOLDEN}/{name}");
    fs::read_to_string(&path).map_err(|e| format!("{path}: {e}").into())
}

/// Go: `hash/fnv` `New64a`.
#[must_use]
pub fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The first line that differs, for a readable failure.
#[must_use]
pub fn first_difference(expected: &str, actual: &str) -> String {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    for line_no in 1.. {
        match (expected_lines.next(), actual_lines.next()) {
            (None, None) => break,
            (e, a) if e == a => {}
            (e, a) => return format!("line {line_no}: Go {e:?}, Rust {a:?}"),
        }
    }
    "no line differs".to_owned()
}
