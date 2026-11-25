//! Glue (Rule B (a)): five adapters read the `ext: null` of bid responses as `json.Unmarshal`.
//!
//! Real traffic sends `ext: null`, and Go's two decoders read it apart: the zentotem, loopme,
//! resetdigital, tradplus and adverxo adapters decode their bid responses with `json.Unmarshal`,
//! which keeps a `json.RawMessage` given null as the bytes `null`, where `jsonutil.Unmarshal`,
//! everywhere else, leaves it nil (`json.null-raw`, restored, pinned by
//! `tests/parity/shapes.rs::raw_null_follows_the_decoder` against the shapes golden).
//! [`EncodingJson`] is that glue: it reads one document as `json.Unmarshal` does.
//!
//! The rest of the module is the generated serde code's bridge to the types it shares with
//! `GoJson`'s derive, and holds no Go behavior of its own. While that derive stays (typegen's
//! `serde_bridge`), a `json.RawMessage` is a [`RawJson`], which has no serde or `MergeClone` impls
//! of its own, and pbs-jsonutil merges no `isize` (Go `int`); each function applies pbs-jsonutil's
//! helper for the case to the `GoJson`-era type, so the fields read, write and merge as
//! pbs-jsonutil's types do: [`raw_json`] for a `json.RawMessage` field, [`bridged`] for a pointer,
//! slice or map that holds them (`map[string]json.RawMessage`), and [`merge_field`] over
//! [`BridgeMerge`], which merges Go `int`, `uint`, `RawJson` and the `Option`, `Vec` and
//! `BTreeMap` that hold them with pbs-jsonutil's own rules. Another crate's generated code uses
//! the module as its `serde_bridge` too (`"serde_bridge": "pbs_openrtb::gojson_bridge"`), so its
//! `json.RawMessage` fields follow [`EncodingJson`] as well. The bridge goes with `GoJson`'s
//! derive, when `json.RawMessage` fields become `Option<Box<RawValue>>`; [`EncodingJson`] outlives
//! it.

use core::{cell::Cell, mem};
use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_gojson::RawJson;
use pbs_jsonutil::{
    MergeClone, MergeCloneError, raw_compact, raw_keep_null, replace_lone_surrogates,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned,
    ser::Error as SerializeError,
};
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
/// (`json.null-raw`). Decode with [`EncodingJson::unmarshal`], which also fails as
/// `json.Unmarshal` fails; the other ways the two Go decoders part are ledger entries of their
/// own (`json.key-case`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodingJson<T>(pub T);

impl<T: DeserializeOwned> EncodingJson<T> {
    /// Go: `json.Unmarshal(data, &v)`: decodes `data` into a `T` as `encoding/json` does.
    ///
    /// Lone surrogate escapes are rewritten first (`pbs_jsonutil::replace_lone_surrogates`), as
    /// for every decode of outside bytes. The error is what `json.Unmarshal`'s is to Prebid: a
    /// plain Go error ([`GoError::generic`]), which no `errortypes` type wraps, so
    /// `errortypes.ReadCode` gives it `UnknownErrorCode` (999) where `jsonutil.Unmarshal`'s
    /// `FailedToUnmarshal` has 14. The zentotem, loopme, tradplus and adverxo adapters return it
    /// as it is and resetdigital puts its text into a `BadServerResponse`; the text is
    /// `serde_json`'s where Go's is `encoding/json`'s (`json.std-error-texts`).
    ///
    /// # Errors
    ///
    /// A [`GoError::generic`] holding `serde_json`'s message when `data` is not a `T`.
    pub fn unmarshal(data: &[u8]) -> Result<T, GoError> {
        serde_json::from_slice::<Self>(&replace_lone_surrogates(data))
            .map(|Self(value)| value)
            .map_err(|e| GoError::generic(e.to_string()))
    }
}

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

/// How the bridge reads and writes a type that holds bridged types (see [`bridged`]): a
/// [`RawJson`], or an `Option`, `Vec` or `BTreeMap` of a type that is one.
pub trait BridgeSerde: Sized {
    /// Writes `self` as the generated code writes it.
    ///
    /// # Errors
    ///
    /// When a value cannot be written, or `serializer` fails.
    fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;

    /// Reads a value as the generated code reads it.
    ///
    /// # Errors
    ///
    /// When the input is not a value of the type.
    fn bridge_deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error>;
}

/// A `json.RawMessage` reads and writes as [`raw_json`] does.
impl BridgeSerde for RawJson {
    fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        raw_json::serialize(self, serializer)
    }

    fn bridge_deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        raw_json::deserialize(deserializer)
    }
}

/// A pointer: nil is null, as Go writes it, and null reads nil.
impl<T: BridgeSerde> BridgeSerde for Option<T> {
    fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(value) => serializer.serialize_some(&Bridged(value)),
            None => serializer.serialize_none(),
        }
    }

    fn bridge_deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<Unbridged<T>>::deserialize(deserializer).map(|value| value.map(|Unbridged(v)| v))
    }
}

/// A slice, element by element.
impl<T: BridgeSerde> BridgeSerde for Vec<T> {
    fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Bridged))
    }

    fn bridge_deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<Unbridged<T>>::deserialize(deserializer)
            .map(|values| values.into_iter().map(|Unbridged(v)| v).collect())
    }
}

/// A map, value by value; its keys read and write as serde reads and writes them.
impl<K: Ord + Serialize + DeserializeOwned, V: BridgeSerde> BridgeSerde for BTreeMap<K, V> {
    fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.iter().map(|(key, value)| (key, Bridged(value))))
    }

    fn bridge_deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::<K, Unbridged<V>>::deserialize(deserializer).map(|entries| {
            entries
                .into_iter()
                .map(|(key, Unbridged(value))| (key, value))
                .collect()
        })
    }
}

/// A value the bridge writes, as `Serialize`.
struct Bridged<'a, T>(&'a T);

impl<T: BridgeSerde> Serialize for Bridged<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.bridge_serialize(serializer)
    }
}

/// A value the bridge reads, as `Deserialize`.
struct Unbridged<T>(T);

impl<'de, T: BridgeSerde> Deserialize<'de> for Unbridged<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::bridge_deserialize(deserializer).map(Self)
    }
}

/// A pointer, slice or map field that holds bridged types (`with = "gojson_bridge::bridged"`).
///
/// `map[string]json.RawMessage` is an `Option<BTreeMap<String, RawJson>>` whose values read and
/// write as [`raw_json`] does, null as nil and nil as null (and, inside an [`EncodingJson`], null
/// as the bytes `null`), at any depth ([`BridgeSerde`]).
pub mod bridged {
    use super::{BridgeSerde, Deserializer, Serializer};

    /// Writes `value`, each bridged value as its bridge writes it.
    ///
    /// # Errors
    ///
    /// When a value cannot be written, or `serializer` fails.
    pub fn serialize<T: BridgeSerde, S: Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.bridge_serialize(serializer)
    }

    /// Reads a value, each bridged value as its bridge reads it.
    ///
    /// # Errors
    ///
    /// When the input is not a value of the type.
    pub fn deserialize<'de, T: BridgeSerde, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        T::bridge_deserialize(deserializer)
    }
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

/// A slice merges as pbs-jsonutil merges a `Vec`, which runs the merge: an array gives a new
/// slice whose elements each merge into a new zero value, and a merge that fails keeps the
/// elements merged before.
impl<T: BridgeMerge + Default> BridgeMerge for Vec<T> {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let mut merged: Vec<Merged<T>> = mem::take(self).into_iter().map(Merged).collect();
        let result = merged.merge_clone(patch);
        *self = merged.into_iter().map(|Merged(value)| value).collect();
        result
    }
}

/// A map merges as pbs-jsonutil merges a `BTreeMap`, which runs the merge: the entries an object
/// does not name stay, and each entry it names becomes its value merged into a new zero value.
impl<K: Ord + DeserializeOwned, V: BridgeMerge + Default> BridgeMerge for BTreeMap<K, V> {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let mut merged: BTreeMap<K, Merged<V>> = mem::take(self)
            .into_iter()
            .map(|(key, value)| (key, Merged(value)))
            .collect();
        let result = merged.merge_clone(patch);
        *self = merged
            .into_iter()
            .map(|(key, Merged(value))| (key, value))
            .collect();
        result
    }
}

/// An element or map value that merges through the bridge, as pbs-jsonutil's `MergeClone`, so
/// that pbs-jsonutil's own `Vec` and `BTreeMap` merges hold it.
#[derive(Default)]
struct Merged<T>(T);

impl<T: BridgeMerge> MergeClone for Merged<T> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.0.bridge_merge(patch)
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
    use std::collections::BTreeMap;

    use pbs_errortypes::{FAILED_TO_UNMARSHAL_ERROR_CODE, UNKNOWN_ERROR_CODE, read_code};
    use pbs_gojson::RawJson;
    use pbs_jsonutil::{is_nil_or_empty, unmarshal};
    use serde::{Deserialize, Serialize};
    use serde_json::value::RawValue;

    use super::{BridgeMerge as _, EncodingJson, merge_field};
    use crate::openrtb2::BidResponse;

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

    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    struct Holders {
        #[serde(
            rename = "m",
            skip_serializing_if = "is_nil_or_empty",
            with = "super::bridged"
        )]
        m: Option<BTreeMap<String, RawJson>>,
        #[serde(rename = "s", with = "super::bridged")]
        s: Option<Vec<RawJson>>,
        #[serde(
            rename = "p",
            skip_serializing_if = "Option::is_none",
            with = "super::bridged"
        )]
        p: Option<RawJson>,
    }

    #[test]
    fn reads_and_writes_containers_of_raw_messages() -> Result<(), String> {
        let doc = r#"{"m":{"a":null,"b":{ "x" : 1 }},"s":[null,[ 1 ]],"p":{ "y" : 2 }}"#;
        let plain: Holders = serde_json::from_str(doc).map_err(|e| e.to_string())?;
        let want = Holders {
            m: Some(BTreeMap::from([
                ("a".to_owned(), RawJson::nil()),
                ("b".to_owned(), RawJson::from_slice(br#"{ "x" : 1 }"#)),
            ])),
            s: Some(vec![RawJson::nil(), RawJson::from_slice(b"[ 1 ]")]),
            p: Some(RawJson::from_slice(br#"{ "y" : 2 }"#)),
        };
        if plain != want {
            return Err(format!("read: {plain:?}"));
        }
        let text = serde_json::to_string(&plain).map_err(|e| e.to_string())?;
        if text != r#"{"m":{"a":null,"b":{"x":1}},"s":[null,[1]],"p":{"y":2}}"# {
            return Err(format!("written: {text}"));
        }
        // json.Unmarshal keeps the bytes null of a map value and a slice element; a pointer is nil.
        let kept = EncodingJson::<Holders>::unmarshal(doc.as_bytes()).map_err(|e| e.to_string())?;
        let null = RawJson::from_slice(b"null");
        let a = kept.m.as_ref().and_then(|m| m.get("a"));
        if a != Some(&null) || kept.s.as_ref().and_then(|s| s.first()) != Some(&null) {
            return Err(format!("inside EncodingJson: {kept:?}"));
        }
        let nil = EncodingJson::<Holders>::unmarshal(br#"{"m":null,"s":null,"p":null}"#)
            .map_err(|e| e.to_string())?;
        if nil != Holders::default() {
            return Err(format!("null: {nil:?}"));
        }
        let empty: Holders =
            serde_json::from_str(r#"{"m":{},"s":[]}"#).map_err(|e| e.to_string())?;
        let written = serde_json::to_string(&empty).map_err(|e| e.to_string())?;
        if empty.m != Some(BTreeMap::new()) || written != r#"{"s":[]}"# {
            return Err(format!("empty: {empty:?} {written}"));
        }
        let none = serde_json::to_string(&Holders::default()).map_err(|e| e.to_string())?;
        if none != r#"{"s":null}"# {
            return Err(format!("nil: {none}"));
        }
        let bad = Holders {
            m: Some(BTreeMap::from([(
                "a".to_owned(),
                RawJson::from_slice(b"{"),
            )])),
            ..Holders::default()
        };
        if serde_json::to_string(&bad).is_ok() {
            return Err("invalid bytes written".to_owned());
        }
        Ok(())
    }

    /// `json.std-error-texts`: `EncodingJson::unmarshal` fails as `json.Unmarshal` does, with a
    /// plain error (`errortypes.ReadCode` 999, where `jsonutil.Unmarshal`'s has 14), whose text is
    /// `serde_json`'s message where Go's is `encoding/json`'s.
    #[test]
    fn unmarshals_and_fails_as_encoding_json() -> Result<(), String> {
        let response = EncodingJson::<BidResponse>::unmarshal(
            br#"{"id":"r","seatbid":[{"bid":[{"id":"b","impid":"i","price":1.5,"ext":null}]}]}"#,
        )
        .map_err(|e| e.to_string())?;
        let ext = response
            .seatbid
            .as_deref()
            .and_then(<[_]>::first)
            .and_then(|seat| seat.bid.as_deref())
            .and_then(<[_]>::first)
            .map(|bid| bid.ext.clone());
        if ext != Some(RawJson::from_slice(b"null")) {
            return Err(format!("a bid's ext: null read as {ext:?}"));
        }
        for (doc, go, text) in [
            (
                &b"{"[..],
                "unexpected end of JSON input",
                "EOF while parsing an object at line 1 column 1",
            ),
            (
                br#"{"seatbid":[{"bid":[{"price":"1"}]}]}"#,
                "json: cannot unmarshal string into Go struct field Bid.seatbid.bid.price of type \
                 float64",
                "invalid type: string \"1\", expected f64 at line 1 column 32",
            ),
        ] {
            let err = EncodingJson::<BidResponse>::unmarshal(doc)
                .err()
                .ok_or_else(|| format!("{go}: decoded"))?;
            if read_code(&err) != UNKNOWN_ERROR_CODE || err.message() != text {
                return Err(format!("{go}: {} {err}", read_code(&err)));
            }
            let wrapped = unmarshal::<BidResponse>(doc)
                .err()
                .ok_or_else(|| format!("{go}: decoded by unmarshal"))?;
            if read_code(&wrapped) != FAILED_TO_UNMARSHAL_ERROR_CODE {
                return Err(format!("{go}: unmarshal's code {}", read_code(&wrapped)));
            }
        }
        // A lone surrogate escape reads as U+FFFD, as for every decode of outside bytes.
        let lone = EncodingJson::<BidResponse>::unmarshal(br#"{"id":"\ud800"}"#)
            .map_err(|e| e.to_string())?;
        if lone.id != "\u{fffd}" {
            return Err(format!("a lone surrogate: {:?}", lone.id));
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
