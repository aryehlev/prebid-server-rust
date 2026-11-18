//! Test support: checks that fail with an error instead of panicking.

use std::fmt::Debug;

use crate::error::{Result, ToolError};

/// Fails with `what` unless `cond` holds.
///
/// # Errors
///
/// When `cond` is false.
pub fn check(cond: bool, what: &str) -> Result<()> {
    if cond {
        Ok(())
    } else {
        Err(ToolError::new(format!("check failed: {what}")))
    }
}

/// Fails, showing both values, unless `actual == expected`.
///
/// # Errors
///
/// When the values differ.
pub fn check_eq<T: PartialEq + Debug + ?Sized>(actual: &T, expected: &T, what: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "{what}: got {actual:?}, want {expected:?}"
        )))
    }
}
