//! Go `net`: the `IP`, `IPMask` and `IPNet` semantics Prebid relies on, with `ParseIP`,
//! `ParseCIDR`, `CIDRMask` and `SplitHostPort`.
//!
//! Go: `net/ip.go`, `net/ipsock.go`, `net/parse.go` and `net/net.go` (Go 1.26).
//!
//! A Go `net.IP` is a byte slice whose length decides its family: 4 bytes are IPv4, 16 bytes
//! are IPv6 or IPv4-mapped IPv6 (`::ffff:a.b.c.d`), and `nil` is no address. [`Ip`] holds the
//! 4- and 16-byte forms, and `Option<Ip>` stands for a slice that may be nil. [`parse_ip`]
//! returns the 16-byte form for every address, as Go does, while [`Ip::to4`] and masking with a
//! 4-byte mask produce the 4-byte form. Go slices of other lengths only arise from hand-built
//! values and are not modelled; their `String` (`?` followed by hex) never appears in Prebid.

use std::{error, fmt, net::SocketAddr};

use super::netip::{self, Parsed};

/// Go: `net.IPv4len`.
pub const IPV4_LEN: usize = 4;
/// Go: `net.IPv6len`.
pub const IPV6_LEN: usize = 16;

/// Go: `v4InV6Prefix`.
const V4_IN_V6_PREFIX: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF];

/// The bytes of a 4- or 16-byte value, by length.
#[derive(Clone, Copy)]
enum Form<'a> {
    Four(&'a [u8; 4]),
    Sixteen(&'a [u8; 16]),
}

/// 4 or 16 bytes, the 4-byte form kept in the first four with the rest zero.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Bytes {
    octets: [u8; 16],
    four: bool,
}

impl Bytes {
    const fn four(a: [u8; 4]) -> Self {
        let [a, b, c, d] = a;
        Self {
            octets: [a, b, c, d, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            four: true,
        }
    }

    const fn sixteen(octets: [u8; 16]) -> Self {
        Self {
            octets,
            four: false,
        }
    }

    fn from_slice(b: &[u8]) -> Option<Self> {
        match b.len() {
            IPV4_LEN => b.first_chunk::<4>().map(|a| Self::four(*a)),
            IPV6_LEN => b.first_chunk::<16>().map(|a| Self::sixteen(*a)),
            _ => None,
        }
    }

    const fn form(&self) -> Form<'_> {
        match self.octets.first_chunk::<4>() {
            Some(a) if self.four => Form::Four(a),
            _ => Form::Sixteen(&self.octets),
        }
    }

    const fn as_slice(&self) -> &[u8] {
        match self.form() {
            Form::Four(a) => a,
            Form::Sixteen(a) => a,
        }
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_hex(f, self.as_slice())
    }
}

/// Go: a non-nil `net.IP` of 4 or 16 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ip(Bytes);

/// Go: a non-nil `net.IPMask` of 4 or 16 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IpMask(Bytes);

/// Go: `net.IPNet`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IpNet {
    /// The network number (Go: `IP`).
    pub ip: Ip,
    /// The network mask (Go: `Mask`).
    pub mask: IpMask,
}

/// Go: `*net.ParseError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The kind of string expected, such as `CIDR address` (Go: `Type`).
    pub kind: &'static str,
    /// The malformed text (Go: `Text`).
    pub text: String,
}

impl fmt::Display for ParseError {
    /// Go: `invalid <Type>: <Text>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {}: {}", self.kind, self.text)
    }
}

impl error::Error for ParseError {}

/// Go: `*net.AddrError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrError {
    /// What is wrong (Go: `Err`).
    pub err: &'static str,
    /// The address (Go: `Addr`).
    pub addr: String,
}

impl fmt::Display for AddrError {
    /// Go: `address <Addr>: <Err>`, or `<Err>` alone when the address is empty.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.addr.is_empty() {
            f.write_str(self.err)
        } else {
            write!(f, "address {}: {}", self.addr, self.err)
        }
    }
}

impl error::Error for AddrError {}

/// Writes bytes as lowercase hex without punctuation (Go: `hexString`).
fn fmt_hex(f: &mut fmt::Formatter<'_>, b: &[u8]) -> fmt::Result {
    b.iter().try_for_each(|x| write!(f, "{x:02x}"))
}

/// Whether every byte is `0xff` (Go: `allFF`).
fn all_ff(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0xFF)
}

impl Ip {
    /// The 4-byte `net.IP{a, b, c, d}`.
    #[must_use]
    pub const fn from4(a: [u8; 4]) -> Self {
        Self(Bytes::four(a))
    }

    /// A 16-byte `net.IP`.
    #[must_use]
    pub const fn from16(a: [u8; 16]) -> Self {
        Self(Bytes::sixteen(a))
    }

    /// Go: `net.IPv4(a, b, c, d)`: the 16-byte IPv4-mapped form.
    #[must_use]
    pub const fn ipv4(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self::from16(netip::v4_in_v6([a, b, c, d]))
    }

    /// The `net.IP` of a 4- or 16-byte slice; `None` for any other length.
    #[must_use]
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        Bytes::from_slice(b).map(Self)
    }

    /// The address bytes, 4 or 16 of them (Go: the slice itself).
    #[must_use]
    pub const fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Whether this is the 4-byte form (Go: `len(ip) == net.IPv4len`).
    #[must_use]
    pub const fn is_4_byte(&self) -> bool {
        self.0.four
    }

    /// Go: `IP.To4`: the 4-byte form of an IPv4 or IPv4-mapped address, `None` otherwise.
    #[must_use]
    pub fn to4(&self) -> Option<Self> {
        match self.0.form() {
            Form::Four(_) => Some(*self),
            Form::Sixteen(a) => netip::is_4in6(a).then(|| Self::from4(netip::last4(a))),
        }
    }

    /// Go: `IP.To16`: the 16-byte form.
    #[must_use]
    pub const fn to16(&self) -> Self {
        match self.0.form() {
            Form::Four(a) => Self::from16(netip::v4_in_v6(*a)),
            Form::Sixteen(_) => *self,
        }
    }

    /// Go: `IP.Mask(mask)`; `None` (Go's nil) when the lengths do not match.
    ///
    /// A 16-byte mask whose first 12 bytes are all ones applies to a 4-byte address, and a
    /// 4-byte mask applies to the last four bytes of an IPv4-mapped address.
    #[must_use]
    pub fn mask(&self, mask: &IpMask) -> Option<Self> {
        let mut m = mask.as_slice();
        let mut ip = self.as_slice();
        if m.len() == IPV6_LEN && ip.len() == IPV4_LEN && m.get(..12).is_some_and(all_ff) {
            m = m.get(12..).unwrap_or_default();
        }
        if m.len() == IPV4_LEN && ip.len() == IPV6_LEN && ip.get(..12) == Some(&V4_IN_V6_PREFIX[..])
        {
            ip = ip.get(12..).unwrap_or_default();
        }
        if ip.len() != m.len() {
            return None;
        }
        // privacy.scrubIP masks every request's device IPs: no heap allocation.
        let mut out = [0_u8; IPV6_LEN];
        for (o, (x, y)) in out.iter_mut().zip(ip.iter().zip(m)) {
            *o = x & y;
        }
        Self::from_slice(out.get(..ip.len())?)
    }

    /// Go: `IP.Equal`: the same address, an IPv4 address being equal to its IPv4-mapped form.
    #[must_use]
    pub fn equal(&self, x: &Self) -> bool {
        if self.is_4_byte() == x.is_4_byte() {
            return self == x;
        }
        let (four, sixteen) = if self.is_4_byte() {
            (self, x)
        } else {
            (x, self)
        };
        let sixteen = sixteen.as_slice();
        sixteen.get(..12) == Some(&V4_IN_V6_PREFIX[..])
            && sixteen.get(12..) == Some(four.as_slice())
    }
}

impl fmt::Display for Ip {
    /// Go: `IP.String`: dotted decimal for IPv4 and IPv4-mapped addresses, RFC 5952 text
    /// otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.form() {
            Form::Four(a) => netip::fmt_v4(f, *a),
            Form::Sixteen(a) => {
                if netip::is_4in6(a) {
                    netip::fmt_v4(f, netip::last4(a))
                } else {
                    netip::fmt_v6(f, a)
                }
            }
        }
    }
}

/// Go: `IP.String` of a possibly nil `net.IP`: `<nil>` for nil.
#[must_use]
pub fn ip_string(ip: Option<&Ip>) -> String {
    ip.map_or_else(|| "<nil>".to_owned(), Ip::to_string)
}

/// Go: `ip.Mask(mask)` where either may be nil.
///
/// `privacy.scrubIP` calls it on the results of `net.ParseIP` and `net.CIDRMask`. A nil
/// address or mask gives a nil result (or, for both, an empty one), which prints as `<nil>`.
#[must_use]
pub fn mask_ip(ip: Option<&Ip>, mask: Option<&IpMask>) -> Option<Ip> {
    ip.zip(mask).and_then(|(ip, mask)| ip.mask(mask))
}

/// Go: `net.ParseIP`: the 16-byte form of an IPv4, IPv6 or IPv4-mapped IPv6 address, or
/// `None` for anything else, including an address with a zone.
#[must_use]
pub fn parse_ip(s: &str) -> Option<Ip> {
    parse_ip_bytes(s.as_bytes())
}

/// Go: `net.ParseIP` of a byte string.
#[must_use]
pub fn parse_ip_bytes(s: &[u8]) -> Option<Ip> {
    match netip::parse(s) {
        Ok(addr) if !addr.has_zone() => Some(Ip::from16(addr.as16())),
        _ => None,
    }
}

impl IpMask {
    /// The 4-byte `net.IPMask{a, b, c, d}` (Go: `net.IPv4Mask(a, b, c, d)`).
    #[must_use]
    pub const fn from4(a: [u8; 4]) -> Self {
        Self(Bytes::four(a))
    }

    /// A 16-byte `net.IPMask`.
    #[must_use]
    pub const fn from16(a: [u8; 16]) -> Self {
        Self(Bytes::sixteen(a))
    }

    /// The `net.IPMask` of a 4- or 16-byte slice; `None` for any other length.
    #[must_use]
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        Bytes::from_slice(b).map(Self)
    }

    /// The mask bytes, 4 or 16 of them.
    #[must_use]
    pub const fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Go: `IPMask.Size`: the number of leading ones and the total number of bits, or `(0, 0)`
    /// when the mask is not ones followed by zeros.
    #[must_use]
    pub fn size(&self) -> (i64, i64) {
        let bits = if self.0.four { 32 } else { 128 };
        simple_mask_length(self.as_slice()).map_or((0, 0), |ones| (ones, bits))
    }
}

impl fmt::Display for IpMask {
    /// Go: `IPMask.String`: hex without punctuation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_hex(f, self.as_slice())
    }
}

/// Go: `net.IPv4Mask(a, b, c, d)`.
#[must_use]
pub const fn ipv4_mask(a: u8, b: u8, c: u8, d: u8) -> IpMask {
    IpMask::from4([a, b, c, d])
}

/// Go: `net.CIDRMask(ones, bits)`: `ones` one bits followed by zeros, for `bits` 32 or 128;
/// `None` (Go's nil) for any other `bits` or an `ones` outside `0..=bits`.
#[must_use]
pub fn cidr_mask(ones: i64, bits: i64) -> Option<IpMask> {
    if bits != 32 && bits != 128 || !(0..=bits).contains(&ones) {
        return None;
    }
    let mut m = [0_u8; 16];
    let mut n = u32::try_from(ones).ok()?;
    for b in &mut m {
        if n >= 8 {
            *b = 0xFF;
            n -= 8;
        } else {
            *b = !(0xFF_u8 >> n);
            n = 0;
        }
    }
    if bits == 32 {
        Some(IpMask::from4(netip::first4(&m)))
    } else {
        Some(IpMask::from16(m))
    }
}

/// Go: `simpleMaskLength`: the number of leading ones when the mask is ones followed by zeros.
fn simple_mask_length(mask: &[u8]) -> Option<i64> {
    let mut n = 0_i64;
    let mut bytes = mask.iter();
    while let Some(&v) = bytes.next() {
        if v == 0xFF {
            n += 8;
            continue;
        }
        // Found a non-ff byte: count its one bits; the rest must be zero bits.
        let ones = v.leading_ones();
        if v.checked_shl(ones).unwrap_or(0) != 0 {
            return None;
        }
        n += i64::from(ones);
        if bytes.any(|&b| b != 0) {
            return None;
        }
        break;
    }
    Some(n)
}

impl IpNet {
    /// Go: `networkNumberAndMask`: the network number in its 4-byte form when it has one, and
    /// a mask of the same length; `None` when the lengths cannot agree.
    fn network_number_and_mask(&self) -> Option<(Ip, IpMask)> {
        let ip = self.ip.to4().unwrap_or(self.ip);
        match (ip.is_4_byte(), self.mask.0.four) {
            (true, true) | (false, false) => Some((ip, self.mask)),
            (true, false) => Some((ip, IpMask::from4(netip::last4(&self.mask.0.octets)))),
            (false, true) => None,
        }
    }

    /// Go: `IPNet.Contains`. An IPv4-mapped address is compared in its 4-byte form, so it is
    /// never inside a network whose number is a 16-byte IPv6 address.
    #[must_use]
    pub fn contains(&self, ip: &Ip) -> bool {
        let Some((nn, m)) = self.network_number_and_mask() else {
            return false;
        };
        let ip = ip.to4().unwrap_or(*ip);
        if ip.is_4_byte() != nn.is_4_byte() {
            return false;
        }
        nn.as_slice()
            .iter()
            .zip(m.as_slice())
            .zip(ip.as_slice())
            .all(|((n, m), x)| n & m == x & m)
    }
}

impl fmt::Display for IpNet {
    /// Go: `IPNet.String`: `192.0.2.0/24`, a hex mask such as `198.51.100.0/c000ff00` when the
    /// mask is not canonical, or `<nil>` when the address and mask lengths disagree.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some((nn, m)) = self.network_number_and_mask() else {
            return f.write_str("<nil>");
        };
        match simple_mask_length(m.as_slice()) {
            Some(l) => write!(f, "{nn}/{l}"),
            None => write!(f, "{nn}/{m}"),
        }
    }
}

/// Go's `big`: the cap of `dtoi`.
const BIG: i64 = 0xFF_FFFF;

/// Go: `dtoi`: the decimal number at the start of `s`, the number of bytes read, and whether
/// any digit was read without reaching [`BIG`].
fn dtoi(s: &[u8]) -> (i64, usize, bool) {
    let mut n = 0_i64;
    let mut i = 0_usize;
    for &c in s {
        if !c.is_ascii_digit() {
            break;
        }
        n = n * 10 + i64::from(c - b'0');
        if n >= BIG {
            return (BIG, i, false);
        }
        i += 1;
    }
    if i == 0 {
        return (0, 0, false);
    }
    (n, i, true)
}

/// Go: `net.ParseCIDR`: the address and the network its prefix length implies.
///
/// An IPv4 network has a 4-byte `ip` and `mask`; an IPv6 network, IPv4-mapped included, has
/// 16. The address is always the 16-byte form.
///
/// # Errors
///
/// `invalid CIDR address: <s>` for anything but an address without a zone, a `/`, and a
/// decimal prefix length within the address size.
pub fn parse_cidr(s: &str) -> Result<(Ip, IpNet), ParseError> {
    let err = || ParseError {
        kind: "CIDR address",
        text: s.to_owned(),
    };
    let bytes = s.as_bytes();
    let slash = bytes.iter().position(|&c| c == b'/').ok_or_else(err)?;
    let addr = bytes.get(..slash).unwrap_or_default();
    let mask = bytes.get(slash + 1..).unwrap_or_default();
    let ip_addr: Parsed<'_> = match netip::parse(addr) {
        Ok(a) if !a.has_zone() => a,
        _ => return Err(err()),
    };
    let bits = ip_addr.bit_len();
    let (n, i, ok) = dtoi(mask);
    if !ok || i != mask.len() || n < 0 || n > bits {
        return Err(err());
    }
    let m = cidr_mask(n, bits).ok_or_else(err)?;
    let ip = Ip::from16(ip_addr.as16());
    let net_ip = ip.mask(&m).ok_or_else(err)?;
    Ok((
        ip,
        IpNet {
            ip: net_ip,
            mask: m,
        },
    ))
}

/// Go: `net.JoinHostPort`: `host:port`, or `[host]:port` when the host contains a colon.
#[must_use]
pub fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Go: `(*net.TCPAddr).String()`, which Go's HTTP server stores in `Request.RemoteAddr` for the
/// peer of an accepted connection.
///
/// The address prints as [`Ip`] does, so the peer of a dual-stack listener that connected over
/// IPv4 (an IPv4-mapped `SocketAddr::V6`) is `1.2.3.4:5678`, never `[::ffff:1.2.3.4]:5678` as
/// `SocketAddr`'s `Display` writes it. That difference reaches
/// [`find_ip`](crate::httputil::find_ip): the bracketed text contains `:`, so the remote
/// address would be IPv6 and set `device.ipv6` where Go sets `device.ip`.
///
/// A non-zero scope id prints as its number. Go prints the name of the interface with that
/// index when there is one (`[fe80::1%eth0]:80`); either way the text has a zone, which
/// `net.ParseIP` rejects, so `FindIP` takes no address from it.
#[must_use]
pub fn tcp_addr_string(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(a) => join_host_port(
            &Ip::from4(a.ip().octets()).to_string(),
            &a.port().to_string(),
        ),
        SocketAddr::V6(a) => {
            let mut host = Ip::from16(a.ip().octets()).to_string();
            if a.scope_id() != 0 {
                host.push('%');
                host.push_str(&a.scope_id().to_string());
            }
            join_host_port(&host, &a.port().to_string())
        }
    }
}

/// Go: `net.SplitHostPort`: splits `host:port`, `[host]:port` or `[host%zone]:port` into the
/// host (without brackets) and the port.
///
/// # Errors
///
/// Go's `*net.AddrError`, such as `address 1.2.3.4: missing port in address`.
pub fn split_host_port(hostport: &str) -> Result<(&str, &str), AddrError> {
    const MISSING_PORT: &str = "missing port in address";
    const TOO_MANY_COLONS: &str = "too many colons in address";
    let addr_err = |why: &'static str| AddrError {
        err: why,
        addr: hostport.to_owned(),
    };
    let b = hostport.as_bytes();
    let (mut j, mut k) = (0_usize, 0_usize);

    // The port starts after the last colon.
    let i = b
        .iter()
        .rposition(|&c| c == b':')
        .ok_or_else(|| addr_err(MISSING_PORT))?;

    let host = if b.first() == Some(&b'[') {
        // Expect the first ']' just before the last ':'.
        let end = b
            .iter()
            .position(|&c| c == b']')
            .ok_or_else(|| addr_err("missing ']' in address"))?;
        if end + 1 == b.len() {
            // There can't be a ':' behind the ']' now.
            return Err(addr_err(MISSING_PORT));
        } else if end + 1 != i {
            // Either ']' isn't followed by a colon, or it is followed by a colon that is not
            // the last one.
            if b.get(end + 1) == Some(&b':') {
                return Err(addr_err(TOO_MANY_COLONS));
            }
            return Err(addr_err(MISSING_PORT));
        }
        // There can't be a '[' resp. ']' before these positions.
        (j, k) = (1, end + 1);
        hostport.get(1..end).unwrap_or_default()
    } else {
        let host = hostport.get(..i).unwrap_or_default();
        if host.contains(':') {
            return Err(addr_err(TOO_MANY_COLONS));
        }
        host
    };
    if b.get(j..).is_some_and(|rest| rest.contains(&b'[')) {
        return Err(addr_err("unexpected '[' in address"));
    }
    if b.get(k..).is_some_and(|rest| rest.contains(&b']')) {
        return Err(addr_err("unexpected ']' in address"));
    }
    Ok((host, hostport.get(i + 1..).unwrap_or_default()))
}
