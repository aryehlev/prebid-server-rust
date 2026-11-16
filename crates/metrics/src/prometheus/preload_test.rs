//! Go: `metrics/prometheus/preload_test.go`.

use std::{collections::BTreeSet, error::Error};

use super::{LabelValues, register_label_permutations};
use crate::prometheus::registry::PromLabels;

type TestResult = Result<(), Box<dyn Error>>;

fn values(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn labels(pairs: &[(&str, &str)]) -> PromLabels {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn permutations(labels_with_values: &LabelValues<'_>) -> BTreeSet<PromLabels> {
    let mut result = BTreeSet::new();
    register_label_permutations(labels_with_values, |l| {
        let _inserted = result.insert(l);
    });
    result
}

/// Go: `TestRegisterLabelPermutations`.
#[test]
fn register_label_permutations_produces_every_combination() -> TestResult {
    let a = values(&["A"]);
    let ab = values(&["A", "B"]);
    let cd = values(&["C", "D"]);

    ensure_eq!(permutations(&[]), BTreeSet::new(), "Empty set.");
    ensure_eq!(
        permutations(&[("1", &a)]),
        BTreeSet::from([labels(&[("1", "A")])]),
        "Set of 1 label and 1 value."
    );
    ensure_eq!(
        permutations(&[("1", &ab)]),
        BTreeSet::from([labels(&[("1", "A")]), labels(&[("1", "B")])]),
        "Set of 1 label and 2 values."
    );
    ensure_eq!(
        permutations(&[("1", &ab), ("2", &cd)]),
        BTreeSet::from([
            labels(&[("1", "A"), ("2", "C")]),
            labels(&[("1", "A"), ("2", "D")]),
            labels(&[("1", "B"), ("2", "C")]),
            labels(&[("1", "B"), ("2", "D")]),
        ]),
        "Set of 2 labels and 2 values."
    );
    Ok(())
}

/// A label without values (an empty syncer list, a module without stages) preloads nothing.
#[test]
fn register_label_permutations_with_an_empty_value_list_registers_nothing() -> TestResult {
    let ab = values(&["A", "B"]);
    ensure_eq!(
        permutations(&[("1", &ab), ("2", &[])]),
        BTreeSet::new(),
        "empty last label"
    );
    ensure_eq!(
        permutations(&[("1", &[]), ("2", &ab)]),
        BTreeSet::new(),
        "empty first label"
    );
    Ok(())
}

/// Repeated values (duplicate syncer keys) register the same combination again.
#[test]
fn register_label_permutations_repeats_duplicate_values() -> TestResult {
    let dup = values(&["dup", "dup"]);
    let mut count = 0_usize;
    register_label_permutations(&[("1", &dup)], |_| count += 1);
    ensure_eq!(count, 2, "every value is registered");
    Ok(())
}
