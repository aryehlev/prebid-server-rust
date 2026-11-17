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

use std::{cmp::Ordering, collections::BTreeMap};

use pbs_gocompat::{
    fmt as gofmt,
    time::{Duration, Time},
};
use pbs_gojson::{GoKind, GoType};

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

    /// Inserts `(key, value)` into a general map's entries, replacing the entry of a key that
    /// is `==` (Go: `SetMapIndex`).
    pub(crate) fn general_map_insert(entries: &mut Vec<(Self, Self)>, key: Self, value: Self) {
        if let Some(entry) = entries.iter_mut().find(|(k, _)| k.go_eq(&key)) {
            *entry = (key, value);
        } else {
            entries.push((key, value));
        }
    }

    /// Whether a general map's entries hold `key` (Go: `out.MapIndex(k).IsValid()`).
    pub(crate) fn general_map_contains(entries: &[(Self, Self)], key: &Self) -> bool {
        entries.iter().any(|(k, _)| k.go_eq(key))
    }

    /// The value as Go's `fmt` sees it, for `%v` and `%#v`.
    #[must_use]
    pub fn to_fmt(&self) -> gofmt::Value {
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
            Self::Time(t) => time_fmt(t),
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
                    .map(|items| items.iter().map(Self::to_fmt).collect()),
            },
            Self::StringMap(elem, entries) => gofmt::Value::Map {
                ty: Type::Map(&Type::String, elem).name(),
                entries: entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, v)| (gofmt::Value::String(k.clone()), v.to_fmt()))
                        .collect()
                }),
            },
            Self::Map(key, elem, entries) => gofmt::Value::Map {
                ty: Type::Map(key, elem).name(),
                entries: entries
                    .as_ref()
                    .map(|m| m.iter().map(|(k, v)| (k.to_fmt(), v.to_fmt())).collect()),
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

impl GoType for Value {
    const KIND: GoKind = GoKind::Interface;

    fn go_type_name(out: &mut String) {
        out.push_str("interface {}");
    }
}
