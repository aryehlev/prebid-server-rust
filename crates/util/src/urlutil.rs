//! Glue (Rule B (d)): the bytes of `net/url`'s escapes, which Prebid writes into URLs; and Go
//! `util/urlutil`.
//!
//! URLs are parsed and written by the url crate, and query strings and form bodies read by
//! axum's extractors or `form_urlencoded` (the url.host-normalization and url.query-parse
//! ledger entries). What stays Go's is the escaping: Prebid's macros and user sync URLs
//! escape values with `url.QueryEscape` and `url.PathEscape`, and adapters write query
//! strings with `url.Values.Encode`, so the bytes bidders and browsers receive are Go's (the
//! url.escape-sets and url.values-order ledger entries). [`is_safe_host`] is a port of
//! Prebid's own check, not glue.

use std::{collections::BTreeMap, sync::LazyLock};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;

use crate::goregex;

/// The bytes Go's `url.QueryEscape` escapes (`shouldEscape` with `encodeQueryComponent`):
/// everything but ASCII letters, digits and `-`, `_`, `.` and `~`. A space, written as `+`, is
/// handled before the set applies.
const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The bytes Go's `url.PathEscape` escapes (`shouldEscape` with `encodePathSegment`): the
/// query set but `$`, `&`, `+`, `:`, `=` and `@`, which a path segment keeps.
const PATH_SEGMENT: &AsciiSet = &QUERY
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b':')
    .remove(b'=')
    .remove(b'@');

/// Go: `url.QueryEscape`: `s` escaped to sit in a URL query, a space as `+` and every other
/// byte outside `[A-Za-z0-9_.~-]` as an upper-case `%XX`.
#[must_use]
pub fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, part) in s.split(' ').enumerate() {
        if i > 0 {
            out.push('+');
        }
        out.extend(utf8_percent_encode(part, QUERY));
    }
    out
}

/// Go: `url.PathEscape`: `s` escaped to sit in a URL path segment, as [`query_escape`] but with
/// a space as `%20` and `$&+:=@` kept.
#[must_use]
pub fn path_escape(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).collect()
}

/// Go: `url.Values.Encode`: the pairs as a query string, `key=value` joined by `&`, sorted by
/// key and in insertion order within a key, both escaped with [`query_escape`]. A key with no
/// values writes nothing.
#[must_use]
pub fn values_encode<K, V>(values: &BTreeMap<K, Vec<V>>) -> String
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut out = String::new();
    for (key, list) in values {
        let key = query_escape(key.as_ref());
        for value in list {
            if !out.is_empty() {
                out.push('&');
            }
            out.push_str(&key);
            out.push('=');
            out.push_str(&query_escape(value.as_ref()));
        }
    }
    out
}

/// Go: `urlutil.safeHostRegex` (util/urlutil/security.go:5), compiled once; `None` if it does
/// not compile, which it does.
static SAFE_HOST: LazyLock<Option<Regex>> =
    LazyLock::new(|| goregex::compile("^[a-zA-Z0-9.-]+(:[0-9]+)?$").ok());

/// Go: `urlutil.IsSafeHost`: whether `host` matches `^[a-zA-Z0-9.-]+(:[0-9]+)?$`, a bare host
/// name with an optional port.
///
/// It rejects URL control characters such as `/`, `?`, `#` and `@`, so a user-supplied host
/// cannot rewrite an outbound request URL. As in Go's regexp, `$` only matches at the end of
/// the text, so a trailing newline is rejected.
#[must_use]
pub fn is_safe_host(host: &str) -> bool {
    SAFE_HOST.as_ref().is_some_and(|re| re.is_match(host))
}

#[cfg(test)]
mod tests {
    use std::str;

    use super::is_safe_host;
    use crate::golden::{TestResult, check, check_eq, fields, read_golden};

    #[test]
    fn is_safe_host_go_cases() -> TestResult {
        // Go: TestIsSafeHost.
        let cases = [
            ("hostname", "example.com", true),
            ("subdomain", "api-us", true),
            ("hostname with port", "example.com:8080", true),
            ("empty", "", false),
            ("path injection", "127.0.0.1:6060/debug/pprof", false),
            ("fragment injection", "127.0.0.1:6060#", false),
            ("query injection", "example.com?x=1", false),
            ("userinfo injection", "example.com@127.0.0.1", false),
            ("scheme injection", "http://127.0.0.1", false),
        ];
        for (name, host, want) in cases {
            check_eq(&is_safe_host(host), &want, name)?;
        }
        Ok(())
    }

    #[test]
    fn is_safe_host_golden() -> TestResult {
        // Go: util/urlutil/security.go:5, against curated hosts and random strings of host,
        // URL and white space bytes. A Rust string is UTF-8, and Go's ASCII-only pattern
        // rejects every string that is not.
        let rows = read_golden("urlutil.txt")?;
        check(rows.len() > 2000, "urlutil.txt is complete")?;
        let mut invalid_utf8 = 0_usize;
        for row in &rows {
            let [s, want] = fields::<2>(row)?;
            let what = String::from_utf8_lossy(s);
            if let Ok(host) = str::from_utf8(s) {
                let got: &[u8] = if is_safe_host(host) {
                    b"true"
                } else {
                    b"false"
                };
                check_eq(got, want, &what)?;
            } else {
                check_eq(want, b"false".as_slice(), &what)?;
                invalid_utf8 += 1;
            }
        }
        check(invalid_utf8 > 0, "the golden has rows that are not UTF-8")
    }
}
