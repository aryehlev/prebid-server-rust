//! Go type identity: the `reflect.Kind` and `reflect.Type.String()` that Go's JSON libraries
//! consult (in `,string` handling and in error messages).
//!
//! The Rust ↔ Go type mapping used throughout the crate:
//!
//! | Rust | Go |
//! |---|---|
//! | `bool` | `bool` |
//! | `i8` `i16` `i32` `i64` | `int8` `int16` `int32` `int64` |
//! | `isize` | `int` (64-bit) |
//! | `u8` `u16` `u32` `u64` | `uint8` `uint16` `uint32` `uint64` |
//! | `usize` | `uint` (64-bit) |
//! | `f32` `f64` | `float32` `float64` |
//! | `String`, `str`, [`GoString`](crate::GoString) | `string` |
//! | `Option<T>` | `*T` (but see below for slices and maps) |
//! | `Box<T>` | `T` (storage only) |
//! | `Vec<T>` | `[]T`, never nil (empty encodes as `[]`) |
//! | `Option<Vec<T>>` | `[]T` that may be nil (`None` encodes as `null`) |
//! | `BTreeMap<K, V>`, `HashMap<K, V>` | `map[K]V`, never nil |
//! | `Option<BTreeMap<K, V>>` | `map[K]V` that may be nil |
//! | [`RawJson`](crate::RawJson) | `json.RawMessage` |
//! | `serde_json::Value` | `interface{}` |
//! | `serde_json::Map<String, Value>` | `map[string]interface{}` |

use std::{
    collections::{BTreeMap, HashMap},
    hash::BuildHasher,
};

use crate::value::drop_value;

/// Go's `reflect.Kind`, restricted to the kinds this crate models.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoKind {
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
    /// `interface{}`
    Interface,
    /// `map[K]V`
    Map,
    /// `*T`
    Ptr,
    /// `[]T`
    Slice,
    /// `struct`
    Struct,
}

impl GoKind {
    /// Whether `encoding/json` honours `,string` for a field of this kind.
    #[must_use]
    pub const fn std_quotable(self) -> bool {
        // Go: encoding/json/encode.go typeFields (quoted)
        matches!(
            self,
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
        )
    }

    /// Whether a Go value of this kind can be nil (`null` sets it to its zero value).
    #[must_use]
    pub const fn nilable(self) -> bool {
        matches!(self, Self::Interface | Self::Ptr | Self::Map | Self::Slice)
    }
}

/// The Go type a Rust type stands for.
///
/// Implemented for the primitive mappings in the module table, and by
/// [`#[derive(GoJson)]`](crate::GoJson) for structs and newtypes.
pub trait GoType {
    /// Go's `reflect.Kind` of the type.
    const KIND: GoKind;
    /// The kind `encoding/json` sees for `,string` purposes: the element kind of an unnamed
    /// pointer, otherwise [`GoType::KIND`].
    const DEREF_KIND: GoKind = Self::KIND;
    /// Go's `reflect.Kind` of `Option<Self>`: `Ptr`, except for slices and maps, where `Option`
    /// models a nil-able value of the same kind.
    #[doc(hidden)]
    const OPTION_KIND: GoKind = GoKind::Ptr;

    /// Appends Go's `reflect.Type.String()` for the type, e.g. `[]*openrtb2.Imp`.
    fn go_type_name(out: &mut String);

    /// Appends the Go type name of `Option<Self>` (`*T` by default).
    #[doc(hidden)]
    fn option_go_type_name(out: &mut String) {
        out.push('*');
        Self::go_type_name(out);
    }

    /// Drops a value the decoders discard (an element replaced by a duplicate key, a slice or
    /// map set to nil, the elements a shorter array leaves over) without recursing through a
    /// deeply nested `serde_json::Value` in it, which [`crate::value::drop_value`] drops with
    /// heap memory instead. Containers of `Value`s forward it to their elements; everything
    /// else, structs included, is dropped as usual.
    #[doc(hidden)]
    fn go_drop(self)
    where
        Self: Sized,
    {
        drop(self);
    }
}

/// Drops the value in `slot`, if any, with [`GoType::go_drop`], leaving `None`.
pub(crate) fn go_clear<T: GoType>(slot: &mut Option<T>) {
    if let Some(old) = slot.take() {
        old.go_drop();
    }
}

/// Go's `reflect.Type.String()` for `T`.
#[must_use]
pub fn go_type_name<T: GoType + ?Sized>() -> String {
    let mut out = String::new();
    T::go_type_name(&mut out);
    out
}

macro_rules! primitive_go_type {
    ($($ty:ty => $kind:ident, $name:literal;)*) => {
        $(
            impl GoType for $ty {
                const KIND: GoKind = GoKind::$kind;
                fn go_type_name(out: &mut String) {
                    out.push_str($name);
                }
            }
        )*
    };
}

primitive_go_type! {
    bool => Bool, "bool";
    i8 => Int8, "int8";
    i16 => Int16, "int16";
    i32 => Int32, "int32";
    i64 => Int64, "int64";
    isize => Int, "int";
    u8 => Uint8, "uint8";
    u16 => Uint16, "uint16";
    u32 => Uint32, "uint32";
    u64 => Uint64, "uint64";
    usize => Uint, "uint";
    f32 => Float32, "float32";
    f64 => Float64, "float64";
    String => String, "string";
    str => String, "string";
}

impl GoType for serde_json::Value {
    const KIND: GoKind = GoKind::Interface;

    fn go_type_name(out: &mut String) {
        out.push_str("interface {}");
    }

    fn go_drop(self) {
        drop_value(self);
    }
}

impl<T: GoType> GoType for Option<T> {
    const DEREF_KIND: GoKind = match T::OPTION_KIND {
        GoKind::Ptr => T::KIND,
        GoKind::Bool
        | GoKind::Int
        | GoKind::Int8
        | GoKind::Int16
        | GoKind::Int32
        | GoKind::Int64
        | GoKind::Uint
        | GoKind::Uint8
        | GoKind::Uint16
        | GoKind::Uint32
        | GoKind::Uint64
        | GoKind::Float32
        | GoKind::Float64
        | GoKind::String
        | GoKind::Interface
        | GoKind::Map
        | GoKind::Slice
        | GoKind::Struct => T::OPTION_KIND,
    };
    const KIND: GoKind = T::OPTION_KIND;

    fn go_type_name(out: &mut String) {
        T::option_go_type_name(out);
    }

    fn go_drop(self) {
        if let Some(value) = self {
            value.go_drop();
        }
    }
}

impl<T: GoType + ?Sized> GoType for Box<T> {
    const DEREF_KIND: GoKind = T::DEREF_KIND;
    const KIND: GoKind = T::KIND;
    const OPTION_KIND: GoKind = T::OPTION_KIND;

    fn go_type_name(out: &mut String) {
        T::go_type_name(out);
    }

    fn option_go_type_name(out: &mut String) {
        T::option_go_type_name(out);
    }
}

impl<T: GoType + ?Sized> GoType for &T {
    const DEREF_KIND: GoKind = T::DEREF_KIND;
    const KIND: GoKind = T::KIND;
    const OPTION_KIND: GoKind = T::OPTION_KIND;

    fn go_type_name(out: &mut String) {
        T::go_type_name(out);
    }

    fn option_go_type_name(out: &mut String) {
        T::option_go_type_name(out);
    }
}

impl<T: GoType> GoType for Vec<T> {
    const KIND: GoKind = GoKind::Slice;
    const OPTION_KIND: GoKind = GoKind::Slice;

    fn go_type_name(out: &mut String) {
        out.push_str("[]");
        T::go_type_name(out);
    }

    fn option_go_type_name(out: &mut String) {
        Self::go_type_name(out);
    }

    fn go_drop(self) {
        self.into_iter().for_each(GoType::go_drop);
    }
}

impl<T: GoType> GoType for [T] {
    const KIND: GoKind = GoKind::Slice;
    const OPTION_KIND: GoKind = GoKind::Slice;

    fn go_type_name(out: &mut String) {
        out.push_str("[]");
        T::go_type_name(out);
    }

    fn option_go_type_name(out: &mut String) {
        Self::go_type_name(out);
    }
}

fn map_type_name<K: GoType, V: GoType>(out: &mut String) {
    out.push_str("map[");
    K::go_type_name(out);
    out.push(']');
    V::go_type_name(out);
}

impl<K: GoType, V: GoType> GoType for BTreeMap<K, V> {
    const KIND: GoKind = GoKind::Map;
    const OPTION_KIND: GoKind = GoKind::Map;

    fn go_type_name(out: &mut String) {
        map_type_name::<K, V>(out);
    }

    fn option_go_type_name(out: &mut String) {
        map_type_name::<K, V>(out);
    }

    fn go_drop(self) {
        self.into_values().for_each(GoType::go_drop);
    }
}

impl<K: GoType, V: GoType, S: BuildHasher> GoType for HashMap<K, V, S> {
    const KIND: GoKind = GoKind::Map;
    const OPTION_KIND: GoKind = GoKind::Map;

    fn go_type_name(out: &mut String) {
        map_type_name::<K, V>(out);
    }

    fn option_go_type_name(out: &mut String) {
        map_type_name::<K, V>(out);
    }

    fn go_drop(self) {
        self.into_values().for_each(GoType::go_drop);
    }
}

impl GoType for serde_json::Map<String, serde_json::Value> {
    const KIND: GoKind = GoKind::Map;
    const OPTION_KIND: GoKind = GoKind::Map;

    fn go_type_name(out: &mut String) {
        out.push_str("map[string]interface {}");
    }

    fn option_go_type_name(out: &mut String) {
        Self::go_type_name(out);
    }

    fn go_drop(self) {
        drop_value(serde_json::Value::Object(self));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;

    use super::{GoKind, GoType, go_type_name};

    /// `Ok` when the values are equal, otherwise an error naming the case.
    macro_rules! check {
        ($got:expr, $want:expr, $what:expr $(,)?) => {{
            let (got, want) = ($got, $want);
            if got == want {
                Ok(())
            } else {
                Err(format!("{}: got {got:?}, want {want:?}", $what))
            }
        }};
    }

    #[test]
    fn names_follow_go_reflect() -> Result<(), String> {
        // Go: reflect.Type.String() of the corresponding Go types
        check!(go_type_name::<Option<i64>>(), "*int64", "pointer")?;
        check!(
            go_type_name::<Option<Vec<String>>>(),
            "[]string",
            "nil-able slice",
        )?;
        check!(
            go_type_name::<Vec<Option<Box<u8>>>>(),
            "[]*uint8",
            "slice of pointers",
        )?;
        check!(
            go_type_name::<BTreeMap<String, Value>>(),
            "map[string]interface {}",
            "map",
        )?;
        check!(
            <Option<i64> as GoType>::DEREF_KIND,
            GoKind::Int64,
            "deref kind"
        )?;
        check!(
            <Option<Vec<u8>> as GoType>::KIND,
            GoKind::Slice,
            "slice kind"
        )
    }
}
