//! Ports of `util/httputil/httputil_test.go` and replays of the `httputil` goldens.

use std::{error::Error, fs};

use pbs_devtools::repo::go_root;
use pbs_gocompat::http::{Header, canonical_header_key};

use super::{
    ContentEncoding, PIXEL_1X1_PNG, RequestHeaders, TRUE_CLIENT_IP, X_FORWARDED_FOR, X_REAL_IP,
    find_ip, trim_space,
};
use crate::{
    golden::{TestResult, check, check_eq, fields, hex, read_golden, text},
    iputil::{
        IpValidator, IpVersion, PublicNetworkIpValidator,
        net::{self, Ip, ip_string, parse_cidr},
    },
};

/// Go: `hardcodedResponseIPValidator`.
struct Hardcoded(bool);

impl IpValidator for Hardcoded {
    fn is_valid(&self, _ip: &Ip, _ver: IpVersion) -> bool {
        self.0
    }
}

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
        let mut header = Header::new();
        for (key, value) in [
            ("True-Client-IP", case.true_client_ip),
            ("X-Forwarded-For", case.x_forwarded_for),
            ("X-Real-IP", case.x_real_ip),
        ] {
            if !value.is_empty() {
                header.add(key, value);
            }
        }
        let got = find_ip(&header, case.remote_addr, &Hardcoded(case.validator));
        let want = match case.expected {
            Some((ip, ver)) => Some((net::parse_ip(ip).ok_or("bad expected IP")?, ver)),
            None => None,
        };
        check_eq(&got, &want, case.description)?;
    }
    Ok(())
}

/// Request headers with byte values, as Go's `http.Header` holds them, in insertion order.
#[derive(Default)]
struct RawHeaders(Vec<(String, Vec<u8>)>);

impl RawHeaders {
    /// Go: `Header.Add`.
    fn add(&mut self, key: &str, value: &[u8]) {
        self.0.push((canonical_header_key(key), value.to_vec()));
    }
}

impl RequestHeaders for RawHeaders {
    fn first_value(&self, canonical_key: &str) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(k, _)| k == canonical_key)
            .map(|(_, v)| v.as_slice())
    }
}

fn default_validator() -> Result<PublicNetworkIpValidator, Box<dyn Error>> {
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
    // FindIP over generated header combinations: public, private, IPv4-mapped, zoned and
    // malformed addresses, wrapped in ASCII and Unicode white space and bytes TrimSpace
    // keeps, with a second X-Forwarded-For value that Header.Get ignores.
    let rows = read_golden("httputil_findip.txt")?;
    check(rows.len() > 6000, "httputil_findip.txt is complete")?;
    let default = default_validator()?;
    let empty = PublicNetworkIpValidator::default();
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
        let got = find_ip(&headers, text(remote)?, validator);
        let what = format!("{row:?}");
        let ip = got.map(|(ip, _)| ip);
        check_eq(
            ip.map_or_else(|| "nil".to_owned(), |ip| hex(ip.as_slice()))
                .as_bytes(),
            want_hex,
            &what,
        )?;
        check_eq(ip_string(ip.as_ref()).as_bytes(), want_string, &what)?;
        check_eq(
            got.map_or(IpVersion::Unknown, |(_, v)| v)
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
fn misc_golden() -> TestResult {
    // The canonical header keys, ContentEncoding.Normalize and Pixel1x1PNG.
    let rows = read_golden("httputil_misc.txt")?;
    let mut pixel_rows = 0;
    for row in &rows {
        match row.first().map(Vec::as_slice) {
            Some(b"key") => {
                let [_, name, want] = fields::<3>(row)?;
                let canonical = canonical_header_key(text(name)?);
                check_eq(canonical.as_bytes(), want, "CanonicalHeaderKey")?;
                check(
                    [TRUE_CLIENT_IP, X_FORWARDED_FOR, X_REAL_IP].contains(&canonical.as_str()),
                    "a FindIP header key",
                )?;
            }
            Some(b"normalize") => {
                let [_, s, want, want_gzip] = fields::<4>(row)?;
                // A Rust string holds valid UTF-8 only; the lossy conversion of a header value
                // gives the U+FFFD that Go's ToLower writes for an invalid byte.
                let encoding = ContentEncoding(String::from_utf8_lossy(s).into_owned());
                let what = encoding.as_str().to_owned();
                check_eq(encoding.normalize().as_str().as_bytes(), want, &what)?;
                let is_gzip: &[u8] = if encoding.is_gzip() {
                    b"true"
                } else {
                    b"false"
                };
                check_eq(is_gzip, want_gzip, &what)?;
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
    check_eq(&pixel_rows, &1, "one pixel row")
}

#[test]
fn pixel_is_the_go_file() -> TestResult {
    // The embedded image is the one Go embeds with //go:embed pixel.png, in the Go root.
    let path = go_root()?.join("util").join("httputil").join("pixel.png");
    let go = fs::read(path)?;
    check_eq(PIXEL_1X1_PNG.content, go.as_slice(), "pixel.png")
}

#[test]
fn trim_space_matches_go() -> TestResult {
    // strings.TrimSpace: ASCII and Unicode white space, never invalid UTF-8 or U+200B.
    let cases: [(&[u8], &[u8]); 9] = [
        (b" \t\n\x0b\x0c\r1.1.1.1\r\n", b"1.1.1.1"),
        ("\u{a0}\u{2003}1\u{3000}\u{85}".as_bytes(), b"1"),
        ("\u{200b}1".as_bytes(), "\u{200b}1".as_bytes()),
        (b"\xa01 ", b"\xa01"),
        (b" 1\xff ", b"1\xff"),
        (b"", b""),
        (b"   ", b""),
        ("\u{a0}".as_bytes(), b""),
        (b" \xe3\x80", b"\xe3\x80"),
    ];
    for (input, want) in cases {
        check_eq(trim_space(input), want, &String::from_utf8_lossy(input))?;
    }
    Ok(())
}
