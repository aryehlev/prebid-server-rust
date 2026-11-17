//! Go `github.com/mitchellh/mapstructure` v1.5.0: decoding [`Value`] trees (what viper holds)
//! into Rust values standing for Go values.
//!
//! [`DecoderConfig::viper`] is viper v1.12.0's `defaultDecoderConfig`: `WeaklyTypedInput`, the
//! `mapstructure` tag, and `ComposeDecodeHookFunc(StringToTimeDurationHookFunc(),
//! StringToSliceHookFunc(","))`. viper's `DecodeHook` option replaces that hook (Prebid's
//! `config.New` passes `AccountModulesHookFunc` that way): set
//! [`DecoderConfig::decode_hook`].
//!
//! Targets implement [`MsDecode`]; structs derive [`GoMapstructure`](crate::GoMapstructure).
//! [`RawJson`] is a `json.RawMessage` target, a `[]byte` that decodes from [`Value::bytes`] (and
//! from anything else `decodeSlice` accepts). A hook sees the target's Go type, also when
//! `decodeMapFromSlice` decodes each item of a slice into a named map type.
//! Errors carry Go's texts: a single failure is a [`DecodeError::Message`] (`error decoding 'a':
//! ...`), collected failures a [`DecodeError::Aggregate`] (`2 error(s) decoding:\n\n* ...`),
//! and a Go runtime panic a [`DecodeError::Panic`].
//!
//! # Not ported
//!
//! - `Metadata`, `ErrorUnset`, `MatchName`, `IgnoreUntaggedFields`, `DecodeHookFuncValue` hooks,
//!   and `TagName`s other than `mapstructure`.
//! - Struct-valued and `json.Number` sources, which [`Value`] cannot hold (`decodeMapFromStruct`,
//!   struct-to-struct decoding).
//! - Decoding into a non-nil `interface{}` holding a slice or a map replaces the value; Go decodes
//!   into a copy of it, keeping its dynamic type (scalars are decoded as Go does).
//! - `DecoderConfig.Squash` (viper leaves it off), and `,squash` on pointer fields.
//! - Map iteration order: see [`crate::Value`].

use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    error, fmt,
    hash::{BuildHasher, Hash},
    mem,
    sync::Arc,
};

use pbs_gocompat::{
    fmt::sprintf,
    math, strconv,
    strings::{equal_fold, split},
    time::{Duration, Time, parse_duration},
};
use pbs_gojson::{GoKind, GoString, RawJson};

use crate::{
    go_uint64, int_to_float, uint_to_float,
    value::{Type, Value, kind_name},
};

/// A mapstructure error (Go: the `error` that `Decode` returns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Go: `*mapstructure.DecodeError`, the errors collected from a map, slice or struct.
    Aggregate(Vec<String>),
    /// Any other error, by its text.
    Message(String),
    /// A Go runtime panic, which mapstructure does not recover (the Go program would crash), by
    /// its recovered value: `ErrorUnused` with a non-string key fails the `rawKey.(string)`
    /// assertion.
    Panic(String),
}

impl DecodeError {
    /// Go: `appendErrors`: flattens aggregated errors into `errors`. A panic is returned, to
    /// unwind as Go's does.
    fn append_to(self, errors: &mut Vec<String>) -> Result<(), Self> {
        match self {
            Self::Aggregate(list) => errors.extend(list),
            Self::Message(text) => errors.push(text),
            Self::Panic(_) => return Err(self),
        }
        Ok(())
    }

    /// Go: `&DecodeError{errors}` when there are any.
    fn collect(errors: Vec<String>) -> Result<(), Self> {
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Self::Aggregate(errors))
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aggregate(errors) => {
                let mut points: Vec<String> = errors.iter().map(|e| format!("* {e}")).collect();
                points.sort();
                write!(
                    f,
                    "{} error(s) decoding:\n\n{}",
                    errors.len(),
                    points.join("\n")
                )
            }
            Self::Message(text) | Self::Panic(text) => f.write_str(text),
        }
    }
}

impl error::Error for DecodeError {}

/// The target type a decode hook sees (Go: the `to` argument of a `DecodeHookFunc`).
#[derive(Clone, Copy, Debug)]
pub struct HookTarget {
    /// Go's `t.Kind()`.
    kind: GoKind,
    /// Appends Go's `t.String()`.
    type_name: fn(&mut String),
}

impl HookTarget {
    /// The target type `T`.
    #[must_use]
    pub fn of<T: MsDecode + ?Sized>() -> Self {
        Self {
            kind: T::MS_KIND,
            type_name: T::ms_type_name,
        }
    }

    /// A map type named by `type_name`.
    const fn map(type_name: fn(&mut String)) -> Self {
        Self {
            kind: GoKind::Map,
            type_name,
        }
    }

    /// Go's `t.Kind()`.
    #[must_use]
    pub const fn kind(&self) -> GoKind {
        self.kind
    }

    /// Go's `t.String()`, such as `time.Duration` or `config.AccountModules`.
    #[must_use]
    pub fn type_name(&self) -> String {
        let mut out = String::new();
        (self.type_name)(&mut out);
        out
    }
}

/// A decode hook (Go: `mapstructure.DecodeHookFunc`).
///
/// Given the target type and the data (whose type is [`Value::type_of`]), it returns
/// replacement data (`Ok(None)` keeps it), or an error that decoding reports as
/// `error decoding '<name>': <error>`.
pub type DecodeHook =
    Arc<dyn Fn(HookTarget, &Value) -> Result<Option<Value>, String> + Send + Sync>;

/// Go: `mapstructure.StringToTimeDurationHookFunc()`.
#[must_use]
pub fn string_to_time_duration_hook() -> DecodeHook {
    Arc::new(|to: HookTarget, data: &Value| {
        let Value::String(s) = data else {
            return Ok(None);
        };
        if to.kind != GoKind::Int64 || to.type_name() != "time.Duration" {
            return Ok(None);
        }
        parse_duration(s)
            .map(|d| Some(Value::Duration(d)))
            .map_err(|e| e.to_string())
    })
}

/// Go: `mapstructure.StringToSliceHookFunc(sep)`.
#[must_use]
pub fn string_to_slice_hook(sep: &str) -> DecodeHook {
    let sep = sep.to_owned();
    Arc::new(move |to: HookTarget, data: &Value| {
        let Value::String(raw) = data else {
            return Ok(None);
        };
        if to.kind != GoKind::Slice {
            return Ok(None);
        }
        let parts = if raw.is_empty() {
            Vec::new()
        } else {
            split(raw, &sep)
                .into_iter()
                .map(|p| Value::String(p.to_owned()))
                .collect()
        };
        Ok(Some(Value::Slice(&Type::String, Some(parts))))
    })
}

/// Go: `mapstructure.ComposeDecodeHookFunc(hooks...)`: runs the hooks in order, each on the
/// previous one's output, stopping at the first error.
#[must_use]
pub fn compose_hooks(hooks: Vec<DecodeHook>) -> DecodeHook {
    Arc::new(move |to: HookTarget, data: &Value| {
        let mut current: Option<Value> = None;
        for hook in &hooks {
            if let Some(next) = hook(to, current.as_ref().unwrap_or(data))? {
                current = Some(next);
            }
        }
        Ok(current)
    })
}

/// Go: `mapstructure.DecoderConfig`, for the options this port supports.
#[derive(Clone, Default)]
pub struct DecoderConfig {
    /// Go: `DecodeHook`.
    pub decode_hook: Option<DecodeHook>,
    /// Go: `ErrorUnused`: report map keys that match no struct field (viper's
    /// `UnmarshalExact`).
    pub error_unused: bool,
    /// Go: `ZeroFields`: zero targets before decoding into them.
    pub zero_fields: bool,
    /// Go: `WeaklyTypedInput`.
    pub weakly_typed_input: bool,
}

impl fmt::Debug for DecoderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecoderConfig")
            .field("decode_hook", &self.decode_hook.is_some())
            .field("error_unused", &self.error_unused)
            .field("zero_fields", &self.zero_fields)
            .field("weakly_typed_input", &self.weakly_typed_input)
            .finish()
    }
}

impl DecoderConfig {
    /// viper v1.12.0's `defaultDecoderConfig`.
    #[must_use]
    pub fn viper() -> Self {
        Self {
            decode_hook: Some(compose_hooks(vec![
                string_to_time_duration_hook(),
                string_to_slice_hook(","),
            ])),
            weakly_typed_input: true,
            ..Self::default()
        }
    }
}

/// Go: `mapstructure.Decoder`.
#[derive(Debug)]
pub struct Decoder {
    /// Go: `config`.
    config: DecoderConfig,
}

/// Go: `mapstructure.Decode(input, out)`: strict typing, no hooks.
///
/// # Errors
///
/// Go's decoding error.
pub fn decode<T: MsDecode + ?Sized>(input: &Value, out: &mut T) -> Result<(), DecodeError> {
    Decoder::new(DecoderConfig::default()).decode(input, out)
}

/// Go: `mapstructure.WeakDecode(input, out)`.
///
/// # Errors
///
/// Go's decoding error.
pub fn weak_decode<T: MsDecode + ?Sized>(input: &Value, out: &mut T) -> Result<(), DecodeError> {
    let config = DecoderConfig {
        weakly_typed_input: true,
        ..DecoderConfig::default()
    };
    Decoder::new(config).decode(input, out)
}

/// A source value by its `getKind` class.
enum Class<'a> {
    /// `reflect.Bool`.
    Bool(bool),
    /// The signed integer kinds.
    Int(i64),
    /// The unsigned integer kinds.
    Uint(u64),
    /// The float kinds.
    Float(f64),
    /// `reflect.String`.
    String(&'a str),
    /// Any other kind.
    Other,
}

/// Go: `getKind(reflect.Indirect(reflect.ValueOf(data)))`, with the value.
fn class(v: &Value) -> Class<'_> {
    match v {
        Value::Bool(b) => Class::Bool(*b),
        Value::Int(i) | Value::Int64(i) => Class::Int(*i),
        Value::Int8(i) => Class::Int((*i).into()),
        Value::Int16(i) => Class::Int((*i).into()),
        Value::Int32(i) => Class::Int((*i).into()),
        Value::Duration(d) => Class::Int(d.0),
        Value::Uint(u) | Value::Uint64(u) => Class::Uint(*u),
        Value::Uint8(u) => Class::Uint((*u).into()),
        Value::Uint16(u) => Class::Uint((*u).into()),
        Value::Uint32(u) => Class::Uint((*u).into()),
        Value::Float32(f) => Class::Float((*f).into()),
        Value::Float64(f) => Class::Float(*f),
        Value::String(s) => Class::String(s),
        Value::Nil | Value::Time(_) | Value::Slice(..) | Value::StringMap(..) | Value::Map(..) => {
            Class::Other
        }
    }
}

/// Go's `int64(u)` for a `uint64`.
const fn wrap_int(u: u64) -> i64 {
    i64::from_ne_bytes(u.to_ne_bytes())
}

/// Go's `uint64(i)` for an `int64`.
const fn wrap_uint(i: i64) -> u64 {
    u64::from_ne_bytes(i.to_ne_bytes())
}

/// Go: `SetInt` on a `bits`-bit integer: the low bits, sign-extended.
const fn truncate_int(v: i64, bits: u32) -> i64 {
    if bits >= 64 {
        return v;
    }
    let shift = 64 - bits;
    v.wrapping_shl(shift).wrapping_shr(shift)
}

/// Go: `SetUint` on a `bits`-bit unsigned integer: the low bits.
const fn truncate_uint(v: u64, bits: u32) -> u64 {
    if bits >= 64 {
        return v;
    }
    v & ((1_u64 << bits) - 1)
}

/// The kind name of a value (Go: `dataVal.Kind()`; `invalid` for nil).
fn value_kind_name(v: &Value) -> &'static str {
    v.kind().map_or("invalid", kind_name)
}

/// Go: `reflect.Value.String()` of a map key: the string itself for a `string` key type,
/// otherwise `<T Value>` for the key type `T`.
fn key_string(key_type: &Type, key: &Value) -> String {
    if key_type.kind() == GoKind::String
        && let Value::String(s) = key
    {
        return s.clone();
    }
    format!("<{} Value>", key_type.name())
}

/// Go: `decodePtr`'s nil test of the data: a nil interface, map or slice.
const fn is_nil_data(v: &Value) -> bool {
    matches!(
        v,
        Value::Nil | Value::Slice(_, None) | Value::StringMap(_, None) | Value::Map(_, _, None)
    )
}

/// The entries of a map value.
struct MapEntries<'a> {
    /// The map's key type.
    key_type: &'static Type,
    /// Whether the map is nil.
    nil: bool,
    /// The entries, keys as `interface{}` values.
    pairs: Vec<(Value, &'a Value)>,
}

/// The entries of `data` if it is a map.
fn map_entries(data: &Value) -> Option<MapEntries<'_>> {
    match data {
        Value::StringMap(_, m) => Some(MapEntries {
            key_type: &Type::String,
            nil: m.is_none(),
            pairs: m
                .iter()
                .flatten()
                .map(|(k, v)| (Value::String(k.clone()), v))
                .collect(),
        }),
        Value::Map(key_type, _, m) => Some(MapEntries {
            key_type,
            nil: m.is_none(),
            pairs: m.iter().flatten().map(|(k, v)| (k.clone(), v)).collect(),
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
        | Value::Slice(..) => None,
    }
}

/// Whether `data` is a map without entries (Go: `dataVal.Len() == 0` for a map kind).
fn is_empty_map(data: &Value) -> bool {
    if let Value::StringMap(_, m) = data {
        return m.as_ref().is_none_or(BTreeMap::is_empty);
    }
    matches!(data, Value::Map(_, _, m) if m.as_ref().is_none_or(Vec::is_empty))
}

/// The `'name' expected a map, got 'kind'` error.
fn expected_map(name: &str, data: &Value) -> DecodeError {
    DecodeError::Message(format!(
        "'{name}' expected a map, got '{}'",
        value_kind_name(data)
    ))
}

/// Go's `*T` around a copy of an `interface{}`'s dynamic value (`decodeBasic`): the hook sees
/// the pointer type, and nil data leaves the value (the pointer is not settable).
struct DynPtr<T>(T);

impl<T: MsDecode> MsDecode for DynPtr<T> {
    const MS_KIND: GoKind = GoKind::Ptr;

    fn ms_type_name(out: &mut String) {
        out.push('*');
        T::ms_type_name(out);
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        if is_nil_data(input) {
            return Ok(());
        }
        d.decode_named(name, input, &mut self.0)
    }

    fn ms_zero(&mut self) {
        self.0.ms_zero();
    }
}

/// Go's `int`, as an `interface{}` holds it.
#[derive(Default)]
struct GoInt(i64);

impl MsDecode for GoInt {
    const MS_KIND: GoKind = GoKind::Int;

    fn ms_type_name(out: &mut String) {
        out.push_str("int");
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        self.0 = d.decode_int(name, input, 64, Self::ms_type_name)?;
        Ok(())
    }

    fn ms_zero(&mut self) {
        self.0 = 0;
    }
}

/// Go's `uint`, as an `interface{}` holds it.
#[derive(Default)]
struct GoUint(u64);

impl MsDecode for GoUint {
    const MS_KIND: GoKind = GoKind::Uint;

    fn ms_type_name(out: &mut String) {
        out.push_str("uint");
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        self.0 = d.decode_uint(name, input, 64, Self::ms_type_name)?;
        Ok(())
    }

    fn ms_zero(&mut self) {
        self.0 = 0;
    }
}

impl Decoder {
    /// Go: `mapstructure.NewDecoder(config)`.
    #[must_use]
    pub const fn new(config: DecoderConfig) -> Self {
        Self { config }
    }

    /// Go: `Decoder.Decode(input)` into `out`.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode<T: MsDecode + ?Sized>(
        &mut self,
        input: &Value,
        out: &mut T,
    ) -> Result<(), DecodeError> {
        self.decode_named("", input, out)
    }

    /// Go: `d.decode(name, input, outVal)`: the nil check, the decode hook, and the decoder of
    /// the target's kind.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_named<T: MsDecode + ?Sized>(
        &mut self,
        name: &str,
        input: &Value,
        out: &mut T,
    ) -> Result<(), DecodeError> {
        if input.is_nil() {
            // If the data is nil, then we don't set anything, unless ZeroFields is set.
            if self.config.zero_fields {
                out.ms_zero();
            }
            return Ok(());
        }
        let hooked = self.run_hook(HookTarget::of::<T>(), name, input)?;
        out.ms_decode(self, name, hooked.as_ref().unwrap_or(input))
    }

    /// Go: `DecodeHookExec` in `decode`: the hook's replacement data, if any.
    fn run_hook(
        &self,
        to: HookTarget,
        name: &str,
        input: &Value,
    ) -> Result<Option<Value>, DecodeError> {
        self.config.decode_hook.as_ref().map_or(Ok(None), |hook| {
            hook(to, input)
                .map_err(|e| DecodeError::Message(format!("error decoding '{name}': {e}")))
        })
    }

    /// The `unconvertible type` error of the basic decoders, for the target type `ty`.
    fn unconvertible(name: &str, data: &Value, ty: fn(&mut String)) -> DecodeError {
        let mut target = String::new();
        ty(&mut target);
        DecodeError::Message(format!(
            "'{name}' expected type '{target}', got unconvertible type '{}', value: '{}'",
            data.type_name(),
            sprintf("%v", &[data.to_fmt()])
        ))
    }

    /// Go: `decodeBool` (`ty` names the target type in errors).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_bool(
        &self,
        name: &str,
        data: &Value,
        ty: fn(&mut String),
    ) -> Result<bool, DecodeError> {
        let weak = self.config.weakly_typed_input;
        match class(data) {
            Class::Bool(b) => Ok(b),
            Class::Int(i) if weak => Ok(i != 0),
            Class::Uint(u) if weak => Ok(u != 0),
            Class::Float(f) if weak => Ok(f.partial_cmp(&0.0).is_none_or(Ordering::is_ne)),
            Class::String(s) if weak => match strconv::parse_bool(s) {
                Ok(b) => Ok(b),
                Err(_) if s.is_empty() => Ok(false),
                Err(e) => Err(DecodeError::Message(format!(
                    "cannot parse '{name}' as bool: {e}"
                ))),
            },
            Class::Int(_) | Class::Uint(_) | Class::Float(_) | Class::String(_) | Class::Other => {
                Err(Self::unconvertible(name, data, ty))
            }
        }
    }

    /// Go: `decodeInt` into a `bits`-bit integer, with `SetInt`'s truncation.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_int(
        &self,
        name: &str,
        data: &Value,
        bits: u32,
        ty: fn(&mut String),
    ) -> Result<i64, DecodeError> {
        let weak = self.config.weakly_typed_input;
        let v = match class(data) {
            Class::Int(i) => i,
            Class::Uint(u) => wrap_int(u),
            Class::Float(f) => math::int64(f),
            Class::Bool(b) if weak => i64::from(b),
            Class::String(s) if weak => {
                let s = if s.is_empty() { "0" } else { s };
                strconv::parse_int(s, 0, bits.into()).map_err(|e| {
                    DecodeError::Message(format!("cannot parse '{name}' as int: {e}"))
                })?
            }
            Class::Bool(_) | Class::String(_) | Class::Other => {
                return Err(Self::unconvertible(name, data, ty));
            }
        };
        Ok(truncate_int(v, bits))
    }

    /// Go: `decodeUint` into a `bits`-bit unsigned integer, with `SetUint`'s truncation.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_uint(
        &self,
        name: &str,
        data: &Value,
        bits: u32,
        ty: fn(&mut String),
    ) -> Result<u64, DecodeError> {
        let weak = self.config.weakly_typed_input;
        let v = match class(data) {
            Class::Int(i) => {
                if i < 0 && !weak {
                    return Err(DecodeError::Message(format!(
                        "cannot parse '{name}', {i} overflows uint"
                    )));
                }
                wrap_uint(i)
            }
            Class::Uint(u) => u,
            Class::Float(f) => {
                if f < 0.0 && !weak {
                    return Err(DecodeError::Message(format!(
                        "cannot parse '{name}', {} overflows uint",
                        strconv::format_float(f, b'f', 6, 64)
                    )));
                }
                go_uint64(f)
            }
            Class::Bool(b) if weak => u64::from(b),
            Class::String(s) if weak => {
                let s = if s.is_empty() { "0" } else { s };
                strconv::parse_uint(s, 0, bits.into()).map_err(|e| {
                    DecodeError::Message(format!("cannot parse '{name}' as uint: {e}"))
                })?
            }
            Class::Bool(_) | Class::String(_) | Class::Other => {
                return Err(Self::unconvertible(name, data, ty));
            }
        };
        Ok(truncate_uint(v, bits))
    }

    /// Go: `decodeFloat` into a `bits`-bit float (the caller converts to `float32`).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_float(
        &self,
        name: &str,
        data: &Value,
        bits: u32,
        ty: fn(&mut String),
    ) -> Result<f64, DecodeError> {
        let weak = self.config.weakly_typed_input;
        match class(data) {
            Class::Int(i) => Ok(int_to_float(i)),
            Class::Uint(u) => Ok(uint_to_float(u)),
            Class::Float(f) => Ok(f),
            Class::Bool(b) if weak => Ok(if b { 1.0 } else { 0.0 }),
            Class::String(s) if weak => {
                let s = if s.is_empty() { "0" } else { s };
                strconv::parse_float(s, bits.into()).map_err(|e| {
                    DecodeError::Message(format!("cannot parse '{name}' as float: {e}"))
                })
            }
            Class::Bool(_) | Class::String(_) | Class::Other => {
                Err(Self::unconvertible(name, data, ty))
            }
        }
    }

    /// Go: `decodeString`, as the string's bytes.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_string_bytes(
        &self,
        name: &str,
        data: &Value,
        ty: fn(&mut String),
    ) -> Result<Vec<u8>, DecodeError> {
        let weak = self.config.weakly_typed_input;
        let text = match class(data) {
            Class::String(s) => s.to_owned(),
            Class::Bool(b) if weak => if b { "1" } else { "0" }.to_owned(),
            Class::Int(i) if weak => strconv::format_int(i, 10),
            Class::Uint(u) if weak => strconv::format_uint(u, 10),
            Class::Float(f) if weak => strconv::format_float(f, b'f', -1, 64),
            Class::Bool(_) | Class::Int(_) | Class::Uint(_) | Class::Float(_) | Class::Other => {
                if weak && let Value::Slice(Type::Uint8, items) = data {
                    return Ok(items
                        .iter()
                        .flatten()
                        .map(|v| if let Value::Uint8(b) = v { *b } else { 0 })
                        .collect());
                }
                return Err(Self::unconvertible(name, data, ty));
            }
        };
        Ok(text.into_bytes())
    }

    /// Go: `decodeString` (invalid UTF-8 from a `[]uint8` source is replaced).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_string(
        &self,
        name: &str,
        data: &Value,
        ty: fn(&mut String),
    ) -> Result<String, DecodeError> {
        let bytes = self.decode_string_bytes(name, data, ty)?;
        Ok(String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    }

    /// Go: `decodeBasic` into an `interface{}`.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_basic(
        &mut self,
        name: &str,
        data: &Value,
        out: &mut Value,
    ) -> Result<(), DecodeError> {
        // Go: a non-nil interface's dynamic value is copied, decoded into through a pointer,
        // and stored back; a nil interface takes the data as is.
        match out {
            Value::Bool(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Int(v) => *v = self.decode_dyn(name, data, GoInt(*v))?.0,
            Value::Int8(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Int16(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Int32(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Int64(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Uint(v) => *v = self.decode_dyn(name, data, GoUint(*v))?.0,
            Value::Uint8(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Uint16(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Uint32(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Uint64(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Float32(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Float64(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::String(v) => *v = self.decode_dyn(name, data, mem::take(v))?,
            Value::Duration(v) => *v = self.decode_dyn(name, data, *v)?,
            Value::Time(v) => *v = self.decode_dyn(name, data, mem::take(v))?,
            Value::Nil | Value::Slice(..) | Value::StringMap(..) | Value::Map(..) => {
                *out = data.clone();
            }
        }
        Ok(())
    }

    /// Decodes into a copy of a dynamic value through Go's unsettable pointer to it.
    fn decode_dyn<T: MsDecode>(
        &mut self,
        name: &str,
        data: &Value,
        value: T,
    ) -> Result<T, DecodeError> {
        let mut ptr = DynPtr(value);
        self.decode_named(name, data, &mut ptr)?;
        Ok(ptr.0)
    }

    /// Go: `decodePtr` into `slot` (`None` is a nil pointer).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_ptr<T: MsDecode + Default>(
        &mut self,
        name: &str,
        data: &Value,
        slot: &mut Option<T>,
    ) -> Result<(), DecodeError> {
        if is_nil_data(data) {
            *slot = None;
            return Ok(());
        }
        match slot {
            Some(inner) if !self.config.zero_fields => self.decode_named(name, data, inner),
            Some(_) | None => {
                let mut inner = T::default();
                self.decode_named(name, data, &mut inner)?;
                *slot = Some(inner);
                Ok(())
            }
        }
    }

    /// Go: `decodeSlice` into `items` (`None` is a nil slice).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_slice<E: MsDecode + Default>(
        &mut self,
        name: &str,
        data: &Value,
        items: &mut Option<Vec<E>>,
    ) -> Result<(), DecodeError> {
        let source: Cow<'_, [Value]> = if let Value::Slice(_, list) = data {
            // If the input value is nil, then don't allocate since empty != nil.
            let Some(list) = list else {
                return Ok(());
            };
            Cow::Borrowed(list)
        } else if !self.config.weakly_typed_input {
            return Err(DecodeError::Message(format!(
                "'{name}': source data must be an array or slice, got {}",
                value_kind_name(data)
            )));
        } else if is_empty_map(data) {
            // Empty maps turn into empty slices.
            *items = Some(Vec::new());
            return Ok(());
        } else if let (Value::String(s), GoKind::Uint8) = (data, E::MS_KIND) {
            Cow::Owned(s.bytes().map(Value::Uint8).collect())
        } else {
            // All other types are lifted into a one-element slice.
            Cow::Owned(vec![data.clone()])
        };
        if self.config.zero_fields {
            *items = None;
        }
        let target = items.get_or_insert_with(Vec::new);
        let mut errors = Vec::new();
        for (i, item) in source.iter().enumerate() {
            while target.len() <= i {
                target.push(E::default());
            }
            if let Some(slot) = target.get_mut(i)
                && let Err(e) = self.decode_named(&format!("{name}[{i}]"), item, slot)
            {
                e.append_to(&mut errors)?;
            }
        }
        DecodeError::collect(errors)
    }

    /// Go: `decodeMap` into `out` (`None` is a nil map).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_map<M: MsMap + Default>(
        &mut self,
        name: &str,
        data: &Value,
        out: &mut Option<M>,
    ) -> Result<(), DecodeError> {
        self.decode_map_as(name, data, out, HookTarget::of::<M>())
    }

    /// [`Decoder::decode_map`] into a map of the (possibly named) type `ty`, which the hook sees
    /// when `decodeMapFromSlice` decodes each item into the map itself.
    fn decode_map_as<M: MsMap + Default>(
        &mut self,
        name: &str,
        data: &Value,
        out: &mut Option<M>,
        ty: HookTarget,
    ) -> Result<(), DecodeError> {
        let fresh = out.is_none() || self.config.zero_fields;
        if let Some(entries) = map_entries(data) {
            return self.decode_map_from_map(name, &entries, out, fresh);
        }
        let (Value::Slice(_, items), true) = (data, self.config.weakly_typed_input) else {
            return Err(expected_map(name, data));
        };
        let items = items.as_deref().unwrap_or_default();
        if items.is_empty() {
            if fresh {
                *out = Some(M::default());
            }
            return Ok(());
        }
        // Go decodes each element into the map itself (`d.decode(name[i], item, val)`), stopping
        // at the first error.
        for (i, item) in items.iter().enumerate() {
            let item_name = format!("{name}[{i}]");
            if item.is_nil() {
                if self.config.zero_fields {
                    *out = None;
                }
                continue;
            }
            let hooked = self.run_hook(ty, &item_name, item)?;
            self.decode_map_as(&item_name, hooked.as_ref().unwrap_or(item), out, ty)?;
        }
        Ok(())
    }

    /// Go: `decodeMapFromMap`.
    fn decode_map_from_map<M: MsMap + Default>(
        &mut self,
        name: &str,
        entries: &MapEntries<'_>,
        out: &mut Option<M>,
        fresh: bool,
    ) -> Result<(), DecodeError> {
        if entries.pairs.is_empty() {
            // If the input data is empty, then we just match what the input data is.
            if entries.nil {
                *out = None;
            } else if fresh {
                *out = Some(M::default());
            }
            return Ok(());
        }
        let map = if fresh {
            out.insert(M::default())
        } else {
            out.get_or_insert_with(M::default)
        };
        let mut errors = Vec::new();
        for (k, v) in &entries.pairs {
            let field = format!("{name}[{}]", key_string(entries.key_type, k));
            if let Err(e) = map.ms_map_entry(self, &field, k, v) {
                e.append_to(&mut errors)?;
            }
        }
        DecodeError::collect(errors)
    }

    /// Go: `d.decodeMap(name, remain, field)` for a `,remain` field: no nil check and no hook.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_remain<T: MsDecode + ?Sized>(
        &mut self,
        name: &str,
        data: &Value,
        out: &mut T,
    ) -> Result<(), DecodeError> {
        out.ms_decode(self, name, data)
    }

    /// Go: `decodeStruct` into a struct.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    pub fn decode_struct<S: MsStruct + ?Sized>(
        &mut self,
        name: &str,
        data: &Value,
        out: &mut S,
    ) -> Result<(), DecodeError> {
        let Some(entries) = map_entries(data) else {
            return Err(expected_map(name, data));
        };
        if !matches!(entries.key_type.kind(), GoKind::String | GoKind::Interface) {
            return Err(DecodeError::Message(format!(
                "'{name}' needs a map with string keys, has '{}' keys",
                kind_name(entries.key_type.kind())
            )));
        }
        let mut errors = Vec::new();
        let (fields, remain) = Self::struct_fields(S::ms_fields(), &mut errors);
        let mut unused: Vec<&Value> = entries.pairs.iter().map(|(k, _)| k).collect();
        for (path, field) in fields {
            let field_name = if field.tag.is_empty() {
                field.name
            } else {
                field.tag
            };
            let found = entries
                .pairs
                .iter()
                .find(|(k, _)| matches!(k, Value::String(s) if s == field_name))
                .or_else(|| {
                    entries
                        .pairs
                        .iter()
                        .find(|(k, _)| matches!(k, Value::String(s) if equal_fold(s, field_name)))
                });
            // If we can't set the field (unexported), then we just continue onwards.
            let Some((key, value)) = found.filter(|_| field.exported) else {
                continue;
            };
            unused.retain(|k| !k.go_eq(key));
            let full = if name.is_empty() {
                field_name.to_owned()
            } else {
                format!("{name}.{field_name}")
            };
            if let Err(e) = out.ms_field(self, &path, &full, value) {
                e.append_to(&mut errors)?;
            }
        }
        if let Some(path) = remain
            && !unused.is_empty()
        {
            let rest = entries
                .pairs
                .iter()
                .filter(|(k, _)| unused.iter().any(|u| u.go_eq(k)))
                .map(|(k, v)| (k.clone(), (*v).clone()))
                .collect();
            let rest = Value::Map(&Type::Interface, &Type::Interface, Some(rest));
            if let Err(e) = out.ms_field(self, &path, name, &rest) {
                e.append_to(&mut errors)?;
            }
            unused.clear();
        }
        if self.config.error_unused && !unused.is_empty() {
            let mut keys = Vec::with_capacity(unused.len());
            for key in &unused {
                // Go asserts every key to a string (`rawKey.(string)`).
                let Some(key) = key.as_str() else {
                    let dynamic = if key.is_nil() {
                        "nil".to_owned()
                    } else {
                        key.type_name()
                    };
                    return Err(DecodeError::Panic(format!(
                        "interface conversion: interface {{}} is {dynamic}, not string"
                    )));
                };
                keys.push(key);
            }
            keys.sort_unstable();
            errors.push(format!("'{name}' has invalid keys: {}", keys.join(", ")));
        }
        DecodeError::collect(errors)
    }

    /// The fields of a struct and, breadth first, of its squashed structs, with the `,remain`
    /// field (Go: the `structs` loop of `decodeStructFromMap`).
    fn struct_fields(top: &'static [MsField], errors: &mut Vec<String>) -> StructFields {
        let mut fields = Vec::new();
        let mut remain = None;
        let mut queue: Vec<(Vec<usize>, &'static [MsField])> = vec![(Vec::new(), top)];
        let mut head = 0;
        while let Some((prefix, list)) = queue.get(head).cloned() {
            head += 1;
            for (i, field) in list.iter().enumerate() {
                let mut path = prefix.clone();
                path.push(i);
                if field.squash {
                    match field.squash_fields {
                        Some(inner) => queue.push((path, inner())),
                        None => errors.push(format!(
                            "{}: unsupported type for squash: {}",
                            field.name,
                            kind_name(field.kind)
                        )),
                    }
                } else if field.remain {
                    remain = Some(path);
                } else {
                    fields.push((path, field));
                }
            }
        }
        (fields, remain)
    }
}

/// The fields `decodeStructFromMap` decodes, by their path (indexes through squashed structs),
/// and the path of the `,remain` field.
type StructFields = (Vec<(Vec<usize>, &'static MsField)>, Option<Vec<usize>>);

/// A Rust type mapstructure can decode into, standing for a Go type.
///
/// Implemented for the types of the [`crate::yaml`] table, and by
/// [`#[derive(GoMapstructure)]`](crate::GoMapstructure) for structs and newtypes. The
/// `#[doc(hidden)]` methods are the decoder's entry points.
pub trait MsDecode {
    /// Go's `reflect.Kind` of the type.
    const MS_KIND: GoKind;
    /// Whether `Option<Self>` stands for `*Self` (slices and maps make it a nil-able value of
    /// the same kind instead).
    #[doc(hidden)]
    const MS_OPTION_IS_PTR: bool = true;

    /// Appends Go's `reflect.Type.String()` of the type.
    fn ms_type_name(out: &mut String);

    /// The decoder of the target's kind, after the nil check and the hook of Go's `decode`.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    #[doc(hidden)]
    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError>;

    /// [`MsDecode::ms_decode`] for a named type over this one, which `ty` names in errors.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    #[doc(hidden)]
    fn ms_decode_as(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
        ty: fn(&mut String),
    ) -> Result<(), DecodeError> {
        let _ = ty;
        self.ms_decode(d, name, input)
    }

    /// Go: `outVal.Set(reflect.Zero(outVal.Type()))`.
    #[doc(hidden)]
    fn ms_zero(&mut self);

    /// Decodes a struct field: a non-nil pointer to a struct is decoded in place (Go
    /// dereferences it before calling `decode`).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    #[doc(hidden)]
    fn ms_decode_field(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        d.decode_named(name, input, self)
    }

    /// Decodes into `Option<Self>`: Go's `decodePtr` for a pointer; slices and maps decode
    /// into their nil-able value.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    #[doc(hidden)]
    fn ms_decode_option(
        slot: &mut Option<Self>,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError>
    where
        Self: Sized + Default,
    {
        d.decode_ptr(name, input, slot)
    }

    /// [`MsDecode::ms_decode_option`] for a named type over `Option<Self>`, which `ty` names:
    /// `decodeMapFromSlice` decodes each item into the map itself, so the hook sees the named
    /// type.
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    #[doc(hidden)]
    fn ms_decode_option_as(
        slot: &mut Option<Self>,
        d: &mut Decoder,
        name: &str,
        input: &Value,
        ty: fn(&mut String),
    ) -> Result<(), DecodeError>
    where
        Self: Sized + Default,
    {
        let _ = ty;
        Self::ms_decode_option(slot, d, name, input)
    }
}

/// A map target of `decodeMap`.
pub trait MsMap: MsDecode {
    /// Decodes one entry into a new key and element and inserts it (Go: an iteration of
    /// `decodeMapFromMap`).
    ///
    /// # Errors
    ///
    /// Go's decoding error of the key or the element.
    #[doc(hidden)]
    fn ms_map_entry(
        &mut self,
        d: &mut Decoder,
        name: &str,
        key: &Value,
        value: &Value,
    ) -> Result<(), DecodeError>;
}

/// A struct field, as [`#[derive(GoMapstructure)]`](crate::GoMapstructure) describes it.
#[derive(Clone, Copy, Debug)]
pub struct MsField {
    /// Go's field name.
    pub name: &'static str,
    /// The name part of the `mapstructure` tag (empty when absent).
    pub tag: &'static str,
    /// Whether the tag has `,squash`.
    pub squash: bool,
    /// Whether the tag has `,remain`.
    pub remain: bool,
    /// Whether the Go field is exported (settable).
    pub exported: bool,
    /// Go's kind of the field, for the squash error.
    pub kind: GoKind,
    /// The fields of a struct-typed field that can be squashed.
    pub squash_fields: Option<fn() -> &'static [Self]>,
}

/// A struct decoded from a map (Go: `decodeStructFromMap`), implemented by
/// [`#[derive(GoMapstructure)]`](crate::GoMapstructure).
pub trait MsStruct: MsDecode {
    /// The struct's fields in declaration order.
    fn ms_fields() -> &'static [MsField];

    /// Decodes `input` into the field at `path` (indexes through squashed structs).
    ///
    /// # Errors
    ///
    /// Go's decoding error.
    fn ms_field(
        &mut self,
        d: &mut Decoder,
        path: &[usize],
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError>;
}

/// Integer impls: Go kind, name, bit size, and decoder.
macro_rules! ms_int_impls {
    ($($ty:ty => $kind:ident, $name:literal, $bits:literal, $method:ident;)*) => {
        $(
            impl MsDecode for $ty {
                const MS_KIND: GoKind = GoKind::$kind;

                fn ms_type_name(out: &mut String) {
                    out.push_str($name);
                }

                fn ms_decode(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                ) -> Result<(), DecodeError> {
                    self.ms_decode_as(d, name, input, Self::ms_type_name)
                }

                fn ms_decode_as(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                    ty: fn(&mut String),
                ) -> Result<(), DecodeError> {
                    let v = d.$method(name, input, $bits, ty)?;
                    *self = <$ty>::try_from(v).unwrap_or_default();
                    Ok(())
                }

                fn ms_zero(&mut self) {
                    *self = 0;
                }
            }
        )*
    };
}

ms_int_impls! {
    i8 => Int8, "int8", 8, decode_int;
    i16 => Int16, "int16", 16, decode_int;
    i32 => Int32, "int32", 32, decode_int;
    i64 => Int64, "int64", 64, decode_int;
    isize => Int, "int", 64, decode_int;
    u8 => Uint8, "uint8", 8, decode_uint;
    u16 => Uint16, "uint16", 16, decode_uint;
    u32 => Uint32, "uint32", 32, decode_uint;
    u64 => Uint64, "uint64", 64, decode_uint;
    usize => Uint, "uint", 64, decode_uint;
}

/// Impls of the remaining basic types: Go kind, name, and the decoding of `$v` from `$d`.
macro_rules! ms_basic_impls {
    ($(
        $ty:ty => $kind:ident,
        $name:literal,
        |$d:ident, $n:ident, $input:ident, $t:ident| $decode:expr,
        $zero:expr;
    )*) => {
        $(
            impl MsDecode for $ty {
                const MS_KIND: GoKind = GoKind::$kind;

                fn ms_type_name(out: &mut String) {
                    out.push_str($name);
                }

                fn ms_decode(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                ) -> Result<(), DecodeError> {
                    self.ms_decode_as(d, name, input, Self::ms_type_name)
                }

                fn ms_decode_as(
                    &mut self,
                    $d: &mut Decoder,
                    $n: &str,
                    $input: &Value,
                    $t: fn(&mut String),
                ) -> Result<(), DecodeError> {
                    *self = $decode;
                    Ok(())
                }

                fn ms_zero(&mut self) {
                    *self = $zero;
                }
            }
        )*
    };
}

ms_basic_impls! {
    bool => Bool, "bool", |d, name, input, ty| d.decode_bool(name, input, ty)?, false;
    f64 => Float64, "float64", |d, name, input, ty| d.decode_float(name, input, 64, ty)?, 0.0;
    f32 => Float32, "float32", |d, name, input, ty| {
        math::float32(d.decode_float(name, input, 32, ty)?)
    }, 0.0;
    String => String, "string", |d, name, input, ty| d.decode_string(name, input, ty)?, Self::new();
    GoString => String, "string", |d, name, input, ty| {
        Self::from_bytes(d.decode_string_bytes(name, input, ty)?)
    }, Self::new();
    Duration => Int64, "time.Duration", |d, name, input, ty| {
        Self(d.decode_int(name, input, 64, ty)?)
    }, Self(0);
}

impl MsDecode for Time {
    const MS_KIND: GoKind = GoKind::Struct;

    fn ms_type_name(out: &mut String) {
        out.push_str("time.Time");
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        if let Value::Time(t) = input {
            *self = t.clone();
            return Ok(());
        }
        d.decode_struct(name, input, self)
    }

    fn ms_zero(&mut self) {
        *self = Self::default();
    }
}

impl MsStruct for Time {
    fn ms_fields() -> &'static [MsField] {
        // Go: time.Time has only unexported fields.
        &[]
    }

    fn ms_field(
        &mut self,
        d: &mut Decoder,
        path: &[usize],
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        let _ = (d, path, name, input);
        Ok(())
    }
}

impl MsDecode for Value {
    const MS_KIND: GoKind = GoKind::Interface;

    fn ms_type_name(out: &mut String) {
        out.push_str("interface {}");
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        d.decode_basic(name, input, self)
    }

    fn ms_zero(&mut self) {
        *self = Self::Nil;
    }
}

/// Go `json.RawMessage`, a `[]byte` (`Option<RawJson>` is a `*json.RawMessage`).
impl MsDecode for RawJson {
    const MS_KIND: GoKind = GoKind::Slice;

    fn ms_type_name(out: &mut String) {
        out.push_str("json.RawMessage");
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        // Go: decodeSlice, as for any []byte.
        let mut bytes = (!self.is_nil()).then(|| self.as_bytes().to_vec());
        let result = d.decode_slice::<u8>(name, input, &mut bytes);
        *self = bytes.map_or_else(Self::nil, Self::from_vec);
        result
    }

    fn ms_zero(&mut self) {
        *self = Self::nil();
    }
}

impl<T: MsDecode + Default> MsDecode for Vec<T> {
    const MS_KIND: GoKind = GoKind::Slice;
    const MS_OPTION_IS_PTR: bool = false;

    fn ms_type_name(out: &mut String) {
        out.push_str("[]");
        T::ms_type_name(out);
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        let mut items = Some(mem::take(self));
        let result = d.decode_slice(name, input, &mut items);
        *self = items.unwrap_or_default();
        result
    }

    fn ms_zero(&mut self) {
        self.clear();
    }

    fn ms_decode_option(
        slot: &mut Option<Self>,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        d.decode_slice(name, input, slot)
    }
}

/// The `MsDecode` and `MsMap` impls of a map type.
macro_rules! ms_map_impls {
    ($($map:ident<$($param:ident),*> where [$($bound:tt)*];)*) => {
        $(
            impl<$($param),*> MsDecode for $map<$($param),*>
            where
                $($bound)*
            {
                const MS_KIND: GoKind = GoKind::Map;
                const MS_OPTION_IS_PTR: bool = false;

                fn ms_type_name(out: &mut String) {
                    out.push_str("map[");
                    K::ms_type_name(out);
                    out.push(']');
                    V::ms_type_name(out);
                }

                fn ms_decode(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                ) -> Result<(), DecodeError> {
                    self.ms_decode_as(d, name, input, Self::ms_type_name)
                }

                fn ms_decode_as(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                    ty: fn(&mut String),
                ) -> Result<(), DecodeError> {
                    let mut slot = Some(mem::take(self));
                    let result = d.decode_map_as(name, input, &mut slot, HookTarget::map(ty));
                    *self = slot.unwrap_or_default();
                    result
                }

                fn ms_zero(&mut self) {
                    self.clear();
                }

                fn ms_decode_option(
                    slot: &mut Option<Self>,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                ) -> Result<(), DecodeError> {
                    d.decode_map(name, input, slot)
                }

                fn ms_decode_option_as(
                    slot: &mut Option<Self>,
                    d: &mut Decoder,
                    name: &str,
                    input: &Value,
                    ty: fn(&mut String),
                ) -> Result<(), DecodeError> {
                    d.decode_map_as(name, input, slot, HookTarget::map(ty))
                }
            }

            impl<$($param),*> MsMap for $map<$($param),*>
            where
                $($bound)*
            {
                fn ms_map_entry(
                    &mut self,
                    d: &mut Decoder,
                    name: &str,
                    key: &Value,
                    value: &Value,
                ) -> Result<(), DecodeError> {
                    let mut k = K::default();
                    d.decode_named(name, key, &mut k)?;
                    let mut v = V::default();
                    d.decode_named(name, value, &mut v)?;
                    let _previous = self.insert(k, v);
                    Ok(())
                }
            }
        )*
    };
}

ms_map_impls! {
    BTreeMap<K, V> where [K: MsDecode + Default + Ord, V: MsDecode + Default];
    HashMap<K, V, S> where [
        K: MsDecode + Default + Eq + Hash,
        V: MsDecode + Default,
        S: BuildHasher + Default,
    ];
}

impl<T: MsDecode + Default> MsDecode for Option<T> {
    const MS_KIND: GoKind = if T::MS_OPTION_IS_PTR {
        GoKind::Ptr
    } else {
        T::MS_KIND
    };

    fn ms_type_name(out: &mut String) {
        if T::MS_OPTION_IS_PTR {
            out.push('*');
        }
        T::ms_type_name(out);
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        T::ms_decode_option(self, d, name, input)
    }

    fn ms_decode_as(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
        ty: fn(&mut String),
    ) -> Result<(), DecodeError> {
        if T::MS_OPTION_IS_PTR {
            return self.ms_decode(d, name, input);
        }
        T::ms_decode_option_as(self, d, name, input, ty)
    }

    fn ms_zero(&mut self) {
        *self = None;
    }

    fn ms_decode_field(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        // Go: decodeStructFromMap handles embedded struct pointers as embedded structs.
        if T::MS_OPTION_IS_PTR
            && T::MS_KIND == GoKind::Struct
            && let Some(inner) = self
        {
            return d.decode_named(name, input, inner);
        }
        d.decode_named(name, input, self)
    }
}

impl<T: MsDecode + Default> MsDecode for Box<T> {
    const MS_KIND: GoKind = T::MS_KIND;
    const MS_OPTION_IS_PTR: bool = T::MS_OPTION_IS_PTR;

    fn ms_type_name(out: &mut String) {
        T::ms_type_name(out);
    }

    fn ms_decode(&mut self, d: &mut Decoder, name: &str, input: &Value) -> Result<(), DecodeError> {
        (**self).ms_decode(d, name, input)
    }

    fn ms_decode_as(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
        ty: fn(&mut String),
    ) -> Result<(), DecodeError> {
        (**self).ms_decode_as(d, name, input, ty)
    }

    fn ms_zero(&mut self) {
        (**self).ms_zero();
    }

    fn ms_decode_field(
        &mut self,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        (**self).ms_decode_field(d, name, input)
    }

    fn ms_decode_option(
        slot: &mut Option<Self>,
        d: &mut Decoder,
        name: &str,
        input: &Value,
    ) -> Result<(), DecodeError> {
        if T::MS_OPTION_IS_PTR {
            return d.decode_ptr(name, input, slot);
        }
        let mut inner = slot.take().map(|b| *b);
        let result = T::ms_decode_option(&mut inner, d, name, input);
        *slot = inner.map(Self::new);
        result
    }
}
