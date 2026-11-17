//! `jsonutil.MergeClone`'s `json.RawMessage` decoder (`extMergeDecoder`).
//!
//! `MergeClone(v, data)` decodes `data` over an existing value with jsoniter, cloning
//! pointers, slices and maps instead of sharing them, and merging every `json.RawMessage`
//! field with `jsonpatch.MergePatch` instead of replacing it. Rust values are never shared, so
//! the cloning is invisible except for slices: Go's `sliceCloneDecoder` nils a slice before
//! decoding it, so its elements start from zero values instead of being decoded over.
//!
//! [`ext_merge_decode`] is the raw-message step, and [`merge_clone_raw`] is `MergeClone` into
//! a `json.RawMessage`. Decoding a struct this way needs the jsoniter decoders to dispatch to
//! them, which this crate does not do yet.

use crate::{
    GoKind, GoType, JsoniterDecode, RawJson,
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

/// A `json.RawMessage` decoded with [`ext_merge_decode`].
struct ExtMerge<'r>(&'r mut RawJson);

impl GoType for ExtMerge<'_> {
    const KIND: GoKind = GoKind::Slice;

    fn go_type_name(out: &mut String) {
        RawJson::go_type_name(out);
    }
}

impl JsoniterDecode for ExtMerge<'_> {
    fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
        ext_merge_decode(self.0, it);
    }
}

/// `jsonutil.MergeClone(&ext, data)` for a `json.RawMessage` target: [`ext_merge_decode`]
/// under jsoniter's `Unmarshal` (trailing bytes are an error).
///
/// # Errors
///
/// [`FailedToUnmarshal`] with the cleaned-up message, e.g. `cannot unmarshal :
/// invalid json on existing object`... exactly as Go words it.
pub fn merge_clone_raw(existing: &mut RawJson, data: &[u8]) -> Result<(), FailedToUnmarshal> {
    // Go: util/jsonutil/merge.go MergeClone
    jsoniter::unmarshal(data, &mut ExtMerge(existing)).map_err(|err| FailedToUnmarshal {
        message: GoString::from_bytes(try_extract_error_message(&err.go_text())),
    })
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
