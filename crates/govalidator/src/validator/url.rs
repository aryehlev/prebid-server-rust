//! Glue (Rule B (c)): the part of Go's `net/url` that govalidator's checks read, which decides
//! which bidder endpoints, user sync URLs, VAST event URLs and floors fetch URLs Prebid accepts
//! (url.govalidator-checks).
//!
//! Go: `url.Parse` and `url.ParseRequestURI` of Go 1.26.2 (`net/url/url.go`): whether they
//! accept a string, and the scheme, host and path of what they accept. The url crate parses by
//! the WHATWG rules, which accept and reject other strings, so it cannot stand in here. Go 1.26
//! splits an `http` or `https` host at its first colon when `urlstrictcolons` is 1; a module
//! whose go.mod says `go 1.25.0`, as Prebid's does, defaults it to 0, so every host splits at its
//! last colon, as before Go 1.26.

use std::{net::Ipv6Addr, str};

/// What `url.Parse` or `url.ParseRequestURI` found in a string it accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Url {
    /// Go: `URL.Scheme`, lower-cased.
    pub(super) scheme: String,
    /// Go: `URL.Host`, unescaped.
    pub(super) host: Vec<u8>,
    /// Go: `URL.Path`, unescaped; empty for an opaque URL (`mailto:x`).
    pub(super) path: Vec<u8>,
}

/// The part of a URL `unescape` decodes (Go: `encoding`), as far as it decides what fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Go: `encodePath`.
    Path,
    /// Go: `encodeHost`.
    Host,
    /// Go: `encodeZone`.
    Zone,
    /// Go: `encodeUserPassword`.
    UserPassword,
    /// Go: `encodeFragment`.
    Fragment,
}

/// Go: `url.Parse`: `raw` cut at its first `#`, parsed as any URL reference, and its fragment
/// unescaped; `None` where Go fails.
pub(super) fn parse(raw: &str) -> Option<Url> {
    let (head, fragment) = raw.split_once('#').unwrap_or((raw, ""));
    let url = parse_with(head, false)?;
    unescape(fragment.as_bytes(), Mode::Fragment).map(|_fragment| url)
}

/// Go: `url.ParseRequestURI`: `raw` read as an absolute URI or an absolute path, with no
/// fragment; `None` where Go fails.
pub(super) fn parse_request_uri(raw: &str) -> Option<Url> {
    parse_with(raw, true)
}

/// Go: `parse(rawURL, viaRequest)`.
fn parse_with(raw: &str, via_request: bool) -> Option<Url> {
    // Go: invalid control character in URL, and empty url.
    if raw.bytes().any(|b| b < b' ' || b == 0x7F) || (raw.is_empty() && via_request) {
        return None;
    }
    if raw == "*" {
        return Some(Url {
            scheme: String::new(),
            host: Vec::new(),
            path: b"*".to_vec(),
        });
    }
    let (scheme, rest) = scheme(raw)?;
    let scheme = scheme.to_ascii_lowercase();
    // Go cuts the query off at the first `?` (a lone trailing `?` sets ForceQuery instead,
    // which leaves the same rest).
    let rest = rest.split_once('?').map_or(rest, |(path, _query)| path);
    if !rest.starts_with('/') {
        if !scheme.is_empty() {
            // Go: a rootless path is opaque, and nothing more is read.
            return Some(Url {
                scheme,
                host: Vec::new(),
                path: Vec::new(),
            });
        }
        // Go: invalid URI for request.
        if via_request {
            return None;
        }
        // Go: first path segment in URL cannot contain colon.
        let segment = rest
            .split_once('/')
            .map_or(rest, |(segment, _path)| segment);
        if segment.contains(':') {
            return None;
        }
    }
    let (host, path) = if (!scheme.is_empty() || (!via_request && !rest.starts_with("///")))
        && let Some(after) = rest.strip_prefix("//")
    {
        let (authority, tail) = after
            .find('/')
            .and_then(|i| after.split_at_checked(i))
            .unwrap_or((after, ""));
        (parse_authority(authority)?, tail)
    } else {
        (Vec::new(), rest)
    };
    let path = unescape(path.as_bytes(), Mode::Path)?;
    Some(Url { scheme, host, path })
}

/// Go: `getScheme`: the scheme before the first `:` and what follows it, or no scheme and all
/// of `raw` when a byte no scheme holds comes first; `None` when `raw` starts with `:` (Go:
/// missing protocol scheme).
fn scheme(raw: &str) -> Option<(&str, &str)> {
    for (i, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' if i > 0 => {}
            b':' if i == 0 => return None,
            b':' => {
                let (scheme, rest) = raw.split_at_checked(i)?;
                return Some((scheme, rest.get(1..).unwrap_or_default()));
            }
            _ => return Some(("", raw)),
        }
    }
    Some(("", raw))
}

/// Go: `parseAuthority`: the host, unescaped, of `[userinfo@]host[:port]`. The userinfo ends at
/// the last `@`, must hold only the bytes `validUserinfo` allows and must unescape.
fn parse_authority(authority: &str) -> Option<Vec<u8>> {
    let Some(at) = authority.rfind('@') else {
        return parse_host(authority);
    };
    let (userinfo, host) = authority.split_at_checked(at)?;
    let host = parse_host(host.get(1..).unwrap_or_default())?;
    let (username, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    (valid_userinfo(userinfo)
        && unescape(username.as_bytes(), Mode::UserPassword).is_some()
        && unescape(password.as_bytes(), Mode::UserPassword).is_some())
    .then_some(host)
}

/// Go: `parseHost`: `host[:port]`, unescaped, where the host is a name, an IPv4 address or a
/// bracketed IPv6 address with an optional zone.
fn parse_host(host: &str) -> Option<Vec<u8>> {
    match host.rfind('[') {
        // Go: invalid IP-literal.
        Some(open) if open > 0 => None,
        Some(_) => ip_literal(host),
        None => {
            // Go: the port after the last colon (urlstrictcolons=0) must be digits.
            if let Some(last) = host.rfind(':')
                && !valid_optional_port(host.get(last..).unwrap_or_default())
            {
                return None;
            }
            unescape(host.as_bytes(), Mode::Host)
        }
    }
}

/// Go: the IP-literal branch of `parseHost`: `[address]` or `[address%25zone]` with an
/// optional port, where the address must be IPv6 (an IPv4-mapped one included) and the zone
/// may escape only the bytes a host may hold, a space and `%`.
fn ip_literal(host: &str) -> Option<Vec<u8>> {
    // Go: missing ']' in host.
    let close = host.rfind(']')?;
    let (bracketed, colon_port) = host.split_at_checked(close)?;
    let colon_port = colon_port.get(1..).unwrap_or_default();
    if !valid_optional_port(colon_port) {
        return None;
    }
    let port = unescape(colon_port.as_bytes(), Mode::Host)?;
    let hostname = bracketed.get(1..).unwrap_or_default();
    let address = match hostname.find("%25") {
        Some(zone) => {
            let (address, zone) = hostname.split_at_checked(zone)?;
            let mut unescaped = unescape(address.as_bytes(), Mode::Host)?;
            unescaped.extend(unescape(zone.as_bytes(), Mode::Zone)?);
            unescaped
        }
        None => unescape(hostname.as_bytes(), Mode::Host)?,
    };
    // Go: invalid host (netip.ParseAddr fails), and invalid IP-literal (an IPv4 address).
    if !ipv6_literal(str::from_utf8(&address).ok()?) {
        return None;
    }
    let mut out = Vec::with_capacity(address.len() + port.len() + 2);
    out.push(b'[');
    out.extend(address);
    out.push(b']');
    out.extend(port);
    Some(out)
}

/// Whether Go's `netip.ParseAddr` reads `s` as an IPv6 address, with or without a zone. It
/// picks its parser by the first of `.`, `:` and `%` in `s`, so only `:` can give IPv6; the
/// zone, after the first `%`, must not be empty.
fn ipv6_literal(s: &str) -> bool {
    if s.bytes().find(|b| matches!(b, b'.' | b':' | b'%')) != Some(b':') {
        return false;
    }
    let (address, zone) = s
        .split_once('%')
        .map_or((s, None), |(address, zone)| (address, Some(zone)));
    zone != Some("") && address.parse::<Ipv6Addr>().is_ok()
}

/// Go: `validUserinfo`: letters, digits and `-._:~!$&'()*+,;=%@` only.
fn valid_userinfo(s: &str) -> bool {
    s.bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c))
}

/// Go: `validOptionalPort`: empty, or `:` followed by digits only.
fn valid_optional_port(port: &str) -> bool {
    port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Go: `shouldEscape(c, encodeHost)`, which `encodeZone` shares: whether byte `c` may not stand
/// as it is in a host. Letters, digits and `-._~!$&'()*+,;=:[]<>"` may; no byte above 0x7F may.
fn host_should_escape(c: u8) -> bool {
    !(c.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:[]<>\"".contains(&c))
}

/// The value of a hex digit.
const fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Go: `unescape(s, mode)`: `s` with each `%XX` decoded (`+` stays `+` outside a query), or
/// `None` where Go fails: a `%` not followed by two hex digits; in a host, an escape of an
/// ASCII byte other than `%25`; in a zone, an escape of a byte a host may not hold, but a space
/// and `%25`; and in either, an ASCII byte a host may not hold.
fn unescape(s: &[u8], mode: Mode) -> Option<Vec<u8>> {
    let host_like = matches!(mode, Mode::Host | Mode::Zone);
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s;
    while let Some((&c, tail)) = rest.split_first() {
        if c != b'%' {
            if host_like && c.is_ascii() && host_should_escape(c) {
                return None;
            }
            out.push(c);
            rest = tail;
            continue;
        }
        let &[hi, lo, ..] = tail else {
            return None;
        };
        let (Some(high), Some(low)) = (hex_value(hi), hex_value(lo)) else {
            return None;
        };
        let byte = (high << 4) | low;
        let percent = hi == b'2' && lo == b'5';
        match mode {
            Mode::Host if high < 8 && !percent => return None,
            Mode::Zone if !percent && byte != b' ' && host_should_escape(byte) => return None,
            Mode::Path | Mode::Host | Mode::Zone | Mode::UserPassword | Mode::Fragment => {}
        }
        out.push(byte);
        rest = tail.get(2..).unwrap_or_default();
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{Url, parse, parse_request_uri};

    fn url((scheme, host, path): (&str, &str, &str)) -> Url {
        Url {
            scheme: scheme.to_owned(),
            host: host.as_bytes().to_vec(),
            path: path.as_bytes().to_vec(),
        }
    }

    /// Go: `TestParse`, `TestParseRequestURI` and the error cases of `url_test.go`, as far as
    /// the verdict, scheme, host and path go.
    #[test]
    fn parses_like_go() -> Result<(), String> {
        let cases = [
            (
                "http://www.google.com",
                Some(("http", "www.google.com", "")),
            ),
            (
                "HTTP://www.google.com/a%2Fb",
                Some(("http", "www.google.com", "/a/b")),
            ),
            (
                "http://user:pa%20ss@[fe80::1%25en0]:8080/x",
                Some(("http", "[fe80::1%en0]:8080", "/x")),
            ),
            ("mailto:webmaster@golang.org", Some(("mailto", "", ""))),
            ("/foo?bar#baz", Some(("", "", "/foo"))),
            ("*", Some(("", "", "*"))),
            ("//foo/bar", Some(("", "foo", "/bar"))),
            ("http://::1/", Some(("http", "::1", "/"))),
            (
                "http://localhost:80:80/",
                Some(("http", "localhost:80:80", "/")),
            ),
            (
                "http://[::ffff:1.2.3.4]/",
                Some(("http", "[::ffff:1.2.3.4]", "/")),
            ),
            ("http://[1.2.3.4]/", None),
            ("http://[fe80::1%25]/", None),
            ("http://[fe80::1%25%C3%A9]/", None),
            ("http://a[::1]/", None),
            ("http://[::1/", None),
            ("http://a b.com/", None),
            ("http://%41.com/", None),
            ("http://%C3%A9.com/", Some(("http", "\u{e9}.com", "/"))),
            ("http://a.com:x/", None),
            ("http://a<b>.com/", Some(("http", "a<b>.com", "/"))),
            ("http://us{er@a.com/", None),
            ("http://a.com/%zz", None),
            ("http://a.com/#%zz", None),
            ("a:b/c", Some(("a", "", ""))),
            ("1a:b", None),
            (":foo", None),
            ("http://a.com/\u{7f}", None),
        ];
        for (raw, want) in cases {
            let want = want.map(url);
            if parse(raw) != want {
                return Err(format!("Parse({raw:?}) = {:?}, want {want:?}", parse(raw)));
            }
        }
        let request = [
            ("/a/b", true),
            ("http://a.com/#frag", true),
            ("a/b", false),
            ("", false),
            ("*", true),
            ("mailto:x", true),
        ];
        for (raw, want) in request {
            if parse_request_uri(raw).is_some() != want {
                return Err(format!("ParseRequestURI({raw:?}): want ok {want}"));
            }
        }
        Ok(())
    }
}
