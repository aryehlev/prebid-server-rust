//! Glue (Rule B (c)): the Go conversions that decide what an operator's configuration value
//! means once it reaches a field or a key. `strconv`'s `ParseBool`, `ParseInt` and `ParseUint`
//! with base 0 and `ParseFloat` are what mapstructure's weak decode and cast parse strings with
//! (ledger: `config.weak-typing`, `config.env-bool-spellings`); Go's `int64(f)` and `float32(f)`
//! conversions are what they store numbers with; `strings.ToLower`, `strings.ToUpper` and
//! `strings.EqualFold` are how Viper and mapstructure match keys (ledger: `config.key-case`,
//! `config.env-names`, `config.key-unicode`).
//!
//! std parses neither Go's base prefixes and `_` separators nor its `Inf` spellings and
//! hexadecimal floats, and its float-to-integer casts saturate where amd64 gives
//! `math.MinInt64`, so the syntax and those edges are Go's here while the numbers come from std:
//! a decimal float is rebuilt from the digits and the exponent Go reads and parsed by std's
//! correctly rounded parser, as Go's parser rounds correctly. Errors keep `strconv.NumError`'s
//! words with the input quoted as Rust quotes strings (ledger: `config.error-texts`).

use std::fmt;

/// Why a parse failed (Go: `NumError.Err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumErrorKind {
    /// Go: `strconv.ErrSyntax`.
    Syntax,
    /// Go: `strconv.ErrRange`.
    Range,
}

/// Go: `*strconv.NumError`: the function, the input and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NumError {
    func: &'static str,
    num: String,
    kind: NumErrorKind,
}

impl NumError {
    fn new(func: &'static str, num: &str, kind: NumErrorKind) -> Self {
        Self {
            func,
            num: num.to_owned(),
            kind,
        }
    }
}

impl fmt::Display for NumError {
    /// Go: `"strconv." + e.Func + ": parsing " + Quote(e.Num) + ": " + e.Err.Error()`, with
    /// Rust's quoting in place of `strconv.Quote`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            NumErrorKind::Syntax => "invalid syntax",
            NumErrorKind::Range => "value out of range",
        };
        write!(f, "strconv.{}: parsing {:?}: {reason}", self.func, self.num)
    }
}

/// Go: `strconv.ParseBool`.
///
/// # Errors
///
/// A syntax error for any other text than Go's twelve spellings.
pub(super) fn parse_bool(s: &str) -> Result<bool, NumError> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(NumError::new("ParseBool", s, NumErrorKind::Syntax)),
    }
}

/// Go: `strconv`'s `lower`: the byte with the ASCII lower-case bit set.
const fn lower(c: u8) -> u8 {
    c | 0x20
}

/// Go: `strconv`'s `underscoreOK`: underscores may only separate digits, or follow a base
/// prefix.
fn underscore_ok(s: &[u8]) -> bool {
    let s = match s.first() {
        Some(b'+' | b'-') => s.get(1..).unwrap_or_default(),
        _ => s,
    };
    // saw is '^' (start), '0' (a digit or the base prefix), '_' or '!' (anything else).
    let (mut saw, hex, rest) = match s {
        [b'0', prefix, tail @ ..] if matches!(lower(*prefix), b'b' | b'o' | b'x') => {
            (b'0', lower(*prefix) == b'x', tail)
        }
        _ => (b'^', false, s),
    };
    for &c in rest {
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else if saw == b'_' {
            return false;
        } else {
            saw = b'!';
        }
    }
    saw != b'_'
}

/// The largest value of an unsigned integer of `bits` bits (8, 16, 32 or 64).
const fn max_uint(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

/// Go: `strconv.ParseUint(s, 0, bits)`: a `0b`, `0o` or `0x` prefix or a leading `0` picks the
/// base, and `_` may separate digits.
///
/// # Errors
///
/// A syntax error for text that is not such a number, a range error for one above the largest
/// value of `bits` bits.
pub(super) fn parse_uint(s: &str, bits: u32) -> Result<u64, NumError> {
    const FUNC: &str = "ParseUint";
    let syntax = || NumError::new(FUNC, s, NumErrorKind::Syntax);
    let range = || NumError::new(FUNC, s, NumErrorKind::Range);
    let bytes = s.as_bytes();
    let (base, digits): (u64, &[u8]) = match bytes {
        [] => return Err(syntax()),
        [b'0', prefix, rest @ ..] if !rest.is_empty() && lower(*prefix) == b'b' => (2, rest),
        [b'0', prefix, rest @ ..] if !rest.is_empty() && lower(*prefix) == b'o' => (8, rest),
        [b'0', prefix, rest @ ..] if !rest.is_empty() && lower(*prefix) == b'x' => (16, rest),
        [b'0', rest @ ..] => (8, rest),
        _ => (10, bytes),
    };
    let max = max_uint(bits);
    // The smallest number whose product with the base overflows.
    let cutoff = u64::MAX / base + 1;
    let mut underscores = false;
    let mut n = 0_u64;
    for &c in digits {
        let digit = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => c - b'0',
            _ if lower(c).is_ascii_lowercase() => lower(c) - b'a' + 10,
            _ => return Err(syntax()),
        };
        if u64::from(digit) >= base {
            return Err(syntax());
        }
        if n >= cutoff {
            return Err(range());
        }
        n = match (n * base).checked_add(u64::from(digit)) {
            Some(next) if next <= max => next,
            _ => return Err(range()),
        };
    }
    if underscores && !underscore_ok(bytes) {
        return Err(syntax());
    }
    Ok(n)
}

/// Go: `strconv.ParseInt(s, 0, bits)`: an optional sign before what [`parse_uint`] reads.
///
/// # Errors
///
/// A syntax error for text that is not such a number, a range error for one outside the range of
/// `bits` bits.
pub(super) fn parse_int(s: &str, bits: u32) -> Result<i64, NumError> {
    const FUNC: &str = "ParseInt";
    let range = || NumError::new(FUNC, s, NumErrorKind::Range);
    if s.is_empty() {
        return Err(NumError::new(FUNC, s, NumErrorKind::Syntax));
    }
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'+') => (false, s.get(1..).unwrap_or_default()),
        Some(b'-') => (true, s.get(1..).unwrap_or_default()),
        _ => (false, s),
    };
    let un = match parse_uint(rest, bits) {
        Ok(un) => un,
        // Go: ParseUint's largest value, which is out of the signed range too.
        Err(e) if e.kind == NumErrorKind::Range => return Err(range()),
        Err(_) => return Err(NumError::new(FUNC, s, NumErrorKind::Syntax)),
    };
    let cutoff = 1_u64 << bits.clamp(1, 64).saturating_sub(1);
    if (!neg && un >= cutoff) || (neg && un > cutoff) {
        return Err(range());
    }
    let n = un.cast_signed();
    Ok(if neg { n.wrapping_neg() } else { n })
}

/// Go: `strconv`'s `special`: `[+-]inf`, `[+-]infinity` and `nan` in any case, with the number
/// of bytes they take.
fn special(s: &[u8]) -> Option<(f64, usize)> {
    let (negative, nsign, rest) = match s.first() {
        Some(b'+') => (false, 1, s.get(1..).unwrap_or_default()),
        Some(b'-') => (true, 1, s.get(1..).unwrap_or_default()),
        _ => (false, 0, s),
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
            // Go: anything longer than "inf" is fine, but short of "infinity" only "inf" counts.
            if 3 < n && n < 8 {
                n = 3;
            }
            let value = if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            (n == 3 || n == 8).then_some((value, nsign + n))
        }
        // Go: a sign before "nan" is not accepted.
        Some(b'n' | b'N') if nsign == 0 => (common(b"nan") == 3).then_some((f64::NAN, 3)),
        _ => None,
    }
}

/// What Go's `readFloat` read.
#[derive(Debug)]
struct Read {
    neg: bool,
    hex: bool,
    /// The bytes read.
    end: usize,
    /// Decimal: the digits from the first non-zero one, and the power of ten they are scaled by.
    digits: String,
    exp10: i64,
    /// Hexadecimal: the first 16 digits, whether a non-zero digit followed them, and the power
    /// of two the mantissa is scaled by.
    mantissa: u64,
    trunc: bool,
    exp2: i64,
}

/// The digits part of Go's `readFloat` loop.
#[derive(Debug, Default)]
struct Digits {
    underscores: bool,
    sawdot: bool,
    sawdigits: bool,
    /// Go: `nd`, `ndMant` and `dp`.
    nd: i64,
    nd_mant: i64,
    dp: i64,
    mantissa: u64,
    trunc: bool,
    /// The decimal digits from the first non-zero one.
    text: String,
}

impl Digits {
    /// Reads digits, `_` and one `.` of `s` from `i` (hexadecimal ones with `hex`), returning
    /// where they end.
    fn read(&mut self, s: &[u8], mut i: usize, hex: bool) -> usize {
        let (base, max_mant_digits) = if hex { (16, 16) } else { (10, 19) };
        while let Some(&c) = s.get(i) {
            let value = if c.is_ascii_digit() {
                c - b'0'
            } else if hex && (b'a'..=b'f').contains(&lower(c)) {
                lower(c) - b'a' + 10
            } else if c == b'_' {
                self.underscores = true;
                i += 1;
                continue;
            } else if c == b'.' && !self.sawdot {
                self.sawdot = true;
                self.dp = self.nd;
                i += 1;
                continue;
            } else {
                return i;
            };
            self.sawdigits = true;
            i += 1;
            // Go: leading zeros are skipped.
            if value == 0 && self.nd == 0 {
                self.dp -= 1;
                continue;
            }
            self.nd += 1;
            if !hex {
                self.text.push(char::from(c));
            }
            if self.nd_mant < max_mant_digits {
                self.mantissa = self.mantissa * base + u64::from(value);
                self.nd_mant += 1;
            } else if value != 0 {
                self.trunc = true;
            }
        }
        i
    }
}

/// The exponent part of Go's `readFloat` at `s[i..]`, after its `e` or `p`: an optional sign,
/// then digits and `_`. Returns the exponent, whether it held an `_`, and where it ends; `None`
/// where it has no digit.
fn read_exponent(s: &[u8], mut i: usize) -> Option<(i64, bool, usize)> {
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
            // Go: a very large exponent only needs to be large.
            if e < 10_000 {
                e = e * 10 + i64::from(c - b'0');
            }
        } else {
            break;
        }
        i += 1;
    }
    Some((e * esign, underscores, i))
}

/// Go: `strconv`'s `readFloat`: a sign, decimal digits with an optional point and `e` exponent
/// or `0x` and hexadecimal digits with a mandatory `p` exponent, and `_` between digits. `None`
/// where Go's `ok` is false.
fn read_float(s: &[u8]) -> Option<Read> {
    let (neg, mut i) = match s.first()? {
        b'+' => (false, 1),
        b'-' => (true, 1),
        _ => (false, 0),
    };
    let hex =
        i + 2 < s.len() && s.get(i) == Some(&b'0') && s.get(i + 1).map(|c| lower(*c)) == Some(b'x');
    if hex {
        i += 2;
    }
    let mut d = Digits::default();
    i = d.read(s, i, hex);
    if !d.sawdigits {
        return None;
    }
    let mut dp = if d.sawdot { d.dp } else { d.nd };
    let mut nd_mant = d.nd_mant;
    if hex {
        dp *= 4;
        nd_mant *= 4;
    }
    let mut underscores = d.underscores;
    let exp_char = if hex { b'p' } else { b'e' };
    if s.get(i).map(|c| lower(*c)) == Some(exp_char) {
        let (e, separated, end) = read_exponent(s, i + 1)?;
        dp += e;
        underscores |= separated;
        i = end;
    } else if hex {
        return None;
    }
    if underscores && !underscore_ok(s.get(..i).unwrap_or_default()) {
        return None;
    }
    Some(Read {
        neg,
        hex,
        end: i,
        exp10: if hex { 0 } else { dp - d.nd },
        digits: d.text,
        mantissa: d.mantissa,
        trunc: d.trunc,
        exp2: if d.mantissa == 0 { 0 } else { dp - nd_mant },
    })
}

/// A decimal [`Read`] as the nearest float of `bits` bits, by std's parser.
fn decimal_float(read: &Read, bits: u32) -> Option<f64> {
    let sign = if read.neg { "-" } else { "" };
    let text = if read.digits.is_empty() {
        format!("{sign}0")
    } else {
        format!("{sign}{}e{}", read.digits, read.exp10)
    };
    if bits == 32 {
        text.parse::<f32>().ok().map(f64::from)
    } else {
        text.parse::<f64>().ok()
    }
}

/// Go: `strconv`'s `atofHex`: a hexadecimal [`Read`] rounded to `bits` bits, ties to even
/// (infinite where it overflows).
fn hex_float(read: &Read, bits: u32) -> f64 {
    let (fraction_width, exponent_width, exponent_bias): (u32, u32, i64) = if bits == 32 {
        (23, 8, -127)
    } else {
        (52, 11, -1023)
    };
    let top_exp = (1_i64 << exponent_width) + exponent_bias - 2;
    let bottom_exp = exponent_bias + 1;
    let mut mantissa = read.mantissa;
    // The mantissa is now implicitly divided by 2^fraction_width.
    let mut exp = read.exp2 + i64::from(fraction_width);
    // A leading 1 and fraction_width more bits, then two rounding bits, the last of them sticky.
    while mantissa != 0 && mantissa >> (fraction_width + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if read.trunc {
        mantissa |= 1;
    }
    while mantissa >> (fraction_width + 3) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // Denormalize an exponent below the range (the -2 is for the rounding bits).
    while mantissa > 1 && exp < bottom_exp - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1;
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (fraction_width + 1) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> fraction_width == 0 {
        exp = exponent_bias;
    }
    if exp > top_exp {
        mantissa = 1 << fraction_width;
        exp = top_exp + 1;
    }
    let biased = u64::try_from((exp - exponent_bias) & ((1 << exponent_width) - 1)).unwrap_or(0);
    let mut out = (mantissa & ((1 << fraction_width) - 1)) | (biased << fraction_width);
    if read.neg {
        out |= 1 << (fraction_width + exponent_width);
    }
    if bits == 32 {
        f64::from(f32::from_bits(u32::try_from(out).unwrap_or(0)))
    } else {
        f64::from_bits(out)
    }
}

/// Go: `strconv.ParseFloat(s, bits)` for `bits` 32 or 64: the nearest float of that size.
///
/// # Errors
///
/// A syntax error for text that is not a float as Go spells one, a range error for one beyond
/// the largest float of `bits` bits.
pub(super) fn parse_float(s: &str, bits: u32) -> Result<f64, NumError> {
    const FUNC: &str = "ParseFloat";
    let syntax = || NumError::new(FUNC, s, NumErrorKind::Syntax);
    let bytes = s.as_bytes();
    if let Some((value, n)) = special(bytes) {
        return if n == bytes.len() {
            Ok(value)
        } else {
            Err(syntax())
        };
    }
    let read = read_float(bytes).ok_or_else(syntax)?;
    if read.end != bytes.len() {
        return Err(syntax());
    }
    let value = if read.hex {
        hex_float(&read, bits)
    } else {
        decimal_float(&read, bits).ok_or_else(syntax)?
    };
    if value.is_infinite() {
        Err(NumError::new(FUNC, s, NumErrorKind::Range))
    } else {
        Ok(value)
    }
}

/// Go: `strconv.FormatFloat(f, 'f', -1, 64)`: std's shortest digits, which are Go's, and Go's
/// `NaN`, `+Inf` and `-Inf`.
#[must_use]
pub(super) fn float_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_owned()
    } else if f.is_infinite() {
        if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
    } else {
        format!("{f}")
    }
}

/// Go: `int64(f)` on amd64: truncated toward zero, and `math.MinInt64` for NaN and values out
/// of the `int64` range, where Rust's casts saturate.
#[must_use]
pub(super) fn float_to_int64(f: f64) -> i64 {
    // 2^63
    const TWO_63: f64 = f64::from_bits(0x43E0_0000_0000_0000);
    if f.is_nan() || f >= TWO_63 || f < -TWO_63 {
        return i64::MIN;
    }
    // The truncated value is a whole number, which `{:.0}` spells exactly.
    format!("{:.0}", f.trunc()).parse().unwrap_or(i64::MIN)
}

/// Go: `float32(f)`, widened back: the nearest `float32`, ties to even, found by std's correctly
/// rounded parser from the exact decimal expansion of `f` (1074 fraction digits spell every
/// `float64` exactly).
#[must_use]
pub(super) fn round_to_float32(f: f64) -> f64 {
    if !f.is_finite() {
        return f;
    }
    format!("{f:.1074}").parse::<f32>().map_or(f, f64::from)
}

/// Go: `unicode.ToUpper` for the letters whose full upper-case mapping, which std gives, has
/// several characters but whose simple one, which Go uses, is a single character: the Greek
/// letters with ypogegrammeni, which upper-case to their prosgegrammeni forms.
fn simple_upper(c: char) -> Option<char> {
    let mapped = match c {
        '\u{1F80}'..='\u{1F87}' | '\u{1F90}'..='\u{1F97}' | '\u{1FA0}'..='\u{1FA7}' => {
            u32::from(c) + 8
        }
        '\u{1FB3}' => 0x1FBC,
        '\u{1FC3}' => 0x1FCC,
        '\u{1FF3}' => 0x1FFC,
        _ => return None,
    };
    char::from_u32(mapped)
}

/// Go: `strings.ToLower`: each character's simple lower-case mapping. std's full mapping is a
/// single character except for U+0130, whose simple mapping is the first of its two.
#[must_use]
pub(super) fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Go: `strings.ToUpper`: each character's simple upper-case mapping. Where std's full mapping
/// has several characters, Go keeps the character unless [`simple_upper`] names its mapping.
#[must_use]
pub(super) fn to_upper(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_uppercase();
    }
    s.chars()
        .map(|c| {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(one), None) => one,
                _ => simple_upper(c).unwrap_or(c),
            }
        })
        .collect()
}

/// Whether two characters are equal under Unicode simple case folding when `field` is ASCII:
/// ASCII letters in either case, and `ſ` (U+017F) and the Kelvin sign (U+212A), which fold to
/// `s` and `k`.
const fn fold_eq(key: char, field: char) -> bool {
    key == field
        || key.eq_ignore_ascii_case(&field)
        || matches!(
            (key, field.to_ascii_lowercase()),
            ('\u{17F}', 's') | ('\u{212A}', 'k')
        )
}

/// Go: `strings.EqualFold(key, field)` for an ASCII `field`, as mapstructure matches a key to a
/// field name (the field names of `config.Configuration` are ASCII).
#[must_use]
pub(super) fn equal_fold(key: &str, field: &str) -> bool {
    key.chars().count() == field.chars().count()
        && key.chars().zip(field.chars()).all(|(k, f)| fold_eq(k, f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_errors_keep_go_words() {
        let texts: Vec<String> = [
            parse_bool("x").err(),
            parse_int("4x2", 64).err(),
            parse_uint("300", 8).err(),
            parse_float("1e400", 64).err(),
        ]
        .into_iter()
        .flatten()
        .map(|e| e.to_string())
        .collect();
        assert_eq!(
            texts,
            [
                "strconv.ParseBool: parsing \"x\": invalid syntax",
                "strconv.ParseInt: parsing \"4x2\": invalid syntax",
                "strconv.ParseUint: parsing \"300\": value out of range",
                "strconv.ParseFloat: parsing \"1e400\": value out of range",
            ],
            "NumError's words"
        );
    }

    #[test]
    fn floats_convert_as_amd64_does() {
        assert_eq!(float_to_int64(f64::NAN), i64::MIN, "NaN");
        assert_eq!(float_to_int64(1e19), i64::MIN, "above the range");
        assert_eq!(float_to_int64(-1e19), i64::MIN, "below the range");
        assert_eq!(float_to_int64(-2.9), -2, "truncated toward zero");
        assert_eq!(
            float_to_int64(f64::from_bits(0xC3E0_0000_0000_0000)),
            i64::MIN,
            "MinInt64 itself"
        );
        assert_eq!(
            round_to_float32(0.1).to_bits(),
            f64::from(0.1_f32).to_bits(),
            "0.1 as a float32"
        );
        assert_eq!(
            round_to_float32(f64::from_bits(0x47EF_FFFF_F000_0000)),
            f64::INFINITY,
            "the midpoint above the largest float32 rounds to even, which is infinity"
        );
    }
}
