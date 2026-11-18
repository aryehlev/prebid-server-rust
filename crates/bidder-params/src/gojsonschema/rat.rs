//! The subset of Go's `math/big` that gojsonschema uses on JSON numbers.
//!
//! gojsonschema parses every `json.Number` with `new(big.Rat).SetString` (exact rational
//! arithmetic), compares with `Rat.Cmp`, checks integers with `Rat.IsInt`, tests `multipleOf`
//! with `Rat.Quo(..).IsInt()`, and prints limits with `%v` of `new(big.Float).SetRat(..)`.
//! JSON number literals are decimal, so a value is kept as `digits * 10^exp`.

use std::cmp::Ordering;

/// Go: math/big/ratconv.go rejects decimal exponents beyond this ("avoid excessively large
/// exponents").
const MAX_EXP5: i128 = 1_000_000;

/// An exact decimal rational: `(-1)^neg * digits * 10^exp`.
///
/// `digits` holds ASCII digits without leading or trailing zeros; zero has no digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Rat {
    neg: bool,
    digits: Vec<u8>,
    exp: i64,
}

impl Rat {
    /// Go: `new(big.Rat).SetString(s)` for a JSON number literal. `None` where Go reports
    /// failure (an exponent that does not fit `int64`, or a decimal exponent beyond 1e6).
    pub(super) fn parse(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        let (neg, rest) = match bytes.split_first() {
            Some((b'-', rest)) => (true, rest),
            Some((b'+', rest)) => (false, rest),
            Some(_) => (false, bytes),
            None => return None,
        };
        let mantissa_end = rest
            .iter()
            .position(|&c| c == b'e' || c == b'E')
            .unwrap_or(rest.len());
        let (mantissa, exponent) = rest.split_at(mantissa_end);
        let mut all_digits = Vec::with_capacity(mantissa.len());
        let mut frac_digits: i128 = 0;
        let mut seen_dot = false;
        for &c in mantissa {
            match c {
                b'0'..=b'9' => {
                    all_digits.push(c);
                    if seen_dot {
                        frac_digits += 1;
                    }
                }
                b'.' if !seen_dot => seen_dot = true,
                _ => return None,
            }
        }
        if all_digits.is_empty() {
            return None;
        }
        let exp = parse_exponent(exponent)?;
        let first = all_digits.iter().position(|&c| c != b'0');
        let Some(first) = first else {
            // Go special-cases a zero mantissa before applying the exponent.
            return Some(Self::zero());
        };
        let exp5 = i128::from(exp) - frac_digits;
        if exp5.abs() > MAX_EXP5 {
            return None;
        }
        let mut digits = all_digits.get(first..).unwrap_or_default().to_vec();
        let mut exp5 = exp5;
        while digits.last() == Some(&b'0') {
            let _zero = digits.pop();
            exp5 += 1;
        }
        Some(Self {
            neg,
            digits,
            exp: i64::try_from(exp5).ok()?,
        })
    }

    const fn zero() -> Self {
        Self {
            neg: false,
            digits: Vec::new(),
            exp: 0,
        }
    }

    /// Go: `Rat.Sign() == 0`.
    pub(super) const fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// Go: `Rat.Sign() < 0`.
    pub(super) const fn is_negative(&self) -> bool {
        self.neg && !self.digits.is_empty()
    }

    /// Go: `Rat.IsInt()`.
    pub(super) const fn is_int(&self) -> bool {
        self.digits.is_empty() || self.exp >= 0
    }

    /// Go: `x.Cmp(y)`.
    pub(super) fn cmp_rat(&self, other: &Self) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (true, true) => return cmp_abs(other, self),
            (false, false) => {}
        }
        cmp_abs(self, other)
    }

    /// Go: `new(big.Rat).Quo(x, m).IsInt()` (`m` is positive: gojsonschema rejects `multipleOf`
    /// values that are not).
    pub(super) fn quo_is_int(&self, m: &Self) -> bool {
        if self.is_zero() {
            return true;
        }
        if m.is_zero() {
            return false;
        }
        // x = a * 10^p, m = b * 10^q with a and b free of trailing zeros.
        let shift = i128::from(self.exp) - i128::from(m.exp);
        if shift < 0 {
            // a / (b * 10^k) needs 10^k | a, impossible without trailing zeros.
            return false;
        }
        remainder_is_zero(&self.digits, shift, &m.digits)
    }

    /// Go: `fmt.Sprint(new(big.Float).SetRat(r))`, i.e. `%v` = shortest `%g` with `eprec = 6`.
    ///
    /// `SetRat` keeps at least 64 bits of precision, so any value with at most 18 significant
    /// digits prints as its own digits. Longer values print their exact digits (Go may round
    /// them to fewer; no bidder schema uses such limits).
    pub(super) fn go_float_string(&self) -> String {
        if self.digits.is_empty() {
            return "0".to_owned();
        }
        let mut out = String::new();
        if self.neg {
            out.push('-');
        }
        let n = i64::try_from(self.digits.len()).unwrap_or(i64::MAX);
        // Decimal exponent of the first digit (Go: d.exp - 1).
        let exp = self.exp.saturating_add(n - 1);
        let digits = String::from_utf8_lossy(&self.digits).into_owned();
        if !(-4..6).contains(&exp) {
            let (head, tail) = digits.split_at(1);
            out.push_str(head);
            if !tail.is_empty() {
                out.push('.');
                out.push_str(tail);
            }
            out.push('e');
            out.push(if exp < 0 { '-' } else { '+' });
            let abs = exp.unsigned_abs();
            if abs < 10 {
                out.push('0');
            }
            out.push_str(&abs.to_string());
            return out;
        }
        out.push_str(&fixed_notation(&digits, self.exp));
        out
    }
}

/// Parses `e[+-]ddd` (or nothing) into an `int64` exponent like Go's `scanExponent`.
fn parse_exponent(s: &[u8]) -> Option<i64> {
    let Some((_, rest)) = s.split_first() else {
        return Some(0);
    };
    let text = str::from_utf8(rest).ok()?;
    let unsigned = text.strip_prefix('+').unwrap_or(text);
    let body = unsigned.strip_prefix('-').unwrap_or(unsigned);
    if body.is_empty() || !body.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    unsigned.parse::<i64>().ok()
}

/// Compares `|x|` and `|y|`.
fn cmp_abs(x: &Rat, y: &Rat) -> Ordering {
    match (x.digits.is_empty(), y.digits.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (false, false) => {}
    }
    let x_mag = i128::from(x.exp) + i128::try_from(x.digits.len()).unwrap_or(i128::MAX);
    let y_mag = i128::from(y.exp) + i128::try_from(y.digits.len()).unwrap_or(i128::MAX);
    match x_mag.cmp(&y_mag) {
        Ordering::Equal => {}
        order @ (Ordering::Less | Ordering::Greater) => return order,
    }
    // Same magnitude: compare digit strings, padding the shorter one with zeros.
    let len = x.digits.len().max(y.digits.len());
    for i in 0..len {
        let a = x.digits.get(i).copied().unwrap_or(b'0');
        let b = y.digits.get(i).copied().unwrap_or(b'0');
        match a.cmp(&b) {
            Ordering::Equal => {}
            order @ (Ordering::Less | Ordering::Greater) => return order,
        }
    }
    Ordering::Equal
}

/// Whether `(a * 10^shift) mod b == 0` for decimal digit strings `a` and `b`.
fn remainder_is_zero(a: &[u8], shift: i128, b: &[u8]) -> bool {
    let divisor: Vec<u8> = b.iter().map(|&c| c - b'0').collect();
    let mut rem: Vec<u8> = Vec::new();
    let feed = |digit: u8, acc: &mut Vec<u8>| {
        acc.push(digit);
        trim_leading_zeros(acc);
        while cmp_digits(acc, &divisor) != Ordering::Less {
            sub_digits(acc, &divisor);
        }
    };
    for &c in a {
        feed(c - b'0', &mut rem);
    }
    let mut remaining = shift;
    while remaining > 0 && !rem.is_empty() {
        feed(0, &mut rem);
        remaining -= 1;
    }
    rem.is_empty()
}

fn trim_leading_zeros(v: &mut Vec<u8>) {
    let first = v.iter().position(|&d| d != 0).unwrap_or(v.len());
    drop(v.drain(..first));
}

fn cmp_digits(a: &[u8], b: &[u8]) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// `a -= b` for digit vectors with `a >= b`.
fn sub_digits(a: &mut Vec<u8>, b: &[u8]) {
    let mut borrow = 0u8;
    let offset = a.len() - b.len();
    for i in (0..a.len()).rev() {
        let sub = i
            .checked_sub(offset)
            .and_then(|j| b.get(j))
            .copied()
            .unwrap_or(0)
            + borrow;
        if let Some(d) = a.get_mut(i) {
            if *d >= sub {
                *d -= sub;
                borrow = 0;
            } else {
                *d = *d + 10 - sub;
                borrow = 1;
            }
        }
    }
    trim_leading_zeros(a);
}

/// Go: math/big/ftoa.go `fmtF` for the shortest digits `digits * 10^exp`.
fn fixed_notation(digits: &str, exp: i64) -> String {
    let n = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    if exp >= 0 {
        let zeros = usize::try_from(exp).unwrap_or(0);
        return format!("{digits}{}", "0".repeat(zeros));
    }
    let point = n + exp; // digits before the decimal point
    if point > 0 {
        let split = usize::try_from(point).unwrap_or(0);
        let (int_part, frac_part) = digits.split_at(split.min(digits.len()));
        return format!("{int_part}.{frac_part}");
    }
    let zeros = usize::try_from(-point).unwrap_or(0);
    format!("0.{}{digits}", "0".repeat(zeros))
}

#[cfg(test)]
mod tests {
    use super::Rat;

    fn rat(s: &str) -> Result<Rat, String> {
        Rat::parse(s).ok_or_else(|| format!("{s} should parse"))
    }

    #[test]
    fn float_strings_match_go() -> Result<(), String> {
        // Go: fmt.Sprint(new(big.Float).SetRat(r)), checked with a throwaway Go program.
        let cases = [
            ("0", "0"),
            ("1", "1"),
            ("17", "17"),
            ("1000000", "1e+06"),
            ("123456", "123456"),
            ("1234567", "1.234567e+06"),
            ("0.1", "0.1"),
            ("0.0001", "0.0001"),
            ("0.00001", "1e-05"),
            ("1.5", "1.5"),
            ("-2.5", "-2.5"),
            ("1e100", "1e+100"),
            (
                "123456789012345678901234567890",
                "1.2345678901234567890123456789e+29",
            ),
            ("0.3333333333333333333333", "0.3333333333333333333333"),
        ];
        for (input, want) in cases {
            let got = rat(input)?.go_float_string();
            if got != want {
                return Err(format!("{input}: got {got}, want {want}"));
            }
        }
        Ok(())
    }

    #[test]
    fn parse_limits_match_go() -> Result<(), String> {
        for bad in ["1e1000001", "1e9223372036854775808", "-", "", "1.2.3"] {
            if Rat::parse(bad).is_some() {
                return Err(format!("{bad} should be rejected"));
            }
        }
        for good in ["0e9999999", "1e1000000", "1.5e-999999", "-0"] {
            let _ok = rat(good)?;
        }
        Ok(())
    }

    #[test]
    fn multiple_of() -> Result<(), String> {
        let cases = [
            ("10", "5", true),
            ("10", "3", false),
            ("0.3", "0.1", true),
            ("0.35", "0.1", false),
            ("1e30", "7", false),
            ("7e30", "7", true),
            ("0", "0.25", true),
            ("1.25", "0.25", true),
            ("2", "0.4", true),
        ];
        for (x, m, want) in cases {
            if rat(x)?.quo_is_int(&rat(m)?) != want {
                return Err(format!("{x} multipleOf {m}: want {want}"));
            }
        }
        Ok(())
    }
}
