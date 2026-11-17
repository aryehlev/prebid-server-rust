//! Go `native1/response/ptr.go`.

/// `Int64Ptr` returns pointer to passed argument.
#[must_use]
pub const fn int64_ptr(n: i64) -> Option<i64> {
    Some(n)
}
