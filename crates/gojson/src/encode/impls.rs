//! [`GoEncode`] for the primitive, container and dynamic types.

use core::slice;
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    hash::BuildHasher,
    vec,
};

use serde_json::{Map, Number, Value};

use super::{Encoder, Flavor, GoEncode};
use crate::{
    base64::encode_std, gostring::GoString, gotype::GoType, jsoniter::Iter, unicode::go_lossy,
};

macro_rules! int_encode {
    ($($ty:ty => $write:ident),* $(,)?) => {
        $(
            impl GoEncode for $ty {
                fn go_encode(&self, enc: &mut Encoder) {
                    enc.$write((*self).into());
                }
                fn go_is_empty(&self) -> bool {
                    *self == 0
                }
                fn go_encode_quoted(&self, enc: &mut Encoder) {
                    quoted(enc, |enc| self.go_encode(enc));
                }
            }
        )*
    };
}

int_encode!(
    i8 => write_i64,
    i16 => write_i64,
    i32 => write_i64,
    i64 => write_i64,
    u16 => write_u64,
    u32 => write_u64,
    u64 => write_u64,
);

impl GoEncode for isize {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_i64(i64::try_from(*self).unwrap_or_default());
    }

    fn go_is_empty(&self) -> bool {
        *self == 0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }
}

impl GoEncode for usize {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_u64(u64::try_from(*self).unwrap_or_default());
    }

    fn go_is_empty(&self) -> bool {
        *self == 0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }
}

impl GoEncode for u8 {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_u64((*self).into());
    }

    fn go_is_empty(&self) -> bool {
        *self == 0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }

    fn go_encode_slice(items: &[Self], enc: &mut Encoder) {
        // Go: []byte encodes as a base64 string in both libraries.
        let mut text = Vec::with_capacity(items.len().div_ceil(3) * 4 + 2);
        text.push(b'"');
        encode_std(items, &mut text);
        text.push(b'"');
        enc.write_raw(&text);
    }
}

/// `encoding/json` quoted (`,string`) encoding of a number or boolean.
fn quoted(enc: &mut Encoder, write: impl FnOnce(&mut Encoder)) {
    enc.write_byte(b'"');
    write(enc);
    enc.write_byte(b'"');
}

impl GoEncode for bool {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_bool(*self);
    }

    fn go_is_empty(&self) -> bool {
        !*self
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }
}

impl GoEncode for f64 {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_f64(*self);
    }

    fn go_is_empty(&self) -> bool {
        *self == 0.0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }
}

impl GoEncode for f32 {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_f32(*self);
    }

    fn go_is_empty(&self) -> bool {
        *self == 0.0
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted(enc, |enc| self.go_encode(enc));
    }
}

/// `encoding/json` quoted string: the escaped string, escaped again without HTML escaping.
fn quoted_string(enc: &mut Encoder, s: &[u8]) {
    // Go: encoding/json/encode.go stringEncoder (opts.quoted)
    let mut inner = Vec::with_capacity(s.len() + 2);
    super::std_string(&mut inner, s, enc.escape_html());
    let mut outer = Vec::with_capacity(inner.len() + 4);
    super::std_string(&mut outer, &inner, false);
    enc.write_raw(&outer);
}

impl GoEncode for str {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_str(self.as_bytes());
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted_string(enc, self.as_bytes());
    }
}

impl GoEncode for String {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_str(self.as_bytes());
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted_string(enc, self.as_bytes());
    }
}

impl GoEncode for GoString {
    fn go_encode(&self, enc: &mut Encoder) {
        enc.write_str(self.as_bytes());
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        quoted_string(enc, self.as_bytes());
    }
}

impl<T: GoEncode> GoEncode for Option<T> {
    fn go_encode(&self, enc: &mut Encoder) {
        // Go: jsoniter OptionalEncoder / encoding/json ptrEncoder
        T::option_go_encode(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        T::option_is_empty(self)
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        match self {
            None => enc.write_null(),
            Some(v) => v.go_encode_quoted(enc),
        }
    }
}

impl<T: GoEncode + ?Sized> GoEncode for Box<T> {
    fn go_encode(&self, enc: &mut Encoder) {
        (**self).go_encode(enc);
    }

    fn go_is_empty(&self) -> bool {
        (**self).go_is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        (**self).go_encode_quoted(enc);
    }
}

impl<T: GoEncode + ?Sized> GoEncode for &T {
    fn go_encode(&self, enc: &mut Encoder) {
        (**self).go_encode(enc);
    }

    fn go_is_empty(&self) -> bool {
        (**self).go_is_empty()
    }

    fn go_encode_quoted(&self, enc: &mut Encoder) {
        (**self).go_encode_quoted(enc);
    }
}

/// Encodes a non-nil Go slice: `[]` when empty, elements separated by commas, and (jsoniter)
/// any error wrapped with the slice type.
pub fn encode_slice<T: GoEncode>(items: &[T], enc: &mut Encoder) {
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
    enc.wrap_error_with_type::<[T]>(": ");
}

impl<T: GoEncode> GoEncode for Vec<T> {
    fn go_encode(&self, enc: &mut Encoder) {
        T::go_encode_slice(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn option_is_empty(value: &Option<Self>) -> bool {
        value.as_ref().is_none_or(Self::is_empty)
    }
}

impl<T: GoEncode> GoEncode for [T] {
    fn go_encode(&self, enc: &mut Encoder) {
        T::go_encode_slice(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }
}

/// A Go map key type: `string` or an integer kind.
pub trait GoMapKey: GoType + Sized {
    /// The key's Go string form (the string itself, or the decimal integer).
    fn key_text(&self) -> Cow<'_, [u8]>;
    /// Writes the key as jsoniter writes it (an HTML-escaped string, or a quoted number).
    #[doc(hidden)]
    fn write_jsoniter_key(&self, enc: &mut Encoder) {
        enc.write_str(&self.key_text());
    }
    /// Parses a key from its decoded JSON string (`encoding/json` semantics); `None` reports
    /// an `UnmarshalTypeError`.
    #[doc(hidden)]
    fn std_parse_key(key: &[u8]) -> Option<Self>;
    /// Decodes a key with jsoniter (the iterator is positioned at the key).
    #[doc(hidden)]
    fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self;
}

impl GoMapKey for String {
    fn key_text(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.as_bytes())
    }

    fn std_parse_key(key: &[u8]) -> Option<Self> {
        Some(go_lossy(key))
    }

    fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
        go_lossy(&it.read_string())
    }
}

impl GoMapKey for GoString {
    fn key_text(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.as_bytes())
    }

    fn std_parse_key(key: &[u8]) -> Option<Self> {
        Some(Self::from(key))
    }

    fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
        Self::from_bytes(it.read_string())
    }
}

macro_rules! int_map_key {
    ($($ty:ty: $parse:ident),* $(,)?) => {
        $(
            impl GoMapKey for $ty {
                fn key_text(&self) -> Cow<'_, [u8]> {
                    Cow::Owned(self.to_string().into_bytes())
                }
                fn write_jsoniter_key(&self, enc: &mut Encoder) {
                    // Go: jsoniter numericMapKeyEncoder
                    enc.write_byte(b'"');
                    self.go_encode(enc);
                    enc.write_byte(b'"');
                }
                fn std_parse_key(key: &[u8]) -> Option<Self> {
                    crate::strconv::$parse(key).and_then(|n| <$ty>::try_from(n).ok())
                }
                fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
                    crate::jsoniter::decode_numeric_map_key(it)
                }
            }
        )*
    };
}

int_map_key!(
    i8: parse_int, i16: parse_int, i32: parse_int, i64: parse_int, isize: parse_int,
    u8: parse_uint, u16: parse_uint, u32: parse_uint, u64: parse_uint, usize: parse_uint,
);

/// Encodes a non-nil Go map with sorted keys.
pub fn encode_map<'a, K, V, I>(entries: I, enc: &mut Encoder)
where
    K: GoMapKey + 'a,
    V: GoEncode + 'a,
    I: IntoIterator<Item = (&'a K, &'a V)>,
{
    let mut sorted: Vec<(Cow<'a, [u8]>, &'a K, &'a V)> = entries
        .into_iter()
        .map(|(k, v)| (k.key_text(), k, v))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    match enc.flavor() {
        Flavor::Jsoniter => {
            // Go: jsoniter reflect_map.go sortKeysMapEncoder.Encode (entries go through a
            // sub-stream whose error is adopted only when the outer stream has none).
            enc.write_byte(b'{');
            let mut sub = Encoder::new(Flavor::Jsoniter);
            for (index, (_, key, value)) in sorted.into_iter().enumerate() {
                let start = sub.len();
                key.write_jsoniter_key(&mut sub);
                if sub.has_error() && !enc.has_error() {
                    enc.adopt_error_if_unset(sub.error_text().map(<[u8]>::to_vec));
                }
                sub.write_byte(b':');
                value.go_encode(&mut sub);
                if index != 0 {
                    enc.write_byte(b',');
                }
                enc.write_raw(sub.as_bytes().get(start..).unwrap_or_default());
            }
            enc.adopt_error_if_unset(sub.take_error());
            enc.write_byte(b'}');
        }
        Flavor::Std { .. } => {
            // Go: encoding/json mapEncoder.encode
            enc.write_byte(b'{');
            for (index, (text, _, value)) in sorted.into_iter().enumerate() {
                if index != 0 {
                    enc.write_byte(b',');
                }
                enc.write_str(&text);
                enc.write_byte(b':');
                value.go_encode(enc);
            }
            enc.write_byte(b'}');
        }
    }
}

impl<K: GoMapKey + Ord, V: GoEncode> GoEncode for BTreeMap<K, V> {
    fn go_encode(&self, enc: &mut Encoder) {
        encode_map(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn option_is_empty(value: &Option<Self>) -> bool {
        value.as_ref().is_none_or(Self::is_empty)
    }
}

impl<K: GoMapKey, V: GoEncode, S: BuildHasher> GoEncode for HashMap<K, V, S> {
    fn go_encode(&self, enc: &mut Encoder) {
        encode_map(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn option_is_empty(value: &Option<Self>) -> bool {
        value.as_ref().is_none_or(Self::is_empty)
    }
}

impl GoEncode for Map<String, Value> {
    fn go_encode(&self, enc: &mut Encoder) {
        encode_value_object(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn option_is_empty(value: &Option<Self>) -> bool {
        value.as_ref().is_none_or(Self::is_empty)
    }
}

impl GoEncode for Value {
    fn go_encode(&self, enc: &mut Encoder) {
        encode_value(self, enc);
    }

    fn go_is_empty(&self) -> bool {
        // A nil interface is empty; any non-nil dynamic value is not.
        self.is_null()
    }
}

fn encode_number(n: &Number, enc: &mut Encoder) {
    if n.is_f64() {
        enc.write_f64(n.as_f64().unwrap_or_default());
    } else if let Some(u) = n.as_u64() {
        enc.write_u64(u);
    } else if let Some(i) = n.as_i64() {
        enc.write_i64(i);
    } else {
        enc.write_f64(n.as_f64().unwrap_or_default());
    }
}

fn sorted_entries(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    entries
}

fn encode_value_object(map: &Map<String, Value>, enc: &mut Encoder) {
    encode_value_tree(None, Some(map), enc);
}

/// Encodes an `interface{}` value (`serde_json::Value`): numbers stored as floats encode as Go
/// `float64`, integers as `int64`/`uint64`, and object keys are sorted. Iterative, so deeply
/// nested values cannot exhaust the stack.
fn encode_value(value: &Value, enc: &mut Encoder) {
    encode_value_tree(Some(value), None, enc);
}

enum Frame<'a> {
    Array(slice::Iter<'a, Value>, bool),
    Object(vec::IntoIter<(&'a String, &'a Value)>, bool),
}

fn open_object<'a>(map: &'a Map<String, Value>, enc: &mut Encoder, stack: &mut Vec<Frame<'a>>) {
    if map.is_empty() {
        enc.write_raw(b"{}");
    } else {
        enc.write_byte(b'{');
        stack.push(Frame::Object(sorted_entries(map).into_iter(), true));
    }
}

fn encode_value_tree<'a>(
    root: Option<&'a Value>,
    root_object: Option<&'a Map<String, Value>>,
    enc: &mut Encoder,
) {
    let mut stack: Vec<Frame<'a>> = Vec::new();
    let mut next = root;
    if let Some(map) = root_object {
        open_object(map, enc, &mut stack);
    }
    loop {
        if let Some(value) = next.take() {
            match value {
                Value::Null => enc.write_null(),
                Value::Bool(b) => enc.write_bool(*b),
                Value::Number(n) => encode_number(n, enc),
                Value::String(s) => enc.write_str(s.as_bytes()),
                Value::Array(items) => {
                    if items.is_empty() {
                        enc.write_raw(b"[]");
                    } else {
                        enc.write_byte(b'[');
                        stack.push(Frame::Array(items.iter(), true));
                    }
                }
                Value::Object(map) => open_object(map, enc, &mut stack),
            }
        }
        let Some(top) = stack.last_mut() else {
            return;
        };
        match top {
            Frame::Array(items, first) => {
                if let Some(item) = items.next() {
                    if !*first {
                        enc.write_byte(b',');
                    }
                    *first = false;
                    next = Some(item);
                } else {
                    enc.write_byte(b']');
                    let _closed = stack.pop();
                }
            }
            Frame::Object(entries, first) => {
                if let Some((key, item)) = entries.next() {
                    if !*first {
                        enc.write_byte(b',');
                    }
                    *first = false;
                    enc.write_str(key.as_bytes());
                    enc.write_byte(b':');
                    next = Some(item);
                } else {
                    enc.write_byte(b'}');
                    let _closed = stack.pop();
                }
            }
        }
    }
}
