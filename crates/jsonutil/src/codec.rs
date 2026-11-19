//! `jsonutil.Unmarshal`, `UnmarshalValid` and `Marshal`: `serde_json` inside Prebid's error
//! wrappers, and the compaction of raw values Prebid's `RawMessageExtension` adds to `Marshal`.
//!
//! The wrappers are Prebid's and stay exact: a decode error is a `FailedToUnmarshal` whose text
//! went through [`try_extract_error_message`], an encode error a `FailedToMarshal`. The text
//! inside is the library's, `serde_json`'s here and jsoniter's in Go (`json.error-texts`).

use std::borrow::Cow;

use pbs_errortypes::GoError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{compact, replace_lone_surrogates};

/// The marker jsoniter puts after its message (Go: `tryExtractErrorMessage`).
const END_MARKER: &str = ", error found in #";

/// Go: `jsonutil.Unmarshal(data, &v)`: decodes `data` into a `T`.
///
/// Lone surrogate escapes are rewritten first ([`replace_lone_surrogates`]). Go's `Unmarshal`
/// skips validating the bytes of a `json.RawMessage` field and `UnmarshalValid` checks them;
/// `serde_json` always validates, so the two are the same function here
/// (`json.unvalidated-raw`).
///
/// # Errors
///
/// A `FailedToUnmarshal` holding [`try_extract_error_message`] of `serde_json`'s message when
/// `data` is not a `T`.
pub fn unmarshal<T: DeserializeOwned>(data: &[u8]) -> Result<T, GoError> {
    serde_json::from_slice(&replace_lone_surrogates(data))
        .map_err(|e| GoError::failed_to_unmarshal(try_extract_error_message(&e.to_string())))
}

/// Go: `jsonutil.UnmarshalValid(data, &v)`, the same as [`unmarshal`] (see there).
///
/// # Errors
///
/// As [`unmarshal`].
pub fn unmarshal_valid<T: DeserializeOwned>(data: &[u8]) -> Result<T, GoError> {
    unmarshal(data)
}

/// Go: `jsonutil.Marshal(v)`: `value` as compact JSON.
///
/// Every `json.RawMessage` in it is compacted as Prebid's `RawMessageExtension` writes it, as a
/// field, a map value, a slice element or the value itself, whether or not the field carries
/// [`raw_compact`](crate::raw_compact).
///
/// # Errors
///
/// A `FailedToMarshal` holding `serde_json`'s message when `value` cannot be written (a map
/// whose keys are not strings or integers, or a `Serialize` implementation that fails).
pub fn marshal<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, GoError> {
    write(value).map(String::into_bytes)
}

/// Go: `json.RawMessage(b)` of the bytes `b` that `jsonutil.Marshal(v)` returns, which Prebid
/// stores in a `json.RawMessage` field or map value: [`marshal`]'s bytes as a `RawValue`.
///
/// # Errors
///
/// As [`marshal`].
pub fn marshal_raw<T: Serialize + ?Sized>(value: &T) -> Result<Box<RawValue>, GoError> {
    RawValue::from_string(write(value)?).map_err(|e| GoError::failed_to_marshal(e.to_string()))
}

/// `value` as `serde_json` writes it, raw values compacted. `serde_json`'s compact writer puts no
/// whitespace between tokens, so all there is came from raw values, and compacting the whole text
/// compacts each of them.
fn write<T: Serialize + ?Sized>(value: &T) -> Result<String, GoError> {
    let text =
        serde_json::to_string(value).map_err(|e| GoError::failed_to_marshal(e.to_string()))?;
    if let Cow::Owned(compacted) = compact(&text) {
        return Ok(compacted);
    }
    Ok(text)
}

/// Go: `tryExtractErrorMessage(err)`: a jsoniter message without the context jsoniter appends.
///
/// The message is cut at jsoniter's `, error found in #` marker and prefixed with the innermost
/// operation (`cannot unmarshal openrtb2.Imp.Banner: ...`) when it does not start with
/// `request.`. A message without the marker, which every `serde_json` message is, comes back as
/// it is.
///
/// Go slices the message from the first `: ` to the marker and panics when the marker comes
/// first; that message comes back unchanged here.
#[must_use]
pub fn try_extract_error_message(msg: &str) -> String {
    let Some(end) = msg.rfind(END_MARKER) else {
        return msg.to_owned();
    };
    let Some(mut start) = msg.find(": ") else {
        return msg.to_owned();
    };
    let mut operations: Vec<&str> = msg.get(..start).into_iter().collect();
    while let Some(next) = msg.get(start + 2..).and_then(|rest| rest.find(": ")) {
        // Matches after the end marker do not count.
        if start + next >= end {
            break;
        }
        // An operation holds no space; one that does is part of the message.
        let Some(operation) = msg.get(start + 2..start + 2 + next) else {
            break;
        };
        if operation.contains(' ') {
            break;
        }
        operations.push(operation);
        start += next + 2;
    }
    let Some(detail) = msg.get(start + 2..end) else {
        return msg.to_owned();
    };
    let detailed = !msg
        .get(start + 2..)
        .is_some_and(|rest| rest.starts_with("request."));
    match operations
        .len()
        .checked_sub(2)
        .and_then(|i| operations.get(i))
    {
        Some(operation) if detailed => format!("cannot unmarshal {operation}: {detail}"),
        Some(_) | None => detail.to_owned(),
    }
}
