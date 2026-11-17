//! Custom `MarshalJSON` / `UnmarshalJSON` methods, and how each Go library calls them.

use crate::{
    encode::{Encoder, Flavor},
    error::JsonError,
    gotype::GoType,
    jsoniter::Iter,
    stdjson::{StdDecoder, append_compact},
};

/// A Go `UnmarshalJSON` method (pointer receiver).
///
/// Use it with `#[gojson(unmarshaler)]` on a derived type. Both libraries hand over the raw
/// bytes of the value: jsoniter through `SkipAndReturnBytes` (`null` included, except for
/// pointer fields where `null` sets nil), `encoding/json` as the exact value text.
pub trait GoUnmarshaler {
    /// Go's `UnmarshalJSON(data)`.
    ///
    /// # Errors
    ///
    /// The error Go's method returns. jsoniter reports it as
    /// `unmarshalerDecoder: <text>, error found in ...`; `encoding/json` returns it as is.
    fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError>;
}

/// A Go `MarshalJSON` method (value receiver).
///
/// Use it with `#[gojson(marshaler)]` on a derived type. jsoniter writes the returned bytes
/// verbatim (minus one trailing newline); `encoding/json` compacts them and reports invalid
/// JSON as a `MarshalerError`.
pub trait GoMarshaler {
    /// Go's `MarshalJSON()`.
    ///
    /// # Errors
    ///
    /// The error Go's method returns.
    fn marshal_json(&self) -> Result<Vec<u8>, JsonError>;
}

/// jsoniter's `unmarshalerDecoder.Decode`.
pub fn jsoniter_unmarshal<T: GoUnmarshaler + ?Sized>(v: &mut T, it: &mut Iter<'_>) {
    // Go: json-iterator/go reflect_marshaler.go unmarshalerDecoder.Decode
    let _ = it.next_token();
    it.unread_byte();
    let bytes = it.skip_and_return_bytes();
    if let Err(err) = v.unmarshal_json(&bytes) {
        it.report_error("unmarshalerDecoder", &err.go_text());
    }
}

/// `encoding/json`: `UnmarshalJSON` with the raw value (the `indirect` hit in `value`).
///
/// # Errors
///
/// Returns the method's error, which aborts decoding.
pub fn std_unmarshal<T: GoUnmarshaler + ?Sized>(
    v: &mut T,
    d: &mut StdDecoder<'_>,
) -> Result<(), JsonError> {
    let raw = d.raw_value();
    v.unmarshal_json(raw)
}

/// `encoding/json`: `literalStore` for a type with `UnmarshalJSON`, named `N`.
///
/// # Errors
///
/// Returns the method's error, which aborts decoding.
pub fn std_unmarshal_literal<N: GoType + ?Sized, T: GoUnmarshaler + ?Sized>(
    v: &mut T,
    item: &[u8],
    d: &mut StdDecoder<'_>,
) -> Result<(), JsonError> {
    if item.is_empty() {
        d.save_error(StdDecoder::invalid_string_use::<N>(item));
        return Ok(());
    }
    v.unmarshal_json(item)
}

/// Go's `MarshalerError` text.
pub(crate) fn marshaler_error<N: GoType + ?Sized>(err: &JsonError) -> Vec<u8> {
    let mut text = b"json: error calling MarshalJSON for type ".to_vec();
    let mut name = String::new();
    N::go_type_name(&mut name);
    text.extend_from_slice(name.as_bytes());
    text.extend_from_slice(b": ");
    text.extend_from_slice(&err.go_text());
    text
}

/// Writes `MarshalJSON` output the way the encoder's flavour does, for a type named `N`.
pub fn encode_marshaled<N: GoType + ?Sized>(result: Result<Vec<u8>, JsonError>, enc: &mut Encoder) {
    match enc.flavor() {
        Flavor::Jsoniter => match result {
            // Go: json-iterator/go reflect_marshaler.go marshalerEncoder.Encode
            Ok(mut bytes) => {
                if bytes.last() == Some(&b'\n') {
                    let _newline = bytes.pop();
                }
                enc.write_raw(&bytes);
            }
            Err(err) => enc.set_error(err.go_text()),
        },
        Flavor::Std { escape_html } => {
            // Go: encoding/json/encode.go marshalerEncoder
            let compacted = result.and_then(|bytes| {
                let mut out = Vec::with_capacity(bytes.len());
                append_compact(&mut out, &bytes, escape_html).map(|()| out)
            });
            match compacted {
                Ok(out) => enc.write_raw(&out),
                Err(err) => enc.set_error(marshaler_error::<N>(&err)),
            }
        }
    }
}

/// Encodes a value through its `MarshalJSON`.
pub fn encode_marshaler<T: GoMarshaler + GoType + ?Sized>(v: &T, enc: &mut Encoder) {
    encode_marshaled::<T>(v.marshal_json(), enc);
}

/// Encodes a pointer to a value with `MarshalJSON`: nil is `null`; otherwise the method runs,
/// and `encoding/json` names the pointer type (`*T`) in its `MarshalerError`.
pub fn encode_marshaler_ptr<T: GoMarshaler + GoType>(v: Option<&T>, enc: &mut Encoder) {
    // Go: encoding/json/encode.go marshalerEncoder (v.Type() is the pointer type);
    // json-iterator/go reflect_marshaler.go marshalerEncoder
    match v {
        None => enc.write_null(),
        Some(inner) => encode_marshaled::<Option<T>>(inner.marshal_json(), enc),
    }
}
