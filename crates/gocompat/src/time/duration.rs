//! Go: `time.Duration` with `String`, the float conversions and `ParseDuration`.

use std::{error::Error as StdError, fmt as stdfmt};

use super::parse::{Bad, leading_int, quote};
use crate::math;

/// Go: `time.Duration`, a signed count of nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Duration(pub i64);

/// Go: `float64(x)` for an `int64`, rounded to nearest even.
fn float64(x: i64) -> f64 {
    let u = x.unsigned_abs();
    let hi = u32::try_from(u >> 32).unwrap_or(0);
    let lo = u32::try_from(u & 0xFFFF_FFFF).unwrap_or(0);
    // hi * 2^32 is exact, so the sum rounds once.
    let f = f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo));
    if x < 0 { -f } else { f }
}

/// Go: `fmtFrac`: the fraction of `v / 10^prec` as `.ddd` without trailing zeros (empty when
/// zero), and `v / 10^prec`.
fn fmt_frac(v: u64, prec: u32) -> (String, u64) {
    let scale = 10_u64.pow(prec);
    let digits = format!(
        "{:0width$}",
        v % scale,
        width = usize::try_from(prec).unwrap_or(0)
    );
    let digits = digits.trim_end_matches('0');
    let frac = if digits.is_empty() || prec == 0 {
        String::new()
    } else {
        format!(".{digits}")
    };
    (frac, v / scale)
}

impl Duration {
    /// Go: `time.Hour`.
    pub const HOUR: Self = Self(3_600_000_000_000);
    /// Go: `time.Microsecond`.
    pub const MICROSECOND: Self = Self(1000);
    /// Go: `time.Millisecond`.
    pub const MILLISECOND: Self = Self(1_000_000);
    /// Go: `time.Minute`.
    pub const MINUTE: Self = Self(60_000_000_000);
    /// Go: `time.Nanosecond`.
    pub const NANOSECOND: Self = Self(1);
    /// Go: `time.Second`.
    pub const SECOND: Self = Self(1_000_000_000);

    /// Go: `Duration.String`, such as `1h2m0.5s`, `1.5ms` or `0s`.
    #[must_use]
    pub fn string(self) -> String {
        let u = self.0.unsigned_abs();
        let mut out = String::new();
        if self.0 < 0 {
            out.push('-');
        }
        if u < 1_000_000_000 {
            if u == 0 {
                return "0s".to_owned();
            }
            // U+00B5 is Go's micro sign.
            let (prec, unit) = if u < 1000 {
                (0, "ns")
            } else if u < 1_000_000 {
                (3, "\u{b5}s")
            } else {
                (6, "ms")
            };
            let (frac, int) = fmt_frac(u, prec);
            out.push_str(&int.to_string());
            out.push_str(&frac);
            out.push_str(unit);
            return out;
        }
        let (frac, secs) = fmt_frac(u, 9);
        let minutes = secs / 60;
        if minutes > 0 {
            let hours = minutes / 60;
            if hours > 0 {
                out.push_str(&hours.to_string());
                out.push('h');
            }
            out.push_str(&(minutes % 60).to_string());
            out.push('m');
        }
        out.push_str(&(secs % 60).to_string());
        out.push_str(&frac);
        out.push('s');
        out
    }

    /// Go: `Duration.Nanoseconds`.
    #[must_use]
    pub const fn nanoseconds(self) -> i64 {
        self.0
    }

    /// Go: `Duration.Microseconds`.
    #[must_use]
    pub const fn microseconds(self) -> i64 {
        self.0 / 1000
    }

    /// Go: `Duration.Milliseconds`.
    #[must_use]
    pub const fn milliseconds(self) -> i64 {
        self.0 / 1_000_000
    }

    /// Go: `Duration.Seconds`.
    #[must_use]
    pub fn seconds(self) -> f64 {
        float64(self.0 / Self::SECOND.0) + float64(self.0 % Self::SECOND.0) / 1e9
    }

    /// Go: `Duration.Minutes`.
    #[must_use]
    pub fn minutes(self) -> f64 {
        float64(self.0 / Self::MINUTE.0) + float64(self.0 % Self::MINUTE.0) / (60.0 * 1e9)
    }

    /// Go: `Duration.Hours`.
    #[must_use]
    pub fn hours(self) -> f64 {
        float64(self.0 / Self::HOUR.0) + float64(self.0 % Self::HOUR.0) / (60.0 * 60.0 * 1e9)
    }
}

impl stdfmt::Display for Duration {
    /// Go: `Duration.String`.
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        f.write_str(&self.string())
    }
}

/// Go: the error of `time.ParseDuration`, such as `time: invalid duration "1x"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationError {
    message: String,
    value: Vec<u8>,
}

impl DurationError {
    fn new(message: &str, value: &[u8]) -> Self {
        Self {
            message: message.to_owned(),
            value: value.to_vec(),
        }
    }

    fn invalid(value: &[u8]) -> Self {
        Self::new("invalid duration", value)
    }
}

impl stdfmt::Display for DurationError {
    fn fmt(&self, f: &mut stdfmt::Formatter<'_>) -> stdfmt::Result {
        f.write_str("time: ")?;
        f.write_str(&self.message)?;
        f.write_str(" ")?;
        f.write_str(&quote(&self.value))
    }
}

impl StdError for DurationError {}

/// Go: `leadingFraction`: the digits of `s` that fit, their scale and the rest.
fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let (mut x, mut scale, mut overflow) = (0_u64, 1.0, false);
    let digits = s.iter().take_while(|c| c.is_ascii_digit()).count();
    for &c in s.iter().take(digits) {
        if overflow {
            continue;
        }
        if x > i64::MAX.cast_unsigned() / 10 {
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, s.get(digits..).unwrap_or_default())
}

/// Go: `unitMap`.
const fn unit(u: &[u8]) -> Option<u64> {
    match u {
        b"ns" => Some(1),
        // "µs" (U+00B5, the micro sign) and "μs" (U+03BC, Greek mu).
        b"us" | b"\xc2\xb5s" | b"\xce\xbcs" => Some(1000),
        b"ms" => Some(1_000_000),
        b"s" => Some(1_000_000_000),
        b"m" => Some(60_000_000_000),
        b"h" => Some(3_600_000_000_000),
        _ => None,
    }
}

/// Go: `float64(x)` for a `uint64`.
fn float64_unsigned(x: u64) -> f64 {
    let hi = u32::try_from(x >> 32).unwrap_or(0);
    let lo = u32::try_from(x & 0xFFFF_FFFF).unwrap_or(0);
    f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
}

/// One `[0-9]*(\.[0-9]*)?[a-z]+` term of a duration, in nanoseconds, and the rest.
fn term<'s>(s: &'s [u8], orig: &[u8]) -> Result<(u64, &'s [u8]), DurationError> {
    let invalid = || DurationError::invalid(orig);
    if !s.first().is_some_and(|&c| c == b'.' || c.is_ascii_digit()) {
        return Err(invalid());
    }
    let (v, mut s_rest) = leading_int(s).map_err(|Bad| invalid())?;
    let pre = s_rest.len() != s.len();
    let (mut f, mut scale, mut post) = (0, 1.0, false);
    if let Some(after_dot) = s_rest.strip_prefix(b".") {
        let (frac, frac_scale, rest) = leading_fraction(after_dot);
        post = rest.len() != after_dot.len();
        (f, scale, s_rest) = (frac, frac_scale, rest);
    }
    if !pre && !post {
        return Err(invalid());
    }
    let unit_len = s_rest
        .iter()
        .take_while(|&&c| c != b'.' && !c.is_ascii_digit())
        .count();
    if unit_len == 0 {
        return Err(DurationError::new("missing unit in duration", orig));
    }
    let (u, rest) = s_rest.split_at(unit_len);
    let Some(unit) = unit(u) else {
        let message = format!("unknown unit {} in duration", quote(u));
        return Err(DurationError::new(&message, orig));
    };
    if v > (1 << 63) / unit {
        return Err(invalid());
    }
    let mut v = v * unit;
    if f > 0 {
        // float64 is nanosecond-accurate for fractions of hours.
        let extra = float64_unsigned(f) * (float64_unsigned(unit) / scale);
        v += math::int64(extra).cast_unsigned();
        if v > 1 << 63 {
            return Err(invalid());
        }
    }
    Ok((v, rest))
}

/// Go: `time.ParseDuration`: a signed sequence of decimal numbers with optional fractions and
/// unit suffixes (`ns`, `us`/`µs`/`μs`, `ms`, `s`, `m`, `h`), such as `300ms` or `-1.5h`.
///
/// # Errors
///
/// [`DurationError`] with Go's text: `time: invalid duration "x"`, `time: missing unit in
/// duration "1"` or `time: unknown unit "d" in duration "1d"`.
pub fn parse_duration(s: &str) -> Result<Duration, DurationError> {
    parse_duration_bytes(s.as_bytes())
}

/// Go: `time.ParseDuration` over a Go string.
pub(super) fn parse_duration_bytes(orig: &[u8]) -> Result<Duration, DurationError> {
    let (neg, mut input) = match orig.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        Some(_) | None => (false, orig),
    };
    if input == b"0" {
        return Ok(Duration(0));
    }
    if input.is_empty() {
        return Err(DurationError::invalid(orig));
    }
    let mut d: u64 = 0;
    while !input.is_empty() {
        let (v, rest) = term(input, orig)?;
        input = rest;
        d = d.wrapping_add(v);
        if d > 1 << 63 {
            return Err(DurationError::invalid(orig));
        }
    }
    if neg {
        return Ok(Duration(d.cast_signed().wrapping_neg()));
    }
    if d > i64::MAX.cast_unsigned() {
        return Err(DurationError::invalid(orig));
    }
    Ok(Duration(d.cast_signed()))
}
