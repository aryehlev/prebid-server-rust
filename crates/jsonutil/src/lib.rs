//! Prebid Server's `util/jsonutil` on plain `serde_json`.
//!
//! The crate holds Prebid's own JSON logic and the serde helpers the generated types name; every
//! other crate calls `serde_json`, the serde derives, axum's `Json` and reqwest's `.json()`
//! directly, and no writer is mandatory.
//!
//! - [`unmarshal`], [`unmarshal_valid`] and [`marshal`]: `serde_json` inside Prebid's error
//!   wrappers (`FailedToUnmarshal` with [`try_extract_error_message`], `FailedToMarshal`).
//! - [`find_element`] and [`drop_element`]: `FindElement` and `DropElement`, byte for byte.
//! - [`merge_clone`] and the [`MergeClone`] trait the generated types implement with
//!   [`merge_struct`] and [`merge_field`]: Go's `MergeClone` rules.
//! - [`merge_patch`]: the RFC 7386 merge `json-patch` implements, where Go calls evanphx's
//!   `MergePatch`.
//! - [`StringInt`], [`IntString`] and [`parse_into_string`]: Prebid's lenient readers.
//! - The serde helpers: [`null_default`] and [`raw_keep_null`] for nulls, [`raw_compact`] for the
//!   writer of `json.RawMessage` (Prebid's `RawMessageExtension`), and [`is_zero`] and
//!   [`is_nil_or_empty`] for `omitempty`.
//! - [`replace_lone_surrogates`], which every decoder here runs on outside bytes first.
//!
//! Go's typed integers (`type X int8` and the like, the integer enums of `openrtb2` and
//! `openrtb_ext`) need no helper: typegen writes each as a `#[serde(transparent)]` newtype over
//! the integer of its width, never a Rust enum, so any integer of that width reads, as in Go, and
//! [`null_default`] on the field reads null as zero (the `typed_ints_accept_any_integer` test
//! pins it against Go).
//!
//! Where the port differs from Go, `differences.toml` names the difference, who sees it and the
//! test that pins it.

mod codec;
mod compact;
mod element;
mod merge;
mod merge_patch;
mod null;
mod omitempty;
mod readers;
mod surrogates;

pub use codec::{marshal, try_extract_error_message, unmarshal, unmarshal_valid};
pub use compact::{CompactRaw, compact, raw_compact};
pub use element::{ElementError, ElementSpan, drop_element, find_element};
pub use merge::{MergeClone, MergeCloneError, merge_clone, merge_field, merge_struct};
pub use merge_patch::{MergePatchError, merge_patch};
pub use null::{null_default, raw_keep_null};
pub use omitempty::{is_nil_or_empty, is_zero};
pub use readers::{IntString, StringInt, parse_into_string};
pub use surrogates::replace_lone_surrogates;
