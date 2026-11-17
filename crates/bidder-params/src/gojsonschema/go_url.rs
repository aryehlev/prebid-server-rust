//! The part of Go's `net/url.Parse` that `gojsonreference.NewJsonReference` exposes to
//! gojsonschema: which strings fail to parse (with Go's error text), and what `URL.String()`
//! gives back for the plain `scheme://host/path` references used as `$schema` values.
//!
//! IPv6 literal hosts (`[...]`) are accepted without checking the address itself.

use std::fmt::Write as _;

use super::go_unicode::CATEGORIES;

/// Go: `gojsonreference.NewJsonReference(s)` then `.String()`, for draft detection.
///
/// Returns `Ok(Some(text))` with Go's `String()` for a reference without user info, query,
/// fragment or escapes, and `Ok(None)` for any other valid reference (whose `String()` is never
/// one of the draft URLs gojsonschema recognizes).
///
/// # Errors
///
/// Go's `*url.Error` text, for example `parse "::": missing protocol scheme`.
pub(super) fn parse_reference(raw: &str) -> Result<Option<String>, String> {
    let (url, fragment) = raw.split_once('#').unwrap_or((raw, ""));
    let parsed = parse(url).map_err(|e| format!("parse {}: {e}", go_quote(url)))?;
    if !fragment.is_empty() {
        unescape(fragment, Mode::Fragment).map_err(|e| format!("parse {}: {e}", go_quote(raw)))?;
        return Ok(None);
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Host,
    Path,
    UserPassword,
    Fragment,
}

/// Go: `net/url.parse(rawURL, false)`.
fn parse(raw: &str) -> Result<Option<String>, String> {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err("net/url: invalid control character in URL".to_owned());
    }
    if raw == "*" {
        return Ok(None);
    }
    let (scheme, rest) = get_scheme(raw)?;
    let scheme = scheme.to_ascii_lowercase();
    let (rest, has_query) = if rest.ends_with('?') && rest.matches('?').count() == 1 {
        (rest.strip_suffix('?').unwrap_or(rest), true)
    } else {
        rest.split_once('?')
            .map_or((rest, false), |(path, _query)| (path, true))
    };
    if !rest.starts_with('/') {
        if !scheme.is_empty() {
            // A rootless path is opaque.
            return Ok(None);
        }
        let segment = rest.split('/').next().unwrap_or(rest);
        if segment.contains(':') {
            return Err("first path segment in URL cannot contain colon".to_owned());
        }
    }
    let mut host = None;
    let mut path = rest;
    let mut has_user = false;
    if (!scheme.is_empty() || !rest.starts_with("///"))
        && let Some(after) = rest.strip_prefix("//")
    {
        let (authority, tail) = after.find('/').map_or((after, ""), |i| after.split_at(i));
        has_user = parse_authority(authority)?;
        host = Some(authority);
        path = tail;
    }
    unescape(path, Mode::Path)?;
    let plain = !has_query && !has_user && !raw.contains('%');
    Ok(match host {
        Some(host) if plain && !scheme.is_empty() => Some(format!("{scheme}://{host}{path}")),
        Some(_) | None => None,
    })
}

/// Go: `getScheme`.
fn get_scheme(raw: &str) -> Result<(&str, &str), String> {
    for (i, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if i > 0 => {}
            b':' if i == 0 => return Err("missing protocol scheme".to_owned()),
            b':' => {
                let (scheme, rest) = raw.split_at(i);
                return Ok((scheme, rest.get(1..).unwrap_or("")));
            }
            _ => return Ok(("", raw)),
        }
    }
    Ok(("", raw))
}

/// Go: `parseAuthority`; returns whether there is user information.
fn parse_authority(authority: &str) -> Result<bool, String> {
    let (userinfo, host) = authority.rfind('@').map_or((None, authority), |i| {
        let (user, at_host) = authority.split_at(i);
        (Some(user), at_host.get(1..).unwrap_or(""))
    });
    parse_host(host)?;
    let Some(userinfo) = userinfo else {
        return Ok(false);
    };
    if !valid_userinfo(userinfo) {
        return Err("net/url: invalid userinfo".to_owned());
    }
    match userinfo.split_once(':') {
        None => {
            unescape(userinfo, Mode::UserPassword)?;
        }
        Some((user, password)) => {
            unescape(user, Mode::UserPassword)?;
            unescape(password, Mode::UserPassword)?;
        }
    }
    Ok(true)
}

/// Go: `parseHost` (without the IPv6 address check).
fn parse_host(host: &str) -> Result<(), String> {
    if let Some(open) = host.rfind('[') {
        let Some(close) = host.rfind(']') else {
            return Err("missing ']' in host".to_owned());
        };
        let colon_port = host.get(close + 1..).unwrap_or("");
        if !valid_optional_port(colon_port) {
            return Err(format!("invalid port {} after host", go_quote(colon_port)));
        }
        unescape(colon_port, Mode::Host)?;
        let _literal = host.get(open + 1..close);
        return Ok(());
    }
    if let Some(i) = host.rfind(':') {
        let colon_port = host.get(i..).unwrap_or("");
        if !valid_optional_port(colon_port) {
            return Err(format!("invalid port {} after host", go_quote(colon_port)));
        }
    }
    unescape(host, Mode::Host)?;
    Ok(())
}

/// Go: `validOptionalPort`.
fn valid_optional_port(port: &str) -> bool {
    if port.is_empty() {
        return true;
    }
    port.strip_prefix(':')
        .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Go: `validUserinfo`.
fn valid_userinfo(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-._:~!$&'()*+,;=%@".contains(c) || !c.is_ascii())
}

/// Go: `unescape(s, mode)`, validating only (the decoded text is not needed).
fn unescape(s: &str, mode: Mode) -> Result<(), String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hi = bytes.get(i + 1).copied().filter(u8::is_ascii_hexdigit);
            let lo = bytes.get(i + 2).copied().filter(u8::is_ascii_hexdigit);
            let (Some(hi), Some(_)) = (hi, lo) else {
                let bad: String = String::from_utf8_lossy(bytes.get(i..).unwrap_or(&[]))
                    .chars()
                    .take(3)
                    .collect();
                return Err(format!("invalid URL escape {}", go_quote(&bad)));
            };
            let escape = String::from_utf8_lossy(bytes.get(i..i + 3).unwrap_or(&[])).into_owned();
            // In a host, %-encoding may only be used for non-ASCII bytes (and %25).
            if mode == Mode::Host && hi < b'8' && escape != "%25" {
                return Err(format!("invalid URL escape {}", go_quote(&escape)));
            }
            i += 3;
            continue;
        }
        if mode == Mode::Host && b < 0x80 && should_escape_host(b) {
            let bad = char::from(b).to_string();
            return Err(format!("invalid character {} in host name", go_quote(&bad)));
        }
        i += 1;
    }
    Ok(())
}

/// Go: `shouldEscape(c, encodeHost)` for ASCII bytes.
const fn should_escape_host(c: u8) -> bool {
    !matches!(
        c,
        b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'['
            | b']'
            | b'<'
            | b'>'
            | b'"'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
    )
}

/// Go: `strconv.Quote`.
pub(super) fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            ' ' => out.push(' '),
            c if is_go_printable(c) => out.push(c),
            c if u32::from(c) < 0x80 => {
                let _ok = write!(out, "\\x{:02x}", u32::from(c));
            }
            c if u32::from(c) < 0x1_0000 => {
                let _ok = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => {
                let _ok = write!(out, "\\U{:08x}", u32::from(c));
            }
        }
    }
    out.push('"');
    out
}

/// Go: `strconv.IsPrint`, which is `unicode.IsPrint`: the ASCII space and the letters, marks,
/// numbers, punctuation and symbols of Go's Unicode tables.
fn is_go_printable(c: char) -> bool {
    let r = u32::from(c);
    c == ' '
        || CATEGORIES.iter().any(|&(name, table, _)| {
            matches!(name, "L" | "M" | "N" | "P" | "S")
                && table.as_chunks::<3>().0.iter().any(|&[lo, hi, stride]| {
                    lo <= r && r <= hi && stride != 0 && (r - lo) % stride == 0
                })
        })
}

#[cfg(test)]
mod tests {
    use super::{go_quote, parse_reference};

    #[test]
    fn references_parse_like_go() -> Result<(), String> {
        let cases: [(&str, Result<Option<&str>, &str>); 10] = [
            (
                "http://json-schema.org/draft-04/schema#",
                Ok(Some("http://json-schema.org/draft-04/schema")),
            ),
            (
                "HTTP://json-schema.org/draft-04/schema",
                Ok(Some("http://json-schema.org/draft-04/schema")),
            ),
            ("http://json-schema.org/draft-04/schema#x", Ok(None)),
            ("http://json-schema.org/draft-04/schema?", Ok(None)),
            ("::", Err(r#"parse "::": missing protocol scheme"#)),
            (
                "http://x/%zz",
                Err(r#"parse "http://x/%zz": invalid URL escape "%zz""#),
            ),
            (
                " http://a",
                Err(r#"parse " http://a": first path segment in URL cannot contain colon"#),
            ),
            (
                "http://a b/",
                Err(r#"parse "http://a b/": invalid character " " in host name"#),
            ),
            (
                "http://a:x/",
                Err(r#"parse "http://a:x/": invalid port ":x" after host"#),
            ),
            (
                "a\u{1}",
                Err(r#"parse "a\x01": net/url: invalid control character in URL"#),
            ),
        ];
        for (input, want) in cases {
            let got = parse_reference(input);
            let want = want.map(|o| o.map(str::to_owned)).map_err(str::to_owned);
            if got != want {
                return Err(format!("{input:?}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn quote_matches_go() -> Result<(), String> {
        let cases = [
            ("a\"b", r#""a\"b""#),
            ("é\u{a0}", "\"é\\u00a0\""),
            ("\u{7f}", r#""\x7f""#),
        ];
        for (input, want) in cases {
            if go_quote(input) != want {
                return Err(format!("{input:?}: got {}, want {want}", go_quote(input)));
            }
        }
        // Go 1.26.2 (Unicode 15.0.0): `Some(escape)` is quoted as a backslash and `escape`,
        // `None` is kept as is.
        let runes = [
            ('\u{378}', Some("u0378")),
            ('\u{E000}', Some("ue000")),
            ('\u{1C89}', Some("u1c89")),
            ('\u{AD}', Some("u00ad")),
            ('\u{2028}', Some("u2028")),
            ('\u{3000}', Some("u3000")),
            ('\u{E0001}', Some("U000e0001")),
            ('\u{10FFFF}', Some("U0010ffff")),
            ('\u{1F600}', None),
            ('\u{FFFD}', None),
            ('\u{300}', None),
        ];
        for (rune, escape) in runes {
            let want = escape.map_or_else(|| format!("\"{rune}\""), |e| format!(r#""\{e}""#));
            let got = go_quote(&rune.to_string());
            if got != want {
                return Err(format!("{rune:?}: got {got}, want {want}"));
            }
        }
        Ok(())
    }
}
