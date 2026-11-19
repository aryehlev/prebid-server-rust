//! Go's `omitempty` tests, for `skip_serializing_if`.

/// Go's `omitempty` test for a number, bool or string field: `true` when `value` is its type's
/// zero value (`T::default()`). Like Go, `-0.0` counts as zero and NaN does not.
#[must_use]
pub fn is_zero<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Go's `omitempty` test for a slice or map field that may be nil: `true` when it is nil (`None`)
/// or holds nothing.
#[must_use]
pub fn is_nil_or_empty<T>(value: &Option<T>) -> bool
where
    for<'a> &'a T: IntoIterator,
{
    value
        .as_ref()
        .is_none_or(|inner| inner.into_iter().next().is_none())
}
