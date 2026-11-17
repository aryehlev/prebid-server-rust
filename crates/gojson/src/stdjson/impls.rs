//! [`StdDecode`] for the primitive, container and dynamic types (`literalStore`, `array`,
//! `object` of `encoding/json/decode.go`).

use core::{hash::Hash, mem};
use std::{
    collections::{BTreeMap, HashMap},
    hash::BuildHasher,
};

use serde_json::{Map, Value};

use super::{
    StdDecode, StdDecoder, Token, unquote,
    value::{convert_number, value_interface},
};
use crate::{
    base64::decode_std,
    encode::GoMapKey,
    error::JsonError,
    gostring::GoString,
    gotype::GoType,
    jsoniter::GoMap,
    strconv::{f64_to_f32, parse_float_go, parse_int, parse_uint},
    unicode::go_lossy,
};

/// How a Go kind treats the literal classes in `literalStore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// `bool`
    Bool,
    /// Integer and float kinds.
    Number,
    /// `string`
    String,
    /// `[]byte` (base64 strings, `null` → nil)
    Bytes,
    /// `interface{}`
    Interface,
    /// Maps and other slices (`null` → nil).
    Nilable,
    /// Structs (every literal but `null` is a mismatch).
    Other,
}

/// What `literalStore` stores after its checks.
#[derive(Debug)]
pub enum Store<'i> {
    /// Nothing (ignored, or an error was saved).
    Nothing,
    /// `null` into a nil-able kind.
    Null,
    /// A boolean.
    Bool(bool),
    /// The unquoted string (or the base64-decoded bytes for [`Class::Bytes`]).
    Str(Vec<u8>),
    /// A number literal to parse for the kind.
    Num(&'i [u8]),
}

/// Go's `literalStore` checks for a kind `class`, naming errors with the Go type `N`.
///
/// # Errors
///
/// Returns the `,string` misuse errors Go returns (rather than saves).
pub fn literal_store<'i, N: GoType + ?Sized>(
    item: &'i [u8],
    from_quoted: bool,
    d: &mut StdDecoder<'_>,
    class: Class,
) -> Result<Store<'i>, JsonError> {
    // Go: encoding/json/decode.go literalStore
    let Some(&c) = item.first() else {
        d.save_error(StdDecoder::invalid_string_use::<N>(item));
        return Ok(Store::Nothing);
    };
    match c {
        b'n' => {
            if from_quoted && item != b"null" {
                d.save_error(StdDecoder::invalid_string_use::<N>(item));
                return Ok(Store::Nothing);
            }
            Ok(match class {
                Class::Interface | Class::Nilable | Class::Bytes => Store::Null,
                Class::Bool | Class::Number | Class::String | Class::Other => Store::Nothing,
            })
        }
        b't' | b'f' => {
            if from_quoted && item != b"true" && item != b"false" {
                d.save_error(StdDecoder::invalid_string_use::<N>(item));
                return Ok(Store::Nothing);
            }
            match class {
                Class::Bool | Class::Interface => Ok(Store::Bool(c == b't')),
                Class::Number | Class::String | Class::Bytes | Class::Nilable | Class::Other => {
                    if from_quoted {
                        d.save_error(StdDecoder::invalid_string_use::<N>(item));
                    } else {
                        d.type_error::<N>("bool");
                    }
                    Ok(Store::Nothing)
                }
            }
        }
        b'"' => {
            let Some(s) = unquote(item) else {
                if from_quoted {
                    return Err(StdDecoder::invalid_string_use::<N>(item));
                }
                return Ok(Store::Nothing);
            };
            match class {
                Class::String | Class::Interface => Ok(Store::Str(s)),
                Class::Bytes => match decode_std(&s) {
                    Ok(bytes) => Ok(Store::Str(bytes)),
                    Err((_, msg)) => {
                        d.save_error(JsonError::Message(msg.into_bytes()));
                        Ok(Store::Nothing)
                    }
                },
                Class::Bool | Class::Number | Class::Nilable | Class::Other => {
                    d.type_error::<N>("string");
                    Ok(Store::Nothing)
                }
            }
        }
        _ => {
            if c != b'-' && !c.is_ascii_digit() {
                if from_quoted {
                    return Err(StdDecoder::invalid_string_use::<N>(item));
                }
                return Ok(Store::Nothing);
            }
            match class {
                Class::Number | Class::Interface => Ok(Store::Num(item)),
                Class::Bool | Class::String | Class::Bytes | Class::Nilable | Class::Other => {
                    if from_quoted {
                        return Err(StdDecoder::invalid_string_use::<N>(item));
                    }
                    d.type_error::<N>("number");
                    Ok(Store::Nothing)
                }
            }
        }
    }
}

/// `value(v)` for a kind that only accepts literals: arrays and objects are mismatches.
///
/// # Errors
///
/// Returns an error that aborts decoding.
pub fn std_decode_scalar<N: GoType + ?Sized, T: StdDecode + ?Sized>(
    v: &mut T,
    d: &mut StdDecoder<'_>,
) -> Result<(), JsonError> {
    match d.peek() {
        Token::Array => {
            d.type_error::<N>("array");
            d.skip();
            Ok(())
        }
        Token::Object => {
            d.type_error::<N>("object");
            d.skip();
            Ok(())
        }
        Token::Literal => {
            let item = d.read_literal();
            v.std_literal_as::<N>(item, false, d)
        }
    }
}

fn number_error<N: GoType + ?Sized>(d: &mut StdDecoder<'_>, s: &[u8]) {
    d.type_error::<N>(&format!("number {}", go_lossy(s)));
}

macro_rules! std_integer {
    ($parse:ident: $($ty:ty),*) => {
        $(
            impl StdDecode for $ty {
                fn std_decode_as<N: GoType + ?Sized>(
                    &mut self,
                    d: &mut StdDecoder<'_>,
                ) -> Result<(), JsonError> {
                    std_decode_scalar::<N, Self>(self, d)
                }
                fn std_literal_as<N: GoType + ?Sized>(
                    &mut self,
                    item: &[u8],
                    from_quoted: bool,
                    d: &mut StdDecoder<'_>,
                ) -> Result<(), JsonError> {
                    let store = literal_store::<N>(item, from_quoted, d, Class::Number)?;
                    if let Store::Num(s) = store {
                        match $parse(s).and_then(|n| <$ty>::try_from(n).ok()) {
                            Some(v) => *self = v,
                            None => number_error::<N>(d, s),
                        }
                    }
                    Ok(())
                }
            }
        )*
    };
}

std_integer!(parse_int: i8, i16, i32, i64, isize);
std_integer!(parse_uint: u16, u32, u64, usize);

impl StdDecode for u8 {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Num(s) = literal_store::<N>(item, from_quoted, d, Class::Number)? {
            match parse_uint(s).and_then(|n| Self::try_from(n).ok()) {
                Some(v) => *self = v,
                None => number_error::<N>(d, s),
            }
        }
        Ok(())
    }

    fn std_slice_literal<N: GoType + ?Sized>(
        slot: &mut Option<Vec<Self>>,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        match literal_store::<N>(item, from_quoted, d, Class::Bytes)? {
            Store::Null => *slot = None,
            Store::Str(bytes) => *slot = Some(bytes),
            Store::Nothing | Store::Bool(_) | Store::Num(_) => {}
        }
        Ok(())
    }
}

impl StdDecode for f64 {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Num(s) = literal_store::<N>(item, from_quoted, d, Class::Number)? {
            match parse_float_go(s, false) {
                (v, None) => *self = v,
                (_, Some(_)) => number_error::<N>(d, s),
            }
        }
        Ok(())
    }
}

impl StdDecode for f32 {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Num(s) = literal_store::<N>(item, from_quoted, d, Class::Number)? {
            match parse_float_go(s, true) {
                (v, None) => *self = f64_to_f32(v),
                (_, Some(_)) => number_error::<N>(d, s),
            }
        }
        Ok(())
    }
}

impl StdDecode for bool {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Bool(v) = literal_store::<N>(item, from_quoted, d, Class::Bool)? {
            *self = v;
        }
        Ok(())
    }
}

impl StdDecode for String {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Str(s) = literal_store::<N>(item, from_quoted, d, Class::String)? {
            *self = go_lossy(&s);
        }
        Ok(())
    }
}

impl StdDecode for GoString {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        std_decode_scalar::<N, Self>(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if let Store::Str(s) = literal_store::<N>(item, from_quoted, d, Class::String)? {
            *self = Self::from_bytes(s);
        }
        Ok(())
    }
}

impl StdDecode for Value {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        match d.peek() {
            Token::Literal => {
                let item = d.read_literal();
                self.std_literal_as::<N>(item, false, d)
            }
            Token::Array | Token::Object => {
                *self = value_interface(d);
                Ok(())
            }
        }
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        match literal_store::<N>(item, from_quoted, d, Class::Interface)? {
            Store::Nothing => {}
            Store::Null => *self = Self::Null,
            Store::Bool(b) => *self = Self::Bool(b),
            Store::Str(s) => *self = Self::String(go_lossy(&s)),
            Store::Num(s) => {
                if let Some(v) = convert_number(d, s) {
                    *self = v;
                }
            }
        }
        Ok(())
    }
}

impl<T: StdDecode + Default> StdDecode for Option<T> {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        T::std_decode_option(self, d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        T::std_literal_option(self, item, from_quoted, d)
    }
}

impl<T: StdDecode + ?Sized> StdDecode for Box<T> {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        (**self).std_decode_as::<N>(d)
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        (**self).std_literal_as::<N>(item, from_quoted, d)
    }
}

/// `array(v)` into a slice: existing elements are decoded into first, the slice is truncated to
/// the elements read, and `[]` gives an empty (non-nil) slice.
fn slice_array<T: StdDecode + Default>(
    slot: &mut Option<Vec<T>>,
    d: &mut StdDecoder<'_>,
) -> Result<(), JsonError> {
    let items = slot.get_or_insert_with(Vec::new);
    let mut i = 0;
    if d.array_begin() {
        loop {
            if let Some(existing) = items.get_mut(i) {
                existing.std_decode(d)?;
            } else {
                let mut elem = T::default();
                let result = elem.std_decode(d);
                items.push(elem);
                result?;
            }
            i += 1;
            if !d.array_next() {
                break;
            }
        }
    }
    items.truncate(i);
    Ok(())
}

/// `value(v)` into a nil-able `[]T` named `N`.
///
/// # Errors
///
/// Returns an error that aborts decoding.
pub fn std_decode_slice<N: GoType + ?Sized, T: StdDecode + Default>(
    slot: &mut Option<Vec<T>>,
    d: &mut StdDecoder<'_>,
) -> Result<(), JsonError> {
    match d.peek() {
        Token::Literal => {
            let item = d.read_literal();
            T::std_slice_literal::<N>(slot, item, false, d)
        }
        Token::Object => {
            d.type_error::<N>("object");
            d.skip();
            Ok(())
        }
        Token::Array => slice_array(slot, d),
    }
}

impl<T: StdDecode + Default> StdDecode for Vec<T> {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        let mut slot = Some(mem::take(self));
        let result = T::std_decode_slice::<N>(&mut slot, d);
        *self = slot.unwrap_or_default();
        result
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        let mut slot = Some(mem::take(self));
        let result = T::std_slice_literal::<N>(&mut slot, item, from_quoted, d);
        *self = slot.unwrap_or_default();
        result
    }

    fn std_decode_option(slot: &mut Option<Self>, d: &mut StdDecoder<'_>) -> Result<(), JsonError> {
        T::std_decode_slice::<Self>(slot, d)
    }
}

/// `value(v)` into a nil-able map named `N`: entries are added to the existing map, each
/// element starting from its zero value; integer keys that do not parse are type errors.
///
/// # Errors
///
/// Returns an error that aborts decoding.
pub fn std_decode_map<N, M>(slot: &mut Option<M>, d: &mut StdDecoder<'_>) -> Result<(), JsonError>
where
    N: GoType + ?Sized,
    M: GoMap,
    M::Elem: StdDecode,
{
    match d.peek() {
        Token::Literal => {
            let item = d.read_literal();
            if matches!(
                literal_store::<N>(item, false, d, Class::Nilable)?,
                Store::Null
            ) {
                *slot = None;
            }
            Ok(())
        }
        Token::Array => {
            d.type_error::<N>("array");
            d.skip();
            Ok(())
        }
        Token::Object => {
            let map = slot.get_or_insert_with(M::default);
            let Some(mut key) = d.object_begin() else {
                return Ok(());
            };
            loop {
                let k = unquote(key).unwrap_or_default();
                let mut elem = M::Elem::default();
                elem.std_decode(d)?;
                match M::Key::std_parse_key(&k) {
                    Some(kv) => map.go_insert(kv, elem),
                    None => number_error::<M::Key>(d, &k),
                }
                match d.object_next() {
                    Some(next) => key = next,
                    None => return Ok(()),
                }
            }
        }
    }
}

macro_rules! std_map {
    ($ty:ty, [$($bounds:tt)*]) => {
        impl<$($bounds)*> StdDecode for $ty {
            fn std_decode_as<N: GoType + ?Sized>(
                &mut self,
                d: &mut StdDecoder<'_>,
            ) -> Result<(), JsonError> {
                let mut slot = Some(mem::take(self));
                let result = std_decode_map::<N, Self>(&mut slot, d);
                *self = slot.unwrap_or_default();
                result
            }

            fn std_literal_as<N: GoType + ?Sized>(
                &mut self,
                item: &[u8],
                from_quoted: bool,
                d: &mut StdDecoder<'_>,
            ) -> Result<(), JsonError> {
                if let Store::Null = literal_store::<N>(item, from_quoted, d, Class::Nilable)? {
                    *self = Self::default();
                }
                Ok(())
            }

            fn std_decode_option(
                slot: &mut Option<Self>,
                d: &mut StdDecoder<'_>,
            ) -> Result<(), JsonError> {
                std_decode_map::<Self, Self>(slot, d)
            }
        }
    };
}

std_map!(BTreeMap<K, V>, [K: GoMapKey + Ord, V: StdDecode + Default]);
std_map!(
    HashMap<K, V, S>,
    [K: GoMapKey + Eq + Hash, V: StdDecode + Default, S: BuildHasher + Default]
);
std_map!(Map<String, Value>, []);
