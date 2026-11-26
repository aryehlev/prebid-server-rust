//! Go `util/httputil`: client IP discovery, the `Content-Encoding` names and the 1x1 tracking
//! pixel.
//!
//! ```
//! use std::net::SocketAddr;
//!
//! use pbs_util::{
//!     httputil::{RequestHeaders, find_ip},
//!     iputil::{IpVersion, PublicNetworkIpValidator, net::parse_cidr},
//! };
//!
//! /// One `X-Forwarded-For` header.
//! struct Forwarded(&'static str);
//!
//! impl RequestHeaders for Forwarded {
//!     fn first_value(&self, name: &str) -> Option<&[u8]> {
//!         name.eq_ignore_ascii_case("X-Forwarded-For")
//!             .then_some(self.0.as_bytes())
//!     }
//! }
//!
//! let validator = PublicNetworkIpValidator {
//!     ipv4_private_networks: vec![parse_cidr("10.0.0.0/8")?.1],
//!     ipv6_private_networks: Vec::new(),
//! };
//! let headers = Forwarded(" 10.0.0.1, 203.0.113.7 , 8.8.8.8");
//! let peer: SocketAddr = "10.1.1.1:5678".parse()?;
//! // The first public address of the list wins; private and malformed ones are skipped.
//! let (ip, ver) = find_ip(&headers, Some(&peer), &validator).ok_or("no IP")?;
//! assert_eq!(
//!     (ip.to_string(), ver),
//!     ("203.0.113.7".to_owned(), IpVersion::V4)
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod pixel;

use std::{
    net::{IpAddr, SocketAddr},
    str,
};

pub use pixel::{PIXEL_1X1_PNG, Pixel};

use crate::iputil::{self, IpValidator, IpVersion};

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

    /// Go: `ContentEncoding.Normalize`: the value lower-cased.
    ///
    /// It is std's `to_lowercase`, which lower-cases U+0130 LATIN CAPITAL LETTER I WITH DOT
    /// ABOVE to `i` and U+0307 where Go's `strings.ToLower` writes `i` alone (the
    /// gzip.content-encoding-lower ledger entry).
    #[must_use]
    pub fn normalize(&self) -> Self {
        Self(self.0.to_lowercase())
    }

    /// The header value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Go: `contentEncoding.Normalize() == httputil.ContentEncodingGZIP`.
    #[must_use]
    pub fn is_gzip(&self) -> bool {
        self.normalize().0 == Self::GZIP
    }
}

/// The request headers [`find_ip`] reads.
///
/// Go reads them with `r.Header.Get`, which returns the first value of the header. Header
/// values are bytes, as they arrive.
pub trait RequestHeaders {
    /// The first value of the header named `name` (one of [`TRUE_CLIENT_IP`],
    /// [`X_FORWARDED_FOR`] and [`X_REAL_IP`], compared without regard to ASCII case), or `None`
    /// when the request has no such header.
    fn first_value(&self, name: &str) -> Option<&[u8]>;
}

impl<T: RequestHeaders + ?Sized> RequestHeaders for &T {
    fn first_value(&self, name: &str) -> Option<&[u8]> {
        (**self).first_value(name)
    }
}

/// Go: `httputil.FindIP`: the first address accepted by `v`, looked up in this order:
///
/// 1. `True-Client-IP`, trimmed of white space;
/// 2. each comma-separated element of `X-Forwarded-For`, trimmed, from left to right;
/// 3. `X-Real-IP`, trimmed;
/// 4. the address of the peer (Go: the host of `r.RemoteAddr`).
///
/// Only the first value of each header is read. The version of a header's address is that of
/// [`iputil::parse_ip`]: IPv6 whenever the text contains `:`. The peer's is that of the text
/// Go's server writes for it: IPv4 for the IPv4-mapped peer of a dual-stack listener. A peer
/// with a scope (Go: a zone in its text) gives no address, and neither does a Unix socket
/// peer (`None`), whose Go text has no port.
#[must_use]
pub fn find_ip<H, V>(r: &H, remote: Option<&SocketAddr>, v: &V) -> Option<(IpAddr, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    find_true_client_ip(r, v)
        .or_else(|| find_forwarded_for(r, v))
        .or_else(|| find_real_ip(r, v))
        .or_else(|| find_remote_addr(remote, v))
}

/// Go: `r.Header.Get(key)` with Go's empty-string check.
fn header<'a, H: RequestHeaders + ?Sized>(r: &'a H, key: &str) -> Option<&'a [u8]> {
    r.first_value(key).filter(|value| !value.is_empty())
}

/// Go: `iputil.ParseIP(strings.TrimSpace(value))`, kept when the validator accepts it. A
/// value that is not UTF-8 is not an address after Go's trim either, which never removes an
/// invalid byte.
fn valid<V: IpValidator + ?Sized>(value: &[u8], v: &V) -> Option<(IpAddr, IpVersion)> {
    let value = str::from_utf8(value).ok()?;
    iputil::parse_ip(value.trim()).filter(|(ip, ver)| v.is_valid(ip, *ver))
}

/// Go: `findTrueClientIP`.
fn find_true_client_ip<H, V>(r: &H, v: &V) -> Option<(IpAddr, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, TRUE_CLIENT_IP).and_then(|value| valid(value, v))
}

/// Go: `findForwardedFor`.
fn find_forwarded_for<H, V>(r: &H, v: &V) -> Option<(IpAddr, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, X_FORWARDED_FOR)
        .and_then(|value| value.split(|&c| c == b',').find_map(|p| valid(p, v)))
}

/// Go: `findRealIP`.
fn find_real_ip<H, V>(r: &H, v: &V) -> Option<(IpAddr, IpVersion)>
where
    H: RequestHeaders + ?Sized,
    V: IpValidator + ?Sized,
{
    header(r, X_REAL_IP).and_then(|value| valid(value, v))
}

/// Go: `findRemoteAddr`.
fn find_remote_addr<V: IpValidator + ?Sized>(
    remote: Option<&SocketAddr>,
    v: &V,
) -> Option<(IpAddr, IpVersion)> {
    let ip = match remote? {
        SocketAddr::V4(a) => IpAddr::V4(*a.ip()),
        SocketAddr::V6(a) if a.scope_id() == 0 => IpAddr::V6(*a.ip()).to_canonical(),
        SocketAddr::V6(_) => return None,
    };
    let ver = if ip.is_ipv4() {
        IpVersion::V4
    } else {
        IpVersion::V6
    };
    v.is_valid(&ip, ver).then_some((ip, ver))
}

#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
