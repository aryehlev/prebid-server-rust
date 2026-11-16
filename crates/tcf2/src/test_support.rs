//! Helpers for the tests that replay the Go-generated goldens under `testdata/`.
//!
//! The goldens are written by the Go program in `testdata/go` (run it with
//! `go run ./rust/crates/tcf2/testdata/go -out rust/crates/tcf2/testdata` from the repository
//! root). They are read at run time rather than embedded, since they are several megabytes.

use std::{error::Error, fs, path::Path, str};

use serde_json::Value;

/// The error type of every test in this crate.
pub(crate) type TestError = Box<dyn Error>;

/// The result type of every test in this crate.
pub(crate) type TestResult = Result<(), TestError>;

/// Loads `testdata/<name>` and returns its `cases` array.
pub(crate) fn load_cases(name: &str) -> Result<Vec<Value>, TestError> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    let text = fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&text)?;
    let header = document.get("header").ok_or("golden file without header")?;
    if header.get("go_gdpr_version").and_then(Value::as_str) != Some("v1.12.0") {
        return Err(format!("{name} was not generated from go-gdpr v1.12.0").into());
    }
    match document.get_mut("cases").map(Value::take) {
        Some(Value::Array(cases)) => Ok(cases),
        _ => Err(format!("{name} has no cases array").into()),
    }
}

/// The string field `key` of `value`, if present.
pub(crate) fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Decodes lowercase or uppercase hex.
pub(crate) fn hex_decode(text: &str) -> Result<Vec<u8>, TestError> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return Err(format!("odd-length hex {text:?}").into());
    }
    pairs
        .iter()
        .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
        .collect()
}

/// Encodes bytes as lowercase hex, like Go's `hex.EncodeToString`.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        for nibble in [byte >> 4, byte & 0x0F] {
            out.push(char::from(
                DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'0'),
            ));
        }
    }
    out
}

/// The input bytes of a golden case: `input` (valid UTF-8) or `input_hex`.
pub(crate) fn input_bytes(case: &Value) -> Result<Vec<u8>, TestError> {
    if let Some(input) = str_field(case, "input") {
        return Ok(input.as_bytes().to_vec());
    }
    hex_decode(str_field(case, "input_hex").unwrap_or_default())
}
