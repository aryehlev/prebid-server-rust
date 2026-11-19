//! Glue (Rule B (c)): `github.com/spf13/cast` v1.5.0, the conversions Viper's `Get` applies to a
//! key whose default has a basic type (`SetTypeByDefaultValue(true)`, config/config.go:1319;
//! ledger: `config.weak-typing`, `config.env-bool-spellings`, `config.file-bool-spellings`).
//!
//! Each function is the cast function without the `E` suffix: Go's `ToBool(i)` is
//! `v, _ := ToBoolE(i); return v`, so a value cast cannot convert gives the zero value (and for
//! `ToStringSlice` a nil slice), never an error. Integers and floats parse and print with
//! `strconv` (`pbs-gocompat`), as cast does.

use pbs_gocompat::{
    math,
    strconv::{format_float, parse_bool, parse_float, parse_int},
    strings,
};

use super::value::GoValue;

/// Go: `cast.ToBool`: `bool`, `nil` (false), `int` (non-zero), and `string` through
/// `strconv.ParseBool`; any other value (an `int64`, a `float64`, a map) is false.
#[must_use]
pub(super) fn to_bool(v: &GoValue) -> bool {
    match v {
        GoValue::Bool(b) => *b,
        GoValue::Int(i) => *i != 0,
        GoValue::String(s) => parse_bool(s).unwrap_or(false),
        GoValue::Nil
        | GoValue::Int64(_)
        | GoValue::Uint64(_)
        | GoValue::Float64(_)
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => false,
    }
}

/// Go: `cast.ToInt`: integers (a `uint64` wraps), floats truncated as amd64 converts them,
/// strings through `strconv.ParseInt(trimZeroDecimal(s), 0, 0)`, and `bool` as 1 or 0.
#[must_use]
pub(super) fn to_int(v: &GoValue) -> i64 {
    match v {
        GoValue::Int(i) | GoValue::Int64(i) => *i,
        GoValue::Uint64(u) => u.cast_signed(),
        GoValue::Float64(f) => math::int64(*f),
        GoValue::String(s) => parse_int(trim_zero_decimal(s), 0, 0).unwrap_or(0),
        GoValue::Bool(b) => i64::from(*b),
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => 0,
    }
}

/// Go: `cast.ToFloat64`: integers converted, strings through `strconv.ParseFloat(s, 64)`, and
/// `bool` as 1 or 0.
#[must_use]
pub(super) fn to_float64(v: &GoValue) -> f64 {
    match v {
        GoValue::Float64(f) => *f,
        GoValue::Int(i) | GoValue::Int64(i) => int_to_float(*i),
        GoValue::Uint64(u) => uint_to_float(*u),
        GoValue::String(s) => parse_float(s, 64).unwrap_or(0.0),
        GoValue::Bool(b) => f64::from(u8::from(*b)),
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => 0.0,
    }
}

/// Go: `cast.ToStringE`: `None` where cast fails (a slice or a map).
#[must_use]
pub(super) fn to_string_checked(v: &GoValue) -> Option<String> {
    match v {
        GoValue::String(s) => Some(s.clone()),
        GoValue::Bool(b) => Some(b.to_string()),
        GoValue::Float64(f) => Some(format_float(*f, b'f', -1, 64)),
        GoValue::Int(i) | GoValue::Int64(i) => Some(i.to_string()),
        GoValue::Uint64(u) => Some(u.to_string()),
        GoValue::Nil => Some(String::new()),
        GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => None,
    }
}

/// Go: `cast.ToString`: the empty string where cast fails.
#[must_use]
pub(super) fn to_string(v: &GoValue) -> String {
    to_string_checked(v).unwrap_or_default()
}

/// Go: `cast.ToStringSlice`: a `[]interface{}` element by element (`cast.ToString`, so an
/// element cast cannot convert is ""; an empty one gives a nil slice), a `[]string` as it is, a
/// string split by `strings.Fields`, and any other value cast can make a string of as a
/// one-element slice. A value it cannot convert gives a nil slice.
#[must_use]
pub(super) fn to_string_slice(v: &GoValue) -> Option<Vec<String>> {
    match v {
        GoValue::List(items) => {
            if items.is_empty() {
                None
            } else {
                Some(items.iter().map(to_string).collect())
            }
        }
        GoValue::Strings(items) => items.clone(),
        GoValue::String(s) => Some(strings::fields(s).into_iter().map(str::to_owned).collect()),
        GoValue::Nil => None,
        GoValue::Bool(_)
        | GoValue::Int(_)
        | GoValue::Int64(_)
        | GoValue::Uint64(_)
        | GoValue::Float64(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => to_string_checked(v).map(|s| vec![s]),
    }
}

/// Go: cast's `trimZeroDecimal`: "1.0" and "1.00" become "1", ".0" becomes "", and any other
/// string is returned as it is.
fn trim_zero_decimal(s: &str) -> &str {
    let mut found_zero = false;
    for (i, c) in s.char_indices().rev() {
        match c {
            '.' if found_zero => return s.get(..i).unwrap_or(s),
            '.' => {}
            '0' => found_zero = true,
            _ => return s,
        }
    }
    s
}

/// Go: `float64(i)` for an `int`/`int64`, rounded to nearest even as Go converts.
pub(super) fn int_to_float(i: i64) -> f64 {
    i.to_string().parse().unwrap_or(0.0)
}

/// Go: `float64(u)` for a `uint64`.
pub(super) fn uint_to_float(u: u64) -> f64 {
    u.to_string().parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn trims_zero_decimals_as_cast_does() {
        for (input, want) in [
            ("1.0", "1"),
            ("1.00", "1"),
            ("10.0", "10"),
            (".0", ""),
            ("1.", "1."),
            ("100", "100"),
            ("1.0.0", "1.0"),
            ("1.5", "1.5"),
        ] {
            assert_eq!(trim_zero_decimal(input), want, "trimZeroDecimal({input:?})");
        }
    }

    #[test]
    fn nil_and_typed_values() {
        assert!(
            !to_bool(&GoValue::Int64(1)),
            "cast.ToBool(int64(1)) is false"
        );
        assert_eq!(
            to_string_slice(&GoValue::List(Vec::new())),
            None,
            "an empty []interface{{}} gives a nil slice"
        );
        assert_eq!(
            to_string_slice(&GoValue::Map(BTreeMap::new())),
            None,
            "a map gives a nil slice"
        );
    }
}
