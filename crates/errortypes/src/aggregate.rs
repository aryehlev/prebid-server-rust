//! Aggregate errors.
//!
//! Go: `errortypes/aggregate.go`. `AggregateError` is not a `Coder`, so converting it into a
//! [`GoError`] yields [`crate::ErrorKind::Generic`] (code 999, fatal).

use std::{error::Error as StdError, fmt};

use crate::GoError;

/// Go: `errortypes.AggregateError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateError {
    /// Go: `AggregateError.Message`.
    pub message: String,
    /// Go: `AggregateError.Errors`.
    pub errors: Vec<GoError>,
}

impl AggregateError {
    /// Go: `errortypes.NewAggregateError`.
    #[must_use]
    pub fn new<M: Into<String>>(msg: M, errs: Vec<GoError>) -> Self {
        Self {
            message: msg.into(),
            errors: errs,
        }
    }
}

impl fmt::Display for AggregateError {
    /// Go: `AggregateError.Error`. Empty when there are no errors; otherwise
    /// `"{msg} (1 error):\n  1: {err}\n"` or `"{msg} (N errors):\n  1: ...\n  2: ...\n"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.errors.is_empty() {
            return Ok(());
        }
        f.write_str(&self.message)?;
        if self.errors.len() == 1 {
            f.write_str(" (1 error):\n")?;
        } else {
            writeln!(f, " ({} errors):", self.errors.len())?;
        }
        for (i, err) in self.errors.iter().enumerate() {
            writeln!(f, "  {}: {}", i.saturating_add(1), err.message())?;
        }
        Ok(())
    }
}

impl StdError for AggregateError {}

impl From<AggregateError> for GoError {
    fn from(value: AggregateError) -> Self {
        Self::generic(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UNKNOWN_ERROR_CODE;

    // Go: errortypes/aggregate_test.go TestAggregateError.
    #[test]
    fn aggregate_error_cases() {
        let cases = [
            ("None", "anyMessage", vec![], ""),
            (
                "One",
                "anyMessage",
                vec![GoError::generic("err1")],
                "anyMessage (1 error):\n  1: err1\n",
            ),
            (
                "Many",
                "anyMessage",
                vec![GoError::generic("err1"), GoError::generic("err2")],
                "anyMessage (2 errors):\n  1: err1\n  2: err2\n",
            ),
        ];
        for (description, message, errors, expected) in cases {
            let err = AggregateError::new(message, errors);
            assert_eq!(err.to_string(), expected, "{description}");
        }
    }

    #[test]
    fn aggregate_is_generic() {
        let err: GoError = AggregateError::new("m", vec![GoError::bad_input("x")]).into();
        assert_eq!(err.code(), UNKNOWN_ERROR_CODE, "not a coder");
        assert_eq!(err.message(), "m (1 error):\n  1: x\n", "message");
    }
}
