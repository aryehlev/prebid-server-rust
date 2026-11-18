//! Go: `util/iputil/parse.go`.

use super::net::{self, Ip};

/// Go: `iputil.IPVersion`, the numerical version of the IP address spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum IpVersion {
    /// Go: `IPvUnknown` (0).
    #[default]
    Unknown,
    /// Go: `IPv4` (4).
    V4,
    /// Go: `IPv6` (6).
    V6,
}

impl IpVersion {
    /// The Go integer value: 0, 4 or 6.
    #[must_use]
    pub const fn as_int(self) -> i64 {
        match self {
            Self::Unknown => 0,
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }
}

/// Go: `iputil.IPv4BitSize`.
pub const IPV4_BIT_SIZE: i64 = 32;
/// Go: `iputil.IPv6BitSize`.
pub const IPV6_BIT_SIZE: i64 = 128;
/// Go: `iputil.IPv4DefaultMaskingBitSize`.
pub const IPV4_DEFAULT_MASKING_BIT_SIZE: i64 = 24;
/// Go: `iputil.IPv6DefaultMaskingBitSize`.
pub const IPV6_DEFAULT_MASKING_BIT_SIZE: i64 = 56;

/// Go: `iputil.ParseIP`: the address (16-byte form, as `net.ParseIP` returns it) and its
/// version, or `None` (Go: `nil, IPvUnknown`) when `v` is not an address.
///
/// The version is [`IpVersion::V6`] whenever the text contains `:`, so `::ffff:1.2.3.4` is
/// IPv6 although `net.IP.String` prints it as `1.2.3.4`.
#[must_use]
pub fn parse_ip(v: &str) -> Option<(Ip, IpVersion)> {
    parse_ip_bytes(v.as_bytes())
}

/// Go: `iputil.ParseIP` of a byte string (a header value, which may not be UTF-8).
#[must_use]
pub fn parse_ip_bytes(v: &[u8]) -> Option<(Ip, IpVersion)> {
    let ip = net::parse_ip_bytes(v)?;
    // Go: strings.ContainsRune(v, ':').
    let ver = if v.contains(&b':') {
        IpVersion::V6
    } else {
        IpVersion::V4
    };
    Some((ip, ver))
}
