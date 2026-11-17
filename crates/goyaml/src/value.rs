//! Go `interface{}` values: the dynamic values yaml.v3 produces when decoding into an empty
//! interface, and that mapstructure decodes from.
//!
//! # Map iteration order
//!
//! Go randomizes map iteration. Where Go's output depends on it, this crate iterates in a fixed
//! order instead, so any one Go run's result is reproduced only when it does not depend on the
//! order:
//!
//! - [`Value::StringMap`] iterates in byte order of its keys (`BTreeMap`).
//! - [`Value::Map`] iterates in insertion order.
//! - mapstructure's case-insensitive key fallback (`strings.EqualFold` over `dataVal.MapKeys()`)
//!   picks the first match in that order; Go picks a random one when several keys fold to the field
//!   name.
//! - mapstructure's aggregated errors are sorted, as in Go, so their order never depends on it.
//! - JSON encoding sorts object keys, as both Go encoders do. jsoniter sorts the keys of a
//!   `map[interface {}]interface {}` by their text, so keys of different types with the same text
//!   (`1` and `"1"`) come out in Go's random order; here they keep the map's order.

use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};

use pbs_gocompat::{
    fmt as gofmt,
    time::{Duration, Location, Time},
};
use pbs_gojson::{
    Encoder, Flavor, GoEncode, GoKind, GoMapKey, GoType, encode::encode_map, jsoniter::Iter,
};

/// A Go type of the values this crate models (`reflect.Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// `bool`
    Bool,
    /// `int`
    Int,
    /// `int8`
    Int8,
    /// `int16`
    Int16,
    /// `int32`
    Int32,
    /// `int64`
    Int64,
    /// `uint`
    Uint,
    /// `uint8`
    Uint8,
    /// `uint16`
    Uint16,
    /// `uint32`
    Uint32,
    /// `uint64`
    Uint64,
    /// `float32`
    Float32,
    /// `float64`
    Float64,
    /// `string`
    String,
    /// `interface {}`
    Interface,
    /// `time.Time`
    Time,
    /// `time.Duration`
    Duration,
    /// `[]T`
    Slice(&'static Self),
    /// `map[K]V`
    Map(&'static Self, &'static Self),
}

/// `[]interface {}`.
pub const SLICE_OF_INTERFACE: Type = Type::Slice(&Type::Interface);
/// `map[string]interface {}`.
pub const STRING_MAP: Type = Type::Map(&Type::String, &Type::Interface);
/// `map[interface {}]interface {}`.
pub const GENERAL_MAP: Type = Type::Map(&Type::Interface, &Type::Interface);

impl Type {
    /// Go's `reflect.Kind` of the type.
    #[must_use]
    pub const fn kind(self) -> GoKind {
        match self {
            Self::Bool => GoKind::Bool,
            Self::Int => GoKind::Int,
            Self::Int8 => GoKind::Int8,
            Self::Int16 => GoKind::Int16,
            Self::Int32 => GoKind::Int32,
            Self::Int64 | Self::Duration => GoKind::Int64,
            Self::Uint => GoKind::Uint,
            Self::Uint8 => GoKind::Uint8,
            Self::Uint16 => GoKind::Uint16,
            Self::Uint32 => GoKind::Uint32,
            Self::Uint64 => GoKind::Uint64,
            Self::Float32 => GoKind::Float32,
            Self::Float64 => GoKind::Float64,
            Self::String => GoKind::String,
            Self::Interface => GoKind::Interface,
            Self::Time => GoKind::Struct,
            Self::Slice(_) => GoKind::Slice,
            Self::Map(..) => GoKind::Map,
        }
    }

    /// Appends Go's `reflect.Type.String()`, such as `map[string]interface {}`.
    pub fn write_name(self, out: &mut String) {
        match self {
            Self::Slice(elem) => {
                out.push_str("[]");
                elem.write_name(out);
            }
            Self::Map(key, elem) => {
                out.push_str("map[");
                key.write_name(out);
                out.push(']');
                elem.write_name(out);
            }
            Self::Bool
            | Self::Int
            | Self::Int8
            | Self::Int16
            | Self::Int32
            | Self::Int64
            | Self::Uint
            | Self::Uint8
            | Self::Uint16
            | Self::Uint32
            | Self::Uint64
            | Self::Float32
            | Self::Float64
            | Self::String
            | Self::Interface
            | Self::Time
            | Self::Duration => out.push_str(self.basic_name()),
        }
    }

    /// The name of a non-composite type.
    const fn basic_name(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Int8 => "int8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Int64 => "int64",
            Self::Uint => "uint",
            Self::Uint8 => "uint8",
            Self::Uint16 => "uint16",
            Self::Uint32 => "uint32",
            Self::Uint64 => "uint64",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::String => "string",
            Self::Interface => "interface {}",
            Self::Time => "time.Time",
            Self::Duration => "time.Duration",
            Self::Slice(_) | Self::Map(..) => "",
        }
    }

    /// Go's `reflect.Type.String()`.
    #[must_use]
    pub fn name(self) -> String {
        let mut out = String::new();
        self.write_name(&mut out);
        out
    }
}

/// Go: `reflect.Kind.String()`.
#[must_use]
pub const fn kind_name(kind: GoKind) -> &'static str {
    match kind {
        GoKind::Bool => "bool",
        GoKind::Int => "int",
        GoKind::Int8 => "int8",
        GoKind::Int16 => "int16",
        GoKind::Int32 => "int32",
        GoKind::Int64 => "int64",
        GoKind::Uint => "uint",
        GoKind::Uint8 => "uint8",
        GoKind::Uint16 => "uint16",
        GoKind::Uint32 => "uint32",
        GoKind::Uint64 => "uint64",
        GoKind::Float32 => "float32",
        GoKind::Float64 => "float64",
        GoKind::String => "string",
        GoKind::Interface => "interface",
        GoKind::Map => "map",
        GoKind::Ptr => "ptr",
        GoKind::Slice => "slice",
        GoKind::Struct => "struct",
    }
}

/// A Go `interface{}` value: `nil`, or a dynamic value with its type.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    /// A nil interface.
    #[default]
    Nil,
    /// Go `bool`.
    Bool(bool),
    /// Go `int` (64 bits), what yaml.v3 decodes integers to.
    Int(i64),
    /// Go `int8`.
    Int8(i8),
    /// Go `int16`.
    Int16(i16),
    /// Go `int32`.
    Int32(i32),
    /// Go `int64`.
    Int64(i64),
    /// Go `uint` (64 bits).
    Uint(u64),
    /// Go `uint8`.
    Uint8(u8),
    /// Go `uint16`.
    Uint16(u16),
    /// Go `uint32`.
    Uint32(u32),
    /// Go `uint64`, what yaml.v3 decodes integers above `math.MaxInt64` to.
    Uint64(u64),
    /// Go `float32`.
    Float32(f32),
    /// Go `float64`.
    Float64(f64),
    /// Go `string`.
    String(String),
    /// Go `time.Time`, what yaml.v3 decodes unquoted timestamps to.
    Time(Time),
    /// Go `time.Duration`.
    Duration(Duration),
    /// A slice of the element type, whose items are values of that type; `None` is a nil slice.
    Slice(&'static Type, Option<Vec<Self>>),
    /// A `map[string]T` of the element type; `None` is a nil map.
    StringMap(&'static Type, Option<BTreeMap<String, Self>>),
    /// A map with another key type (such as `map[interface {}]interface {}`), in insertion
    /// order; `None` is a nil map. Keys are unique under Go's `==`.
    Map(&'static Type, &'static Type, Option<Vec<(Self, Self)>>),
}

impl Value {
    /// Go: `reflect.TypeOf(v)`; `None` for a nil interface.
    #[must_use]
    pub const fn type_of(&self) -> Option<Type> {
        Some(match self {
            Self::Nil => return None,
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Int8(_) => Type::Int8,
            Self::Int16(_) => Type::Int16,
            Self::Int32(_) => Type::Int32,
            Self::Int64(_) => Type::Int64,
            Self::Uint(_) => Type::Uint,
            Self::Uint8(_) => Type::Uint8,
            Self::Uint16(_) => Type::Uint16,
            Self::Uint32(_) => Type::Uint32,
            Self::Uint64(_) => Type::Uint64,
            Self::Float32(_) => Type::Float32,
            Self::Float64(_) => Type::Float64,
            Self::String(_) => Type::String,
            Self::Time(_) => Type::Time,
            Self::Duration(_) => Type::Duration,
            Self::Slice(elem, _) => Type::Slice(elem),
            Self::StringMap(elem, _) => Type::Map(&Type::String, elem),
            Self::Map(key, elem, _) => Type::Map(key, elem),
        })
    }

    /// Go's `reflect.Kind` of the dynamic value; `None` for a nil interface.
    #[must_use]
    pub fn kind(&self) -> Option<GoKind> {
        self.type_of().map(Type::kind)
    }

    /// The dynamic type's name (`reflect.Type.String()`), or `<nil>` for a nil interface.
    #[must_use]
    pub fn type_name(&self) -> String {
        self.type_of()
            .map_or_else(|| "<nil>".to_owned(), Type::name)
    }

    /// A Go `[]byte` holding `data`. A `json.RawMessage`, which a decode hook may produce for a
    /// target of that type, is decoded from it as Go decodes one (its type prints as `[]uint8`,
    /// and it encodes to JSON as base64, like any `[]byte`).
    #[must_use]
    pub fn bytes(data: &[u8]) -> Self {
        Self::Slice(
            &Type::Uint8,
            Some(data.iter().copied().map(Self::Uint8).collect()),
        )
    }

    /// An empty `[]interface {}`.
    #[must_use]
    pub const fn new_slice() -> Self {
        Self::Slice(&Type::Interface, Some(Vec::new()))
    }

    /// An empty `map[string]interface {}`.
    #[must_use]
    pub const fn new_string_map() -> Self {
        Self::StringMap(&Type::Interface, Some(BTreeMap::new()))
    }

    /// Whether the value is a nil interface.
    #[must_use]
    pub const fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }

    /// The string, if the value is a Go `string`.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(s) = self {
            Some(s)
        } else {
            None
        }
    }

    /// The entries of a `map[string]T` (a nil map has none).
    #[must_use]
    pub const fn as_string_map(&self) -> Option<&BTreeMap<String, Self>> {
        if let Self::StringMap(_, Some(m)) = self {
            Some(m)
        } else {
            None
        }
    }

    /// The items of a slice (a nil slice has none).
    #[must_use]
    pub fn as_slice(&self) -> Option<&[Self]> {
        if let Self::Slice(_, Some(items)) = self {
            Some(items)
        } else {
            None
        }
    }

    /// Go's `==` on two interface values: the same dynamic type and equal values (NaN is
    /// never equal, `+0 == -0`). Slices and maps, which Go cannot compare, are never equal.
    #[must_use]
    pub fn go_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) | (Self::Int64(a), Self::Int64(b)) => a == b,
            (Self::Int8(a), Self::Int8(b)) => a == b,
            (Self::Int16(a), Self::Int16(b)) => a == b,
            (Self::Int32(a), Self::Int32(b)) => a == b,
            (Self::Uint(a), Self::Uint(b)) | (Self::Uint64(a), Self::Uint64(b)) => a == b,
            (Self::Uint8(a), Self::Uint8(b)) => a == b,
            (Self::Uint16(a), Self::Uint16(b)) => a == b,
            (Self::Uint32(a), Self::Uint32(b)) => a == b,
            (Self::Float32(a), Self::Float32(b)) => a.partial_cmp(b).is_some_and(Ordering::is_eq),
            (Self::Float64(a), Self::Float64(b)) => a.partial_cmp(b).is_some_and(Ordering::is_eq),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Time(a), Self::Time(b)) => a == b,
            (Self::Duration(a), Self::Duration(b)) => a == b,
            _ => false,
        }
    }

    /// Go's `==` on two map keys yaml.v3 decoded one after the other: [`Value::go_eq`], except
    /// that two `time.Time`s also need the same `*time.Location` pointer. Each `time.Parse` of
    /// an offset takes its location from `time.FixedZone`, which allocates a new one, but for
    /// the unnamed whole-hour offsets from -12h to +14h, which it caches; UTC is nil.
    pub(crate) fn decoded_key_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Time(a), Self::Time(b)) => a == b && shared_location(a.location()),
            _ => self.go_eq(other),
        }
    }

    /// Inserts a decoded `(key, value)` into a general map's entries, replacing the entry of a
    /// key that is `==` (Go: `SetMapIndex`).
    pub(crate) fn general_map_insert(entries: &mut Vec<(Self, Self)>, key: Self, value: Self) {
        if let Some(entry) = entries.iter_mut().find(|(k, _)| k.decoded_key_eq(&key)) {
            *entry = (key, value);
        } else {
            entries.push((key, value));
        }
    }

    /// Whether a general map's entries hold a decoded `key` (Go: `out.MapIndex(k).IsValid()`).
    pub(crate) fn general_map_contains(entries: &[(Self, Self)], key: &Self) -> bool {
        entries.iter().any(|(k, _)| k.decoded_key_eq(key))
    }

    /// The value as Go's `fmt` sees it, for `%v` (`%#v` would print a `time.Time` without its
    /// `GoString` method: [`Value::go_syntax`] does).
    #[must_use]
    pub fn to_fmt(&self) -> gofmt::Value {
        self.fmt_with(&mut time_fmt)
    }

    /// Go: `fmt.Sprintf("%#v", v)`.
    ///
    /// `fmt` prints a `time.Time` with its `GoString` method, which [`gofmt`] does not model.
    /// Each time is printed as a placeholder struct that sorts as Go sorts `time.Time` map keys
    /// (by their fields: the nanoseconds, the seconds, then the location pointer, nil first),
    /// and the placeholders are then replaced with `GoString`'s text. Their field names hold a
    /// NUL, which `%#v` escapes in strings, so no other text matches one.
    #[must_use]
    pub fn go_syntax(&self) -> String {
        let mut times = Vec::new();
        let tree = self.fmt_with(&mut |t: &Time| {
            let index = i64::try_from(times.len()).unwrap_or(i64::MAX);
            times.push(go_string(t));
            let field = |value| (PLACEHOLDER_FIELD.to_owned(), value);
            gofmt::Value::Struct {
                ty: "time.Time".to_owned(),
                fields: vec![
                    field(gofmt::Value::Int64(t.nanosecond())),
                    field(gofmt::Value::Int64(t.unix())),
                    field(gofmt::Value::Bool(*t.location() != Location::Utc)),
                    field(gofmt::Value::Int64(index)),
                ],
            }
        });
        let text = gofmt::sprintf("%#v", &[tree]);
        if times.is_empty() {
            return text;
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some((before, after)) = rest.split_once(PLACEHOLDER) {
            out.push_str(before);
            let Some((fields, tail)) = after.split_once('}') else {
                out.push_str(PLACEHOLDER);
                rest = after;
                break;
            };
            let shown = fields
                .rsplit_once(PLACEHOLDER_FIELD)
                .and_then(|(_, index)| index.strip_prefix(':')?.parse::<usize>().ok())
                .and_then(|index| times.get(index));
            if let Some(shown) = shown {
                out.push_str(shown);
            } else {
                out.push_str(PLACEHOLDER);
                out.push_str(fields);
                out.push('}');
            }
            rest = tail;
        }
        out.push_str(rest);
        out
    }

    /// The value as Go's `fmt` sees it, with `time` making the `fmt` value of a `time.Time`.
    fn fmt_with<F: FnMut(&Time) -> gofmt::Value>(&self, time: &mut F) -> gofmt::Value {
        match self {
            Self::Nil => gofmt::Value::Nil,
            Self::Bool(v) => gofmt::Value::Bool(*v),
            Self::Int(v) => gofmt::Value::Int(*v),
            Self::Int8(v) => gofmt::Value::Int8(*v),
            Self::Int16(v) => gofmt::Value::Int16(*v),
            Self::Int32(v) => gofmt::Value::Int32(*v),
            Self::Int64(v) => gofmt::Value::Int64(*v),
            Self::Uint(v) => gofmt::Value::Uint(*v),
            Self::Uint8(v) => gofmt::Value::Uint8(*v),
            Self::Uint16(v) => gofmt::Value::Uint16(*v),
            Self::Uint32(v) => gofmt::Value::Uint32(*v),
            Self::Uint64(v) => gofmt::Value::Uint64(*v),
            Self::Float32(v) => gofmt::Value::Float32(*v),
            Self::Float64(v) => gofmt::Value::Float64(*v),
            Self::String(v) => gofmt::Value::String(v.clone()),
            Self::Time(t) => time(t),
            Self::Duration(d) => gofmt::Value::Method {
                kind: gofmt::MethodKind::Stringer,
                text: d.string(),
                value: Box::new(gofmt::Value::Named {
                    ty: "time.Duration".to_owned(),
                    value: Box::new(gofmt::Value::Int64(d.0)),
                }),
            },
            Self::Slice(elem, items) => gofmt::Value::Slice {
                ty: Type::Slice(elem).name(),
                elems: items
                    .as_ref()
                    .map(|items| items.iter().map(|item| item.fmt_with(time)).collect()),
            },
            Self::StringMap(elem, entries) => gofmt::Value::Map {
                ty: Type::Map(&Type::String, elem).name(),
                entries: entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, v)| (gofmt::Value::String(k.clone()), v.fmt_with(time)))
                        .collect()
                }),
            },
            Self::Map(key, elem, entries) => gofmt::Value::Map {
                ty: Type::Map(key, elem).name(),
                entries: entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.fmt_with(time), v.fmt_with(time)))
                        .collect()
                }),
            },
        }
    }
}

/// A `time.Time` as `fmt` sees it: a struct with a `String` method.
fn time_fmt(t: &Time) -> gofmt::Value {
    gofmt::Value::Method {
        kind: gofmt::MethodKind::Stringer,
        text: t.to_string(),
        value: Box::new(gofmt::Value::Struct {
            ty: "time.Time".to_owned(),
            fields: Vec::new(),
        }),
    }
}

/// What [`Value::go_syntax`] replaces: the start of a placeholder `time.Time`.
const PLACEHOLDER: &str = "time.Time{\u{0}:";

/// The name of each field of a placeholder `time.Time`.
const PLACEHOLDER_FIELD: &str = "\u{0}";

/// Go: `Time.GoString`, where `Local` is `UTC`.
fn go_string(t: &Time) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let month = usize::try_from(t.month())
        .ok()
        .and_then(|m| MONTHS.get(m.checked_sub(1)?))
        .map_or_else(
            || format!("time.Month({})", t.month()),
            |name| format!("time.{name}"),
        );
    let loc = match t.location() {
        Location::Utc => "time.UTC".to_owned(),
        Location::Fixed { name, .. } => format!("time.Location({})", go_quoted(name)),
    };
    format!(
        "time.Date({}, {month}, {}, {}, {}, {}, {}, {loc})",
        t.year(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond()
    )
}

/// Go: `time.quoted`: `s` between double quotes, `"` and `\` escaped with a backslash, and the
/// bytes of control and non-ASCII characters as `\x` escapes.
fn go_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c.is_ascii() && c >= ' ' {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        } else {
            let mut utf8 = [0; 4];
            for b in c.encode_utf8(&mut utf8).bytes() {
                out.push_str("\\x");
                out.extend(
                    [b >> 4, b & 0xF].map(|n| char::from_digit(u32::from(n), 16).unwrap_or('0')),
                );
            }
        }
    }
    out.push('"');
    out
}

/// Whether all `time.Time`s in `loc` share one `*time.Location` pointer: UTC (nil), and the
/// unnamed whole-hour zones from -12h to +14h that `time.FixedZone` caches (since Go 1.21).
fn shared_location(loc: &Location) -> bool {
    match loc {
        Location::Utc => true,
        Location::Fixed { name, offset } => {
            name.is_empty() && (-12..=14).any(|hour: i64| hour * 3600 == *offset)
        }
    }
}

impl GoType for Value {
    const KIND: GoKind = GoKind::Interface;

    fn go_type_name(out: &mut String) {
        out.push_str("interface {}");
    }
}

/// JSON encoding of an `interface{}` holding the value (`jsonutil.Marshal`, jsoniter, and
/// `json.Marshal`): the encoder of its dynamic type, so `time.Time` encodes with `MarshalJSON`,
/// `[]byte` as base64, and a map's keys as Go writes keys of their type.
///
/// jsoniter encodes a `map[interface {}]interface {}` (`encoding/json` reports it as an
/// unsupported type); a nil key, on which jsoniter panics (a nil pointer dereference), is
/// reported as an error with the panic's text. jsoniter reaches every value whatever it failed
/// on before, and the panic ends the encoding, so a tree with such a key anywhere encodes to that
/// error, whatever else fails in it. Inside a value of another Go type, the panic also wins over
/// an error recorded before it, but for one recorded outside a map that the tree is an element
/// of (`pbs_gojson` encodes map elements into a sub-stream, which keeps the first error).
impl GoEncode for Value {
    fn go_encode(&self, enc: &mut Encoder) {
        if enc.is_jsoniter() {
            if panicked(enc) {
                return;
            }
            // Scan the tree where an encoding starts, and where an error could hide the panic.
            if (enc.is_empty() || enc.has_error()) && has_nil_key(self) {
                enc.set_error(NIL_KEY_PANIC.as_bytes().to_vec());
                return;
            }
        }
        match self {
            Self::Nil | Self::Slice(_, None) | Self::StringMap(_, None) => enc.write_null(),
            Self::Bool(v) => v.go_encode(enc),
            Self::Int(v) | Self::Int64(v) => v.go_encode(enc),
            Self::Int8(v) => v.go_encode(enc),
            Self::Int16(v) => v.go_encode(enc),
            Self::Int32(v) => v.go_encode(enc),
            Self::Uint(v) | Self::Uint64(v) => v.go_encode(enc),
            Self::Uint8(v) => v.go_encode(enc),
            Self::Uint16(v) => v.go_encode(enc),
            Self::Uint32(v) => v.go_encode(enc),
            Self::Float32(v) => v.go_encode(enc),
            Self::Float64(v) => v.go_encode(enc),
            Self::String(v) => v.go_encode(enc),
            Self::Time(t) => encode_time(t, enc),
            Self::Duration(d) => d.0.go_encode(enc),
            Self::Slice(elem, Some(items)) => encode_items(elem, items, enc),
            Self::StringMap(_, Some(entries)) => encode_map(entries, enc),
            Self::Map(key, elem, entries) => encode_general_map(key, elem, entries.as_deref(), enc),
        }
    }

    fn go_is_empty(&self) -> bool {
        // omitempty: only a nil interface is empty.
        self.is_nil()
    }
}

/// The message of the runtime panic of jsoniter's `dynamicMapKeyEncoder` on a nil key.
const NIL_KEY_PANIC: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Whether jsoniter's nil-key panic has ended the encoding.
fn panicked(enc: &Encoder) -> bool {
    enc.is_jsoniter() && enc.error_text() == Some(NIL_KEY_PANIC.as_bytes())
}

/// Whether `v` holds a map with a key jsoniter's `dynamicMapKeyEncoder` panics on: nil (slices
/// and maps cannot be Go map keys, and fail the same way here).
fn has_nil_key(v: &Value) -> bool {
    match v {
        Value::Slice(_, Some(items)) => items.iter().any(has_nil_key),
        Value::StringMap(_, Some(entries)) => entries.values().any(has_nil_key),
        Value::Map(_, _, Some(entries)) => entries.iter().any(|(key, elem)| {
            matches!(
                key,
                Value::Nil | Value::Slice(..) | Value::StringMap(..) | Value::Map(..)
            ) || has_nil_key(elem)
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
        | Value::String(_)
        | Value::Time(_)
        | Value::Duration(_)
        | Value::Slice(_, None)
        | Value::StringMap(_, None)
        | Value::Map(_, _, None) => false,
    }
}

/// Go: `Time.MarshalJSON`, through jsoniter's `marshalerEncoder` or `encoding/json`'s
/// `marshalerEncoder`.
fn encode_time(t: &Time, enc: &mut Encoder) {
    match t.marshal_json() {
        Ok(text) => enc.write_raw(text.as_bytes()),
        Err(e) if enc.is_jsoniter() => enc.set_error(e.to_string().into_bytes()),
        Err(e) => enc.set_error(
            format!("json: error calling MarshalJSON for type time.Time: {e}").into_bytes(),
        ),
    }
}

/// A non-nil slice of `elem`.
fn encode_items(elem: &'static Type, items: &[Value], enc: &mut Encoder) {
    if *elem == Type::Uint8 {
        // Go: []byte encodes as a base64 string in both libraries.
        let bytes: Vec<u8> = items
            .iter()
            .filter_map(|item| {
                if let Value::Uint8(b) = item {
                    Some(*b)
                } else {
                    None
                }
            })
            .collect();
        <u8 as GoEncode>::go_encode_slice(&bytes, enc);
        return;
    }
    // Go: jsoniter reflect_slice.go sliceEncoder.Encode / encoding/json arrayEncoder
    let Some((first, rest)) = items.split_first() else {
        enc.write_raw(b"[]");
        return;
    };
    enc.write_byte(b'[');
    first.go_encode(enc);
    for item in rest {
        enc.write_byte(b',');
        item.go_encode(enc);
    }
    enc.write_byte(b']');
    if enc.has_error() && !panicked(enc) {
        // jsoniter wraps any error with the slice type (`encoding/json` does not).
        enc.wrap_error(&Type::Slice(elem).name(), ": ");
    }
}

/// A map whose keys are not of type `string`.
fn encode_general_map(
    key: &'static Type,
    elem: &'static Type,
    entries: Option<&[(Value, Value)]>,
    enc: &mut Encoder,
) {
    if !enc.is_jsoniter() && !std_map_key(*key) {
        // Go: encoding/json newMapEncoder picks unsupportedTypeEncoder (even for a nil map).
        let name = Type::Map(key, elem).name();
        enc.set_error(format!("json: unsupported type: {name}").into_bytes());
        return;
    }
    let Some(entries) = entries else {
        enc.write_null();
        return;
    };
    let keys: Vec<MapKey<'_>> = entries.iter().map(|(k, _)| MapKey(k)).collect();
    encode_map(keys.iter().zip(entries.iter().map(|(_, v)| v)), enc);
}

/// Whether `encoding/json` encodes maps with keys of type `key`: strings, integers, and
/// `encoding.TextMarshaler`s.
const fn std_map_key(key: Type) -> bool {
    match key {
        Type::String
        | Type::Int
        | Type::Int8
        | Type::Int16
        | Type::Int32
        | Type::Int64
        | Type::Uint
        | Type::Uint8
        | Type::Uint16
        | Type::Uint32
        | Type::Uint64
        | Type::Duration
        | Type::Time => true,
        Type::Bool
        | Type::Float32
        | Type::Float64
        | Type::Interface
        | Type::Slice(_)
        | Type::Map(..) => false,
    }
}

/// A map key as Go's encoders write it, by its dynamic type (jsoniter's `encoderOfMapKey`,
/// `encoding/json`'s `resolveKeyName`).
struct MapKey<'a>(&'a Value);

/// The key [`MapKey`]'s decoding methods return; Go never decodes into such a map.
static NIL_KEY: Value = Value::Nil;

impl GoType for MapKey<'_> {
    const KIND: GoKind = GoKind::Interface;

    fn go_type_name(out: &mut String) {
        out.push_str("interface {}");
    }
}

impl GoMapKey for MapKey<'_> {
    fn key_text(&self) -> Cow<'_, [u8]> {
        // The key as the encoders sort it: jsoniter sorts by the encoded key read back as a
        // string, `encoding/json` by the resolved key name.
        match self.0 {
            Value::String(s) => Cow::Borrowed(s.as_bytes()),
            Value::Time(t) => Cow::Owned(t.marshal_text().unwrap_or_default().into_bytes()),
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
            | Value::Duration(_) => {
                let mut text = Encoder::new(Flavor::Jsoniter);
                self.0.go_encode(&mut text);
                Cow::Owned(text.as_bytes().to_vec())
            }
            Value::Nil | Value::Slice(..) | Value::StringMap(..) | Value::Map(..) => {
                Cow::Borrowed(b"")
            }
        }
    }

    fn write_jsoniter_key(&self, enc: &mut Encoder) {
        if panicked(enc) {
            return;
        }
        match self.0 {
            // Go: the HTML-escaping string encoder, and textMarshalerEncoder for time.Time.
            Value::String(s) => enc.write_str(s.as_bytes()),
            Value::Time(t) => match t.marshal_text() {
                Ok(text) => enc.write_str(text.as_bytes()),
                Err(e) => enc.set_error(e.to_string().into_bytes()),
            },
            // Go: numericMapKeyEncoder writes the number (or bool) between quotes.
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
            | Value::Duration(_) => {
                enc.write_byte(b'"');
                self.0.go_encode(enc);
                enc.write_byte(b'"');
            }
            // A nil key makes dynamicMapKeyEncoder panic; slices and maps cannot be Go map keys.
            Value::Nil | Value::Slice(..) | Value::StringMap(..) | Value::Map(..) => {
                enc.set_error(NIL_KEY_PANIC.as_bytes().to_vec());
            }
        }
    }

    fn std_parse_key(key: &[u8]) -> Option<Self> {
        let _ = key;
        None
    }

    fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
        let _ = it;
        MapKey(&NIL_KEY)
    }
}
