//! Go: `net/url` (`url.go`, `encoding_table.go`), as built into Prebid Server.
//!
//! - Escaping: [`query_escape`], [`path_escape`], [`query_unescape`], [`path_unescape`] (and
//!   `_bytes` forms for byte strings).
//! - [`Url`]: [`Url::parse`], [`Url::parse_request_uri`], `String` (the [`Display`](fmt::Display)
//!   impl), [`Url::query`], [`Url::hostname`], [`Url::port`], [`Url::request_uri`],
//!   [`Url::escaped_path`], [`Url::escaped_fragment`], [`Url::is_abs`].
//! - [`Values`] with [`Values::encode`] (keys sorted) and [`parse_query`].
//!
//! GODEBUG defaults follow the repository's `go.mod` (`go 1.25.0`): `urlstrictcolons=0`, so a
//! host such as `host:1:2` is accepted for every scheme and its port is taken after the last
//! colon; `urlmaxqueryparams` keeps Go's limit of 10000 query parameters.
//!
//! Go strings are bytes, so percent-decoding can produce invalid UTF-8 (`%FF`). Decoded parts
//! are stored as Rust strings with each invalid byte replaced by `U+FFFD`
//! ([`crate::utf8::lossy_string`]), which is also what Go's JSON encoder emits for them. To keep
//! `String()` exact, [`Url::raw_path`] and [`Url::raw_fragment`] hold Go's `EscapedPath()` and
//! `EscapedFragment()` whenever the decoded part is not valid UTF-8 (Go's fields may then be
//! empty or hold an encoding that is not valid).
//! A host or userinfo with percent-encoded invalid UTF-8, and query values re-encoded by
//! [`Values::encode`], are re-escaped from the replacement character; the `_bytes` escaping
//! functions are exact.

mod netip;

use std::{collections::BTreeMap, error::Error as StdError, fmt};

pub use netip::ParseAddrError;

use crate::{
    strconv::{quote, quote_bytes},
    utf8::lossy_string,
};

const UPPERHEX: &[u8; 16] = b"0123456789ABCDEF";

/// Go: `encoding`, the URL component an escaping applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Path,
    PathSegment,
    Host,
    Zone,
    UserPassword,
    QueryComponent,
    Fragment,
}

const fn unhex(c: u8) -> u8 {
    9 * (c >> 6) + (c & 15)
}

/// Go: `shouldEscape` (the reference implementation in `gen_encoding_table.go`).
const fn should_escape(c: u8, mode: Encoding) -> bool {
    // §2.3 Unreserved characters (alphanum)
    if c.is_ascii_alphanumeric() {
        return false;
    }
    if matches!(mode, Encoding::Host | Encoding::Zone)
        && matches!(
            c,
            b'!' | b'$'
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
        )
    {
        return false;
    }
    match c {
        // §2.3 Unreserved characters (mark)
        b'-' | b'_' | b'.' | b'~' => return false,
        // §2.2 Reserved characters (reserved)
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => match mode {
            Encoding::Path => return c == b'?',
            Encoding::PathSegment => return matches!(c, b'/' | b';' | b',' | b'?'),
            Encoding::UserPassword => return matches!(c, b'@' | b'/' | b'?' | b':'),
            Encoding::QueryComponent => return true,
            Encoding::Fragment => return false,
            Encoding::Host | Encoding::Zone => {}
        },
        _ => {}
    }
    if matches!(mode, Encoding::Fragment) && matches!(c, b'!' | b'(' | b')' | b'*') {
        return false;
    }
    // Everything else must be escaped.
    true
}

/// Go: `url.EscapeError`: a malformed `%` escape (up to three bytes from the `%`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeError(pub Vec<u8>);

impl fmt::Display for EscapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid URL escape {}", quote_bytes(&self.0))
    }
}

impl StdError for EscapeError {}

/// Go: `url.InvalidHostError`: a byte that is not allowed in a host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHostError(pub String);

impl fmt::Display for InvalidHostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid character {} in host name", quote(&self.0))
    }
}

impl StdError for InvalidHostError {}

/// The error inside a [`UrlError`]: what `url.parse` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Go: `EscapeError`.
    Escape(EscapeError),
    /// Go: `InvalidHostError`.
    InvalidHost(InvalidHostError),
    /// Go: `fmt.Errorf("invalid port %q after host", colonPort)`.
    InvalidPort(String),
    /// Go: `fmt.Errorf("invalid host: %w", err)` for a bracketed host that is not an IP.
    InvalidHostAddr(ParseAddrError),
    /// Go: the `errors.New` texts (`missing protocol scheme`, `empty url`, ...).
    Message(&'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Escape(e) => e.fmt(f),
            Self::InvalidHost(e) => e.fmt(f),
            Self::InvalidPort(port) => write!(f, "invalid port {} after host", quote(port)),
            Self::InvalidHostAddr(e) => write!(f, "invalid host: {e}"),
            Self::Message(m) => f.write_str(m),
        }
    }
}

impl StdError for ParseError {}

impl From<UnescapeError> for ParseError {
    fn from(e: UnescapeError) -> Self {
        match e {
            UnescapeError::Escape(e) => Self::Escape(e),
            UnescapeError::InvalidHost(e) => Self::InvalidHost(e),
        }
    }
}

/// Go: `*url.Error`: `{op} {quote(url)}: {err}`, e.g. `parse "http://[::1": missing ']' in host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlError {
    /// Go: `Op` (`"parse"`).
    pub op: &'static str,
    /// Go: `URL`.
    pub url: String,
    /// Go: `Err`.
    pub err: ParseError,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.op, quote(&self.url), self.err)
    }
}

impl StdError for UrlError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.err)
    }
}

/// The errors of Go's `unescape`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UnescapeError {
    Escape(EscapeError),
    InvalidHost(InvalidHostError),
}

/// Go: `unescape`.
fn unescape(s: &[u8], mode: Encoding) -> Result<Vec<u8>, UnescapeError> {
    let escape_err = |from: usize, len: usize| {
        let end = from.saturating_add(len).min(s.len());
        UnescapeError::Escape(EscapeError(s.get(from..end).unwrap_or_default().to_vec()))
    };
    // Count %, check that they're well-formed.
    let mut n = 0_usize;
    let mut has_plus = false;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        match c {
            b'%' => {
                n += 1;
                let (Some(&h1), Some(&h2)) = (s.get(i + 1), s.get(i + 2)) else {
                    return Err(escape_err(i, 3));
                };
                if !h1.is_ascii_hexdigit() || !h2.is_ascii_hexdigit() {
                    return Err(escape_err(i, 3));
                }
                let is_pct25 = h1 == b'2' && h2 == b'5';
                // RFC 3986: in the host, %-encoding is only for non-ASCII bytes; RFC 6874
                // allows %25 for the zone separator.
                if mode == Encoding::Host && unhex(h1) < 8 && !is_pct25 {
                    return Err(escape_err(i, 3));
                }
                if mode == Encoding::Zone {
                    let v = (unhex(h1) << 4) | unhex(h2);
                    if !is_pct25 && v != b' ' && should_escape(v, Encoding::Host) {
                        return Err(escape_err(i, 3));
                    }
                }
                i += 3;
            }
            b'+' => {
                has_plus = mode == Encoding::QueryComponent;
                i += 1;
            }
            _ => {
                if matches!(mode, Encoding::Host | Encoding::Zone)
                    && c < 0x80
                    && should_escape(c, mode)
                {
                    return Err(UnescapeError::InvalidHost(InvalidHostError(
                        char::from(c).to_string(),
                    )));
                }
                i += 1;
            }
        }
    }
    if n == 0 && !has_plus {
        return Ok(s.to_vec());
    }
    let plus = if mode == Encoding::QueryComponent {
        b' '
    } else {
        b'+'
    };
    let mut t = Vec::with_capacity(s.len().saturating_sub(2 * n));
    let mut bytes = s.iter();
    while let Some(&c) = bytes.next() {
        match c {
            b'%' => {
                // Validated above: two hex digits follow.
                let hi = bytes.next().copied().map_or(0, unhex);
                let lo = bytes.next().copied().map_or(0, unhex);
                t.push((hi << 4) | lo);
            }
            b'+' => t.push(plus),
            _ => t.push(c),
        }
    }
    Ok(t)
}

/// Go: `escape`. The result is ASCII: every non-ASCII byte is escaped.
fn escape(s: &[u8], mode: Encoding) -> String {
    let mut t = String::with_capacity(s.len());
    for &c in s {
        if c == b' ' && mode == Encoding::QueryComponent {
            t.push('+');
        } else if should_escape(c, mode) {
            t.push('%');
            t.push(char::from(
                UPPERHEX.get(usize::from(c >> 4)).copied().unwrap_or(b'0'),
            ));
            t.push(char::from(
                UPPERHEX.get(usize::from(c & 15)).copied().unwrap_or(b'0'),
            ));
        } else {
            t.push(char::from(c));
        }
    }
    t
}

/// Go: `url.QueryEscape`.
#[must_use]
pub fn query_escape(s: &str) -> String {
    escape(s.as_bytes(), Encoding::QueryComponent)
}

/// Go: `url.QueryEscape` of a byte string.
#[must_use]
pub fn query_escape_bytes(s: &[u8]) -> String {
    escape(s, Encoding::QueryComponent)
}

/// Go: `url.PathEscape`.
#[must_use]
pub fn path_escape(s: &str) -> String {
    escape(s.as_bytes(), Encoding::PathSegment)
}

/// Go: `url.PathEscape` of a byte string.
#[must_use]
pub fn path_escape_bytes(s: &[u8]) -> String {
    escape(s, Encoding::PathSegment)
}

fn escape_only(r: Result<Vec<u8>, UnescapeError>) -> Result<Vec<u8>, EscapeError> {
    r.map_err(|e| match e {
        UnescapeError::Escape(e) => e,
        // Only the host and zone modes report invalid host characters.
        UnescapeError::InvalidHost(h) => EscapeError(h.0.into_bytes()),
    })
}

/// Go: `url.QueryUnescape`, returning the decoded bytes.
///
/// # Errors
///
/// [`EscapeError`] when a `%` is not followed by two hexadecimal digits.
pub fn query_unescape_bytes(s: &str) -> Result<Vec<u8>, EscapeError> {
    escape_only(unescape(s.as_bytes(), Encoding::QueryComponent))
}

/// Go: `url.QueryUnescape` (`+` decodes to a space). Invalid UTF-8 in the result is replaced
/// byte by byte with `U+FFFD`.
///
/// # Errors
///
/// [`EscapeError`] when a `%` is not followed by two hexadecimal digits.
pub fn query_unescape(s: &str) -> Result<String, EscapeError> {
    query_unescape_bytes(s).map(|b| lossy_string(&b))
}

/// Go: `url.PathUnescape`, returning the decoded bytes.
///
/// # Errors
///
/// [`EscapeError`] when a `%` is not followed by two hexadecimal digits.
pub fn path_unescape_bytes(s: &str) -> Result<Vec<u8>, EscapeError> {
    escape_only(unescape(s.as_bytes(), Encoding::PathSegment))
}

/// Go: `url.PathUnescape` (`+` stays `+`). Invalid UTF-8 in the result is replaced byte by
/// byte with `U+FFFD`.
///
/// # Errors
///
/// [`EscapeError`] when a `%` is not followed by two hexadecimal digits.
pub fn path_unescape(s: &str) -> Result<String, EscapeError> {
    path_unescape_bytes(s).map(|b| lossy_string(&b))
}

/// Go: `url.Userinfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Userinfo {
    username: String,
    password: String,
    password_set: bool,
}

impl Userinfo {
    /// Go: `url.User`.
    #[must_use]
    pub fn user<U: Into<String>>(username: U) -> Self {
        Self {
            username: username.into(),
            password: String::new(),
            password_set: false,
        }
    }

    /// Go: `url.UserPassword`.
    #[must_use]
    pub fn user_password<U: Into<String>, P: Into<String>>(username: U, password: P) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            password_set: true,
        }
    }

    /// Go: `Userinfo.Username`.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Go: `Userinfo.Password`: the password, or `None` when none was set.
    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.password_set.then_some(self.password.as_str())
    }
}

/// Go: `Userinfo.String`: `username[:password]`, escaped.
impl fmt::Display for Userinfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&escape(self.username.as_bytes(), Encoding::UserPassword))?;
        if self.password_set {
            write!(
                f,
                ":{}",
                escape(self.password.as_bytes(), Encoding::UserPassword)
            )?;
        }
        Ok(())
    }
}

/// Go: `url.URL`. Fields are in Go's order; see the module docs for how decoded bytes that are
/// not valid UTF-8 are held.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Url {
    /// Go: `Scheme` (lower-cased by `Parse`).
    pub scheme: String,
    /// Go: `Opaque` (encoded opaque data).
    pub opaque: String,
    /// Go: `User` (`nil` is `None`).
    pub user: Option<Userinfo>,
    /// Go: `Host` (`host` or `host:port`, decoded).
    pub host: String,
    /// Go: `Path` (decoded).
    pub path: String,
    /// Go: `Fragment` (decoded, without `#`).
    pub fragment: String,
    /// Go: `RawQuery` (encoded, without `?`).
    pub raw_query: String,
    /// Go: `RawPath`, the encoded path hint.
    pub raw_path: String,
    /// Go: `RawFragment`, the encoded fragment hint.
    pub raw_fragment: String,
    /// Go: `ForceQuery` (a trailing `?` with an empty query).
    pub force_query: bool,
    /// Go: `OmitHost` (an empty authority after the scheme).
    pub omit_host: bool,
}

/// Go: `getScheme`. Returns the scheme (empty when there is none) and the rest.
fn get_scheme(raw: &str) -> Result<(&str, &str), ParseError> {
    for (i, c) in raw.bytes().enumerate() {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' => {}
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                if i == 0 {
                    return Ok(("", raw));
                }
            }
            b':' => {
                if i == 0 {
                    return Err(ParseError::Message("missing protocol scheme"));
                }
                let (scheme, rest) = raw.split_at(i);
                return Ok((scheme, rest.get(1..).unwrap_or_default()));
            }
            // An invalid character: there is no valid scheme.
            _ => return Ok(("", raw)),
        }
    }
    Ok(("", raw))
}

/// Go: `stringContainsCTLByte`.
fn contains_ctl_byte(s: &str) -> bool {
    s.bytes().any(|b| b < b' ' || b == 0x7F)
}

/// Go: `validUserinfo`.
fn valid_userinfo(s: &str) -> bool {
    s.chars().all(|r| {
        r.is_ascii_alphanumeric()
            || matches!(
                r,
                '-' | '.'
                    | '_'
                    | ':'
                    | '~'
                    | '!'
                    | '$'
                    | '&'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
                    | '='
                    | '%'
                    | '@'
            )
    })
}

/// Go: `validOptionalPort`: empty, or `:` followed by digits.
fn valid_optional_port(port: &str) -> bool {
    port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Go: `validEncoded`.
fn valid_encoded(s: &str, mode: Encoding) -> bool {
    s.bytes().all(|c| {
        matches!(
            c,
            b'!' | b'$'
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
                | b'@'
                | b'['
                | b']'
                | b'%'
        ) || !should_escape(c, mode)
    })
}

/// Go: `parseAuthority`.
fn parse_authority(authority: &str) -> Result<(Option<Userinfo>, String), ParseError> {
    let at = authority.rfind('@');
    let host = parse_host(at.map_or(authority, |i| authority.get(i + 1..).unwrap_or_default()))?;
    let Some(i) = at else {
        return Ok((None, host));
    };
    let userinfo = authority.get(..i).unwrap_or_default();
    if !valid_userinfo(userinfo) {
        return Err(ParseError::Message("net/url: invalid userinfo"));
    }
    let decode = |s: &str| -> Result<String, ParseError> {
        Ok(lossy_string(&unescape(
            s.as_bytes(),
            Encoding::UserPassword,
        )?))
    };
    let user = match userinfo.split_once(':') {
        None => Userinfo::user(decode(userinfo)?),
        Some((username, password)) => Userinfo::user_password(decode(username)?, decode(password)?),
    };
    Ok((Some(user), host))
}

/// Go: `parseHost`, with `urlstrictcolons=0` (see the module docs).
fn parse_host(host: &str) -> Result<String, ParseError> {
    if let Some(open) = host.rfind('[') {
        if open > 0 {
            return Err(ParseError::Message("invalid IP-literal"));
        }
        // Parse an IP-Literal in RFC 3986 and RFC 6874: "[fe80::1]", "[fe80::1%25en0]:80".
        let Some(close) = host.rfind(']') else {
            return Err(ParseError::Message("missing ']' in host"));
        };
        let colon_port = host.get(close + 1..).unwrap_or_default();
        if !valid_optional_port(colon_port) {
            return Err(ParseError::InvalidPort(colon_port.to_owned()));
        }
        let unescaped_colon_port = unescape(colon_port.as_bytes(), Encoding::Host)?;
        let hostname = host.get(1..close).unwrap_or_default();
        // %25 introduces the zone identifier, which may %-encode more than the host can.
        let unescaped_hostname = match hostname.find("%25") {
            Some(zone) => {
                let (host_part, zone_part) = hostname.split_at(zone);
                let mut h = unescape(host_part.as_bytes(), Encoding::Host)?;
                h.extend(unescape(zone_part.as_bytes(), Encoding::Zone)?);
                h
            }
            None => unescape(hostname.as_bytes(), Encoding::Host)?,
        };
        // Only a valid IPv6 address may be enclosed in brackets (not IPv4, but IPv4-mapped).
        if netip::parse_addr(&unescaped_hostname).map_err(ParseError::InvalidHostAddr)? {
            return Err(ParseError::Message("invalid IP-literal"));
        }
        let mut out = Vec::with_capacity(unescaped_hostname.len() + 2 + colon_port.len());
        out.push(b'[');
        out.extend(unescaped_hostname);
        out.push(b']');
        out.extend(unescaped_colon_port);
        return Ok(lossy_string(&out));
    }
    if let Some(last_colon) = host.rfind(':') {
        // Go 1.26 enforces a single colon for http(s) unless urlstrictcolons=0, which this
        // module's go.mod (go 1.25.0) selects: the port always follows the last colon.
        let colon_port = host.get(last_colon..).unwrap_or_default();
        if !valid_optional_port(colon_port) {
            return Err(ParseError::InvalidPort(colon_port.to_owned()));
        }
    }
    Ok(lossy_string(&unescape(host.as_bytes(), Encoding::Host)?))
}

/// Go: `splitHostPort`.
fn split_host_port(host_port: &str) -> (&str, &str) {
    let (mut host, mut port) = (host_port, "");
    if let Some(colon) = host.rfind(':')
        && valid_optional_port(host.get(colon..).unwrap_or_default())
    {
        port = host.get(colon + 1..).unwrap_or_default();
        host = host.get(..colon).unwrap_or_default();
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        host = inner;
    }
    (host, port)
}

/// The `RawPath`/`RawFragment` value for `raw`, which decoded to `decoded`.
///
/// Go keeps `raw` only when it differs from the default escaping of `decoded`. When `decoded` is
/// not valid UTF-8 the Rust field holds `U+FFFD` instead of the bytes, so the hint instead keeps
/// what Go's `EscapedPath`/`EscapedFragment` returns: `raw` when it is a valid encoding,
/// otherwise the default escaping of the bytes.
fn raw_hint(raw: &str, decoded: &[u8], mode: Encoding) -> String {
    let escaped = escape(decoded, mode);
    if str::from_utf8(decoded).is_ok() {
        if escaped == raw {
            String::new()
        } else {
            raw.to_owned()
        }
    } else if valid_encoded(raw, mode) {
        raw.to_owned()
    } else {
        escaped
    }
}

/// Go: `strings.Cut(s, sep)` before the first `sep`, the rest and whether `sep` was found.
fn cut(s: &str, sep: char) -> (&str, &str) {
    s.split_once(sep).unwrap_or((s, ""))
}

impl Url {
    /// Go: `url.Parse`: a URL or URL reference, with an optional `#fragment`.
    ///
    /// # Errors
    ///
    /// [`UrlError`] with Go's text, e.g. `parse "http://a b": invalid character " " in host name`.
    pub fn parse(raw: &str) -> Result<Self, UrlError> {
        let (u, frag) = cut(raw, '#');
        let mut url = Self::parse_inner(u, false).map_err(|err| UrlError {
            op: "parse",
            url: u.to_owned(),
            err,
        })?;
        if frag.is_empty() {
            return Ok(url);
        }
        url.set_fragment(frag).map_err(|err| UrlError {
            op: "parse",
            url: raw.to_owned(),
            err: err.into(),
        })?;
        Ok(url)
    }

    /// Go: `url.ParseRequestURI`: an absolute URI or absolute path from an HTTP request line
    /// (no fragment).
    ///
    /// # Errors
    ///
    /// [`UrlError`] with Go's text, e.g. `parse "x": invalid URI for request`.
    pub fn parse_request_uri(raw: &str) -> Result<Self, UrlError> {
        Self::parse_inner(raw, true).map_err(|err| UrlError {
            op: "parse",
            url: raw.to_owned(),
            err,
        })
    }

    /// Go: `parse`.
    fn parse_inner(raw: &str, via_request: bool) -> Result<Self, ParseError> {
        if contains_ctl_byte(raw) {
            return Err(ParseError::Message(
                "net/url: invalid control character in URL",
            ));
        }
        if raw.is_empty() && via_request {
            return Err(ParseError::Message("empty url"));
        }
        if raw == "*" {
            return Ok(Self {
                path: "*".to_owned(),
                ..Self::default()
            });
        }
        let mut url = Self::default();
        // Split off a possible leading "http:", "mailto:", etc.
        let (scheme, mut rest) = get_scheme(raw)?;
        url.scheme = scheme.to_ascii_lowercase();
        if let Some(stripped) = rest.strip_suffix('?')
            && !stripped.contains('?')
        {
            url.force_query = true;
            rest = stripped;
        } else {
            let (before, query) = cut(rest, '?');
            rest = before;
            query.clone_into(&mut url.raw_query);
        }
        if !rest.starts_with('/') {
            if !url.scheme.is_empty() {
                // Rootless paths per RFC 3986 are opaque.
                rest.clone_into(&mut url.opaque);
                return Ok(url);
            }
            if via_request {
                return Err(ParseError::Message("invalid URI for request"));
            }
            // RFC 3986 §3.3: the first segment of a relative-path reference has no colon.
            if cut(rest, '/').0.contains(':') {
                return Err(ParseError::Message(
                    "first path segment in URL cannot contain colon",
                ));
            }
        }
        if (!url.scheme.is_empty() || !via_request && !rest.starts_with("///"))
            && let Some(after) = rest.strip_prefix("//")
        {
            let (authority, path) = after.find('/').map_or((after, ""), |i| after.split_at(i));
            (url.user, url.host) = parse_authority(authority)?;
            rest = path;
        } else if !url.scheme.is_empty() && rest.starts_with('/') {
            // An empty host (authority); see golang.org/issue/46059.
            url.omit_host = true;
        }
        url.set_path(rest)?;
        Ok(url)
    }

    /// Go: `setPath`, except that `raw_path` holds Go's `EscapedPath()` when the decoded path is
    /// not valid UTF-8.
    fn set_path(&mut self, p: &str) -> Result<(), UnescapeError> {
        let path = unescape(p.as_bytes(), Encoding::Path)?;
        self.raw_path = raw_hint(p, &path, Encoding::Path);
        self.path = lossy_string(&path);
        Ok(())
    }

    /// Go: `setFragment`, except that `raw_fragment` holds Go's `EscapedFragment()` when the
    /// decoded fragment is not valid UTF-8.
    fn set_fragment(&mut self, f: &str) -> Result<(), UnescapeError> {
        let frag = unescape(f.as_bytes(), Encoding::Fragment)?;
        self.raw_fragment = raw_hint(f, &frag, Encoding::Fragment);
        self.fragment = lossy_string(&frag);
        Ok(())
    }

    /// Whether `raw` is a valid encoding of `decoded` in `mode` (the `EscapedPath` check).
    fn raw_hint_matches(raw: &str, decoded: &str, mode: Encoding) -> bool {
        !raw.is_empty()
            && valid_encoded(raw, mode)
            && unescape(raw.as_bytes(), mode).is_ok_and(|p| lossy_string(&p) == decoded)
    }

    /// Go: `URL.EscapedPath`: `raw_path` when it is a valid encoding of `path`, otherwise
    /// `path` escaped.
    #[must_use]
    pub fn escaped_path(&self) -> String {
        if Self::raw_hint_matches(&self.raw_path, &self.path, Encoding::Path) {
            return self.raw_path.clone();
        }
        if self.path == "*" {
            return "*".to_owned(); // don't escape (Issue 11202)
        }
        escape(self.path.as_bytes(), Encoding::Path)
    }

    /// Go: `URL.EscapedFragment`.
    #[must_use]
    pub fn escaped_fragment(&self) -> String {
        if Self::raw_hint_matches(&self.raw_fragment, &self.fragment, Encoding::Fragment) {
            return self.raw_fragment.clone();
        }
        escape(self.fragment.as_bytes(), Encoding::Fragment)
    }

    /// Go: `URL.IsAbs` (the scheme is not empty).
    #[must_use]
    pub const fn is_abs(&self) -> bool {
        !self.scheme.is_empty()
    }

    /// Go: `URL.Query`: `raw_query` parsed, silently dropping malformed pairs.
    #[must_use]
    pub fn query(&self) -> Values {
        parse_query_pair(&self.raw_query).0
    }

    /// Go: `URL.RequestURI`: the encoded `path?query` or `opaque?query`.
    #[must_use]
    pub fn request_uri(&self) -> String {
        let mut result = if self.opaque.is_empty() {
            let p = self.escaped_path();
            if p.is_empty() { "/".to_owned() } else { p }
        } else if self.opaque.starts_with("//") {
            format!("{}:{}", self.scheme, self.opaque)
        } else {
            self.opaque.clone()
        };
        if self.force_query || !self.raw_query.is_empty() {
            result.push('?');
            result.push_str(&self.raw_query);
        }
        result
    }

    /// Go: `URL.Hostname`: the host without a valid port and without IPv6 brackets.
    #[must_use]
    pub fn hostname(&self) -> &str {
        split_host_port(&self.host).0
    }

    /// Go: `URL.Port`: the numeric port without the colon, or empty.
    #[must_use]
    pub fn port(&self) -> &str {
        split_host_port(&self.host).1
    }
}

/// Go: `URL.String`.
impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = String::new();
        if !self.scheme.is_empty() {
            buf.push_str(&self.scheme);
            buf.push(':');
        }
        if self.opaque.is_empty() {
            let omit_empty_host = self.omit_host && self.host.is_empty() && self.user.is_none();
            if (!self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some())
                && !omit_empty_host
            {
                if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                    buf.push_str("//");
                }
                if let Some(ui) = &self.user {
                    buf.push_str(&ui.to_string());
                    buf.push('@');
                }
                if !self.host.is_empty() {
                    buf.push_str(&escape(self.host.as_bytes(), Encoding::Host));
                }
            }
            let path = self.escaped_path();
            if !path.is_empty() && !path.starts_with('/') && !self.host.is_empty() {
                buf.push('/');
            }
            // RFC 3986 §4.2: a first segment with a colon needs a "./" prefix.
            if buf.is_empty() && cut(&path, '/').0.contains(':') {
                buf.push_str("./");
            }
            buf.push_str(&path);
        } else {
            buf.push_str(&self.opaque);
        }
        if self.force_query || !self.raw_query.is_empty() {
            buf.push('?');
            buf.push_str(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            buf.push('#');
            buf.push_str(&self.escaped_fragment());
        }
        f.write_str(&buf)
    }
}

/// Go: `url.Values`: query parameters by case-sensitive key. Go's map is unordered; this map is
/// sorted by key, which is the order [`Values::encode`] uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(BTreeMap<String, Vec<String>>);

impl Values {
    /// An empty set of values (Go: `url.Values{}`).
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Go: `Values.Get`: the first value for `key`, or `""`.
    #[must_use]
    pub fn get(&self, key: &str) -> &str {
        self.0
            .get(key)
            .and_then(|vs| vs.first())
            .map_or("", String::as_str)
    }

    /// Go: `v[key]`: every value for `key` (`None` when the key is absent).
    #[must_use]
    pub fn get_all(&self, key: &str) -> Option<&[String]> {
        self.0.get(key).map(Vec::as_slice)
    }

    /// Go: `Values.Set`: replaces the values for `key` with `value`.
    pub fn set<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        let _previous = self.0.insert(key.into(), vec![value.into()]);
    }

    /// Go: `Values.Add`: appends `value` to the values for `key`.
    pub fn add<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.0.entry(key.into()).or_default().push(value.into());
    }

    /// Go: `v[key] = values`.
    pub fn insert<K: Into<String>>(&mut self, key: K, values: Vec<String>) {
        let _previous = self.0.insert(key.into(), values);
    }

    /// Go: `Values.Del`.
    pub fn del(&mut self, key: &str) {
        let _removed = self.0.remove(key);
    }

    /// Go: `Values.Has`: whether `key` is present (even with no values).
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Go: `len(v)`: the number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The keys and their values, sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The underlying map.
    #[must_use]
    pub const fn as_map(&self) -> &BTreeMap<String, Vec<String>> {
        &self.0
    }

    /// The underlying map.
    #[must_use]
    pub fn into_map(self) -> BTreeMap<String, Vec<String>> {
        self.0
    }

    /// Go: `Values.Encode`: `key=value` pairs joined by `&`, sorted by key, each part
    /// [`query_escape`]d; a key with no values contributes nothing.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut buf = String::new();
        for (k, vs) in &self.0 {
            let key = query_escape(k);
            for v in vs {
                if !buf.is_empty() {
                    buf.push('&');
                }
                buf.push_str(&key);
                buf.push('=');
                buf.push_str(&query_escape(v));
            }
        }
        buf
    }
}

impl From<BTreeMap<String, Vec<String>>> for Values {
    fn from(m: BTreeMap<String, Vec<String>>) -> Self {
        Self(m)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, Vec<V>)> for Values {
    fn from_iter<I: IntoIterator<Item = (K, Vec<V>)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, vs)| (k.into(), vs.into_iter().map(Into::into).collect()))
                .collect(),
        )
    }
}

/// The error of [`parse_query`]: the first problem Go's `parseQuery` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// Go: `number of URL query parameters exceeded limit` (more than 10000 `&`-separated
    /// parts; no values are parsed).
    TooManyParams,
    /// Go: `invalid semicolon separator in query`.
    Semicolon,
    /// Go: the [`EscapeError`] of a key or value.
    Escape(EscapeError),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyParams => f.write_str("number of URL query parameters exceeded limit"),
            Self::Semicolon => f.write_str("invalid semicolon separator in query"),
            Self::Escape(e) => e.fmt(f),
        }
    }
}

impl StdError for QueryError {}

/// Go: `defaultMaxParams` (GODEBUG `urlmaxqueryparams` unset).
const MAX_PARAMS: usize = 10000;

/// Go: `url.ParseQuery`, returning the values even when there is an error, as Go does: every
/// valid pair is kept and the error describes the first problem.
#[must_use]
pub fn parse_query_pair(query: &str) -> (Values, Option<QueryError>) {
    let mut m = Values::new();
    if query.matches('&').count() + 1 > MAX_PARAMS {
        return (m, Some(QueryError::TooManyParams));
    }
    let mut err = None;
    for pair in query.split('&') {
        if pair.contains(';') {
            // Go: a semicolon error replaces any earlier error.
            err = Some(QueryError::Semicolon);
            continue;
        }
        if pair.is_empty() {
            continue;
        }
        let (key, value) = cut(pair, '=');
        let decoded = query_unescape(key).and_then(|k| Ok((k, query_unescape(value)?)));
        match decoded {
            Ok((k, v)) => m.add(k, v),
            Err(e) => {
                if err.is_none() {
                    err = Some(QueryError::Escape(e));
                }
            }
        }
    }
    (m, err)
}

/// Go: `url.ParseQuery`.
///
/// # Errors
///
/// The first [`QueryError`]; use [`parse_query_pair`] to keep the valid pairs as well.
pub fn parse_query(query: &str) -> Result<Values, QueryError> {
    match parse_query_pair(query) {
        (m, None) => Ok(m),
        (_, Some(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests;
