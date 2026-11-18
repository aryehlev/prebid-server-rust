//! Replays of the systematic goldens written by `testdata/go/review`: every IPv4 octet
//! spelling, embedded-IPv4 prefixes, zones and brackets; `privacy.scrubIP` masking for every
//! prefix length; private network boundaries in every textual form; and the text Go's HTTP
//! server stores in `Request.RemoteAddr`.

use std::{
    error::Error,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
};

use super::{
    IpValidator as _, IpVersion, PublicNetworkIpValidator, VersionIpValidator,
    net::{self, Ip, IpNet, cidr_mask, ip_string, join_host_port, parse_cidr, tcp_addr_string},
    netip, parse_ip, parse_ip_bytes,
};
use crate::{
    golden::{TestResult, check, check_eq, err_text, fields, hex, read_golden, text},
    httputil::{RequestHeaders, find_ip},
};

fn ip_hex(ip: Option<&Ip>) -> String {
    ip.map_or_else(|| "nil".to_owned(), |ip| hex(ip.as_slice()))
}

fn nets(cidrs: &[&str]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    cidrs.iter().map(|s| Ok(parse_cidr(s)?.1)).collect()
}

#[test]
fn review_parse_golden() -> TestResult {
    // net.ParseIP, IP.String, iputil.ParseIP's version and netip.ParseAddr over every octet
    // spelling in every position, embedded IPv4 after every prefix, zones, brackets, group
    // widths and ellipsis positions.
    let rows = read_golden("review_parse.txt")?;
    check(rows.len() > 700, "review_parse.txt is complete")?;
    for row in &rows {
        let [s, want_hex, want_string, want_ver, want_addr] = fields::<5>(row)?;
        let what = String::from_utf8_lossy(s);
        let ip = net::parse_ip_bytes(s);
        check_eq(ip_hex(ip.as_ref()).as_bytes(), want_hex, &what)?;
        check_eq(ip_string(ip.as_ref()).as_bytes(), want_string, &what)?;
        let ver = parse_ip_bytes(s).map_or(IpVersion::Unknown, |(_, v)| v);
        check_eq(ver.as_int().to_string().as_bytes(), want_ver, &what)?;
        let addr = netip::parse_addr_bytes(s);
        let addr_text = addr.as_ref().map_or_else(
            |_| err_text(&addr).into_bytes(),
            |a| [b"ok ".as_slice(), &a.string_bytes()].concat(),
        );
        check_eq(addr_text.as_slice(), want_addr, &what)?;
    }
    Ok(())
}

/// Go: `privacy.scrubIP(ip, ones, bits)`.
fn scrub_ip(ip: &str, ones: i64, bits: i64) -> String {
    if ip.is_empty() {
        return String::new();
    }
    let masked = net::mask_ip(net::parse_ip(ip).as_ref(), cidr_mask(ones, bits).as_ref());
    ip_string(masked.as_ref())
}

#[test]
fn review_mask_golden() -> TestResult {
    // privacy.scrubIP and the masking of the 4-byte form for every prefix length from -1 to
    // 129 with 32- and 128-bit masks, over IPv4, IPv4-mapped, IPv4-compatible, SIIT, NAT64
    // and IPv6 addresses.
    let rows = read_golden("review_mask.txt")?;
    check(rows.len() == 48, "review_mask.txt is complete")?;
    for row in &rows {
        let [s, bits, want_scrubbed, want_four] = fields::<4>(row)?;
        let s = text(s)?;
        let bits: i64 = text(bits)?.parse()?;
        let mut scrubbed = Vec::new();
        let mut four = Vec::new();
        for ones in -1..=129 {
            scrubbed.push(scrub_ip(s, ones, bits));
            let short = net::parse_ip(s).and_then(|ip| ip.to4());
            four.push(ip_string(
                net::mask_ip(short.as_ref(), cidr_mask(ones, bits).as_ref()).as_ref(),
            ));
        }
        let what = format!("{s} /{bits}");
        check_eq(scrubbed.join(",").as_bytes(), want_scrubbed, &what)?;
        check_eq(four.join(",").as_bytes(), want_four, &what)?;
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
    // The first and last address of each network and their neighbours, written in dotted,
    // IPv4-mapped (lower and upper case, uncompressed, hex), IPv4-compatible, NAT64,
    // compressed and expanded forms: iputil.ParseIP's version, IPNet.Contains for each network,
    // and PublicNetworkIPValidator.IsValid with the config defaults and with mapped networks.
    let rows = read_golden("review_boundaries.txt")?;
    check(rows.len() > 400, "review_boundaries.txt is complete")?;
    let all = nets(&BOUNDARY_NETWORKS)?;
    let default = PublicNetworkIpValidator {
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
    };
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
        check_eq(ip_string(ip.as_ref()).as_bytes(), want_string, s)?;
        let valid = |v: &PublicNetworkIpValidator| ip.is_some_and(|ip| v.is_valid(&ip, ver));
        check_eq(valid(&default).to_string().as_bytes(), want_default, s)?;
        check_eq(valid(&mapped).to_string().as_bytes(), want_mapped, s)?;
        let bits: String = all
            .iter()
            .map(|n| {
                if ip.is_some_and(|ip| n.contains(&ip)) {
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
    fn first_value(&self, _canonical_key: &str) -> Option<&[u8]> {
        None
    }
}

/// The `SocketAddr` of a golden row: a 4-byte address is `V4`, a 16-byte one `V6`.
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
    // (*net.TCPAddr).String() for accepted peers, IPv4-mapped ones of a dual-stack listener
    // included, and the address FindIP derives from that text.
    let rows = read_golden("review_remoteaddr.txt")?;
    check(rows.len() > 60, "review_remoteaddr.txt is complete")?;
    let v4 = VersionIpValidator {
        version: IpVersion::V4,
    };
    let v6 = VersionIpValidator {
        version: IpVersion::V6,
    };
    for row in &rows {
        let [ip_hex_text, port, zone, want_text, want_found, want_ver] = fields::<6>(row)?;
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
        let got = tcp_addr_string(&addr);
        check_eq(got.as_bytes(), want_text, ip_hex_text)?;
        let found = find_ip(&NoHeaders, &got, &v4).or_else(|| find_ip(&NoHeaders, &got, &v6));
        check_eq(
            ip_string(found.map(|(found_ip, _)| found_ip).as_ref()).as_bytes(),
            want_found,
            &got,
        )?;
        check_eq(
            found
                .map_or(IpVersion::Unknown, |(_, v)| v)
                .as_int()
                .to_string()
                .as_bytes(),
            want_ver,
            &got,
        )?;
    }
    Ok(())
}

#[test]
fn display_of_a_mapped_socket_addr_is_not_go_remote_addr() -> TestResult {
    // The trap tcp_addr_string avoids: std writes the IPv4 peer of a dual-stack listener in
    // brackets, which iputil.ParseIP then calls IPv6.
    let addr = SocketAddr::V6(SocketAddrV6::new(
        Ipv4Addr::new(8, 8, 8, 8).to_ipv6_mapped(),
        80,
        0,
        0,
    ));
    check_eq(
        addr.to_string().as_str(),
        "[::ffff:8.8.8.8]:80",
        "std Display",
    )?;
    check_eq(
        tcp_addr_string(&addr).as_str(),
        "8.8.8.8:80",
        "Go TCPAddr.String",
    )?;
    let any = VersionIpValidator {
        version: IpVersion::V6,
    };
    check(
        find_ip(&NoHeaders, &addr.to_string(), &any).is_some(),
        "the bracketed text is IPv6",
    )?;
    check(
        find_ip(&NoHeaders, &tcp_addr_string(&addr), &any).is_none(),
        "Go's text is IPv4",
    )?;
    check_eq(
        join_host_port("::1", "80").as_str(),
        "[::1]:80",
        "JoinHostPort v6",
    )?;
    check_eq(
        join_host_port("1.2.3.4", "").as_str(),
        "1.2.3.4:",
        "JoinHostPort v4",
    )?;
    check_eq(
        join_host_port("", "80").as_str(),
        ":80",
        "JoinHostPort empty",
    )
}
