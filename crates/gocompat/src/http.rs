//! Go `net/http.Header` and `net/textproto` header-key canonicalization.
//!
//! Go: `net/http/header.go`, `net/textproto/reader.go` (`CanonicalMIMEHeaderKey`),
//! `net/textproto/textproto.go` (`TrimString`).
//!
//! A Go `http.Header` is a plain `map[string][]string`:
//!
//! - The methods (`Add`, `Set`, `Get`, `Values`, `Del`) canonicalize the key first.
//! - Map literals and index expressions (`http.Header{"x-key": {"v"}}`, `h["x-key"] = ...`) do not,
//!   so such keys keep their spelling and are invisible to `Get`. [`Header::insert_raw`],
//!   [`Header::get_raw`] and [`Header::remove_raw`] reproduce those operations.
//! - Iteration order in Go is random; everything Go derives from a header deterministically
//!   (`Header.Write`, `encoding/json`) sorts keys bytewise, which is the order of this type.
//!
//! Never route a Go header through a case-folding header map (such as `http::HeaderMap`): the
//! key spelling is part of Prebid's observable behaviour (outgoing requests, debug output).

use std::{
    collections::{BTreeMap, BTreeSet, btree_map},
    fmt::Write as _,
};

/// Go: `textproto.validHeaderFieldByte` and `httpguts.isTokenTable`: the RFC 7230 token bytes.
const fn valid_header_field_byte(c: u8) -> bool {
    matches!(
        c,
        b'0'..=b'9'
            | b'a'..=b'z'
            | b'A'..=b'Z'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}

/// Go: `textproto.canonicalMIMEHeaderKey` (the slow path).
fn canonicalize(s: &str) -> String {
    let bytes = s.as_bytes();
    // A key containing a byte that is neither a token byte nor a space is returned unchanged;
    // a key containing a space (and otherwise token bytes) is also returned unchanged.
    if bytes.iter().any(|&c| !valid_header_field_byte(c)) {
        return s.to_owned();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut upper = true;
    for &c in bytes {
        let mapped = if upper {
            c.to_ascii_uppercase()
        } else {
            c.to_ascii_lowercase()
        };
        out.push(mapped);
        upper = mapped == b'-';
    }
    // Every byte is ASCII, so this cannot fail; fall back to the input to stay total.
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// Go: `textproto.CanonicalMIMEHeaderKey` (and `http.CanonicalHeaderKey`).
///
/// The first letter and every letter after a hyphen are upper-cased, the rest lower-cased. A key
/// containing any byte that is not an RFC 7230 token byte (including space and non-ASCII) is
/// returned unchanged (Go 1.20+ behaviour).
#[must_use]
pub fn canonical_mime_header_key(s: &str) -> String {
    let mut upper = true;
    for &c in s.as_bytes() {
        if !valid_header_field_byte(c) {
            return s.to_owned();
        }
        if (upper && c.is_ascii_lowercase()) || (!upper && c.is_ascii_uppercase()) {
            return canonicalize(s);
        }
        upper = c == b'-';
    }
    s.to_owned()
}

/// Go: `http.CanonicalHeaderKey`.
#[must_use]
pub fn canonical_header_key(s: &str) -> String {
    canonical_mime_header_key(s)
}

/// Go: `httpguts.ValidHeaderFieldName`.
#[must_use]
pub fn valid_header_field_name(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(valid_header_field_byte)
}

/// Go: `textproto.TrimString`: trims leading and trailing ASCII space, tab, CR and LF.
#[must_use]
pub fn trim_string(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

/// Go: `http.Header` (`map[string][]string`), kept in bytewise key order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Header(BTreeMap<String, Vec<String>>);

/// Go: `net/http.reqWriteExcludeHeader`: the keys `Request.Write` does not take from the header
/// map (it writes `Host`, `User-Agent`, `Content-Length`, `Transfer-Encoding` and `Trailer`
/// itself).
pub const REQUEST_WRITE_EXCLUDE: [&str; 5] = [
    "Host",
    "User-Agent",
    "Content-Length",
    "Transfer-Encoding",
    "Trailer",
];

/// Go: `net/http.defaultUserAgent`, sent when the request header has no `User-Agent` key.
pub const DEFAULT_USER_AGENT: &str = "Go-http-client/1.1";

impl Header {
    /// An empty header (Go: `http.Header{}` or `make(http.Header)`).
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Go: `Header.Add`: appends `value` under the canonical form of `key`.
    pub fn add<K: AsRef<str>, V: Into<String>>(&mut self, key: K, value: V) {
        self.0
            .entry(canonical_mime_header_key(key.as_ref()))
            .or_default()
            .push(value.into());
    }

    /// Go: `Header.Set`: replaces the values under the canonical form of `key` with `value`.
    pub fn set<K: AsRef<str>, V: Into<String>>(&mut self, key: K, value: V) {
        let _previous = self
            .0
            .insert(canonical_mime_header_key(key.as_ref()), vec![value.into()]);
    }

    /// Go: `Header.Get`: the first value under the canonical form of `key`, or `""`.
    #[must_use]
    pub fn get(&self, key: &str) -> &str {
        self.values(key).first().map_or("", String::as_str)
    }

    /// Go: `Header.Values`: every value under the canonical form of `key` (empty when absent).
    #[must_use]
    pub fn values(&self, key: &str) -> &[String] {
        self.0
            .get(&canonical_mime_header_key(key))
            .map_or(&[], Vec::as_slice)
    }

    /// Go: `Header.Del`: removes the canonical form of `key`.
    pub fn del(&mut self, key: &str) {
        let _removed = self.0.remove(&canonical_mime_header_key(key));
    }

    /// Go: `Header.has` / `_, ok := h[CanonicalHeaderKey(key)]`.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(&canonical_mime_header_key(key))
    }

    /// Go: `h[key] = values` (no canonicalization).
    pub fn insert_raw<K: Into<String>>(&mut self, key: K, values: Vec<String>) {
        let _previous = self.0.insert(key.into(), values);
    }

    /// Go: `v, ok := h[key]` (no canonicalization).
    #[must_use]
    pub fn get_raw(&self, key: &str) -> Option<&[String]> {
        self.0.get(key).map(Vec::as_slice)
    }

    /// Go: `delete(h, key)` (no canonicalization).
    pub fn remove_raw(&mut self, key: &str) -> Option<Vec<String>> {
        self.0.remove(key)
    }

    /// Go: `len(h)`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Go: `len(h) == 0`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The entries in bytewise key order (the order of `Header.Write` and `encoding/json`).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The underlying map.
    #[must_use]
    pub const fn as_map(&self) -> &BTreeMap<String, Vec<String>> {
        &self.0
    }

    /// Consumes the header and returns the underlying map.
    #[must_use]
    pub fn into_map(self) -> BTreeMap<String, Vec<String>> {
        self.0
    }

    /// Go: `Header.WriteSubset`: the wire form of every entry whose key is not in `exclude`.
    ///
    /// Keys are sorted bytewise, keys that are not valid field names are dropped, and each
    /// value has CR and LF replaced by spaces and is then trimmed with [`trim_string`]. Each
    /// value is written as `Key: value\r\n`.
    #[must_use]
    pub fn write_subset(&self, exclude: &[&str]) -> String {
        let exclude: BTreeSet<&str> = exclude.iter().copied().collect();
        let mut out = String::new();
        for (key, value) in self.wire_fields(&exclude) {
            let _infallible = write!(out, "{key}: {value}\r\n");
        }
        out
    }

    /// Go: `Header.Write`.
    #[must_use]
    pub fn write(&self) -> String {
        self.write_subset(&[])
    }

    /// The `(key, value)` lines [`Header::write_subset`] writes, in order.
    #[must_use]
    pub fn wire_lines(&self, exclude: &[&str]) -> Vec<(String, String)> {
        let exclude: BTreeSet<&str> = exclude.iter().copied().collect();
        self.wire_fields(&exclude)
            .map(|(k, v)| (k.to_owned(), v))
            .collect()
    }

    fn wire_fields<'a>(
        &'a self,
        exclude: &'a BTreeSet<&str>,
    ) -> impl Iterator<Item = (&'a str, String)> + 'a {
        self.0
            .iter()
            .filter(move |(key, _)| !exclude.contains(key.as_str()))
            .filter(|(key, _)| valid_header_field_name(key))
            .flat_map(|(key, values)| {
                values.iter().map(move |value| {
                    let replaced = value.replace(['\n', '\r'], " ");
                    (key.as_str(), trim_string(&replaced).to_owned())
                })
            })
    }
}

impl<K: Into<String>> FromIterator<(K, Vec<String>)> for Header {
    /// Go: an `http.Header{...}` map literal: keys are stored exactly as written.
    fn from_iter<I: IntoIterator<Item = (K, Vec<String>)>>(iter: I) -> Self {
        Self(iter.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
}

impl From<BTreeMap<String, Vec<String>>> for Header {
    fn from(value: BTreeMap<String, Vec<String>>) -> Self {
        Self(value)
    }
}

impl<'a> IntoIterator for &'a Header {
    type IntoIter = btree_map::Iter<'a, String, Vec<String>>;
    type Item = (&'a String, &'a Vec<String>);

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{golden, testutil::ensure_eq};

    // Go: net/textproto TestCanonicalMIMEHeaderKey vectors plus Prebid header names, replayed
    // from testdata/go/golden/http_canonical.txt.
    #[test]
    fn canonical_keys_match_go() -> Result<(), String> {
        for row in golden::rows(include_str!("../testdata/go/golden/http_canonical.txt"))? {
            let [input, want] = golden::fields::<2>(&row)?;
            ensure_eq(
                canonical_mime_header_key(&input).as_str(),
                want.as_str(),
                &input,
            )?;
        }
        Ok(())
    }

    // Header method semantics and Write output, replayed from Go.
    #[test]
    fn header_ops_match_go() -> Result<(), String> {
        let mut h = Header::new();
        h.add("x-openrtb-version", "2.5");
        h.add("X-OPENRTB-VERSION", "2.6");
        h.set("content-type", "application/json;charset=utf-8");
        h.add("accept", " application/json\r\n");
        h.insert_raw("TOKEN", vec!["raw".to_owned()]);
        h.insert_raw("x-lower", vec!["literal".to_owned()]);
        h.insert_raw("bad key", vec!["dropped".to_owned()]);
        h.add("User-Agent", "ua");
        let got = [
            h.get("X-Openrtb-Version").to_owned(),
            h.values("x-openrtb-version").join(","),
            h.get("token").to_owned(),
            h.get("x-lower").to_owned(),
            h.get_raw("x-lower")
                .map_or_else(String::new, |v| v.join(",")),
            h.write(),
            h.write_subset(&REQUEST_WRITE_EXCLUDE),
        ];
        let rows = golden::rows(include_str!("../testdata/go/golden/http_header.txt"))?;
        ensure_eq(&rows.len(), &got.len(), "row count")?;
        for (row, value) in rows.iter().zip(got.iter()) {
            let [name, want] = golden::fields::<2>(row)?;
            ensure_eq(value.as_str(), want.as_str(), &name)?;
        }
        h.del("x-OpenRTB-version");
        ensure_eq(&h.has("X-Openrtb-Version"), &false, "deleted")
    }

    #[test]
    fn trim_string_trims_ascii_space_only() {
        assert_eq!(trim_string(" \t\r\nv \u{a0}\n"), "v \u{a0}", "NBSP is kept");
    }
}
