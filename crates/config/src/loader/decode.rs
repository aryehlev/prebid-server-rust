//! Glue (Rule B (c)): `github.com/mitchellh/mapstructure` v1.5.0 with `WeaklyTypedInput`, as
//! Viper's `Unmarshal` runs it over `AllSettings` into `config.Configuration`
//! (config/config.go:758, with `AccountModulesHookFunc`, `config/account_module_decoder.go:16`),
//! and as `UnmarshalKey` runs it into a `*bool` (config/config.go:886-889). This decides which
//! values an operator's configuration may give a field and what the field then holds: a string
//! parsed into a number or a bool, a scalar lifted into a one-element list, a list of maps merged
//! into a map, a number truncated to the field's size, and which values fail the load, worded as
//! mapstructure words it (ledger: `config.weak-typing`, `config.error-texts`).
//!
//! The decoder is driven by the Go types (`go_types.json`, written by `testdata/go` from the Go
//! code by reflection): each function is the mapstructure function of the same name. Where Go's
//! result depends on the order it iterates a map in, keys go in sorted order (ledger:
//! `config.map-order`).

use std::{collections::BTreeMap, fmt, sync::OnceLock};

use pbs_gocompat::{
    math,
    strconv::{
        format_float, format_int, format_uint, parse_bool, parse_float, parse_int, parse_uint,
    },
    strings::equal_fold,
};
use serde::Deserialize;
use serde_json::{Map as JsonMap, Number, Value as Json};

use super::{
    cast,
    value::{GoValue, compact_float, tagged},
};

/// The reflect kinds mapstructure decodes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Kind {
    /// `bool`.
    Bool,
    /// The signed integers.
    Int,
    /// The unsigned integers.
    Uint,
    /// The floats.
    Float,
    /// `string`.
    String,
    /// A struct.
    Struct,
    /// A pointer.
    Ptr,
    /// A slice.
    Slice,
    /// An array.
    Array,
    /// A map.
    Map,
    /// `interface{}`.
    Interface,
}

/// A Go type of `go_types.json`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GoType {
    /// The kind.
    pub(super) kind: Kind,
    /// The size of a number kind.
    #[serde(default)]
    pub(super) bits: Option<u32>,
    /// The element type of a pointer, slice, array or map.
    #[serde(default)]
    pub(super) elem: Option<String>,
    /// The key type of a map.
    #[serde(default)]
    pub(super) key: Option<String>,
    /// The length of an array.
    #[serde(default)]
    pub(super) len: Option<usize>,
    /// The exported fields of a struct, in declaration order.
    #[serde(default)]
    pub(super) fields: Vec<Field>,
    /// `account_modules` for `config.AccountModules`, which the decode hook converts.
    #[serde(default)]
    pub(super) hook: Option<String>,
}

/// An exported struct field.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Field {
    /// mapstructure's key: the tag name, else the Go name.
    pub(super) name: String,
    /// The field's type.
    #[serde(rename = "type")]
    pub(super) ty: String,
}

/// The types file.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GoTypes {
    /// Every type reachable from the roots, by `reflect.Type.String()`.
    pub(super) types: BTreeMap<String, GoType>,
}

/// `config.Configuration`'s type name.
pub(super) const CONFIGURATION: &str = "config.Configuration";

/// The types of `config.Configuration`, parsed once.
pub(super) fn configuration_types() -> Result<&'static GoTypes, String> {
    static TYPES: OnceLock<Result<GoTypes, String>> = OnceLock::new();
    TYPES
        .get_or_init(|| {
            serde_json::from_str(include_str!("go_types.json")).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// A value mapstructure decoded into a Go type.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    /// A `bool`.
    Bool(bool),
    /// A signed integer, truncated to its size.
    Int(i64),
    /// An unsigned integer, truncated to its size.
    Uint(u64),
    /// A float (a `float32` rounded to its precision).
    Float(f64),
    /// A string.
    String(String),
    /// A slice; `None` is nil.
    Slice(Option<Vec<Self>>),
    /// A slice of a `uint8` kind (`[]byte`, `json.RawMessage`); `None` is nil.
    Bytes(Option<Vec<u8>>),
    /// An array.
    Array(Vec<Self>),
    /// A map, its keys decoded into the key type; `None` is nil.
    Map(Option<Vec<(Self, Self)>>),
    /// A struct: every exported field under its mapstructure key, in declaration order.
    Struct(Vec<(String, Self)>),
    /// A pointer; `None` is nil.
    Ptr(Option<Box<Self>>),
    /// An `interface{}`; `None` is nil.
    Interface(Option<GoValue>),
}

impl Decoded {
    /// Go: `reflect.Value.IsZero`.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Bool(b) => !*b,
            Self::Int(i) => *i == 0,
            Self::Uint(u) => *u == 0,
            Self::Float(f) => f.to_bits() == 0,
            Self::String(s) => s.is_empty(),
            Self::Slice(items) => items.is_none(),
            Self::Bytes(bytes) => bytes.is_none(),
            Self::Map(entries) => entries.is_none(),
            Self::Ptr(target) => target.is_none(),
            Self::Interface(value) => value.is_none(),
            Self::Array(items) => items.iter().all(Self::is_zero),
            Self::Struct(fields) => fields.iter().all(|(_, v)| v.is_zero()),
        }
    }

    /// The target of a non-nil pointer.
    #[must_use]
    pub fn target(&self) -> Option<&Self> {
        match self {
            Self::Ptr(Some(target)) => Some(target),
            Self::Bool(_)
            | Self::Int(_)
            | Self::Uint(_)
            | Self::Float(_)
            | Self::String(_)
            | Self::Slice(_)
            | Self::Bytes(_)
            | Self::Array(_)
            | Self::Map(_)
            | Self::Struct(_)
            | Self::Ptr(None)
            | Self::Interface(_) => None,
        }
    }

    /// The field `name` of a struct.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Struct(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            Self::Bool(_)
            | Self::Int(_)
            | Self::Uint(_)
            | Self::Float(_)
            | Self::String(_)
            | Self::Slice(_)
            | Self::Bytes(_)
            | Self::Array(_)
            | Self::Map(_)
            | Self::Ptr(_)
            | Self::Interface(_) => None,
        }
    }

    /// The value in the typed form of the Go goldens (`testdata/go/encode.go`'s `typed`): a
    /// struct as an object of its non-zero fields, a pointer as its target, a map as an object
    /// keyed by `fmt.Sprint` of the key, a `[]byte` as `{"$bytes": text}`, an `interface{}` in the
    /// compact form of [`GoValue::compact`], and a nil pointer, map, slice or interface as null.
    #[must_use]
    pub fn compact(&self) -> Json {
        match self {
            Self::Bool(b) => Json::Bool(*b),
            Self::Int(i) => Json::from(*i),
            Self::Uint(u) => Json::from(*u),
            Self::Float(f) => compact_float(*f),
            Self::String(s) => Json::String(s.clone()),
            Self::Slice(None) | Self::Bytes(None) | Self::Map(None) | Self::Ptr(None) => Json::Null,
            Self::Interface(value) => value.as_ref().map_or(Json::Null, GoValue::compact),
            Self::Slice(Some(items)) | Self::Array(items) => {
                Json::Array(items.iter().map(Self::compact).collect())
            }
            Self::Bytes(Some(bytes)) => tagged(
                "$bytes",
                Json::String(String::from_utf8_lossy(bytes).into_owned()),
            ),
            Self::Map(Some(entries)) => Json::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.key_text(), v.compact()))
                    .collect::<JsonMap<_, _>>(),
            ),
            Self::Struct(fields) => Json::Object(
                fields
                    .iter()
                    .filter(|(_, v)| !v.is_zero())
                    .map(|(k, v)| (k.clone(), v.compact()))
                    .collect::<JsonMap<_, _>>(),
            ),
            Self::Ptr(Some(target)) => target.compact(),
        }
    }

    /// Go: `fmt.Sprint(key)` of a map key.
    fn key_text(&self) -> String {
        match self {
            Self::String(s) => s.clone(),
            Self::Int(i) => i.to_string(),
            Self::Uint(u) => u.to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Float(f) => format_float(*f, b'g', -1, 64),
            Self::Interface(Some(value)) => value.to_string(),
            Self::Interface(None)
            | Self::Slice(_)
            | Self::Bytes(_)
            | Self::Array(_)
            | Self::Map(_)
            | Self::Struct(_)
            | Self::Ptr(_) => String::new(),
        }
    }
}

/// A decode failure: one error, or mapstructure's `*Error` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Fail {
    /// A single error.
    One(String),
    /// Several, as `*mapstructure.Error` holds them.
    Many(Vec<String>),
}

impl fmt::Display for Fail {
    /// Go: the error's `Error()`: mapstructure's list sorted under its count.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One(text) => f.write_str(text),
            Self::Many(errors) => {
                let mut points: Vec<String> = errors.iter().map(|e| format!("* {e}")).collect();
                points.sort();
                write!(
                    f,
                    "{} error(s) decoding:\n\n{}",
                    errors.len(),
                    points.join("\n")
                )
            }
        }
    }
}

/// Go: `appendErrors`.
fn append(errors: &mut Vec<String>, fail: Fail) {
    match fail {
        Fail::One(text) => errors.push(text),
        Fail::Many(list) => errors.extend(list),
    }
}

/// Go: `&Error{errors}` when there are any.
fn finish(errors: Vec<String>) -> Result<(), Fail> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Fail::Many(errors))
    }
}

/// A decoder over a set of Go types.
#[derive(Debug, Clone, Copy)]
pub(super) struct Decoder<'t> {
    /// The types.
    types: &'t GoTypes,
    /// Whether `AccountModulesHookFunc` runs (`v.Unmarshal` in `config.New`) or not
    /// (`UnmarshalKey`, whose default hooks do nothing for its targets).
    account_modules_hook: bool,
}

impl<'t> Decoder<'t> {
    /// A decoder over `types`.
    pub(super) const fn new(types: &'t GoTypes, account_modules_hook: bool) -> Self {
        Self {
            types,
            account_modules_hook,
        }
    }

    /// The type `name`.
    fn ty(&self, name: &str) -> Result<&'t GoType, Fail> {
        self.types
            .types
            .get(name)
            .ok_or_else(|| Fail::One(format!("{name}: unsupported type")))
    }

    /// The element type of a pointer, slice, array or map type.
    fn elem(t: &GoType) -> Result<&str, Fail> {
        t.elem
            .as_deref()
            .ok_or_else(|| Fail::One("a type without an element type".to_owned()))
    }

    /// The zero value of the type `name`.
    pub(super) fn zero(&self, name: &str) -> Result<Decoded, Fail> {
        let t = self.ty(name)?;
        Ok(match t.kind {
            Kind::Bool => Decoded::Bool(false),
            Kind::Int => Decoded::Int(0),
            Kind::Uint => Decoded::Uint(0),
            Kind::Float => Decoded::Float(0.0),
            Kind::String => Decoded::String(String::new()),
            Kind::Slice => {
                if self.is_byte_kind(Self::elem(t)?) {
                    Decoded::Bytes(None)
                } else {
                    Decoded::Slice(None)
                }
            }
            Kind::Array => {
                let elem = self.zero(Self::elem(t)?)?;
                Decoded::Array(vec![elem; t.len.unwrap_or(0)])
            }
            Kind::Map => Decoded::Map(None),
            Kind::Ptr => Decoded::Ptr(None),
            Kind::Interface => Decoded::Interface(None),
            Kind::Struct => Decoded::Struct(
                t.fields
                    .iter()
                    .map(|f| Ok((f.name.clone(), self.zero(&f.ty)?)))
                    .collect::<Result<_, Fail>>()?,
            ),
        })
    }

    /// Whether the type `name` is of a `uint8` kind.
    fn is_byte_kind(&self, name: &str) -> bool {
        self.types
            .types
            .get(name)
            .is_some_and(|t| t.kind == Kind::Uint && t.bits == Some(8))
    }

    /// Go: `Decoder.decode(name, input, outVal)`.
    pub(super) fn decode(
        &self,
        name: &str,
        input: &GoValue,
        ty: &str,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        if matches!(input, GoValue::Nil) {
            return Ok(());
        }
        let t = self.ty(ty)?;
        if self.account_modules_hook
            && t.hook.as_deref() == Some("account_modules")
            && input.is_map()
        {
            let modules = account_modules(input)
                .map_err(|e| Fail::One(format!("error decoding '{name}': {e}")))?;
            set_modules(out, modules);
            return Ok(());
        }
        match t.kind {
            Kind::Bool => decode_bool(name, input, ty, out),
            Kind::Interface => {
                *out = Decoded::Interface(Some(input.clone()));
                Ok(())
            }
            Kind::String => decode_string(name, input, ty, out),
            Kind::Int => decode_int(name, input, ty, t.bits.unwrap_or(64), out),
            Kind::Uint => decode_uint(name, input, ty, t.bits.unwrap_or(64), out),
            Kind::Float => decode_float(name, input, ty, t.bits.unwrap_or(64), out),
            Kind::Struct => self.decode_struct(name, input, t, out),
            Kind::Map => self.decode_map(name, input, ty, t, out),
            Kind::Ptr => self.decode_ptr(name, input, t, out),
            Kind::Slice => self.decode_slice(name, input, t, out),
            Kind::Array => self.decode_array(name, input, t, out),
        }
    }

    /// Go: `decodeStruct` and `decodeStructFromMap`.
    fn decode_struct(
        &self,
        name: &str,
        input: &GoValue,
        t: &GoType,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        let entries: Vec<(GoValue, GoValue)> = match input {
            GoValue::Map(map) => map
                .iter()
                .map(|(k, v)| (GoValue::String(k.clone()), v.clone()))
                .collect(),
            GoValue::AnyMap(entries) => entries.clone(),
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => {
                return Err(Fail::One(format!(
                    "'{name}' expected a map, got '{}'",
                    input.go_kind()
                )));
            }
        };
        let Decoded::Struct(fields) = out else {
            return Err(Fail::One(format!("'{name}' is not a struct")));
        };
        let mut errors = Vec::new();
        for (field, (_, slot)) in t.fields.iter().zip(fields.iter_mut()) {
            // Go: the key spelled as the field's key, else the first key equal under Unicode case
            // folding.
            let exact = entries
                .iter()
                .find(|(k, _)| matches!(k, GoValue::String(k) if *k == field.name));
            let found = exact.or_else(|| {
                entries
                    .iter()
                    .find(|(k, _)| matches!(k, GoValue::String(k) if equal_fold(k, &field.name)))
            });
            let Some((_, raw)) = found else {
                continue;
            };
            let full = if name.is_empty() {
                field.name.clone()
            } else {
                format!("{name}.{}", field.name)
            };
            if let Err(fail) = self.decode(&full, raw, &field.ty, slot) {
                append(&mut errors, fail);
            }
        }
        finish(errors)
    }

    /// Go: `decodeMap`, `decodeMapFromMap` and `decodeMapFromSlice`.
    fn decode_map(
        &self,
        name: &str,
        input: &GoValue,
        ty: &str,
        t: &GoType,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        let existing = match out {
            Decoded::Map(Some(entries)) => entries.clone(),
            Decoded::Bool(_)
            | Decoded::Int(_)
            | Decoded::Uint(_)
            | Decoded::Float(_)
            | Decoded::String(_)
            | Decoded::Slice(_)
            | Decoded::Bytes(_)
            | Decoded::Array(_)
            | Decoded::Map(None)
            | Decoded::Struct(_)
            | Decoded::Ptr(_)
            | Decoded::Interface(_) => Vec::new(),
        };
        let (entries, interface_keys) = match input {
            GoValue::Map(map) => (
                map.iter()
                    .map(|(k, v)| (GoValue::String(k.clone()), v.clone()))
                    .collect::<Vec<_>>(),
                false,
            ),
            GoValue::AnyMap(entries) => (entries.clone(), true),
            GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => {
                let items = input.elements().unwrap_or_default();
                if items.is_empty() {
                    *out = Decoded::Map(Some(existing));
                    return Ok(());
                }
                for (i, item) in items.iter().enumerate() {
                    self.decode(&format!("{name}[{i}]"), item, ty, out)?;
                }
                return Ok(());
            }
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_) => {
                return Err(Fail::One(format!(
                    "'{name}' expected a map, got '{}'",
                    input.go_kind()
                )));
            }
        };
        let mut map = existing;
        if entries.is_empty() {
            *out = Decoded::Map(Some(map));
            return Ok(());
        }
        let key_ty = t
            .key
            .as_deref()
            .ok_or_else(|| Fail::One("a map type without a key type".to_owned()))?;
        let elem_ty = Self::elem(t)?;
        let mut errors = Vec::new();
        for (key, value) in &entries {
            // Go: name + "[" + k.String() + "]"; String() of an interface{} key is a placeholder.
            let key_text = if interface_keys {
                "<interface {} Value>".to_owned()
            } else {
                cast::to_string(key)
            };
            let field = format!("{name}[{key_text}]");
            let mut current_key = self.zero(key_ty)?;
            if let Err(fail) = self.decode(&field, key, key_ty, &mut current_key) {
                append(&mut errors, fail);
                continue;
            }
            let mut current = self.zero(elem_ty)?;
            if let Err(fail) = self.decode(&field, value, elem_ty, &mut current) {
                append(&mut errors, fail);
                continue;
            }
            match map.iter_mut().find(|(k, _)| *k == current_key) {
                Some(slot) => slot.1 = current,
                None => map.push((current_key, current)),
            }
        }
        *out = Decoded::Map(Some(map));
        finish(errors)
    }

    /// Go: `decodePtr`.
    fn decode_ptr(
        &self,
        name: &str,
        input: &GoValue,
        t: &GoType,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        if matches!(input, GoValue::Strings(None)) {
            // A typed nil slice sets the pointer to nil.
            *out = Decoded::Ptr(None);
            return Ok(());
        }
        let elem = Self::elem(t)?;
        let mut target = match out {
            Decoded::Ptr(Some(target)) => (**target).clone(),
            Decoded::Bool(_)
            | Decoded::Int(_)
            | Decoded::Uint(_)
            | Decoded::Float(_)
            | Decoded::String(_)
            | Decoded::Slice(_)
            | Decoded::Bytes(_)
            | Decoded::Array(_)
            | Decoded::Map(_)
            | Decoded::Struct(_)
            | Decoded::Ptr(None)
            | Decoded::Interface(_) => self.zero(elem)?,
        };
        self.decode(name, input, elem, &mut target)?;
        *out = Decoded::Ptr(Some(Box::new(target)));
        Ok(())
    }

    /// The elements of a slice input, lifting a value that is not a slice as mapstructure's weak
    /// decode does; `Ok(None)` when the slice is set as it is.
    fn slice_items(
        &self,
        input: &GoValue,
        elem: &str,
        empty: &mut Option<Decoded>,
    ) -> Option<Vec<GoValue>> {
        match input {
            GoValue::Map(map) if map.is_empty() => {
                *empty = Some(self.empty_slice(elem));
                None
            }
            GoValue::AnyMap(entries) if entries.is_empty() => {
                *empty = Some(self.empty_slice(elem));
                None
            }
            GoValue::String(s) if self.is_byte_kind(elem) => {
                Some(s.bytes().map(|b| GoValue::Uint64(u64::from(b))).collect())
            }
            GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => input.elements(),
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::Map(_)
            | GoValue::AnyMap(_) => Some(vec![input.clone()]),
        }
    }

    /// An empty, non-nil slice of the element type `elem`.
    fn empty_slice(&self, elem: &str) -> Decoded {
        if self.is_byte_kind(elem) {
            Decoded::Bytes(Some(Vec::new()))
        } else {
            Decoded::Slice(Some(Vec::new()))
        }
    }

    /// Go: `decodeSlice`.
    fn decode_slice(
        &self,
        name: &str,
        input: &GoValue,
        t: &GoType,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        let elem = Self::elem(t)?;
        let mut empty = None;
        let Some(items) = self.slice_items(input, elem, &mut empty) else {
            if let Some(empty) = empty {
                *out = empty;
            }
            // Go: a nil input slice is not allocated.
            return Ok(());
        };
        let mut current: Vec<Decoded> = match out {
            Decoded::Slice(Some(existing)) => existing.clone(),
            Decoded::Bytes(Some(bytes)) => {
                bytes.iter().map(|b| Decoded::Uint(u64::from(*b))).collect()
            }
            Decoded::Bool(_)
            | Decoded::Int(_)
            | Decoded::Uint(_)
            | Decoded::Float(_)
            | Decoded::String(_)
            | Decoded::Slice(None)
            | Decoded::Bytes(None)
            | Decoded::Array(_)
            | Decoded::Map(_)
            | Decoded::Struct(_)
            | Decoded::Ptr(_)
            | Decoded::Interface(_) => Vec::new(),
        };
        let mut errors = Vec::new();
        for (i, item) in items.iter().enumerate() {
            while current.len() <= i {
                current.push(self.zero(elem)?);
            }
            if let Some(slot) = current.get_mut(i)
                && let Err(fail) = self.decode(&format!("{name}[{i}]"), item, elem, slot)
            {
                append(&mut errors, fail);
            }
        }
        *out = if self.is_byte_kind(elem) {
            Decoded::Bytes(Some(
                current
                    .iter()
                    .map(|v| match v {
                        Decoded::Uint(u) => u8::try_from(*u).unwrap_or(0),
                        Decoded::Bool(_)
                        | Decoded::Int(_)
                        | Decoded::Float(_)
                        | Decoded::String(_)
                        | Decoded::Slice(_)
                        | Decoded::Bytes(_)
                        | Decoded::Array(_)
                        | Decoded::Map(_)
                        | Decoded::Struct(_)
                        | Decoded::Ptr(_)
                        | Decoded::Interface(_) => 0,
                    })
                    .collect(),
            ))
        } else {
            Decoded::Slice(Some(current))
        };
        finish(errors)
    }

    /// Go: `decodeArray` into a zero array.
    fn decode_array(
        &self,
        name: &str,
        input: &GoValue,
        t: &GoType,
        out: &mut Decoded,
    ) -> Result<(), Fail> {
        let elem = Self::elem(t)?;
        let len = t.len.unwrap_or(0);
        let items = match input {
            GoValue::Map(map) if map.is_empty() => {
                *out = Decoded::Array(vec![self.zero(elem)?; len]);
                return Ok(());
            }
            GoValue::AnyMap(entries) if entries.is_empty() => {
                *out = Decoded::Array(vec![self.zero(elem)?; len]);
                return Ok(());
            }
            GoValue::Map(_) | GoValue::AnyMap(_) => {
                return Err(Fail::One(format!(
                    "'{name}': source data must be an array or slice, got map"
                )));
            }
            GoValue::List(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => input.elements().unwrap_or_default(),
            GoValue::Nil
            | GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_) => vec![input.clone()],
        };
        if items.len() > len {
            return Err(Fail::One(format!(
                "'{name}': expected source data to have length less or equal to {len}, got {}",
                items.len()
            )));
        }
        let mut array = vec![self.zero(elem)?; len];
        let mut errors = Vec::new();
        for (i, (item, slot)) in items.iter().zip(array.iter_mut()).enumerate() {
            if let Err(fail) = self.decode(&format!("{name}[{i}]"), item, elem, slot) {
                append(&mut errors, fail);
            }
        }
        *out = Decoded::Array(array);
        finish(errors)
    }
}

/// Go: mapstructure's "expected type ... got unconvertible type" error.
fn unconvertible(name: &str, input: &GoValue, ty: &str) -> Fail {
    Fail::One(format!(
        "'{name}' expected type '{ty}', got unconvertible type '{}', value: '{input}'",
        input.go_type()
    ))
}

/// Go: `decodeBool`.
fn decode_bool(name: &str, input: &GoValue, ty: &str, out: &mut Decoded) -> Result<(), Fail> {
    let b = match input {
        GoValue::Bool(b) => *b,
        GoValue::Int(i) | GoValue::Int64(i) => *i != 0,
        GoValue::Uint64(u) => *u != 0,
        GoValue::Float64(f) => *f != 0.0,
        GoValue::String(s) => match parse_bool(s) {
            Ok(b) => b,
            Err(_) if s.is_empty() => false,
            Err(err) => return Err(Fail::One(format!("cannot parse '{name}' as bool: {err}"))),
        },
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => return Err(unconvertible(name, input, ty)),
    };
    *out = Decoded::Bool(b);
    Ok(())
}

/// Go: `decodeString`.
fn decode_string(name: &str, input: &GoValue, ty: &str, out: &mut Decoded) -> Result<(), Fail> {
    let s = match input {
        GoValue::String(s) => s.clone(),
        GoValue::Bool(b) => if *b { "1" } else { "0" }.to_owned(),
        GoValue::Int(i) | GoValue::Int64(i) => format_int(*i, 10),
        GoValue::Uint64(u) => format_uint(*u, 10),
        GoValue::Float64(f) => format_float(*f, b'f', -1, 64),
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => return Err(unconvertible(name, input, ty)),
    };
    *out = Decoded::String(s);
    Ok(())
}

/// Go: `reflect.Value.SetInt` on a field of `bits` bits: the value truncated.
const fn truncate_int(x: i64, bits: u32) -> i64 {
    if bits >= 64 {
        x
    } else {
        let shift = 64 - bits;
        (x << shift) >> shift
    }
}

/// Go: `reflect.Value.SetUint` on a field of `bits` bits: the value truncated.
const fn truncate_uint(x: u64, bits: u32) -> u64 {
    if bits >= 64 { x } else { x & ((1 << bits) - 1) }
}

/// Go: `uint64(f)` on amd64: below 2^63 through `int64(f)`, else `int64(f - 2^63)` with the top
/// bit set.
fn float_to_uint64(f: f64) -> u64 {
    // 2^63
    const TWO_63: f64 = f64::from_bits(0x43E0_0000_0000_0000);
    if f < TWO_63 {
        math::int64(f).cast_unsigned()
    } else {
        math::int64(f - TWO_63).cast_unsigned() | (1 << 63)
    }
}

/// Go: `decodeInt`.
fn decode_int(
    name: &str,
    input: &GoValue,
    ty: &str,
    bits: u32,
    out: &mut Decoded,
) -> Result<(), Fail> {
    let i = match input {
        GoValue::Int(i) | GoValue::Int64(i) => *i,
        GoValue::Uint64(u) => u.cast_signed(),
        GoValue::Float64(f) => math::int64(*f),
        GoValue::Bool(b) => i64::from(*b),
        GoValue::String(s) => {
            let s = if s.is_empty() { "0" } else { s.as_str() };
            parse_int(s, 0, i64::from(bits))
                .map_err(|err| Fail::One(format!("cannot parse '{name}' as int: {err}")))?
        }
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => return Err(unconvertible(name, input, ty)),
    };
    *out = Decoded::Int(truncate_int(i, bits));
    Ok(())
}

/// Go: `decodeUint`; with weak typing a negative number wraps.
fn decode_uint(
    name: &str,
    input: &GoValue,
    ty: &str,
    bits: u32,
    out: &mut Decoded,
) -> Result<(), Fail> {
    let u = match input {
        GoValue::Int(i) | GoValue::Int64(i) => i.cast_unsigned(),
        GoValue::Uint64(u) => *u,
        GoValue::Float64(f) => float_to_uint64(*f),
        GoValue::Bool(b) => u64::from(*b),
        GoValue::String(s) => {
            let s = if s.is_empty() { "0" } else { s.as_str() };
            parse_uint(s, 0, i64::from(bits))
                .map_err(|err| Fail::One(format!("cannot parse '{name}' as uint: {err}")))?
        }
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => return Err(unconvertible(name, input, ty)),
    };
    *out = Decoded::Uint(truncate_uint(u, bits));
    Ok(())
}

/// Go: `decodeFloat`.
fn decode_float(
    name: &str,
    input: &GoValue,
    ty: &str,
    bits: u32,
    out: &mut Decoded,
) -> Result<(), Fail> {
    let f = match input {
        GoValue::Int(i) | GoValue::Int64(i) => cast::int_to_float(*i),
        GoValue::Uint64(u) => cast::uint_to_float(*u),
        GoValue::Float64(f) => *f,
        GoValue::Bool(b) => f64::from(u8::from(*b)),
        GoValue::String(s) => {
            let s = if s.is_empty() { "0" } else { s.as_str() };
            parse_float(s, i64::from(bits))
                .map_err(|err| Fail::One(format!("cannot parse '{name}' as float: {err}")))?
        }
        GoValue::Nil
        | GoValue::List(_)
        | GoValue::Map(_)
        | GoValue::AnyMap(_)
        | GoValue::Strings(_)
        | GoValue::StringLists(_)
        | GoValue::EmptySlice(_) => return Err(unconvertible(name, input, ty)),
    };
    let f = if bits == 32 {
        f64::from(math::float32(f))
    } else {
        f
    };
    *out = Decoded::Float(f);
    Ok(())
}

/// The modules `AccountModulesHookFunc` converts: each module's configuration (nil for a null
/// one) as its keys and their `jsonutil.Marshal` bytes.
type Modules = Vec<(String, Option<Vec<(String, Vec<u8>)>>)>;

/// Go: `AccountModulesHookFunc` and `convertToRawMessageMap`.
fn account_modules(input: &GoValue) -> Result<Modules, String> {
    let GoValue::Map(outer) = input else {
        // Go: data.(map[string]interface{}) panics on any other map type.
        return Err(format!(
            "interface conversion: interface {{}} is {}, not map[string]interface {{}}",
            input.go_type()
        ));
    };
    let mut modules = Vec::new();
    for (key, value) in outer {
        match value {
            GoValue::Nil => modules.push((key.clone(), None)),
            GoValue::Map(inner) => {
                let mut raw = Vec::new();
                for (inner_key, inner_value) in inner {
                    let bytes = marshal(inner_value).map_err(|e| {
                        format!(
                            "failed to convert account modules map: failed to marshal value for \
                             {key}.{inner_key}: {e}"
                        )
                    })?;
                    raw.push((inner_key.clone(), bytes));
                }
                modules.push((key.clone(), Some(raw)));
            }
            GoValue::Bool(_)
            | GoValue::Int(_)
            | GoValue::Int64(_)
            | GoValue::Uint64(_)
            | GoValue::Float64(_)
            | GoValue::String(_)
            | GoValue::List(_)
            | GoValue::AnyMap(_)
            | GoValue::Strings(_)
            | GoValue::StringLists(_)
            | GoValue::EmptySlice(_) => {
                return Err(format!(
                    "failed to convert account modules map: failed to convert inner map for {key}"
                ));
            }
        }
    }
    Ok(modules)
}

/// Go: decoding the hook's `map[string]map[string]json.RawMessage` into `config.AccountModules`.
fn set_modules(out: &mut Decoded, modules: Modules) {
    let mut map = match out {
        Decoded::Map(Some(entries)) => entries.clone(),
        Decoded::Bool(_)
        | Decoded::Int(_)
        | Decoded::Uint(_)
        | Decoded::Float(_)
        | Decoded::String(_)
        | Decoded::Slice(_)
        | Decoded::Bytes(_)
        | Decoded::Array(_)
        | Decoded::Map(None)
        | Decoded::Struct(_)
        | Decoded::Ptr(_)
        | Decoded::Interface(_) => Vec::new(),
    };
    for (key, inner) in modules {
        let value = Decoded::Map(inner.map(|entries| {
            entries
                .into_iter()
                .map(|(k, bytes)| (Decoded::String(k), Decoded::Bytes(Some(bytes))))
                .collect()
        }));
        let key = Decoded::String(key);
        match map.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => map.push((key, value)),
        }
    }
    *out = Decoded::Map(Some(map));
}

/// Go: `jsonutil.Marshal(v)` of an `interface{}` value, written by `serde_json`: keys sorted as
/// Go sorts them, a whole `float64` as the integer Go writes (a module reading the field as an
/// integer takes it, as in Go), and otherwise `<`, `>` and `&` unescaped and floats in
/// `serde_json`'s spelling (ledger: `config.module-json`).
fn marshal(value: &GoValue) -> Result<Vec<u8>, String> {
    let json = to_json(value)?;
    serde_json::to_vec(&json).map_err(|e| e.to_string())
}

/// A whole `float64` as `jsonutil.Marshal` writes it (`strconv.FormatFloat(f, 'f', -1, 64)` below
/// 1e21), when that text is an integer `serde_json` holds as one: `5` where `serde_json` writes
/// `5.0`.
fn whole_number(f: f64) -> Option<Json> {
    let go = format_float(f, b'f', -1, 64);
    go.parse::<i64>()
        .map(Json::from)
        .or_else(|_| go.parse::<u64>().map(Json::from))
        .ok()
}

/// A value as `serde_json`'s model of `interface{}`.
fn to_json(value: &GoValue) -> Result<Json, String> {
    Ok(match value {
        GoValue::Nil | GoValue::Strings(None) => Json::Null,
        GoValue::Bool(b) => Json::Bool(*b),
        GoValue::Int(i) | GoValue::Int64(i) => Json::from(*i),
        GoValue::Uint64(u) => Json::from(*u),
        GoValue::Float64(f) => whole_number(*f).map_or_else(
            || {
                Number::from_f64(*f)
                    .map(Json::Number)
                    .ok_or_else(|| format!("unsupported value: {value}"))
            },
            Ok,
        )?,
        GoValue::String(s) => Json::String(s.clone()),
        GoValue::List(items) => Json::Array(items.iter().map(to_json).collect::<Result<_, _>>()?),
        GoValue::Map(map) => Json::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), to_json(v)?)))
                .collect::<Result<JsonMap<_, _>, String>>()?,
        ),
        GoValue::AnyMap(_) => return Err(format!("unsupported type: {}", value.go_type())),
        GoValue::Strings(Some(items)) => {
            Json::Array(items.iter().cloned().map(Json::String).collect())
        }
        GoValue::StringLists(lists) => Json::Array(
            lists
                .iter()
                .map(|l| Json::Array(l.iter().cloned().map(Json::String).collect()))
                .collect(),
        ),
        GoValue::EmptySlice(_) => Json::Array(Vec::new()),
    })
}
