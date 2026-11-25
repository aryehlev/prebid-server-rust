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
//! `GoJson`'s derive. While that derive stays (typegen's `serde_bridge`), a `json.RawMessage` is a
//! [`RawJson`], a `json.Number` a [`JsonNumber`] and `jsonutil.StringInt` and `IntString`
//! pbs-gojson's [`StringInt`] and [`IntString`], which have no serde or `MergeClone` impls of their
//! own, and pbs-jsonutil merges no `isize` (Go `int`); each function applies pbs-jsonutil's helper
//! or type for the case to the `GoJson`-era type, so the fields read, write and merge as
//! pbs-jsonutil's types do: [`raw_json`] for a `json.RawMessage` field, [`string_int`] and
//! [`int_string`] for pbs-jsonutil's `StringInt` and `IntString`, [`bridged`] for a pointer, slice
//! or map that holds them (`map[string]json.RawMessage`), and [`merge_field`] over
//! [`BridgeMerge`], which merges them, Go `int` and `uint`, and the `Option`, `Vec` and `BTreeMap`
//! that hold them with pbs-jsonutil's own rules. pbs-jsonutil has no `json.Number`, so
//! [`json_number`] reads and writes one itself, by pbs-gojson's rules for it. Another crate's
//! generated code uses the module as its `serde_bridge` too (`"serde_bridge":
//! "pbs_openrtb::gojson_bridge"`, and an extern type's `serde_bridge` names `string_int` or
//! `int_string`), so its fields follow [`EncodingJson`] as well. The bridge goes with `GoJson`'s
//! derive, when the fields take pbs-jsonutil's types (`Option<Box<RawValue>>` for a
//! `json.RawMessage`); [`EncodingJson`] outlives it.

use core::{cell::Cell, mem, num::TryFromIntError};
use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_gojson::{
    JsonNumber, RawJson,
    jsonutil::{IntString, StringInt},
};
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

/// A `json.Number` field (`with = "gojson_bridge::json_number"`).
///
/// Read as jsoniter reads it (pbs-gojson's [`JsonNumber`] gives the rules): a string's text
/// unvalidated, null as `""` and a number's literal text as it is written; inside an
/// [`EncodingJson`], as `encoding/json` reads it, a string only when its text is a JSON number.
/// Written as `jsonutil.Marshal` writes it: the text as it is, and `0` for `""`. `serde_json`
/// reads and writes only JSON, where jsoniter keeps any run of number bytes (`+1`, `.5`) and writes
/// any text (`json.number-text`).
pub mod json_number {
    use serde::de::Error as _;

    use super::{
        Deserialize as _, Deserializer, ENCODING_JSON, JsonNumber, RawValue, Serialize as _,
        SerializeError as _, Serializer,
    };

    /// Writes the text as it is, and `0` for `""`.
    ///
    /// # Errors
    ///
    /// When the text is not one JSON value, which Go writes as it is (`json.number-text`), or
    /// `serializer` fails.
    pub fn serialize<S: Serializer>(number: &JsonNumber, serializer: S) -> Result<S::Ok, S::Error> {
        let text = if number.is_empty() {
            "0".to_owned()
        } else {
            String::from_utf8_lossy(number.as_bytes()).into_owned()
        };
        RawValue::from_string(text)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }

    /// Reads a number as jsoniter does, and inside an [`EncodingJson`](super::EncodingJson) as
    /// `encoding/json` does.
    ///
    /// # Errors
    ///
    /// When the value is not a number, a string or null, or is a string whose text is not a JSON
    /// number inside an [`EncodingJson`](super::EncodingJson).
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JsonNumber, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        read(&raw, ENCODING_JSON.get()).map_err(D::Error::custom)
    }

    /// Go's `omitempty` test for a `json.Number`: `""`.
    #[must_use]
    pub const fn is_empty(number: &JsonNumber) -> bool {
        number.is_empty()
    }

    /// `raw` read as a `json.Number`: as `encoding/json` reads it when `std`, else as jsoniter
    /// does.
    pub(super) fn read(raw: &RawValue, std: bool) -> Result<JsonNumber, String> {
        let text = raw.get();
        match text.as_bytes().first() {
            Some(b'"') => {
                let inner: String = serde_json::from_str(text).map_err(|e| e.to_string())?;
                if std && !is_number(&inner) {
                    return Err(format!(
                        "invalid number literal, trying to unmarshal {text} into Number"
                    ));
                }
                Ok(JsonNumber::from(inner))
            }
            Some(b'n') => Ok(JsonNumber::default()),
            Some(b'-' | b'0'..=b'9') => Ok(JsonNumber::from(text)),
            _ => Err("readNumberAsString: invalid number".to_owned()),
        }
    }

    /// Go's `isValidNumber`: whether `text` is one JSON number literal, with nothing around it.
    fn is_number(text: &str) -> bool {
        let bytes = text.as_bytes();
        matches!(bytes.first(), Some(b'-' | b'0'..=b'9'))
            && bytes.last().is_some_and(u8::is_ascii_digit)
            && serde_json::from_str::<&RawValue>(text).is_ok()
    }
}

/// A `jsonutil.StringInt` field (`with = "gojson_bridge::string_int"`): read and written as
/// pbs-jsonutil's `StringInt` (`json.string-int-parse`).
pub mod string_int {
    use serde::de::Error as _;

    use super::{
        Deserialize as _, Deserializer, Serialize as _, SerializeError as _, Serializer, StringInt,
    };

    /// Writes the integer as a number.
    ///
    /// # Errors
    ///
    /// When `serializer` fails.
    pub fn serialize<S: Serializer>(value: &StringInt, serializer: S) -> Result<S::Ok, S::Error> {
        pbs_jsonutil::StringInt(i64::try_from(value.0).map_err(S::Error::custom)?)
            .serialize(serializer)
    }

    /// Reads an integer, or a string holding one, as `(*StringInt).UnmarshalJSON` does.
    ///
    /// # Errors
    ///
    /// When the value is neither.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<StringInt, D::Error> {
        let pbs_jsonutil::StringInt(value) = pbs_jsonutil::StringInt::deserialize(deserializer)?;
        isize::try_from(value)
            .map(StringInt)
            .map_err(D::Error::custom)
    }

    /// Go's `omitempty` test for a `jsonutil.StringInt`: 0.
    #[must_use]
    pub const fn is_empty(value: &StringInt) -> bool {
        value.0 == 0
    }
}

/// A `jsonutil.IntString` field (`with = "gojson_bridge::int_string"`): read and written as
/// pbs-jsonutil's `IntString`.
pub mod int_string {
    use super::{Deserialize as _, Deserializer, IntString, Serialize as _, Serializer};

    /// Writes the text as a string.
    ///
    /// # Errors
    ///
    /// When `serializer` fails.
    pub fn serialize<S: Serializer>(value: &IntString, serializer: S) -> Result<S::Ok, S::Error> {
        pbs_jsonutil::IntString(value.0.to_string_lossy()).serialize(serializer)
    }

    /// Reads a string as its text, and a number as gjson writes it, as
    /// `(*IntString).UnmarshalJSON` does.
    ///
    /// # Errors
    ///
    /// When the value is neither.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<IntString, D::Error> {
        let pbs_jsonutil::IntString(text) = pbs_jsonutil::IntString::deserialize(deserializer)?;
        Ok(IntString(text.into()))
    }

    /// Go's `omitempty` test for a `jsonutil.IntString`: `""`.
    #[must_use]
    pub const fn is_empty(value: &IntString) -> bool {
        value.0.is_empty()
    }
}

/// `raw` as a `RawValue`.
fn raw_value<E: SerializeError>(raw: &RawJson) -> Result<Box<RawValue>, E> {
    serde_json::from_slice(raw.as_bytes()).map_err(E::custom)
}

/// How the bridge reads and writes a type that holds bridged types (see [`bridged`]): a
/// [`RawJson`], [`JsonNumber`], [`StringInt`] or [`IntString`], or an `Option`, `Vec` or
/// `BTreeMap` of a type that is one.
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

/// A bridged type reads and writes as its module does.
macro_rules! bridge_serde {
    ($($ty:ty => $module:ident),+) => {$(
        impl BridgeSerde for $ty {
            fn bridge_serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                $module::serialize(self, serializer)
            }

            fn bridge_deserialize<'de, D: Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                $module::deserialize(deserializer)
            }
        }
    )+};
}

bridge_serde!(
    RawJson => raw_json,
    JsonNumber => json_number,
    StringInt => string_int,
    IntString => int_string
);

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

/// A `json.Number` merges as jsoniter decodes one ([`json_number`]): the value replaces it, null
/// with `""`.
impl BridgeMerge for JsonNumber {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        *self = json_number::read(patch, false).map_err(MergeCloneError::new)?;
        Ok(())
    }
}

/// A `jsonutil.StringInt` merges as pbs-jsonutil's `StringInt` does: `""` leaves it as it is, and
/// any other value is read into it.
impl BridgeMerge for StringInt {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let wide = |e: TryFromIntError| MergeCloneError::new(e.to_string());
        let mut value = pbs_jsonutil::StringInt(i64::try_from(self.0).map_err(wide)?);
        value.merge_clone(patch)?;
        self.0 = isize::try_from(value.0).map_err(wide)?;
        Ok(())
    }
}

/// A `jsonutil.IntString` merges as pbs-jsonutil's `IntString` does: the value, null included, is
/// read into it.
impl BridgeMerge for IntString {
    fn bridge_merge(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let mut value = pbs_jsonutil::IntString(self.0.to_string_lossy());
        value.merge_clone(patch)?;
        self.0 = value.0.into();
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
    use pbs_gojson::{
        JsonNumber, RawJson,
        jsoniter::JsoniterDecode,
        jsonutil::{self as gojsonutil, IntString, StringInt},
        stdjson,
    };
    use pbs_jsonutil::{is_nil_or_empty, marshal, merge_clone, unmarshal};
    use serde::{Deserialize, Serialize};
    use serde_json::value::RawValue;

    use super::{BridgeMerge, EncodingJson, Merged, merge_field};
    use crate::openrtb2::BidResponse;

    /// A `json.Number`, held as the generated code holds one.
    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct Number(#[serde(with = "super::json_number")] JsonNumber);

    /// A `jsonutil.StringInt`, held as the generated code holds one.
    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct Int(#[serde(with = "super::string_int")] StringInt);

    /// A `jsonutil.IntString`, held as the generated code holds one.
    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct Text(#[serde(with = "super::int_string")] IntString);

    /// The inputs whose outcome differs between pbs-gojson's port of Go (`go`) and the bridge
    /// (`port`), each `None` where it fails.
    fn differing<I: Copy, T: PartialEq>(
        inputs: &[I],
        go: impl Fn(I) -> Option<T>,
        port: impl Fn(I) -> Option<T>,
    ) -> Vec<I> {
        inputs
            .iter()
            .copied()
            .filter(|&input| go(input) != port(input))
            .collect()
    }

    /// `doc` read as `jsonutil.Unmarshal` reads it, by pbs-gojson.
    fn go_read<T: JsoniterDecode + Default>(doc: &str) -> Option<T> {
        let mut value = T::default();
        gojsonutil::unmarshal(doc.as_bytes(), &mut value)
            .ok()
            .map(|()| value)
    }

    /// `doc` read as `json.Unmarshal` reads it, by pbs-gojson.
    fn go_read_std(doc: &str) -> Option<JsonNumber> {
        let mut value = JsonNumber::default();
        stdjson::unmarshal(doc.as_bytes(), &mut value)
            .ok()
            .map(|()| value)
    }

    /// `doc` merged into `value` as `jsonutil.MergeClone` merges it, by pbs-gojson.
    fn go_merge<T: JsoniterDecode>(mut value: T, doc: &str) -> Option<T> {
        gojsonutil::merge_clone(&mut value, doc.as_bytes())
            .ok()
            .map(|()| value)
    }

    /// `doc` merged into `value` through the bridge, as the generated code merges a field.
    fn port_merge<T: BridgeMerge>(value: T, doc: &str) -> Option<T> {
        let mut merged = Merged(value);
        merge_clone(&mut merged, doc.as_bytes())
            .ok()
            .map(|()| merged.0)
    }

    #[test]
    fn numbers_read_merge_and_write_as_jsoniter() -> Result<(), String> {
        let docs = [
            "1", "-0", "1.50", "1e400", r#""abc""#, r#""1""#, r#""""#, "null", "true", "[1]", "{}",
            "+1", ".5", "1.2.3", "01", "-",
        ];
        let read = differing(&docs, go_read::<JsonNumber>, |doc| {
            unmarshal::<Number>(doc.as_bytes()).ok().map(|Number(n)| n)
        });
        let std = differing(&docs, go_read_std, |doc| {
            EncodingJson::<Number>::unmarshal(doc.as_bytes())
                .ok()
                .map(|Number(n)| n)
        });
        let merged = differing(
            &docs,
            |doc| go_merge(JsonNumber::from("7"), doc),
            |doc| port_merge(JsonNumber::from("7"), doc),
        );
        // json.number-text: jsoniter reads any run of number bytes, which serde_json rejects.
        let runs = ["+1", ".5", "1.2.3", "01", "-"];
        if read != runs || merged != runs || !std.is_empty() {
            return Err(format!(
                "read {read:?}, merged {merged:?}, json.Unmarshal {std:?}"
            ));
        }
        let texts = [
            "", "1", "1.50", "-0", r#""abc""#, "true", "abc", "1 2", "+1", "<1>",
        ];
        let written = differing(
            &texts,
            |text| gojsonutil::marshal(&JsonNumber::from(text)).ok(),
            |text| marshal(&Number(JsonNumber::from(text))).ok(),
        );
        let std_written = differing(
            &texts,
            |text| stdjson::marshal(&JsonNumber::from(text)).ok(),
            |text| marshal(&Number(JsonNumber::from(text))).ok(),
        );
        // json.number-text: `jsonutil.Marshal` writes the text as it is, JSON or not, and
        // `json.Marshal` fails on a text that is not a number literal.
        if written != ["abc", "1 2", "+1", "<1>"] || std_written != [r#""abc""#, "true"] {
            return Err(format!("written {written:?}, json.Marshal {std_written:?}"));
        }
        Ok(())
    }

    #[test]
    fn string_ints_and_int_strings_follow_pbs_jsonutil() -> Result<(), String> {
        let ints = [
            "5",
            r#""5""#,
            r#""""#,
            "-0",
            r#"" 5""#,
            r#""\u0035""#,
            "9223372036854775807",
            "9223372036854775808",
            "1.5",
            r#""1.5""#,
            "null",
            "true",
            "[]",
            "{}",
            r#""+5""#,
            r#""-""#,
        ];
        let read = differing(&ints, go_read::<StringInt>, |doc| {
            unmarshal::<Int>(doc.as_bytes())
                .ok()
                .map(|Int(value)| value)
        });
        let merged = differing(
            &ints,
            |doc| go_merge(StringInt(7), doc),
            |doc| port_merge(StringInt(7), doc),
        );
        let texts = [
            r#""a""#,
            "5",
            "1.50",
            "15e-1",
            "-0",
            "1E2",
            "0.1",
            r#""\u00e9""#,
            "null",
            "true",
            "[1]",
            "{}",
            "1e400",
        ];
        let text_read = differing(&texts, go_read::<IntString>, |doc| {
            unmarshal::<Text>(doc.as_bytes())
                .ok()
                .map(|Text(value)| value)
        });
        let text_merged = differing(
            &texts,
            |doc| go_merge(IntString::default(), doc),
            |doc| port_merge(IntString::default(), doc),
        );
        // json.string-int-parse: `"+5"` reads 5 where Go fails, and `"-"` fails where Go reads 0;
        // json.out-of-range-numbers: jsoniter fails the document on `1e400`, which reads +Inf.
        let parsed = [r#""+5""#, r#""-""#];
        if read != parsed || merged != parsed || text_read != ["1e400"] || text_merged != ["1e400"]
        {
            return Err(format!(
                "StringInt read {read:?}, merged {merged:?}; IntString read {text_read:?}, \
                 merged {text_merged:?}"
            ));
        }
        let written = differing(
            &[0, 5, -5],
            |n| gojsonutil::marshal(&StringInt(n)).ok(),
            |n| marshal(&Int(StringInt(n))).ok(),
        );
        let text_written = differing(
            &["", "a", "\u{e9}", "<"],
            |text| gojsonutil::marshal(&IntString(text.into())).ok(),
            |text| marshal(&Text(IntString(text.into()))).ok(),
        );
        // json.html-escape: jsoniter writes `<` as `\u003c`, serde_json as it is.
        if !written.is_empty() || text_written != ["<"] {
            return Err(format!("written {written:?} {text_written:?}"));
        }
        Ok(())
    }

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
