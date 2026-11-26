//! Ports of `util/iputil/*_test.go` and replays of the `net` and `iputil` goldens.
//!
//! The goldens were written for a port of Go's `net` types. What they record of `IPMask` and
//! `IP.To4` on their own has no counterpart on std's addresses and is not replayed, nor is
//! `netip.ParseAddr`, which no Prebid code calls: Prebid reaches the others only through the
//! parse, mask and contains functions replayed here. httputil's tests replay `SplitHostPort` on
//! the texts Go's server writes for a peer.

use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str,
};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};

use super::{
    IpValidator as _, IpVersion, PublicNetworkIpValidator, VersionIpValidator,
    net::{self, ip_string, parse_cidr},
    parse_ip,
};
use crate::golden::{
    SplitMix64, Sweep, TestResult, check, check_eq, err_text, fields, hex, read_golden, text,
};

/// The byte alphabet of the Go generator's random IP-shaped strings (Go: `ipLikeAlphabet`).
const IP_LIKE_ALPHABET: &[u8] = b"0123456789abcdefABCDEF.:%  \t01:1.ffg[]/-+xz\xff\xa0";

/// The 16 bytes of Go's `net.IP` from `ParseIP`, in hex, or `nil`.
pub(super) fn go_hex(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| "nil".to_owned(), |ip| hex(&sixteen(ip)))
}

/// Go's 16-byte form of an address.
fn sixteen(ip: IpAddr) -> [u8; 16] {
    match ip {
        IpAddr::V4(a) => a.to_ipv6_mapped().octets(),
        IpAddr::V6(a) => a.octets(),
    }
}

/// `net.ParseIP` of a golden field: a field that is not UTF-8 is not ASCII, so Go's parse
/// fails on it as well.
fn parse_field(s: &[u8]) -> Option<IpAddr> {
    str::from_utf8(s).ok().and_then(net::parse_ip)
}

fn bool_text(b: bool) -> &'static [u8] {
    if b { b"true" } else { b"false" }
}

fn nets(cidrs: &[&str]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    cidrs.iter().map(|s| Ok(parse_cidr(s)?.1)).collect()
}

/// Go: `net.IPNet{IP: net.ParseIP(ip), Mask: net.CIDRMask(ones, bits)}`: with a 32-bit mask
/// the IPv4 network of the address, with a 128-bit mask the IPv6 network of its 16 bytes.
fn literal_net(ip: &str, ones: u8, bits: u8) -> Result<IpNet, Box<dyn Error>> {
    let ip = net::parse_ip(ip).ok_or("bad network IP")?;
    Ok(match (bits, ip) {
        (32, IpAddr::V4(a)) => IpNet::V4(Ipv4Net::new(a, ones)?),
        (128, _) => IpNet::V6(Ipv6Net::new(Ipv6Addr::from(sixteen(ip)), ones)?),
        _ => return Err("no such literal network".into()),
    })
}

fn must_parse(s: &str) -> Result<IpAddr, Box<dyn Error>> {
    net::parse_ip(s).ok_or_else(|| format!("not an IP: {s}").into())
}

/// The validator built from the defaults of `request_validation.ipv4_private_networks` and
/// `ipv6_private_networks`.
pub(super) fn default_validator() -> Result<PublicNetworkIpValidator, Box<dyn Error>> {
    Ok(PublicNetworkIpValidator {
        ipv4_private_networks: nets(&[
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "127.0.0.0/8",
        ])?,
        ipv6_private_networks: nets(&[
            "::1/128",
            "fc00::/7",
            "fe80::/10",
            "ff00::/8",
            "2001:db8::/32",
        ])?,
    })
}

#[test]
fn parse_ip_go_cases() -> TestResult {
    // Go: TestParseIP. Go's 16-byte IP of "::ffff:1.1.1.1" is the IP of "1.1.1.1".
    let cases: [(&str, IpVersion, Option<IpAddr>); 7] = [
        ("", IpVersion::Unknown, None),
        (
            "1.1.1.1",
            IpVersion::V4,
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))),
        ),
        ("-1.-1.-1.-1", IpVersion::Unknown, None),
        ("256.256.256.256", IpVersion::Unknown, None),
        (
            "::ffff:1.1.1.1",
            IpVersion::V6,
            Some(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))),
        ),
        (
            "0101::",
            IpVersion::V6,
            Some(IpAddr::V6(Ipv6Addr::new(0x0101, 0, 0, 0, 0, 0, 0, 0))),
        ),
        ("zzzz::", IpVersion::Unknown, None),
    ];
    for (input, want_ver, want_ip) in cases {
        let got = parse_ip(input);
        check_eq(
            &got.map_or(IpVersion::Unknown, |(_, v)| v),
            &want_ver,
            input,
        )?;
        check_eq(&got.map(|(ip, _)| ip), &want_ip, input)?;
    }
    Ok(())
}

/// Go: `net.IPNet{IP: net.ParseIP(ip), Mask: net.CIDRMask(ones, bits)}`, as `(ip, ones, bits)`.
type NetSpec = (&'static str, u8, u8);

const V4_NET1: NetSpec = ("1.0.0.0", 8, 32);
const V4_NET2: NetSpec = ("2.0.0.0", 8, 32);
const V6_NET1: NetSpec = ("3300::", 8, 128);
const V6_NET2: NetSpec = ("4400::", 8, 128);
const MAPPED_V6: NetSpec = ("::FFFF:2.0.0.0", 108, 128);

/// A case of Go's `TestPublicNetworkIPValidator`.
struct ValidatorCase {
    description: &'static str,
    ip: &'static str,
    ver: IpVersion,
    ipv4_private_networks: &'static [NetSpec],
    ipv6_private_networks: &'static [NetSpec],
    expected: bool,
}

const fn validator_case(
    description: &'static str,
    (ip, ver): (&'static str, IpVersion),
    ipv4_private_networks: &'static [NetSpec],
    ipv6_private_networks: &'static [NetSpec],
    expected: bool,
) -> ValidatorCase {
    ValidatorCase {
        description,
        ip,
        ver,
        ipv4_private_networks,
        ipv6_private_networks,
        expected,
    }
}

const V4: IpVersion = IpVersion::V4;
const V6: IpVersion = IpVersion::V6;

/// Go: the cases of `TestPublicNetworkIPValidator`.
const VALIDATOR_CASES: [ValidatorCase; 17] = [
    validator_case("IPv4 - Public - None", ("1.1.1.1", V4), &[], &[], true),
    validator_case(
        "IPv4 - Public - One",
        ("2.2.2.2", V4),
        &[V4_NET1],
        &[],
        true,
    ),
    validator_case(
        "IPv4 - Public - Many",
        ("3.3.3.3", V4),
        &[V4_NET1, V4_NET2],
        &[],
        true,
    ),
    validator_case(
        "IPv4 - Private - One",
        ("1.1.1.1", V4),
        &[V4_NET1],
        &[],
        false,
    ),
    validator_case(
        "IPv4 - Private - Many",
        ("2.2.2.2", V4),
        &[V4_NET1, V4_NET2],
        &[],
        false,
    ),
    validator_case("IPv6 - Public - None", ("3333::", V6), &[], &[], true),
    validator_case("IPv6 - Public - One", ("4444::", V6), &[], &[V6_NET1], true),
    validator_case(
        "IPv6 - Public - Many",
        ("5555::", V6),
        &[],
        &[V6_NET1, V6_NET2],
        true,
    ),
    validator_case(
        "IPv6 - Private - One",
        ("3333::", V6),
        &[],
        &[V6_NET1],
        false,
    ),
    validator_case(
        "IPv6 - Private - Many",
        ("4444::", V6),
        &[],
        &[V6_NET1, V6_NET2],
        false,
    ),
    validator_case(
        "Mixed - Unknown",
        ("3.3.3.3", IpVersion::Unknown),
        &[V4_NET1, V4_NET1],
        &[V6_NET1, V6_NET2],
        false,
    ),
    validator_case(
        "Mixed - Public - IPv4",
        ("3.3.3.3", V4),
        &[V4_NET1, V4_NET1],
        &[V6_NET1, V6_NET2],
        true,
    ),
    validator_case(
        "Mixed - Public - IPv6",
        ("5555::", V6),
        &[V4_NET1, V4_NET1],
        &[V6_NET1, V6_NET2],
        true,
    ),
    validator_case(
        "Mixed - Private - IPv4",
        ("1.1.1.1", V4),
        &[V4_NET1, V4_NET1],
        &[V6_NET1, V6_NET2],
        false,
    ),
    validator_case(
        "Mixed - Private - IPv6",
        ("3333::", V6),
        &[V4_NET1, V4_NET1],
        &[V6_NET1, V6_NET2],
        false,
    ),
    validator_case(
        "Mixed - Public - IPv6 Encoded IPv4",
        ("::FFFF:1.1.1.1", V6),
        &[V4_NET1],
        &[MAPPED_V6],
        true,
    ),
    validator_case(
        "Mixed - Private - IPv6 Encoded IPv4",
        ("::FFFF:2.2.2.2", V6),
        &[V4_NET1],
        &[MAPPED_V6],
        false,
    ),
];

fn spec_nets(specs: &[NetSpec]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    specs
        .iter()
        .map(|&(ip, ones, bits)| literal_net(ip, ones, bits))
        .collect()
}

#[test]
fn public_network_ip_validator_go_cases() -> TestResult {
    // Go: TestPublicNetworkIPValidator.
    for case in &VALIDATOR_CASES {
        let validator = PublicNetworkIpValidator {
            ipv4_private_networks: spec_nets(case.ipv4_private_networks)?,
            ipv6_private_networks: spec_nets(case.ipv6_private_networks)?,
        };
        check_eq(
            &validator.is_valid(&must_parse(case.ip)?, case.ver),
            &case.expected,
            case.description,
        )?;
    }
    Ok(())
}

#[test]
fn version_ip_validator_go_cases() -> TestResult {
    // Go: TestVersionIPValidator. Go passes a nil IP with IPvUnknown; any address stands in.
    let cases = [
        ("IPv4", IpVersion::V4, "1.1.1.1", IpVersion::V4, true),
        (
            "IPv4 - Given Unknown",
            IpVersion::V4,
            "::",
            IpVersion::Unknown,
            false,
        ),
        ("IPv6", IpVersion::V6, "1111::", IpVersion::V6, true),
        (
            "IPv6 - Given Unknown",
            IpVersion::V6,
            "::",
            IpVersion::Unknown,
            false,
        ),
    ];
    for (description, version, ip, ver, want) in cases {
        let validator = VersionIpValidator { version };
        check_eq(
            &validator.is_valid(&must_parse(ip)?, ver),
            &want,
            description,
        )?;
    }
    Ok(())
}

#[test]
fn net_parse_golden() -> TestResult {
    // Go: net.ParseIP, IP.String and IP.To4 (as the address's family) and iputil.ParseIP over
    // curated, random, mutated and random-byte inputs.
    let rows = read_golden("net_parse.txt")?;
    check(rows.len() > 4000, "net_parse.txt is complete")?;
    for row in &rows {
        let [
            s,
            want_hex,
            want_string,
            want_to4,
            _addr,
            want_util_hex,
            want_ver,
        ] = fields::<7>(row)?;
        let what = String::from_utf8_lossy(s);
        let ip = parse_field(s);
        check_eq(go_hex(ip).as_bytes(), want_hex, &what)?;
        check_eq(ip_string(ip).as_bytes(), want_string, &what)?;
        check_eq(
            bool_text(ip.is_some_and(|ip| ip.is_ipv4())),
            want_to4,
            &what,
        )?;
        let util = str::from_utf8(s).ok().and_then(parse_ip);
        check_eq(
            go_hex(util.map(|(util_ip, _)| util_ip)).as_bytes(),
            want_util_hex,
            &what,
        )?;
        check_eq(
            util.map_or(IpVersion::Unknown, |(_, v)| v)
                .as_int()
                .to_string()
                .as_bytes(),
            want_ver,
            &what,
        )?;
    }
    Ok(())
}

#[test]
fn net_parse_sweep_golden() -> TestResult {
    // Go: net.ParseIP and IP.String over 200000 random IP-shaped strings.
    let rows = read_golden("net_sweeps.txt")?;
    let [name, want] = fields::<2>(rows.first().ok_or("no sweep")?)?;
    check_eq(name, b"parse_ip_200000".as_slice(), "sweep name")?;
    let mut r = SplitMix64(0x5EED_0002);
    let mut sweep = Sweep::default();
    for _ in 0..200_000 {
        let s = r.sweep_string(IP_LIKE_ALPHABET, 20);
        let ip = parse_field(&s);
        sweep.add(format!("{} {}", go_hex(ip), ip_string(ip)).as_bytes());
    }
    check_eq(sweep.sum().as_bytes(), want, "parse_ip_200000")
}

/// Whether Go's `net.CIDRMask(ones, bits)` is not nil.
fn mask_exists(ones: i64, bits: i64) -> bool {
    (bits == 32 || bits == 128) && (0..=bits).contains(&ones)
}

#[test]
fn net_mask_golden() -> TestResult {
    // Go: net.ParseIP(s).Mask(net.CIDRMask(ones, bits)), the masking of privacy.scrubIP: its
    // bytes (4 for a 32-bit mask, 16 for a 128-bit one; none at all when both are nil) and
    // its text.
    let rows = read_golden("net_mask.txt")?;
    check(rows.len() > 1800, "net_mask.txt is complete")?;
    for row in &rows {
        let [s, ones, bits, .., want_full_hex, want_full_text, _, _] = fields::<11>(row)?;
        let (ones, bits): (i64, i64) = (text(ones)?.parse()?, text(bits)?.parse()?);
        let what = format!("{} {ones} {bits}", text(s)?);
        let ip = net::parse_ip(text(s)?);
        let masked = ip.and_then(|ip| net::mask(ip, ones, bits));
        let got_hex = match masked {
            None if ip.is_none() && !mask_exists(ones, bits) => String::new(),
            None => "nil".to_owned(),
            Some(IpAddr::V4(a)) if bits == 32 => hex(&a.octets()),
            Some(m) => hex(&sixteen(m)),
        };
        check_eq(got_hex.as_bytes(), want_full_hex, &what)?;
        check_eq(ip_string(masked).as_bytes(), want_full_text, &what)?;
    }
    Ok(())
}

/// The bytes of Go's `IPNet.IP` (4 for an IPv4 network, 16 for an IPv6 one) and `IPNet.Mask`.
fn net_bytes(n: &IpNet) -> (Vec<u8>, Vec<u8>) {
    match n {
        IpNet::V4(n) => (n.addr().octets().to_vec(), n.netmask().octets().to_vec()),
        IpNet::V6(n) => (n.addr().octets().to_vec(), n.netmask().octets().to_vec()),
    }
}

#[test]
fn net_parse_cidr_golden() -> TestResult {
    // Go: net.ParseCIDR: the verdict and error text, the 16 bytes of the address, the bytes of
    // the network and its mask (whose length request_validation checks), and IPNet.String.
    // Go writes a network in IPv4-mapped form as the IPv4 network it maps; ipnet keeps its
    // IPv6 text, which nothing in Prebid prints (the ip.network-edges ledger entry).
    let rows = read_golden("net_cidr.txt")?;
    check(rows.len() > 800, "net_cidr.txt is complete")?;
    let mut mapped_texts = 0_usize;
    for row in &rows {
        let [input, want_ip, want_net_ip, want_mask, want_net] = fields::<5>(row)?;
        let input = text(input)?;
        match parse_cidr(input) {
            Ok((ip, n)) => {
                let (net_ip, mask) = net_bytes(&n);
                check_eq(go_hex(Some(ip)).as_bytes(), want_ip, input)?;
                check_eq(hex(&net_ip).as_bytes(), want_net_ip, input)?;
                check_eq(hex(&mask).as_bytes(), want_mask, input)?;
                let mapped = matches!(n, IpNet::V6(v6) if v6.addr().to_ipv4_mapped().is_some());
                if mapped {
                    check(n.to_string().as_bytes() != want_net, input)?;
                    mapped_texts += 1;
                } else {
                    check_eq(n.to_string().as_bytes(), want_net, input)?;
                }
            }
            Err(e) => {
                check_eq(err_text::<(), _>(&Err(e)).as_bytes(), want_ip, input)?;
                check(want_net.is_empty(), "no network for an error")?;
            }
        }
    }
    check(
        mapped_texts > 0,
        "networks in IPv4-mapped form, which Go writes as IPv4 networks",
    )
}

/// The prefix length of a 4- or 16-byte mask whose ones all lead, else `None`.
fn prefix_of(m: &[u8]) -> Option<u8> {
    let (ones, zeros, width) = if let Ok(b) = <[u8; 4]>::try_from(m) {
        let v = u32::from_be_bytes(b);
        (v.leading_ones(), v.trailing_zeros(), 32)
    } else {
        let v = u128::from_be_bytes(<[u8; 16]>::try_from(m).ok()?);
        (v.leading_ones(), v.trailing_zeros(), 128)
    };
    if ones + zeros == width {
        u8::try_from(ones).ok()
    } else {
        None
    }
}

/// Go's `networkNumberAndMask` of a hand-built `IPNet`, as ipnet's network, when ipnet can
/// hold it: `None` for a mask whose ones do not all lead or a 4-byte mask on an IPv6 address,
/// which `net.ParseCIDR`, the only way Prebid makes a network, never writes. A 16-byte
/// IPv4-mapped address with a 16-byte mask stays an IPv6 network, as `ParseCIDR` makes it.
fn hand_built(ip: &[u8], mask: &[u8]) -> Option<IpNet> {
    if let Ok(b) = <[u8; 16]>::try_from(ip) {
        let a = Ipv6Addr::from(b);
        return match (a.to_ipv4_mapped(), mask.len()) {
            (_, 16) => Ipv6Net::new(a, prefix_of(mask)?).ok().map(IpNet::V6),
            (Some(v4), 4) => Ipv4Net::new(v4, prefix_of(mask)?).ok().map(IpNet::V4),
            _ => None,
        };
    }
    let a = Ipv4Addr::from(<[u8; 4]>::try_from(ip).ok()?);
    // Go: a 16-byte mask on a 4-byte address is cut to its last 4 bytes.
    let mask = if mask.len() == 16 {
        mask.get(12..)?
    } else {
        mask
    };
    Ipv4Net::new(a, prefix_of(mask)?).ok().map(IpNet::V4)
}

#[test]
fn net_contains_golden() -> TestResult {
    // Go: IPNet.Contains for parsed networks and hand-built ones, against the 16- and 4-byte
    // forms of each candidate.
    let rows = read_golden("net_contains.txt")?;
    check(rows.len() == 22, "net_contains.txt is complete")?;
    let mut replayed = 0_usize;
    for row in &rows {
        let [ip, mask, want_string] = fields::<3>(row)?;
        let Some(n) = hand_built(&decode_hex(text(ip)?)?, &decode_hex(text(mask)?)?) else {
            continue;
        };
        replayed += 1;
        let what = text(want_string)?;
        let pairs = row.get(3..).ok_or("no candidates")?;
        check(pairs.len() > 50, "candidates are complete")?;
        for pair in pairs.chunks(2) {
            let [candidate, want] = fields::<2>(pair)?;
            let b = decode_hex(text(candidate)?)?;
            let c = match <[u8; 16]>::try_from(b.as_slice()) {
                Ok(b16) => IpAddr::V6(Ipv6Addr::from(b16)),
                Err(_) => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(b.as_slice())?)),
            };
            check_eq(
                bool_text(net::contains(&n, c)),
                want,
                &format!("{what} contains {c}"),
            )?;
        }
    }
    check_eq(&replayed, &19, "the networks ipnet holds")
}

fn decode_hex(s: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    s.as_bytes()
        .chunks(2)
        .map(|pair| Ok(u8::from_str_radix(str::from_utf8(pair)?, 16)?))
        .collect()
}

#[test]
fn public_network_validator_golden() -> TestResult {
    // Go: PublicNetworkIPValidator.IsValid for every parsable input of net_parse.txt, with the
    // config defaults, no networks, and networks written in IPv4-mapped form.
    let rows = read_golden("iputil_validator.txt")?;
    check(rows.len() > 5000, "iputil_validator.txt is complete")?;
    let default = default_validator()?;
    let empty = PublicNetworkIpValidator::default();
    let mapped = PublicNetworkIpValidator {
        ipv4_private_networks: nets(&["::ffff:10.0.0.0/104"])?,
        ipv6_private_networks: nets(&["::ffff:10.0.0.0/104", "::ffff:0:0/96"])?,
    };
    for row in &rows {
        let [name, s, unknown, v4, v6] = fields::<5>(row)?;
        let validator = match name {
            b"default" => &default,
            b"empty" => &empty,
            b"mapped" => &mapped,
            _ => return Err(format!("unknown validator {name:?}").into()),
        };
        let what = String::from_utf8_lossy(s);
        let ip = parse_field(s).ok_or("unparsable validator input")?;
        for (ver, want) in [
            (IpVersion::Unknown, unknown),
            (IpVersion::V4, v4),
            (IpVersion::V6, v6),
        ] {
            check_eq(
                bool_text(validator.is_valid(&ip, ver)),
                want,
                &format!("{} {what} {ver:?}", text(name)?),
            )?;
        }
    }
    Ok(())
}

#[test]
fn ip_helpers_match_go() -> TestResult {
    // Go: IP.Equal of an address and its mapped form, the error text of ParseCIDR, and
    // privacy.scrubIP of an address that does not parse.
    check_eq(
        &must_parse("::ffff:1.2.3.4")?,
        &must_parse("1.2.3.4")?,
        "an IPv4 address equals its mapped form",
    )?;
    check(
        must_parse("::1.2.3.4")? != must_parse("1.2.3.4")?,
        "::1.2.3.4 is not mapped",
    )?;
    check_eq(
        err_text(&parse_cidr("1.2.3.4")).as_str(),
        "error: invalid CIDR address: 1.2.3.4",
        "ParseCIDR error",
    )?;
    check_eq(
        &net::parse_ip("1.2.3.4.").and_then(|ip| net::mask(ip, 24, 32)),
        &None,
        "privacy.scrubIP of an unparsable IP",
    )?;
    check_eq(
        &parse_cidr("10.0.0.0/08")?.1,
        &parse_cidr("10.0.0.0/8")?.1,
        "a prefix with a leading zero",
    )?;
    check(
        parse_cidr("fe80::1%eth0/64").is_err(),
        "ParseCIDR rejects a zone",
    )
}
