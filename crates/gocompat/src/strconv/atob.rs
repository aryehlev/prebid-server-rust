//! Go: `internal/strconv/atob.go` (`ParseBool`, `FormatBool`).

use super::{NumError, NumErrorKind};

/// Go: `strconv.ParseBool`: accepts `1 t T true TRUE True` and `0 f F false FALSE False`.
///
/// # Errors
///
/// A [`NumError`] with `Func` `ParseBool` and [`NumErrorKind::Syntax`] for any other input.
pub fn parse_bool(s: &str) -> Result<bool, NumError> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(NumError::new("ParseBool", s, NumErrorKind::Syntax)),
    }
}

/// Go: `strconv.FormatBool`.
#[must_use]
pub const fn format_bool(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}
