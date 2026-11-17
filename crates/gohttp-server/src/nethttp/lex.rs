//! Lexical helpers from `golang.org/x/net/http/httpguts`, `net/http/internal/ascii`,
//! `net/textproto` and `net/http` (`hasToken`, `hexEscapeNonASCII`, `foreachHeaderElement`).

use pbs_gocompat::http::trim_string;

/// Go: `httpguts` `isTokenTable` / `textproto.validHeaderFieldByte` (RFC 7230 `tchar`).
pub(super) const fn is_token_byte(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
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
            | b'0'..=b'9'
            | b'a'..=b'z'
            | b'A'..=b'Z'
    )
}

/// Go: `httpguts.ValidHeaderFieldName` (also `http.isToken` and `validMethod`).
pub(super) fn is_token(v: &[u8]) -> bool {
    !v.is_empty() && v.iter().all(|&b| is_token_byte(b))
}

/// Go: `textproto.validHeaderValueByte`: VCHAR, SP, HTAB and obs-text.
pub(super) const fn valid_header_value_byte(c: u8) -> bool {
    c >= 0x80 || c == b' ' || c == b'\t' || (c >= 0x21 && c <= 0x7E)
}

/// Go: `httpguts.ValidHostHeader`.
pub(super) fn valid_host_header(h: &str) -> bool {
    h.bytes().all(|b| {
        matches!(
            b,
            b'0'..=b'9'
                | b'a'..=b'z'
                | b'A'..=b'Z'
                | b'!'
                | b'$'
                | b'%'
                | b'&'
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b':'
                | b';'
                | b'='
                | b'['
                | b'\''
                | b']'
                | b'_'
                | b'~'
        )
    })
}

/// Go: `httpguts.ValidHeaderFieldValue`: no control bytes other than space and tab.
pub(super) fn valid_header_field_value(v: &str) -> bool {
    v.bytes()
        .all(|b| !((b < b' ' || b == 0x7F) && b != b' ' && b != b'\t'))
}

/// Go: `internal/ascii.EqualFold`.
pub(super) const fn ascii_equal_fold(s: &str, t: &str) -> bool {
    s.eq_ignore_ascii_case(t)
}

const fn is_token_boundary(b: u8) -> bool {
    b == b' ' || b == b',' || b == b'\t'
}

/// Go: `http.hasToken`: whether `token` (lower case) appears in `v` with space, tab or comma
/// boundaries, ASCII case-insensitively.
pub(super) fn has_token(v: &str, token: &str) -> bool {
    let (vb, tb) = (v.as_bytes(), token.as_bytes());
    let Some(&first) = tb.first() else {
        return false;
    };
    if tb.len() > vb.len() {
        return false;
    }
    if v == token {
        return true;
    }
    for sp in 0..=(vb.len() - tb.len()) {
        let Some(&b) = vb.get(sp) else {
            break;
        };
        if b != first && (b | 0x20) != first {
            continue;
        }
        if sp > 0 && !vb.get(sp - 1).copied().is_some_and(is_token_boundary) {
            continue;
        }
        let end = sp + tb.len();
        if end != vb.len() && !vb.get(end).copied().is_some_and(is_token_boundary) {
            continue;
        }
        if vb
            .get(sp..end)
            .is_some_and(|w| w.iter().zip(tb).all(|(a, c)| a.to_ascii_lowercase() == *c))
        {
            return true;
        }
    }
    false
}

const fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn trim_ows(x: &str) -> &str {
    x.trim_matches(|c| c == ' ' || c == '\t')
}

/// Go: `httpguts.tokenEqual`: ASCII case-insensitive equality that rejects non-ASCII bytes in
/// `t1`.
const fn token_equal(t1: &str, t2: &str) -> bool {
    t1.len() == t2.len() && t1.is_ascii() && t1.eq_ignore_ascii_case(t2)
}

/// Go: `httpguts.headerValueContainsToken`.
fn header_value_contains_token(v: &str, token: &str) -> bool {
    v.split(',').any(|part| token_equal(trim_ows(part), token))
}

/// Go: `httpguts.HeaderValuesContainsToken`.
pub(super) fn header_values_contains_token(values: &[String], token: &str) -> bool {
    values.iter().any(|v| header_value_contains_token(v, token))
}

/// Go: `http.hexEscapeNonASCII`: every byte >= 0x80 becomes `%` and its lower-case hex value.
pub(super) fn hex_escape_non_ascii(s: &str) -> String {
    if s.is_ascii() {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b >= 0x80 {
            out.push('%');
            push_lower_hex(&mut out, u64::from(b));
        } else {
            out.push(char::from(b));
        }
    }
    out
}

/// Appends `v` in lower-case hexadecimal without leading zeros (Go `%x` / `AppendInt(.., 16)`).
pub(super) fn push_lower_hex(out: &mut String, v: u64) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0_u8; 16];
    let mut i = buf.len();
    let mut n = v;
    loop {
        i -= 1;
        let digit = DIGITS
            .get(usize::try_from(n & 0xF).unwrap_or(0))
            .copied()
            .unwrap_or(b'0');
        if let Some(slot) = buf.get_mut(i) {
            *slot = digit;
        }
        n >>= 4;
        if n == 0 || i == 0 {
            break;
        }
    }
    for &b in buf.get(i..).unwrap_or_default() {
        out.push(char::from(b));
    }
}

/// Go: `http.foreachHeaderElement`: the trimmed, non-empty comma-separated elements of `v`.
pub(super) fn header_elements(v: &str) -> impl Iterator<Item = &str> {
    let v = trim_string(v);
    v.split(',').map(trim_string).filter(|e| !e.is_empty())
}

/// Go: `httpguts.ValidTrailerHeader` (`name` already canonical).
pub(super) fn valid_trailer_header(name: &str) -> bool {
    !(name.starts_with("If-")
        || matches!(
            name,
            "Authorization"
                | "Cache-Control"
                | "Connection"
                | "Content-Encoding"
                | "Content-Length"
                | "Content-Range"
                | "Content-Type"
                | "Expect"
                | "Host"
                | "Keep-Alive"
                | "Max-Forwards"
                | "Pragma"
                | "Proxy-Authenticate"
                | "Proxy-Authorization"
                | "Proxy-Connection"
                | "Range"
                | "Realm"
                | "Te"
                | "Trailer"
                | "Transfer-Encoding"
                | "Www-Authenticate"
        ))
}

/// Go: `strings.TrimLeft(v, " \t")` on bytes.
pub(super) fn trim_left_ows(v: &[u8]) -> &[u8] {
    let start = v.iter().position(|&b| !is_ows(b)).unwrap_or(v.len());
    v.get(start..).unwrap_or_default()
}

/// Go: `textproto.trim`: leading and trailing spaces and tabs removed.
pub(super) fn trim_ows_bytes(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| !is_ows(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|&b| !is_ows(b)).map_or(start, |i| i + 1);
    s.get(start..end.max(start)).unwrap_or_default()
}

/// Converts wire bytes to a Go-like string: valid UTF-8 is kept, each invalid byte becomes
/// U+FFFD (the crate-wide convention for Go strings that are not UTF-8).
pub(super) fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;

    use super::{
        has_token, header_elements, header_values_contains_token, hex_escape_non_ascii,
        trim_ows_bytes, valid_host_header,
    };

    fn check<T: PartialEq + Debug>(got: &T, want: &T, name: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{name}: got {got:?}, want {want:?}"))
        }
    }

    #[test]
    fn has_token_matches_go() -> Result<(), String> {
        // Go: net/http/header_test.go TestHasToken.
        let cases: [(&str, &str, bool); 21] = [
            ("", "", false),
            ("", "foo", false),
            ("foo", "foo", true),
            ("foo ", "foo", true),
            (" foo", "foo", true),
            (" foo ", "foo", true),
            ("foo,bar", "foo", true),
            ("bar,foo", "foo", true),
            ("bar, foo", "foo", true),
            ("bar,foo, baz", "foo", true),
            ("bar, foo,baz", "foo", true),
            ("bar,foo, baz", "foo", true),
            ("bar, foo, baz", "foo", true),
            ("FOO", "foo", true),
            ("FOO ", "foo", true),
            (" FOO", "foo", true),
            (" FOO ", "foo", true),
            ("FOO,BAR", "foo", true),
            ("BAR,FOO", "foo", true),
            ("barfoo", "foo", false),
            ("foobar", "foo", false),
        ];
        for (header, token, want) in cases {
            check(&has_token(header, token), &want, header)?;
        }
        check(&has_token("bar, foo, baz", "baz"), &true, "last")?;
        check(&has_token("bar, foobar", "foo"), &false, "prefix")
    }

    #[test]
    fn helpers() -> Result<(), String> {
        check(
            &hex_escape_non_ascii("/a\u{e9}"),
            &"/a%c3%a9".to_owned(),
            "hex",
        )?;
        check(&valid_host_header("a b"), &false, "host space")?;
        check(&valid_host_header("[::1]:80"), &true, "host v6")?;
        check(
            &header_elements(" a, ,b ,").collect::<Vec<_>>(),
            &vec!["a", "b"],
            "elements",
        )?;
        check(
            &header_values_contains_token(&["Keep-Alive, x".to_owned()], "keep-alive"),
            &true,
            "contains",
        )?;
        check(&trim_ows_bytes(b" \tab \t"), &&b"ab"[..], "trim")
    }
}
