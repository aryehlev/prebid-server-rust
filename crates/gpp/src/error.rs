//! The error value returned by every fallible go-gpp operation.
//!
//! go-gpp builds all of its errors with `fmt.Errorf` or `errors.New`, and Prebid Server only ever
//! prints them (`%s`, `%v`, `err.Error()`). The Rust port therefore carries the Go message verbatim
//! and nothing else.

use std::{error::Error, fmt};

/// An error produced by GPP parsing or encoding. Its [`Display`](fmt::Display) output is
/// byte-identical to the Go `error.Error()` text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GppError {
    message: String,
}

impl GppError {
    /// Wraps a fully formatted Go error message.
    pub(crate) const fn new(message: String) -> Self {
        Self { message }
    }

    /// Returns the Go error text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for GppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for GppError {}

#[cfg(test)]
mod tests {
    use super::GppError;

    #[test]
    fn display_is_the_message() {
        let err = GppError::new("numFields is invalid".to_owned());
        assert_eq!(err.to_string(), "numFields is invalid", "display");
        assert_eq!(err.message(), "numFields is invalid", "message");
    }
}
