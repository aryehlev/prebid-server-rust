//! Readers for the golden files written by `testdata/go/golden` (test-only).
//!
//! Each line is a tab-separated list of `strconv.Quote`d fields. Sweeps are FNV-1a 64 checksums
//! over newline-terminated outputs for inputs drawn from the shared splitmix64 generator.

use std::fmt::Display;

use crate::strconv::{unquote, unquote_bytes};

/// Splits golden text into rows of unquoted fields.
///
/// # Errors
///
/// A message naming the line when a field is not a valid Go string literal.
pub fn rows(text: &str) -> Result<Vec<Vec<String>>, String> {
    text.lines()
        .enumerate()
        .map(|(n, line)| {
            line.split('\t')
                .map(|field| unquote(field).map_err(|e| format!("line {}: {e}: {field}", n + 1)))
                .collect()
        })
        .collect()
}

/// Like [`rows`] but keeps the exact bytes of each field (Go strings may be invalid UTF-8).
///
/// # Errors
///
/// A message naming the line when a field is not a valid Go string literal.
pub fn rows_bytes(text: &str) -> Result<Vec<Vec<Vec<u8>>>, String> {
    text.lines()
        .enumerate()
        .map(|(n, line)| {
            line.split('\t')
                .map(|field| {
                    unquote_bytes(field).map_err(|e| format!("line {}: {e}: {field}", n + 1))
                })
                .collect()
        })
        .collect()
}

/// The first `N` fields of `row`.
///
/// # Errors
///
/// A message when the row has fewer than `N` fields.
pub fn fields<const N: usize>(row: &[String]) -> Result<[String; N], String> {
    let got: Vec<String> = row.iter().take(N).cloned().collect();
    got.try_into()
        .map_err(|short: Vec<String>| format!("row has {} of {N} fields: {row:?}", short.len()))
}

/// The checksum recorded under `name` in a two-column golden file.
///
/// # Errors
///
/// A message when the name is missing.
pub fn checksum(text: &str, name: &str) -> Result<String, String> {
    rows(text)?
        .into_iter()
        .find_map(|row| match row.as_slice() {
            [n, sum] if n == name => Some(sum.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("no checksum named {name}"))
}

/// The splitmix64 generator of the Go golden program.
#[derive(Debug, Clone, Copy)]
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    /// The next value.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// FNV-1a 64 over newline-terminated outputs (Go: `sweep`).
#[derive(Debug, Clone, Copy)]
pub struct Sweep(u64);

impl Default for Sweep {
    fn default() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }
}

impl Sweep {
    /// Adds one output.
    pub fn add(&mut self, v: &str) {
        self.add_bytes(v.as_bytes());
    }

    /// Adds one output given as bytes.
    pub fn add_bytes(&mut self, v: &[u8]) {
        for &b in v.iter().chain(b"\n") {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    /// The checksum as 16 hex digits.
    #[must_use]
    pub fn sum(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Go: `errText`: `""` for success, otherwise `"error: " + err`.
pub fn err_text<T, E: Display>(r: &Result<T, E>) -> String {
    r.as_ref()
        .err()
        .map_or_else(String::new, |e| format!("error: {e}"))
}
