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
//!   `>`, `&`, U+2028 and U+2029 become `\u003c`, ... ), so its key order, duplicate keys and
//!   string escapes survive;
//! * the merge runs without recursion, so it accepts documents nested as deep as Go does
//!   (`encoding/json`'s limit of 10000 levels) on any thread stack;
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

use core::{error::Error, fmt, mem, slice, str};
use std::{
    borrow::Cow,
    collections::{BTreeMap, btree_map},
};

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

/// A decoded object: `partialDoc`, a `map[string]*lazyNode` whose values are indices into the
/// [`Arena`] (`None` is a JSON `null`).
type Doc<'a> = BTreeMap<Cow<'a, [u8]>, Option<usize>>;

/// A decoded array: `partialArray`.
type Ary = Vec<Option<usize>>;

/// Go's `lazyNode`: raw bytes until the merge needs to look inside.
#[derive(Debug)]
enum Node<'a> {
    /// `eRaw`: the exact bytes of a value that is never `null`.
    Raw(&'a [u8]),
    /// `eDoc`.
    Doc(Doc<'a>),
    /// `eAry`.
    Ary(Ary),
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
    classify_valid(data)
}

/// [`classify`] for a value already known to be valid JSON (a member or element of a valid
/// document: every part of a valid document is itself valid, the nesting limit included).
fn classify_valid(data: &[u8]) -> Top {
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

/// The members of a valid object, in input order: each key as `encoding/json` decodes it and
/// the exact bytes of its value.
struct Members<'a> {
    raw: &'a [u8],
    i: usize,
}

impl<'a> Members<'a> {
    fn new(raw: &'a [u8]) -> Self {
        Self {
            raw,
            i: skip_space(raw, 0) + 1,
        }
    }
}

impl<'a> Iterator for Members<'a> {
    type Item = (Cow<'a, [u8]>, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let raw = self.raw;
        loop {
            self.i = skip_space(raw, self.i);
            match raw.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'"') => break,
                _ => return None,
            }
        }
        let key_end = string_end(raw, self.i);
        let key = key_of(raw.get(self.i..key_end).unwrap_or_default());
        // Skip the colon.
        let start = skip_space(raw, skip_space(raw, key_end) + 1);
        let end = value_end(raw, start);
        self.i = end;
        Some((key, raw.get(start..end).unwrap_or_default()))
    }
}

/// The elements of a valid array, in order, as their exact bytes.
struct Elements<'a> {
    raw: &'a [u8],
    i: usize,
}

impl<'a> Elements<'a> {
    fn new(raw: &'a [u8]) -> Self {
        Self {
            raw,
            i: skip_space(raw, 0) + 1,
        }
    }
}

impl<'a> Iterator for Elements<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let raw = self.raw;
        loop {
            self.i = skip_space(raw, self.i);
            match raw.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') | None => return None,
                Some(_) => break,
            }
        }
        let start = self.i;
        self.i = value_end(raw, start);
        Some(raw.get(start..self.i).unwrap_or_default())
    }
}

fn first_byte(raw: &[u8]) -> Option<u8> {
    raw.get(skip_space(raw, 0)).copied()
}

/// Every `lazyNode` of one merge. Nodes refer to each other by index, so that neither the
/// merge nor dropping its result recurses: Go's goroutine stacks grow, and it merges documents
/// nested as deep as `encoding/json` accepts (10000 levels), which a recursive port could not do
/// on a small thread stack.
struct Arena<'a> {
    nodes: Vec<Node<'a>>,
}

impl<'a> Arena<'a> {
    fn new() -> Self {
        Self {
            nodes: Vec::with_capacity(16),
        }
    }

    fn push(&mut self, node: Node<'a>) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// A `*lazyNode` as `json.Unmarshal` stores it: nil for `null`.
    fn value(&mut self, raw: &'a [u8]) -> Option<usize> {
        (raw != b"null").then(|| self.push(Node::Raw(raw)))
    }

    /// `lazyNode.intoDoc`: whether the node is (now) an object.
    fn load_doc(&mut self, i: usize) -> bool {
        let raw = match self.nodes.get(i) {
            Some(Node::Doc(_)) => return true,
            Some(Node::Ary(_)) | None => return false,
            Some(&Node::Raw(raw)) => raw,
        };
        if first_byte(raw) != Some(b'{') {
            return false;
        }
        // json.Unmarshal into a map: the last of duplicate keys wins.
        let mut doc = Doc::new();
        for (key, value) in Members::new(raw) {
            let value = self.value(value);
            let _replaced = doc.insert(key, value);
        }
        if let Some(node) = self.nodes.get_mut(i) {
            *node = Node::Doc(doc);
        }
        true
    }

    /// `lazyNode.intoAry`: whether the node is (now) an array.
    fn load_ary(&mut self, i: usize) -> bool {
        let raw = match self.nodes.get(i) {
            Some(Node::Ary(_)) => return true,
            Some(Node::Doc(_)) | None => return false,
            Some(&Node::Raw(raw)) => raw,
        };
        if first_byte(raw) != Some(b'[') {
            return false;
        }
        let ary = Elements::new(raw).map(|value| self.value(value)).collect();
        if let Some(node) = self.nodes.get_mut(i) {
            *node = Node::Ary(ary);
        }
        true
    }

    /// Go's `pruneNulls`, which recurses through `pruneDocNulls` (objects lose their `null`
    /// members) and `pruneAryNulls` (`null` elements stay); the order does not matter.
    fn prune_nulls(&mut self, root: usize) {
        let mut pending = vec![root];
        while let Some(i) = pending.pop() {
            if self.load_doc(i) {
                if let Some(Node::Doc(doc)) = self.nodes.get_mut(i) {
                    doc.retain(|_, v| v.is_some());
                    pending.extend(doc.values().flatten());
                }
            } else if self.load_ary(i)
                && let Some(Node::Ary(ary)) = self.nodes.get(i)
            {
                pending.extend(ary.iter().flatten());
            }
        }
    }

    /// Go's `mergeDocs(doc, patch, false)` for two object nodes, with its recursion through
    /// `merge` turned into a list of the object pairs left to merge.
    fn merge_docs(&mut self, root_doc: usize, root_patch: usize) {
        // Go: json-patch merge.go mergeDocs, merge
        let mut pending = vec![(root_doc, root_patch)];
        while let Some((doc, patch)) = pending.pop() {
            let patch_doc = match self.nodes.get_mut(patch) {
                Some(node @ Node::Doc(_)) => match mem::replace(node, Node::Ary(Vec::new())) {
                    Node::Doc(patch_doc) => patch_doc,
                    Node::Raw(_) | Node::Ary(_) => continue,
                },
                Some(Node::Raw(_) | Node::Ary(_)) | None => continue,
            };
            for (key, value) in patch_doc {
                let Some(value) = value else {
                    if let Some(Node::Doc(members)) = self.nodes.get_mut(doc) {
                        let _removed = members.remove(&key);
                    }
                    continue;
                };
                let cur = match self.nodes.get(doc) {
                    Some(Node::Doc(cur)) => cur.get(&key).copied().flatten(),
                    Some(Node::Raw(_) | Node::Ary(_)) | None => None,
                };
                let merged = match cur {
                    // merge(cur, patch): a patch over a non-object replaces it, pruned...
                    Some(cur) if !self.load_doc(cur) => {
                        self.prune_nulls(value);
                        value
                    }
                    // ...a non-object patch replaces an object as it is...
                    Some(_) if !self.load_doc(value) => value,
                    // ...and two objects merge.
                    Some(cur) => {
                        pending.push((cur, value));
                        cur
                    }
                    // A new key (or one that is null): the value, pruned.
                    None => {
                        self.prune_nulls(value);
                        value
                    }
                };
                if let Some(Node::Doc(members)) = self.nodes.get_mut(doc) {
                    let _replaced = members.insert(key, Some(merged));
                }
            }
        }
    }

    /// `json.Marshal` of a node: objects with sorted keys, raw values compacted with HTML
    /// escaping. Iterative, with a stack of the open objects and arrays.
    fn write(&self, out: &mut Vec<u8>, root: usize) {
        enum Open<'n, 'a> {
            Doc(btree_map::Iter<'n, Cow<'a, [u8]>, Option<usize>>, bool),
            Ary(slice::Iter<'n, Option<usize>>, bool),
        }
        let mut open: Vec<Open<'_, 'a>> = Vec::new();
        let mut next = Some(Some(root));
        loop {
            match next.take() {
                Some(None) => out.extend_from_slice(b"null"),
                Some(Some(i)) => match self.nodes.get(i) {
                    Some(Node::Raw(raw)) => {
                        // lazyNode.MarshalJSON of an eRaw node, compacted by json.Marshal.
                        let len = out.len();
                        if append_compact(out, raw, true).is_err() {
                            out.truncate(len);
                            out.extend_from_slice(raw);
                        }
                    }
                    Some(Node::Doc(doc)) => {
                        out.push(b'{');
                        open.push(Open::Doc(doc.iter(), true));
                    }
                    Some(Node::Ary(ary)) => {
                        out.push(b'[');
                        open.push(Open::Ary(ary.iter(), true));
                    }
                    None => {}
                },
                None => {}
            }
            let Some(top) = open.last_mut() else {
                return;
            };
            match top {
                Open::Doc(members, first) => {
                    if let Some((key, value)) = members.next() {
                        if !mem::take(first) {
                            out.push(b',');
                        }
                        std_string(out, key, true);
                        out.push(b':');
                        next = Some(*value);
                    } else {
                        out.push(b'}');
                        let _closed = open.pop();
                    }
                }
                Open::Ary(elements, first) => {
                    if let Some(value) = elements.next() {
                        if !mem::take(first) {
                            out.push(b',');
                        }
                        next = Some(*value);
                    } else {
                        out.push(b']');
                        let _closed = open.pop();
                    }
                }
            }
        }
    }
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
    let mut arena = Arena::new();
    let root = match (doc_top, patch_top) {
        (Top::Object, Top::Object) => {
            let merged = arena.push(Node::Raw(doc));
            let patch = arena.push(Node::Raw(patch));
            // json.Unmarshal of both into a partialDoc, then mergeDocs.
            if arena.load_doc(merged) && arena.load_doc(patch) {
                arena.merge_docs(merged, patch);
            }
            merged
        }
        // Not a document: the result is the patch, pruned (pruneDocNulls or pruneAryNulls).
        (_, Top::Object | Top::Array) => {
            let replaced = arena.push(Node::Raw(patch));
            arena.prune_nulls(replaced);
            replaced
        }
        (_, Top::Scalar | Top::Null | Top::Syntax) => return Err(MergePatchError::BadJsonPatch),
    };
    let mut out = Vec::with_capacity(doc.len() + patch.len());
    arena.write(&mut out, root);
    Ok(out)
}

/// A node of [`equal`]: Go's `lazyNode`, whose `tryDoc`/`tryAry` re-decode the raw bytes
/// (`null` decodes into either, as an empty one).
enum Probe<'a> {
    Doc(BTreeMap<Cow<'a, [u8]>, Option<&'a [u8]>>),
    Ary(Vec<Option<&'a [u8]>>),
}

/// A member or element as a `*lazyNode`: nil for `null`.
fn probe_value(raw: &[u8]) -> Option<&[u8]> {
    (raw != b"null").then_some(raw)
}

/// `lazyNode.tryDoc`: `json.Unmarshal(raw, &n.doc)` succeeds for an object or `null`. `top` is
/// what `raw` decodes to.
fn try_doc(raw: &[u8], top: Top) -> Option<Probe<'_>> {
    match top {
        Top::Object => {
            let mut doc = BTreeMap::new();
            for (key, value) in Members::new(raw) {
                let _replaced = doc.insert(key, probe_value(value));
            }
            Some(Probe::Doc(doc))
        }
        Top::Null => Some(Probe::Doc(BTreeMap::new())),
        Top::Syntax | Top::Array | Top::Scalar => None,
    }
}

/// `lazyNode.tryAry`.
fn try_ary(raw: &[u8], top: Top) -> Option<Probe<'_>> {
    match top {
        Top::Array => Some(Probe::Ary(Elements::new(raw).map(probe_value).collect())),
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
///
/// Go recurses into members and elements; the pairs still to compare are kept on a heap work
/// list here instead, so that documents as deep as `encoding/json` accepts compare on a small
/// stack. Comparing has no side effects, so the order does not change the result.
///
/// Go's `tryDoc` and `tryAry` validate the bytes again at every level, which makes a deep
/// comparison quadratic; only the two documents are validated here, since their members and
/// elements are then valid too, so every probe decodes as Go's does.
fn equal_raw(a: &[u8], b: &[u8]) -> bool {
    // Go: json-patch patch.go lazyNode.equal
    let mut pending = vec![(a, b)];
    let mut validated = false;
    while let Some((n, o)) = pending.pop() {
        let classify_one = if validated { classify_valid } else { classify };
        validated = true;
        let (n_top, o_top) = (classify_one(n), classify_one(o));
        match try_doc(n, n_top).or_else(|| try_ary(n, n_top)) {
            None => {
                if compact_or_raw(n) != compact_or_raw(o) {
                    return false;
                }
            }
            Some(Probe::Doc(n_doc)) => {
                let Some(Probe::Doc(o_doc)) = try_doc(o, o_top) else {
                    return false;
                };
                if n_doc.len() != o_doc.len() {
                    return false;
                }
                for (key, value) in &n_doc {
                    match (value, o_doc.get(key)) {
                        (None, Some(None)) => {}
                        (Some(v), Some(Some(ov))) => pending.push((v, ov)),
                        (Some(_), Some(None)) | (None, Some(Some(_))) | (_, None) => return false,
                    }
                }
            }
            Some(Probe::Ary(n_ary)) => {
                let Some(Probe::Ary(o_ary)) = try_ary(o, o_top) else {
                    return false;
                };
                if n_ary.len() != o_ary.len() {
                    return false;
                }
                // Go dereferences a nil element here and panics; such arrays compare unequal.
                for pair in n_ary.iter().zip(&o_ary) {
                    match pair {
                        (Some(v), Some(ov)) => pending.push((v, ov)),
                        (None, _) | (_, None) => return false,
                    }
                }
            }
        }
    }
    true
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
    use std::thread;

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

    /// `{"b":1,"x":` + `{"a":` (depth - 1 times) + `1` and the closing braces: an object nested
    /// `depth` levels deep, with its keys sorted.
    fn deep_object(depth: usize) -> Vec<u8> {
        let mut out = br#"{"b":1,"x":"#.to_vec();
        for _ in 1..depth {
            out.extend_from_slice(br#"{"a":"#);
        }
        out.push(b'1');
        out.resize(out.len() + depth, b'}');
        out
    }

    fn deep_merges() -> Result<(), String> {
        // Go merges every document encoding/json accepts (nesting up to 10000); the arena
        // merge needs no stack per level. Object merged into object, at every level:
        let doc = deep_object(9_999);
        if merge_patch(&doc, &doc).map_err(|e| e.to_string())? != doc {
            return Err("deep object merge".to_owned());
        }
        // A deep object added to a document (pruned, re-encoded), and a deep array:
        if merge_patch(br#"{"b":1}"#, &doc).map_err(|e| e.to_string())? != doc {
            return Err("deep object insert".to_owned());
        }
        let mut arr = br#"{"x":"#.to_vec();
        arr.resize(arr.len() + 9_998, b'[');
        arr.resize(arr.len() + 9_998, b']');
        arr.push(b'}');
        for base in [&br#"{"x":1}"#[..], br#"{"x":{"y":null}}"#] {
            // Pruned over a non-object, kept as it is over an object.
            if merge_patch(base, &arr).map_err(|e| e.to_string())? != arr {
                return Err("deep array insert".to_owned());
            }
        }
        // One level more than encoding/json allows is a syntax error.
        let too_deep = deep_object(10_001);
        if merge_patch(b"{}", &too_deep) != Err(MergePatchError::BadJsonPatch) {
            return Err("10001 levels".to_owned());
        }
        Ok(())
    }

    #[test]
    fn deep_nesting_on_a_small_stack() -> Result<(), String> {
        // Tokio's worker threads have 2 MiB stacks; a recursive merge overflowed them at this
        // depth (and aborted the process). check_valid needs no stack per level either.
        thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(deep_merges)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the merge thread panicked".to_owned())?
    }
}
