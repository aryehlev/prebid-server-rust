//! The parts of Go's `strconv` (and `math/big` number scanning) that the JSON libraries depend on:
//! shortest float formatting, float parsing with Go's error values, and `Quote`.

use core::str::from_utf8;

use crate::unicode::{decode_rune, is_print};

/// Float formatting verb, as in `strconv.FormatFloat`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloatFmt {
    /// `'e'`: `-d.dddde±dd`.
    E,
    /// `'f'`: `-ddd.dddd`.
    F,
}

/// The shortest decimal digits that round-trip a finite float, with the decimal point position:
/// the value is `0.d1d2d3… × 10^dp`. Zero has no digits and `dp == 0`.
struct Decimal {
    digits: Vec<u8>,
    dp: i64,
}

fn shortest_decimal(abs_text: &str) -> Decimal {
    // `abs_text` is Rust's `{:e}` rendering of a non-negative finite float, which is the shortest
    // digit string that round-trips (the same digits Go's Ryu-based shortest formatting picks).
    let (mantissa, exponent) = abs_text.split_once('e').unwrap_or((abs_text, "0"));
    let exp: i64 = exponent.parse().unwrap_or(0);
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    if digits.iter().all(|&d| d == b'0') {
        return Decimal {
            digits: Vec::new(),
            dp: 0,
        };
    }
    Decimal {
        digits,
        dp: exp + 1,
    }
}

fn decimal_of(value: f64, bits32: bool) -> Decimal {
    let text = if bits32 {
        format!("{:e}", f64_to_f32(value).abs())
    } else {
        format!("{:e}", value.abs())
    };
    let mut d = shortest_decimal(&text);
    round_tie_to_even(&mut d, value, bits32);
    d
}

/// The odd mantissa and binary exponent of a finite non-zero float: `|x| == m × 2^e`.
fn odd_parts(value: f64, bits32: bool) -> (u64, i64) {
    let (mant, exp) = if bits32 {
        let bits = f64_to_f32(value).to_bits();
        let frac = u64::from(bits & 0x007F_FFFF);
        match (bits >> 23) & 0xFF {
            0 => (frac, -149),
            biased => (frac | 0x0080_0000, i64::from(biased) - 150),
        }
    } else {
        let bits = value.to_bits();
        let frac = bits & 0x000F_FFFF_FFFF_FFFF;
        match (bits >> 52) & 0x7FF {
            0 => (frac, -1074),
            biased => (
                frac | 0x0010_0000_0000_0000,
                i64::try_from(biased).unwrap_or(0) - 1075,
            ),
        }
    };
    if mant == 0 {
        return (0, 0);
    }
    let zeros = mant.trailing_zeros();
    (mant >> zeros, exp + i64::from(zeros))
}

/// When a float lies exactly halfway between the two nearest shortest candidates, Go's Ryu takes
/// the one with the even last digit while Rust's formatter takes the larger one; this moves `d`
/// to Go's choice.
fn round_tie_to_even(d: &mut Decimal, value: f64, bits32: bool) {
    // Go: strconv/ftoaryu.go ryuDigits32 (`cup` rounds an exact half to the even digit)
    // Rust rounded the half up only if the candidate below is even, so the last digit is odd.
    if d.digits
        .last()
        .is_none_or(|&last| (last - b'0').is_multiple_of(2))
    {
        return;
    }
    let Ok(len) = i64::try_from(d.digits.len()) else {
        return;
    };
    // The last digit's unit is 10^-scale. The halfway point (2D - 1) / (2^(scale+1) × 5^scale)
    // is a binary float only when scale > 0, and has at most 18 digits only when scale <= 25.
    let scale = len - d.dp;
    let Ok(scale32) = u32::try_from(scale) else {
        return;
    };
    if !(1..=25).contains(&scale32) {
        return;
    }
    let (mant, exp2) = odd_parts(value, bits32);
    // With 2D - 1 odd: value == (2D - 1) / (2^(scale+1) × 5^scale)
    //   ⇔ exp2 == -(scale + 1) && mant × 5^scale == 2D - 1.
    if exp2 != -(scale + 1) {
        return;
    }
    let digits = d.digits.iter().fold(0_u128, |acc, &digit| {
        acc.saturating_mul(10)
            .saturating_add(u128::from(digit - b'0'))
    });
    let halfway = u128::from(mant).saturating_mul(5_u128.pow(scale32));
    if halfway != digits.saturating_mul(2).saturating_sub(1) {
        return;
    }
    // Go keeps the even candidate only when it is inside the rounding interval, which is exactly
    // when it parses back to the same float.
    if let Some(last) = d.digits.last_mut() {
        *last -= 1;
    }
    let candidate = format!("{}e{}", from_utf8(&d.digits).unwrap_or("0"), d.dp - len);
    let round_trips = if bits32 {
        candidate.parse::<f32>().map(f32::to_bits).ok() == Some(f64_to_f32(value).abs().to_bits())
    } else {
        candidate.parse::<f64>().map(f64::to_bits).ok() == Some(value.abs().to_bits())
    };
    if !round_trips {
        if let Some(digit) = d.digits.last_mut() {
            *digit += 1;
        }
        return;
    }
    while d.digits.last() == Some(&b'0') {
        let _zero = d.digits.pop();
    }
}

/// Converts an `f64` to the nearest `f32` (round half to even), like Go's `float32(x)`.
pub(crate) fn f64_to_f32(x: f64) -> f32 {
    // Rust's float formatting is exact with enough precision, and parsing rounds correctly, so
    // going through the exact decimal expansion gives the correctly rounded conversion without a
    // lossy `as` cast. Values that are already exact `f32`s take the fast path.
    if x.is_nan() {
        return f32::NAN;
    }
    if x.is_infinite() {
        return if x > 0.0 {
            f32::INFINITY
        } else {
            f32::NEG_INFINITY
        };
    }
    // Fast path: the shortest rendering of an exactly representable value parses back to it.
    if let Ok(candidate) = format!("{x:e}").parse::<f32>()
        && f64::from(candidate).to_bits() == x.to_bits()
    {
        return candidate;
    }
    // 1100 significant digits cover the exact decimal expansion of every finite f64.
    let text = format!("{x:.1100e}");
    text.parse::<f32>().unwrap_or(0.0)
}

/// Appends Go's `strconv.FormatFloat(value, fmt, -1, bits)` for a finite value.
pub(crate) fn append_float(out: &mut Vec<u8>, value: f64, fmt: FloatFmt, bits32: bool) {
    // Go: strconv/ftoa.go (shortest formatting, fmtE and fmtF)
    let negative = if bits32 {
        f64_to_f32(value).is_sign_negative()
    } else {
        value.is_sign_negative()
    };
    if negative {
        out.push(b'-');
    }
    let d = decimal_of(value, bits32);
    let nd = i64::try_from(d.digits.len()).unwrap_or(i64::MAX);
    match fmt {
        FloatFmt::E => {
            out.push(d.digits.first().copied().unwrap_or(b'0'));
            if d.digits.len() > 1 {
                out.push(b'.');
                out.extend_from_slice(d.digits.get(1..).unwrap_or_default());
            }
            out.push(b'e');
            let mut exp = if d.digits.is_empty() { 0 } else { d.dp - 1 };
            if exp < 0 {
                out.push(b'-');
                exp = -exp;
            } else {
                out.push(b'+');
            }
            let text = exp.to_string();
            if text.len() < 2 {
                out.push(b'0');
            }
            out.extend_from_slice(text.as_bytes());
        }
        FloatFmt::F => {
            if d.dp > 0 {
                let mut m = 0_i64;
                while m < d.dp {
                    out.push(digit_at(&d.digits, m));
                    m += 1;
                }
            } else {
                out.push(b'0');
            }
            let prec = (nd - d.dp).max(0);
            if prec > 0 {
                out.push(b'.');
                let mut i = 1_i64;
                while i <= prec {
                    out.push(digit_at(&d.digits, d.dp + i - 1));
                    i += 1;
                }
            }
        }
    }
}

fn digit_at(digits: &[u8], index: i64) -> u8 {
    usize::try_from(index)
        .ok()
        .and_then(|i| digits.get(i).copied())
        .unwrap_or(b'0')
}

/// Go's `fmt` rendering of a non-finite float for `%f`/`%g` (`NaN`, `+Inf`, `-Inf`).
pub(crate) const fn non_finite_text(value: f64) -> &'static str {
    if value.is_nan() {
        "NaN"
    } else if value > 0.0 {
        "+Inf"
    } else {
        "-Inf"
    }
}

/// Why `parse_float` failed (`strconv.ErrSyntax` or `strconv.ErrRange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumErrorKind {
    Syntax,
    Range,
}

/// Go's `strconv.NumError` text: `strconv.<func>: parsing "<input>": <reason>`.
pub(crate) fn num_error_text(func: &str, input: &[u8], kind: NumErrorKind) -> Vec<u8> {
    let mut out = format!("strconv.{func}: parsing ").into_bytes();
    out.extend_from_slice(&quote(input));
    out.extend_from_slice(match kind {
        NumErrorKind::Syntax => b": invalid syntax",
        NumErrorKind::Range => b": value out of range",
    });
    out
}

/// Go's `strconv.NumError` text for `ParseFloat`.
pub(crate) fn parse_float_error_text(input: &[u8], kind: NumErrorKind) -> Vec<u8> {
    num_error_text("ParseFloat", input, kind)
}

/// Whether `s` is a decimal floating-point literal Go's `strconv.ParseFloat` accepts.
fn is_go_decimal_float(s: &[u8]) -> bool {
    // Go: strconv/atof.go readFloat (decimal, no underscores, no special values)
    let mut i = usize::from(matches!(s.first(), Some(b'+' | b'-')));
    let mut saw_digits = false;
    let mut saw_dot = false;
    while let Some(&c) = s.get(i) {
        if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
        } else if c.is_ascii_digit() {
            saw_digits = true;
        } else {
            break;
        }
        i += 1;
    }
    if !saw_digits {
        return false;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !s.get(i).is_some_and(u8::is_ascii_digit) {
            return false;
        }
        while s.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    i == s.len()
}

/// Go's `strconv.ParseFloat(s, 64)` (or 32) for decimal input.
pub(crate) fn parse_float(s: &[u8], bits32: bool) -> Result<f64, NumErrorKind> {
    if !is_go_decimal_float(s) {
        return Err(NumErrorKind::Syntax);
    }
    let text = from_utf8(s).ok().ok_or(NumErrorKind::Syntax)?;
    if bits32 {
        let v: f32 = text.parse().ok().ok_or(NumErrorKind::Syntax)?;
        if v.is_infinite() {
            return Err(NumErrorKind::Range);
        }
        Ok(f64::from(v))
    } else {
        let v: f64 = text.parse().ok().ok_or(NumErrorKind::Syntax)?;
        if v.is_infinite() {
            return Err(NumErrorKind::Range);
        }
        Ok(v)
    }
}

/// The NaN Go's `strconv` returns (`math.NaN()`).
const GO_NAN: u64 = 0x7FF8_0000_0000_0001;

/// Go's `floatInfo`: mantissa bits, exponent bits and exponent bias of a float format.
struct FloatInfo {
    mant_bits: u32,
    exp_bits: u32,
    bias: i64,
}

const FLOAT64_INFO: FloatInfo = FloatInfo {
    mant_bits: 52,
    exp_bits: 11,
    bias: -1023,
};

const FLOAT32_INFO: FloatInfo = FloatInfo {
    mant_bits: 23,
    exp_bits: 8,
    bias: -127,
};

/// Go's `commonPrefixLenIgnoreCase` (`prefix` is lower case).
fn common_prefix_len_ignore_case(s: &[u8], prefix: &[u8]) -> usize {
    s.iter()
        .zip(prefix)
        .take_while(|&(c, p)| c.to_ascii_lowercase() == *p)
        .count()
}

/// Go's `special`: a prefix of `s` that spells `inf`, `infinity` (any case, optional sign) or
/// `nan` (any case, no sign), with the prefix length.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    // Go: internal/strconv/atof.go special
    let &first = s.first()?;
    let signed = matches!(first, b'+' | b'-');
    if signed || matches!(first, b'i' | b'I') {
        let body = if signed {
            s.get(1..).unwrap_or_default()
        } else {
            s
        };
        let mut n = common_prefix_len_ignore_case(body, b"infinity");
        // Anything longer than "inf" is ok, but without "infinity" only "inf" is consumed.
        if 3 < n && n < 8 {
            n = 3;
        }
        if n == 3 || n == 8 {
            let inf = if first == b'-' {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            return Some((inf, usize::from(signed) + n));
        }
    } else if matches!(first, b'n' | b'N') && common_prefix_len_ignore_case(s, b"nan") == 3 {
        return Some((f64::from_bits(GO_NAN), 3));
    }
    None
}

/// What Go's `readFloat` scanned.
struct ScannedFloat {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
    /// How many bytes of the input form the number.
    end: usize,
}

/// Go's `lower`: ASCII letters fold to lower case (other bytes change too, harmlessly).
const fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

/// Go's `readFloat`: scans a decimal or hexadecimal mantissa (with `_` separators) and an
/// optional exponent from the start of `s`.
fn read_float(s: &[u8]) -> Option<ScannedFloat> {
    // Go: internal/strconv/atof.go readFloat
    let mut i = 0;
    let mut underscores = false;
    let neg = s.first() == Some(&b'-');
    if matches!(s.first()?, b'+' | b'-') {
        i += 1;
    }
    let hex = i + 2 < s.len()
        && s.get(i) == Some(&b'0')
        && s.get(i + 1).copied().map(lower) == Some(b'x');
    // 10^19 and 16^16 fit in a uint64.
    let (base, max_mant_digits, exp_char): (u64, i64, u8) =
        if hex { (16, 16, b'p') } else { (10, 19, b'e') };
    if hex {
        i += 2;
    }
    let (mut saw_dot, mut saw_digits) = (false, false);
    let (mut nd, mut nd_mant, mut dp) = (0_i64, 0_i64, 0_i64);
    let (mut mantissa, mut trunc) = (0_u64, false);
    while let Some(&c) = s.get(i) {
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
            dp = nd;
        } else if c.is_ascii_digit() {
            saw_digits = true;
            if c == b'0' && nd == 0 {
                // Leading zeros are ignored.
                dp -= 1;
            } else {
                nd += 1;
                if nd_mant < max_mant_digits {
                    mantissa = mantissa * base + u64::from(c - b'0');
                    nd_mant += 1;
                } else if c != b'0' {
                    trunc = true;
                }
            }
        } else if base == 16 && (b'a'..=b'f').contains(&lower(c)) {
            saw_digits = true;
            nd += 1;
            if nd_mant < max_mant_digits {
                mantissa = mantissa * 16 + u64::from(lower(c) - b'a' + 10);
                nd_mant += 1;
            } else {
                trunc = true;
            }
        } else {
            break;
        }
        i += 1;
    }
    if !saw_digits {
        return None;
    }
    if !saw_dot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }
    if s.get(i).is_some_and(|&c| lower(c) == exp_char) {
        let (e, end, separated) = read_exponent(s, i + 1)?;
        dp += e;
        i = end;
        underscores |= separated;
    } else if base == 16 {
        // A hexadecimal mantissa must have an exponent.
        return None;
    }
    let exp = if mantissa == 0 { 0 } else { dp - nd_mant };
    if underscores && !underscore_ok(s.get(..i).unwrap_or_default()) {
        return None;
    }
    Some(ScannedFloat {
        mantissa,
        exp,
        neg,
        trunc,
        hex,
        end: i,
    })
}

/// The exponent digits of `readFloat`, starting after the `e` or `p` at `i`: the signed
/// exponent (saturating near 10000, as in Go), the end index, and whether `_` appeared.
fn read_exponent(s: &[u8], mut i: usize) -> Option<(i64, usize, bool)> {
    // Go: internal/strconv/atof.go readFloat (optional exponent)
    let mut esign = 1;
    match s.get(i)? {
        b'+' => i += 1,
        b'-' => {
            i += 1;
            esign = -1;
        }
        _ => {}
    }
    if !s.get(i).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut e = 0_i64;
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
    Some((e * esign, i, underscores))
}

/// Go's `underscoreOK`: underscores only separate digits (or follow a base prefix).
fn underscore_ok(s: &[u8]) -> bool {
    // Go: internal/strconv/atoi.go underscoreOK
    let s = match s.first() {
        Some(b'+' | b'-') => s.get(1..).unwrap_or_default(),
        _ => s,
    };
    let prefixed = s.len() >= 2
        && s.first() == Some(&b'0')
        && s.get(1)
            .is_some_and(|&c| matches!(lower(c), b'b' | b'o' | b'x'));
    let hex = prefixed && s.get(1).is_some_and(|&c| lower(c) == b'x');
    // `saw` is the class of the last byte: `^` the start, `0` a digit (a base prefix counts as
    // one), `_` an underscore, `!` anything else.
    let (mut saw, mut i) = if prefixed { (b'0', 2) } else { (b'^', 0) };
    while let Some(&c) = s.get(i) {
        i += 1;
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
    }
    saw != b'_'
}

/// Go's `atofHex`: rounds a scanned hexadecimal mantissa and binary exponent to the format, and
/// reports overflow (the value is then infinite).
fn atof_hex(info: &FloatInfo, scanned: &ScannedFloat) -> (f64, bool) {
    // Go: internal/strconv/atof.go atofHex
    let mant_bits = info.mant_bits;
    let max_exp = (1_i64 << info.exp_bits) + info.bias - 2;
    let min_exp = info.bias + 1;
    let mut mantissa = scanned.mantissa;
    let mut exp = scanned.exp + i64::from(mant_bits);
    // Bring the mantissa to a leading 1 bit, the mantissa bits and two rounding bits (the lowest
    // sticky).
    while mantissa != 0 && mantissa >> (mant_bits + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if scanned.trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + mant_bits + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Denormalize a too-small exponent in the hope of making it representable.
    while mantissa > 1 && exp < min_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Round to nearest, ties to even, with the two bottom bits.
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + mant_bits) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> mant_bits == 0 {
        // Denormal or zero.
        exp = info.bias;
    }
    let overflow = exp > max_exp;
    if overflow {
        mantissa = 1 << mant_bits;
        exp = max_exp + 1;
    }
    let mut bits = mantissa & ((1 << mant_bits) - 1);
    let biased = u64::try_from((exp - info.bias) & ((1 << info.exp_bits) - 1)).unwrap_or(0);
    bits |= biased << mant_bits;
    if scanned.neg {
        bits |= 1 << mant_bits << info.exp_bits;
    }
    if info.exp_bits == FLOAT32_INFO.exp_bits {
        let bits32 = u32::try_from(bits).unwrap_or(0);
        return (f64::from(f32::from_bits(bits32)), overflow);
    }
    (f64::from_bits(bits), overflow)
}

/// Go's `parseFloatPrefix`: the value, how many bytes it used, and the error kind.
fn parse_float_prefix(s: &[u8], bits32: bool) -> (f64, usize, Option<NumErrorKind>) {
    // Go: internal/strconv/atof.go atof64 / atof32
    if let Some((v, n)) = special(s) {
        return (v, n, None);
    }
    let Some(scanned) = read_float(s) else {
        return (0.0, 0, Some(NumErrorKind::Syntax));
    };
    let end = scanned.end;
    if scanned.hex {
        let info = if bits32 { &FLOAT32_INFO } else { &FLOAT64_INFO };
        let (v, overflow) = atof_hex(info, &scanned);
        return (v, end, overflow.then_some(NumErrorKind::Range));
    }
    // Go's decimal conversion is correctly rounded, as Rust's is; underscores only separate
    // digits.
    let digits: Vec<u8> = s
        .get(..end)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&c| c != b'_')
        .collect();
    let text = from_utf8(&digits).unwrap_or_default();
    let parsed = if bits32 {
        text.parse::<f32>().ok().map(f64::from)
    } else {
        text.parse::<f64>().ok()
    };
    parsed.map_or((0.0, end, Some(NumErrorKind::Syntax)), |v| {
        (v, end, v.is_infinite().then_some(NumErrorKind::Range))
    })
}

/// Go's `strconv.ParseFloat(s, 64)` (or 32) over any input: the value Go returns together with
/// the error kind, if any (`±Inf` with `Range` on overflow, `0` with `Syntax`). Special values,
/// `_` digit separators and hexadecimal mantissas (`0x1.8p1`) are accepted, as in Go.
pub(crate) fn parse_float_go(s: &[u8], bits32: bool) -> (f64, Option<NumErrorKind>) {
    // Go: internal/strconv/atof.go ParseFloat
    let (v, n, err) = parse_float_prefix(s, bits32);
    if n != s.len() {
        return (0.0, Some(NumErrorKind::Syntax));
    }
    (v, err)
}

/// Go's `strconv.ParseInt(s, 10, 64)`, failing with the error kind Go reports. As in Go, an
/// overflow is reported even when invalid bytes follow (`99999999999999999999x`).
pub(crate) fn parse_int_go(s: &[u8]) -> Result<i64, NumErrorKind> {
    // Go: internal/strconv/atoi.go ParseInt / ParseUint (base 10, bitSize 64)
    const DIGITS_CUTOFF: u64 = u64::MAX / 10 + 1;
    const SIGN_CUTOFF: u64 = 1 << 63;
    let (negative, digits) = match s.split_first() {
        None => return Err(NumErrorKind::Syntax),
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        Some(_) => (false, s),
    };
    if digits.is_empty() {
        return Err(NumErrorKind::Syntax);
    }
    let mut magnitude: u64 = 0;
    let mut overflow = false;
    for &c in digits {
        if !c.is_ascii_digit() {
            return Err(NumErrorKind::Syntax);
        }
        if magnitude >= DIGITS_CUTOFF {
            overflow = true;
            break;
        }
        if let Some(next) = (magnitude * 10).checked_add(u64::from(c - b'0')) {
            magnitude = next;
        } else {
            overflow = true;
            break;
        }
    }
    if overflow || (!negative && magnitude >= SIGN_CUTOFF) || magnitude > SIGN_CUTOFF {
        return Err(NumErrorKind::Range);
    }
    if negative {
        return Ok(0_i64.wrapping_sub_unsigned(magnitude));
    }
    i64::try_from(magnitude).ok().ok_or(NumErrorKind::Range)
}

/// Go's `strconv.ParseInt(s, 10, 64)` succeeding (only success matters to the callers).
pub(crate) fn parse_int(s: &[u8]) -> Option<i64> {
    let (negative, digits) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, s),
    };
    let magnitude = parse_uint(digits)?;
    if negative {
        if magnitude == 1_u64 << 63 {
            return Some(i64::MIN);
        }
        i64::try_from(magnitude).ok().map(|m| -m)
    } else {
        i64::try_from(magnitude).ok()
    }
}

/// Go's `strconv.ParseUint(s, 10, 64)` succeeding.
pub(crate) fn parse_uint(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(n)
}

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

fn push_hex(out: &mut Vec<u8>, value: u32, digits: u32) {
    let mut shift = digits * 4;
    while shift > 0 {
        shift -= 4;
        let nibble = usize::try_from((value >> shift) & 0xF).unwrap_or(0);
        out.push(LOWER_HEX.get(nibble).copied().unwrap_or(b'0'));
    }
}

/// Go's `strconv.Quote` over string bytes.
pub(crate) fn quote(s: &[u8]) -> Vec<u8> {
    let mut out = vec![b'"'];
    append_quoted_inner(&mut out, s, b'"');
    out.push(b'"');
    out
}

fn append_quoted_inner(out: &mut Vec<u8>, s: &[u8], quote_char: u8) {
    // Go: strconv/quote.go appendQuotedWith
    let mut rest = s;
    while let Some(&first) = rest.first() {
        let (r, width) = if first < 0x80 {
            (Some(char::from(first)), 1)
        } else {
            decode_rune(rest)
        };
        rest = rest.get(width.max(1)..).unwrap_or_default();
        let Some(r) = r else {
            out.extend_from_slice(b"\\x");
            push_hex(out, u32::from(first), 2);
            continue;
        };
        append_escaped_rune(out, r, quote_char);
    }
}

fn append_escaped_rune(out: &mut Vec<u8>, r: char, quote_char: u8) {
    // Go: strconv/quote.go appendEscapedRune
    let mut buf = [0_u8; 4];
    if u32::from(r) == u32::from(quote_char) || r == '\\' {
        out.push(b'\\');
        out.extend_from_slice(r.encode_utf8(&mut buf).as_bytes());
        return;
    }
    if is_print(r) {
        out.extend_from_slice(r.encode_utf8(&mut buf).as_bytes());
        return;
    }
    match r {
        '\u{7}' => out.extend_from_slice(b"\\a"),
        '\u{8}' => out.extend_from_slice(b"\\b"),
        '\u{C}' => out.extend_from_slice(b"\\f"),
        '\n' => out.extend_from_slice(b"\\n"),
        '\r' => out.extend_from_slice(b"\\r"),
        '\t' => out.extend_from_slice(b"\\t"),
        '\u{B}' => out.extend_from_slice(b"\\v"),
        _ => {
            let code = u32::from(r);
            if code < 0x20 || code == 0x7F {
                out.extend_from_slice(b"\\x");
                push_hex(out, code, 2);
            } else if code < 0x10000 {
                out.extend_from_slice(b"\\u");
                push_hex(out, code, 4);
            } else {
                out.extend_from_slice(b"\\U");
                push_hex(out, code, 8);
            }
        }
    }
}

/// Go's `%q` of a single byte (a `uint8`, formatted as a quoted character literal).
pub(crate) fn quote_byte_char(c: u8) -> Vec<u8> {
    // Go: fmt %q on an integer → strconv.QuoteRune(rune(c))
    let mut out = vec![b'\''];
    append_escaped_rune(&mut out, char::from(c), b'\'');
    out.push(b'\'');
    out
}

/// `encoding/json`'s `quoteChar`: a byte rendered for scanner error messages.
pub(crate) fn json_quote_char(c: u8) -> Vec<u8> {
    // Go: encoding/json/scanner.go quoteChar
    if c == b'\'' {
        return b"'\\''".to_vec();
    }
    if c == b'"' {
        return b"'\"'".to_vec();
    }
    let mut buf = [0_u8; 4];
    let quoted = quote(char::from(c).encode_utf8(&mut buf).as_bytes());
    let inner = quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or_default();
    let mut out = vec![b'\''];
    out.extend_from_slice(inner);
    out.push(b'\'');
    out
}

/// The error `math/big.ParseFloat(s, 10, prec, ToZero)` reports, if any, for a string made of
/// the characters `+-.eE0-9` (the only input the jsoniter skip fallback can pass it).
pub(crate) fn big_parse_float_error(s: &[u8]) -> Option<Vec<u8>> {
    // Go: math/big/floatconv.go Parse/scan, natconv.go scan, ratconv.go scanExponent
    let mut i = 0;
    if s.is_empty() {
        return Some(b"EOF".to_vec());
    }
    if matches!(s.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    // Mantissa: digits with at most one '.'.
    let mut count = 0;
    let mut frac_ok = true;
    while let Some(&c) = s.get(i) {
        if c == b'.' && frac_ok {
            frac_ok = false;
        } else if c.is_ascii_digit() {
            count += 1;
        } else {
            break;
        }
        i += 1;
    }
    if count == 0 {
        return Some(b"number has no digits".to_vec());
    }
    // Exponent.
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        let mut digits = Vec::new();
        if let Some(&sign) = s.get(i)
            && (sign == b'+' || sign == b'-')
        {
            if sign == b'-' {
                digits.push(b'-');
            }
            i += 1;
        }
        let mut has_digits = false;
        while let Some(&c) = s.get(i) {
            if !c.is_ascii_digit() {
                break;
            }
            digits.push(c);
            has_digits = true;
            i += 1;
        }
        if !has_digits {
            return Some(b"number has no digits".to_vec());
        }
        if parse_int(&digits).is_none() {
            let mut msg = b"strconv.ParseInt: parsing ".to_vec();
            msg.extend_from_slice(&quote(&digits));
            msg.extend_from_slice(b": value out of range");
            return Some(msg);
        }
    }
    if let Some(&c) = s.get(i) {
        let mut msg = b"expected end of string, found ".to_vec();
        msg.extend_from_slice(&quote_byte_char(c));
        return Some(msg);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{FloatFmt, append_float, json_quote_char, parse_float, quote};

    /// `Ok` when the values are equal, otherwise an error naming the case.
    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let (got, want) = ($got, $want);
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    fn fmt(value: f64, f: FloatFmt, bits32: bool) -> String {
        let mut out = Vec::new();
        append_float(&mut out, value, f, bits32);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn formats_like_go() -> Result<(), String> {
        // Go: strconv.FormatFloat(v, 'e' or 'f', -1, 64 or 32)
        check!(fmt(1e-7, FloatFmt::E, false), "1e-07", "small e")?;
        check!(fmt(1e21, FloatFmt::E, false), "1e+21", "large e")?;
        check!(
            fmt(1.5e300, FloatFmt::E, false),
            "1.5e+300",
            "three digit exponent",
        )?;
        check!(fmt(0.1, FloatFmt::F, false), "0.1", "f")?;
        check!(fmt(123_456.0, FloatFmt::F, false), "123456", "integer f")?;
        check!(
            fmt(1e20, FloatFmt::F, false),
            "100000000000000000000",
            "big f",
        )?;
        check!(fmt(-0.0, FloatFmt::F, false), "-0", "negative zero")?;
        check!(fmt(0.0, FloatFmt::E, false), "0e+00", "zero e")?;
        check!(fmt(0.1, FloatFmt::F, true), "0.1", "float32 shortest")?;
        // Exact halfway cases: Go's Ryu keeps the even digit where Rust's formatter rounds up.
        check!(
            fmt(3_429_852.25, FloatFmt::F, true),
            "3429852.2",
            "float32 tie",
        )?;
        check!(
            // 2^50 + 0.25, which needs more digits than its shortest form has.
            fmt(f64::from_bits(0x4310_0000_0000_0001), FloatFmt::F, false),
            "1125899906842624.2",
            "float64 tie",
        )
    }

    #[test]
    fn parses_like_go() -> Result<(), String> {
        // Go: strconv.ParseFloat
        check!(parse_float(b"1.5", false).ok(), Some(1.5), "plain")?;
        check!(parse_float(b"1e", false).is_err(), true, "bad exponent")?;
        check!(parse_float(b"1e999", false).is_err(), true, "range")?;
        check!(
            parse_float(b"1e-999", false).ok(),
            Some(0.0),
            "underflow is not an error",
        )
    }

    #[test]
    fn quotes_like_go() -> Result<(), String> {
        // Go: strconv.Quote and encoding/json quoteChar
        check!(
            quote(b"a\"\x00\xff"),
            b"\"a\\\"\\x00\\xff\"".to_vec(),
            "quote",
        )?;
        check!(
            json_quote_char(0xFF),
            "'\u{ff}'".as_bytes().to_vec(),
            "latin1"
        )?;
        check!(json_quote_char(0x80), b"'\\u0080'".to_vec(), "c1 control")?;
        check!(json_quote_char(b'\\'), b"'\\\\'".to_vec(), "backslash")
    }
}
