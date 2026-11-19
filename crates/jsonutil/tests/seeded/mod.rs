//! The seeded corpora: Go's `splitmix64` and the per-batch digests of `testdata/go/gen/seeded.go`.
//!
//! Each test file ports its corpus's generator onto [`SplitMix64`], so both sides build the same
//! inputs from the seed, and [`check_digests`] compares the SHA-256 digests of every batch's
//! inputs and outcomes with the golden's.

use std::fmt::Write as _;

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::support::{TestResult, text};

/// Go's `splitmix64`.
pub(super) struct SplitMix64(u64);

impl SplitMix64 {
    /// Go's `next`.
    pub(super) const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Go's `below(n)`, a number in `0..n`.
    pub(super) fn below(&mut self, n: usize) -> usize {
        let bound = u64::try_from(n).unwrap_or(u64::MAX).max(1);
        usize::try_from(self.next_u64() % bound).unwrap_or_default()
    }

    /// Go's `pick(list)`.
    pub(super) fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        let at = self.below(list.len());
        list.get(at).copied().unwrap_or_default()
    }
}

/// Rebuilds the seeded corpus whose digests Go wrote at `pointer` in `doc` with `next`, which
/// returns one case's input and outcome lines, and compares each batch's digests with Go's. `set`
/// names the corpus for `go run ./gen -dump SET:BATCH`, which prints a batch that differs.
pub(super) fn check_digests<F>(doc: &Value, pointer: &str, set: &str, mut next: F) -> TestResult
where
    F: FnMut(&mut SplitMix64) -> TestResult<(String, String)>,
{
    let seeded = doc
        .pointer(pointer)
        .ok_or_else(|| format!("the golden has nothing at {pointer}"))?;
    let seed = text(seeded, "/seed")?;
    let seed = u64::from_str_radix(seed.trim_start_matches("0x"), 16)?;
    let count = number(seeded, "/count")?;
    let batch = number(seeded, "/batch")?;
    let batches = seeded
        .pointer("/batches")
        .and_then(Value::as_array)
        .ok_or("the golden has no batches")?;
    if count == 0 || batch == 0 || batches.len() != count / batch {
        return Err(format!(
            "{set}: {} batches for {count} cases of {batch}",
            batches.len()
        )
        .into());
    }
    let mut rng = SplitMix64(seed);
    let mut differ = Vec::new();
    for (index, want) in batches.iter().enumerate() {
        let mut inputs = Sha256::new();
        let mut outcomes = Sha256::new();
        for _ in 0..batch {
            let (input, outcome) = next(&mut rng)?;
            inputs.update(format!("{input}\n"));
            outcomes.update(format!("{outcome}\n"));
        }
        let inputs = hex(&inputs.finalize());
        let outcomes = hex(&outcomes.finalize());
        if inputs != text(want, "/inputs_sha256")? {
            differ.push(format!(
                "{set}:{index} (inputs: the generator port is wrong)"
            ));
        } else if outcomes != text(want, "/outcomes_sha256")? {
            differ.push(format!("{set}:{index}"));
        }
    }
    if differ.is_empty() {
        Ok(())
    } else {
        Err(
            format!("batches that differ from Go (go run ./gen -dump SET:BATCH): {differ:?}")
                .into(),
        )
    }
}

/// The unsigned integer at `pointer` in `value`.
fn number(value: &Value, pointer: &str) -> TestResult<usize> {
    let n = value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("no number at {pointer}"))?;
    Ok(usize::try_from(n)?)
}

/// Lower-case hex, as Go's `hex.EncodeToString`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _written = write!(out, "{byte:02x}");
        out
    })
}
