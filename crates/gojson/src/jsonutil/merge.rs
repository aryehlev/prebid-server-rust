//! `jsonutil.MergeClone` (`util/jsonutil/merge.go`): [`merge_clone`] for any decodable value,
//! [`merge_clone_raw`] for a `json.RawMessage` and [`ext_merge_decode`], the raw-message step
//! itself.

use crate::{
    JsoniterDecode, RawJson,
    gostring::GoString,
    jsoniter::{self, Iter},
    jsonutil::{FailedToUnmarshal, try_extract_error_message},
    mergepatch::{MergePatchError, merge_patch},
};

/// `extMergeDecoder.Decode`: merges the next value of `it` into `existing`.
///
/// `null` keeps `existing`; a value is taken as it is when `existing` is empty, and merged into
/// it with [`merge_patch`] otherwise. Nothing changes when reading the value fails, including
/// when it runs into the end of the input (a number at the very end), as in Go. A failed merge
/// records `merge: invalid json on existing object` (a malformed or `null` existing message)
/// or `merge: Invalid JSON Patch`.
pub fn ext_merge_decode(existing: &mut RawJson, it: &mut Iter<'_>) {
    // Go: util/jsonutil/merge.go extMergeDecoder.Decode
    if it.read_nil() {
        return;
    }
    let incoming = it.skip_and_return_bytes();
    // iter.Error != nil, io.EOF included.
    if it.has_any_error() {
        return;
    }
    if existing.is_empty() {
        *existing = RawJson::from_vec(incoming);
        return;
    }
    match merge_patch(existing.as_bytes(), &incoming) {
        Ok(merged) => *existing = RawJson::from_vec(merged),
        Err(MergePatchError::BadJsonDoc) => {
            it.report_error("merge", b"invalid json on existing object");
        }
        Err(err @ MergePatchError::BadJsonPatch) => {
            it.report_error("merge", err.as_str().as_bytes());
        }
    }
}

/// `jsonutil.MergeClone(v, data)`: decodes `data` over `v` with the merge-clone decoders.
///
/// Go decodes with a frozen jsoniter config that carries the `mergeCloneExtension`. Its
/// decorators clone pointers (`ptrCloneDecoder`), slices (`sliceCloneDecoder`) and maps
/// (`mapCloneDecoder`) instead of decoding into the shared originals, and its `extMergeDecoder`
/// merges every `json.RawMessage` with `jsonpatch.MergePatch` instead of replacing it. Rust
/// values are never shared, so the cloning shows only where it changes the decoded result:
///
/// * a slice is set to nil before it is decoded, so its elements start from zero values instead of
///   being decoded over the existing ones (`[{"tagid":"2"}]` over `[{"id":"1"}]` gives
///   `[{"tagid":"2"}]`), and a `null` slice is consumed before the slice decoder, so the error of a
///   malformed `null` is not prefixed with the slice type;
/// * a `json.RawMessage` keeps its bytes on `null`, takes the incoming value when it is empty, and
///   is merged with [`merge_patch`] otherwise; a malformed existing message fails with `invalid
///   json on existing object`;
/// * a pointer and a map are checked for `null` before their decoder runs, which checks again, as
///   the decorators do. That changes nothing until an error is recorded; after one, jsoniter's
///   `unreadByte` does nothing, so each check consumes a byte, and decoding goes on after an error
///   where Go's does (the decoded fields and the wrapped error text both depend on it).
///
/// Everything else decodes as [`jsonutil::unmarshal`] decodes it: a pointer is set to nil by
/// `null` and decoded over otherwise, and map entries are added to the existing map. The root
/// value is decoded in place (Go does not clone it), except that a root slice is reset too.
/// Trailing bytes are an error.
///
/// ```
/// use pbs_gojson::{GoJson, RawJson, jsonutil};
///
/// #[derive(Clone, Debug, Default, GoJson)]
/// #[gojson(go_type = "openrtb2.Imp")]
/// pub struct Imp {
///     #[gojson(go = "ID", json = "id")]
///     pub id: String,
///     #[gojson(json = "iframebuster,omitempty")]
///     pub iframe_buster: Option<Vec<String>>,
///     #[gojson(json = "ext,omitempty")]
///     pub ext: RawJson,
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut imp = Imp {
///     id: "1".to_owned(),
///     iframe_buster: Some(vec!["a".to_owned(), "b".to_owned()]),
///     ext: RawJson::from(r#"{"a":1,"b":2}"#),
/// };
/// jsonutil::merge_clone(
///     &mut imp,
///     br#"{"iframebuster":["c"],"ext":{"b":null,"c":3}}"#,
/// )?;
/// assert_eq!(
///     jsonutil::marshal(&imp)?,
///     br#"{"id":"1","iframebuster":["c"],"ext":{"a":1,"c":3}}"#
/// );
///
/// let mut bad = Imp {
///     ext: RawJson::from("malformed"),
///     ..Imp::default()
/// };
/// let err = jsonutil::merge_clone(&mut bad, br#"{"ext":{"a":1}}"#).err();
/// assert_eq!(
///     err.map(|e| e.to_string()).as_deref(),
///     Some("cannot unmarshal openrtb2.Imp.Ext: invalid json on existing object")
/// );
/// # Ok(())
/// # }
/// ```
///
/// [`jsonutil::unmarshal`]: crate::jsonutil::unmarshal
///
/// # Errors
///
/// [`FailedToUnmarshal`] with the cleaned-up jsoniter message, exactly as Go words it, e.g.
/// `cannot unmarshal openrtb2.Imp.IframeBuster: expect ull`. `v` keeps what was decoded
/// before the error.
pub fn merge_clone<T: JsoniterDecode + ?Sized>(
    v: &mut T,
    data: &[u8],
) -> Result<(), FailedToUnmarshal> {
    // Go: util/jsonutil/merge.go MergeClone
    jsoniter::unmarshal_merge_clone(data, v).map_err(|err| FailedToUnmarshal {
        message: GoString::from_bytes(try_extract_error_message(&err.go_text())),
    })
}

/// `jsonutil.MergeClone(&ext, data)` for a `json.RawMessage` target: [`ext_merge_decode`]
/// under jsoniter's `Unmarshal` (trailing bytes are an error); the same as [`merge_clone`].
///
/// # Errors
///
/// [`FailedToUnmarshal`] with the cleaned-up message, e.g. `invalid json on existing object`,
/// exactly as Go words it.
pub fn merge_clone_raw(existing: &mut RawJson, data: &[u8]) -> Result<(), FailedToUnmarshal> {
    merge_clone(existing, data)
}

#[cfg(test)]
mod tests {
    use super::merge_clone_raw;
    use crate::RawJson;

    /// A case: name, existing message (`None` is nil), incoming value and expected message.
    type ExtCase<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);

    #[test]
    fn merge_clone_ext() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeCloneExt (the ext merge itself; Go wraps the
        // incoming value as {"ext": ...} of an openrtb2.BidRequest, and an omitted ext never
        // reaches the decoder)
        let cases: [ExtCase<'_>; 9] = [
            (
                "both-populated",
                Some(r#"{"a":1,"b":2}"#),
                Some(r#"{"b":200,"c":3}"#),
                Some(r#"{"a":1,"b":200,"c":3}"#),
            ),
            ("both-nil", None, Some("null"), None),
            ("both-empty", None, Some("{}"), Some("{}")),
            (
                "ext-nil",
                Some(r#"{"b":2}"#),
                Some("null"),
                Some(r#"{"b":2}"#),
            ),
            (
                "ext-empty",
                Some(r#"{"b":2}"#),
                Some("{}"),
                Some(r#"{"b":2}"#),
            ),
            ("existing-nil", None, Some(r#"{"a":1}"#), Some(r#"{"a":1}"#)),
            (
                "existing-empty",
                Some("{}"),
                Some(r#"{"a":1}"#),
                Some(r#"{"a":1}"#),
            ),
            (
                "existing-omitted",
                None,
                Some(r#"{"b":2}"#),
                Some(r#"{"b":2}"#),
            ),
            (
                "ext-merged-sorted",
                Some(r#"{"z":1}"#),
                Some(r#"{"a":1}"#),
                Some(r#"{"a":1,"z":1}"#),
            ),
        ];
        for (name, existing, incoming, want) in cases {
            let mut ext = existing.map_or_else(RawJson::nil, RawJson::from);
            if let Some(incoming) = incoming {
                merge_clone_raw(&mut ext, incoming.as_bytes())
                    .map_err(|e| format!("{name}: {e}"))?;
            }
            let got = (!ext.is_nil()).then(|| String::from_utf8_lossy(ext.as_bytes()).into_owned());
            if got.as_deref() != want {
                return Err(format!("{name}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn merge_clone_ext_errors() -> Result<(), String> {
        let mut ext = RawJson::from("malformed");
        let err = merge_clone_raw(&mut ext, br#"{"a":1}"#)
            .err()
            .map(|e| e.to_string());
        // At the top level no type prefixes the operation (Go's TestMergeCloneExt, through
        // openrtb2.BidRequest.Ext, gets `cannot unmarshal openrtb2.BidRequest.Ext: ...`).
        let want = "invalid json on existing object";
        if err.as_deref() != Some(want) || ext.as_bytes() != b"malformed" {
            return Err(format!("existing-malformed: {err:?}"));
        }
        let mut bad = RawJson::from(r#"{"b":2}"#);
        let bad_err = merge_clone_raw(&mut bad, b"malformed").err();
        if bad_err.is_none() || bad.as_bytes() != br#"{"b":2}"# {
            return Err(format!("ext-malformed: {bad_err:?}"));
        }
        Ok(())
    }
}
