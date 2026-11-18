//! Go `util/iterutil`: iterating over the elements of a slice in place.
//!
//! Go's `for _, v := range s` copies each element, so Prebid iterates with
//! `SlicePointerValues` or `SlicePointers` to change elements in place. In Rust these are
//! `iter_mut` and `iter_mut().enumerate()`; the functions name the Go helpers for ported code.

use std::{iter::Enumerate, slice::IterMut};

/// Go: `iterutil.SlicePointerValues`: a mutable reference to each element.
pub fn slice_pointer_values<T>(s: &mut [T]) -> IterMut<'_, T> {
    s.iter_mut()
}

/// Go: `iterutil.SlicePointers`: each index with a mutable reference to its element.
pub fn slice_pointers<T>(s: &mut [T]) -> Enumerate<IterMut<'_, T>> {
    s.iter_mut().enumerate()
}

#[cfg(test)]
mod tests {
    use super::{slice_pointer_values, slice_pointers};
    use crate::golden::{TestResult, check_eq};

    #[test]
    fn changes_are_made_in_place() -> TestResult {
        // Go: TestSlicePointerValues and TestSlicePointers.
        let mut v = vec![1, 2, 3];
        for x in slice_pointer_values(&mut v) {
            *x *= 10;
        }
        check_eq(&v, &vec![10, 20, 30], "values")?;
        for (i, x) in slice_pointers(&mut v) {
            *x += i64::try_from(i)?;
        }
        check_eq(&v, &vec![10, 21, 32], "indices")?;
        let mut stop_early = vec![1, 2, 3];
        for x in slice_pointer_values(&mut stop_early) {
            if *x == 2 {
                break;
            }
            *x = 0;
        }
        check_eq(&stop_early, &vec![0, 2, 3], "breaking stops the iteration")
    }
}
