//! Go `util/sliceutil`.

use std::{collections::HashMap, hash::Hash};

/// Go: `sliceutil.EqualIgnoreOrder`: whether both slices hold the same elements the same number
/// of times, in any order. Empty and nil slices are equal.
#[must_use]
pub fn equal_ignore_order<T: Eq + Hash>(s1: &[T], s2: &[T]) -> bool {
    if s1.len() != s2.len() {
        return false;
    }
    counts(s1) == counts(s2)
}

/// How many times each element occurs.
fn counts<T: Eq + Hash>(s: &[T]) -> HashMap<&T, usize> {
    let mut counts: HashMap<&T, usize> = HashMap::with_capacity(s.len());
    for item in s {
        *counts.entry(item).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::equal_ignore_order;
    use crate::golden::{TestResult, check_eq};

    #[test]
    fn equal_ignore_order_go_cases() -> TestResult {
        // Go: TestEqualIgnoreOrderInt (a nil slice is an empty one) and
        // TestEqualIgnoreOrderString.
        let ints: [(&str, &[i64], &[i64], bool); 16] = [
            ("nil-both", &[], &[], true),
            ("nil-s1", &[], &[1], false),
            ("nil-s2", &[1], &[], false),
            ("empty-both", &[], &[], true),
            ("empty-s1", &[], &[1], false),
            ("empty-s2", &[1], &[], false),
            ("nil-empty-mix-1", &[], &[], true),
            ("nil-empty-mix-2", &[], &[], true),
            ("one-same", &[1], &[1], true),
            ("one-different", &[1], &[2], false),
            ("many-same-ordered", &[1, 2], &[1, 2], true),
            ("many-same-unordered", &[1, 2], &[2, 1], true),
            ("many-different-lengths", &[1], &[2, 3], false),
            ("many-different", &[1, 2], &[3, 4], false),
            (
                "many-different-duplicates-unordered",
                &[1, 2, 2, 3],
                &[2, 1, 2, 4],
                false,
            ),
            (
                "many-same-duplicates-unordered",
                &[1, 2, 2],
                &[2, 1, 2],
                true,
            ),
        ];
        for (name, s1, s2, want) in ints {
            check_eq(&equal_ignore_order(s1, s2), &want, name)?;
        }
        let strings: [(&str, &[&str], &[&str], bool); 4] = [
            ("one-same", &["a"], &["a"], true),
            ("many-same-unordered", &["a", "b"], &["b", "a"], true),
            ("case-sensitive", &["a"], &["A"], false),
            ("duplicates", &["a", "a", "b"], &["a", "b", "b"], false),
        ];
        for (name, s1, s2, want) in strings {
            check_eq(&equal_ignore_order(s1, s2), &want, name)?;
        }
        Ok(())
    }
}
