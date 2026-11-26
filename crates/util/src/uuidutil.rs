//! Go `util/uuidutil`: an injectable generator of random (version 4) UUIDs, on the uuid
//! crate.

use std::{error, fmt};

use rand::{
    TryRng as _,
    rngs::{SysError, SysRng},
};
use uuid::Builder;

/// Go: `uuidutil.UUIDGenerator`.
pub trait UuidGenerator: Send + Sync {
    /// Go: `Generate`: a new UUID in its canonical text form.
    ///
    /// # Errors
    ///
    /// When no UUID can be made, such as when the random source fails.
    fn generate(&self) -> Result<String, UuidError>;
}

/// The error of [`UuidRandomGenerator`]: the operating system's random source failed (Go:
/// the `crypto/rand` error that `uuid.NewV4` returns). Its text is the getrandom crate's.
#[derive(Clone, Copy, Debug)]
pub struct UuidError(SysError);

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
/// operating system's secure random source (Go: `crypto/rand`) with the version and variant
/// bits set.
///
/// uuid's own `Uuid::new_v4` panics when the source fails; this returns the error, as Go does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UuidRandomGenerator;

impl UuidGenerator for UuidRandomGenerator {
    fn generate(&self) -> Result<String, UuidError> {
        let mut b = [0_u8; 16];
        SysRng.try_fill_bytes(&mut b).map_err(UuidError)?;
        Ok(v4_string(b))
    }
}

/// Go: `uuid.UUID.SetVersion(V4)`, `SetVariant(VariantRFC4122)` and `String`: the canonical
/// lower-case `8-4-4-4-12` text of the version 4 UUID made from 16 random bytes.
#[must_use]
pub fn v4_string(b: [u8; 16]) -> String {
    Builder::from_random_bytes(b).into_uuid().to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{UuidGenerator as _, UuidRandomGenerator, v4_string};
    use crate::golden::{TestResult, check, check_eq, fields, read_golden, text};

    #[test]
    fn v4_layout_golden() -> TestResult {
        // Go: gofrs/uuid's version and variant bits and text form for random byte patterns.
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
    fn random_generator_makes_distinct_v4_uuids() -> TestResult {
        // Go: uuid.NewV4 draws 16 bytes from crypto/rand for each UUID.
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
