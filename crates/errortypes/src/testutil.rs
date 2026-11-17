//! Test helpers: comparisons that return `Err` instead of panicking, so tests that use `?` stay
//! free of assertions (clippy `panic_in_result_fn`).

use std::fmt::Debug;

/// Compares `got` with `want`.
///
/// # Errors
///
/// Returns a message naming `what` and both values when they differ.
pub fn ensure_eq<T: PartialEq + Debug + ?Sized>(
    got: &T,
    want: &T,
    what: &str,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}
