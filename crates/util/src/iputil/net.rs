//! Glue (Rule B (a)): Go's `net` semantics on std's addresses and ipnet's networks.
//!
//! Prebid uses `net` three ways: the private networks of `request_validation` (Go:
//! `net.ParseCIDR`), whether a request's address is in one of them (Go: `IPNet.Contains`), and
//! the masking of `device.ip` (Go: `IP.Mask` in `privacy.scrubIP`). Real traffic sends IPv4
//! addresses in IPv4-mapped form (`::ffff:10.1.2.3`, from dual-stack proxies) and operators
//! may write prefixes with leading zeros (`10.0.0.0/008`); ipnet calls such an address outside
//! every IPv4 network and rejects such a prefix, where Go reads both (the ip.parse-edges
//! ledger entry).
//!
//! Addresses are parsed by std (Go: `net.ParseIP`, which std matches on every address form,
//! leading zeros and zones included) and kept in canonical form: Go's 16-byte `net.IP` makes
//! no difference between `1.2.3.4` and `::ffff:1.2.3.4`, so an IPv4-mapped address is the
//! [`IpAddr::V4`] it maps. Networks are ipnet's [`IpNet`], which keeps the family of the text:
//! `::ffff:10.0.0.0/104` is an IPv6 network, as its 16-byte `IPNet.IP` makes it in Go.

use std::{
    error, fmt,
    net::{IpAddr, Ipv6Addr},
};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

/// Go: `net.ParseIP`: the address `s` spells, in canonical form, or `None` (Go: `nil`).
#[must_use]
pub fn parse_ip(s: &str) -> Option<IpAddr> {
    s.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

/// Go: `ip.String()` of a `net.IP` that may be nil: `<nil>` for `None`, dotted decimal for an
/// IPv4 address and RFC 5952 text for an IPv6 one.
#[must_use]
pub fn ip_string(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| "<nil>".to_owned(), |ip| ip.to_canonical().to_string())
}

/// Go: `ip.Mask(net.CIDRMask(ones, bits))` of an address from `net.ParseIP`, as
/// `privacy.scrubIP` masks `device.ip` and `device.ipv6`.
///
/// A 32-bit mask applies to an IPv4 address only; a 128-bit mask applies to the 16 bytes of
/// any address, an IPv4 one in its mapped form, so that only a prefix of 96 bits or more keeps
/// anything of it. `None` (Go: `nil`, which prints as `<nil>`) when the mask does not apply or
/// `ones` is outside `[0, bits]`, and for any `bits` other than 32 and 128.
#[must_use]
pub fn mask(ip: IpAddr, ones: i64, bits: i64) -> Option<IpAddr> {
    let prefix = u8::try_from(ones).ok()?;
    match (bits, ip.to_canonical()) {
        (32, IpAddr::V4(a)) => Ipv4Net::new(a, prefix)
            .ok()
            .map(|n| IpAddr::V4(n.network())),
        (128, canonical) => Ipv6Net::new(as_ipv6(canonical), prefix)
            .ok()
            .map(|n| IpAddr::V6(n.network()).to_canonical()),
        _ => None,
    }
}

/// The 16-byte form of an address (Go: `IP.To16`).
const fn as_ipv6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(a) => a.to_ipv6_mapped(),
        IpAddr::V6(a) => a,
    }
}

/// Go: `n.Contains(ip)`.
///
/// An IPv4 network contains an IPv4 address in either spelling. An IPv6 network whose address
/// is IPv4-mapped (`::ffff:10.0.0.0/104`) contains the IPv4 addresses it maps, in either
/// spelling, and no other address; any other IPv6 network contains no IPv4 address, not even
/// `::/0` or `::ffff:0:0/80`, because Go compares the 4-byte form of such an address.
#[must_use]
pub fn contains(n: &IpNet, ip: IpAddr) -> bool {
    match (n, ip.to_canonical()) {
        (IpNet::V4(n), IpAddr::V4(a)) => n.contains(&a),
        (IpNet::V6(n), IpAddr::V4(a)) => {
            n.addr().to_ipv4_mapped().is_some() && n.contains(&a.to_ipv6_mapped())
        }
        (IpNet::V6(n), IpAddr::V6(a)) => n.addr().to_ipv4_mapped().is_none() && n.contains(&a),
        (IpNet::V4(_), IpAddr::V6(_)) => false,
    }
}

/// The error of [`parse_cidr`] (Go: `*net.ParseError` of type `CIDR address`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseCidrError(String);

impl fmt::Display for ParseCidrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid CIDR address: {}", self.0)
    }
}

impl error::Error for ParseCidrError {}

/// Go: `net.ParseCIDR`: the address of `addr/prefix` and its network, the address cut to the
/// prefix.
///
/// The address is parsed as by [`parse_ip`], without a zone; the prefix is decimal digits with
/// any number of leading zeros, at most 32 for an IPv4 address and 128 for an IPv6 one. The
/// network has the family of the text, so `::ffff:10.0.0.0/104` is [`IpNet::V6`], and
/// `request_validation` rejects it as an IPv4 network (Go: `len(ipNet.IP) != net.IPv4len`).
///
/// # Errors
///
/// When `s` has no `/`, the address does not parse or the prefix is not such a number.
pub fn parse_cidr(s: &str) -> Result<(IpAddr, IpNet), ParseCidrError> {
    let invalid = || ParseCidrError(s.to_owned());
    let (addr, prefix) = s.split_once('/').ok_or_else(invalid)?;
    let ip = addr.parse::<IpAddr>().ok().ok_or_else(invalid)?;
    let prefix = prefix_len(prefix).ok_or_else(invalid)?;
    let n = IpNet::new(ip, prefix).ok().ok_or_else(invalid)?;
    Ok((ip.to_canonical(), n.trunc()))
}

/// Go: `dtoi` on a CIDR prefix: one or more decimal digits, all of them.
fn prefix_len(s: &str) -> Option<u8> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = s.trim_start_matches('0');
    if digits.is_empty() {
        Some(0)
    } else {
        digits.parse().ok()
    }
}
