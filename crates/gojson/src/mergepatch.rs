//! `gopkg.in/evanphx/json-patch.v5` v5.9.0: `MergePatch` (RFC 7386) and `Equal`.
//!
//! Prebid Server merges stored requests, stored imps, default requests, account defaults,
//! first-party data and hook results with `MergePatch`.
//!
//! The import path `gopkg.in/evanphx/json-patch.v5` resolves to the *root* package of the
//! `evanphx/json-patch` repository at tag v5.9.0, which is the map-based (v4-style) code, not the
//! order-preserving `v5/` subpackage. Its output therefore has this shape:
//!
//! * every object the merge had to look into is re-encoded the way `json.Marshal` encodes a
//!   `map[string]*lazyNode`: keys unescaped, deduplicated (the last one wins), sorted byte-wise and
//!   re-escaped (HTML-escaped as well);
//! * every other value keeps its input bytes, compacted with `json.Compact`'s HTML escaping (`<`,
//!   `>`, `&`, U+2028 and U+2029 become `<`, ... ), so its key order, duplicate keys and string
//!   escapes survive;
//! * a `null` in the patch deletes the key; `null`s nested in an added object are pruned, except
//!   inside a non-object value that replaces an object, which is kept as it is.
//!
//! ```
//! use pbs_gojson::mergepatch::{MergePatchError, merge_patch};
//!
//! # fn main() -> Result<(), MergePatchError> {
//! let out = merge_patch(
//!     br#"{"z": 1, "imp": {"tagid": "t", "id": "1"}, "a": {"y": 2, "b": 3}}"#,
//!     br#"{"imp": {"secure": 1, "tagid": null}}"#,
//! )?;
//! // The merged object is sorted; the untouched "a" keeps its key order.
//! assert_eq!(
//!     out,
//!     br#"{"a":{"y":2,"b":3},"imp":{"id":"1","secure":1},"z":1}"#
//! );
//!
//! assert_eq!(
//!     merge_patch(b"null", b"{}"),
//!     Err(MergePatchError::BadJsonDoc)
//! );
//! assert_eq!(
//!     merge_patch(b"{}", br#""x""#).map_err(|e| e.to_string()),
//!     Err("Invalid JSON Patch".to_owned()),
//! );
//! # Ok(())
//! # }
//! ```

use core::{error::Error, fmt, mem, str};
use std::{borrow::Cow, collections::BTreeMap};

use crate::{
    encode::std_string,
    stdjson::{append_compact, scanner::check_valid, unquote},
};

/// The errors [`merge_patch`] returns (Go's `ErrBadJSONDoc` and `ErrBadJSONPatch`).
///
/// Go code compares them with `errors.Is(err, jsonpatch.ErrBadJSONDoc)`; compare the variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergePatchError {
    /// `jsonpatch.ErrBadJSONDoc`: `Invalid JSON Document`. The document is malformed or `null`.
    BadJsonDoc,
    /// `jsonpatch.ErrBadJSONPatch`: `Invalid JSON Patch`. The patch is malformed, `null`, or a
    /// string, number or boolean.
    BadJsonPatch,
}

impl MergePatchError {
    /// The error's text, as Go's `err.Error()`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadJsonDoc => "Invalid JSON Document",
            Self::BadJsonPatch => "Invalid JSON Patch",
        }
    }
}

impl fmt::Display for MergePatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Error for MergePatchError {}

/// A decoded object: `partialDoc`, a `map[string]*lazyNode` (`None` is a JSON `null`).
type Doc<'a> = BTreeMap<Cow<'a, [u8]>, Option<Node<'a>>>;

/// Go's `lazyNode`: raw bytes until the merge needs to look inside.
#[derive(Debug)]
enum Node<'a> {
    /// `eRaw`: the exact bytes of a value that is never `null`.
    Raw(&'a [u8]),
    /// `eDoc`.
    Doc(Doc<'a>),
    /// `eAry` (`partialArray`).
    Ary(Vec<Option<Self>>),
}

/// What a whole input decodes to with `json.Unmarshal` into a `partialDoc`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Top {
    /// A `*json.SyntaxError`.
    Syntax,
    /// `null`: no error, but a nil map.
    Null,
    Object,
    Array,
    /// A string, number or boolean (an `*json.UnmarshalTypeError`).
    Scalar,
}

const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn skip_space(buf: &[u8], mut i: usize) -> usize {
    while buf.get(i).copied().is_some_and(is_space) {
        i += 1;
    }
    i
}

fn classify(data: &[u8]) -> Top {
    if check_valid(data).is_err() {
        return Top::Syntax;
    }
    match data.get(skip_space(data, 0)) {
        Some(b'{') => Top::Object,
        Some(b'[') => Top::Array,
        Some(b'n') => Top::Null,
        _ => Top::Scalar,
    }
}

/// The index just past the string starting at `i` (input already validated).
fn string_end(buf: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while let Some(&c) = buf.get(j) {
        match c {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    buf.len()
}

/// The index just past the value starting at `i` (input already validated).
fn value_end(buf: &[u8], i: usize) -> usize {
    match buf.get(i) {
        Some(b'"') => string_end(buf, i),
        Some(b'{' | b'[') => {
            let mut depth = 0_usize;
            let mut j = i;
            while let Some(&c) = buf.get(j) {
                match c {
                    b'"' => {
                        j = string_end(buf, j);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        if depth <= 1 {
                            return j + 1;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
                j += 1;
            }
            buf.len()
        }
        _ => {
            let mut j = i;
            while buf
                .get(j)
                .is_some_and(|&c| !matches!(c, b',' | b'}' | b']') && !is_space(c))
            {
                j += 1;
            }
            j
        }
    }
}

/// An object key as `encoding/json` decodes it into a `map[string]...` key.
fn key_of(quoted: &[u8]) -> Cow<'_, [u8]> {
    let inner = quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or_default();
    // Go: encoding/json/decode.go unquoteBytes (fast path: no escapes, valid UTF-8)
    if !inner.contains(&b'\\') && str::from_utf8(inner).is_ok() {
        return Cow::Borrowed(inner);
    }
    unquote(quoted).map_or(Cow::Borrowed(inner), Cow::Owned)
}

fn node_of(raw: &[u8]) -> Option<Node<'_>> {
    if raw == b"null" {
        None
    } else {
        Some(Node::Raw(raw))
    }
}

/// `json.Unmarshal(raw, &partialDoc)` of a valid object.
fn parse_doc(raw: &[u8]) -> Doc<'_> {
    let mut doc = Doc::new();
    let mut i = skip_space(raw, 0) + 1;
    loop {
        i = skip_space(raw, i);
        match raw.get(i) {
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(b'"') => {}
            _ => return doc,
        }
        let key_end = string_end(raw, i);
        let key = key_of(raw.get(i..key_end).unwrap_or_default());
        // Skip the colon.
        i = skip_space(raw, skip_space(raw, key_end) + 1);
        let end = value_end(raw, i);
        let _replaced = doc.insert(key, node_of(raw.get(i..end).unwrap_or_default()));
        i = end;
    }
}

/// `json.Unmarshal(raw, &partialArray)` of a valid array.
fn parse_ary(raw: &[u8]) -> Vec<Option<Node<'_>>> {
    let mut ary = Vec::new();
    let mut i = skip_space(raw, 0) + 1;
    loop {
        i = skip_space(raw, i);
        match raw.get(i) {
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(b']') | None => return ary,
            Some(_) => {}
        }
        let end = value_end(raw, i);
        ary.push(node_of(raw.get(i..end).unwrap_or_default()));
        i = end;
    }
}

fn first_byte(raw: &[u8]) -> Option<u8> {
    raw.get(skip_space(raw, 0)).copied()
}

/// `lazyNode.intoDoc`: whether the node is (now) an object.
fn into_doc(node: &mut Node<'_>) -> bool {
    match node {
        Node::Doc(_) => true,
        Node::Ary(_) => false,
        Node::Raw(raw) => {
            if first_byte(raw) != Some(b'{') {
                return false;
            }
            *node = Node::Doc(parse_doc(raw));
            true
        }
    }
}

/// `lazyNode.intoAry`: whether the node is (now) an array.
fn into_ary(node: &mut Node<'_>) -> bool {
    match node {
        Node::Ary(_) => true,
        Node::Doc(_) => false,
        Node::Raw(raw) => {
            if first_byte(raw) != Some(b'[') {
                return false;
            }
            *node = Node::Ary(parse_ary(raw));
            true
        }
    }
}

/// Go's `pruneNulls`.
fn prune_nulls(node: &mut Node<'_>) {
    if into_doc(node) {
        if let Node::Doc(doc) = node {
            prune_doc_nulls(doc);
        }
    } else if into_ary(node)
        && let Node::Ary(ary) = node
    {
        prune_ary_nulls(ary);
    }
}

/// Go's `pruneDocNulls`.
fn prune_doc_nulls(doc: &mut Doc<'_>) {
    doc.retain(|_, v| v.is_some());
    for node in doc.values_mut().flatten() {
        prune_nulls(node);
    }
}

/// Go's `pruneAryNulls`: `null` elements stay.
fn prune_ary_nulls(ary: &mut [Option<Node<'_>>]) {
    for node in ary.iter_mut().flatten() {
        prune_nulls(node);
    }
}

/// Go's `merge(cur, patch, false)`.
fn merge<'a>(mut cur: Node<'a>, mut patch: Node<'a>) -> Node<'a> {
    // Go: json-patch merge.go merge
    if !into_doc(&mut cur) {
        prune_nulls(&mut patch);
        return patch;
    }
    if !into_doc(&mut patch) {
        return patch;
    }
    if let (Node::Doc(cur_doc), Node::Doc(patch_doc)) = (&mut cur, patch) {
        merge_docs(cur_doc, patch_doc);
    }
    cur
}

/// Go's `mergeDocs(doc, patch, false)`.
fn merge_docs<'a>(doc: &mut Doc<'a>, patch: Doc<'a>) {
    // Go: json-patch merge.go mergeDocs
    for (key, value) in patch {
        match value {
            None => {
                let _removed = doc.remove(&key);
            }
            Some(mut value) => {
                if let Some(Some(cur)) = doc.get_mut(&key) {
                    let taken = mem::replace(cur, Node::Raw(&[]));
                    *cur = merge(taken, value);
                } else {
                    prune_nulls(&mut value);
                    let _replaced = doc.insert(key, Some(value));
                }
            }
        }
    }
}

/// `json.Marshal` of a `*lazyNode` (nil writes `null`).
fn write_opt(out: &mut Vec<u8>, node: Option<&Node<'_>>) {
    match node {
        None => out.extend_from_slice(b"null"),
        Some(node) => write_node(out, node),
    }
}

/// `lazyNode.MarshalJSON`, as `json.Marshal` compacts it.
fn write_node(out: &mut Vec<u8>, node: &Node<'_>) {
    match node {
        Node::Raw(raw) => {
            if append_compact(out, raw, true).is_err() {
                out.extend_from_slice(raw);
            }
        }
        Node::Doc(doc) => write_doc(out, doc),
        Node::Ary(ary) => write_ary(out, ary),
    }
}

/// `json.Marshal` of a `partialDoc`: sorted keys.
fn write_doc(out: &mut Vec<u8>, doc: &Doc<'_>) {
    out.push(b'{');
    for (i, (key, value)) in doc.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        std_string(out, key, true);
        out.push(b':');
        write_opt(out, value.as_ref());
    }
    out.push(b'}');
}

/// `json.Marshal` of a `partialArray`.
fn write_ary(out: &mut Vec<u8>, ary: &[Option<Node<'_>>]) {
    out.push(b'[');
    for (i, value) in ary.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        write_opt(out, value.as_ref());
    }
    out.push(b']');
}

/// `jsonpatch.MergePatch(docData, patchData)`: applies the RFC 7386 merge patch `patch` to
/// `doc`.
///
/// When both are objects the patch is merged into the document. A patch object over a
/// non-object document replaces it (with its `null`s pruned); a patch array replaces any
/// document (its `null` elements stay).
///
/// # Errors
///
/// [`MergePatchError::BadJsonDoc`] when `doc` is malformed (checked first) or `null`;
/// [`MergePatchError::BadJsonPatch`] when `patch` is malformed, `null`, or a string, number or
/// boolean.
pub fn merge_patch(doc: &[u8], patch: &[u8]) -> Result<Vec<u8>, MergePatchError> {
    // Go: json-patch merge.go doMergePatch(docData, patchData, false)
    let doc_top = classify(doc);
    if doc_top == Top::Syntax {
        return Err(MergePatchError::BadJsonDoc);
    }
    let patch_top = classify(patch);
    if patch_top == Top::Syntax {
        return Err(MergePatchError::BadJsonPatch);
    }
    if doc_top == Top::Null {
        return Err(MergePatchError::BadJsonDoc);
    }
    if patch_top == Top::Null {
        return Err(MergePatchError::BadJsonPatch);
    }
    let mut out = Vec::with_capacity(doc.len() + patch.len());
    match (doc_top, patch_top) {
        (Top::Object, Top::Object) => {
            let mut merged = parse_doc(doc);
            merge_docs(&mut merged, parse_doc(patch));
            write_doc(&mut out, &merged);
        }
        (_, Top::Object) => {
            let mut replaced = parse_doc(patch);
            prune_doc_nulls(&mut replaced);
            write_doc(&mut out, &replaced);
        }
        (_, Top::Array) => {
            let mut replaced = parse_ary(patch);
            prune_ary_nulls(&mut replaced);
            write_ary(&mut out, &replaced);
        }
        (_, Top::Scalar | Top::Null | Top::Syntax) => return Err(MergePatchError::BadJsonPatch),
    }
    Ok(out)
}

/// A node of [`equal`]: Go's `lazyNode`, whose `tryDoc`/`tryAry` re-decode the raw bytes
/// (`null` decodes into either, as an empty one).
enum Probe<'a> {
    Doc(BTreeMap<Cow<'a, [u8]>, Option<&'a [u8]>>),
    Ary(Vec<Option<&'a [u8]>>),
}

/// `lazyNode.tryDoc`: `json.Unmarshal(raw, &n.doc)` succeeds for an object or `null`.
fn try_doc(raw: &[u8]) -> Option<Probe<'_>> {
    match classify(raw) {
        Top::Object => {
            let doc = parse_doc(raw)
                .into_iter()
                .map(|(k, v)| {
                    let member = match v {
                        Some(Node::Raw(bytes)) => Some(bytes),
                        Some(Node::Doc(_) | Node::Ary(_)) | None => None,
                    };
                    (k, member)
                })
                .collect();
            Some(Probe::Doc(doc))
        }
        Top::Null => Some(Probe::Doc(BTreeMap::new())),
        Top::Syntax | Top::Array | Top::Scalar => None,
    }
}

/// `lazyNode.tryAry`.
fn try_ary(raw: &[u8]) -> Option<Probe<'_>> {
    match classify(raw) {
        Top::Array => {
            let ary = parse_ary(raw)
                .into_iter()
                .map(|v| match v {
                    Some(Node::Raw(bytes)) => Some(bytes),
                    Some(Node::Doc(_) | Node::Ary(_)) | None => None,
                })
                .collect();
            Some(Probe::Ary(ary))
        }
        Top::Null => Some(Probe::Ary(Vec::new())),
        Top::Syntax | Top::Object | Top::Scalar => None,
    }
}

/// `lazyNode.compact`: `json.Compact`, or the raw bytes when they are not valid JSON.
fn compact_or_raw(raw: &[u8]) -> Cow<'_, [u8]> {
    let mut out = Vec::with_capacity(raw.len());
    if append_compact(&mut out, raw, false).is_ok() {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(raw)
    }
}

/// `n.equal(o)` with `o` still raw (it always is when Go reaches it).
fn equal_raw(n: &[u8], o: &[u8]) -> bool {
    // Go: json-patch patch.go lazyNode.equal
    match try_doc(n).or_else(|| try_ary(n)) {
        None => compact_or_raw(n) == compact_or_raw(o),
        Some(Probe::Doc(n_doc)) => {
            let Some(Probe::Doc(o_doc)) = try_doc(o) else {
                return false;
            };
            n_doc.len() == o_doc.len()
                && n_doc
                    .iter()
                    .all(|(key, value)| match (value, o_doc.get(key)) {
                        (None, Some(None)) => true,
                        (Some(v), Some(Some(ov))) => equal_raw(v, ov),
                        (Some(_), Some(None)) | (None, Some(Some(_))) | (_, None) => false,
                    })
        }
        Some(Probe::Ary(n_ary)) => {
            let Some(Probe::Ary(o_ary)) = try_ary(o) else {
                return false;
            };
            // Go dereferences a nil element here and panics; such arrays compare unequal.
            n_ary.len() == o_ary.len()
                && n_ary.iter().zip(&o_ary).all(|pair| match pair {
                    (Some(v), Some(ov)) => equal_raw(v, ov),
                    (None, _) | (_, None) => false,
                })
        }
    }
}

/// `jsonpatch.Equal(a, b)`: whether two documents are equal as JSON, with Go's quirks.
///
/// Objects compare by unescaped keys (the last duplicate wins) and arrays element by element;
/// any other value compares by its `json.Compact` bytes, or by its raw bytes when it is not valid
/// JSON, so `1.0` and `1` differ. A top-level `null` equals any object, and equals an array when
/// the array comes first (`Equal("[]", "null")` is true, `Equal("null", "[]")` false).
///
/// Prebid calls it only from tests. Where Go panics (comparing arrays with a `null` element),
/// this returns false.
#[must_use]
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    // Go: json-patch patch.go Equal
    equal_raw(a, b)
}

#[cfg(test)]
mod tests {
    use super::{MergePatchError, equal, merge_patch};

    fn merged(doc: &str, patch: &str) -> Result<String, String> {
        merge_patch(doc.as_bytes(), patch.as_bytes())
            .map(|out| String::from_utf8_lossy(&out).into_owned())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn rfc7386_appendix_a() -> Result<(), String> {
        // Go: json-patch merge_test.go rfcTests (compared byte for byte with Go's output)
        let cases: [(&str, &str, &str); 13] = [
            (r#"{"a":"b"}"#, r#"{"a":"c"}"#, r#"{"a":"c"}"#),
            (r#"{"a":"b"}"#, r#"{"b":"c"}"#, r#"{"a":"b","b":"c"}"#),
            (r#"{"a":"b"}"#, r#"{"a":null}"#, "{}"),
            (r#"{"a":"b","b":"c"}"#, r#"{"a":null}"#, r#"{"b":"c"}"#),
            (r#"{"a":["b"]}"#, r#"{"a":"c"}"#, r#"{"a":"c"}"#),
            (r#"{"a":"c"}"#, r#"{"a":["b"]}"#, r#"{"a":["b"]}"#),
            (
                r#"{"a":{"b": "c"}}"#,
                r#"{"a": {"b": "d","c": null}}"#,
                r#"{"a":{"b":"d"}}"#,
            ),
            (r#"{"a":[{"b":"c"}]}"#, r#"{"a":[1]}"#, r#"{"a":[1]}"#),
            (r#"["a","b"]"#, r#"["c","d"]"#, r#"["c","d"]"#),
            (r#"{"a":"b"}"#, r#"["c"]"#, r#"["c"]"#),
            (r#"{"e":null}"#, r#"{"a":1}"#, r#"{"a":1,"e":null}"#),
            ("[1,2]", r#"{"a":"b","c":null}"#, r#"{"a":"b"}"#),
            ("{}", r#"{"a":{"bb":{"ccc":null}}}"#, r#"{"a":{"bb":{}}}"#),
        ];
        for (doc, patch, want) in cases {
            let got = merged(doc, patch)?;
            if got != want {
                return Err(format!("{doc} + {patch}: got {got}, want {want}"));
            }
        }
        Ok(())
    }

    #[test]
    fn errors() -> Result<(), String> {
        // Go: json-patch merge_test.go TestMergePatchFailRFCCases,
        // TestMergePatchReturnsErrorOnBadJSON
        let cases: [(&str, &str, MergePatchError); 6] = [
            (r#"{"a":"foo"}"#, "null", MergePatchError::BadJsonPatch),
            (r#"{"a":"foo"}"#, r#""bar""#, MergePatchError::BadJsonPatch),
            ("[[[[", "1", MergePatchError::BadJsonDoc),
            ("1", "[[[[", MergePatchError::BadJsonPatch),
            ("null", "{}", MergePatchError::BadJsonDoc),
            ("{", "{", MergePatchError::BadJsonDoc),
        ];
        for (doc, patch, want) in cases {
            let got = merge_patch(doc.as_bytes(), patch.as_bytes());
            if got != Err(want) {
                return Err(format!("{doc} + {patch}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn equal_quirks() -> Result<(), String> {
        let cases: [(&str, &str, bool); 7] = [
            (
                r#"{"a":1,"b":[1,{"c":2}]}"#,
                r#"{"b":[1,{"c":2}],"a":1}"#,
                true,
            ),
            ("null", "{}", true),
            ("[]", "null", true),
            ("null", "[]", false),
            ("1.0", "1", false),
            ("[null]", "[null]", false),
            ("x", "x", true),
        ];
        for (a, b, want) in cases {
            if equal(a.as_bytes(), b.as_bytes()) != want {
                return Err(format!("equal({a}, {b}) != {want}"));
            }
        }
        Ok(())
    }
}
