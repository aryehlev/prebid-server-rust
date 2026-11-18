//! Ports of `util/iputil/*_test.go` and replays of the `net`, `net/netip` and `iputil` goldens.

use std::{error::Error, str};

use super::{
    IpValidator as _, IpVersion, PublicNetworkIpValidator, VersionIpValidator,
    net::{self, Ip, IpMask, IpNet, cidr_mask, ip_string, parse_cidr, split_host_port},
    netip, parse_ip, parse_ip_bytes,
};
use crate::golden::{
    SplitMix64, Sweep, TestResult, check, check_eq, err_text, fields, hex, read_golden, text,
};

/// The byte alphabet of the Go generator's random IP-shaped strings (Go: `ipLikeAlphabet`).
const IP_LIKE_ALPHABET: &[u8] = b"0123456789abcdefABCDEF.:%  \t01:1.ffg[]/-+xz\xff\xa0";

fn ip_hex(ip: Option<&Ip>) -> String {
    ip.map_or_else(|| "nil".to_owned(), |ip| hex(ip.as_slice()))
}

fn mask_hex(m: Option<&IpMask>) -> String {
    m.map_or_else(|| "nil".to_owned(), |m| hex(m.as_slice()))
}

fn ver_text(ver: IpVersion) -> String {
    ver.as_int().to_string()
}

fn bool_text(b: bool) -> &'static [u8] {
    if b { b"true" } else { b"false" }
}

fn cidr_net(s: &str) -> Result<IpNet, Box<dyn Error>> {
    Ok(parse_cidr(s)?.1)
}

fn nets(cidrs: &[&str]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    cidrs.iter().map(|s| cidr_net(s)).collect()
}

/// Go: `net.IPNet{IP: net.ParseIP(ip), Mask: net.CIDRMask(ones, bits)}`.
fn literal_net(ip: &str, ones: i64, bits: i64) -> Result<IpNet, Box<dyn Error>> {
    Ok(IpNet {
        ip: net::parse_ip(ip).ok_or("bad network IP")?,
        mask: cidr_mask(ones, bits).ok_or("bad mask")?,
    })
}

fn must_parse(s: &str) -> Result<Ip, Box<dyn Error>> {
    net::parse_ip(s).ok_or_else(|| format!("not an IP: {s}").into())
}

/// The validator built from the defaults of `request_validation.ipv4_private_networks` and
/// `ipv6_private_networks`.
fn default_validator() -> Result<PublicNetworkIpValidator, Box<dyn Error>> {
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
    // Go: TestParseIP.
    let cases: [(&str, IpVersion, Option<Ip>); 7] = [
        ("", IpVersion::Unknown, None),
        ("1.1.1.1", IpVersion::V4, Some(Ip::ipv4(1, 1, 1, 1))),
        ("-1.-1.-1.-1", IpVersion::Unknown, None),
        ("256.256.256.256", IpVersion::Unknown, None),
        (
            "::ffff:1.1.1.1",
            IpVersion::V6,
            Some(Ip::from16([
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 1, 1, 1, 1,
            ])),
        ),
        (
            "0101::",
            IpVersion::V6,
            Some(Ip::from16([1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
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
type NetSpec = (&'static str, i64, i64);

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
    // net.ParseIP, IP.String, IP.To4, netip.ParseAddr and iputil.ParseIP over curated,
    // random, mutated and random-byte inputs.
    let rows = read_golden("net_parse.txt")?;
    check(rows.len() > 4000, "net_parse.txt is complete")?;
    for row in &rows {
        let [
            s,
            want_hex,
            want_string,
            want_to4,
            want_addr,
            want_util_hex,
            want_ver,
        ] = fields::<7>(row)?;
        let what = String::from_utf8_lossy(s);
        let ip = net::parse_ip_bytes(s);
        check_eq(ip_hex(ip.as_ref()).as_bytes(), want_hex, &what)?;
        check_eq(ip_string(ip.as_ref()).as_bytes(), want_string, &what)?;
        check_eq(
            bool_text(ip.and_then(|ip| ip.to4()).is_some()),
            want_to4,
            &what,
        )?;
        let addr = netip::parse_addr_bytes(s);
        let addr_text = addr
            .as_ref()
            .map_or_else(|_| err_text(&addr).into_bytes(), netip::Addr::string_bytes);
        check_eq(addr_text.as_slice(), want_addr, &what)?;
        let util = parse_ip_bytes(s);
        check_eq(
            ip_hex(util.map(|(util_ip, _)| util_ip).as_ref()).as_bytes(),
            want_util_hex,
            &what,
        )?;
        check_eq(
            ver_text(util.map_or(IpVersion::Unknown, |(_, v)| v)).as_bytes(),
            want_ver,
            &what,
        )?;
    }
    Ok(())
}

#[test]
fn net_parse_sweep_golden() -> TestResult {
    // net.ParseIP and IP.String over 200000 random IP-shaped strings.
    let rows = read_golden("net_sweeps.txt")?;
    let [name, want] = fields::<2>(rows.first().ok_or("no sweep")?)?;
    check_eq(name, b"parse_ip_200000".as_slice(), "sweep name")?;
    let mut r = SplitMix64(0x5EED_0002);
    let mut sweep = Sweep::default();
    for _ in 0..200_000 {
        let s = r.sweep_string(IP_LIKE_ALPHABET, 20);
        let ip = net::parse_ip_bytes(&s);
        sweep.add(format!("{} {}", ip_hex(ip.as_ref()), ip_string(ip.as_ref())).as_bytes());
    }
    check_eq(sweep.sum().as_bytes(), want, "parse_ip_200000")
}

/// Go: `ip.Mask(m)` where either may be nil, rendered as the golden does: `make(IP, 0)` when
/// both are nil (hex `""`), nil (hex `nil`) on a length mismatch.
fn go_masked(ip: Option<&Ip>, m: Option<&IpMask>) -> (String, String) {
    if ip.is_none() && m.is_none() {
        return (String::new(), "<nil>".to_owned());
    }
    let masked = net::mask_ip(ip, m);
    (ip_hex(masked.as_ref()), ip_string(masked.as_ref()))
}

#[test]
fn net_mask_golden() -> TestResult {
    // CIDRMask, IPMask.Size, IPMask.String and IP.Mask of the 16- and 4-byte forms.
    let rows = read_golden("net_mask.txt")?;
    check(rows.len() > 1800, "net_mask.txt is complete")?;
    for row in &rows {
        let [
            s,
            ones,
            bits,
            want_mask,
            want_ones,
            want_bits,
            want_mask_string,
        ] = fields::<7>(row)?;
        let what = format!("{} {} {}", text(s)?, text(ones)?, text(bits)?);
        let m = cidr_mask(text(ones)?.parse()?, text(bits)?.parse()?);
        check_eq(mask_hex(m.as_ref()).as_bytes(), want_mask, &what)?;
        let (got_ones, got_bits) = m.map_or((0, 0), |m| m.size());
        check_eq(got_ones.to_string().as_bytes(), want_ones, &what)?;
        check_eq(got_bits.to_string().as_bytes(), want_bits, &what)?;
        check_eq(
            m.map_or_else(|| "<nil>".to_owned(), |m| m.to_string())
                .as_bytes(),
            want_mask_string,
            &what,
        )?;
        let [
            ..,
            want_full_hex,
            want_full_text,
            want_short_hex,
            want_short_text,
        ] = fields::<11>(row)?;
        let ip = net::parse_ip(text(s)?);
        let (full_hex, full_text) = go_masked(ip.as_ref(), m.as_ref());
        check_eq(full_hex.as_bytes(), want_full_hex, &what)?;
        check_eq(full_text.as_bytes(), want_full_text, &what)?;
        let short = ip.and_then(|full| full.to4());
        let (short_hex, short_text) = go_masked(short.as_ref(), m.as_ref());
        check_eq(short_hex.as_bytes(), want_short_hex, &what)?;
        check_eq(short_text.as_bytes(), want_short_text, &what)?;
    }
    Ok(())
}

#[test]
fn net_parse_cidr_golden() -> TestResult {
    // ParseCIDR and IPNet.String.
    let rows = read_golden("net_cidr.txt")?;
    check(rows.len() > 800, "net_cidr.txt is complete")?;
    for row in &rows {
        let [input, want_ip, want_net_ip, want_mask, want_net] = fields::<5>(row)?;
        let input = text(input)?;
        let outputs = match parse_cidr(input) {
            Ok((ip, network)) => [
                hex(ip.as_slice()),
                hex(network.ip.as_slice()),
                hex(network.mask.as_slice()),
                network.to_string(),
            ],
            Err(e) => [
                err_text::<(), _>(&Err(e)),
                String::new(),
                String::new(),
                String::new(),
            ],
        };
        let wants = [want_ip, want_net_ip, want_mask, want_net];
        for (output, want) in outputs.iter().zip(wants) {
            check_eq(output.as_bytes(), want, input)?;
        }
    }
    Ok(())
}

#[test]
fn net_contains_golden() -> TestResult {
    // IPNet.Contains and IPNet.String for parsed networks and hand-built ones, against the
    // 16- and 4-byte forms of each candidate.
    let rows = read_golden("net_contains.txt")?;
    check(rows.len() == 22, "net_contains.txt is complete")?;
    for row in &rows {
        let [ip, mask, want_string] = fields::<3>(row)?;
        let n = IpNet {
            ip: Ip::from_slice(&decode_hex(text(ip)?)?).ok_or("bad network IP")?,
            mask: IpMask::from_slice(&decode_hex(text(mask)?)?).ok_or("bad mask")?,
        };
        let what = text(want_string)?;
        check_eq(n.to_string().as_str(), what, "IPNet.String")?;
        let pairs = row.get(3..).ok_or("no candidates")?;
        check(pairs.len() > 50, "candidates are complete")?;
        for pair in pairs.chunks(2) {
            let [candidate, want] = fields::<2>(pair)?;
            let c = Ip::from_slice(&decode_hex(text(candidate)?)?).ok_or("bad candidate")?;
            check_eq(
                bool_text(n.contains(&c)),
                want,
                &format!("{what} contains {c}"),
            )?;
        }
    }
    Ok(())
}

fn decode_hex(s: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let digits = s.as_bytes();
    digits
        .chunks(2)
        .map(|pair| {
            let pair = str::from_utf8(pair)?;
            Ok(u8::from_str_radix(pair, 16)?)
        })
        .collect()
}

#[test]
fn net_split_host_port_golden() -> TestResult {
    let rows = read_golden("net_splithostport.txt")?;
    check(rows.len() > 1500, "net_splithostport.txt is complete")?;
    for row in &rows {
        let [s, want_host, want_port, want_err] = fields::<4>(row)?;
        let s = text(s)?;
        let got = split_host_port(s);
        let (host, port) = got.as_ref().map_or(("", ""), |&(h, p)| (h, p));
        check_eq(host.as_bytes(), want_host, s)?;
        check_eq(port.as_bytes(), want_port, s)?;
        check_eq(err_text(&got).as_bytes(), want_err, s)?;
    }
    Ok(())
}

#[test]
fn public_network_validator_golden() -> TestResult {
    // PublicNetworkIPValidator.IsValid for every parsable input of net_parse.txt, with the
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
        let ip = net::parse_ip_bytes(s).ok_or("unparsable validator input")?;
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
    // IP.Equal, IP.To16, net.IPv4, IPv4Mask, and the error texts of ParseCIDR and
    // SplitHostPort.
    let mapped = must_parse("1.2.3.4")?;
    check(
        mapped.equal(&Ip::from4([1, 2, 3, 4])),
        "IPv4 equals its mapped form",
    )?;
    check(Ip::from4([1, 2, 3, 4]).equal(&mapped), "Equal is symmetric")?;
    check(
        !must_parse("::1.2.3.4")?.equal(&Ip::from4([1, 2, 3, 4])),
        "::1.2.3.4 is not mapped",
    )?;
    check_eq(&Ip::from4([1, 2, 3, 4]).to16(), &mapped.to16(), "To16")?;
    check_eq(&Ip::ipv4(1, 2, 3, 4), &mapped, "net.IPv4")?;
    check_eq(
        &net::ipv4_mask(255, 255, 0, 0).size(),
        &(16, 32),
        "IPv4Mask size",
    )?;
    check_eq(&Ip::from_slice(&[1, 2, 3]), &None, "3-byte IP")?;
    check_eq(
        err_text(&parse_cidr("1.2.3.4")).as_str(),
        "error: invalid CIDR address: 1.2.3.4",
        "ParseCIDR error",
    )?;
    check_eq(
        err_text(&split_host_port("1.2.3.4")).as_str(),
        "error: address 1.2.3.4: missing port in address",
        "SplitHostPort error",
    )?;
    check_eq(
        &net::mask_ip(None, cidr_mask(24, 32).as_ref()),
        &None,
        "privacy.scrubIP of an unparsable IP",
    )?;
    check_eq(
        netip::Addr::default().to_string().as_str(),
        "invalid IP",
        "zero Addr",
    )
}
