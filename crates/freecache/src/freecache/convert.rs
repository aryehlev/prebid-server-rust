//! Go integer conversions and slice helpers used by the port.
//!
//! freecache does its arithmetic in Go `int`/`int64` (64-bit on every platform Prebid runs on) and
//! converts with Go's truncating conversions. These helpers give the same results without
//! panicking: an out-of-range slice that Go would panic on yields an empty slice instead, which
//! only matters on paths that freecache's invariants make unreachable.

/// Go `uint32(x)` for a Go `int`/`int64`: the low 32 bits.
pub(super) const fn low_u32(x: i64) -> u32 {
    (x.cast_unsigned() & 0xFFFF_FFFF) as u32
}

/// Go `int64(len(x))`. Lengths of Rust allocations always fit.
pub(super) fn wide(x: usize) -> i64 {
    i64::try_from(x).unwrap_or(i64::MAX)
}

/// A Go `int` used as a slice index, or `None` when it is negative.
pub(super) fn index(x: i64) -> Option<usize> {
    usize::try_from(x).ok()
}

/// Go `float64(x)` for an `int64`: rounds to nearest, ties to even.
///
/// Both halves convert exactly and the single addition rounds once, so the result is the
/// correctly rounded value of `x`.
pub(super) fn int64_to_float64(x: i64) -> f64 {
    let high = f64::from(i32::try_from(x >> 32).unwrap_or(0));
    let low = f64::from(low_u32(x));
    high.mul_add(4_294_967_296.0, low)
}

/// Go `data[start:end]`; empty when the bounds are invalid.
pub(super) fn span(data: &[u8], start: i64, end: i64) -> &[u8] {
    match (index(start), index(end)) {
        (Some(start), Some(end)) => data.get(start..end).unwrap_or_default(),
        _ => &[],
    }
}

/// Go `data[start:end]` for writing; empty when the bounds are invalid.
pub(super) fn span_mut(data: &mut [u8], start: i64, end: i64) -> &mut [u8] {
    match (index(start), index(end)) {
        (Some(start), Some(end)) => data.get_mut(start..end).unwrap_or_default(),
        _ => &mut [],
    }
}

/// Go `copy(dst, src)`: copies `min(len(dst), len(src))` elements and returns the count.
pub(super) fn copy<T: Copy>(dst: &mut [T], src: &[T]) -> usize {
    let n = dst.len().min(src.len());
    if let (Some(dst), Some(src)) = (dst.get_mut(..n), src.get(..n)) {
        dst.copy_from_slice(src);
    }
    n
}

/// Go `copy(data[dst:], data[start:end])` inside one buffer, with `memmove` semantics.
///
/// Returns the count copied as a Go `int`; nothing is copied when a bound is invalid.
pub(super) fn copy_within(data: &mut [u8], start: i64, end: i64, dst: i64) -> i64 {
    let (Some(start), Some(end), Some(dst)) = (index(start), index(end), index(dst)) else {
        return 0;
    };
    if start > end || end > data.len() || dst > data.len() {
        return 0;
    }
    let n = (end - start).min(data.len() - dst);
    data.copy_within(start..start + n, dst);
    wide(n)
}

#[cfg(test)]
mod tests {
    use super::{
        super::golden::{TestResult, check, check_eq},
        copy, copy_within, int64_to_float64, low_u32, span,
    };

    #[test]
    fn low_u32_truncates_like_go() -> TestResult {
        check_eq(&low_u32(1 << 32), &0, "uint32(1<<32)")?;
        check_eq(&low_u32((1 << 32) + 7), &7, "uint32(1<<32 + 7)")?;
        check_eq(&low_u32(-1), &u32::MAX, "uint32(-1)")?;
        check_eq(&low_u32(i64::MAX), &u32::MAX, "uint32(MaxInt64)")?;
        check_eq(&low_u32(i64::MIN), &0, "uint32(MinInt64)")
    }

    #[test]
    fn int64_to_float64_rounds_like_go() -> TestResult {
        let cases: [(i64, u64); 6] = [
            (0, 0),
            (1, 0x3FF0_0000_0000_0000),
            (-1, 0xBFF0_0000_0000_0000),
            // 2^53 + 1 rounds to even (2^53).
            ((1 << 53) + 1, 0x4340_0000_0000_0000),
            // 2^53 + 3 rounds up to 2^53 + 4.
            ((1 << 53) + 3, 0x4340_0000_0000_0002),
            (i64::MIN, 0xC3E0_0000_0000_0000),
        ];
        for (input, bits) in cases {
            check_eq(
                &int64_to_float64(input).to_bits(),
                &bits,
                &format!("float64({input})"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn copy_helpers_match_go_copy() -> TestResult {
        let mut dst = [0_u8; 3];
        check_eq(&copy(&mut dst, b"abcdef"), &3, "copy into shorter dst")?;
        check_eq(&dst, b"abc", "copied bytes")?;
        let mut data = *b"0123456789";
        check_eq(
            &copy_within(&mut data, 0, 4, 2),
            &4,
            "overlapping forward copy",
        )?;
        check_eq(&data, b"0101236789", "memmove result")?;
        check_eq(
            &copy_within(&mut data, 0, 4, 8),
            &2,
            "copy clipped at the end",
        )?;
        check_eq(&data, b"0101236701", "clipped result")?;
        check_eq(
            &copy_within(&mut data, 4, 2, 0),
            &0,
            "inverted bounds copy nothing",
        )?;
        check(span(&data, 3, 2).is_empty(), "inverted span is empty")
    }
}
