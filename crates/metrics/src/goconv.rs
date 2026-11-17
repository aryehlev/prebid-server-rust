//! The Go standard library conversions the engines depend on: numeric conversions,
//! `strconv.FormatFloat(f, 'g', -1, 64)`, `strings.ToLower` and `strconv.Quote`.
//!
//! These belong in `pbs-gocompat` (`strconv`/`strings`) once it exposes them; they live here so
//! this crate does not depend on unported code.

/// Go: `float64(v)` for a `uint64` (round to nearest, ties to even).
///
/// The value is split into two exactly representable halves; `hi * 2^32` is exact, so the fused
/// multiply-add rounds the exact integer once, which is the rounding of the integer conversion.
#[must_use]
pub fn u64_to_f64(v: u64) -> f64 {
    let hi = u32::try_from(v >> 32).unwrap_or(u32::MAX);
    let lo = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(u32::MAX);
    f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
}

/// Go: `float64(v)` for an `int64`.
#[must_use]
pub fn i64_to_f64(v: i64) -> f64 {
    let magnitude = u64_to_f64(v.unsigned_abs());
    if v < 0 { -magnitude } else { magnitude }
}

/// Go: `float64(v)` for an `int` (`usize` on the Rust side).
#[must_use]
pub fn usize_to_f64(v: usize) -> f64 {
    u64_to_f64(u64::try_from(v).unwrap_or(u64::MAX))
}

/// 2^64 (`0x43F0_0000_0000_0000` is its IEEE-754 encoding).
const TWO_POW_64: f64 = f64::from_bits(0x43F0_0000_0000_0000);

/// The integer `v` holds when it is a whole number in `[0, 2^64)`: exactly the values for which
/// Go's `uint64(v)` round-trips (`float64(uint64(v)) == v`).
#[must_use]
pub fn f64_to_u64_exact(v: f64) -> Option<u64> {
    if v == 0.0 {
        return Some(0);
    }
    if !(v.is_finite() && v > 0.0 && v < TWO_POW_64) || v.fract() != 0.0 {
        return None;
    }
    let bits = v.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7FF).ok()? - 1075;
    let mantissa = (bits & 0x000F_FFFF_FFFF_FFFF) | 0x0010_0000_0000_0000;
    if exponent >= 0 {
        mantissa.checked_shl(u32::try_from(exponent).ok()?)
    } else {
        mantissa.checked_shr(u32::try_from(-exponent).ok()?)
    }
}

/// Go: `strconv.FormatFloat(f, 'g', -1, 64)`.
///
/// Go prints the shortest decimal digits that round-trip, in `%e` form when the decimal exponent
/// is below -4 or at least 6 (for the shortest format Go fixes the `%e` threshold precision at
/// 6, `strconv/ftoa.go` `formatDigits`), and in `%f` form otherwise.
#[must_use]
pub fn format_float_g(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let shortest = shortest_scientific(f);
    let (neg, unsigned) = shortest
        .strip_prefix('-')
        .map_or((false, shortest.as_str()), |rest| (true, rest));
    let (mantissa, exp) = unsigned.split_once('e').unwrap_or((unsigned, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let mut out = String::with_capacity(24);
    if neg {
        out.push('-');
    }
    if digits.iter().all(|d| *d == b'0') {
        // Go's decimal for zero has no digits and dp = 0, which prints as "0".
        out.push('0');
        return out;
    }
    if (-4..6).contains(&exp) {
        let nd = i64::try_from(digits.len()).unwrap_or(i64::MAX);
        fmt_f(&mut out, &digits, nd, exp + 1);
    } else {
        fmt_e(&mut out, &digits, exp);
    }
    out
}

/// The shortest round-trip digits of `f` as `d.ddde<exp>`, chosen like Go's `ryuFtoaShortest`.
///
/// Rust's `{:e}` prints the shortest digit count that round-trips, but when the exact value lies
/// halfway between two such decimals it takes the upper one, where Go rounds half to even (for
/// 1126620304556358.25 Rust prints `...583` and Go `...582`). Rust's fixed-precision `{:.N$e}`
/// rounds the exact value half to even, so with the shortest digit count it gives Go's digits
/// whenever they round-trip.
fn shortest_scientific(f: f64) -> String {
    let shortest = format!("{f:e}");
    let mantissa = shortest
        .split_once('e')
        .map_or(shortest.as_str(), |(m, _)| m);
    let precision = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .count()
        .saturating_sub(1);
    let nearest = format!("{f:.precision$e}");
    if nearest != shortest
        && nearest
            .parse::<f64>()
            .is_ok_and(|p| p.to_bits() == f.to_bits())
    {
        nearest
    } else {
        shortest
    }
}

/// Go: `strconv.fmtE` with the shortest precision (`nd - 1` fractional digits).
fn fmt_e(out: &mut String, digits: &[u8], exp: i64) {
    let mut chars = digits.iter().map(|d| char::from(*d));
    out.push(chars.next().unwrap_or('0'));
    let rest: String = chars.collect();
    if !rest.is_empty() {
        out.push('.');
        out.push_str(&rest);
    }
    out.push('e');
    out.push(if exp < 0 { '-' } else { '+' });
    let magnitude = exp.unsigned_abs();
    if magnitude < 10 {
        out.push('0');
    }
    out.push_str(&magnitude.to_string());
}

/// Go: `strconv.fmtF` with `max(nd - dp, 0)` fractional digits.
fn fmt_f(out: &mut String, digits: &[u8], nd: i64, dp: i64) {
    let digit_at = |j: i64| -> char {
        usize::try_from(j)
            .ok()
            .and_then(|j| digits.get(j))
            .map_or('0', |d| char::from(*d))
    };
    if dp > 0 {
        for j in 0..dp {
            out.push(digit_at(j));
        }
    } else {
        out.push('0');
    }
    let frac = (nd - dp).max(0);
    if frac > 0 {
        out.push('.');
        for i in 1..=frac {
            out.push(digit_at(dp + i - 1));
        }
    }
}

/// Go: `strings.ToLower`, which maps every rune with `unicode.ToLower` (the simple, one-rune
/// case mapping) with Go's Unicode tables, not the toolchain's (`goconv_tables.rs`).
#[must_use]
pub fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars().map(to_lower_rune).collect()
}

/// Go: `unicode.ToLower`.
fn to_lower_rune(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let r = u32::from(c);
    let (runs, _) = tables::LOWER_RUNS.as_chunks::<4>();
    let index = runs.partition_point(|&[_, last, _, _]| last < r);
    match runs.get(index) {
        Some(&[first, _, step, first_lower])
            if first <= r && (r - first).checked_rem(step) == Some(0) =>
        {
            char::from_u32(first_lower + (r - first)).unwrap_or(c)
        }
        _ => c,
    }
}

/// Go: `strconv.IsPrint` (letters, marks, numbers, punctuation, symbols and the ASCII space),
/// with Go's Unicode tables.
fn is_print(c: char) -> bool {
    if c.is_ascii() {
        return c == ' ' || c.is_ascii_graphic();
    }
    let r = u32::from(c);
    let (runs, _) = tables::NOT_PRINTABLE.as_chunks::<2>();
    let index = runs.partition_point(|&[_, last]| last < r);
    runs.get(index).is_none_or(|&[first, _]| first > r)
}

/// Go: `strconv.Quote`, as `fmt`'s `%q` verb prints a string.
///
/// `"` and `\` are backslashed, printable runes (Go's `strconv.IsPrint`) are kept, and every
/// other rune is escaped (`\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\xXX` below U+0020 and for
/// DEL, `\uXXXX`, `\UXXXXXXXX`), as `strconv/quote.go` `appendEscapedRune` does.
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if is_print(c) => out.push(c),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if c < ' ' || c == '\u{7f}' => push_hex_escape(&mut out, "\\x", u32::from(c), 2),
            c if u32::from(c) < 0x1_0000 => push_hex_escape(&mut out, "\\u", u32::from(c), 4),
            c => push_hex_escape(&mut out, "\\U", u32::from(c), 8),
        }
    }
    out.push('"');
    out
}

/// Appends `prefix` and the `width` lowercase hex digits of `value`.
fn push_hex_escape(out: &mut String, prefix: &str, value: u32, width: u32) {
    out.push_str(prefix);
    for shift in (0..width).rev() {
        let nibble = value.checked_shr(shift * 4).unwrap_or(0) & 0xF;
        out.push(char::from_digit(nibble, 16).unwrap_or('0'));
    }
}

#[path = "goconv_tables.rs"]
mod tables;

#[cfg(test)]
#[path = "goconv_test.rs"]
mod tests;
