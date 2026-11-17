//! Replays of the `strings.TrimSpace` and `FindIP` goldens written by `testdata/go/review`.

use std::error::Error;

use pbs_gocompat::{http::canonical_header_key, unicode};

use super::{RequestHeaders, find_ip, trim_space};
use crate::{
    golden::{Row, Sweep, TestResult, check, check_eq, fields, hex, read_golden, text},
    iputil::{
        IpValidator, IpVersion, PublicNetworkIpValidator, VersionIpValidator,
        net::{IpNet, ip_string, parse_cidr},
    },
};

/// The splitmix64 step of the review generator.
const fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The fields after the first of the row whose first field is `name`.
fn named_row<'a>(rows: &'a [Row], name: &[u8]) -> Result<&'a [Vec<u8>], Box<dyn Error>> {
    rows.iter()
        .find(|r| r.first().map(Vec::as_slice) == Some(name))
        .and_then(|r| r.get(1..))
        .ok_or_else(|| format!("no {} row", String::from_utf8_lossy(name)).into())
}

#[test]
fn review_trim_golden() -> TestResult {
    // strings.TrimSpace around every unicode.IsSpace rune, look-alikes it keeps (U+200B,
    // U+FEFF, U+180E, U+00AD, U+FFFD) and broken UTF-8 (lone and truncated sequences,
    // overlong forms, surrogates, code points above U+10FFFF).
    let rows = read_golden("review_trim.txt")?;
    check(rows.len() > 5000, "review_trim.txt is complete")?;

    // The rune set itself: Go's unicode.IsSpace, which trim_space relies on.
    let spaces = named_row(&rows, b"spaces")?
        .iter()
        .map(|f| Ok(u32::from_str_radix(text(f)?, 16)?))
        .collect::<Result<Vec<u32>, Box<dyn Error>>>()?;
    let ours: Vec<u32> = (0..=u32::from(char::MAX))
        .filter_map(char::from_u32)
        .filter(|&c| unicode::is_space(c))
        .map(u32::from)
        .collect();
    check_eq(&ours, &spaces, "unicode.IsSpace runes")?;

    let mut cases = 0_usize;
    for row in &rows {
        if !matches!(
            row.first().map(Vec::as_slice),
            Some(b"spaces" | b"frags" | b"sweep")
        ) {
            let [input, want] = fields::<2>(row)?;
            check_eq(trim_space(input), want, &String::from_utf8_lossy(input))?;
            cases += 1;
        }
    }
    check(cases > 5000, "trim cases")?;

    let frags = named_row(&rows, b"frags")?;
    let want = named_row(&rows, b"sweep")?
        .first()
        .ok_or("no sweep checksum")?;
    let len = u64::try_from(frags.len())?;
    let mut state = 0x5EED_7001_u64;
    let mut sweep = Sweep::default();
    let mut s = Vec::new();
    for _ in 0..100_000 {
        s.clear();
        for _ in 0..next(&mut state) % 7 {
            let i = usize::try_from(next(&mut state) % len)?;
            s.extend_from_slice(frags.get(i).ok_or("fragment index")?);
        }
        sweep.add(trim_space(&s));
    }
    check_eq(sweep.sum().as_bytes(), want.as_slice(), "TrimSpace sweep")
}

/// Byte-valued request headers in insertion order, as Go's `http.Header` holds them.
#[derive(Default)]
struct RawHeaders(Vec<(String, Vec<u8>)>);

impl RequestHeaders for RawHeaders {
    fn first_value(&self, canonical_key: &str) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(k, _)| k == canonical_key)
            .map(|(_, v)| v.as_slice())
    }
}

fn nets(cidrs: &[&str]) -> Result<Vec<IpNet>, Box<dyn Error>> {
    cidrs.iter().map(|s| Ok(parse_cidr(s)?.1)).collect()
}

/// The `(hex, String, version)` text of a `FindIP` result.
fn result_text<V: IpValidator>(
    headers: &RawHeaders,
    remote: &str,
    v: &V,
) -> (String, String, String) {
    let got = find_ip(headers, remote, v);
    let ip = got.map(|(ip, _)| ip);
    (
        ip.map_or_else(|| "nil".to_owned(), |ip| hex(ip.as_slice())),
        ip_string(ip.as_ref()),
        got.map_or(IpVersion::Unknown, |(_, ver)| ver)
            .as_int()
            .to_string(),
    )
}

#[test]
fn review_find_ip_golden() -> TestResult {
    // FindIP over X-Forwarded-For lists of 1 to 10 entries mixing public, private, mapped,
    // zoned, ported, bracketed, CIDR and junk entries, empty entries and padding with every
    // kind of space, with separators ",", ", ", " ,", ",," and ",\t", plus True-Client-IP,
    // X-Real-IP and remote addresses, for the default, mapped-network and version validators.
    // Each request is replayed with absent headers and with present but empty ones.
    let rows = read_golden("review_findip.txt")?;
    check(rows.len() > 1600, "review_findip.txt is complete")?;
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
        ipv4_private_networks: nets(&["::ffff:10.0.0.0/104"])?,
        ipv6_private_networks: nets(&["::ffff:0:0/96", "fc00::/7"])?,
    };
    let v4 = VersionIpValidator {
        version: IpVersion::V4,
    };
    let v6 = VersionIpValidator {
        version: IpVersion::V6,
    };
    for row in &rows {
        let [tci, xff, xri, remote] = fields::<4>(row)?;
        let remote = text(remote)?;
        let want: Vec<&[u8]> = row.iter().skip(4).map(Vec::as_slice).collect();
        check_eq(&want.len(), &12, "three fields per validator")?;
        for keep_empty in [false, true] {
            let mut headers = RawHeaders::default();
            for (key, value) in [
                ("True-Client-IP", tci),
                ("X-Forwarded-For", xff),
                ("X-Real-IP", xri),
            ] {
                if keep_empty || !value.is_empty() {
                    headers.0.push((canonical_header_key(key), value.to_vec()));
                }
            }
            let got = [
                result_text(&headers, remote, &default),
                result_text(&headers, remote, &mapped),
                result_text(&headers, remote, &v4),
                result_text(&headers, remote, &v6),
            ];
            let got: Vec<&[u8]> = got
                .iter()
                .flat_map(|(h, s, v)| [h.as_bytes(), s.as_bytes(), v.as_bytes()])
                .collect();
            check_eq(&got, &want, &format!("{row:?} keep_empty={keep_empty}"))?;
        }
    }
    Ok(())
}
