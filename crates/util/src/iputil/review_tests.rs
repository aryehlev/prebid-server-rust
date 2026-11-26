//! Replays of the systematic goldens written by `testdata/go/review`: every IPv4 octet
//! spelling, embedded-IPv4 prefixes, zones and brackets; `privacy.scrubIP` masking for every
//! prefix length; private network boundaries in every textual form; and the address Go's HTTP
//! server reports for each kind of peer.

use std::{
    error::Error,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    str,
};

use ipnet::IpNet;

use super::{
    IpValidator as _, IpVersion, PublicNetworkIpValidator, VersionIpValidator,
    net::{self, ip_string, parse_cidr},
    parse_ip,
    tests::{default_validator, go_hex},
};
use crate::{
    golden::{TestResult, check, check_eq, fields, read_golden, text},
    httputil::{RequestHeaders, find_ip},
};

fn nets(cidrs: &[&str]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    cidrs.iter().map(|s| Ok(parse_cidr(s)?.1)).collect()
}

#[test]
fn review_parse_golden() -> TestResult {
    // Go: net.ParseIP, IP.String and iputil.ParseIP's version over every octet spelling in
    // every position, embedded IPv4 after every prefix, zones, brackets, group widths and
    // ellipsis positions.
    let rows = read_golden("review_parse.txt")?;
    check(rows.len() > 700, "review_parse.txt is complete")?;
    for row in &rows {
        let [s, want_hex, want_string, want_ver, _addr] = fields::<5>(row)?;
        let what = String::from_utf8_lossy(s);
        let parsed = str::from_utf8(s).ok().and_then(parse_ip);
        let ip = parsed.map(|(ip, _)| ip);
        check_eq(go_hex(ip).as_bytes(), want_hex, &what)?;
        check_eq(ip_string(ip).as_bytes(), want_string, &what)?;
        let ver = parsed.map_or(IpVersion::Unknown, |(_, v)| v);
        check_eq(ver.as_int().to_string().as_bytes(), want_ver, &what)?;
    }
    Ok(())
}

/// Go: `privacy.scrubIP(ip, ones, bits)`.
fn scrub_ip(ip: &str, ones: i64, bits: i64) -> String {
    if ip.is_empty() {
        return String::new();
    }
    ip_string(net::parse_ip(ip).and_then(|ip| net::mask(ip, ones, bits)))
}

#[test]
fn review_mask_golden() -> TestResult {
    // Go: privacy.scrubIP for every prefix length from -1 to 129 with 32- and 128-bit masks,
    // over IPv4, IPv4-mapped, IPv4-compatible, SIIT, NAT64 and IPv6 addresses.
    let rows = read_golden("review_mask.txt")?;
    check(rows.len() == 48, "review_mask.txt is complete")?;
    for row in &rows {
        let [s, bits, want_scrubbed, _four] = fields::<4>(row)?;
        let s = text(s)?;
        let bits: i64 = text(bits)?.parse()?;
        let scrubbed: Vec<String> = (-1..=129).map(|ones| scrub_ip(s, ones, bits)).collect();
        check_eq(
            scrubbed.join(",").as_bytes(),
            want_scrubbed,
            &format!("{s} /{bits}"),
        )?;
    }
    Ok(())
}

/// Go: `v4Networks` and `v6Networks` of the review generator, in order.
const BOUNDARY_NETWORKS: [&str; 19] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "127.0.0.0/8",
    "100.64.0.0/10",
    "0.0.0.0/8",
    "224.0.0.0/4",
    "255.255.255.255/32",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
    "2001:db8::/32",
    "::ffff:0:0/96",
    "64:ff9b::/96",
    "2002::/16",
    "::/128",
    "::ffff:10.0.0.0/104",
];

#[test]
fn review_boundaries_golden() -> TestResult {
    // Go: the first and last address of each network and their neighbours, written in
    // dotted, IPv4-mapped (lower and upper case, uncompressed, hex), IPv4-compatible, NAT64,
    // compressed and expanded forms: iputil.ParseIP's version, IPNet.Contains for each
    // network, and PublicNetworkIPValidator.IsValid with the config defaults and with mapped
    // networks.
    let rows = read_golden("review_boundaries.txt")?;
    check(rows.len() > 400, "review_boundaries.txt is complete")?;
    let all = nets(&BOUNDARY_NETWORKS)?;
    let default = default_validator()?;
    let mapped = PublicNetworkIpValidator {
        ipv4_private_networks: nets(&["::ffff:10.0.0.0/104", "::ffff:0:0/96"])?,
        ipv6_private_networks: nets(&[
            "::ffff:10.0.0.0/104",
            "::ffff:0:0/96",
            "::ffff:192.168.0.0/112",
        ])?,
    };
    for row in &rows {
        let [
            s,
            want_ver,
            want_string,
            want_default,
            want_mapped,
            want_bits,
        ] = fields::<6>(row)?;
        let s = text(s)?;
        let parsed = parse_ip(s);
        let ver = parsed.map_or(IpVersion::Unknown, |(_, v)| v);
        check_eq(ver.as_int().to_string().as_bytes(), want_ver, s)?;
        let ip = parsed.map(|(ip, _)| ip);
        check_eq(ip_string(ip).as_bytes(), want_string, s)?;
        let valid = |v: &PublicNetworkIpValidator| ip.is_some_and(|ip| v.is_valid(&ip, ver));
        check_eq(valid(&default).to_string().as_bytes(), want_default, s)?;
        check_eq(valid(&mapped).to_string().as_bytes(), want_mapped, s)?;
        let bits: String = all
            .iter()
            .map(|n| {
                if ip.is_some_and(|ip| net::contains(n, ip)) {
                    '1'
                } else {
                    '0'
                }
            })
            .collect();
        check_eq(bits.as_bytes(), want_bits, s)?;
    }
    Ok(())
}

/// A request without the headers `find_ip` reads.
struct NoHeaders;

impl RequestHeaders for NoHeaders {
    fn first_value(&self, _name: &str) -> Option<&[u8]> {
        None
    }
}

/// The peer of a golden row: a 4-byte address is `V4`, a 16-byte one `V6` with the row's
/// scope.
fn socket_addr(ip: &[u8], port: u16, scope_id: u32) -> Result<SocketAddr, Box<dyn Error>> {
    if let Ok(a) = <[u8; 4]>::try_from(ip) {
        check(scope_id == 0, "no zone for IPv4")?;
        return Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(a), port)));
    }
    let a = <[u8; 16]>::try_from(ip)?;
    Ok(SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::from(a),
        port,
        0,
        scope_id,
    )))
}

#[test]
fn review_remote_addr_golden() -> TestResult {
    // Go: the address FindIP takes from Request.RemoteAddr, which Go's server writes with
    // (*net.TCPAddr).String(), for accepted peers, IPv4-mapped ones of a dual-stack listener
    // and zoned ones included.
    let rows = read_golden("review_remoteaddr.txt")?;
    check(rows.len() > 60, "review_remoteaddr.txt is complete")?;
    let v4 = VersionIpValidator {
        version: IpVersion::V4,
    };
    let v6 = VersionIpValidator {
        version: IpVersion::V6,
    };
    for row in &rows {
        let [ip_hex_text, port, zone, remote_text, want_found, want_ver] = fields::<6>(row)?;
        let ip_hex_text = text(ip_hex_text)?;
        let ip = (0..ip_hex_text.len())
            .step_by(2)
            .map(|i| {
                Ok(u8::from_str_radix(
                    ip_hex_text.get(i..i + 2).ok_or("odd hex")?,
                    16,
                )?)
            })
            .collect::<Result<Vec<u8>, Box<dyn Error>>>()?;
        let addr = socket_addr(&ip, text(port)?.parse()?, text(zone)?.parse()?)?;
        let found =
            find_ip(&NoHeaders, Some(&addr), &v4).or_else(|| find_ip(&NoHeaders, Some(&addr), &v6));
        let what = text(remote_text)?;
        check_eq(
            ip_string(found.map(|(found_ip, _)| found_ip)).as_bytes(),
            want_found,
            what,
        )?;
        check_eq(
            found
                .map_or(IpVersion::Unknown, |(_, v)| v)
                .as_int()
                .to_string()
                .as_bytes(),
            want_ver,
            what,
        )?;
    }
    Ok(())
}

#[test]
fn a_mapped_peer_is_ipv4() -> TestResult {
    // Go: the server writes the IPv4 peer of a dual-stack listener as "8.8.8.8:80", whose
    // address iputil.ParseIP calls IPv4.
    let addr = SocketAddr::V6(SocketAddrV6::new(
        Ipv4Addr::new(8, 8, 8, 8).to_ipv6_mapped(),
        80,
        0,
        0,
    ));
    let v4 = VersionIpValidator {
        version: IpVersion::V4,
    };
    check_eq(
        &find_ip(&NoHeaders, Some(&addr), &v4).map(|(ip, _)| ip.to_string()),
        &Some("8.8.8.8".to_owned()),
        "the mapped peer",
    )?;
    check(
        find_ip(&NoHeaders, None, &v4).is_none(),
        "a Unix socket peer has no address",
    )
}
