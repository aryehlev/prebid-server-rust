//! The Go value model: what `fmt` sees of an argument through `reflect`, plus Go's map key
//! order (Go: `internal/fmtsort/sort.go`).

use std::cmp::Ordering;

/// The `fmt`-relevant method a Go type implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// `Error() string`: the type implements `error` (checked before `Stringer`, as in Go).
    Error,
    /// `String() string`: the type implements `fmt.Stringer`.
    Stringer,
}

/// A Go value as `fmt` prints it.
///
/// Basic variants carry Go's predeclared types (`Value::Int` is Go `int`, `Value::Int64` is
/// `int64`, ...). Composite variants carry their Go type as `reflect.Type.String()` writes it
/// (`"[]string"`, `"map[string]int"`, `"openrtb2.Imp"`, `"*openrtb2.Imp"`), which `%T`, `%#v` and
/// the `%!verb(type=value)` error forms print.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A nil interface: an untyped `nil` argument, or a nil `any`/`error` element, field or map
    /// entry. `%#v` prints a nested one as `interface {}(nil)`.
    Nil,
    /// Go `bool`.
    Bool(bool),
    /// Go `int` (64 bits).
    Int(i64),
    /// Go `int8`.
    Int8(i8),
    /// Go `int16`.
    Int16(i16),
    /// Go `int32` (also `rune`).
    Int32(i32),
    /// Go `int64`.
    Int64(i64),
    /// Go `uint` (64 bits).
    Uint(u64),
    /// Go `uint8` (also `byte`).
    Uint8(u8),
    /// Go `uint16`.
    Uint16(u16),
    /// Go `uint32`.
    Uint32(u32),
    /// Go `uint64`.
    Uint64(u64),
    /// Go `uintptr`.
    Uintptr(u64),
    /// Go `float32`.
    Float32(f32),
    /// Go `float64`.
    Float64(f64),
    /// Go `string`.
    String(String),
    /// Go `[]byte`; `None` is a nil slice. Byte strings may hold invalid UTF-8.
    Bytes(Option<Vec<u8>>),
    /// A value of a defined type over a basic type or `[]byte`, e.g. `openrtb_ext.BidderName`
    /// (a `string`) or `json.RawMessage` (a `[]byte`): printed as `value`, typed as `ty`.
    Named {
        /// The Go type, e.g. `openrtb_ext.BidderName`.
        ty: String,
        /// The underlying value.
        value: Box<Self>,
    },
    /// A slice; `None` is a nil slice. A slice whose type ends in `]uint8` prints as bytes for
    /// `%s %q %x %X`.
    Slice {
        /// The Go type, e.g. `[]string`.
        ty: String,
        /// The elements.
        elems: Option<Vec<Self>>,
    },
    /// An array.
    Array {
        /// The Go type, e.g. `[2]int`.
        ty: String,
        /// The elements.
        elems: Vec<Self>,
    },
    /// A map; `None` is a nil map. Entries print in Go's sorted key order.
    Map {
        /// The Go type, e.g. `map[string]int`.
        ty: String,
        /// The entries, in any order.
        entries: Option<Vec<(Self, Self)>>,
    },
    /// A struct; field names starting with a lowercase letter are unexported, and values reached
    /// through them never have their `Error`/`String` methods called (Go's `CanInterface`).
    Struct {
        /// The Go type, e.g. `openrtb2.Imp`.
        ty: String,
        /// `(name, value)` in declaration order; an embedded field is named after its type.
        fields: Vec<(String, Self)>,
    },
    /// A pointer; `None` is nil. Go prints a non-nil pointer that is not at the top level, or
    /// does not point to a struct, slice, array or map, as its address: `addr`.
    Pointer {
        /// The Go type, e.g. `*openrtb2.Imp`.
        ty: String,
        /// The address printed for the pointer.
        addr: u64,
        /// The pointee.
        elem: Option<Box<Self>>,
    },
    /// A value whose type has an `Error` or `String` method, and the text that method returns.
    Method {
        /// Which method.
        kind: MethodKind,
        /// What the method returns.
        text: String,
        /// The value itself, printed when the verb does not call the method (`%d`, `%#v`, or
        /// inside an unexported field).
        value: Box<Self>,
    },
}

/// The placeholder address of [`Value::error`] values.
pub const ERROR_ADDR: u64 = 0x00C0_0001_0000;

impl Value {
    /// Go: `errors.New(text)`: a `*errors.errorString` at [`ERROR_ADDR`].
    #[must_use]
    pub fn error(text: &str) -> Self {
        Self::Method {
            kind: MethodKind::Error,
            value: Box::new(Self::Pointer {
                ty: "*errors.errorString".to_owned(),
                addr: ERROR_ADDR,
                elem: Some(Box::new(Self::Struct {
                    ty: "errors.errorString".to_owned(),
                    fields: vec![("s".to_owned(), text.into())],
                })),
            }),
            text: text.to_owned(),
        }
    }

    /// Go: `reflect.TypeOf(v).String()`; `<nil>` for [`Value::Nil`].
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::Nil => "<nil>",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Int8(_) => "int8",
            Self::Int16(_) => "int16",
            Self::Int32(_) => "int32",
            Self::Int64(_) => "int64",
            Self::Uint(_) => "uint",
            Self::Uint8(_) => "uint8",
            Self::Uint16(_) => "uint16",
            Self::Uint32(_) => "uint32",
            Self::Uint64(_) => "uint64",
            Self::Uintptr(_) => "uintptr",
            Self::Float32(_) => "float32",
            Self::Float64(_) => "float64",
            Self::String(_) => "string",
            Self::Bytes(_) => "[]uint8",
            Self::Named { ty, .. }
            | Self::Slice { ty, .. }
            | Self::Array { ty, .. }
            | Self::Map { ty, .. }
            | Self::Struct { ty, .. }
            | Self::Pointer { ty, .. } => ty,
            Self::Method { value, .. } => value.type_name(),
        }
    }

    /// Go's `internal/fmtsort` order of map keys (see [`compare`]).
    pub(crate) fn sort_cmp(&self, other: &Self) -> Ordering {
        compare(self, other)
    }

    /// The value with [`Value::Named`] and [`Value::Method`] wrappers removed: what Go's
    /// `reflect.Value.Kind` dispatches on.
    pub(crate) fn underlying(&self) -> &Self {
        match self {
            Self::Named { value, .. } | Self::Method { value, .. } => value.underlying(),
            other @ (Self::Nil
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Int8(_)
            | Self::Int16(_)
            | Self::Int32(_)
            | Self::Int64(_)
            | Self::Uint(_)
            | Self::Uint8(_)
            | Self::Uint16(_)
            | Self::Uint32(_)
            | Self::Uint64(_)
            | Self::Uintptr(_)
            | Self::Float32(_)
            | Self::Float64(_)
            | Self::String(_)
            | Self::Bytes(_)
            | Self::Slice { .. }
            | Self::Array { .. }
            | Self::Map { .. }
            | Self::Struct { .. }
            | Self::Pointer { .. }) => other,
        }
    }

    /// The signed integer of an `int*` kind.
    pub(super) fn as_int(&self) -> Option<i64> {
        match *self.underlying() {
            Self::Int(v) | Self::Int64(v) => Some(v),
            Self::Int8(v) => Some(v.into()),
            Self::Int16(v) => Some(v.into()),
            Self::Int32(v) => Some(v.into()),
            Self::Nil
            | Self::Bool(_)
            | Self::Uint(_)
            | Self::Uint8(_)
            | Self::Uint16(_)
            | Self::Uint32(_)
            | Self::Uint64(_)
            | Self::Uintptr(_)
            | Self::Float32(_)
            | Self::Float64(_)
            | Self::String(_)
            | Self::Bytes(_)
            | Self::Named { .. }
            | Self::Slice { .. }
            | Self::Array { .. }
            | Self::Map { .. }
            | Self::Struct { .. }
            | Self::Pointer { .. }
            | Self::Method { .. } => None,
        }
    }

    /// The unsigned integer of a `uint*` kind.
    pub(super) fn as_uint(&self) -> Option<u64> {
        match *self.underlying() {
            Self::Uint(v) | Self::Uint64(v) | Self::Uintptr(v) => Some(v),
            Self::Uint8(v) => Some(v.into()),
            Self::Uint16(v) => Some(v.into()),
            Self::Uint32(v) => Some(v.into()),
            Self::Nil
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Int8(_)
            | Self::Int16(_)
            | Self::Int32(_)
            | Self::Int64(_)
            | Self::Float32(_)
            | Self::Float64(_)
            | Self::String(_)
            | Self::Bytes(_)
            | Self::Named { .. }
            | Self::Slice { .. }
            | Self::Array { .. }
            | Self::Map { .. }
            | Self::Struct { .. }
            | Self::Pointer { .. }
            | Self::Method { .. } => None,
        }
    }

    /// The address Go's `reflect.Value.Pointer` reports: 0 for nil, and 0 for slices and maps,
    /// which carry no address in the model.
    pub(super) fn pointer_addr(&self) -> Option<u64> {
        match self.underlying() {
            Self::Pointer { addr, elem, .. } => Some(if elem.is_some() { *addr } else { 0 }),
            Self::Slice { .. } | Self::Map { .. } | Self::Bytes(_) => Some(0),
            Self::Nil
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Int8(_)
            | Self::Int16(_)
            | Self::Int32(_)
            | Self::Int64(_)
            | Self::Uint(_)
            | Self::Uint8(_)
            | Self::Uint16(_)
            | Self::Uint32(_)
            | Self::Uint64(_)
            | Self::Uintptr(_)
            | Self::Float32(_)
            | Self::Float64(_)
            | Self::String(_)
            | Self::Named { .. }
            | Self::Array { .. }
            | Self::Struct { .. }
            | Self::Method { .. } => None,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i8> for Value {
    fn from(v: i8) -> Self {
        Self::Int8(v)
    }
}

impl From<i16> for Value {
    fn from(v: i16) -> Self {
        Self::Int16(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::Int32(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::Int64(v)
    }
}

/// Go `int`.
impl From<isize> for Value {
    fn from(v: isize) -> Self {
        Self::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl From<u8> for Value {
    fn from(v: u8) -> Self {
        Self::Uint8(v)
    }
}

impl From<u16> for Value {
    fn from(v: u16) -> Self {
        Self::Uint16(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self::Uint32(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Self::Uint64(v)
    }
}

/// Go `uint`.
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Self::Uint(u64::try_from(v).unwrap_or(u64::MAX))
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Self::Float32(v)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::String(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&String> for Value {
    fn from(v: &String) -> Self {
        Self::String(v.clone())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Self::Bytes(Some(v))
    }
}

impl From<&[u8]> for Value {
    fn from(v: &[u8]) -> Self {
        Self::Bytes(Some(v.to_vec()))
    }
}

/// Go: `cmp.Compare` on floats: NaN first and equal to itself, `-0 == +0`.
fn compare_floats(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

fn as_float(v: &Value) -> Option<f64> {
    match *v.underlying() {
        Value::Float32(f) => Some(f.into()),
        Value::Float64(f) => Some(f),
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
        | Value::Uintptr(_)
        | Value::String(_)
        | Value::Bytes(_)
        | Value::Named { .. }
        | Value::Slice { .. }
        | Value::Array { .. }
        | Value::Map { .. }
        | Value::Struct { .. }
        | Value::Pointer { .. }
        | Value::Method { .. } => None,
    }
}

fn compare_all(a: &[Value], b: &[Value]) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| compare(x, y))
        .find(|c| c.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// Go: `fmtsort.compare`, the order of map keys.
///
/// Go orders keys of different dynamic types (in a `map[any]T`) by runtime type pointers, which
/// are not stable; here they order by type name, so the order is total.
pub(super) fn compare(a: &Value, b: &Value) -> Ordering {
    match (a.underlying(), b.underlying()) {
        (Value::Nil, Value::Nil) => return Ordering::Equal,
        (Value::Nil, _) => return Ordering::Less,
        (_, Value::Nil) => return Ordering::Greater,
        _ => {}
    }
    if a.type_name() != b.type_name() {
        return a.type_name().cmp(b.type_name());
    }
    if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
        return x.cmp(&y);
    }
    if let (Some(x), Some(y)) = (a.as_uint(), b.as_uint()) {
        return x.cmp(&y);
    }
    if let (Some(x), Some(y)) = (as_float(a), as_float(b)) {
        return compare_floats(x, y);
    }
    if let (Some(x), Some(y)) = (a.pointer_addr(), b.pointer_addr()) {
        return x.cmp(&y);
    }
    match (a.underlying(), b.underlying()) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Struct { fields: x, .. }, Value::Struct { fields: y, .. }) => x
            .iter()
            .zip(y)
            .map(|((_, x), (_, y))| compare(x, y))
            .find(|c| c.is_ne())
            .unwrap_or(Ordering::Equal),
        (Value::Array { elems: x, .. }, Value::Array { elems: y, .. }) => compare_all(x, y),
        _ => Ordering::Equal,
    }
}
