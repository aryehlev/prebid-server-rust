//! Go `util/uuidutil`: an injectable generator of random (version 4) UUIDs.

use std::{
    error, fmt,
    fs::File,
    io::{self, Read as _},
    ops::Range,
    sync::OnceLock,
};

/// Go: `uuidutil.UUIDGenerator`.
pub trait UuidGenerator: Send + Sync {
    /// Go: `Generate`: a new UUID in its canonical text form.
    ///
    /// # Errors
    ///
    /// When no UUID can be made, such as when the random source fails.
    fn generate(&self) -> Result<String, UuidError>;
}

/// The error of [`UuidRandomGenerator`]: reading the random source failed (Go: the error of
/// `io.ReadFull(rand.Reader, ...)` in `uuid.NewV4`).
#[derive(Debug)]
pub struct UuidError(io::Error);

impl fmt::Display for UuidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl error::Error for UuidError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Go: `uuidutil.UUIDRandomGenerator`: `gofrs/uuid.NewV4().String()`, 16 bytes from the
/// operating system's secure random source (Go: `crypto/rand`), with the version and variant
/// bits set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UuidRandomGenerator;

/// The operating system's random source, opened once it opens.
///
/// A failure to open it is not remembered: Go's `crypto/rand` needs no file descriptor, so a
/// passing condition such as `EMFILE` on the first call must fail that call only, not every
/// later UUID of the process.
fn urandom() -> Result<&'static File, UuidError> {
    static URANDOM: OnceLock<File> = OnceLock::new();
    open_once(&URANDOM, "/dev/urandom")
}

/// The file in `cell`, or `path` opened and kept there; a failed open leaves `cell` empty.
fn open_once<'a>(cell: &'a OnceLock<File>, path: &str) -> Result<&'a File, UuidError> {
    if let Some(file) = cell.get() {
        return Ok(file);
    }
    let file = File::open(path).map_err(UuidError)?;
    // Another thread may have opened it first; its file is kept and this one closed.
    Ok(cell.get_or_init(|| file))
}

impl UuidGenerator for UuidRandomGenerator {
    fn generate(&self) -> Result<String, UuidError> {
        let mut b = [0_u8; 16];
        let mut source = urandom()?;
        source.read_exact(&mut b).map_err(UuidError)?;
        Ok(v4_string(b))
    }
}

/// Go: `uuid.UUID.SetVersion(V4)`, `SetVariant(VariantRFC4122)` and `String`: the canonical
/// lowercase `8-4-4-4-12` text of the version 4 UUID made from 16 random bytes.
#[must_use]
pub fn v4_string(mut b: [u8; 16]) -> String {
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let part = |r: Range<usize>| hex_lower(b.get(r).unwrap_or_default());
    format!(
        "{}-{}-{}-{}-{}",
        part(0..4),
        part(4..6),
        part(6..8),
        part(8..10),
        part(10..16),
    )
}

/// Lowercase hex without punctuation (Go: `hex.EncodeToString`).
pub(crate) fn hex_lower(b: &[u8]) -> String {
    let digit = |n: u8| char::from_digit(u32::from(n), 16).unwrap_or('0');
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(digit(x >> 4));
        s.push(digit(x & 0x0F));
    }
    s
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::OnceLock};

    use super::{UuidGenerator as _, UuidRandomGenerator, open_once, v4_string};
    use crate::golden::{TestResult, check, check_eq, fields, read_golden, text};

    #[test]
    fn v4_layout_golden() -> TestResult {
        // gofrs/uuid's version and variant bits and text form for random byte patterns.
        let rows = read_golden("uuid.txt")?;
        check(rows.len() == 300, "uuid.txt is complete")?;
        for row in &rows {
            let [raw, want] = fields::<2>(row)?;
            let raw = text(raw)?;
            let mut b = [0_u8; 16];
            for (dst, i) in b.iter_mut().zip((0..raw.len()).step_by(2)) {
                *dst = u8::from_str_radix(raw.get(i..i + 2).ok_or("short hex")?, 16)?;
            }
            check_eq(v4_string(b).as_bytes(), want, raw)?;
        }
        Ok(())
    }

    #[test]
    fn a_failed_open_of_the_random_source_is_not_remembered() -> TestResult {
        // Go's crypto/rand has no file to open, so one failure (say EMFILE) must not make
        // every later Generate fail.
        let cell = OnceLock::new();
        check(
            open_once(&cell, "/nonexistent/pbs-urandom").is_err(),
            "the first open fails",
        )?;
        check(
            open_once(&cell, "/dev/urandom").is_ok(),
            "a later open succeeds",
        )?;
        check(
            open_once(&cell, "/nonexistent/pbs-urandom").is_ok(),
            "the opened file is kept",
        )
    }

    #[test]
    fn random_generator_makes_distinct_v4_uuids() -> TestResult {
        let ids = (0..100)
            .map(|_| UuidRandomGenerator.generate())
            .collect::<Result<BTreeSet<String>, _>>()?;
        check_eq(&ids.len(), &100, "distinct")?;
        for id in &ids {
            let b = id.as_bytes();
            check_eq(&b.len(), &36, id)?;
            check(b.get(14) == Some(&b'4'), "version 4")?;
            check(
                b.get(19).is_some_and(|c| b"89ab".contains(c)),
                "RFC 4122 variant",
            )?;
        }
        Ok(())
    }
}
