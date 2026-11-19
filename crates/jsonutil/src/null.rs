//! Glue (Rule B (a)): real traffic sends JSON null for value fields and for `ext`. Go leaves a
//! non-pointer field at its zero value when it reads null, where a serde derive rejects the whole
//! document (`json.null-into-value`, restored by [`null_default`]); and `encoding/json` keeps a
//! `json.RawMessage` given null as the bytes `null` and writes it back, where serde reads null into
//! an `Option` as `None` (`json.null-raw`, restored by [`raw_keep_null`] for the fields Go decodes
//! with `encoding/json`).

use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

/// A `deserialize_with` helper that reads JSON null into a value field as Go's zero value.
///
/// The field is one Go declares as a value, not a pointer: null gives `T::default()` instead of
/// an error, and any other value decodes as `T` does. The generated types put it on every value
/// field, with `#[serde(default)]` on the container for the fields a document leaves out.
///
/// # Errors
///
/// Whatever `T`'s own decoder reports for a value that is not null.
pub fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// A `deserialize_with` helper that keeps JSON null in a `json.RawMessage` field as `Some(null)`.
///
/// It is for the fields Go decodes with `encoding/json`, which keeps null as the bytes `null`:
/// the field is written back as `null`, and an absent field stays `None` through
/// `#[serde(default)]`.
///
/// `jsonutil.Unmarshal` (jsoniter) reads null into a `json.RawMessage` as nil, as a plain
/// `Option<Box<RawValue>>` does, so the fields it decodes do not take this helper.
///
/// # Errors
///
/// When the value is not valid JSON.
pub fn raw_keep_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(deserializer).map(Some)
}
