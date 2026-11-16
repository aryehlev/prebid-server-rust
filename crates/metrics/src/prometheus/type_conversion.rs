//! Label value lists for preloading.
//!
//! Go: `metrics/prometheus/type_conversion.go`.

use std::fmt;

use crate::goconv;

/// Go: `enumAsString` — the string value of every enumeration value.
pub(super) fn enum_as_string<T: fmt::Display>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Go: `enumAsLowerCaseString` — every value lowercased with `strings.ToLower`.
pub(super) fn enum_as_lower_case_string<T: AsRef<str>>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .map(|v| goconv::to_lower(v.as_ref()))
        .collect()
}

/// Go: `boolValuesAsString` — `strconv.FormatBool(true)` and `strconv.FormatBool(false)`.
pub(super) fn bool_values_as_string() -> Vec<String> {
    vec!["true".to_owned(), "false".to_owned()]
}
