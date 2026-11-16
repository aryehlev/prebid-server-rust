//! Go `math` functions and numeric conversions whose Rust counterparts behave differently.
//!
//! Functions with identical semantics are not wrapped: Go's `math.Round` (half away from zero),
//! `Floor`, `Ceil`, `Trunc`, `Abs`, `Sqrt` equal Rust's `f64::round`, `floor`, `ceil`, `trunc`,
//! `abs`, `sqrt`, and `math.Pow` of exactly representable powers equals `f64::powf`/`powi`.
//!
//! - [`NAN`]: Go's NaN bit pattern, which `strconv.FormatFloat(x, 'b', ..)` and `math.Float64bits`
//!   expose.
//! - [`max`] and [`min`]: Go propagates NaN (Rust returns the other operand) and orders the zeros
//!   (`Max(+0, -0) == +0`).
//! - [`float32`]: Go's `float32(x)` conversion (round to nearest, ties to even).
//! - [`int64`]: Go's `int64(x)` conversion as compiled for linux/amd64, the production target:
//!   truncation toward zero, and `math.MinInt64` for NaN and out-of-range values (the `CVTTSD2SQ`
//!   "integer indefinite" result). Rust's `as` saturates instead.

/// Go: `math.NaN()`, whose bits are `0x7FF8000000000001` (Rust's `f64::NAN` has payload 0).
pub const NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0001);

/// Go: `math.Max`.
#[must_use]
pub fn max(x: f64, y: f64) -> f64 {
    if x.is_infinite() && x.is_sign_positive() || y.is_infinite() && y.is_sign_positive() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if x == 0.0 && y == 0.0 {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y { x } else { y }
}

/// Go: `math.Min`.
#[must_use]
pub fn min(x: f64, y: f64) -> f64 {
    if x.is_infinite() && x.is_sign_negative() || y.is_infinite() && y.is_sign_negative() {
        return f64::NEG_INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if x == 0.0 && y == 0.0 {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y { x } else { y }
}

/// Go: `float32(x)`: rounds to the nearest `float32`, ties to even (without a lossy cast).
#[must_use]
pub fn float32(x: f64) -> f32 {
    let bits = x.to_bits();
    let sign = u32::from(bits >> 63 == 1) << 31;
    let exp = i64::try_from((bits >> 52) & 0x7FF).unwrap_or(0);
    let frac = bits & ((1 << 52) - 1);
    if exp == 0x7FF {
        if frac == 0 {
            return f32::from_bits(sign | 0x7F80_0000);
        }
        // Keep the quiet bit and the top payload bits, as the hardware conversion does.
        let payload = u32::try_from(frac >> 29).unwrap_or(0);
        return f32::from_bits(sign | 0x7FC0_0000 | payload);
    }
    if exp == 0 {
        // f64 subnormals (and zero) are far below the smallest f32 subnormal.
        return f32::from_bits(sign);
    }
    let m = frac | (1 << 52);
    let e = exp - 1023;
    let round = |value: u64, shift: u32| -> u64 {
        if shift >= 64 {
            return 0;
        }
        let q = value >> shift;
        let rem = value & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && q & 1 == 1) {
            q + 1
        } else {
            q
        }
    };
    if e < -126 {
        // f32 subnormal: units of 2^-149; m is units of 2^(e-52).
        let shift = u32::try_from(-149 - (e - 52)).unwrap_or(u32::MAX);
        let r = u32::try_from(round(m, shift)).unwrap_or(0);
        return f32::from_bits(sign | r);
    }
    let mut r = round(m, 29);
    let mut e = e;
    if r == 1 << 24 {
        r >>= 1;
        e += 1;
    }
    if e > 127 {
        return f32::from_bits(sign | 0x7F80_0000);
    }
    let biased = u32::try_from(e + 127).unwrap_or(0);
    let mant = u32::try_from(r & 0x007F_FFFF).unwrap_or(0);
    f32::from_bits(sign | (biased << 23) | mant)
}

/// Go: `int64(x)` on linux/amd64: truncates toward zero; NaN and values outside the `int64`
/// range yield `math.MinInt64`.
#[must_use]
pub fn int64(x: f64) -> i64 {
    // 2^63
    const TWO_63: f64 = f64::from_bits(0x43E0_0000_0000_0000);
    if x.is_nan() || x >= TWO_63 || x < -TWO_63 {
        return i64::MIN;
    }
    let bits = x.to_bits();
    let exp = i64::try_from((bits >> 52) & 0x7FF).unwrap_or(0) - 1023;
    if exp < 0 {
        return 0;
    }
    let m = (bits & ((1 << 52) - 1)) | (1 << 52);
    let magnitude = if exp >= 52 {
        m << u32::try_from(exp - 52).unwrap_or(0)
    } else {
        m >> u32::try_from(52 - exp).unwrap_or(0)
    };
    let magnitude = i64::try_from(magnitude).unwrap_or(i64::MIN);
    if x.is_sign_negative() {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_min_follow_go_special_cases() {
        assert!(max(f64::NAN, 1.0).is_nan(), "Max(NaN, 1)");
        assert_eq!(
            max(f64::NAN, f64::INFINITY),
            f64::INFINITY,
            "Max(NaN, +Inf)"
        );
        assert!(max(0.0, -0.0).is_sign_positive(), "Max(+0, -0)");
        assert!(max(-0.0, -0.0).is_sign_negative(), "Max(-0, -0)");
        assert!(min(0.0, -0.0).is_sign_negative(), "Min(+0, -0)");
        assert_eq!(
            min(f64::NAN, f64::NEG_INFINITY),
            f64::NEG_INFINITY,
            "Min(NaN, -Inf)"
        );
        assert!(min(1.0, f64::NAN).is_nan(), "Min(1, NaN)");
        assert_eq!(max(2.5, 3.0).to_bits(), 3.0_f64.to_bits(), "Max");
        assert_eq!(min(2.5, 3.0).to_bits(), 2.5_f64.to_bits(), "Min");
    }

    #[test]
    fn int64_matches_amd64() {
        assert_eq!(int64(f64::NAN), i64::MIN, "NaN");
        assert_eq!(int64(1e19), i64::MIN, "overflow");
        assert_eq!(int64(-1e19), i64::MIN, "underflow");
        assert_eq!(int64(-2.9), -2, "truncates toward zero");
        assert_eq!(int64(2.9), 2, "truncates toward zero");
        assert_eq!(
            int64(-(2_f64.powi(63))),
            i64::MIN,
            "MinInt64 is exact"
        );
        assert_eq!(
            int64(4_503_599_627_370_497.0),
            4_503_599_627_370_497,
            "2^52 + 1"
        );
    }

    #[test]
    fn float32_rounds_to_nearest_even() {
        assert_eq!(float32(0.1).to_bits(), 0x3DCC_CCCD, "0.1");
        assert_eq!(float32(1e39).to_bits(), 0x7F80_0000, "overflow");
        assert_eq!(float32(1e-46).to_bits(), 0, "underflow");
        assert_eq!(float32(-0.0).to_bits(), 0x8000_0000, "negative zero");
        // 1 + 2^-24 is exactly halfway between 1 and the next float32: ties to even (1).
        assert_eq!(
            float32(1.0 + 2_f64.powi(-24)).to_bits(),
            0x3F80_0000,
            "tie"
        );
    }
}
