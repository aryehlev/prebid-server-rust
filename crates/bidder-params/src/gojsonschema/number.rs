//! Glue (Rule B (d)): gojsonschema's numbers, which it reads as `json.Number` with `math/big`,
//! on `serde_json`'s, and the way it prints a limit in an error.
//!
//! `serde_json` holds an integer literal in the range of `u64` or `i64` exactly and every other
//! number as the nearest `f64`, where Go keeps the literal and reads it as an exact rational, so
//! the checks here are Go's for every number `f64` holds exactly, which includes every integer
//! up to 2^53 and every limit the schemas use (jsonschema.number-precision).

use std::cmp::Ordering;

use serde_json::{Number, Value};

/// Go: `checkJSONInteger`: whether `big.Rat` reads the number as an integer.
pub(super) fn is_integer(n: &Number) -> bool {
    n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
}

/// The number as an exact integer, where `serde_json` holds it as one.
fn exact_integer(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}

/// Go: `big.Rat.Cmp`: exact for two integers, by value as `f64` otherwise.
pub(super) fn compare(a: &Number, b: &Number) -> Ordering {
    if let (Some(x), Some(y)) = (exact_integer(a), exact_integer(b)) {
        return x.cmp(&y);
    }
    let (x, y) = (
        a.as_f64().unwrap_or_default(),
        b.as_f64().unwrap_or_default(),
    );
    // Numbers are finite, and -0 equals 0, as for rationals.
    x.partial_cmp(&y).unwrap_or(Ordering::Equal)
}

/// Go: `mustBeNumber`.
pub(super) const fn must_be_number(v: &Value) -> Option<&Number> {
    if let Value::Number(n) = v {
        Some(n)
    } else {
        None
    }
}

/// Go: `mustBeInteger`: a number `big.Rat` reads as an integer and `json.Number.Int64`
/// (`strconv.ParseInt`) reads, which a fraction or an exponent (`1.0`, `1e2`) fails.
///
/// `serde_json` reads such a literal as an `i64` or a `u64`, and every literal with a fraction
/// or an exponent as an `f64`, with one exception: `-0` is the `f64` -0.0, as `-0.0` is. The
/// port reads -0.0 as the integer `-0` (jsonschema.number-precision: Go refuses `-0.0`).
pub(super) fn must_be_integer(v: &Value) -> Option<i64> {
    let Value::Number(n) = v else {
        return None;
    };
    n.as_i64().or_else(|| {
        n.as_f64()
            .filter(|f| *f == 0.0 && f.is_sign_negative())
            .map(|_| 0)
    })
}

/// The decimal digits of a non-zero number without leading or trailing zeros, and the position
/// of the decimal point: the value is `0.<digits> * 10^exponent`, negated when `negative`.
struct Decimal {
    negative: bool,
    digits: String,
    exponent: i64,
}

impl Decimal {
    /// `None` for zero. Integers are exact; other numbers have the shortest digits that read
    /// back as the same `f64`.
    fn of(n: &Number) -> Option<Self> {
        if let Some(i) = exact_integer(n) {
            let text = i.unsigned_abs().to_string();
            let digits = text.trim_end_matches('0');
            if digits.is_empty() {
                return None;
            }
            return Some(Self {
                negative: i < 0,
                digits: digits.to_owned(),
                exponent: i64::try_from(text.len()).ok()?,
            });
        }
        let f = n.as_f64()?;
        if f == 0.0 {
            return None;
        }
        // `{:e}` writes the shortest digits: `1.5e-7`, `1e21`.
        let text = format!("{:e}", f.abs());
        let (mantissa, exp) = text.split_once('e')?;
        let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
        Some(Self {
            negative: f < 0.0,
            digits: digits.trim_end_matches('0').to_owned(),
            exponent: exp.parse::<i64>().ok()? + 1,
        })
    }
}

/// Go: `new(big.Float).SetRat(limit)` as the error templates print it (`%v`, which is `%g` with
/// the shortest digits): `1`, `0.5`, `10000`, `1e+06`, `1.5e-07`.
pub(super) fn limit_text(n: &Number) -> String {
    let Some(d) = Decimal::of(n) else {
        return "0".to_owned();
    };
    let mut out = String::new();
    if d.negative {
        out.push('-');
    }
    // Go: %e when the exponent of the first digit is below -4 or at least 6 (eprec for the
    // shortest digits), %f otherwise.
    let first = d.exponent - 1;
    let (lead, rest) = d.digits.split_at(1.min(d.digits.len()));
    if !(-4..6).contains(&first) {
        out.push_str(lead);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push_str(if first < 0 { "e-" } else { "e+" });
        let magnitude = first.unsigned_abs();
        if magnitude < 10 {
            out.push('0');
        }
        out.push_str(&magnitude.to_string());
    } else if d.exponent <= 0 {
        out.push_str("0.");
        for _ in d.exponent..0 {
            out.push('0');
        }
        out.push_str(&d.digits);
    } else {
        let point = usize::try_from(d.exponent).unwrap_or(usize::MAX);
        if let Some((int, frac)) = d.digits.split_at_checked(point) {
            out.push_str(int);
            if !frac.is_empty() {
                out.push('.');
                out.push_str(frac);
            }
        } else {
            out.push_str(&d.digits);
            for _ in d.digits.len()..point {
                out.push('0');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use serde_json::{Number, Value, json};

    use super::{compare, is_integer, limit_text, must_be_integer};

    fn number(text: &str) -> Result<Number, String> {
        let value = serde_json::from_str::<Value>(text).map_err(|e| e.to_string())?;
        value
            .as_number()
            .cloned()
            .ok_or_else(|| format!("{value} is not a number"))
    }

    /// Go: `fmt.Sprint(new(big.Float).SetRat(r))` for each literal, under Go 1.26.2.
    #[test]
    fn limits_print_like_big_float() -> Result<(), String> {
        let cases = [
            ("0", "0"),
            ("-0", "0"),
            ("1", "1"),
            ("17", "17"),
            ("100", "100"),
            ("10000", "10000"),
            ("100000", "100000"),
            ("999999", "999999"),
            ("1000000", "1e+06"),
            ("1234567", "1.234567e+06"),
            ("-12", "-12"),
            ("0.5", "0.5"),
            ("1.25", "1.25"),
            ("0.0001", "0.0001"),
            ("0.00001", "1e-05"),
            ("1.5e-7", "1.5e-07"),
            ("1e100", "1e+100"),
            ("-2.5e10", "-2.5e+10"),
            ("18446744073709551615", "1.8446744073709551615e+19"),
            ("-9223372036854775808", "-9.223372036854775808e+18"),
        ];
        for (literal, want) in cases {
            let got = limit_text(&number(literal)?);
            if got != want {
                return Err(format!("{literal}: got {got}, want {want}"));
            }
        }
        Ok(())
    }

    #[test]
    fn integers_and_comparisons_follow_rationals() -> Result<(), String> {
        for (literal, integer) in [
            ("1", true),
            ("-0", true),
            ("1.0", true),
            ("1e2", true),
            ("1.5", false),
            ("18446744073709551615", true),
        ] {
            if is_integer(&number(literal)?) != integer {
                return Err(format!("{literal}: want integer {integer}"));
            }
        }
        let ordered = [
            ("9007199254740993", "9007199254740992", Ordering::Greater),
            ("-0", "0", Ordering::Equal),
            ("0.5", "1", Ordering::Less),
            ("1.0", "1", Ordering::Equal),
            ("18446744073709551615", "-1", Ordering::Greater),
        ];
        for (a, b, want) in ordered {
            if compare(&number(a)?, &number(b)?) != want {
                return Err(format!("{a} vs {b}: want {want:?}"));
            }
        }
        let integers = [
            (json!(3), Some(3)),
            (json!(-1), Some(-1)),
            (Value::Number(number("-0")?), Some(0)),
            (json!(1.0), None),
            (Value::Number(number("1e2")?), None),
            (json!(u64::MAX), None),
            (json!("1"), None),
        ];
        for (value, want) in integers {
            if must_be_integer(&value) != want {
                return Err(format!("{value}: want {want:?}"));
            }
        }
        Ok(())
    }
}
