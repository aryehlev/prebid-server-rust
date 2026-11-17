//! Go `github.com/spf13/cast` v1.5.0 (`cast.go`, `caste.go`): the conversions viper v1.12.0's
//! typed getters (`GetBool`, `GetStringSlice`, ...) and `SetTypeByDefaultValue` apply to
//! [`Value`]s.
//!
//! viper calls the forms that drop the error (`cast.ToBool`, not `cast.ToBoolE`), so each function
//! here returns what the Go one returns: on a failed conversion, the zero value the `E` form
//! returned alongside its error. That is `false`, `0` or `""` for scalars, a nil slice from
//! [`to_string_slice`], an empty one from [`to_int_slice`], and an empty (or, from a JSON string,
//! partly decoded) map from the map conversions. `None` stands for a nil slice or map.
//!
//! Go's type switches match exact dynamic types: a `time.Duration` is not an `int64` to them, a
//! `uint16` is no case of [`to_time`], and only a Go `int` (not `int64`) is true in [`to_bool`].
//! Go `int` and `uint` are 64 bits, so [`to_int`] returns `i64` and [`to_uint`] `u64`; numeric
//! conversions wrap, truncate and convert floats as Go's do on amd64 (`int32(3e9)` is
//! `-2147483648`, `uint32(-3e9)` is `1294967296`, `uint64(NaN)` is `1 << 63`).

use std::collections::BTreeMap;

use pbs_gocompat::{
    math, strconv,
    strings::fields,
    time::{
        self, ANSIC, DateFields, Duration, KITCHEN, Location, RFC822, RFC822Z, RFC850, RFC1123,
        RFC1123Z, RFC3339, RUBY_DATE, STAMP, STAMP_MICRO, STAMP_MILLI, STAMP_NANO, Time, UNIX_DATE,
        parse_duration,
    },
};
use pbs_gojson::stdjson;
use pbs_goyaml::{Type, Value};

/// A conversion that failed (Go: the `E` forms' non-nil error, whose text viper never shows).
#[derive(Debug, Clone, Copy)]
struct Failed;

/// A number or bool, by the class Go's `case int64:`-style arms put it in.
#[derive(Debug, Clone, Copy)]
enum Number {
    /// `int`, `int64`, `int32`, `int16` and `int8`.
    Signed(i64),
    /// `uint`, `uint64`, `uint32`, `uint16` and `uint8`.
    Unsigned(u64),
    /// `float64`, and `float32` widened exactly.
    Float(f64),
    /// `bool`.
    Bool(bool),
}

/// The value as a [`Number`], if its dynamic type is a Go number or bool (a `time.Duration` is
/// not one).
fn number(v: &Value) -> Option<Number> {
    Some(match *v {
        Value::Int(i) | Value::Int64(i) => Number::Signed(i),
        Value::Int32(i) => Number::Signed(i.into()),
        Value::Int16(i) => Number::Signed(i.into()),
        Value::Int8(i) => Number::Signed(i.into()),
        Value::Uint(u) | Value::Uint64(u) => Number::Unsigned(u),
        Value::Uint32(u) => Number::Unsigned(u.into()),
        Value::Uint16(u) => Number::Unsigned(u.into()),
        Value::Uint8(u) => Number::Unsigned(u.into()),
        Value::Float64(f) => Number::Float(f),
        Value::Float32(f) => Number::Float(f.into()),
        Value::Bool(b) => Number::Bool(b),
        Value::Nil
        | Value::String(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => return None,
    })
}

/// `2^63` (Go's `float64(math.MaxInt64)`).
const TWO_POW_63: f64 = 9.223_372_036_854_776e18;

/// Go's `int64(u)` for a `uint64`.
const fn wrap_int(u: u64) -> i64 {
    i64::from_ne_bytes(u.to_ne_bytes())
}

/// Go's `uint64(i)` for an `int64`.
const fn wrap_uint(i: i64) -> u64 {
    u64::from_ne_bytes(i.to_ne_bytes())
}

/// Go's `int32(v)` for an `int64`: the low 32 bits.
const fn truncate_i32(value: i64) -> i32 {
    let [b0, b1, b2, b3, ..] = value.to_le_bytes();
    i32::from_le_bytes([b0, b1, b2, b3])
}

/// Go's `uint32(v)` for a `uint64`: the low 32 bits.
const fn truncate_u32(value: u64) -> u32 {
    let [b0, b1, b2, b3, ..] = value.to_le_bytes();
    u32::from_le_bytes([b0, b1, b2, b3])
}

/// Go's `int32(f)` on amd64 (`CVTTSD2SL`): `-2147483648` when out of range or NaN.
fn float_to_i32(f: f64) -> i32 {
    i32::try_from(math::int64(f)).unwrap_or(i32::MIN)
}

/// Go's `uint32(f)` on amd64: the low 32 bits of `int64(f)`.
fn float_to_u32(f: f64) -> u32 {
    truncate_u32(wrap_uint(math::int64(f)))
}

/// Go's `uint64(f)` on amd64: values below `2^63` convert through `int64` (so negative values
/// wrap), larger ones through `int64(f - 2^63)` with the top bit set; NaN and out-of-range values
/// give `1 << 63`.
fn float_to_u64(f: f64) -> u64 {
    if f < TWO_POW_63 {
        wrap_uint(math::int64(f))
    } else {
        wrap_uint(math::int64(f - TWO_POW_63)) | (1 << 63)
    }
}

/// Go's `float64(v)` for an `int64` (rounded to nearest, ties to even).
fn int_to_float(v: i64) -> f64 {
    i32::try_from(v).map_or_else(
        |_| strconv::parse_float(&strconv::format_int(v, 10), 64).unwrap_or(0.0),
        f64::from,
    )
}

/// Go's `float64(v)` for a `uint64` (rounded to nearest, ties to even).
fn uint_to_float(v: u64) -> f64 {
    u32::try_from(v).map_or_else(
        |_| strconv::parse_float(&strconv::format_uint(v, 10), 64).unwrap_or(0.0),
        f64::from,
    )
}

/// Go: `trimZeroDecimal`: drops a fractional part made only of zeros (`"8.00"` is `"8"`;
/// `"8.10"` and `"8."` are kept).
fn trim_zero_decimal(s: &str) -> &str {
    let mut found_zero = false;
    for (i, b) in s.bytes().enumerate().rev() {
        match b {
            b'.' => {
                if found_zero {
                    return s.get(..i).unwrap_or(s);
                }
            }
            b'0' => found_zero = true,
            _ => return s,
        }
    }
    s
}

/// Go: `strconv.ParseInt(trimZeroDecimal(s), 0, 0)`, the string case of the integer
/// conversions (base prefixes, so `"010"` is octal and `"08"` fails).
fn parse_integer(s: &str) -> Result<i64, Failed> {
    strconv::parse_int(trim_zero_decimal(s), 0, 0)
        .ok()
        .ok_or(Failed)
}

/// Go: `cast.ToBool`.
#[must_use]
pub fn to_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        // Only a Go int: int64 and the other integer types are not cases of the switch.
        Value::Int(i) => *i != 0,
        Value::String(s) => strconv::parse_bool(s).unwrap_or(false),
        Value::Nil
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => false,
    }
}

/// Go: `cast.ToStringE`, with `None` for its error.
fn string_e(v: &Value) -> Result<String, Failed> {
    Ok(match v {
        Value::Nil => String::new(),
        Value::Bool(b) => strconv::format_bool(*b).to_owned(),
        Value::Int(i) | Value::Int64(i) => strconv::format_int(*i, 10),
        Value::Int32(i) => strconv::format_int((*i).into(), 10),
        Value::Int16(i) => strconv::format_int((*i).into(), 10),
        Value::Int8(i) => strconv::format_int((*i).into(), 10),
        Value::Uint(u) | Value::Uint64(u) => strconv::format_uint(*u, 10),
        Value::Uint32(u) => strconv::format_uint((*u).into(), 10),
        Value::Uint16(u) => strconv::format_uint((*u).into(), 10),
        Value::Uint8(u) => strconv::format_uint((*u).into(), 10),
        Value::Float64(f) => strconv::format_float(*f, b'f', -1, 64),
        Value::Float32(f) => strconv::format_float((*f).into(), b'f', -1, 32),
        Value::String(s) => s.clone(),
        // Go: the fmt.Stringer case.
        Value::Duration(d) => d.string(),
        Value::Time(t) => t.to_string(),
        // Go: the []byte case (a []uint8, nil or not).
        Value::Slice(Type::Uint8, items) => {
            let bytes: Vec<u8> = items
                .iter()
                .flatten()
                .map(|item| if let Value::Uint8(b) = item { *b } else { 0 })
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Value::Slice(..) | Value::StringMap(..) | Value::Map(..) => return Err(Failed),
    })
}

/// Go: `cast.ToString`.
#[must_use]
pub fn to_string(v: &Value) -> String {
    string_e(v).unwrap_or_default()
}

/// Go: `cast.ToInt64`.
#[must_use]
pub fn to_int64(v: &Value) -> i64 {
    match number(v) {
        Some(Number::Signed(i)) => i,
        Some(Number::Unsigned(u)) => wrap_int(u),
        Some(Number::Float(f)) => math::int64(f),
        Some(Number::Bool(b)) => i64::from(b),
        None => v.as_str().map_or(0, |s| parse_integer(s).unwrap_or(0)),
    }
}

/// Go: `cast.ToInt` (Go's `int` is 64 bits, so this is [`to_int64`]).
#[must_use]
pub fn to_int(v: &Value) -> i64 {
    to_int64(v)
}

/// Go: `cast.ToIntE`, with `None` for its error.
fn int_e(v: &Value) -> Result<i64, Failed> {
    if number(v).is_some() || v.is_nil() {
        return Ok(to_int64(v));
    }
    v.as_str().map_or(Err(Failed), parse_integer)
}

/// Go: `cast.ToInt32`.
#[must_use]
pub fn to_int32(v: &Value) -> i32 {
    match number(v) {
        Some(Number::Signed(i)) => truncate_i32(i),
        Some(Number::Unsigned(u)) => truncate_i32(wrap_int(u)),
        Some(Number::Float(f)) => float_to_i32(f),
        Some(Number::Bool(b)) => i32::from(b),
        None => v
            .as_str()
            .map_or(0, |s| parse_integer(s).map_or(0, truncate_i32)),
    }
}

/// Go: `cast.ToUint64` (and `cast.ToUint`): negative values fail (and give 0).
#[must_use]
pub fn to_uint64(v: &Value) -> u64 {
    match number(v) {
        Some(Number::Signed(i)) => u64::try_from(i).unwrap_or(0),
        Some(Number::Unsigned(u)) => u,
        Some(Number::Float(f)) => {
            if f < 0.0 {
                0
            } else {
                float_to_u64(f)
            }
        }
        Some(Number::Bool(b)) => u64::from(b),
        // Go parses the string as a signed integer, so values above MaxInt64 fail.
        None => v.as_str().map_or(0, |s| {
            parse_integer(s).map_or(0, |i| u64::try_from(i).unwrap_or(0))
        }),
    }
}

/// Go: `cast.ToUint` (Go's `uint` is 64 bits, so this is [`to_uint64`]).
#[must_use]
pub fn to_uint(v: &Value) -> u64 {
    to_uint64(v)
}

/// Go: `cast.ToUint32`: negative values fail (and give 0), others truncate.
#[must_use]
pub fn to_uint32(v: &Value) -> u32 {
    match number(v) {
        Some(Number::Signed(i)) => u64::try_from(i).map_or(0, truncate_u32),
        Some(Number::Unsigned(u)) => truncate_u32(u),
        Some(Number::Float(f)) => {
            if f < 0.0 {
                0
            } else {
                float_to_u32(f)
            }
        }
        Some(Number::Bool(b)) => u32::from(b),
        None => v.as_str().map_or(0, |s| {
            parse_integer(s).map_or(0, |i| u64::try_from(i).map_or(0, truncate_u32))
        }),
    }
}

/// Go: `cast.ToFloat64`.
#[must_use]
pub fn to_float64(v: &Value) -> f64 {
    match number(v) {
        Some(Number::Signed(i)) => int_to_float(i),
        Some(Number::Unsigned(u)) => uint_to_float(u),
        Some(Number::Float(f)) => f,
        Some(Number::Bool(b)) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        None => v
            .as_str()
            .map_or(0.0, |s| strconv::parse_float(s, 64).unwrap_or(0.0)),
    }
}

/// Go: `cast.ToDuration`: integers are nanoseconds, floats are truncated, and a string without
/// a unit letter (`n s u µ m h`) is read as nanoseconds.
#[must_use]
pub fn to_duration(v: &Value) -> Duration {
    if let Value::Duration(d) = v {
        return *d;
    }
    if let Value::String(s) = v {
        let parsed = if s.contains(['n', 's', 'u', '\u{b5}', 'm', 'h']) {
            parse_duration(s)
        } else {
            parse_duration(&format!("{s}ns"))
        };
        return parsed.unwrap_or(Duration(0));
    }
    match number(v) {
        Some(Number::Signed(_) | Number::Unsigned(_)) => Duration(to_int64(v)),
        Some(Number::Float(f)) => Duration(math::int64(f)),
        // Go: bool is no case of the switch.
        Some(Number::Bool(_)) | None => Duration(0),
    }
}

/// `cast`'s `timeFormats`: each layout, and whether the parsed wall clock is moved into the
/// default location (UTC for `cast.ToTime`): the layouts without a numeric zone offset.
const TIME_FORMATS: [(&str, bool); 24] = [
    (RFC3339, false),
    ("2006-01-02T15:04:05", true),
    (RFC1123Z, false),
    (RFC1123, true),
    (RFC822Z, false),
    (RFC822, true),
    (RFC850, true),
    ("2006-01-02 15:04:05.999999999 -0700 MST", false),
    ("2006-01-02T15:04:05-0700", false),
    ("2006-01-02 15:04:05Z0700", false),
    ("2006-01-02 15:04:05", true),
    (ANSIC, true),
    (UNIX_DATE, true),
    (RUBY_DATE, false),
    ("2006-01-02 15:04:05Z07:00", false),
    ("2006-01-02", true),
    ("02 Jan 2006", true),
    ("2006-01-02 15:04:05 -07:00", false),
    ("2006-01-02 15:04:05 -0700", false),
    (KITCHEN, false),
    (STAMP, false),
    (STAMP_MILLI, false),
    (STAMP_MICRO, false),
    (STAMP_NANO, false),
];

/// Go: `parseDateWith(s, time.UTC, timeFormats)`: the first layout that parses; the zero time
/// when none does.
fn parse_date(s: &str) -> Time {
    for (layout, in_default_location) in TIME_FORMATS {
        let Ok(d) = time::parse(layout, s) else {
            continue;
        };
        if !in_default_location {
            return d;
        }
        return Time::date(
            &DateFields {
                year: d.year(),
                month: d.month(),
                day: d.day(),
                hour: d.hour(),
                minute: d.minute(),
                second: d.second(),
                nanosecond: d.nanosecond(),
            },
            Location::Utc,
        );
    }
    Time::default()
}

/// Go: `cast.ToTime`: strings in `cast`'s layouts, and `int`, `int64`, `int32`, `uint`, `uint64`
/// and `uint32` as Unix seconds (in `Local`, which is UTC); anything else is the zero time.
#[must_use]
pub fn to_time(v: &Value) -> Time {
    match *v {
        Value::Time(ref t) => t.clone(),
        Value::String(ref s) => parse_date(s),
        Value::Int(i) | Value::Int64(i) => Time::from_unix(i, 0),
        Value::Int32(i) => Time::from_unix(i.into(), 0),
        Value::Uint(u) | Value::Uint64(u) => Time::from_unix(wrap_int(u), 0),
        Value::Uint32(u) => Time::from_unix(u.into(), 0),
        Value::Nil
        | Value::Bool(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => Time::default(),
    }
}

/// Converts each item with [`to_string`]; a nil result (Go appends to a nil slice) when there
/// are none.
fn strings_of(items: Option<&Vec<Value>>) -> Option<Vec<String>> {
    let out: Vec<String> = items.into_iter().flatten().map(to_string).collect();
    (!out.is_empty()).then_some(out)
}

/// Go: `cast.ToStringSliceE`, with `Err` for its error (whose value is a nil slice).
fn string_slice_e(v: &Value) -> Result<Option<Vec<String>>, Failed> {
    match v {
        Value::Slice(elem, items) => match **elem {
            // Go: the []string case returns the slice itself.
            Type::String => Ok(items
                .as_ref()
                .map(|items| items.iter().map(to_string).collect())),
            Type::Interface
            | Type::Int8
            | Type::Int
            | Type::Int32
            | Type::Int64
            | Type::Float32
            | Type::Float64 => Ok(strings_of(items.as_ref())),
            // Go: the interface{} case: one string, if the slice converts to one ([]byte does).
            Type::Bool
            | Type::Int16
            | Type::Uint
            | Type::Uint8
            | Type::Uint16
            | Type::Uint32
            | Type::Uint64
            | Type::Time
            | Type::Duration
            | Type::Slice(_)
            | Type::Map(..) => string_e(v).map(|s| Some(vec![s])),
        },
        Value::String(s) => Ok(Some(fields(s).into_iter().map(str::to_owned).collect())),
        Value::Nil => Err(Failed),
        Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::StringMap(..)
        | Value::Map(..) => string_e(v).map(|s| Some(vec![s])),
    }
}

/// Go: `cast.ToStringSlice`: a string is split with `strings.Fields`; `None` is a nil slice
/// (what an empty `[]interface{}` and a failed conversion give).
#[must_use]
pub fn to_string_slice(v: &Value) -> Option<Vec<String>> {
    string_slice_e(v).unwrap_or(None)
}

/// Go: `cast.ToIntSlice`: a `[]int` as it is, the items of any other slice through `ToIntE`
/// (an empty slice if one fails), and an empty slice for anything else.
#[must_use]
pub fn to_int_slice(v: &Value) -> Option<Vec<i64>> {
    let Value::Slice(elem, items) = v else {
        return Some(Vec::new());
    };
    if **elem == Type::Int {
        return items
            .as_ref()
            .map(|items| items.iter().map(to_int64).collect());
    }
    let mut out = Vec::new();
    for item in items.iter().flatten() {
        let Ok(i) = int_e(item) else {
            return Some(Vec::new());
        };
        out.push(i);
    }
    Some(out)
}

/// A JSON value decoded by `encoding/json` into an `interface{}`, as a [`Value`].
pub(crate) fn from_json(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => Value::Float64(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(items) => Value::Slice(
            &Type::Interface,
            Some(items.into_iter().map(from_json).collect()),
        ),
        serde_json::Value::Object(entries) => Value::StringMap(
            &Type::Interface,
            Some(
                entries
                    .into_iter()
                    .map(|(k, item)| (k, from_json(item)))
                    .collect(),
            ),
        ),
    }
}

/// Go: `json.Unmarshal([]byte(s), &m)` into a fresh non-nil map, the error dropped: `None` for
/// JSON `null`, and what was decoded before any error otherwise.
fn json_map<V>(s: &str) -> Option<BTreeMap<String, V>>
where
    BTreeMap<String, V>: Default,
    Option<BTreeMap<String, V>>: stdjson::StdDecode,
{
    let mut m = Some(BTreeMap::new());
    let _dropped = stdjson::unmarshal(s.as_bytes(), &mut m);
    m
}

/// The entries of a `map[interface{}]T` with each key through [`to_string`]; of keys with the
/// same text, the last one wins (Go: whichever its random iteration reaches last).
fn by_string_key<'a, T>(
    entries: Option<&'a Vec<(Value, Value)>>,
    mut convert: impl FnMut(&'a Value) -> T,
) -> BTreeMap<String, T> {
    entries
        .into_iter()
        .flatten()
        .map(|(k, v)| (to_string(k), convert(v)))
        .collect()
}

/// Go: `cast.ToStringMap`: a `map[interface{}]interface{}` with its keys through `ToString`, a
/// `map[string]interface{}` as it is, a JSON object string, and an empty map for anything else.
#[must_use]
pub fn to_string_map(v: &Value) -> Option<BTreeMap<String, Value>> {
    match v {
        Value::Map(Type::Interface, Type::Interface, entries) => {
            Some(by_string_key(entries.as_ref(), Value::clone))
        }
        Value::StringMap(Type::Interface, entries) => entries.clone(),
        Value::String(s) => json_map::<serde_json::Value>(s).map(|m| {
            m.into_iter()
                .map(|(k, item)| (k, from_json(item)))
                .collect()
        }),
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => Some(BTreeMap::new()),
    }
}

/// Go: `cast.ToStringMapString`.
#[must_use]
pub fn to_string_map_string(v: &Value) -> Option<BTreeMap<String, String>> {
    match v {
        // Go returns the map itself.
        Value::StringMap(Type::String, entries) => entries.as_ref().map(|m| {
            m.iter()
                .map(|(k, item)| (k.clone(), to_string(item)))
                .collect()
        }),
        Value::StringMap(Type::Interface, entries) => Some(
            entries
                .iter()
                .flatten()
                .map(|(k, item)| (k.clone(), to_string(item)))
                .collect(),
        ),
        Value::Map(Type::Interface, Type::String | Type::Interface, entries) => {
            Some(by_string_key(entries.as_ref(), to_string))
        }
        Value::String(s) => json_map::<String>(s),
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => Some(BTreeMap::new()),
    }
}

/// `map[string][]string`, as [`to_string_map_string_slice`] returns it.
pub type StringMapStringSlice = BTreeMap<String, Option<Vec<String>>>;

/// Go: the `map[interface{}]interface{}` case of `cast.ToStringMapStringSliceE`: an entry whose
/// value does not convert ends the conversion, keeping the entries converted before it (in
/// insertion order here; Go's iteration order is random).
fn general_string_slices(entries: Option<&Vec<(Value, Value)>>) -> StringMapStringSlice {
    let mut m = BTreeMap::new();
    for (k, v) in entries.into_iter().flatten() {
        let (Ok(key), Ok(value)) = (string_e(k), string_slice_e(v)) else {
            break;
        };
        let _previous = m.insert(key, value);
    }
    m
}

/// Go: `cast.ToStringMapStringSlice`.
#[must_use]
pub fn to_string_map_string_slice(v: &Value) -> Option<StringMapStringSlice> {
    let converted = match v {
        // Go returns the map itself.
        Value::StringMap(Type::Slice(Type::String), entries) => {
            return entries.as_ref().map(|m| {
                m.iter()
                    .map(|(k, item)| (k.clone(), to_string_slice(item)))
                    .collect()
            });
        }
        Value::StringMap(Type::Slice(Type::Interface), entries) => entries
            .iter()
            .flatten()
            .map(|(k, item)| (k.clone(), to_string_slice(item)))
            .collect(),
        Value::StringMap(Type::String, entries) => entries
            .iter()
            .flatten()
            .map(|(k, item)| (k.clone(), Some(vec![to_string(item)])))
            .collect(),
        Value::StringMap(Type::Interface, entries) => entries
            .iter()
            .flatten()
            .map(|(k, item)| {
                let value = match item {
                    Value::Slice(Type::Interface | Type::String, _) => to_string_slice(item),
                    Value::Nil
                    | Value::Bool(_)
                    | Value::Int(_)
                    | Value::Int8(_)
                    | Value::Int16(_)
                    | Value::Int32(_)
                    | Value::Int64(_)
                    | Value::Uint(_)
                    | Value::Uint8(_)
                    | Value::Uint16(_)
                    | Value::Uint32(_)
                    | Value::Uint64(_)
                    | Value::Float32(_)
                    | Value::Float64(_)
                    | Value::String(_)
                    | Value::Time(_)
                    | Value::Duration(_)
                    | Value::Slice(..)
                    | Value::StringMap(..)
                    | Value::Map(..) => Some(vec![to_string(item)]),
                };
                (k.clone(), value)
            })
            .collect(),
        // Go: map[interface{}][]string, map[interface{}]string (split with strings.Fields) and
        // map[interface{}][]interface{}.
        Value::Map(
            Type::Interface,
            Type::Slice(Type::String | Type::Interface) | Type::String,
            entries,
        ) => by_string_key(entries.as_ref(), to_string_slice),
        Value::Map(Type::Interface, Type::Interface, entries) => {
            general_string_slices(entries.as_ref())
        }
        Value::String(s) => return json_map::<Option<Vec<String>>>(s),
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Int8(_)
        | Value::Int16(_)
        | Value::Int32(_)
        | Value::Int64(_)
        | Value::Uint(_)
        | Value::Uint8(_)
        | Value::Uint16(_)
        | Value::Uint32(_)
        | Value::Uint64(_)
        | Value::Float32(_)
        | Value::Float64(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(..)
        | Value::StringMap(..)
        | Value::Map(..) => BTreeMap::new(),
    };
    Some(converted)
}

#[cfg(test)]
mod tests {
    use super::{trim_zero_decimal, truncate_i32, truncate_u32};

    #[test]
    fn trims_only_zero_fractions() -> Result<(), String> {
        for (input, want) in [
            ("8000.00", "8000"),
            ("8000.10", "8000.10"),
            ("100.", "100."),
            ("0.0", "0"),
            (".0", ""),
            (".", "."),
            ("10", "10"),
            ("1.0.0", "1.0"),
        ] {
            let got = trim_zero_decimal(input);
            if got != want {
                return Err(format!(
                    "trimZeroDecimal({input:?}) = {got:?}, want {want:?}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn truncates_like_go() -> Result<(), String> {
        if truncate_i32(3_000_000_000) != -1_294_967_296
            || truncate_u32(5_000_000_000) != 705_032_704
        {
            return Err("integer truncation differs from Go".to_owned());
        }
        Ok(())
    }
}
