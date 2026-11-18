//! Go `openrtb2/ptr.go`.

/// `Int8Ptr` returns pointer to passed argument.
#[must_use]
pub const fn int8_ptr(n: i8) -> Option<i8> {
    Some(n)
}

/// `Int64Ptr` returns pointer to passed argument.
#[must_use]
pub const fn int64_ptr(n: i64) -> Option<i64> {
    Some(n)
}
