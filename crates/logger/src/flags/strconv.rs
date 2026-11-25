//! Glue (Rule B (c): CLI flags): the `strconv` functions glog's and the flag package's `Set`
//! methods read flag values with, as Go 1.26.2 writes them (internal/strconv/atob.go and
//! atoi.go): `ParseBool`, `Atoi` and `ParseInt` with base 0, whose prefixes and `_` separators
//! std does not read and whose error kind depends on where the scan stops. Errors keep
//! `strconv.NumError`'s words with the input quoted as Rust quotes strings (ledger:
//! `logging.flag-texts`).

use std::fmt;

/// Why a parse failed (Go: `NumError.Err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NumErrorKind {
    /// `strconv.ErrSyntax`.
    Syntax,
    /// `strconv.ErrRange`.
    Range,
}

/// Go: the `*strconv.NumError` `Atoi` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AtoiError {
    num: String,
    kind: NumErrorKind,
}

impl fmt::Display for AtoiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.kind {
            NumErrorKind::Syntax => "invalid syntax",
            NumErrorKind::Range => "value out of range",
        };
        write!(f, "strconv.Atoi: parsing {:?}: {reason}", self.num)
    }
}

/// Go: `strconv.ParseBool`.
pub(super) fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Some(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Some(false),
        _ => None,
    }
}

/// Go: `strconv.Atoi` on a 64-bit platform, which is `ParseInt(s, 10, 64)`.
pub(super) fn atoi(s: &str) -> Result<i64, AtoiError> {
    parse_int(s, false).map_err(|kind| AtoiError {
        num: s.to_owned(),
        kind,
    })
}

/// Go: `strconv.ParseInt(s, base, 64)` with base 10, or with base 0 when `base0`: a `0b`, `0o`,
/// `0x` or `0` prefix picks the base and `_` may separate digits.
pub(super) fn parse_int(s: &str, base0: bool) -> Result<i64, NumErrorKind> {
    let (negative, magnitude) = match s.as_bytes() {
        [b'+', rest @ ..] => (false, rest),
        [b'-', rest @ ..] => (true, rest),
        all => (false, all),
    };
    let magnitude = match parse_uint(magnitude, base0) {
        Ok(n) => n,
        Err(NumErrorKind::Range) => u64::MAX,
        Err(NumErrorKind::Syntax) => return Err(NumErrorKind::Syntax),
    };
    let value = if negative {
        0_i64.checked_sub_unsigned(magnitude)
    } else {
        i64::try_from(magnitude).ok()
    };
    value.ok_or(NumErrorKind::Range)
}

/// Go: `strconv.ParseUint(s, base, 64)`, which stops at the first digit that is not one, or that
/// overflows, and only then checks the separators.
fn parse_uint(s: &[u8], base0: bool) -> Result<u64, NumErrorKind> {
    if s.is_empty() {
        return Err(NumErrorKind::Syntax);
    }
    let (base, digits): (u64, &[u8]) = match s {
        [b'0', prefix, rest @ ..] if base0 && !rest.is_empty() => {
            match prefix.to_ascii_lowercase() {
                b'b' => (2, rest),
                b'o' => (8, rest),
                b'x' => (16, rest),
                _ => (8, s.get(1..).unwrap_or_default()),
            }
        }
        [b'0', rest @ ..] if base0 => (8, rest),
        _ => (10, s),
    };
    let cutoff = u64::MAX / base + 1;
    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in digits {
        let digit = if c == b'_' && base0 {
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            c - b'0'
        } else if c.is_ascii_alphabetic() {
            c.to_ascii_lowercase() - b'a' + 10
        } else {
            return Err(NumErrorKind::Syntax);
        };
        if u64::from(digit) >= base {
            return Err(NumErrorKind::Syntax);
        }
        if n >= cutoff {
            return Err(NumErrorKind::Range);
        }
        n = (n * base)
            .checked_add(u64::from(digit))
            .ok_or(NumErrorKind::Range)?;
    }
    if underscores && !underscore_ok(s) {
        return Err(NumErrorKind::Syntax);
    }
    Ok(n)
}

/// Go: `underscoreOK`: an `_` only between digits, or between a base prefix and a digit.
fn underscore_ok(s: &[u8]) -> bool {
    let s = match s {
        [b'+' | b'-', rest @ ..] => rest,
        all => all,
    };
    let (mut saw, hex, digits) = match s {
        [b'0', prefix, rest @ ..] if matches!(prefix.to_ascii_lowercase(), b'b' | b'o' | b'x') => {
            (b'0', prefix.eq_ignore_ascii_case(&b'x'), rest)
        }
        all => (b'^', false, all),
    };
    for &c in digits {
        if c.is_ascii_digit() || hex && c.is_ascii_hexdigit() {
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

/// Go's `int32(v)`: the low 32 bits.
pub(super) const fn to_int32(v: i64) -> i32 {
    let [b0, b1, b2, b3, ..] = v.to_le_bytes();
    i32::from_le_bytes([b0, b1, b2, b3])
}

/// Go's `int8(v)`: the low 8 bits.
pub(super) const fn to_int8(v: i64) -> i8 {
    let [low, ..] = v.to_le_bytes();
    i8::from_le_bytes([low])
}

#[cfg(test)]
mod tests {
    use super::{NumErrorKind, atoi, parse_bool, parse_int, to_int8, to_int32};

    /// Cases of Go 1.26.2's `parseInt64Tests` (`strconv/atoi_test.go`) and `Atoi`'s error text;
    /// the flag goldens check the same functions through the command line.
    #[test]
    fn decimal_cases_match_go() {
        let decimal: &[(&str, Result<i64, NumErrorKind>)] = &[
            ("", Err(NumErrorKind::Syntax)),
            ("0", Ok(0)),
            ("-0", Ok(0)),
            ("+0", Ok(0)),
            ("1", Ok(1)),
            ("-1", Ok(-1)),
            ("+1", Ok(1)),
            ("12345", Ok(12345)),
            ("-12345", Ok(-12345)),
            ("012345", Ok(12345)),
            ("-012345", Ok(-12345)),
            ("98765432100", Ok(98_765_432_100)),
            ("9223372036854775807", Ok(i64::MAX)),
            ("-9223372036854775807", Ok(-i64::MAX)),
            ("9223372036854775808", Err(NumErrorKind::Range)),
            ("-9223372036854775808", Ok(i64::MIN)),
            ("9223372036854775809", Err(NumErrorKind::Range)),
            ("-9223372036854775809", Err(NumErrorKind::Range)),
            ("-1_2_3_4_5", Err(NumErrorKind::Syntax)),
            ("-_12345", Err(NumErrorKind::Syntax)),
            ("_12345", Err(NumErrorKind::Syntax)),
            ("1__2345", Err(NumErrorKind::Syntax)),
            ("12345_", Err(NumErrorKind::Syntax)),
            ("123%45", Err(NumErrorKind::Syntax)),
            ("+", Err(NumErrorKind::Syntax)),
            ("-", Err(NumErrorKind::Syntax)),
            ("0x10", Err(NumErrorKind::Syntax)),
            ("9999999999999999999x", Err(NumErrorKind::Syntax)),
            ("99999999999999999999x", Err(NumErrorKind::Range)),
        ];
        for (s, want) in decimal {
            assert_eq!(&parse_int(s, false), want, "ParseInt({s:?}, 10, 64)");
        }
        assert_eq!(
            atoi("x").map_err(|err| err.to_string()),
            Err("strconv.Atoi: parsing \"x\": invalid syntax".to_owned()),
            "Atoi's error"
        );
    }

    /// Cases of Go 1.26.2's `parseInt64BaseTests` with base 0 (`strconv/atoi_test.go`).
    #[test]
    fn base_zero_cases_match_go() {
        let prefixed: &[(&str, Result<i64, NumErrorKind>)] = &[
            ("0", Ok(0)),
            ("-0", Ok(0)),
            ("1", Ok(1)),
            ("-1", Ok(-1)),
            ("12345", Ok(12345)),
            ("-12345", Ok(-12345)),
            ("012345", Ok(0o12345)),
            ("-012345", Ok(-0o12345)),
            ("0x12345", Ok(0x12345)),
            ("-0X12345", Ok(-0x12345)),
            ("12345x", Err(NumErrorKind::Syntax)),
            ("-12345x", Err(NumErrorKind::Syntax)),
            ("98765432100", Ok(98_765_432_100)),
            ("9223372036854775807", Ok(i64::MAX)),
            ("9223372036854775808", Err(NumErrorKind::Range)),
            ("-9223372036854775808", Ok(i64::MIN)),
            ("0x7FFFFFFFFFFFFFFF", Ok(i64::MAX)),
            ("0X8000000000000000", Err(NumErrorKind::Range)),
            ("-0x8000000000000000", Ok(i64::MIN)),
            ("0b1010", Ok(10)),
            ("0B1010", Ok(10)),
            ("0o17", Ok(15)),
            ("0O17", Ok(15)),
            ("08", Err(NumErrorKind::Syntax)),
            ("0x", Err(NumErrorKind::Syntax)),
            ("0b", Err(NumErrorKind::Syntax)),
            ("0b2", Err(NumErrorKind::Syntax)),
            ("1_2_3_4_5", Ok(12345)),
            ("-1_2_3_4_5", Ok(-12345)),
            ("0x_1_2_3_4_5", Ok(0x12345)),
            ("0_1_2_3_4_5", Ok(0o12345)),
            ("0o_1_2_3_4_5", Ok(0o12345)),
            ("_12345", Err(NumErrorKind::Syntax)),
            ("-_12345", Err(NumErrorKind::Syntax)),
            ("1__2345", Err(NumErrorKind::Syntax)),
            ("12345_", Err(NumErrorKind::Syntax)),
            ("0x_", Err(NumErrorKind::Syntax)),
            ("0_", Err(NumErrorKind::Syntax)),
        ];
        for (s, want) in prefixed {
            assert_eq!(&parse_int(s, true), want, "ParseInt({s:?}, 0, 64)");
        }
    }

    /// Cases of Go 1.26.2's `atobtests` (`strconv/atob_test.go`) and Go's `int32` and `int8`
    /// conversions.
    #[test]
    fn bools_and_conversions_match_go() {
        let bools: &[(&str, Option<bool>)] = &[
            ("", None),
            ("asdf", None),
            ("0", Some(false)),
            ("f", Some(false)),
            ("F", Some(false)),
            ("FALSE", Some(false)),
            ("false", Some(false)),
            ("False", Some(false)),
            ("1", Some(true)),
            ("t", Some(true)),
            ("T", Some(true)),
            ("TRUE", Some(true)),
            ("true", Some(true)),
            ("True", Some(true)),
            ("tRUE", None),
        ];
        for (s, want) in bools {
            assert_eq!(&parse_bool(s), want, "ParseBool({s:?})");
        }
        assert_eq!(
            (
                to_int32(4_294_967_297),
                to_int32(-1),
                to_int32(2_147_483_648)
            ),
            (1, -1, i32::MIN),
            "int32(v)"
        );
        assert_eq!(
            (to_int8(256), to_int8(259), to_int8(-129), to_int8(128)),
            (0, 3, 127, i8::MIN),
            "int8(v)"
        );
    }
}
