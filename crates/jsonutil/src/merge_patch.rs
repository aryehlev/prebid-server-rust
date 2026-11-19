//! JSON merge patch where Prebid calls `jsonpatch.MergePatch` (gopkg.in/evanphx/json-patch.v5
//! v5.9.0): stored requests and imps, accounts, first-party data, `ext` merges and event URLs.
//!
//! The merge is RFC 7386 as the `json-patch` crate implements it. evanphx departs from the RFC in
//! a few places and writes each value with the spelling it was read with; each such case is a
//! ledgered difference (`json.merge-patch-*` in `differences.toml`).

use std::{
    error::Error,
    fmt::{self, Display},
};

use pbs_errortypes::GoError;
use serde_json::Value;

use crate::replace_lone_surrogates;

/// Why [`merge_patch`] failed (Go: `jsonpatch.ErrBadJSONDoc` and `jsonpatch.ErrBadJSONPatch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePatchError {
    /// The document is not JSON `serde_json` reads.
    BadDoc,
    /// The patch is not JSON `serde_json` reads.
    BadPatch,
}

impl Display for MergePatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadDoc => "Invalid JSON Document",
            Self::BadPatch => "Invalid JSON Patch",
        })
    }
}

impl Error for MergePatchError {}

/// Go: evanphx's `ErrBadJSONDoc` and `ErrBadJSONPatch` are `fmt.Errorf` values, so a caller that
/// returns or collects them passes on an error that is no `errortypes.Coder` (code 999), with its
/// text.
impl From<MergePatchError> for GoError {
    fn from(error: MergePatchError) -> Self {
        Self::generic(error.to_string())
    }
}

/// Go: `jsonpatch.MergePatch(doc, patch)`: `patch` merged into `doc` by RFC 7386, written as
/// compact JSON with object members sorted by key.
///
/// Lone surrogate escapes in either input are rewritten first ([`replace_lone_surrogates`]).
///
/// Where evanphx differs, this follows the RFC: a null document becomes the patch (Go: `Invalid
/// JSON Document`, whatever the patch), a null or scalar patch replaces the document (Go:
/// `Invalid JSON Patch`), and null members of objects inside a patch's arrays are kept (Go drops
/// them where the array is new to the document, replaces a value that is not an object or is the
/// patch itself). Numbers and strings are written as `serde_json` writes them, not as they were
/// spelled, objects with their members in key order, and a number beyond `f64`'s range or a
/// document nested deeper than 128 levels is rejected, which Go merges.
///
/// # Errors
///
/// [`MergePatchError::BadDoc`] or [`MergePatchError::BadPatch`] when that input is not JSON.
pub fn merge_patch(doc: &[u8], patch: &[u8]) -> Result<Vec<u8>, MergePatchError> {
    let Ok(mut target) = serde_json::from_slice::<Value>(&replace_lone_surrogates(doc)) else {
        return Err(MergePatchError::BadDoc);
    };
    let Ok(patch) = serde_json::from_slice::<Value>(&replace_lone_surrogates(patch)) else {
        return Err(MergePatchError::BadPatch);
    };
    json_patch::merge(&mut target, &patch);
    Ok(target.to_string().into_bytes())
}
