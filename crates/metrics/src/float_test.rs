//! The conversions against the values Go's `float64(n)` gives (go1.26.2, amd64).

use std::error::Error;

use super::{
    float_from_i64 as from_i64, float_from_u64 as from_u64, float_from_usize as from_usize,
    i64_from_float as to_i64,
};

type TestResult = Result<(), Box<dyn Error>>;

/// 2^53, the first integer above which an `f64` cannot hold every integer.
const TWO_POW_53: u64 = 9_007_199_254_740_992;

/// 2^63 and 2^64, as products: clippy reads a literal above `u64::MAX` or `i64::MAX` as lossy.
const TWO_POW_63: f64 = 2_147_483_648.0 * 4_294_967_296.0;
const TWO_POW_64: f64 = 4_294_967_296.0 * 4_294_967_296.0;

/// Below 2^53 Go's conversion is exact.
// Go: metrics/prometheus/prometheus.go:732 (Observe(float64(labels.RequestSize))) and go1.26.2
// src/time/time.go:1090-1094 (Duration.Seconds: float64(sec) + float64(nsec)/1e9)
#[test]
fn whole_numbers_below_two_pow_53_convert_exactly() -> TestResult {
    ensure_eq!(from_u64(0), 0.0, "zero");
    ensure_eq!(from_u64(1), 1.0, "one");
    ensure_eq!(from_u64(4_294_967_295), 4_294_967_295.0, "2^32 - 1");
    ensure_eq!(from_u64(4_294_967_296), 4_294_967_296.0, "2^32");
    ensure_eq!(from_u64(123_456_789), 123_456_789.0, "a request size");
    ensure_eq!(
        from_u64(TWO_POW_53 - 1),
        9_007_199_254_740_991.0,
        "2^53 - 1"
    );
    ensure_eq!(from_usize(1024), 1024.0, "usize");
    ensure_eq!(from_i64(-1), -1.0, "minus one");
    ensure_eq!(from_i64(-4_294_967_296), -4_294_967_296.0, "-2^32");
    ensure_eq!(
        from_i64(9_223_372_036),
        9_223_372_036.0,
        "the seconds of the longest duration"
    );
    ensure_eq!(
        from_i64(-9_223_372_036),
        -9_223_372_036.0,
        "and of the shortest"
    );
    Ok(())
}

/// Above 2^53 Go rounds to the nearest `f64`, a tie to the even one.
// Go: metrics/prometheus/prometheus.go:732 (Observe(float64(labels.RequestSize))) and go1.26.2
// src/time/time.go:1090-1094 (Duration.Seconds)
#[test]
fn larger_numbers_round_to_nearest_even() -> TestResult {
    ensure_eq!(
        from_u64(TWO_POW_53 + 1),
        9_007_199_254_740_992.0,
        "a tie rounds down to even"
    );
    ensure_eq!(
        from_u64(TWO_POW_53 + 3),
        9_007_199_254_740_996.0,
        "a tie rounds up to even"
    );
    ensure_eq!(
        from_u64(123_456_789_012_345_678),
        123_456_789_012_345_680.0,
        "the nearest"
    );
    ensure_eq!(from_u64(u64::MAX), TWO_POW_64, "2^64 - 1 rounds up");
    ensure_eq!(from_i64(i64::MIN), -TWO_POW_63, "-2^63 is exact");
    ensure_eq!(from_i64(i64::MAX), TWO_POW_63, "2^63 - 1 rounds up");
    ensure_eq!(
        from_i64(-9_007_199_254_740_993),
        -9_007_199_254_740_992.0,
        "a negative tie rounds to even"
    );
    Ok(())
}

/// Go's `int64(f)` on amd64 truncates toward zero and gives `math.MinInt64` for NaN and for every
/// value outside `int64`'s range, of either sign.
#[test]
fn float_to_int64_truncates_and_overflows_to_min() -> TestResult {
    // Go: metrics/go_metrics.go:859 (Update(int64(cpm))) on go1.26.2's amd64 conversion
    // (CVTTSD2SQ), which rcrowley/go-metrics cf1acfcdf475 sample.go:290 (int(pos)) also uses
    for (f, expected) in [
        (0.0, 0),
        (-0.0, 0),
        (0.999_999, 0),
        (-0.999_999, 0),
        (1.0, 1),
        (1.5, 1),
        (-1.5, -1),
        (2.5e-300, 0),
        (f64::MIN_POSITIVE, 0),
        (9_007_199_254_740_994.0, 9_007_199_254_740_994),
        (-TWO_POW_63, i64::MIN),
        (TWO_POW_63, i64::MIN),
        (TWO_POW_64, i64::MIN),
        (-TWO_POW_64, i64::MIN),
        (f64::INFINITY, i64::MIN),
        (f64::NEG_INFINITY, i64::MIN),
        (f64::NAN, i64::MIN),
        (
            f64::from_bits(0x43DF_FFFF_FFFF_FFFF),
            9_223_372_036_854_774_784,
        ),
        (
            -f64::from_bits(0x43DF_FFFF_FFFF_FFFF),
            -9_223_372_036_854_774_784,
        ),
    ] {
        ensure_eq!(to_i64(f), expected, "int64({f:?})");
    }
    Ok(())
}
