//! The error type every devtools command returns.

use std::{error::Error as StdError, fmt, io, path::Path, result::Result as StdResult};

/// A devtools failure: a message meant for the person running the tool.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    /// Creates an error with the given message.
    #[must_use]
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Wraps an I/O error with the path it concerns.
    #[must_use]
    pub fn io(path: &Path, err: &io::Error) -> Self {
        Self::new(format!("{}: {err}", path.display()))
    }

    /// The message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

// `main` returns `Result<(), ToolError>` and the runtime prints the error with `Debug`; print the
// message itself so failures read as plain text.
impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for ToolError {}

impl From<io::Error> for ToolError {
    fn from(err: io::Error) -> Self {
        Self::new(err.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(err: serde_json::Error) -> Self {
        Self::new(format!("JSON: {err}"))
    }
}

/// Shorthand for results carrying [`ToolError`].
pub type Result<T, E = ToolError> = StdResult<T, E>;
