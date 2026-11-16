//! Go: `internal/strconv/atof.go` (`ParseFloat`).
//!
//! The syntax check (`special`, `readFloat`, `underscoreOK`) and the hexadecimal path
//! (`atofHex`) are ports of Go. Decimal mantissas are converted by Rust's correctly rounded
//! parser, which yields the same value as Go's Eisel-Lemire/decimal fallback.

use super::{NumError, NumErrorKind, lower, underscore_ok};
use crate::math::{NAN, float32};

/// Go: `floatInfo`.
struct FloatInfo {
    mantbits: u32,
    expbits: u32,
    bias: i64,
}

const FLOAT32: FloatInfo = FloatInfo {
    mantbits: 23,
    expbits: 8,
    bias: -127,
};
const FLOAT64: FloatInfo = FloatInfo {
    mantbits: 52,
    expbits: 11,
    bias: -1023,
};

/// Go: `special`: `[+-]inf`, `[+-]infinity` and `nan`, ignoring case. Returns the value and the
/// number of bytes consumed.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let (sign, nsign, rest) = match s.first() {
        Some(b'+') => (1.0, 1, s.get(1..).unwrap_or_default()),
        Some(b'-') => (-1.0, 1, s.get(1..).unwrap_or_default()),
        _ => (1.0, 0, s),
    };
    let common = |word: &[u8]| {
        rest.iter()
            .zip(word)
            .take_while(|(a, b)| lower(**a) == **b)
            .count()
    };
    match rest.first() {
        Some(b'i' | b'I') => {
            let mut n = common(b"infinity");
            // Anything longer than "inf" is ok, but if we don't have "infinity", only consume
            // "inf".
            if 3 < n && n < 8 {
                n = 3;
            }
            (n == 3 || n == 8).then_some((sign * f64::INFINITY, nsign + n))
        }
        // Go: a sign before "nan" is not accepted.
        Some(b'n' | b'N') if nsign == 0 => (common(b"nan") == 3).then_some((NAN, 3)),
        _ => None,
    }
}

/// The result of Go's `readFloat`.
struct ReadFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    /// Bytes consumed.
    n: usize,
    /// For decimal input: the mantissa text without underscores and the (clamped) exponent.
    decimal_text: Vec<u8>,
    decimal_exp: i64,
}

/// The mantissa part of Go's `readFloat` loop.
struct Mantissa {
    value: u64,
    trunc: bool,
    sawdigits: bool,
    /// Go: `dp` once the decimal point was seen (`sawdot`): `nd` at the point, minus the
    /// leading zeros skipped after it. Without a point Go uses `nd`.
    dot: Option<i64>,
    nd: i64,
    nd_mant: i64,
    underscores: bool,
    text: Vec<u8>,
}

/// Go: the digit loop of `readFloat`, starting at `i`. Returns the parts and the next index.
fn read_mantissa(s: &[u8], mut i: usize, base: u64) -> (Mantissa, usize) {
    // 10^19 fits in uint64; 16^16 fits in uint64.
    let max_mant_digits = if base == 16 { 16 } else { 19 };
    let mut m = Mantissa {
        value: 0,
        trunc: false,
        sawdigits: false,
        dot: None,
        nd: 0,
        nd_mant: 0,
        underscores: false,
        text: Vec::new(),
    };
    while let Some(&c) = s.get(i) {
        if c == b'_' {
            m.underscores = true;
        } else if c == b'.' {
            if m.dot.is_some() {
                break;
            }
            m.dot = Some(m.nd);
            m.text.push(c);
        } else if c.is_ascii_digit() {
            m.sawdigits = true;
            m.text.push(c);
            if c == b'0' && m.nd == 0 {
                // ignore leading zeros
                if let Some(dp) = m.dot.as_mut() {
                    *dp -= 1;
                }
            } else {
                m.nd += 1;
                if m.nd_mant < max_mant_digits {
                    m.value = m.value * base + u64::from(c - b'0');
                    m.nd_mant += 1;
                } else if c != b'0' {
                    m.trunc = true;
                }
            }
        } else if base == 16 && lower(c).is_ascii_hexdigit() {
            m.sawdigits = true;
            m.nd += 1;
            if m.nd_mant < max_mant_digits {
                m.value = m.value * 16 + u64::from(lower(c) - b'a' + 10);
                m.nd_mant += 1;
            } else {
                m.trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    (m, i)
}

/// Go: the exponent part of `readFloat` after the exponent character, starting at `i`.
/// Returns the signed (clamped) exponent, whether underscores were seen, and the next index;
/// `None` (with the index reached) when no digit follows.
fn read_exponent(s: &[u8], mut i: usize) -> Result<(i64, bool, usize), usize> {
    let mut esign = 1;
    match s.get(i) {
        None => return Err(i),
        Some(b'+') => i += 1,
        Some(b'-') => {
            i += 1;
            esign = -1;
        }
        Some(_) => {}
    }
    if !s.get(i).is_some_and(u8::is_ascii_digit) {
        return Err(i);
    }
    let mut e: i64 = 0;
    let mut underscores = false;
    while let Some(&c) = s.get(i) {
        if c == b'_' {
            underscores = true;
        } else if c.is_ascii_digit() {
            if e < 10000 {
                e = e * 10 + i64::from(c - b'0');
            }
        } else {
            break;
        }
        i += 1;
    }
    Ok((e * esign, underscores, i))
}

/// Go: `readFloat`. Returns the bytes consumed and, when the prefix is a valid number, its
/// parts.
fn read_float(s: &[u8]) -> (usize, Option<ReadFloat>) {
    let mut i = 0;
    let mut neg = false;
    match s.first() {
        None => return (0, None),
        Some(b'+') => i += 1,
        Some(b'-') => {
            i += 1;
            neg = true;
        }
        Some(_) => {}
    }
    let hex =
        i + 2 < s.len() && s.get(i) == Some(&b'0') && s.get(i + 1).map(|&c| lower(c)) == Some(b'x');
    let (base, exp_char) = if hex {
        i += 2;
        (16, b'p')
    } else {
        (10, b'e')
    };
    let (m, next) = read_mantissa(s, i, base);
    i = next;
    if !m.sawdigits {
        return (i, None);
    }
    let mut dp = m.dot.unwrap_or(m.nd);
    let mut nd_mant = m.nd_mant;
    if hex {
        dp *= 4;
        nd_mant *= 4;
    }
    let mut underscores = m.underscores;
    let mut decimal_exp = 0;
    if s.get(i).map(|&c| lower(c)) == Some(exp_char) {
        match read_exponent(s, i + 1) {
            Ok((e, exp_underscores, after_exp)) => {
                i = after_exp;
                dp += e;
                decimal_exp = e;
                underscores |= exp_underscores;
            }
            Err(at) => return (at, None),
        }
    } else if hex {
        // Must have exponent.
        return (i, None);
    }
    let exp = if m.value == 0 { 0 } else { dp - nd_mant };
    if underscores && !underscore_ok(s.get(..i).unwrap_or_default()) {
        return (i, None);
    }
    let parsed = ReadFloat {
        mantissa: m.value,
        exp,
        neg,
        trunc: m.trunc,
        hex,
        n: i,
        decimal_text: m.text,
        decimal_exp,
    };
    (i, Some(parsed))
}

/// Go: `atofHex`. Returns the float bits for `flt` and whether the value overflowed.
fn atof_hex(flt: &FloatInfo, r: &ReadFloat) -> (u64, bool) {
    let max_exp = (1_i64 << flt.expbits) + flt.bias - 2;
    let min_exp = flt.bias + 1;
    let mut mantissa = r.mantissa;
    let mut exp = r.exp + i64::from(flt.mantbits); // mantissa now implicitly divided by 2^mantbits.

    // Shift mantissa and exponent to bring representation into float range: a leading 1-bit,
    // mantbits other bits and two rounding bits (the bottom one sticky).
    while mantissa != 0 && mantissa >> (flt.mantbits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if r.trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + flt.mantbits + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // If exponent is too negative, denormalize in hopes of making it representable.
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Round using two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1; // round to even (round up if mantissa is odd)
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + flt.mantbits) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> flt.mantbits == 0 {
        // Denormal or zero.
        exp = flt.bias;
    }
    let overflow = exp > max_exp;
    if overflow {
        // infinity and range error
        mantissa = 1 << flt.mantbits;
        exp = max_exp + 1;
    }
    let mut bits = mantissa & ((1 << flt.mantbits) - 1);
    let exp_field = u64::try_from((exp - flt.bias) & ((1 << flt.expbits) - 1)).unwrap_or(0);
    bits |= exp_field << flt.mantbits;
    if r.neg {
        bits |= 1 << flt.mantbits << flt.expbits;
    }
    (bits, overflow)
}

/// Parses a validated decimal number with Rust's correctly rounded parser.
fn parse_decimal(r: &ReadFloat, bit_size: i64) -> (f64, bool) {
    let mut text = String::with_capacity(r.decimal_text.len() + 8);
    if r.neg {
        text.push('-');
    }
    text.extend(r.decimal_text.iter().copied().map(char::from));
    text.push('e');
    text.push_str(&r.decimal_exp.to_string());
    if bit_size == 32 {
        let v = text.parse::<f32>().unwrap_or(0.0);
        (f64::from(v), v.is_infinite())
    } else {
        let v = text.parse::<f64>().unwrap_or(0.0);
        (v, v.is_infinite())
    }
}

/// Go: `strconv.ParseFloat(s, bitSize)`.
///
/// Accepts Go's syntax: an optional sign, decimal or `0x` hexadecimal mantissas (hexadecimal
/// requires a `p` exponent), underscores between digits only after a base prefix, and `inf`,
/// `infinity`, `nan` in any case. `bit_size == 32` rounds to `float32`; any other value means
/// `float64`.
///
/// # Errors
///
/// A [`NumError`] with `Func` `ParseFloat`: [`NumErrorKind::Syntax`] for malformed input and
/// [`NumErrorKind::Range`] when the value overflows (Go then also returns ±Inf). Underflow to
/// zero is not an error.
pub fn parse_float(s: &str, bit_size: i64) -> Result<f64, NumError> {
    match parse_float_pair(s, bit_size) {
        (val, None) => Ok(val),
        (_, Some(err)) => Err(err),
    }
}

/// Go: `strconv.ParseFloat` returning Go's `(value, err)` pair: ±Inf with a range error, 0 with
/// a syntax error.
#[must_use]
pub fn parse_float_pair(s: &str, bit_size: i64) -> (f64, Option<NumError>) {
    let (val, err) = parse_float_raw(s, bit_size);
    (val, err.map(|err| NumError::new("ParseFloat", s, err)))
}

fn parse_float_raw(s: &str, bit_size: i64) -> (f64, Option<NumErrorKind>) {
    let syntax = (0.0, Some(NumErrorKind::Syntax));
    let bytes = s.as_bytes();
    if let Some((val, n)) = special(bytes) {
        if n != bytes.len() {
            return syntax;
        }
        return (
            if bit_size == 32 {
                f64::from(float32(val))
            } else {
                val
            },
            None,
        );
    }
    let (n, parsed) = read_float(bytes);
    let Some(r) = parsed else {
        return syntax;
    };
    if n != bytes.len() || r.n != n {
        return syntax;
    }
    let (val, overflow) = if r.hex {
        let flt = if bit_size == 32 { &FLOAT32 } else { &FLOAT64 };
        let (bits, overflow) = atof_hex(flt, &r);
        let val = if bit_size == 32 {
            f64::from(f32::from_bits(u32::try_from(bits).unwrap_or(0)))
        } else {
            f64::from_bits(bits)
        };
        (val, overflow)
    } else {
        parse_decimal(&r, bit_size)
    };
    (val, overflow.then_some(NumErrorKind::Range))
}
