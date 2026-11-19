//! `jsonutil.MergeClone`: JSON merged into a value that may share memory with other values, with
//! Go's clone rules (`util/jsonutil/merge.go`).
//!
//! Go clones what a pointer, map or slice reaches before it writes there, so the merge never
//! changes a value it shares with another request. Rust values own their data, so most clones are
//! implicit; a pointer to a struct the value shares (`Option<Arc<T>>`, D-COW) is cloned through
//! `Arc::make_mut` where Go's `ptrCloneDecoder` clones it. The rules themselves are what
//! [`MergeClone`]'s implementations apply: null sets a pointer, slice or map to nil and leaves a
//! value field as it is (a string becomes empty); objects merge into structs member by member and
//! into maps entry by entry, each named entry replaced by a new value; arrays replace slices; and a
//! `json.RawMessage` merges by RFC 7386 merge patch. The generated types implement the trait with
//! [`merge_struct`] and [`merge_field`].

use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{self, Display},
    marker::PhantomData,
    sync::Arc,
};

use pbs_errortypes::GoError;
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, MapAccess, Visitor},
};
use serde_json::{Value, value::RawValue};

use crate::{
    IntString, MergePatchError, StringInt, merge_patch, replace_lone_surrogates,
    try_extract_error_message,
};

/// Why a merge failed: the innermost Go field it failed in, if any, and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCloneError {
    context: Option<String>,
    message: String,
}

impl MergeCloneError {
    /// An error with `message`, not yet inside a field.
    #[must_use]
    pub fn new<M: Into<String>>(message: M) -> Self {
        Self {
            context: None,
            message: message.into(),
        }
    }

    /// The error inside the field `go_field` of the Go type `go_type` (`openrtb2.Imp` and
    /// `Banner`), unless it is inside a field already: the innermost field is the one named.
    #[must_use]
    pub fn within(mut self, go_type: &str, go_field: &str) -> Self {
        if self.context.is_none() {
            self.context = Some(format!("{go_type}.{go_field}"));
        }
        self
    }

    /// The field the merge failed in, as `Type.Field`.
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// The message, without the field.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for MergeCloneError {
    /// The text Go's `tryExtractErrorMessage` builds: `cannot unmarshal <field>: <message>`, or
    /// the message alone outside every field.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.context {
            Some(context) => write!(f, "cannot unmarshal {context}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl Error for MergeCloneError {}

/// A type JSON can be merged into with Go's `MergeClone` rules.
pub trait MergeClone {
    /// Merges `patch`, one JSON value, into `self`.
    ///
    /// # Errors
    ///
    /// When `patch`, or a value inside it, is not of the kind the type reads. Go leaves what it
    /// merged before the error in place, and so does this; callers drop the value.
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError>;
}

/// Go: `jsonutil.MergeClone(v, data)`: merges the JSON `data` into `value`. Lone surrogate
/// escapes are rewritten first ([`replace_lone_surrogates`]). JSON null leaves `value` as it is.
///
/// # Errors
///
/// A `FailedToUnmarshal` when `data` is not one JSON value or the merge fails; its text is
/// `cannot unmarshal <Type.Field>: <message>` inside a field, as Go builds it, with
/// `serde_json`'s and this crate's messages where Go has jsoniter's (`json.error-texts`).
pub fn merge_clone<T: MergeClone + ?Sized>(value: &mut T, data: &[u8]) -> Result<(), GoError> {
    let data = replace_lone_surrogates(data);
    let patch: &RawValue = serde_json::from_slice(&data)
        .map_err(|e| GoError::failed_to_unmarshal(try_extract_error_message(&e.to_string())))?;
    value
        .merge_clone(patch)
        .map_err(|e| GoError::failed_to_unmarshal(e.to_string()))
}

/// Merges the JSON object `patch` into a struct, member by member.
///
/// `member(key, value)` merges each member into the field `key` names and ignores a key no field
/// has, in the order the members are written, a repeated key each time (each merge builds on the
/// one before). Null leaves the struct as it is (Go: a struct field given null).
///
/// Keys are matched as written; Go also matches a field whose name differs only in case
/// (`json.key-case`).
///
/// # Errors
///
/// When `patch` is neither an object nor null, or `member` fails.
pub fn merge_struct<F>(patch: &RawValue, mut member: F) -> Result<(), MergeCloneError>
where
    F: FnMut(&str, &RawValue) -> Result<(), MergeCloneError>,
{
    let Some(members) = members::<String>(patch)? else {
        return Ok(());
    };
    for (key, value) in members {
        member(&key, value)?;
    }
    Ok(())
}

/// Merges `value` into the field `go_field` of the Go type `go_type`, naming the field in the
/// error (see [`MergeCloneError::within`]).
///
/// # Errors
///
/// When the field's merge fails.
pub fn merge_field<T: MergeClone + ?Sized>(
    target: &mut T,
    value: &RawValue,
    go_type: &str,
    go_field: &str,
) -> Result<(), MergeCloneError> {
    target
        .merge_clone(value)
        .map_err(|e| e.within(go_type, go_field))
}

/// Whether `patch` is JSON null.
fn is_null(patch: &RawValue) -> bool {
    patch.get() == "null"
}

/// What kind of JSON value `patch` is, for messages.
fn kind(patch: &RawValue) -> &'static str {
    match patch.get().as_bytes().first() {
        Some(b'"') => "a string",
        Some(b'{') => "an object",
        Some(b'[') => "an array",
        Some(b't' | b'f') => "a boolean",
        Some(b'n') => "null",
        _ => "a number",
    }
}

/// `serde_json`'s error as a merge error.
fn library(error: &serde_json::Error) -> MergeCloneError {
    MergeCloneError::new(error.to_string())
}

/// The members of the JSON object `patch` in order, `None` for null.
fn members<'a, K: Deserialize<'a>>(
    patch: &'a RawValue,
) -> Result<Option<Vec<(K, &'a RawValue)>>, MergeCloneError> {
    match patch.get().as_bytes().first() {
        Some(b'n') => Ok(None),
        Some(b'{') => serde_json::from_str::<Members<'a, K>>(patch.get())
            .map(|members| Some(members.0))
            .map_err(|e| library(&e)),
        _ => Err(MergeCloneError::new(format!(
            "expected an object or null, found {}",
            kind(patch)
        ))),
    }
}

/// The elements of the JSON array `patch`, `None` for null.
fn elements(patch: &RawValue) -> Result<Option<Vec<&RawValue>>, MergeCloneError> {
    match patch.get().as_bytes().first() {
        Some(b'n') => Ok(None),
        Some(b'[') => serde_json::from_str(patch.get())
            .map(Some)
            .map_err(|e| library(&e)),
        _ => Err(MergeCloneError::new(format!(
            "expected an array or null, found {}",
            kind(patch)
        ))),
    }
}

/// An object's members as written, repeated keys included.
struct Members<'a, K>(Vec<(K, &'a RawValue)>);

impl<'de, K: Deserialize<'de>> Deserialize<'de> for Members<'de, K> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MembersVisitor(PhantomData))
    }
}

/// Reads [`Members`].
struct MembersVisitor<K>(PhantomData<K>);

impl<'de, K: Deserialize<'de>> Visitor<'de> for MembersVisitor<K> {
    type Value = Members<'de, K>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut members = Vec::new();
        while let Some(member) = map.next_entry::<K, &'de RawValue>()? {
            members.push(member);
        }
        Ok(Members(members))
    }
}

/// Go: a number or bool field: null leaves it as it is, any other value replaces it.
macro_rules! merge_scalar {
    ($($ty:ty),+) => {$(
        impl MergeClone for $ty {
            fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
                if !is_null(patch) {
                    *self = serde_json::from_str(patch.get()).map_err(|e| library(&e))?;
                }
                Ok(())
            }
        }
    )+};
}

merge_scalar!(bool, i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);

/// Go: a string field: null makes it empty (jsoniter's `ReadString`), any other value replaces it.
impl MergeClone for String {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        if is_null(patch) {
            self.clear();
        } else {
            *self = serde_json::from_str(patch.get()).map_err(|e| library(&e))?;
        }
        Ok(())
    }
}

/// Go: an `interface{}` field: the value replaces it, null included.
impl MergeClone for Value {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        *self = serde_json::from_str(patch.get()).map_err(|e| library(&e))?;
        Ok(())
    }
}

/// Go: a pointer (and a slice or map that may be nil): null sets it to nil; any other value is
/// merged into what it points to, or into a new zero value when it is nil.
impl<T: MergeClone + Default> MergeClone for Option<T> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        if is_null(patch) {
            *self = None;
            return Ok(());
        }
        self.get_or_insert_with(T::default).merge_clone(patch)
    }
}

/// Go: a `json.RawMessage` (`extMergeDecoder`): null keeps it, a nil one takes the value as
/// written, and any other is merge-patched with the value ([`merge_patch`]).
impl MergeClone for Option<Box<RawValue>> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        if is_null(patch) {
            return Ok(());
        }
        let merged = match self {
            None => patch.to_owned(),
            Some(existing) => {
                match merge_patch(existing.get().as_bytes(), patch.get().as_bytes()) {
                    Ok(bytes) => raw_value(bytes)?,
                    Err(MergePatchError::BadDoc) => {
                        return Err(MergeCloneError::new("invalid json on existing object"));
                    }
                    Err(error @ MergePatchError::BadPatch) => {
                        return Err(MergeCloneError::new(error.to_string()));
                    }
                }
            }
        };
        *self = Some(merged);
        Ok(())
    }
}

/// `merge_patch`'s output as a `RawValue`.
fn raw_value(bytes: Vec<u8>) -> Result<Box<RawValue>, MergeCloneError> {
    let text = String::from_utf8(bytes).map_err(|e| MergeCloneError::new(e.to_string()))?;
    RawValue::from_string(text).map_err(|e| library(&e))
}

/// Go: a slice is replaced, never merged: null empties it (Go: nil) and an array gives a new
/// slice whose elements are each merged into a new zero value.
impl<T: MergeClone + Default> MergeClone for Vec<T> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.clear();
        let Some(elements) = elements(patch)? else {
            return Ok(());
        };
        for element in elements {
            let mut value = T::default();
            value.merge_clone(element)?;
            self.push(value);
        }
        Ok(())
    }
}

/// Go: a map: null empties it (Go: nil); an object keeps the entries it does not name and
/// replaces each entry it names with its value merged into a new zero value.
impl<K: Ord + DeserializeOwned, V: MergeClone + Default> MergeClone for BTreeMap<K, V> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        let Some(members) = members::<K>(patch)? else {
            self.clear();
            return Ok(());
        };
        for (key, value) in members {
            let mut fresh = V::default();
            fresh.merge_clone(value)?;
            let _replaced = self.insert(key, fresh);
        }
        Ok(())
    }
}

/// Go: a pointer to a struct that other values may share (`ptrCloneDecoder`): what it points to
/// is cloned before the value is merged into it, so the values sharing it keep theirs.
/// `Arc::make_mut` clones only while the struct is shared, which no one else can tell from Go's
/// clone every time. `Option<Arc<T>>` adds Go's nil: null sets it to `None` and a value merges
/// into a new `T` when it is `None`.
impl<T: MergeClone + Clone> MergeClone for Arc<T> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        Self::make_mut(self).merge_clone(patch)
    }
}

impl<T: MergeClone + ?Sized> MergeClone for Box<T> {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        (**self).merge_clone(patch)
    }
}

/// Go: `MergeClone` hands the value, null included, to `(*StringInt).UnmarshalJSON`.
impl MergeClone for StringInt {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.set_from_json(patch.get())
            .map_err(|e| MergeCloneError::new(e.into_message()))
    }
}

/// Go: `MergeClone` hands the value, null included, to `(*IntString).UnmarshalJSON`.
impl MergeClone for IntString {
    fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
        self.set_from_json(patch.get())
            .map_err(|e| MergeCloneError::new(e.into_message()))
    }
}
