//! Go `util/httputil`: client IP discovery, the `Content-Encoding` names and the 1x1 tracking
//! pixel.
//!
//! ```
//! use pbs_gocompat::http::Header;
//! use pbs_util::{
//!     httputil::find_ip,
//!     iputil::{IpVersion, PublicNetworkIpValidator, net::parse_cidr},
//! };
//!
//! let validator = PublicNetworkIpValidator {
//!     ipv4_private_networks: vec![parse_cidr("10.0.0.0/8")?.1],
//!     ipv6_private_networks: Vec::new(),
//! };
//! let mut headers = Header::default();
//! headers.add("X-Forwarded-For", " 10.0.0.1, 203.0.113.7 , 8.8.8.8");
//! // The first public address of the list wins; private and malformed ones are skipped.
//! let (ip, ver) = find_ip(&headers, "10.1.1.1:5678", &validator).ok_or("no IP")?;
//! assert_eq!(
//!     (ip.to_string(), ver),
//!     ("203.0.113.7".to_owned(), IpVersion::V4)
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod pixel;

use std::str;

use pbs_gocompat::{http::Header, strings, unicode};
pub use pixel::{PIXEL_1X1_PNG, Pixel};

use crate::iputil::{self, IpValidator, IpVersion, net::Ip};

/// Go: `http.CanonicalHeaderKey("True-Client-IP")`.
pub const TRUE_CLIENT_IP: &str = "True-Client-Ip";
/// Go: `http.CanonicalHeaderKey("X-Forwarded-For")`.
pub const X_FORWARDED_FOR: &str = "X-Forwarded-For";
/// Go: `http.CanonicalHeaderKey("X-Real-IP")`.
pub const X_REAL_IP: &str = "X-Real-Ip";

/// Go: `httputil.ContentEncoding`, a `Content-Encoding` header value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentEncoding(pub String);

impl ContentEncoding {
    /// Go: `httputil.ContentEncodingGZIP`.
    pub const GZIP: &'static str = "gzip";

    /// Go: `ContentEncoding.Normalize`: the value lowercased with `strings.ToLower`.
    #[must_use]
    pub fn normalize(&self) -> Self {
        Self(strings::to_lower(&self.0))
    }

    /// The header value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Go: `contentEncoding.Normalize() == httputil.ContentEncodingGZIP`.
    ///
    /// This is not an ASCII case-insensitive comparison: Go's `ToLower` maps U+0130 LATIN
    /// CAPITAL LETTER I WITH DOT ABOVE to `i`, so `gzİp` is gzip.
    #[must_use]
    pub fn is_gzip(&self) -> bool {
        self.normalize().0 == Self::GZIP
    }
}

/// The request headers [`find_ip`] reads.
///
/// Go reads them with `r.Header.Get`, which returns the first value of the canonical key. Go
/// header values are byte strings, so values are bytes here.
pub trait RequestHeaders {
    /// The first value of the header whose canonical name is `canonical_key`, or `None` when
    /// the request has no such header.
    fn first_value(&self, canonical_key: &str) -> Option<&[u8]>;
}

impl RequestHeaders for Header {
    fn first_value(&self, canonical_key: &str) -> Option<&[u8]> {
        self.get_raw(canonical_key)
            .and_then(<[String]>::first)
            .map(String::as_bytes)
    }
}

impl<T: RequestHeaders + ?Sized> RequestHeaders for &T {
    fn first_value(&self, canonical_key: &str) -> Option<&[u8]> {
        (**self).first_value(canonical_key)
    }
}

/// Go: `httputil.FindIP`: the first address accepted by `v`, looked up in this order:
///
/// 1. `True-Client-IP`, trimmed of white space;
/// 2. each comma-separated element of `X-Forwarded-For`, trimmed, from left to right;
/// 3. `X-Real-IP`, trimmed;
/// 4. the host of `remote_addr` (Go: `r.RemoteAddr`) as split by `net.SplitHostPort`.
///
/// Only the first value of each header is read. The version is that of
/// [`iputil::parse_ip`]: IPv6 whenever the text contains `:`.
///
/// `remote_addr` must be the text Go's server would store for the peer: build it with
/// [`net::tcp_addr_string`](iputil::net::tcp_addr_string), not with `SocketAddr`'s `Display`,
/// which writes an IPv4 peer of a dual-stack listener as `[::ffff:a.b.c.d]:port` and so turns
/// its version to IPv6. Go gives a Unix socket peer its socket name (`@` on Linux for the
/// usual unnamed peer), which has no port, so no address comes from it.
#[must_use]
pub fn find_ip<H, V>(r: &H, remote_addr: &str, v: &V) -> Option<(Ip, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    find_true_client_ip(r, v)
        .or_else(|| find_forwarded_for(r, v))
        .or_else(|| find_real_ip(r, v))
        .or_else(|| find_remote_addr(remote_addr, v))
}

/// Go: `r.Header.Get(key)` with Go's empty-string check.
fn header<'a, H: RequestHeaders + ?Sized>(r: &'a H, key: &str) -> Option<&'a [u8]> {
    r.first_value(key).filter(|value| !value.is_empty())
}

/// Parses `value` and keeps it when the validator accepts it.
fn valid<V: IpValidator + ?Sized>(value: &[u8], v: &V) -> Option<(Ip, IpVersion)> {
    iputil::parse_ip_bytes(value).filter(|(ip, ver)| v.is_valid(ip, *ver))
}

/// Go: `findTrueClientIP`.
fn find_true_client_ip<H, V>(r: &H, v: &V) -> Option<(Ip, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, TRUE_CLIENT_IP).and_then(|value| valid(trim_space(value), v))
}

/// Go: `findForwardedFor`.
fn find_forwarded_for<H, V>(r: &H, v: &V) -> Option<(Ip, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, X_FORWARDED_FOR).and_then(|value| {
        value
            .split(|&c| c == b',')
            .find_map(|p| valid(trim_space(p), v))
    })
}

/// Go: `findRealIP`.
fn find_real_ip<H, V>(r: &H, v: &V) -> Option<(Ip, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, X_REAL_IP).and_then(|value| valid(trim_space(value), v))
}

/// Go: `findRemoteAddr`.
fn find_remote_addr<V: IpValidator + ?Sized>(remote_addr: &str, v: &V) -> Option<(Ip, IpVersion)> {
    let (host, _port) = iputil::net::split_host_port(remote_addr).ok()?;
    valid(host.as_bytes(), v)
}

/// The rune that a valid UTF-8 sequence starting `s` encodes, and its width; `None` for an
/// invalid sequence, which Go decodes as `RuneError` (never a space).
fn first_rune(s: &[u8]) -> Option<(char, usize)> {
    let &lead = s.first()?;
    let width = match lead {
        0x00..=0x7F => return Some((char::from(lead), 1)),
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let text = str::from_utf8(s.get(..width)?).ok()?;
    text.chars().next().map(|c| (c, width))
}

/// The rune that a valid UTF-8 sequence ending `s` encodes, and its width (Go:
/// `utf8.DecodeLastRune`); `None` when Go would decode `RuneError`.
fn last_rune(s: &[u8]) -> Option<(char, usize)> {
    let &last = s.last()?;
    if last < 0x80 {
        return Some((char::from(last), 1));
    }
    (2..=4).find_map(|width| {
        let start = s.len().checked_sub(width)?;
        let text = str::from_utf8(s.get(start..)?).ok()?;
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Some((c, width)),
            _ => None,
        }
    })
}

/// Go: `strings.TrimSpace` on a byte string: leading and trailing `unicode.IsSpace` runes
/// removed; invalid UTF-8 is never trimmed.
fn trim_space(s: &[u8]) -> &[u8] {
    let mut start = 0_usize;
    while let Some((c, w)) = s.get(start..).and_then(first_rune) {
        if !unicode::is_space(c) {
            break;
        }
        start += w;
    }
    let mut end = s.len();
    while end > start {
        match s.get(start..end).and_then(last_rune) {
            Some((c, w)) if unicode::is_space(c) => end -= w,
            _ => break,
        }
    }
    s.get(start..end).unwrap_or_default()
}

#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
