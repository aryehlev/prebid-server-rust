//! json-iterator (`jsoniter.ConfigCompatibleWithStandardLibrary`) decoding, as used by
//! `jsonutil.Unmarshal` and `jsonutil.UnmarshalValid`.
//!
//! [`Iter`] is a port of jsoniter's `Iterator`; [`JsoniterDecode`] is the per-type decoder
//! (Go's `ValDecoder`), implemented for the crate's type mapping and by
//! [`#[derive(GoJson)]`](crate::GoJson).

use core::mem;

use crate::base64::decode_std;

mod iter;

use core::hash::{BuildHasher, Hash};
use std::collections::{BTreeMap, HashMap};

pub use iter::{Iter, ValueType, read_value};
use serde_json::{Map, Value};

use crate::{
    encode::GoMapKey, error::JsonError, gostring::GoString, gotype::GoType, unicode::go_lossy,
};

/// One step of jsoniter's field-name hash (`hash ^= int64(b); hash *= 0x1000193`).
fn fnv_step(hash: i64, b: u8) -> i64 {
    (hash ^ i64::from(b)).wrapping_mul(0x0100_0193)
}

/// jsoniter `calcHash(strings.ToLower(name), false)`.
pub(crate) fn calc_hash(lowered: &[u8]) -> i64 {
    lowered
        .iter()
        .fold(0x811C_9DC5_i64, |hash, &b| fnv_step(hash, b))
}

/// A Rust value that jsoniter decodes exactly like the Go value it stands for.
///
/// Decoding writes into an existing value, as Go does: struct fields absent from the input keep
/// their values, `null` leaves numbers and booleans unchanged, and so on.
pub trait JsoniterDecode: GoType {
    /// Decodes the next value from `it` into `self`, recording errors in the iterator.
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>);

    /// Decodes into `Option<Self>` (Go's `OptionalDecoder` for `*T`; slices and maps override
    /// it, since `Option` models their nil state).
    #[doc(hidden)]
    fn jsoniter_decode_option(slot: &mut Option<Self>, it: &mut Iter<'_>)
    where
        Self: Sized + Default,
    {
        // Go: json-iterator/go reflect_optional.go OptionalDecoder.Decode
        if it.read_nil() {
            *slot = None;
        } else {
            slot.get_or_insert_with(Self::default).jsoniter_decode(it);
        }
    }

    /// Decodes a nil-able `[]Self` (`[]byte` overrides it with Go's base64 codec).
    #[doc(hidden)]
    fn jsoniter_decode_slice(slot: &mut Option<Vec<Self>>, it: &mut Iter<'_>)
    where
        Self: Sized + Default,
    {
        decode_slice::<Vec<Self>, Self>(slot, it);
    }

    /// The `,string` decoding of a `string`-kind field (`stringModeStringDecoder`); other kinds
    /// never reach it.
    #[doc(hidden)]
    fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
        self.jsoniter_decode(it);
    }
}

/// `jsoniter.ConfigCompatibleWithStandardLibrary.Unmarshal(data, v)`, returning the raw
/// jsoniter error (before `jsonutil`'s message clean-up).
///
/// # Errors
///
/// Returns jsoniter's error text, for example
/// `openrtb2.Bid.W: readUint64: unexpected character: \xff, error found in #10 byte of ...`.
pub fn unmarshal<T: JsoniterDecode + ?Sized>(data: &[u8], v: &mut T) -> Result<(), JsonError> {
    // Go: json-iterator/go config.go frozenConfig.Unmarshal (+ Iterator.ReadVal)
    let mut it = Iter::new(data);
    let depth = it.depth;
    v.jsoniter_decode(&mut it);
    if it.depth != depth {
        it.report_error("ReadVal", b"unexpected mismatched nesting");
    }
    let c = it.next_token();
    if c == 0 {
        return it.error().map_or(Ok(()), Err);
    }
    it.report_error("Unmarshal", b"there are bytes left after unmarshal");
    it.error().map_or(Ok(()), Err)
}

macro_rules! read_number {
    ($($ty:ty => $read:ident),* $(,)?) => {
        $(
            impl JsoniterDecode for $ty {
                fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
                    // Go: json-iterator/go reflect_native.go *Codec.Decode
                    if !it.read_nil() {
                        *self = it.$read();
                    }
                }
            }
        )*
    };
}

read_number!(
    bool => read_bool,
    i8 => read_int8,
    i16 => read_int16,
    i32 => read_int32,
    i64 => read_int64,
    u16 => read_uint16,
    u32 => read_uint32_value,
    u64 => read_uint64_value,
    f32 => read_float32,
    f64 => read_float64,
);

impl JsoniterDecode for isize {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        if !it.read_nil() {
            *self = Self::try_from(it.read_int64()).unwrap_or_default();
        }
    }
}

impl JsoniterDecode for usize {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        if !it.read_nil() {
            *self = Self::try_from(it.read_uint64_value()).unwrap_or_default();
        }
    }
}

impl JsoniterDecode for u8 {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        if !it.read_nil() {
            *self = it.read_uint8();
        }
    }

    fn jsoniter_decode_slice(slot: &mut Option<Vec<Self>>, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_native.go base64Codec.Decode
        if it.read_nil() {
            *slot = None;
            return;
        }
        match it.what_is_next() {
            ValueType::String => {
                let src = it.read_string();
                match decode_std(&src) {
                    Ok(dst) => *slot = Some(dst),
                    Err((_, msg)) => it.report_error("decode base64", msg.as_bytes()),
                }
            }
            ValueType::Array => decode_slice::<Vec<Self>, Self>(slot, it),
            ValueType::Invalid
            | ValueType::Number
            | ValueType::Nil
            | ValueType::Bool
            | ValueType::Object => it.report_error("base64Codec", b"invalid input"),
        }
    }
}

/// Re-reads a decoded string as a JSON string (`stringModeStringDecoder`); a value that is not
/// itself a JSON string literal becomes empty.
pub(crate) fn reread_string(decoded: &[u8]) -> Vec<u8> {
    Iter::new(decoded).read_string()
}

impl JsoniterDecode for String {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        *self = go_lossy(&it.read_string());
    }

    fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
        self.jsoniter_decode(it);
        *self = go_lossy(&reread_string(self.as_bytes()));
    }
}

impl JsoniterDecode for GoString {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        *self = Self::from_bytes(it.read_string());
    }

    fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
        self.jsoniter_decode(it);
        *self = Self::from_bytes(reread_string(self.as_bytes()));
    }
}

impl<T: JsoniterDecode + Default> JsoniterDecode for Option<T> {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        T::jsoniter_decode_option(self, it);
    }
}

impl<T: JsoniterDecode + ?Sized> JsoniterDecode for Box<T> {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        (**self).jsoniter_decode(it);
    }

    fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
        (**self).jsoniter_decode_string_mode(it);
    }
}

/// jsoniter's `sliceDecoder`, with errors wrapped in the slice type `N`.
///
/// `null` makes the slice nil and `[]` empty. Elements are decoded into the existing elements
/// first, as Go's `UnsafeGrow` reuses the backing array; errors are wrapped with the slice
/// type (`"[]openrtb2.Imp: ..."`).
pub fn decode_slice<N: GoType + ?Sized, T: JsoniterDecode + Default>(
    slot: &mut Option<Vec<T>>,
    it: &mut Iter<'_>,
) {
    // Go: json-iterator/go reflect_slice.go sliceDecoder.Decode / doDecode
    decode_slice_inner(slot, it);
    it.wrap_error_with_type::<N>(": ");
}

fn decode_slice_inner<T: JsoniterDecode + Default>(slot: &mut Option<Vec<T>>, it: &mut Iter<'_>) {
    let mut c = it.next_token();
    if c == b'n' {
        it.skip_three_bytes(b'u', b'l', b'l');
        *slot = None;
        return;
    }
    if c != b'[' {
        it.report_error_byte("decode slice", "expect [ or n, but found ", c);
        return;
    }
    c = it.next_token();
    if c == b']' {
        *slot = Some(Vec::new());
        return;
    }
    it.unread_byte();
    let items = slot.get_or_insert_with(Vec::new);
    let mut length = 0;
    loop {
        if let Some(existing) = items.get_mut(length) {
            existing.jsoniter_decode(it);
        } else {
            let mut elem = T::default();
            elem.jsoniter_decode(it);
            items.push(elem);
        }
        length += 1;
        c = it.next_token();
        if c == b',' {
            continue;
        }
        // Go's slice length is the number of elements decoded, also on error.
        items.truncate(length);
        if c != b']' {
            it.report_error_byte("decode slice", "expect ], but found ", c);
        }
        return;
    }
}

impl<T: JsoniterDecode + Default> JsoniterDecode for Vec<T> {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        let mut slot = Some(mem::take(self));
        T::jsoniter_decode_slice(&mut slot, it);
        *self = slot.unwrap_or_default();
    }

    fn jsoniter_decode_option(slot: &mut Option<Self>, it: &mut Iter<'_>) {
        T::jsoniter_decode_slice(slot, it);
    }
}

/// A Go map type: the operations jsoniter's and `encoding/json`'s map decoders need.
#[doc(hidden)]
pub trait GoMap: Default {
    /// The key type.
    type Key: GoMapKey;
    /// The element type.
    type Elem: Default;
    /// Go's `SetIndex`: inserts or replaces.
    fn go_insert(&mut self, key: Self::Key, elem: Self::Elem);
}

impl<K: GoMapKey + Ord, V: Default> GoMap for BTreeMap<K, V> {
    type Elem = V;
    type Key = K;

    fn go_insert(&mut self, key: K, elem: V) {
        let _replaced = self.insert(key, elem);
    }
}

impl<K: GoMapKey + Eq + Hash, V: Default, S: BuildHasher + Default> GoMap for HashMap<K, V, S> {
    type Elem = V;
    type Key = K;

    fn go_insert(&mut self, key: K, elem: V) {
        let _replaced = self.insert(key, elem);
    }
}

impl GoMap for Map<String, Value> {
    type Elem = Value;
    type Key = String;

    fn go_insert(&mut self, key: String, elem: Value) {
        let _replaced = self.insert(key, elem);
    }
}

/// jsoniter's `mapDecoder`: `null` makes the map nil; entries are inserted into the existing
/// map (a nil map is created first); elements start from their zero value.
pub fn decode_map<M>(slot: &mut Option<M>, it: &mut Iter<'_>)
where
    M: GoMap,
    M::Elem: JsoniterDecode,
{
    // Go: json-iterator/go reflect_map.go mapDecoder.Decode
    let mut c = it.next_token();
    if c == b'n' {
        it.skip_three_bytes(b'u', b'l', b'l');
        *slot = None;
        return;
    }
    let map = slot.get_or_insert_with(M::default);
    if c != b'{' {
        it.report_error_byte("ReadMapCB", "expect { or n, but found ", c);
        return;
    }
    c = it.next_token();
    if c == b'}' {
        return;
    }
    it.unread_byte();
    loop {
        let key = M::Key::jsoniter_decode_key(it);
        c = it.next_token();
        if c != b':' {
            it.report_error_byte("ReadMapCB", "expect : after object field, but found ", c);
            return;
        }
        let mut elem = M::Elem::default();
        elem.jsoniter_decode(it);
        map.go_insert(key, elem);
        c = it.next_token();
        if c == b',' {
            continue;
        }
        if c != b'}' {
            it.report_error_byte("ReadMapCB", "expect }, but found ", c);
        }
        return;
    }
}

/// jsoniter's `numericMapKeyDecoder`: an integer key written as a JSON string.
pub(crate) fn decode_numeric_map_key<T: JsoniterDecode + Default>(it: &mut Iter<'_>) -> T {
    let mut key = T::default();
    let mut c = it.next_token();
    if c != b'"' {
        it.report_error_byte("ReadMapCB", "expect \", but found ", c);
        return key;
    }
    key.jsoniter_decode(it);
    c = it.next_token();
    if c != b'"' {
        it.report_error_byte("ReadMapCB", "expect \", but found ", c);
    }
    key
}

impl<K, V> JsoniterDecode for BTreeMap<K, V>
where
    K: GoMapKey + Ord,
    V: JsoniterDecode + Default,
{
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        let mut slot = Some(mem::take(self));
        decode_map(&mut slot, it);
        *self = slot.unwrap_or_default();
    }

    fn jsoniter_decode_option(slot: &mut Option<Self>, it: &mut Iter<'_>) {
        decode_map(slot, it);
    }
}

impl<K, V, S> JsoniterDecode for HashMap<K, V, S>
where
    K: GoMapKey + Eq + Hash,
    V: JsoniterDecode + Default,
    S: BuildHasher + Default,
{
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        let mut slot = Some(mem::take(self));
        decode_map(&mut slot, it);
        *self = slot.unwrap_or_default();
    }

    fn jsoniter_decode_option(slot: &mut Option<Self>, it: &mut Iter<'_>) {
        decode_map(slot, it);
    }
}

impl JsoniterDecode for Map<String, Value> {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        let mut slot = Some(mem::take(self));
        decode_map(&mut slot, it);
        *self = slot.unwrap_or_default();
    }

    fn jsoniter_decode_option(slot: &mut Option<Self>, it: &mut Iter<'_>) {
        decode_map(slot, it);
    }
}

impl JsoniterDecode for Value {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_dynamic.go efaceDecoder.Decode → Iterator.Read
        *self = read_value(it);
    }
}

/// jsoniter's `stringModeNumberDecoder` (a `,string` field of a non-string kind): the value
/// must be wrapped in quotes, except `null`.
pub fn decode_string_mode_number<T: JsoniterDecode + ?Sized>(v: &mut T, it: &mut Iter<'_>) {
    // Go: json-iterator/go reflect_extension.go stringModeNumberDecoder.Decode
    if it.what_is_next() == ValueType::Nil {
        v.jsoniter_decode(it);
        return;
    }
    let mut c = it.next_token();
    if c != b'"' {
        it.report_error_byte("stringModeNumberDecoder", "expect \", but found ", c);
        return;
    }
    v.jsoniter_decode(it);
    if it.has_any_error() {
        return;
    }
    c = it.read_byte();
    if c != b'"' {
        it.report_error_byte("stringModeNumberDecoder", "expect \", but found ", c);
    }
}
