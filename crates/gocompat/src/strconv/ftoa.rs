//! Go: `internal/strconv/ftoa.go` (`FormatFloat`).
//!
//! Go computes decimal digits with its own algorithms (`dboxFtoa` for the shortest digits,
//! `fixedFtoa`/`bigFtoa` for a fixed count, rounding ties to even). Rust's float formatting
//! produces the same digits (shortest round-trip digits closest to the exact value, and exact
//! digits rounded half to even), so this module asks Rust for the digits and then lays them out
//! with ports of Go's `formatDigits`, `fmtE`, `fmtF`, `fmtB` and `fmtX`.
//!
//! One difference is corrected: when the value lies exactly halfway between two shortest digit
//! strings, Go picks the even last digit and Rust the upper one (`ties_to_even`). The random
//! 64- and 32-bit sweeps in the golden tests hit this for about one value in 4000.

use crate::math::float32;

/// Go: `decimalSlice`: digits `d` (ASCII, no trailing zeros) with the decimal point `dp`
/// positions from the left. Zero has no digits and `dp == 0`.
struct Decimal {
    d: Vec<u8>,
    dp: i64,
}

impl Decimal {
    const fn zero() -> Self {
        Self {
            d: Vec::new(),
            dp: 0,
        }
    }

    fn nd(&self) -> i64 {
        i64::try_from(self.d.len()).unwrap_or(i64::MAX)
    }

    /// Parses Rust's `{:e}` output (`"1.25e-7"`, `"3e2"`) of a non-negative finite value.
    fn from_rust_exp(s: &str) -> Self {
        let Some((mantissa, exp)) = s.split_once('e') else {
            return Self::zero();
        };
        let exp: i64 = exp.parse().unwrap_or(0);
        let mut d: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
        while d.last() == Some(&b'0') {
            let _zero = d.pop();
        }
        if d.is_empty() {
            return Self::zero();
        }
        Self { d, dp: exp + 1 }
    }

    /// Parses Rust's `{:.N}` output (`"12.3400"`, `"0.0012"`) of a non-negative finite value.
    fn from_rust_fixed(s: &str) -> Self {
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let mut dp = i64::try_from(int.len()).unwrap_or(0);
        let all: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
        let leading = all.iter().take_while(|&&c| c == b'0').count();
        if leading == all.len() {
            return Self::zero();
        }
        let mut d: Vec<u8> = all.into_iter().skip(leading).collect();
        dp -= i64::try_from(leading).unwrap_or(0);
        while d.last() == Some(&b'0') {
            let _zero = d.pop();
        }
        Self { d, dp }
    }

    fn digit(&self, i: i64) -> u8 {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.d.get(i))
            .copied()
            .unwrap_or(b'0')
    }
}

/// The IEEE fields Go's `genericFtoa` works with.
struct Bits {
    neg: bool,
    /// Biased exponent field.
    exp_field: u64,
    mant_field: u64,
    expbits: u32,
    mantbits: u32,
    bias: i64,
}

impl Bits {
    fn new(val: f64, bit_size: i64) -> Option<Self> {
        match bit_size {
            32 => {
                let b = u64::from(float32(val).to_bits());
                Some(Self {
                    neg: b >> 31 != 0,
                    exp_field: (b >> 23) & 0xFF,
                    mant_field: b & ((1 << 23) - 1),
                    expbits: 8,
                    mantbits: 23,
                    bias: -127,
                })
            }
            64 => {
                let b = val.to_bits();
                Some(Self {
                    neg: b >> 63 != 0,
                    exp_field: (b >> 52) & 0x7FF,
                    mant_field: b & ((1 << 52) - 1),
                    expbits: 11,
                    mantbits: 52,
                    bias: -1023,
                })
            }
            _ => None,
        }
    }
}

/// Go: `strconv.FormatFloat(f, fmt, prec, bitSize)`.
///
/// `fmt` is one of `b e E f g G x X`; any other byte yields `%` followed by it, as in Go.
/// `prec < 0` selects the shortest digits that round-trip at `bit_size`. Go panics for a bit
/// size other than 32 or 64; this returns an empty string instead.
#[must_use]
pub fn format_float(f: f64, fmt: u8, prec: i64, bit_size: i64) -> String {
    let Some(bits) = Bits::new(f, bit_size) else {
        return String::new();
    };
    let mut out = Vec::with_capacity(24);
    generic_ftoa(&mut out, f, fmt, prec, &bits, bit_size);
    String::from_utf8(out).unwrap_or_default()
}

/// More digits than the exact decimal expansion of any `f64` has (at most 767 significant and
/// 1074 fractional digits). Formatting with a larger precision only appends zeros, which
/// [`Decimal`] trims and `format_digits` restores; Rust's formatter also rejects precisions above
/// `u16::MAX`, which Go accepts.
const MAX_EXACT_DIGITS: usize = 1100;

fn generic_ftoa(dst: &mut Vec<u8>, val: f64, fmt: u8, prec: i64, b: &Bits, bit_size: i64) {
    let neg = b.neg;
    let mut exp = i64::try_from(b.exp_field).unwrap_or(0);
    let mut mant = b.mant_field;
    if b.exp_field == (1 << b.expbits) - 1 {
        let s: &[u8] = if mant != 0 {
            b"NaN"
        } else if neg {
            b"-Inf"
        } else {
            b"+Inf"
        };
        dst.extend_from_slice(s);
        return;
    }
    if exp == 0 {
        exp += 1;
    } else {
        mant |= 1 << b.mantbits;
    }
    exp += b.bias;
    if fmt == b'b' {
        fmt_b(dst, neg, mant, exp, b.mantbits);
        return;
    }
    if fmt == b'x' || fmt == b'X' {
        fmt_x(dst, prec, fmt, neg, mant, exp, b.mantbits);
        return;
    }
    if !matches!(fmt, b'e' | b'E' | b'f' | b'g' | b'G') {
        // Go: formatDigits falls through to `%` + fmt for an unknown format.
        dst.push(b'%');
        dst.push(fmt);
        return;
    }
    let shortest = prec < 0;
    if mant == 0 {
        format_digits(dst, shortest, neg, &Decimal::zero(), prec, fmt);
        return;
    }
    let abs = val.abs();
    let abs32 = float32(val).abs();
    if shortest {
        let digs = if bit_size == 32 {
            Decimal::from_rust_exp(&format!("{abs32:e}"))
        } else {
            Decimal::from_rust_exp(&format!("{abs:e}"))
        };
        let target = if bit_size == 32 {
            f64::from(abs32)
        } else {
            abs
        };
        let digs = ties_to_even(digs, mant, exp - i64::from(b.mantbits), target, bit_size);
        let shortest_prec = match fmt {
            b'e' | b'E' => (digs.nd() - 1).max(0),
            b'f' => (digs.nd() - digs.dp).max(0),
            _ => digs.nd(),
        };
        format_digits(dst, true, neg, &digs, shortest_prec, fmt);
        return;
    }
    let mut fixed_prec = prec;
    let digs = if fmt == b'f' {
        let p = usize::try_from(prec).unwrap_or(0).min(MAX_EXACT_DIGITS);
        if bit_size == 32 {
            Decimal::from_rust_fixed(&format!("{abs32:.p$}"))
        } else {
            Decimal::from_rust_fixed(&format!("{abs:.p$}"))
        }
    } else {
        // 'e' keeps prec+1 significant digits, 'g' keeps prec (at least one).
        let significant = if matches!(fmt, b'e' | b'E') {
            prec.saturating_add(1)
        } else {
            fixed_prec = prec.max(1);
            fixed_prec
        };
        let p = usize::try_from(significant - 1)
            .unwrap_or(0)
            .min(MAX_EXACT_DIGITS);
        if bit_size == 32 {
            Decimal::from_rust_exp(&format!("{abs32:.p$e}"))
        } else {
            Decimal::from_rust_exp(&format!("{abs:.p$e}"))
        }
    };
    format_digits(dst, false, neg, &digs, fixed_prec, fmt);
}

/// The odd part and power of two of `x` (`x == odd << twos`); `x` must be non-zero.
const fn split_twos(x: u128) -> (u128, i64) {
    let twos = x.trailing_zeros();
    (x >> twos, twos as i64)
}

/// Whether `m * 10^k` equals `mant * 2^exp2` exactly.
fn decimal_equals_binary(m: u64, k: i64, mant: u64, exp2: i64) -> bool {
    if m == 0 || mant == 0 {
        return false;
    }
    let (v_odd, v_twos) = split_twos(u128::from(mant));
    let v_twos = v_twos + exp2;
    let j = u32::try_from(k.unsigned_abs()).unwrap_or(u32::MAX);
    // An odd part of 5^j above 2^60 cannot match: both odd parts are below 2^60.
    if j > 26 {
        return false;
    }
    let five = 5_u128.pow(j);
    if k >= 0 {
        // m * 5^k * 2^k == mant * 2^exp2
        let (t_odd, t_twos) = split_twos(u128::from(m) * five);
        t_odd == v_odd && t_twos + k == v_twos
    } else {
        // mant * 5^j * 2^(exp2 + j) == m
        let (a_odd, a_twos) = split_twos(u128::from(mant) * five);
        let (m_odd, m_twos) = split_twos(u128::from(m));
        a_odd == m_odd && a_twos + exp2 + k.unsigned_abs().cast_signed() == m_twos
    }
}

/// Go's shortest formatting (`dboxFtoa`) breaks a tie between two equally close shortest digit
/// strings towards the even last digit; Rust's breaks it upwards. When the value lies exactly
/// halfway between `d` and its neighbour and `d` is odd, switch to the neighbour if it also
/// round-trips.
fn ties_to_even(digs: Decimal, mant: u64, exp2: i64, target: f64, bit_size: i64) -> Decimal {
    let Ok(d) = str::from_utf8(&digs.d).unwrap_or("x").parse::<u64>() else {
        return digs;
    };
    if d % 2 == 0 {
        return digs;
    }
    // digs is d * 10^(dp - nd); the halfway points are (10d -+ 5) * 10^(dp - nd - 1).
    let k = digs.dp - digs.nd() - 1;
    for (half, neighbour) in [(d * 10 - 5, d - 1), (d * 10 + 5, d + 1)] {
        if !decimal_equals_binary(half, k, mant, exp2) {
            continue;
        }
        let text = format!("{neighbour}e{}", k + 1);
        let round_trips = if bit_size == 32 {
            text.parse::<f32>()
                .is_ok_and(|x| f64::from(x).to_bits() == target.to_bits())
        } else {
            text.parse::<f64>()
                .is_ok_and(|x| x.to_bits() == target.to_bits())
        };
        if round_trips {
            return Decimal::from_rust_exp(&text_exp(neighbour, k + 1));
        }
    }
    digs
}

/// `neighbour * 10^exp` in Rust `{:e}` form (`"d.ddde-7"`), for [`Decimal::from_rust_exp`].
fn text_exp(neighbour: u64, exp: i64) -> String {
    let digits = neighbour.to_string();
    let extra = i64::try_from(digits.len()).unwrap_or(1) - 1;
    let (lead, rest) = digits.split_at(1);
    format!("{lead}.{rest}e{}", exp + extra)
}

/// Go: `formatDigits`.
fn format_digits(dst: &mut Vec<u8>, shortest: bool, neg: bool, digs: &Decimal, prec: i64, fmt: u8) {
    match fmt {
        b'e' | b'E' => fmt_e(dst, neg, digs, prec, fmt),
        b'f' => fmt_f(dst, neg, digs, prec),
        _ => {
            let mut prec = prec;
            let mut eprec = prec;
            if eprec > digs.nd() && digs.nd() >= digs.dp {
                eprec = digs.nd();
            }
            // %e is used if the exponent from the conversion is less than -4 or greater than or
            // equal to the precision. If precision was the shortest possible, use precision 6.
            if shortest {
                eprec = 6;
            }
            let exp = digs.dp - 1;
            if exp < -4 || exp >= eprec {
                if prec > digs.nd() {
                    prec = digs.nd();
                }
                fmt_e(dst, neg, digs, prec - 1, fmt + b'e' - b'g');
                return;
            }
            if prec > digs.dp {
                prec = digs.nd();
            }
            fmt_f(dst, neg, digs, (prec - digs.dp).max(0));
        }
    }
}

/// Go: `fmtE`: `-d.ddddde±dd`.
fn fmt_e(dst: &mut Vec<u8>, neg: bool, d: &Decimal, prec: i64, fmt: u8) {
    if neg {
        dst.push(b'-');
    }
    dst.push(d.d.first().copied().unwrap_or(b'0'));
    if prec > 0 {
        dst.push(b'.');
        for i in 1..=prec {
            dst.push(d.digit(i));
        }
    }
    dst.push(fmt);
    // special case: 0 has exponent 0
    let exp = if d.d.is_empty() { 0 } else { d.dp - 1 };
    dst.push(if exp < 0 { b'-' } else { b'+' });
    let exp = exp.unsigned_abs();
    if exp < 10 {
        dst.push(b'0');
    }
    dst.extend_from_slice(exp.to_string().as_bytes());
}

/// Go: `fmtF`: `-ddddd.dddd`.
fn fmt_f(dst: &mut Vec<u8>, neg: bool, d: &Decimal, prec: i64) {
    if neg {
        dst.push(b'-');
    }
    if d.dp > 0 {
        for i in 0..d.dp {
            dst.push(d.digit(i));
        }
    } else {
        dst.push(b'0');
    }
    if prec > 0 {
        dst.push(b'.');
        for i in 0..prec {
            dst.push(d.digit(d.dp + i));
        }
    }
}

/// Go: `fmtB`: `-ddddp±ddd`.
fn fmt_b(dst: &mut Vec<u8>, neg: bool, mant: u64, exp: i64, mantbits: u32) {
    if neg {
        dst.push(b'-');
    }
    dst.extend_from_slice(mant.to_string().as_bytes());
    dst.push(b'p');
    let exp = exp - i64::from(mantbits);
    if exp >= 0 {
        dst.push(b'+');
    }
    dst.extend_from_slice(exp.to_string().as_bytes());
}

/// Go: `fmtX`: `-0x1.yyyyyyyyp±ddd`.
fn fmt_x(dst: &mut Vec<u8>, prec: i64, fmt: u8, neg: bool, mant: u64, exp: i64, mantbits: u32) {
    let mut exp = if mant == 0 { 0 } else { exp };
    // Shift digits so leading 1 (if any) is at bit 1<<60.
    let mut mant = mant << (60 - mantbits);
    while mant != 0 && mant & (1 << 60) == 0 {
        mant <<= 1;
        exp -= 1;
    }
    // Round if requested.
    if (0..15).contains(&prec) {
        let shift = u32::try_from(prec * 4).unwrap_or(0);
        let extra = (mant << shift) & ((1 << 60) - 1);
        mant >>= 60 - shift;
        if extra | (mant & 1) > 1 << 59 {
            mant += 1;
        }
        mant <<= 60 - shift;
        if mant & (1 << 61) != 0 {
            // Wrapped around.
            mant >>= 1;
            exp += 1;
        }
    }
    let hex: &[u8; 16] = if fmt == b'X' {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let nibble = |m: u64| {
        hex.get(usize::try_from((m >> 60) & 15).unwrap_or(0))
            .copied()
    };
    if neg {
        dst.push(b'-');
    }
    dst.extend_from_slice(&[b'0', fmt, b'0' + u8::from((mant >> 60) & 1 == 1)]);
    mant <<= 4; // remove leading 0 or 1
    if prec < 0 && mant != 0 {
        dst.push(b'.');
        while mant != 0 {
            dst.extend(nibble(mant));
            mant <<= 4;
        }
    } else if prec > 0 {
        dst.push(b'.');
        for _ in 0..prec {
            dst.extend(nibble(mant));
            mant <<= 4;
        }
    }
    dst.push(if fmt == b'x' { b'p' } else { b'P' });
    dst.push(if exp < 0 { b'-' } else { b'+' });
    let exp = exp.unsigned_abs();
    if exp < 10 {
        dst.push(b'0');
    }
    dst.extend_from_slice(exp.to_string().as_bytes());
}
