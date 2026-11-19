//! The generated serde code's bridge to the types it shares with `GoJson`'s derive.
//!
//! While that derive stays (typegen's `serde_bridge`), a `json.RawMessage` is a [`RawJson`], which
//! has no serde or `MergeClone` impls of its own, and pbs-jsonutil merges no `isize` (Go `int`).
//! The bridge holds no Go behavior of its own: each function applies pbs-jsonutil's helper for the
//! case to the `GoJson`-era type, so the fields read, write and merge as pbs-jsonutil's types do.
//! It goes with `GoJson`'s derive, when `json.RawMessage` fields become `Option<Box<RawValue>>`.
//!
//! [`EncodingJson`] is the one piece that outlives it: the generated types read a
//! `json.RawMessage` given null as `jsonutil.Unmarshal` does, and through it as `json.Unmarshal`
//! does (`json.null-raw`).

use core::cell::Cell;

use pbs_gojson::RawJson;
use pbs_jsonutil::{MergeClone as _, MergeCloneError, raw_compact, raw_keep_null};
use serde::{Deserialize, Deserializer, Serializer, ser::Error as SerializeError};
use serde_json::value::RawValue;

thread_local! {
    /// Whether the decode running on this thread reads as `encoding/json` does: set while an
    /// [`EncodingJson`] reads its value.
    static ENCODING_JSON: Cell<bool> = const { Cell::new(false) };
}

/// A `T` read as Go's `encoding/json` reads it.
///
/// For the sites where Prebid decodes a type of this crate with `json.Unmarshal`: the bid
/// responses of the zentotem, loopme, resetdigital, tradplus and adverxo adapters. A
/// `json.RawMessage` given null keeps the bytes `null` (`pbs_jsonutil::raw_keep_null`), as
/// `json.Unmarshal` keeps them, where `T` alone reads nil, as `jsonutil.Unmarshal` does
/// (`json.null-raw`). Decode `pbs_jsonutil::unmarshal::<EncodingJson<T>>` and take the value.
/// The other ways the two Go decoders part are ledger entries of their own (`json.key-case`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodingJson<T>(pub T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for EncodingJson<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let _mode = Mode::enter();
        T::deserialize(deserializer).map(Self)
    }
}

/// [`ENCODING_JSON`] set while an [`EncodingJson`] reads, put back as it was on drop, so a failed
/// or nested read leaves the thread as it found it.
struct Mode {
    /// The setting before.
    previous: bool,
}

impl Mode {
    /// Sets [`ENCODING_JSON`].
    fn enter() -> Self {
        Self {
            previous: ENCODING_JSON.replace(true),
        }
    }
}

impl Drop for Mode {
    fn drop(&mut self) {
        ENCODING_JSON.set(self.previous);
    }
}

/// pbs-jsonutil's text for a merge into a `json.RawMessage` that is not JSON.
const INVALID_EXISTING: &str = "invalid json on existing object";

/// A `json.RawMessage` field (`with = "gojson_bridge::raw_json"`).
///
/// Read as `jsonutil.Unmarshal` reads it, JSON null as nil (inside an [`EncodingJson`], as
/// `json.Unmarshal` reads it), and written as `jsonutil.Marshal` writes it.
pub mod raw_json {
    use super::{
        Deserialize as _, Deserializer, ENCODING_JSON, RawJson, RawValue, Serializer, raw_compact,
        raw_keep_null, raw_value,
    };

    /// Writes the message compacted, as Prebid's `RawMessageExtension` does
    /// (`pbs_jsonutil::raw_compact`); nil or empty writes `null`, where Go writes nothing at all
    /// (`json.nil-raw-written`).
    ///
    /// # Errors
    ///
    /// When the message is not JSON, or `serializer` fails.
    pub fn serialize<S: Serializer>(raw: &RawJson, serializer: S) -> Result<S::Ok, S::Error> {
        if raw.is_empty() {
            return serializer.serialize_none();
        }
        let value = raw_value::<S::Error>(raw)?;
        raw_compact(&value, serializer)
    }

    /// Reads the value's bytes as written, and JSON null as nil, as jsoniter does; inside an
    /// [`EncodingJson`](super::EncodingJson), JSON null as the bytes `null`, as `encoding/json`
    /// does.
    ///
    /// # Errors
    ///
    /// When the input is not JSON.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<RawJson, D::Error> {
        let raw = if ENCODING_JSON.get() {
            raw_keep_null(deserializer)?
        } else {
            Option::<Box<RawValue>>::deserialize(deserializer)?
        };
        Ok(from_raw(raw.as_deref()))
    }

    /// Go's `omitempty` test for a `json.RawMessage`: nil or empty.
    #[must_use]
    pub fn is_empty(raw: &RawJson) -> bool {
        raw.is_empty()
    }

    /// The message holding `raw`'s bytes, nil for `None`.
    pub(super) fn from_raw(raw: Option<&RawValue>) -> RawJson {
        raw.map_or_else(RawJson::nil, |raw| {
            RawJson::from_slice(raw.get().as_bytes())
        })
    }
}

/// A `json.RawMessage` field that Prebid decodes with `encoding/json`.
///
/// Typegen's `keep_null` gives it `with = "gojson_bridge::raw_json_keep_null"`: JSON null is kept
/// as the bytes `null` (`pbs_jsonutil::raw_keep_null`), and written back.
pub mod raw_json_keep_null {
    pub use super::raw_json::{is_empty, serialize};
    use super::{Deserializer, RawJson, raw_json::from_raw, raw_keep_null};

    /// Reads the value's bytes as written, JSON null included, as `encoding/json` does.
    ///
    /// # Errors
    ///
    /// When the input is not JSON.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<RawJson, D::Error> {
        raw_keep_null(deserializer).map(|raw| from_raw(raw.as_deref()))
    }
}

/// `raw` as a `RawValue`.
fn raw_value<E: SerializeError>(raw: &RawJson) -> Result<Box<RawValue>, E> {
    serde_json::from_slice(raw.as_bytes()).map_err(E::custom)
}

/// `MergeClone` for the types the generated code merges through the bridge (see
/// [`merge_field`]).
pub trait BridgeMerge {
    /// Merges `patch`, one JSON value, into `self` with Go's `MergeClone` rules.
    ///
    /// # Errors
    ///
    /// When `patch` is not of the kind the type reads.
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError>;
}

/// A `json.RawMessage` merges as pbs-jsonutil merges an `Option<Box<RawValue>>`: null keeps it,
/// a nil or empty one takes the value as written, and any other is merge-patched.
impl BridgeMerge for RawJson {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let mut raw = if self.is_empty() {
            None
        } else {
            match serde_json::from_slice::<Box<RawValue>>(self.as_bytes()) {
                Ok(raw) => Some(raw),
                Err(_) => return Err(MergeCloneError::new(INVALID_EXISTING)),
            }
        };
        raw.merge_clone(patch)?;
        if let Some(raw) = raw {
            *self = Self::from_slice(raw.get().as_bytes());
        }
        Ok(())
    }
}

/// Go `int` and `uint` merge as pbs-jsonutil merges the other integers: null leaves the value as
/// it is, any other value replaces it.
macro_rules! bridge_integer {
    ($($ty:ty),+) => {$(
        impl BridgeMerge for $ty {
            fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
                if patch.get() != "null" {
                    *self = serde_json::from_str(patch.get())
                        .map_err(|e| MergeCloneError::new(e.to_string()))?;
                }
                Ok(())
            }
        }
    )+};
}

bridge_integer!(isize, usize);

/// A pointer merges as pbs-jsonutil merges an `Option`: null sets it to nil; any other value is
/// merged into what it points to, or into a new zero value when it is nil.
impl<T: BridgeMerge + Default> BridgeMerge for Option<T> {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        if patch.get() == "null" {
            *self = None;
            return Ok(());
        }
        self.get_or_insert_with(T::default).bridge_merge(patch)
    }
}

/// `pbs_jsonutil::merge_field` for a field that merges through the bridge: merges `value` into
/// the field `go_field` of the Go type `go_type`, naming the field in the error.
///
/// # Errors
///
/// When the field's merge fails.
pub fn merge_field<T: BridgeMerge + ?Sized>(
    target: &mut T,
    value: &RawValue,
    go_type: &str,
    go_field: &str,
) -> Result<(), MergeCloneError> {
    target
        .bridge_merge(value)
        .map_err(|e| e.within(go_type, go_field))
}

/// The merge of a named type whose underlying type merges through the bridge.
///
/// # Errors
///
/// When the merge fails.
pub fn merge<T: BridgeMerge + ?Sized>(
    target: &mut T,
    patch: &RawValue,
) -> Result<(), MergeCloneError> {
    target.bridge_merge(patch)
}

#[cfg(test)]
mod tests {
    use pbs_gojson::RawJson;
    use serde::{Deserialize, Serialize};
    use serde_json::value::RawValue;

    use super::{BridgeMerge as _, EncodingJson, merge_field};

    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    struct Both {
        #[serde(rename = "a", with = "super::raw_json")]
        a: RawJson,
        #[serde(rename = "b", with = "super::raw_json_keep_null")]
        b: RawJson,
        #[serde(rename = "c", skip_serializing_if = "super::raw_json::is_empty")]
        #[serde(with = "super::raw_json")]
        c: RawJson,
    }

    #[test]
    fn reads_and_writes_raw_messages() -> Result<(), String> {
        let both: Both = serde_json::from_str(r#"{"a":null,"b":null,"c":{ "x" : 1 }}"#)
            .map_err(|e| e.to_string())?;
        if both.a != RawJson::nil() || both.b != RawJson::from_slice(b"null") {
            return Err(format!("null: {both:?}"));
        }
        if both.c != RawJson::from_slice(br#"{ "x" : 1 }"#) {
            return Err(format!("bytes as written: {both:?}"));
        }
        let text = serde_json::to_string(&both).map_err(|e| e.to_string())?;
        if text != r#"{"a":null,"b":null,"c":{"x":1}}"# {
            return Err(format!("written: {text}"));
        }
        let empty = serde_json::to_string(&Both::default()).map_err(|e| e.to_string())?;
        if empty != r#"{"a":null,"b":null}"# {
            return Err(format!("nil: {empty}"));
        }
        let bad = Both {
            a: RawJson::from_slice(b"{"),
            ..Both::default()
        };
        if serde_json::to_string(&bad).is_ok() {
            return Err("invalid bytes written".to_owned());
        }
        Ok(())
    }

    #[test]
    fn reads_null_as_encoding_json_does_inside_encoding_json() -> Result<(), String> {
        let doc = r#"{"a":null,"b":null}"#;
        let EncodingJson(kept) =
            serde_json::from_str::<EncodingJson<Both>>(doc).map_err(|e| e.to_string())?;
        let null = RawJson::from_slice(b"null");
        if kept.a != null || kept.b != null {
            return Err(format!("inside EncodingJson: {kept:?}"));
        }
        let nested = r#"[{"a":null},{"a":null,"c":nope}]"#;
        if serde_json::from_str::<EncodingJson<Vec<Both>>>(nested).is_ok() {
            return Err("a document that is not JSON read".to_owned());
        }
        let plain: Both = serde_json::from_str(doc).map_err(|e| e.to_string())?;
        if plain.a != RawJson::nil() || plain.b != null {
            return Err(format!("after a read that failed: {plain:?}"));
        }
        Ok(())
    }

    #[test]
    fn merges_through_the_bridge() -> Result<(), String> {
        let patch = |text: &str| -> Result<Box<RawValue>, String> {
            RawValue::from_string(text.to_owned()).map_err(|e| e.to_string())
        };
        let mut ext = RawJson::nil();
        ext.bridge_merge(&patch(r#"{ "a" : 1 }"#)?)
            .map_err(|e| e.to_string())?;
        ext.bridge_merge(&patch("null")?)
            .map_err(|e| e.to_string())?;
        ext.bridge_merge(&patch(r#"{"b":2}"#)?)
            .map_err(|e| e.to_string())?;
        if ext != RawJson::from_slice(br#"{"a":1,"b":2}"#) {
            return Err(format!("merged: {ext:?}"));
        }
        let mut broken = RawJson::from_slice(b"{");
        let invalid = broken.bridge_merge(&patch("{}")?).err();
        if invalid.map(|e| e.to_string()).as_deref() != Some("invalid json on existing object") {
            return Err("an existing message that is not JSON".to_owned());
        }
        let mut count: Option<isize> = None;
        let context = merge_field(&mut count, &patch(r#""1""#)?, "openrtb2.Refresh", "Count")
            .err()
            .map(|e| e.context().map(str::to_owned));
        if count != Some(0) || context != Some(Some("openrtb2.Refresh.Count".to_owned())) {
            return Err(format!("wrong type: {count:?} {context:?}"));
        }
        merge_field(&mut count, &patch("7")?, "openrtb2.Refresh", "Count")
            .map_err(|e| e.to_string())?;
        merge_field(&mut count, &patch("null")?, "openrtb2.Refresh", "Count")
            .map_err(|e| e.to_string())?;
        let mut wide: usize = 3;
        wide.bridge_merge(&patch("null")?)
            .map_err(|e| e.to_string())?;
        if count.is_some() || wide != 3 {
            return Err(format!("null: {count:?} {wide}"));
        }
        Ok(())
    }
}
