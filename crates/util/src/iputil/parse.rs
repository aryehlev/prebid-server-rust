//! Go: `util/iputil/parse.go`.

use std::net::IpAddr;

use super::net;

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

/// Go: `iputil.ParseIP`: the address in canonical form (see [`net::parse_ip`]) and its
/// version, or `None` (Go: `nil, IPvUnknown`) when `v` is not an address.
///
/// The version is [`IpVersion::V6`] whenever the text contains `:`, so `::ffff:1.2.3.4` is
/// IPv6 although the address is the IPv4 address it maps.
#[must_use]
pub fn parse_ip(v: &str) -> Option<(IpAddr, IpVersion)> {
    let ip = net::parse_ip(v)?;
    let ver = if v.contains(':') {
        IpVersion::V6
    } else {
        IpVersion::V4
    };
    Some((ip, ver))
}
