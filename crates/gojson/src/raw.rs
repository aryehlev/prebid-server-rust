//! [`RawJson`]: Go's `json.RawMessage`.

use core::fmt;
use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value;

use crate::{
    custom::marshaler_error,
    encode::{Encoder, Flavor, GoEncode, marshal_with},
    error::JsonError,
    gotype::{GoKind, GoType},
    jsoniter::{self, Iter, JsoniterDecode},
    jsonutil::try_extract_error_message,
    stdjson::{StdDecode, StdDecoder, append_compact},
    unicode::go_lossy,
};

/// Go's `json.RawMessage`: the exact bytes of a JSON value, cheap to clone.
///
/// Like the Go type it distinguishes nil ([`RawJson::nil`], an absent `ext`) from empty, and it
/// never validates or reformats its bytes on its own. How it encodes depends on the flavour,
/// exactly as in Go:
///
/// * jsoniter (`jsonutil.Marshal`, with Prebid's `RawMessageExtension`): the bytes are compacted
///   with `json.Compact`; a nil, empty or invalid message writes *nothing*, so a struct field
///   without `omitempty` produces `"ext":` followed directly by the next byte, as Go does. Through
///   a pointer (`Option<RawJson>`) jsoniter instead calls `MarshalJSON`, so the bytes are written
///   as they are (nil writes `null`).
/// * `encoding/json`: nil writes `null`; the bytes are compacted with HTML escaping per the
///   encoder; invalid bytes fail with `json: error calling MarshalJSON for type json.RawMessage:
///   ...`.
///
/// Decoding stores the value's exact bytes: jsoniter stores `null` as nil, `encoding/json`
/// stores the four bytes `null`.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct RawJson(Option<Arc<[u8]>>);

impl fmt::Debug for RawJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => f.write_str("RawJson(nil)"),
            Some(bytes) => write!(f, "RawJson({:?})", go_lossy(bytes)),
        }
    }
}

impl RawJson {
    /// The nil message.
    #[must_use]
    pub const fn nil() -> Self {
        Self(None)
    }

    /// A (non-nil) message holding `bytes`.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(Some(Arc::from(bytes)))
    }

    /// A (non-nil) message holding `bytes`.
    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self(Some(Arc::from(bytes)))
    }

    /// Whether the message is nil.
    #[must_use]
    pub const fn is_nil(&self) -> bool {
        self.0.is_none()
    }

    /// The bytes (empty for nil).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_deref().unwrap_or_default()
    }

    /// `len(m)`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// `len(m) == 0` (nil or empty).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    /// `jsonutil.Marshal(v)` as a message.
    ///
    /// # Errors
    ///
    /// The `FailedToMarshal` message (jsoniter's error text).
    pub fn marshal<T: GoEncode + ?Sized>(v: &T) -> Result<Self, JsonError> {
        marshal_with(v, Flavor::Jsoniter).map(Self::from_vec)
    }

    /// `jsonutil.Marshal(value)` of an `interface{}` value (sorted object keys).
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        Self::from_vec(marshal_with(value, Flavor::Jsoniter).unwrap_or_default())
    }

    /// `jsonutil.Unmarshal(m, &v)` into an existing value.
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` message (`jsonutil`'s cleaned-up jsoniter error).
    pub fn unmarshal_into<T: JsoniterDecode + ?Sized>(&self, v: &mut T) -> Result<(), JsonError> {
        jsoniter::unmarshal(self.as_bytes(), v)
            .map_err(|err| JsonError::Message(try_extract_error_message(&err.go_text())))
    }

    /// `jsonutil.Unmarshal(m, &v)` into a zero value.
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` message.
    pub fn unmarshal<T: JsoniterDecode + Default>(&self) -> Result<T, JsonError> {
        let mut v = T::default();
        self.unmarshal_into(&mut v)?;
        Ok(v)
    }

    /// `jsonutil.Unmarshal` into `interface{}`: numbers become `float64`, objects
    /// `map[string]interface{}`.
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` message.
    pub fn to_value(&self) -> Result<Value, JsonError> {
        self.unmarshal()
    }

    /// `jsonutil.Unmarshal` into `map[string]json.RawMessage` (nil and empty messages give an
    /// empty map, as Go code guarded by `len(ext) > 0` does).
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` message.
    pub fn to_object(&self) -> Result<BTreeMap<String, Self>, JsonError> {
        if self.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut map: Option<BTreeMap<String, Self>> = None;
        self.unmarshal_into(&mut map)?;
        Ok(map.unwrap_or_default())
    }

    /// `jsonutil.Marshal` of `map[string]json.RawMessage`: keys sorted, values compacted, and
    /// nil or invalid values written as nothing (Go's `RawMessageExtension`).
    ///
    /// # Errors
    ///
    /// The `FailedToMarshal` message.
    pub fn from_object(map: &BTreeMap<String, Self>) -> Result<Self, JsonError> {
        Self::marshal(map)
    }

    /// `m[key]` after `jsonutil.Unmarshal` into `map[string]json.RawMessage`.
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` message.
    pub fn get(&self, key: &str) -> Result<Option<Self>, JsonError> {
        Ok(self.to_object()?.remove(key))
    }

    /// Go's edit pattern: unmarshal into `map[string]json.RawMessage`, set `key`, and
    /// `jsonutil.Marshal` the map back (sorted keys, compacted values).
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` or `FailedToMarshal` message.
    pub fn with_key(&self, key: &str, value: Self) -> Result<Self, JsonError> {
        let mut map = self.to_object()?;
        let _replaced = map.insert(key.to_owned(), value);
        Self::from_object(&map)
    }

    /// Go's edit pattern with `delete(m, key)`.
    ///
    /// # Errors
    ///
    /// The `FailedToUnmarshal` or `FailedToMarshal` message.
    pub fn without_key(&self, key: &str) -> Result<Self, JsonError> {
        let mut map = self.to_object()?;
        let _removed = map.remove(key);
        Self::from_object(&map)
    }

    /// `encoding/json`'s `marshalerEncoder` for a value of the type named `N` (`json.RawMessage`
    /// or `*json.RawMessage`, which is what its `MarshalerError` names).
    fn std_encode<N: GoType + ?Sized>(&self, enc: &mut Encoder, escape_html: bool) {
        // Go: RawMessage.MarshalJSON + encoding/json marshalerEncoder
        let src: &[u8] = if self.is_nil() {
            b"null"
        } else {
            self.as_bytes()
        };
        let mut out = Vec::with_capacity(src.len());
        match append_compact(&mut out, src, escape_html) {
            Ok(()) => enc.write_raw(&out),
            Err(err) => enc.set_error(marshaler_error::<N>(&err)),
        }
    }
}

impl From<Vec<u8>> for RawJson {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

impl From<&[u8]> for RawJson {
    fn from(bytes: &[u8]) -> Self {
        Self::from_slice(bytes)
    }
}

impl From<&str> for RawJson {
    fn from(text: &str) -> Self {
        Self::from_slice(text.as_bytes())
    }
}

impl From<String> for RawJson {
    fn from(text: String) -> Self {
        Self::from_vec(text.into_bytes())
    }
}

impl GoType for RawJson {
    const KIND: GoKind = GoKind::Slice;

    fn go_type_name(out: &mut String) {
        out.push_str("json.RawMessage");
    }
}

impl GoEncode for RawJson {
    fn go_encode(&self, enc: &mut Encoder) {
        match enc.flavor() {
            Flavor::Jsoniter => {
                // Go: util/jsonutil rawMessageCodec.Encode
                let mut out = Vec::with_capacity(self.len());
                if append_compact(&mut out, self.as_bytes(), false).is_ok() {
                    enc.write_raw(&out);
                }
            }
            Flavor::Std { escape_html } => self.std_encode::<Self>(enc, escape_html),
        }
    }

    fn go_is_empty(&self) -> bool {
        self.is_empty()
    }

    fn option_go_encode(value: &Option<Self>, enc: &mut Encoder) {
        match (value, enc.flavor()) {
            (None, _) => enc.write_null(),
            (Some(raw), Flavor::Jsoniter) => {
                // Go: jsoniter marshalerEncoder on *json.RawMessage (no extension applies)
                if raw.is_nil() {
                    enc.write_null();
                    return;
                }
                let bytes = raw.as_bytes();
                let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
                enc.write_raw(trimmed);
            }
            (Some(raw), Flavor::Std { escape_html }) => {
                raw.std_encode::<Option<Self>>(enc, escape_html);
            }
        }
    }
}

impl JsoniterDecode for RawJson {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        // Go: json-iterator/go reflect_json_raw_message.go jsonRawMessageCodec.Decode
        if it.read_nil() {
            *self = Self::nil();
            return;
        }
        // The capture buffer is never nil, so a value that fails to skip without consuming
        // anything leaves an empty, non-nil message.
        *self = Self::from_vec(it.skip_and_return_bytes());
    }
}

impl StdDecode for RawJson {
    fn std_decode_as<N: GoType + ?Sized>(
        &mut self,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        // Go: (*RawMessage).UnmarshalJSON
        *self = Self::from_slice(d.raw_value());
        Ok(())
    }

    fn std_literal_as<N: GoType + ?Sized>(
        &mut self,
        item: &[u8],
        _from_quoted: bool,
        d: &mut StdDecoder<'_>,
    ) -> Result<(), JsonError> {
        if item.is_empty() {
            d.save_error(StdDecoder::invalid_string_use::<N>(item));
            return Ok(());
        }
        *self = Self::from_slice(item);
        Ok(())
    }
}
