//! The serde code of every ported type against Go's shapes (`testdata/golden/shapes.jsonl`,
//! written by `testdata/go/shapes.go`, which documents the format).
//!
//! Each field's samples, zero value, null and integer extremes are decoded with
//! `pbs_jsonutil::unmarshal` and written with `marshal` (which must write the bytes `serde_json`
//! writes), against `jsonutil.Unmarshal` and `jsonutil.Marshal`, and decoded through
//! `EncodingJson`, against `json.Unmarshal` and `json.Marshal`; samples, null and values of the
//! wrong JSON type are merged with the types' `MergeClone`, against `jsonutil.MergeClone`.
//! Results compare token by token, strings by their text and numbers by value, and every
//! difference must fall in a ledger entry, which the comparison names.

use core::{fmt, iter::zip};
use std::collections::{BTreeMap, BTreeSet};

use pbs_gojson::gotype::go_type_name;
use pbs_jsonutil::{MergeClone, MergeCloneError, marshal, unmarshal};
use pbs_openrtb::gojson_bridge::EncodingJson;
use pbs_openrtb_ext::openrtb_ext;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, MapAccess, Visitor},
};
use serde_json::{Value, value::RawValue};

use super::{
    index::{TYPES, for_each_type},
    support::{TestResult, crate_dir, ensure_eq, read},
};

/// `json.error-texts`: the library message inside Prebid's wrapper is `serde_json`'s.
pub(super) const ERROR_TEXTS: &str = "json.error-texts";
/// `json.std-error-texts`: a document `EncodingJson::unmarshal` cannot decode fails with
/// `serde_json`'s message, where `json.Unmarshal`'s is `encoding/json`'s.
pub(super) const STD_ERROR_TEXTS: &str = "json.std-error-texts";
/// `json.element-merge-context`: a merge error in a slice element names the field, where Go names
/// the slice's type.
pub(super) const ELEMENT_CONTEXT: &str = "json.element-merge-context";
/// `json.failed-merge-elements`: after a merge that fails inside a slice or map, the field holds
/// what merged before the failure, where Go's holds the element or entry that failed as well.
pub(super) const FAILED_MERGE_ELEMENTS: &str = "json.failed-merge-elements";
/// `json.failed-merge-empty-slice`: a merge that fails on a nil slice leaves it empty, where Go
/// leaves it nil.
pub(super) const FAILED_MERGE_SLICE: &str = "json.failed-merge-empty-slice";
/// `json.html-escape`: jsoniter escapes `<`, `>`, `&`, U+2028 and U+2029 in strings.
pub(super) const HTML_ESCAPE: &str = "json.html-escape";
/// `json.float-format`: a number spelled differently.
pub(super) const FLOAT_FORMAT: &str = "json.float-format";
/// `json.nil-raw-written`: Go writes no value for a nil `json.RawMessage`, the port `null`.
pub(super) const NIL_RAW_WRITTEN: &str = "json.nil-raw-written";
/// `json.null-document`: Go reads the document `null`, serde rejects it.
pub(super) const NULL_DOCUMENT: &str = "json.null-document";
/// `json.promoted-field-context`: a merge error in a promoted field names the struct that
/// declares it, where Go names the bare field.
const PROMOTED_CONTEXT: &str = "json.promoted-field-context";
/// `json.duplicate-keys`: a repeated member that a typed partial struct reads fails the document,
/// where jsonparser takes the first.
pub(super) const DUPLICATE_KEYS: &str = "json.duplicate-keys";
/// `json.failed-merge-scalar`: a merge that fails on a string or number leaves its value, where
/// jsoniter leaves its zero value.
const FAILED_MERGE_SCALAR: &str = "json.failed-merge-scalar";
/// `json.prebid-message-context`: Prebid's message from an `UnmarshalJSON` method, which Go gives
/// bare, comes with `serde_json`'s position, or a merge's field, around it.
pub(super) const PREBID_MESSAGE: &str = "json.prebid-message-context";
/// `json.string-or-array-null`: `stringOrStringArray` given null is nil, where Go's is `[""]`.
pub(super) const STRING_OR_ARRAY_NULL: &str = "json.string-or-array-null";
/// `json.number-text`: a `json.Number` read from a string keeps text that is not a number, which
/// jsoniter writes as it is, where the port fails to write it.
pub(super) const NUMBER_TEXT: &str = "json.number-text";
/// `json.key-case`: jsoniter matches a member to a field whatever the case of its key.
pub(super) const KEY_CASE: &str = "json.key-case";
/// `json.null-elements`: null as a slice element that is not a pointer fails the document, where
/// Go reads the element's zero value.
pub(super) const NULL_ELEMENTS: &str = "json.null-elements";
/// `json.partial-results`: a patch that is not JSON fails before it merges, where jsoniter merges
/// what it reads before the fault (and hands an `UnmarshalJSON` method the bytes up to it).
pub(super) const PARTIAL_RESULTS: &str = "json.partial-results";
/// `json.struct-from-array`: the serde derive reads a struct from an array, where Go fails.
pub(super) const STRUCT_FROM_ARRAY: &str = "json.struct-from-array";

/// The Go types whose `UnmarshalJSON` methods decode with `jsonutil.Unmarshal` and fail with its
/// error: Go's names the field of that decode, the port's is `serde_json`'s message alone.
const INNER_DECODES: &[&str] = &[
    "openrtb_ext.ExtImpAppnexusKeywords",
    "openrtb_ext.PriceGranularity",
    "openrtb_ext.stringOrStringArray",
];

/// A JSON value as written: its members in document order, a repeated key kept, and each
/// scalar's text (a number beyond float64's range included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    /// An object's members.
    Object(Vec<(String, Self)>),
    /// An array's elements.
    Array(Vec<Self>),
    /// A string, number or literal, as written.
    Scalar(String),
}

/// An object's members in document order, each value as written.
struct Members(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The visitor of [`Members`].
        struct MembersVisitor;

        impl<'de> Visitor<'de> for MembersVisitor {
            type Value = Members;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Members, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }

        deserializer.deserialize_map(MembersVisitor)
    }
}

impl Node {
    /// `text` as a node, `None` when it is not JSON.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let raw: &RawValue = serde_json::from_str(text).ok()?;
        let json = raw.get();
        Some(match json.as_bytes().first()? {
            b'{' => {
                let Members(members) = serde_json::from_str(json).ok()?;
                Self::Object(
                    members
                        .into_iter()
                        .map(|(key, value)| Some((key, Self::parse(value.get())?)))
                        .collect::<Option<_>>()?,
                )
            }
            b'[' => {
                let elements: Vec<Box<RawValue>> = serde_json::from_str(json).ok()?;
                Self::Array(
                    elements
                        .iter()
                        .map(|element| Self::parse(element.get()))
                        .collect::<Option<_>>()?,
                )
            }
            _ => Self::Scalar(json.to_owned()),
        })
    }

    /// The node written as JSON: keys as `serde_json` writes them, scalars as they were read.
    pub(super) fn write(&self, out: &mut String) {
        match self {
            Self::Object(members) => {
                out.push('{');
                for (at, (key, value)) in members.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::from(key.as_str()).to_string());
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
            Self::Array(elements) => {
                out.push('[');
                for (at, element) in elements.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    element.write(out);
                }
                out.push(']');
            }
            Self::Scalar(text) => out.push_str(text),
        }
    }

    /// Adds the keys of every object in the node to `out`.
    pub(super) fn keys_into(&self, out: &mut BTreeSet<String>) {
        match self {
            Self::Object(members) => {
                for (key, value) in members {
                    let _ = out.insert(key.clone());
                    value.keys_into(out);
                }
            }
            Self::Array(elements) => elements.iter().for_each(|element| element.keys_into(out)),
            Self::Scalar(_) => {}
        }
    }
}

/// The keys of every object in Go's bytes, a nil `json.RawMessage` read as null; none when Go
/// wrote no JSON.
pub(super) fn go_keys(go: &Outcome) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if let Outcome::Encoded(bytes) = go
        && let Some(node) = Node::parse(&nil_raw_as_null(bytes))
    {
        node.keys_into(&mut keys);
    }
    keys
}

/// The document with every object key in lower case and the first of two members with one key
/// dropped, `None` when it is not JSON or nothing changes: what the port reads where jsoniter
/// matches keys whatever their case.
pub(super) fn lower_case_keys(doc: &str) -> Option<String> {
    let mode = Respelling {
        last_key: false,
        lower_unknown: true,
        repeats: Repeats::Last,
    };
    respelled_keys(doc, &BTreeSet::new(), mode).map(|respelled| respelled.doc)
}

/// Which of two members with one key [`respelled_keys`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Repeats {
    /// The last, whose value jsoniter and `encoding/json` set a field to.
    Last,
    /// The first, which jsonparser reads.
    First,
    /// Both, for a field Go decodes both into.
    Both,
}

/// How [`respelled_keys`] respells a document.
#[derive(Debug, Clone, Copy)]
pub(super) struct Respelling {
    /// Take the last of two keys a key matches whatever the case, not the first.
    last_key: bool,
    /// Write a key that matches none in lower case, not as it is.
    lower_unknown: bool,
    /// Which of two members with one key to keep.
    repeats: Repeats,
}

/// Every [`Respelling`], to try in turn.
pub(super) const RESPELLINGS: [Respelling; 6] = [
    Respelling {
        last_key: false,
        lower_unknown: false,
        repeats: Repeats::Last,
    },
    Respelling {
        last_key: true,
        lower_unknown: false,
        repeats: Repeats::Last,
    },
    Respelling {
        last_key: false,
        lower_unknown: false,
        repeats: Repeats::First,
    },
    Respelling {
        last_key: true,
        lower_unknown: false,
        repeats: Repeats::First,
    },
    Respelling {
        last_key: false,
        lower_unknown: false,
        repeats: Repeats::Both,
    },
    Respelling {
        last_key: true,
        lower_unknown: false,
        repeats: Repeats::Both,
    },
];

/// A document [`respelled_keys`] respelled.
pub(super) struct Respelled {
    /// The respelled document.
    pub(super) doc: String,
    /// The ledger entries of what changed: `json.key-case` where a key was respelled,
    /// `json.duplicate-keys` where a member was dropped.
    pub(super) classes: Vec<&'static str>,
}

/// The document with each object key that is not one of `keys` respelled as the key of `keys`
/// it matches whatever the case (or, as `mode` says, in lower case when it matches none), and of
/// two members that then have one key, the one `mode` keeps; `None` when it is not JSON or
/// nothing changes. It is what the port reads where jsoniter matches keys whatever their case.
pub(super) fn respelled_keys(
    doc: &str,
    keys: &BTreeSet<String>,
    mode: Respelling,
) -> Option<Respelled> {
    fn respell(
        node: Node,
        keys: &BTreeSet<String>,
        mode: Respelling,
        changes: &mut BTreeSet<&'static str>,
    ) -> Node {
        match node {
            Node::Object(members) => {
                let mut out: Vec<(String, Node)> = Vec::new();
                for (key, value) in members {
                    let value = respell(value, keys, mode, changes);
                    let key = if keys.contains(&key) {
                        key
                    } else {
                        let lower = key.to_lowercase();
                        let mut matching = keys.iter().filter(|k| k.to_lowercase() == lower);
                        let found = if mode.last_key {
                            matching.next_back()
                        } else {
                            matching.next()
                        };
                        let respelled = match found {
                            Some(found) => found.clone(),
                            None if mode.lower_unknown => lower,
                            None => key.clone(),
                        };
                        if respelled != key {
                            let _ = changes.insert(KEY_CASE);
                        }
                        respelled
                    };
                    let repeated = out.iter().any(|(kept, _)| *kept == key);
                    if repeated && mode.repeats != Repeats::Both {
                        let _ = changes.insert(DUPLICATE_KEYS);
                        if mode.repeats == Repeats::First {
                            continue;
                        }
                        out.retain(|(kept, _)| *kept != key);
                    }
                    out.push((key, value));
                }
                Node::Object(out)
            }
            Node::Array(elements) => Node::Array(
                elements
                    .into_iter()
                    .map(|element| respell(element, keys, mode, changes))
                    .collect(),
            ),
            Node::Scalar(text) => Node::Scalar(text),
        }
    }
    let mut changes = BTreeSet::new();
    let respelled = respell(Node::parse(doc)?, keys, mode, &mut changes);
    if changes.is_empty() {
        return None;
    }
    let mut written = String::new();
    respelled.write(&mut written);
    Some(Respelled {
        doc: written,
        classes: changes.into_iter().collect(),
    })
}

/// Whether the port's error text is `serde_json`'s for a null where a value is read.
fn is_null_rejected(port: &str) -> bool {
    port.starts_with("invalid type: null")
}

/// The start of each message Prebid's `UnmarshalJSON` methods write (Go:
/// `openrtb_ext/site.go:16`, `device.go:78` and `imp_goldbach.go:33`); the other methods pass on
/// the library's.
const PREBID_MESSAGES: &[&str] = &["request.", "value should be of type "];

/// Whether `text` is a message Prebid's `UnmarshalJSON` methods write.
pub(super) fn is_prebid_message(text: &str) -> bool {
    PREBID_MESSAGES.iter().any(|start| text.starts_with(start))
}

/// Whether the port's error text is Go's Prebid message `go`, with `serde_json`'s position after
/// it.
fn is_positioned(port: &str, go: &str) -> bool {
    port.strip_prefix(go)
        .and_then(|rest| rest.strip_prefix(" at line "))
        .and_then(|rest| rest.split_once(" column "))
        .is_some_and(|(line, column)| is_digits(line) && is_digits(column))
}

/// Whether `text` is a run of decimal digits.
fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Whether the port's error text is `serde_json`'s for a member a typed partial struct reads
/// twice.
fn is_duplicate_field(port: &str) -> bool {
    port.starts_with("duplicate field `")
}

/// The classes the goldens' differences fall in, each pinned here; any other difference fails.
const EXPECTED: &[&str] = &[
    DUPLICATE_KEYS,
    ERROR_TEXTS,
    FAILED_MERGE_ELEMENTS,
    FAILED_MERGE_SLICE,
    FAILED_MERGE_SCALAR,
    FLOAT_FORMAT,
    HTML_ESCAPE,
    KEY_CASE,
    NIL_RAW_WRITTEN,
    NULL_DOCUMENT,
    NULL_ELEMENTS,
    PARTIAL_RESULTS,
    PREBID_MESSAGE,
    PROMOTED_CONTEXT,
    STD_ERROR_TEXTS,
    STRING_OR_ARRAY_NULL,
    STRUCT_FROM_ARRAY,
];

/// What one side made of a document: the bytes it wrote, or the decoder's or encoder's error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    /// The encoded bytes.
    Encoded(String),
    /// The decoder's error text.
    DecodeFailed(String),
    /// The encoder's error text.
    EncodeFailed(String),
}

impl Outcome {
    /// A golden cell: `!` and the decoder's error, `?` and the encoder's, or the bytes.
    fn from_cell(cell: &str) -> Self {
        match cell.split_at_checked(1) {
            Some(("!", text)) => Self::DecodeFailed(text.to_owned()),
            Some(("?", text)) => Self::EncodeFailed(text.to_owned()),
            _ => Self::Encoded(cell.to_owned()),
        }
    }
}

/// A merge's error and the merged value written.
#[derive(Debug)]
pub(super) struct Merged {
    /// The merge's error.
    pub(super) error: Option<MergeCloneError>,
    /// The value after the merge, written.
    pub(super) encoded: Outcome,
}

/// `value` written with `marshal`, which must write what `serde_json` writes.
fn encoded<T: Serialize>(value: &T) -> TestResult<Outcome> {
    match marshal(value) {
        Ok(bytes) => {
            let direct = serde_json::to_vec(value).map_err(|e| e.to_string())?;
            if direct != bytes {
                return Err(format!(
                    "serde_json writes {} where marshal writes {}",
                    String::from_utf8_lossy(&direct),
                    String::from_utf8_lossy(&bytes)
                ));
            }
            String::from_utf8(bytes)
                .map(Outcome::Encoded)
                .map_err(|e| e.to_string())
        }
        Err(e) => Ok(Outcome::EncodeFailed(e.to_string())),
    }
}

/// `doc` decoded as `T` and written back.
fn decode<T: DeserializeOwned + Serialize>(doc: &[u8]) -> TestResult<Outcome> {
    match unmarshal::<T>(doc) {
        Ok(value) => encoded(&value),
        Err(e) => Ok(Outcome::DecodeFailed(e.to_string())),
    }
}

/// `doc` decoded as `T` the way `json.Unmarshal` decodes it, and written back.
fn decode_std<T: DeserializeOwned + Serialize>(doc: &[u8]) -> TestResult<Outcome> {
    match EncodingJson::<T>::unmarshal(doc) {
        Ok(value) => encoded(&value),
        Err(e) => Ok(Outcome::DecodeFailed(e.to_string())),
    }
}

/// The ledger entries of a difference from `json.Unmarshal`'s outcome: its error texts are
/// `json.std-error-texts`, not jsoniter's `json.error-texts`.
pub(super) fn std_classes(classes: Vec<&'static str>) -> Vec<&'static str> {
    classes
        .into_iter()
        .map(|class| {
            if class == ERROR_TEXTS {
                STD_ERROR_TEXTS
            } else {
                class
            }
        })
        .collect()
}

/// `patch` merged into `doc` decoded as `T`.
fn merge<T: DeserializeOwned + Serialize + MergeClone>(
    doc: &str,
    patch: &str,
) -> TestResult<Merged> {
    let mut value = unmarshal::<T>(doc.as_bytes())
        .map_err(|e| format!("the merge target {doc} does not decode: {e}"))?;
    // A patch that is not JSON fails before anything merges (`json.partial-results`).
    let error = match serde_json::from_str::<&RawValue>(patch) {
        Ok(patch) => value.merge_clone(patch).err(),
        Err(e) => Some(MergeCloneError::new(e.to_string())),
    };
    Ok(Merged {
        error,
        encoded: encoded(&value)?,
    })
}

/// A decode of a document that writes the value back.
pub(super) type Decode = fn(&[u8]) -> TestResult<Outcome>;

/// The serde code of one type.
pub(super) struct Runner {
    /// Decodes a document and writes it back.
    pub(super) decode: Decode,
    /// Decodes a document as `json.Unmarshal` does and writes it back.
    pub(super) decode_std: Decode,
    /// Merges a patch into a decoded document.
    pub(super) merge: fn(&str, &str) -> TestResult<Merged>,
}

/// The runner of `T`.
pub(super) fn runner<T: DeserializeOwned + Serialize + MergeClone>() -> Runner {
    Runner {
        decode: decode::<T>,
        decode_std: decode_std::<T>,
        merge: merge::<T>,
    }
}

/// The runner of every ported type by Go name; fails unless the names are those of [`TYPES`].
pub(super) fn runners() -> TestResult<BTreeMap<String, Runner>> {
    let mut by_name = BTreeMap::new();
    macro_rules! runner {
        ($ty:ty) => {{
            if by_name
                .insert(go_type_name::<$ty>(), runner::<$ty>())
                .is_some()
            {
                return Err(format!(
                    "two Rust types for Go type {}",
                    go_type_name::<$ty>()
                ));
            }
        }};
    }
    for_each_type!(runner);
    let names: BTreeSet<&str> = by_name.keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = TYPES.iter().copied().collect();
    ensure_eq(&names, &want, "the Go names of the ported types")?;
    Ok(by_name)
}

/// One case of the golden.
#[derive(Debug, Clone)]
pub(super) enum Case {
    /// A document decoded and written back by both Go decoders.
    Decode {
        /// The Go type.
        ty: String,
        /// The case's label.
        label: String,
        /// The document.
        doc: String,
        /// jsoniter's outcome.
        jsoniter: Outcome,
        /// `encoding/json`'s outcome.
        std: Outcome,
    },
    /// A patch merged into a decoded document.
    Merge {
        /// The Go type.
        ty: String,
        /// The case's label.
        label: String,
        /// The document of the merge target.
        doc: String,
        /// The patch.
        patch: String,
        /// `jsonutil.MergeClone`'s error text, empty for none.
        err: String,
        /// The merged value written.
        merged: Outcome,
    },
}

impl Case {
    /// The Go type and label.
    fn name(&self) -> String {
        match self {
            Self::Decode { ty, label, .. } | Self::Merge { ty, label, .. } => {
                format!("{ty} {label}")
            }
        }
    }
}

/// The golden's cases.
fn cases() -> TestResult<Vec<Case>> {
    cases_in("shapes.jsonl")
}

/// The cases of the golden `testdata/golden/<name>`, in the format of `shapes.jsonl`.
pub(super) fn cases_in(name: &str) -> TestResult<Vec<Case>> {
    let golden = crate_dir().join("testdata/golden").join(name);
    let text = String::from_utf8(read(&golden)?).map_err(|e| e.to_string())?;
    let mut lines = text.lines();
    let head: Vec<String> =
        serde_json::from_str(lines.next().unwrap_or_default()).map_err(|e| e.to_string())?;
    if head.first().map(String::as_str) != Some("#go") {
        return Err(format!("{}: no [\"#go\", version] line", golden.display()));
    }
    lines
        .enumerate()
        .map(|(n, line)| {
            let row: Vec<String> = serde_json::from_str(line).map_err(|e| e.to_string())?;
            match row.as_slice() {
                [kind, ty, label, doc, jsoniter, std] if kind == "D" => Ok(Case::Decode {
                    ty: ty.clone(),
                    label: label.clone(),
                    doc: doc.clone(),
                    jsoniter: Outcome::from_cell(jsoniter),
                    std: Outcome::from_cell(std),
                }),
                [kind, ty, label, doc, patch, err, merged] if kind == "M" => Ok(Case::Merge {
                    ty: ty.clone(),
                    label: label.clone(),
                    doc: doc.clone(),
                    patch: patch.clone(),
                    err: err.clone(),
                    merged: Outcome::from_cell(merged),
                }),
                _ => Err(format!("{name}:{}: bad row {line}", n + 2)),
            }
        })
        .collect()
}

/// A JSON token as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// `{`, `}`, `[`, `]`, `:` or `,`.
    Punct(u8),
    /// A string, quotes and escapes included.
    Str(&'a str),
    /// A number.
    Num(&'a str),
    /// `true`, `false` or `null`.
    Lit(&'a str),
}

/// The tokens of `text`, `None` when it is not a sequence of JSON tokens.
fn tokens(text: &str) -> Option<Vec<Token<'_>>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        let end = match b {
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                out.push(Token::Punct(b));
                at + 1
            }
            b' ' | b'\t' | b'\n' | b'\r' => at + 1,
            b'"' => {
                let mut end = at + 1;
                loop {
                    match bytes.get(end)? {
                        b'\\' => end += 2,
                        b'"' => break,
                        _ => end += 1,
                    }
                }
                out.push(Token::Str(text.get(at..=end)?));
                end + 1
            }
            b't' | b'f' | b'n' => {
                let lit = ["true", "false", "null"]
                    .into_iter()
                    .find(|lit| text.get(at..).is_some_and(|rest| rest.starts_with(lit)))?;
                out.push(Token::Lit(lit));
                at + lit.len()
            }
            _ => {
                let len = bytes
                    .get(at..)?
                    .iter()
                    .take_while(|c| b"+-0123456789.eE".contains(c))
                    .count();
                if len == 0 {
                    return None;
                }
                out.push(Token::Num(text.get(at..at + len)?));
                at + len
            }
        };
        at = end;
    }
    Some(out)
}

/// Whether two string tokens hold the same text.
fn same_string(a: &str, b: &str) -> bool {
    let text = |token: &str| serde_json::from_str::<String>(token).ok();
    text(a).is_some() && text(a) == text(b)
}

/// A number token as an exact decimal: whether it is negative, its significant digits and the
/// power of ten of the last one; zero as no digits, of either sign.
fn decimal(token: &str) -> Option<(bool, String, i64)> {
    let (negative, rest) = token
        .strip_prefix('-')
        .map_or((false, token), |rest| (true, rest));
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{int}{frac}");
    if int.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let significant = digits.trim_end_matches('0');
    let shift = i64::try_from(digits.len() - significant.len()).ok()?;
    let places = i64::try_from(frac.len()).ok()?;
    let significant = significant.trim_start_matches('0');
    if significant.is_empty() {
        return Some((false, String::new(), 0));
    }
    let power = exponent.checked_add(shift)?.checked_sub(places)?;
    Some((negative, significant.to_owned(), power))
}

/// Whether two number tokens hold the same value, compared as exact decimals: an integer beyond
/// float64's 53 bits that differs in its last digits differs.
fn same_number(a: &str, b: &str) -> bool {
    decimal(a).is_some() && decimal(a) == decimal(b)
}

#[test]
fn numbers_compare_as_exact_decimals() -> TestResult {
    let same = [
        ("1", "1.0"),
        ("1e+21", "1e21"),
        ("100000000000000000000", "1e20"),
        ("0", "-0.0"),
        ("0.5", "5e-1"),
        ("-12.5", "-1.25E1"),
    ];
    for (a, b) in same {
        ensure_eq(&same_number(a, b), &true, &format!("{a} and {b}"))?;
    }
    let different = [
        ("9223372036854775807", "9223372036854775806"),
        ("18446744073709551615", "1.8446744073709552e19"),
        ("1", "-1"),
        ("1.5", "15"),
        ("x", "x"),
    ];
    for (a, b) in different {
        ensure_eq(&same_number(a, b), &false, &format!("{a} and {b}"))?;
    }
    Ok(())
}

/// The ledger entries the difference between two token sequences falls in: strings with the
/// same text and numbers with the same value, spelled differently; `None` for any other
/// difference.
fn lexical(port: &[Token<'_>], go: &[Token<'_>]) -> Option<Vec<&'static str>> {
    if port.len() != go.len() {
        return None;
    }
    let mut classes = Vec::new();
    for (p, g) in zip(port, go) {
        let class = match (p, g) {
            _ if p == g => continue,
            (Token::Str(a), Token::Str(b)) if same_string(a, b) => HTML_ESCAPE,
            (Token::Num(a), Token::Num(b)) if same_number(a, b) => FLOAT_FORMAT,
            _ => return None,
        };
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    Some(classes)
}

/// Go's bytes with each nil `json.RawMessage` without `omitempty`, which Go writes as nothing at
/// all, written as null.
pub(super) fn nil_raw_as_null(go: &str) -> String {
    go.replace("\":,", "\":null,")
        .replace("\":}", "\":null}")
        .replace("\":]", "\":null]")
}

/// The ledger entries a difference between the port's bytes and Go's falls in (none for the
/// same bytes), or what is unexplained.
pub(super) fn compare_encoded(port: &str, go: &str) -> Result<Vec<&'static str>, String> {
    if port == go {
        return Ok(Vec::new());
    }
    let unexplained = || format!("port {port}, Go {go}");
    // A raw value, which takes a number beyond float64's range that a `json.RawMessage` holds.
    let is_json = |text: &str| serde_json::from_str::<&RawValue>(text).is_ok();
    if !is_json(go) {
        let fixed = nil_raw_as_null(go);
        if !is_json(&fixed) {
            return Err(format!("{} (not JSON)", unexplained()));
        }
        let mut classes = vec![NIL_RAW_WRITTEN];
        classes.extend(compare_encoded(port, &fixed)?);
        return Ok(classes);
    }
    let (Some(port_tokens), Some(go_tokens)) = (tokens(port), tokens(go)) else {
        return Err(format!("{}: not JSON tokens", unexplained()));
    };
    lexical(&port_tokens, &go_tokens).ok_or_else(unexplained)
}

/// The ledger entries a difference between two outcomes falls in, or what is unexplained.
pub(super) fn compare(
    doc: &str,
    port: &Outcome,
    go: &Outcome,
) -> Result<Vec<&'static str>, String> {
    match (port, go) {
        (Outcome::Encoded(p), Outcome::Encoded(g))
            if doc.trim() == "null" && p == "null" && g == "[\"\"]" =>
        {
            Ok(vec![STRING_OR_ARRAY_NULL])
        }
        (Outcome::Encoded(p), Outcome::Encoded(g))
            if doc.contains("null") && g.contains("[\"\"]") && compare_encoded(p, g).is_err() =>
        {
            let mut classes = compare_encoded(p, &g.replace("[\"\"]", "null"))?;
            classes.push(STRING_OR_ARRAY_NULL);
            Ok(classes)
        }
        (Outcome::Encoded(p), Outcome::Encoded(g)) => compare_encoded(p, g),
        // Go read the first (jsonparser) or last (jsoniter) of the repeated members and failed
        // on it.
        (Outcome::DecodeFailed(p), Outcome::DecodeFailed(_)) if is_duplicate_field(p) => {
            Ok(vec![DUPLICATE_KEYS])
        }
        (Outcome::DecodeFailed(p), Outcome::Encoded(g))
            if (p.starts_with("invalid number") || p.starts_with("expected value"))
                && serde_json::from_str::<&RawValue>(&nil_raw_as_null(g)).is_err() =>
        {
            Ok(vec![NUMBER_TEXT])
        }
        (Outcome::DecodeFailed(p), Outcome::DecodeFailed(g)) if is_prebid_message(g) => {
            if p == g {
                Ok(Vec::new())
            } else if is_positioned(p, g) {
                Ok(vec![PREBID_MESSAGE])
            } else {
                Err(format!("port {p:?}, Go's Prebid message {g:?}"))
            }
        }
        (Outcome::DecodeFailed(p), Outcome::DecodeFailed(g))
        | (Outcome::EncodeFailed(p), Outcome::EncodeFailed(g)) => Ok(if p == g {
            Vec::new()
        } else {
            vec![ERROR_TEXTS]
        }),
        (Outcome::DecodeFailed(_), Outcome::Encoded(_)) if doc.trim() == "null" => {
            Ok(vec![NULL_DOCUMENT])
        }
        (Outcome::DecodeFailed(p), Outcome::Encoded(_)) if is_duplicate_field(p) => {
            Ok(vec![DUPLICATE_KEYS])
        }
        (Outcome::Encoded(_), Outcome::DecodeFailed(g))
            if g.contains("expect { or n, but found [")
                || g.contains("cannot unmarshal array into Go")
                || (g.contains("expect object or null") && doc.trim_start().starts_with('[')) =>
        {
            Ok(vec![STRUCT_FROM_ARRAY])
        }
        (Outcome::EncodeFailed(_), Outcome::Encoded(g))
            if serde_json::from_str::<&RawValue>(g).is_err() =>
        {
            Ok(vec![NUMBER_TEXT])
        }
        (Outcome::DecodeFailed(p), Outcome::Encoded(_))
            if (is_null_rejected(p) || is_prebid_message(p)) && doc.contains("null") =>
        {
            Ok(vec![NULL_ELEMENTS])
        }
        _ => Err(format!("port {port:?}, Go {go:?}")),
    }
}

/// Go's merge error text split into its field (`openrtb2.Imp.Banner`) and its message.
pub(super) fn go_merge_error(text: &str) -> (Option<&str>, &str) {
    text.strip_prefix("cannot unmarshal ")
        .and_then(|rest| rest.split_once(": "))
        .map_or((None, text), |(context, message)| (Some(context), message))
}

/// The ledger entries a merge case falls in, or what is unexplained.
fn compare_merge(
    ty: &str,
    merged: &Merged,
    patch: &str,
    err: &str,
    go_merged: &Outcome,
) -> Result<Vec<&'static str>, String> {
    let mut classes = Vec::new();
    if serde_json::from_str::<&RawValue>(patch).is_err() {
        return if merged.error.is_some() && !err.is_empty() {
            Ok(vec![PARTIAL_RESULTS])
        } else {
            Err(format!(
                "patch {patch} is not JSON: merge error {:?}, Go's {err:?}",
                merged.error
            ))
        };
    }
    match (&merged.error, err.is_empty()) {
        (None, true) => {}
        (Some(error), true) if patch.trim() == "null" && is_null_rejected(error.message()) => {
            return Ok(vec![NULL_DOCUMENT]);
        }
        (Some(error), true)
            if (is_null_rejected(error.message()) || is_prebid_message(error.message()))
                && patch.contains("null") =>
        {
            return Ok(vec![NULL_ELEMENTS]);
        }
        (None, false) if patch.trim_start().starts_with('[') => {
            return Ok(vec![STRUCT_FROM_ARRAY]);
        }
        // A struct inside the patch given an array, which jsoniter rejects and the derive reads
        // as the struct's fields (`[]` as its defaults): the merge goes on, and the values part
        // in the members the patch names (and lexically elsewhere).
        (None, false) if go_merge_error(err).1 == JSONITER_STRUCT_FROM_ARRAY => {
            let lexical =
                outside_patched_members(&merged.encoded, patch, go_merged).ok_or_else(|| {
                    format!(
                        "merged {:?}, Go's {go_merged:?} after {err:?}",
                        merged.encoded
                    )
                })?;
            return Ok([vec![STRUCT_FROM_ARRAY], lexical].concat());
        }
        (Some(error), false) if error.context().is_none() && INNER_DECODES.contains(&ty) => {
            classes.push(ERROR_TEXTS);
        }
        // Go merged the first of the repeated members; the merged values part.
        (Some(error), true) if is_duplicate_field(error.message()) => {
            return Ok(vec![DUPLICATE_KEYS]);
        }
        (Some(error), false) => {
            let (go_context, message) = go_merge_error(err);
            match (error.context(), go_context) {
                (ours, theirs) if ours == theirs => {}
                (Some(_), None) if is_prebid_message(message) => classes.push(PREBID_MESSAGE),
                (Some(ours), Some(theirs)) if is_promoted_context(ours, theirs) => {
                    classes.push(PROMOTED_CONTEXT);
                }
                (Some(ours), Some(theirs)) if theirs.starts_with("[]") && ours.contains('.') => {
                    classes.push(ELEMENT_CONTEXT);
                }
                (ours, theirs) => {
                    return Err(format!("merge error in {ours:?}, Go's in {theirs:?}"));
                }
            }
            if error.message() != message {
                classes.push(ERROR_TEXTS);
            }
        }
        (error, _) => return Err(format!("merge error {error:?}, Go's {err:?}")),
    }
    match compare("", &merged.encoded, go_merged) {
        Ok(found) => classes.extend(found),
        Err(e) => {
            let emptied = emptied_slices_as_nil(merged)
                .map(|nil| compare("", &nil, go_merged))
                .and_then(Result::ok);
            if let Some(found) = emptied {
                classes.push(FAILED_MERGE_SLICE);
                classes.extend(found);
            } else if differs_only_in_patched_fields(merged, patch, go_merged) {
                classes.push(FAILED_MERGE_ELEMENTS);
            } else if merged.error.is_some() && is_scalar_zero(go_merged) {
                classes.push(FAILED_MERGE_SCALAR);
            } else {
                return Err(e);
            }
        }
    }
    Ok(classes)
}

/// jsoniter's message for a struct read from a JSON array (its `readObjectStart`), as
/// `tryExtractErrorMessage` leaves it.
const JSONITER_STRUCT_FROM_ARRAY: &str = "expect { or n, but found [";

/// The ledger entries of the lexical differences between the port's merged object and Go's
/// outside the members the patch names, `None` when the values differ there or are not objects.
fn outside_patched_members(port: &Outcome, patch: &str, go: &Outcome) -> Option<Vec<&'static str>> {
    let (Outcome::Encoded(port), Outcome::Encoded(go)) = (port, go) else {
        return None;
    };
    let parse = |text: &str| serde_json::from_str::<serde_json::Map<String, Value>>(text).ok();
    let (patch, mut port, mut go) = (parse(patch)?, parse(port)?, parse(go)?);
    for key in patch.keys() {
        let _port = port.remove(key);
        let _go = go.remove(key);
    }
    let write = |rest: &serde_json::Map<String, Value>| serde_json::to_string(rest).ok();
    compare_encoded(&write(&port)?, &write(&go)?).ok()
}

/// Whether, after a merge that failed, the port's value and Go's differ only in the members the
/// patch names: the fields the failure was in.
fn differs_only_in_patched_fields(merged: &Merged, patch: &str, go: &Outcome) -> bool {
    let (Some(_), Outcome::Encoded(port), Outcome::Encoded(go)) =
        (&merged.error, &merged.encoded, go)
    else {
        return false;
    };
    let parse = |text: &str| serde_json::from_str::<serde_json::Map<String, Value>>(text).ok();
    let (Some(patch), Some(mut port), Some(mut go)) = (parse(patch), parse(port), parse(go)) else {
        return false;
    };
    for key in patch.keys() {
        let _port = port.remove(key);
        let _go = go.remove(key);
    }
    port == go
}

/// Whether Go's merged value is the zero value of a string or number, which jsoniter leaves in
/// a named string or number type whose merge fails.
fn is_scalar_zero(go: &Outcome) -> bool {
    matches!(go, Outcome::Encoded(text) if text == "\"\"" || text == "0")
}

/// After a merge that failed, the merged value with its empty slices written as Go writes nil
/// ones, `None` when the merge succeeded or has no empty slices.
pub(super) fn emptied_slices_as_nil(merged: &Merged) -> Option<Outcome> {
    match &merged.encoded {
        Outcome::Encoded(text) if merged.error.is_some() && text.contains(":[]") => {
            Some(Outcome::Encoded(text.replace(":[]", ":null")))
        }
        Outcome::Encoded(_) | Outcome::DecodeFailed(_) | Outcome::EncodeFailed(_) => None,
    }
}

/// Whether the port's merge error names `Type.Field` where Go's names the bare field, as jsoniter
/// does for a field promoted from an embedded struct.
fn is_promoted_context(port: &str, go: &str) -> bool {
    !go.contains('.') && port.strip_suffix(go).is_some_and(|ty| ty.ends_with('.'))
}

/// The ledger entries one case falls in, or what is unexplained.
fn check_case(
    runners: &BTreeMap<String, Runner>,
    case: &Case,
) -> Result<Vec<&'static str>, String> {
    match case {
        Case::Decode {
            ty,
            doc,
            jsoniter,
            std,
            ..
        } => {
            let runner = runners
                .get(ty)
                .ok_or_else(|| format!("no Rust type for {ty}"))?;
            // The comparison of one decoder's outcome, retried with the keys in lower case.
            let side = |decode: Decode, go: &Outcome, name: &str| match (
                compare(doc, &decode(doc.as_bytes())?, go),
                lower_case_keys(doc),
            ) {
                (Ok(classes), _) => Ok(classes),
                (Err(e), Some(lowered)) => {
                    let mut classes = compare(&lowered, &decode(lowered.as_bytes())?, go).map_err(
                        |in_lower_case| {
                            format!("{name}: {e} (with the keys in lower case: {in_lower_case})")
                        },
                    )?;
                    classes.push(KEY_CASE);
                    Ok(classes)
                }
                (Err(e), None) => Err(format!("{name}: {e}")),
            };
            let mut classes = side(runner.decode, jsoniter, "jsoniter")?;
            classes.extend(std_classes(side(runner.decode_std, std, "encoding/json")?));
            Ok(classes)
        }
        Case::Merge {
            ty,
            doc,
            patch,
            err,
            merged,
            ..
        } => {
            let runner = runners
                .get(ty)
                .ok_or_else(|| format!("no Rust type for {ty}"))?;
            let found = compare_merge(ty, &(runner.merge)(doc, patch)?, patch, err, merged);
            match (found, lower_case_keys(patch)) {
                (Ok(classes), _) => Ok(classes),
                (Err(e), Some(lowered)) => {
                    let mut classes =
                        compare_merge(ty, &(runner.merge)(doc, &lowered)?, &lowered, err, merged)
                            .map_err(|in_lower_case| {
                            format!("{e} (with the keys in lower case: {in_lower_case})")
                        })?;
                    classes.push(KEY_CASE);
                    Ok(classes)
                }
                (Err(e), None) => Err(e),
            }
        }
    }
}

/// Every case: the ledger entries the differences fall in, with the number of cases of each,
/// and what is unexplained.
pub(super) fn run(
    runners: &BTreeMap<String, Runner>,
    cases: &[Case],
) -> (BTreeMap<&'static str, usize>, Vec<String>) {
    let mut classes = BTreeMap::new();
    let mut unexplained = Vec::new();
    for case in cases {
        match check_case(runners, case) {
            Ok(found) => {
                for class in found {
                    *classes.entry(class).or_insert(0) += 1;
                }
            }
            Err(e) => unexplained.push(format!("{}: {e}", case.name())),
        }
    }
    (classes, unexplained)
}

/// Fails, listing the first of them, when some cases differ from Go outside the ledger.
pub(super) fn ensure_explained(unexplained: &[String]) -> TestResult {
    if unexplained.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} cases differ from Go outside the ledger:\n{}",
        unexplained.len(),
        unexplained
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Every case of the golden matches Go, or differs in the ledger entries of [`EXPECTED`], each of
/// which some case shows.
#[test]
fn shapes_match_go() -> TestResult {
    let cases = cases()?;
    let types: BTreeSet<&str> = cases
        .iter()
        .map(|case| match case {
            Case::Decode { ty, .. } | Case::Merge { ty, .. } => ty.as_str(),
        })
        .collect();
    ensure_eq(
        &types,
        &TYPES.iter().copied().collect(),
        "the types the golden has cases for",
    )?;
    let (classes, unexplained) = run(&runners()?, &cases);
    ensure_explained(&unexplained)?;
    ensure_eq(
        &classes.keys().copied().collect::<Vec<_>>(),
        &EXPECTED.to_vec(),
        &format!("the ledger entries the differences fall in ({classes:?})"),
    )
}

/// The decode case of `ty` labelled `label`: its document and jsoniter's and `encoding/json`'s
/// outcomes.
fn decode_case<'a>(
    cases: &'a [Case],
    ty: &str,
    label: &str,
) -> TestResult<(&'a str, &'a Outcome, &'a Outcome)> {
    cases
        .iter()
        .find_map(|case| match case {
            Case::Decode {
                ty: t,
                label: l,
                doc,
                jsoniter,
                std,
            } if t == ty && l == label => Some((doc.as_str(), jsoniter, std)),
            Case::Decode { .. } | Case::Merge { .. } => None,
        })
        .ok_or_else(|| format!("no decode case {ty} {label}"))
}

/// The merge case of `ty` labelled `label`: the merge target's document, the patch,
/// `jsonutil.MergeClone`'s error text and the merged value.
fn merge_case<'a>(
    cases: &'a [Case],
    ty: &str,
    label: &str,
) -> TestResult<(&'a str, &'a str, &'a str, &'a Outcome)> {
    cases
        .iter()
        .find_map(|case| match case {
            Case::Merge {
                ty: t,
                label: l,
                doc,
                patch,
                err,
                merged,
            } if t == ty && l == label => {
                Some((doc.as_str(), patch.as_str(), err.as_str(), merged))
            }
            Case::Decode { .. } | Case::Merge { .. } => None,
        })
        .ok_or_else(|| format!("no merge case {ty} {label}"))
}

/// The runner of the Go type `ty`.
fn runner_of<'a>(runners: &'a BTreeMap<String, Runner>, ty: &str) -> TestResult<&'a Runner> {
    runners
        .get(ty)
        .ok_or_else(|| format!("no Rust type for {ty}"))
}

/// `json.failed-merge-scalar`: a merge of a value of the wrong type into a named string or
/// number fails on both sides; jsoniter leaves the type's zero value, the port the value it had.
#[test]
fn failed_scalar_merges_keep_the_value() -> TestResult {
    let runners = runners()?;
    let mut found = Vec::new();
    for case in cases()? {
        let Case::Merge {
            ty,
            label,
            doc,
            patch,
            err,
            merged,
        } = &case
        else {
            continue;
        };
        let runner = runner_of(&runners, ty)?;
        let port = (runner.merge)(doc, patch)?;
        if port.error.is_none()
            || err.is_empty()
            || !is_scalar_zero(merged)
            || compare("", &port.encoded, merged).is_ok()
        {
            continue;
        }
        ensure_eq(
            &port.encoded,
            &(runner.decode)(doc.as_bytes())?,
            &format!("{ty} {label}: the port's value after the failed merge"),
        )?;
        found.push(format!("{ty} {label}"));
    }
    // Each named string and integer type's four patches of another JSON type.
    let want: Vec<String> = [
        "openrtb_ext.BidType",
        "openrtb_ext.TargetingKey",
        "openrtb_ext.BidderName",
        "openrtb_ext.IOSAppTrackingStatus",
        "openrtb_ext.AuctionEnvironmentType",
        "openrtb_ext.CookieStatus",
        "openrtb_ext.UserSyncType",
    ]
    .iter()
    .flat_map(|ty| (0..4).map(move |n| format!("{ty} wrong {n}")))
    .collect();
    ensure_eq(&found, &want, "the failed merges into scalars")
}

/// `json.prebid-message-context`: Prebid's message from an `UnmarshalJSON` method is Go's text
/// where the method reads the whole document, and comes with `serde_json`'s position where it
/// reads a member; a merge's error names the field it failed in, where Go's is the message alone.
/// The entry is restored under Rule B (d) by pbs-jsonutil glue that has not landed yet; with it,
/// these texts become Go's and this test turns into the restore pin.
#[test]
fn prebid_messages_carry_position_and_field() -> TestResult {
    const WIDTH: &str =
        "request.device.ext.prebid.interstitial.minwidthperc must be a number between 0 and 100";
    let runners = runners()?;
    let cases = cases()?;
    let go = Outcome::DecodeFailed(WIDTH.to_owned());
    let (top, top_jsoniter, top_std) = decode_case(&cases, "openrtb_ext.ExtDeviceInt", "edge 0")?;
    ensure_eq(
        &(top_jsoniter, top_std),
        &(&go, &go),
        "Go's errors of ExtDeviceInt edge 0",
    )?;
    let device_int = runner_of(&runners, "openrtb_ext.ExtDeviceInt")?;
    ensure_eq(
        &(device_int.decode)(top.as_bytes())?,
        &go,
        "the port's error at the top",
    )?;
    let (doc, jsoniter, std) =
        decode_case(&cases, "openrtb_ext.ExtDevicePrebid", "interstitial edge 0")?;
    ensure_eq(
        &(jsoniter, std),
        &(&go, &go),
        "Go's errors of ExtDevicePrebid",
    )?;
    let device_prebid = runner_of(&runners, "openrtb_ext.ExtDevicePrebid")?;
    let positioned = Outcome::DecodeFailed(format!("{WIDTH} at line 1 column 55"));
    for decode in [device_prebid.decode, device_prebid.decode_std] {
        ensure_eq(
            &decode(doc.as_bytes())?,
            &positioned,
            "the port's error inside",
        )?;
    }
    let (target, patch, go_err, go_merged) =
        merge_case(&cases, "openrtb_ext.ExtDevicePrebid", "interstitial edge 0")?;
    ensure_eq(&go_err, &WIDTH, "jsonutil.MergeClone's error")?;
    let merged = (device_prebid.merge)(target, patch)?;
    ensure_eq(
        &merged.encoded,
        go_merged,
        "the value the failed merge leaves",
    )?;
    ensure_eq(
        &merged.error.map(|e| e.to_string()),
        &Some(format!(
            "cannot unmarshal openrtb_ext.ExtDevicePrebid.Interstitial: {WIDTH}"
        )),
        "the port's merge error",
    )
}

/// `json.string-or-array-null`: `stringOrStringArray` given null reads `[""]` in Go (its
/// `UnmarshalJSON` reads null as an empty string first), nil in the port (`unmarshal` rejects
/// null as a string, `json.null-document`, and reads it as an absent array).
#[test]
fn string_or_array_null_is_nil() -> TestResult {
    let cases = cases()?;
    let (doc, jsoniter, std) = decode_case(&cases, "openrtb_ext.stringOrStringArray", "null")?;
    let go = Outcome::Encoded(r#"[""]"#.to_owned());
    ensure_eq(&(jsoniter, std), &(&go, &go), "Go's values")?;
    let runners = runners()?;
    let runner = runner_of(&runners, "openrtb_ext.stringOrStringArray")?;
    let nil = Outcome::Encoded("null".to_owned());
    ensure_eq(
        &(runner.decode)(doc.as_bytes())?,
        &nil,
        "jsonutil.Unmarshal's",
    )?;
    ensure_eq(
        &(runner.decode_std)(doc.as_bytes())?,
        &nil,
        "json.Unmarshal's",
    )
}
