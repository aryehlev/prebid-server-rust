//! Go `net/netip`: `ParseAddr` and `Addr.String`, the parsing and formatting that `net.ParseIP`,
//! `net.ParseCIDR` and `net.IP.String` are built on.
//!
//! Go: `net/netip/netip.go` (Go 1.26). Parsing works on bytes because Go strings are byte
//! strings; error texts quote the input with `strconv.Quote`, so invalid UTF-8 prints as `\x..`.

use std::{error, fmt, ops::Range};

use pbs_gocompat::strconv;

/// Go: `netip.Addr`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum Addr {
    /// Go: the zero `Addr`, which prints as `invalid IP`.
    #[default]
    Invalid,
    /// An IPv4 address.
    V4([u8; 4]),
    /// An IPv6 address (IPv4-mapped included) and its zone, empty when it has none.
    V6 {
        /// The address bytes.
        octets: [u8; 16],
        /// The zone after `%`, as the bytes of the input; empty for none (Go: `z6noz`).
        zone: Vec<u8>,
    },
}

/// Go: `netip.parseAddrError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseAddrError {
    /// The input of `ParseAddr`.
    pub input: Vec<u8>,
    /// What failed.
    pub msg: &'static str,
    /// The unparsed rest of the input where the failure occurred, if Go reports it.
    pub at: Option<Vec<u8>>,
}

impl fmt::Display for ParseAddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ParseAddr({}): {}",
            strconv::quote_bytes(&self.input),
            self.msg
        )?;
        match &self.at {
            Some(at) if !at.is_empty() => write!(f, " (at {})", strconv::quote_bytes(at)),
            _ => Ok(()),
        }
    }
}

impl error::Error for ParseAddrError {}

/// A parse failure before its text is built: the message and the range of the input Go
/// reports as `at`. Hot paths (header IPs) only need to know that parsing failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AddrErr {
    msg: &'static str,
    at: Option<Range<usize>>,
}

impl AddrErr {
    const fn new(msg: &'static str) -> Self {
        Self { msg, at: None }
    }

    const fn at(msg: &'static str, at: Range<usize>) -> Self {
        Self { msg, at: Some(at) }
    }

    fn into_error(self, input: &[u8]) -> ParseAddrError {
        ParseAddrError {
            input: input.to_vec(),
            msg: self.msg,
            at: self.at.and_then(|r| input.get(r)).map(<[u8]>::to_vec),
        }
    }
}

/// A parsed address that borrows its zone from the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Parsed<'a> {
    V4([u8; 4]),
    V6([u8; 16], &'a [u8]),
}

impl Parsed<'_> {
    /// Go: `Addr.As16`.
    pub(crate) const fn as16(&self) -> [u8; 16] {
        match self {
            Self::V4(a) => v4_in_v6(*a),
            Self::V6(a, _) => *a,
        }
    }

    /// Go: `Addr.Zone() != ""`.
    pub(crate) const fn has_zone(&self) -> bool {
        match self {
            Self::V4(_) => false,
            Self::V6(_, zone) => !zone.is_empty(),
        }
    }

    /// Go: `Addr.BitLen`.
    pub(crate) const fn bit_len(&self) -> i64 {
        match self {
            Self::V4(_) => 32,
            Self::V6(..) => 128,
        }
    }
}

/// The IPv4-mapped IPv6 form of an IPv4 address (Go: `v4InV6Prefix` + the address).
pub(crate) const fn v4_in_v6(a: [u8; 4]) -> [u8; 16] {
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, a[0], a[1], a[2], a[3],
    ]
}

/// Go: `ParseAddr` without building the error text.
pub(crate) fn parse(input: &[u8]) -> Result<Parsed<'_>, AddrErr> {
    for &c in input {
        match c {
            b'.' => return parse_ipv4(input).map(Parsed::V4),
            b':' => return parse_ipv6(input),
            // Assume that this was trying to be an IPv6 address with a zone specifier, but
            // the address is missing.
            b'%' => return Err(AddrErr::new("missing IPv6 address")),
            _ => {}
        }
    }
    Err(AddrErr::new("unable to parse IP"))
}

/// Go: `netip.ParseAddr`.
///
/// # Errors
///
/// Go's `parseAddrError`, such as `ParseAddr("1.2.3"): IPv4 address too short`.
pub fn parse_addr(s: &str) -> Result<Addr, ParseAddrError> {
    parse_addr_bytes(s.as_bytes())
}

/// Go: `netip.ParseAddr` of a byte string.
///
/// # Errors
///
/// Go's `parseAddrError`.
pub fn parse_addr_bytes(s: &[u8]) -> Result<Addr, ParseAddrError> {
    match parse(s) {
        Ok(Parsed::V4(a)) => Ok(Addr::V4(a)),
        Ok(Parsed::V6(octets, zone)) => Ok(Addr::V6 {
            octets,
            zone: zone.to_vec(),
        }),
        Err(e) => Err(e.into_error(s)),
    }
}

/// Go: `parseIPv4Fields(in, off, end, fields)`.
fn parse_ipv4_fields(input: &[u8], off: usize, end: usize) -> Result<[u8; 4], AddrErr> {
    let s = input.get(off..end).unwrap_or_default();
    let mut fields = [0_u8; 4];
    let mut val: u16 = 0;
    let mut pos = 0_usize;
    // The number of digits in the current octet.
    let mut dig_len = 0_u8;
    let mut prev = None;
    for (i, &c) in s.iter().enumerate() {
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(AddrErr::new("IPv4 field has octet with leading zero"));
            }
            val = val * 10 + u16::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(AddrErr::new("IPv4 field has value >255"));
            }
        } else if c == b'.' {
            // .1.2.3
            // 1.2.3.
            // 1..2.3
            if i == 0 || i + 1 == s.len() || prev == Some(b'.') {
                return Err(AddrErr::at(
                    "IPv4 field must have at least one digit",
                    off + i..end,
                ));
            }
            // 1.2.3.4.5
            if pos == 3 {
                return Err(AddrErr::new("IPv4 address too long"));
            }
            if let Some(field) = fields.get_mut(pos) {
                *field = u8::try_from(val).unwrap_or(u8::MAX);
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(AddrErr::at("unexpected character", off + i..end));
        }
        prev = Some(c);
    }
    if pos < 3 {
        return Err(AddrErr::new("IPv4 address too short"));
    }
    fields[3] = u8::try_from(val).unwrap_or(u8::MAX);
    Ok(fields)
}

/// Go: `parseIPv4`.
fn parse_ipv4(s: &[u8]) -> Result<[u8; 4], AddrErr> {
    parse_ipv4_fields(s, 0, s.len())
}

/// The value of a hex digit.
const fn hex_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// The address part of an IPv6 input: the bytes before the zone.
#[derive(Clone, Copy)]
struct V6Text<'a> {
    input: &'a [u8],
    /// Where the address ends: the `%` of the zone, or the end of the input.
    end: usize,
}

impl V6Text<'_> {
    /// The byte at `p`, if it is part of the address.
    fn at(self, p: usize) -> Option<u8> {
        if p < self.end {
            self.input.get(p).copied()
        } else {
            None
        }
    }

    /// Go: the hex number loop of `parseIPv6`: the value of the group at `p` and its digits.
    fn hex_group(self, p: usize) -> Result<(u32, usize), AddrErr> {
        let mut off = 0_usize;
        let mut acc = 0_u32;
        while let Some(v) = self.at(p + off).and_then(hex_value) {
            acc = (acc << 4) + v;
            if off > 3 {
                // More than 4 digits in group, fail.
                return Err(AddrErr::at(
                    "each group must have 4 or less digits",
                    p..self.end,
                ));
            }
            if acc > u32::from(u16::MAX) {
                return Err(AddrErr::at("IPv6 field has value >=2^16", p..self.end));
            }
            off += 1;
        }
        if off == 0 {
            // No digits found, fail.
            return Err(AddrErr::at(
                "each colon-separated field must have at least one digit",
                p..self.end,
            ));
        }
        Ok((acc, off))
    }
}

/// Go: the start of `parseIPv6`: the end of the address and the zone after `%`.
fn split_zone(input: &[u8]) -> Result<(usize, &[u8]), AddrErr> {
    let Some(i) = input.iter().position(|&c| c == b'%') else {
        return Ok((input.len(), &[]));
    };
    let zone = input.get(i + 1..).unwrap_or_default();
    if zone.is_empty() {
        // Not allowed to have an empty zone if explicitly specified.
        return Err(AddrErr::new("zone must be a non-empty string"));
    }
    Ok((i, zone))
}

/// Go: the end of `parseIPv6`: moves the `i` parsed bytes after the ellipsis to the end.
fn expand_ellipsis(ip: [u8; 16], i: usize, ellipsis: Option<usize>) -> Result<[u8; 16], AddrErr> {
    if i >= 16 {
        if ellipsis.is_some() {
            // Ellipsis must represent at least one 0 group.
            return Err(AddrErr::new(
                "the :: must expand to at least one field of zeros",
            ));
        }
        return Ok(ip);
    }
    let Some(e) = ellipsis else {
        return Err(AddrErr::new("address string too short"));
    };
    let n = 16 - i;
    let mut out = [0_u8; 16];
    for (dst, src) in out.iter_mut().zip(ip.iter().take(e)) {
        *dst = *src;
    }
    for (dst, src) in out
        .iter_mut()
        .skip(e + n)
        .zip(ip.iter().skip(e).take(i - e))
    {
        *dst = *src;
    }
    Ok(out)
}

/// Go: `parseIPv6`.
fn parse_ipv6(input: &[u8]) -> Result<Parsed<'_>, AddrErr> {
    let (end, zone) = split_zone(input)?;
    let text = V6Text { input, end };

    let mut ip = [0_u8; 16];
    // The position of the ellipsis in ip.
    let mut ellipsis: Option<usize> = None;
    // The offset of Go's `s` in the input.
    let mut p = 0_usize;

    // Might have leading ellipsis.
    if text.at(0) == Some(b':') && text.at(1) == Some(b':') {
        ellipsis = Some(0);
        p = 2;
        // Might be only ellipsis.
        if p == end {
            return Ok(Parsed::V6(ip, zone));
        }
    }

    // Loop, parsing hex numbers followed by colon.
    let mut i = 0_usize;
    while i < 16 {
        let (acc, off) = text.hex_group(p)?;

        // If followed by dot, might be in trailing IPv4.
        if text.at(p + off) == Some(b'.') {
            if ellipsis.is_none() && i != 12 {
                // Not the right place.
                return Err(AddrErr::at(
                    "embedded IPv4 address must replace the final 2 fields of the address",
                    p..end,
                ));
            }
            if i + 4 > 16 {
                // Not enough room.
                return Err(AddrErr::at(
                    "too many hex fields to fit an embedded IPv4 at the end of the address",
                    p..end,
                ));
            }
            let fields = parse_ipv4_fields(input, p, end)?;
            for (dst, src) in ip.iter_mut().skip(i).zip(fields) {
                *dst = src;
            }
            p = end;
            i += 4;
            break;
        }

        // Save this 16-bit chunk.
        let [_, _, hi, lo] = acc.to_be_bytes();
        if let Some(dst) = ip.get_mut(i..i + 2) {
            dst.copy_from_slice(&[hi, lo]);
        }
        i += 2;

        // Stop at end of string.
        p += off;
        if p == end {
            break;
        }

        // Otherwise must be followed by colon and more.
        if text.at(p) != Some(b':') {
            return Err(AddrErr::at("unexpected character, want colon", p..end));
        } else if p + 1 == end {
            return Err(AddrErr::at(
                "colon must be followed by more characters",
                p..end,
            ));
        }
        p += 1;

        // Look for ellipsis.
        if text.at(p) == Some(b':') {
            if ellipsis.is_some() {
                // Already have one.
                return Err(AddrErr::at("multiple :: in address", p..end));
            }
            ellipsis = Some(i);
            p += 1;
            if p == end {
                // Can be at end.
                break;
            }
        }
    }

    // Must have used entire string.
    if p != end {
        return Err(AddrErr::at("trailing garbage after address", p..end));
    }

    // If didn't parse enough, expand ellipsis.
    Ok(Parsed::V6(expand_ellipsis(ip, i, ellipsis)?, zone))
}

/// Go: `Addr.appendTo4`.
pub(crate) fn fmt_v4(out: &mut fmt::Formatter<'_>, octets: [u8; 4]) -> fmt::Result {
    let [first, second, third, fourth] = octets;
    write!(out, "{first}.{second}.{third}.{fourth}")
}

/// The eight 16-bit groups of an IPv6 address (Go: `Addr.v6u16`).
fn groups(a: &[u8; 16]) -> [u16; 8] {
    let mut g = [0_u16; 8];
    for (dst, pair) in g.iter_mut().zip(a.as_chunks::<2>().0) {
        *dst = u16::from_be_bytes(*pair);
    }
    g
}

/// Go: `Addr.appendTo6` without the zone: RFC 5952 text, with `::` replacing the first
/// longest run of two or more zero groups.
pub(crate) fn fmt_v6(f: &mut fmt::Formatter<'_>, a: &[u8; 16]) -> fmt::Result {
    let g = groups(a);
    let (mut zero_start, mut zero_end) = (usize::MAX, usize::MAX);
    let mut best = 0_usize;
    for i in 0..8 {
        let run = g.iter().skip(i).take_while(|&&x| x == 0).count();
        if run >= 2 && run > best {
            (zero_start, zero_end, best) = (i, i + run, run);
        }
    }
    let mut i = 0_usize;
    while i < 8 {
        if i == zero_start {
            f.write_str("::")?;
            i = zero_end;
            if i >= 8 {
                break;
            }
        } else if i > 0 {
            f.write_str(":")?;
        }
        if let Some(x) = g.get(i) {
            write!(f, "{x:x}")?;
        }
        i += 1;
    }
    Ok(())
}

/// Whether the 16 bytes are an IPv4-mapped IPv6 address (Go: `Addr.Is4In6`).
pub(crate) fn is_4in6(a: &[u8; 16]) -> bool {
    a.iter().take(10).all(|&b| b == 0) && a.get(10..12) == Some(&[0xFF, 0xFF][..])
}

/// The IPv4 address in the last four bytes.
pub(crate) const fn last4(a: &[u8; 16]) -> [u8; 4] {
    [a[12], a[13], a[14], a[15]]
}

/// The first four bytes.
pub(crate) const fn first4(a: &[u8; 16]) -> [u8; 4] {
    [a[0], a[1], a[2], a[3]]
}

impl Addr {
    /// Go: `Addr.String` as bytes, exact for a zone that is not UTF-8 (Go strings are byte
    /// strings), which [`Display`](fmt::Display) prints lossily.
    #[must_use]
    pub fn string_bytes(&self) -> Vec<u8> {
        match self {
            Self::V6 { octets, zone } if !zone.is_empty() => {
                let mut out = Self::V6 {
                    octets: *octets,
                    zone: Vec::new(),
                }
                .to_string()
                .into_bytes();
                out.push(b'%');
                out.extend_from_slice(zone);
                out
            }
            Self::Invalid | Self::V4(_) | Self::V6 { .. } => self.to_string().into_bytes(),
        }
    }
}

impl fmt::Display for Addr {
    /// Go: `Addr.String`: `invalid IP`, dotted decimal, `::ffff:` and dotted decimal for an
    /// IPv4-mapped address, or RFC 5952 text, each followed by `%zone` when there is one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid IP"),
            Self::V4(a) => fmt_v4(f, *a),
            Self::V6 { octets, zone } => {
                if is_4in6(octets) {
                    f.write_str("::ffff:")?;
                    fmt_v4(f, last4(octets))?;
                } else {
                    fmt_v6(f, octets)?;
                }
                if zone.is_empty() {
                    Ok(())
                } else {
                    write!(f, "%{}", String::from_utf8_lossy(zone))
                }
            }
        }
    }
}
