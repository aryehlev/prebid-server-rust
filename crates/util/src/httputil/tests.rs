//! Ports of `util/httputil/httputil_test.go` and replays of the `httputil` goldens.

use std::{
    error::Error,
    fs,
    net::{IpAddr, SocketAddr},
    str,
};

use pbs_devtools::repo::go_root;

use super::{
    ContentEncoding, PIXEL_1X1_PNG, RequestHeaders, TRUE_CLIENT_IP, X_FORWARDED_FOR, X_REAL_IP,
    find_ip,
};
use crate::{
    golden::{TestResult, check, check_eq, fields, hex, read_golden, text},
    iputil::{
        IpValidator, IpVersion, PublicNetworkIpValidator,
        net::{self, ip_string, parse_cidr},
        parse_ip,
    },
};

/// Go: `hardcodedResponseIPValidator`.
struct Hardcoded(bool);

impl IpValidator for Hardcoded {
    fn is_valid(&self, _ip: &IpAddr, _ver: IpVersion) -> bool {
        self.0
    }
}

/// The peer whose `Request.RemoteAddr` Go's server writes as `remote`, or `None` when no peer
/// has that text (Go's `SplitHostPort` or `ParseIP` then fails on it, or it names a Unix
/// socket).
pub(super) fn peer(remote: &str) -> Option<SocketAddr> {
    let addr: SocketAddr = remote.parse().ok()?;
    // Go writes an IPv4-mapped peer in dotted decimal, without brackets.
    let mapped = matches!(addr, SocketAddr::V6(a) if a.ip().to_ipv4_mapped().is_some());
    (addr.to_string() == remote && !mapped).then_some(addr)
}

/// Remote address texts of the goldens that Go's server never writes, whose host Go's
/// `SplitHostPort` and `ParseIP` still read as an address. No peer has them, so where the
/// headers give no address the rows that use them have no Rust counterpart.
pub(super) const UNREACHABLE_REMOTES: [&str; 6] = [
    "6.6.6.6:",
    "8.8.8.8:",
    "[8.8.8.8]:80",
    "[::ffff:5.5.5.5]:5",
    "[::ffff:8.8.8.8]:80",
    "[::ffff:10.0.0.1]:80",
];

/// A case of Go's `TestFindIP`.
struct FindIpCase {
    description: &'static str,
    true_client_ip: &'static str,
    x_forwarded_for: &'static str,
    x_real_ip: &'static str,
    remote_addr: &'static str,
    validator: bool,
    expected: Option<(&'static str, IpVersion)>,
}

impl FindIpCase {
    /// A case where every header and the remote address are set.
    const fn all(
        description: &'static str,
        headers: [&'static str; 4],
        validator: bool,
        expected: Option<(&'static str, IpVersion)>,
    ) -> Self {
        let [true_client_ip, x_forwarded_for, x_real_ip, remote_addr] = headers;
        Self {
            description,
            true_client_ip,
            x_forwarded_for,
            x_real_ip,
            remote_addr,
            validator,
            expected,
        }
    }
}

const V4_HEADERS: [&str; 4] = ["1.1.1.1", "2.2.2.2, 3.3.3.3", "4.4.4.4", "5.5.5.5:5"];
const V6_HEADERS: [&str; 4] = ["1111::", "2222::, 3333::", "4444::", "[5555::]:5"];

/// Go: the cases of `TestFindIP`.
const FIND_IP_CASES: [FindIpCase; 21] = [
    FindIpCase::all("No Address", ["", "", "", ""], false, None),
    FindIpCase::all("False Validator - IPv4", V4_HEADERS, false, None),
    FindIpCase::all(
        "False Validator - IPv6",
        ["1111::", "2222::, 3333::", "4444::", "[5555::]:5]"],
        false,
        None,
    ),
    FindIpCase::all(
        "True Validator - IPv4 - True Client IP",
        V4_HEADERS,
        true,
        Some(("1.1.1.1", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - True Client IP - Ignore Whitespace",
        ["   1.1.1.1 ", "2.2.2.2, 3.3.3.3", "4.4.4.4", "5.5.5.5:5"],
        true,
        Some(("1.1.1.1", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - X Forwarded For",
        ["", "2.2.2.2, 3.3.3.3", "4.4.4.4", "5.5.5.5:5"],
        true,
        Some(("2.2.2.2", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - X Forwarded For - Ignore Whitespace",
        ["", "   2.2.2.2, 3.3.3.3 ", "4.4.4.4", "5.5.5.5:5"],
        true,
        Some(("2.2.2.2", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - X Real IP",
        ["", "", "4.4.4.4", "5.5.5.5:5"],
        true,
        Some(("4.4.4.4", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - X Real IP - Ignore Whitespace",
        ["", "", "   4.4.4.4 ", "5.5.5.5:5"],
        true,
        Some(("4.4.4.4", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv4 - Remote Address",
        ["", "", "", "5.5.5.5:80"],
        true,
        Some(("5.5.5.5", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - True Client IP",
        V6_HEADERS,
        true,
        Some(("1111::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - True Client IP - Ignore Whitespace",
        ["   1111:: ", "2222::, 3333::", "4444::", "[5555::]:5"],
        true,
        Some(("1111::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - X Forwarded For",
        ["", "2222::, 3333::", "4444::", "[5555::]:5"],
        true,
        Some(("2222::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - X Forwarded For - Ignore Whitespace",
        ["", "   2222::, 3333:: ", "4444::", "[5555::]:5"],
        true,
        Some(("2222::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - X Real IP",
        ["", "", "4444::", "[5555::]:5"],
        true,
        Some(("4444::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - X Real IP - Ignore Whitespace",
        ["", "", "    4444:: ", "[5555::]:5"],
        true,
        Some(("4444::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - IPv6 - Remote Address",
        ["", "", "", "[5555::]:5"],
        true,
        Some(("5555::", IpVersion::V6)),
    ),
    FindIpCase::all(
        "True Validator - Malformed - All",
        ["malformed", "malformed", "malformed", "malformed"],
        true,
        None,
    ),
    FindIpCase::all(
        "True Validator - Malformed - Some",
        ["malformed", "malformed", "4.4.4.4", "malformed"],
        true,
        Some(("4.4.4.4", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - Malformed - X Forwarded For - IPv4",
        [
            "malformed",
            "malformed, 4.4.4.4, 3333::, malformed",
            "malformed",
            "malformed",
        ],
        true,
        Some(("4.4.4.4", IpVersion::V4)),
    ),
    FindIpCase::all(
        "True Validator - Malformed - X Forwarded For - IPv6",
        [
            "malformed",
            "malformed, 3333::, 4.4.4.4, malformed",
            "malformed",
            "malformed",
        ],
        true,
        Some(("3333::", IpVersion::V6)),
    ),
];

#[test]
fn find_ip_go_cases() -> TestResult {
    // Go: TestFindIP, with the headers added to an http.Header as the Go test does.
    for case in &FIND_IP_CASES {
        let mut headers = RawHeaders::default();
        for (key, value) in [
            ("True-Client-IP", case.true_client_ip),
            ("X-Forwarded-For", case.x_forwarded_for),
            ("X-Real-IP", case.x_real_ip),
        ] {
            if !value.is_empty() {
                headers.add(key, value.as_bytes());
            }
        }
        let remote = peer(case.remote_addr);
        let got = find_ip(&headers, remote.as_ref(), &Hardcoded(case.validator));
        let want = match case.expected {
            Some((ip, ver)) => Some((net::parse_ip(ip).ok_or("bad expected IP")?, ver)),
            None => None,
        };
        check_eq(&got, &want, case.description)?;
    }
    Ok(())
}

/// Request headers with byte values in insertion order, as Go's `http.Header` holds them.
#[derive(Default)]
pub(super) struct RawHeaders(Vec<(String, Vec<u8>)>);

impl RawHeaders {
    /// Go: `Header.Add`.
    pub(super) fn add(&mut self, key: &str, value: &[u8]) {
        self.0.push((key.to_owned(), value.to_vec()));
    }
}

impl RequestHeaders for RawHeaders {
    fn first_value(&self, name: &str) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }
}

/// Go's 16-byte `net.IP` in hex, or `nil`.
pub(super) fn go_hex(ip: Option<IpAddr>) -> String {
    ip.map_or_else(
        || "nil".to_owned(),
        |ip| {
            let b = match ip {
                IpAddr::V4(a) => a.to_ipv6_mapped().octets(),
                IpAddr::V6(a) => a.octets(),
            };
            hex(&b)
        },
    )
}

pub(super) fn default_validator() -> Result<PublicNetworkIpValidator, Box<dyn Error>> {
    let nets = |cidrs: &[&str]| -> Result<Vec<_>, Box<dyn Error>> {
        cidrs.iter().map(|s| Ok(parse_cidr(s)?.1)).collect()
    };
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
fn find_ip_golden() -> TestResult {
    // Go: FindIP over generated header combinations: public, private, IPv4-mapped, zoned and
    // malformed addresses, wrapped in ASCII and Unicode white space and bytes TrimSpace
    // keeps, with a second X-Forwarded-For value that Header.Get ignores.
    let rows = read_golden("httputil_findip.txt")?;
    check(rows.len() > 6000, "httputil_findip.txt is complete")?;
    let default = default_validator()?;
    let empty = PublicNetworkIpValidator::default();
    let mut unreachable = 0_usize;
    for row in &rows {
        let [
            name,
            tci,
            xff,
            second_xff,
            xri,
            remote,
            want_hex,
            want_string,
            want_ver,
        ] = fields::<9>(row)?;
        let validator = match name {
            b"default" => &default,
            b"empty" => &empty,
            _ => return Err(format!("unknown validator {name:?}").into()),
        };
        let mut headers = RawHeaders::default();
        for (key, value) in [
            ("True-Client-IP", tci),
            ("X-Forwarded-For", xff),
            ("X-Forwarded-For", second_xff),
            ("X-Real-IP", xri),
        ] {
            if !value.is_empty() {
                headers.add(key, value);
            }
        }
        let remote = text(remote)?;
        let got = find_ip(&headers, peer(remote).as_ref(), validator);
        let what = format!("{row:?}");
        let ip = got.map(|(ip, _)| ip);
        let ver = got
            .map_or(IpVersion::Unknown, |(_, v)| v)
            .as_int()
            .to_string();
        if UNREACHABLE_REMOTES.contains(&remote) && go_hex(ip).as_bytes() != want_hex {
            unreachable += 1;
            continue;
        }
        check_eq(go_hex(ip).as_bytes(), want_hex, &what)?;
        check_eq(ip_string(ip).as_bytes(), want_string, &what)?;
        check_eq(ver.as_bytes(), want_ver, &what)?;
    }
    check(unreachable < 600, "most rows replay")
}

#[test]
fn split_host_port_golden() -> TestResult {
    // Go: findRemoteAddr's net.SplitHostPort(r.RemoteAddr) and iputil.ParseIP of the host
    // (util/httputil/httputil.go:81). find_ip reads the peer's address, not its text, so the
    // rows replayed are the texts Go's server writes for a peer: Go splits off the peer's
    // port, and its host parses to the address find_ip takes from that peer.
    let rows = read_golden("net_splithostport.txt")?;
    check(rows.len() > 1500, "net_splithostport.txt is complete")?;
    let mut peers = 0_usize;
    for row in &rows {
        let [s, host, port, err] = fields::<4>(row)?;
        let Some(addr) = str::from_utf8(s).ok().and_then(peer) else {
            continue;
        };
        peers += 1;
        let what = String::from_utf8_lossy(s);
        check_eq(err, b"".as_slice(), &what)?;
        check_eq(port, addr.port().to_string().as_bytes(), &what)?;
        let want = parse_ip(text(host)?);
        let got = find_ip(&RawHeaders::default(), Some(&addr), &Hardcoded(true));
        check_eq(&got, &want, &what)?;
    }
    check_eq(&peers, &4, "the rows that are a peer's text")
}

#[test]
fn misc_golden() -> TestResult {
    // Go: the canonical header keys, ContentEncoding.Normalize and Pixel1x1PNG. The keys are
    // compared without regard to ASCII case, as RequestHeaders compares them. Normalize keeps
    // std's lower case for U+0130 (the gzip.content-encoding-lower ledger entry).
    let rows = read_golden("httputil_misc.txt")?;
    let mut pixel_rows = 0;
    let mut dotted_i_rows = 0;
    for row in &rows {
        match row.first().map(Vec::as_slice) {
            Some(b"key") => {
                let [_, name, want] = fields::<3>(row)?;
                let name = text(name)?;
                check(
                    [TRUE_CLIENT_IP, X_FORWARDED_FOR, X_REAL_IP]
                        .iter()
                        .any(|key| key.as_bytes() == want && key.eq_ignore_ascii_case(name)),
                    "a FindIP header key",
                )?;
            }
            Some(b"normalize") => {
                let [_, s, want, want_gzip] = fields::<4>(row)?;
                // A Rust string holds valid UTF-8 only; the lossy conversion of a header value
                // gives the U+FFFD that Go's ToLower writes for an invalid byte.
                let encoding = ContentEncoding(String::from_utf8_lossy(s).into_owned());
                let what = encoding.as_str().to_owned();
                let is_gzip: &[u8] = if encoding.is_gzip() {
                    b"true"
                } else {
                    b"false"
                };
                if what.contains('\u{130}') {
                    let ledgered = String::from_utf8_lossy(want).replace('i', "i\u{307}");
                    check_eq(encoding.normalize().as_str(), ledgered.as_str(), &what)?;
                    check_eq(is_gzip, b"false".as_slice(), &what)?;
                    dotted_i_rows += 1;
                } else {
                    check_eq(encoding.normalize().as_str().as_bytes(), want, &what)?;
                    check_eq(is_gzip, want_gzip, &what)?;
                }
            }
            Some(b"pixel") => {
                let [_, content_type, content] = fields::<3>(row)?;
                check_eq(
                    PIXEL_1X1_PNG.content_type.as_bytes(),
                    content_type,
                    "pixel type",
                )?;
                check_eq(hex(PIXEL_1X1_PNG.content).as_bytes(), content, "pixel")?;
                pixel_rows += 1;
            }
            other => return Err(format!("unknown misc row {other:?}").into()),
        }
    }
    check_eq(&dotted_i_rows, &3, "the U+0130 rows")?;
    check_eq(&pixel_rows, &1, "one pixel row")
}

#[test]
fn pixel_is_the_go_file() -> TestResult {
    // Go: util/httputil/pixel.go:10, whose //go:embed pixel.png is the image embedded here,
    // read from the Go root.
    let path = go_root()?.join("util").join("httputil").join("pixel.png");
    let go = fs::read(path)?;
    check_eq(PIXEL_1X1_PNG.content, go.as_slice(), "pixel.png")
}
