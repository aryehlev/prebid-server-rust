//! Go: `internal/strconv/atoi.go` (`ParseUint`, `ParseInt`, `Atoi`).
//!
//! The `*_pair` functions return Go's `(value, error)` pair: on a range error Go still returns
//! the clamped value (the maximum or minimum of the bit size), which callers that ignore the
//! error (`v, _ := strconv.ParseInt(...)`) observe.

use super::{INT_SIZE, NumError, NumErrorKind, lower, underscore_ok};

/// Go: `internal/strconv.ParseUint`: the value Go returns and the unwrapped error.
fn parse_uint_raw(s: &str, base: i64, bit_size: i64) -> (u64, Option<NumErrorKind>) {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return (0, Some(NumErrorKind::Syntax));
    };
    let base0 = base == 0;
    let (base, digits): (u64, &[u8]) = match base {
        2..=36 => (base.unsigned_abs(), bytes),
        0 => {
            if first == b'0' {
                match bytes.get(1).copied().map(lower) {
                    Some(b'b') if bytes.len() >= 3 => (2, bytes.get(2..).unwrap_or_default()),
                    Some(b'o') if bytes.len() >= 3 => (8, bytes.get(2..).unwrap_or_default()),
                    Some(b'x') if bytes.len() >= 3 => (16, bytes.get(2..).unwrap_or_default()),
                    _ => (8, bytes.get(1..).unwrap_or_default()),
                }
            } else {
                (10, bytes)
            }
        }
        _ => return (0, Some(NumErrorKind::Base(base))),
    };
    let bit_size = match bit_size {
        0 => INT_SIZE,
        1..=64 => bit_size,
        _ => return (0, Some(NumErrorKind::BitSize(bit_size))),
    };
    // Go: cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / base + 1;
    let max_val = if bit_size == 64 {
        u64::MAX
    } else {
        (1_u64 << bit_size) - 1
    };

    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in digits {
        let d = match c {
            b'_' if base0 => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => u64::from(c - b'0'),
            _ if lower(c).is_ascii_lowercase() => u64::from(lower(c) - b'a') + 10,
            _ => return (0, Some(NumErrorKind::Syntax)),
        };
        if d >= base {
            return (0, Some(NumErrorKind::Syntax));
        }
        if n >= cutoff {
            // n*base overflows
            return (max_val, Some(NumErrorKind::Range));
        }
        n *= base;
        match n.checked_add(d) {
            Some(n1) if n1 <= max_val => n = n1,
            _ => return (max_val, Some(NumErrorKind::Range)),
        }
    }
    if underscores && !underscore_ok(bytes) {
        return (0, Some(NumErrorKind::Syntax));
    }
    (n, None)
}

/// Go: `internal/strconv.ParseInt`: the value Go returns and the unwrapped error.
fn parse_int_raw(s: &str, base: i64, bit_size: i64) -> (i64, Option<NumErrorKind>) {
    let (neg, rest) = match s.as_bytes().first() {
        None => return (0, Some(NumErrorKind::Syntax)),
        Some(b'+') => (false, s.get(1..).unwrap_or_default()),
        Some(b'-') => (true, s.get(1..).unwrap_or_default()),
        Some(_) => (false, s),
    };
    let un = match parse_uint_raw(rest, base, bit_size) {
        (un, None | Some(NumErrorKind::Range)) => un,
        (_, Some(err)) => return (0, Some(err)),
    };
    let bit_size = if bit_size == 0 { INT_SIZE } else { bit_size };
    let cutoff = 1_u64 << (bit_size - 1);
    let max = i64::try_from(cutoff - 1).unwrap_or(i64::MAX);
    let min = max.wrapping_neg().wrapping_sub(1);
    if !neg && un >= cutoff {
        return (max, Some(NumErrorKind::Range));
    }
    if neg && un > cutoff {
        return (min, Some(NumErrorKind::Range));
    }
    let n = if un == cutoff {
        min
    } else {
        i64::try_from(un).unwrap_or(max)
    };
    (if neg && un != cutoff { -n } else { n }, None)
}

fn to_result<T>(
    func: &'static str,
    s: &str,
    (value, err): (T, Option<NumErrorKind>),
) -> Result<T, NumError> {
    err.map_or_else(|| Ok(value), |kind| Err(NumError::new(func, s, kind)))
}

fn to_pair<T>(
    func: &'static str,
    s: &str,
    (value, err): (T, Option<NumErrorKind>),
) -> (T, Option<NumError>) {
    (value, err.map(|err| NumError::new(func, s, err)))
}

/// Go: `strconv.ParseUint(s, base, bitSize)`.
///
/// # Errors
///
/// A [`NumError`] with `Func` `ParseUint` for a syntax, range, base or bit-size error.
pub fn parse_uint(s: &str, base: i64, bit_size: i64) -> Result<u64, NumError> {
    to_result("ParseUint", s, parse_uint_raw(s, base, bit_size))
}

/// Go: `strconv.ParseUint` returning Go's `(value, err)` pair (the maximum value on a range
/// error, 0 otherwise).
#[must_use]
pub fn parse_uint_pair(s: &str, base: i64, bit_size: i64) -> (u64, Option<NumError>) {
    to_pair("ParseUint", s, parse_uint_raw(s, base, bit_size))
}

/// Go: `strconv.ParseInt(s, base, bitSize)`.
///
/// # Errors
///
/// A [`NumError`] with `Func` `ParseInt` for a syntax, range, base or bit-size error.
pub fn parse_int(s: &str, base: i64, bit_size: i64) -> Result<i64, NumError> {
    to_result("ParseInt", s, parse_int_raw(s, base, bit_size))
}

/// Go: `strconv.ParseInt` returning Go's `(value, err)` pair (the clamped value on a range
/// error, 0 otherwise).
#[must_use]
pub fn parse_int_pair(s: &str, base: i64, bit_size: i64) -> (i64, Option<NumError>) {
    to_pair("ParseInt", s, parse_int_raw(s, base, bit_size))
}

/// Go: `internal/strconv.Atoi`.
fn atoi_raw(s: &str) -> (i64, Option<NumErrorKind>) {
    let bytes = s.as_bytes();
    if !(1..19).contains(&bytes.len()) {
        return parse_int_raw(s, 10, 0);
    }
    let (neg, digits) = match bytes.first() {
        Some(b'-') => (true, bytes.get(1..).unwrap_or_default()),
        Some(b'+') => (false, bytes.get(1..).unwrap_or_default()),
        _ => (false, bytes),
    };
    if digits.is_empty() {
        return (0, Some(NumErrorKind::Syntax));
    }
    let mut n: i64 = 0;
    for &ch in digits {
        if !ch.is_ascii_digit() {
            return (0, Some(NumErrorKind::Syntax));
        }
        // At most 18 digits, so this cannot overflow.
        n = n * 10 + i64::from(ch - b'0');
    }
    (if neg { -n } else { n }, None)
}

/// Go: `strconv.Atoi(s)` (`ParseInt(s, 10, 0)` with a fast path for short inputs).
///
/// # Errors
///
/// A [`NumError`] with `Func` `Atoi`.
pub fn atoi(s: &str) -> Result<i64, NumError> {
    to_result("Atoi", s, atoi_raw(s))
}

/// Go: `strconv.Atoi` returning Go's `(value, err)` pair.
#[must_use]
pub fn atoi_pair(s: &str) -> (i64, Option<NumError>) {
    to_pair("Atoi", s, atoi_raw(s))
}
