//! Go `golang.org/x/net/http/httpguts` and the small `net/http` string helpers built on it.

use std::fmt::Write as _;

use pbs_gocompat::strconv::quote;
use pbs_util::iputil::net::{join_host_port, split_host_port};

use crate::error::HttpError;

/// Go: `httpguts.isTokenTable` (RFC 7230 `tchar`).
const fn is_token_byte(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`'
        | b'|' | b'~' | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}

/// Go: `httpguts.validHostByte`.
const fn is_valid_host_byte(b: u8) -> bool {
    matches!(b,
        b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'!' | b'$' | b'%' | b'&' | b'(' | b')' | b'*'
        | b'+' | b',' | b'-' | b'.' | b':' | b';' | b'=' | b'[' | b'\'' | b']' | b'_' | b'~')
}

/// Go: `httpguts.ValidHeaderFieldName`.
#[must_use]
pub fn valid_header_field_name(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(is_token_byte)
}

/// Go: `httpguts.ValidHeaderFieldValue`: no control bytes other than space and tab.
#[must_use]
pub fn valid_header_field_value(v: &str) -> bool {
    v.bytes()
        .all(|b| !(b < b' ' || b == 0x7F) || b == b' ' || b == b'\t')
}

/// Go: `httpguts.ValidHostHeader`.
#[must_use]
pub fn valid_host_header(h: &str) -> bool {
    h.bytes().all(is_valid_host_byte)
}

/// Go: `net/http.validMethod`.
#[must_use]
pub fn valid_method(method: &str) -> bool {
    valid_header_field_name(method)
}

fn trim_ows(x: &str) -> &str {
    x.trim_matches(|c| c == ' ' || c == '\t')
}

/// Go: `httpguts.tokenEqual`: ASCII case-insensitive, non-ASCII never equal.
fn token_equal(t1: &str, t2: &str) -> bool {
    t1.len() == t2.len()
        && t1.is_ascii()
        && t1
            .bytes()
            .zip(t2.bytes())
            .all(|(a, b)| a.eq_ignore_ascii_case(&b))
}

/// Go: `httpguts.HeaderValuesContainsToken`.
#[must_use]
pub fn header_values_contains_token(values: &[String], token: &str) -> bool {
    values
        .iter()
        .any(|v| v.split(',').any(|t| token_equal(trim_ows(t), token)))
}

/// Go: `net/http.hasToken`: whether `v` contains `token` as a whole word, ignoring ASCII case.
#[must_use]
pub fn has_token(v: &str, token: &str) -> bool {
    if token.len() > v.len() || token.is_empty() {
        return false;
    }
    if v == token {
        return true;
    }
    let vb = v.as_bytes();
    let boundary =
        |i: usize| -> bool { vb.get(i).is_none_or(|&b| matches!(b, b' ' | b',' | b'\t')) };
    (0..=vb.len() - token.len()).any(|sp| {
        let Some(window) = vb.get(sp..sp + token.len()) else {
            return false;
        };
        window.eq_ignore_ascii_case(token.as_bytes())
            && (sp == 0 || boundary(sp - 1))
            && boundary(sp + token.len())
    })
}

/// Go: `net/http.hasPort`.
#[must_use]
pub fn has_port(s: &str) -> bool {
    match (s.rfind(':'), s.rfind(']')) {
        (Some(c), Some(b)) => c > b,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Go: `net/http.removeEmptyPort`.
#[must_use]
pub fn remove_empty_port(host: &str) -> &str {
    if has_port(host) {
        host.strip_suffix(':').unwrap_or(host)
    } else {
        host
    }
}

/// Go: `net/http.removeZone`: drops an IPv6 zone from a bracketed host.
#[must_use]
pub fn remove_zone(host: &str) -> String {
    if !host.starts_with('[') {
        return host.to_owned();
    }
    let Some(i) = host.rfind(']') else {
        return host.to_owned();
    };
    let (before, after) = host.split_at(i);
    before.rfind('%').map_or_else(
        || host.to_owned(),
        |j| {
            let (kept, _zone) = before.split_at(j);
            format!("{kept}{after}")
        },
    )
}

/// Go: `net/http.stringContainsCTLByte`.
#[must_use]
pub fn string_contains_ctl_byte(s: &str) -> bool {
    s.bytes().any(|b| b < b' ' || b == 0x7F)
}

/// Go: `httpguts.PunycodeHostPort`: the host with each non-ASCII label Punycode-encoded.
///
/// # Errors
///
/// Go's `idna: invalid label` error when a label cannot be encoded.
pub fn punycode_host_port(v: &str) -> Result<String, HttpError> {
    if v.is_ascii() {
        return Ok(v.to_owned());
    }
    let (host, port) = split_host_port(v).unwrap_or((v, ""));
    let host = idna_to_ascii(host)?;
    if port.is_empty() {
        Ok(host)
    } else {
        Ok(join_host_port(&host, port))
    }
}

/// Go: `idna.ToASCII` with the Punycode profile (no mapping): each non-ASCII label becomes
/// `xn--` plus its Punycode encoding.
///
/// # Errors
///
/// Go's `idna: invalid label %q` when the encoding overflows.
pub fn idna_to_ascii(s: &str) -> Result<String, HttpError> {
    if s.is_ascii() {
        return Ok(s.to_owned());
    }
    let labels: Result<Vec<String>, HttpError> = s
        .split('.')
        .map(|label| {
            if label.is_ascii() {
                Ok(label.to_owned())
            } else {
                punycode_encode("xn--", label)
            }
        })
        .collect();
    Ok(labels?.join("."))
}

/// Go: `idna.encode` (RFC 3492).
fn punycode_encode(prefix: &str, label: &str) -> Result<String, HttpError> {
    const BASE: i32 = 36;
    const TMIN: i32 = 1;
    const TMAX: i32 = 26;
    let puny_err = || HttpError::Message(format!("idna: invalid label {}", quote(label)));
    let encode_digit = |value: i32| -> char {
        let digit = u8::try_from(value).unwrap_or(0);
        if digit < 26 {
            char::from(b'a' + digit)
        } else {
            char::from(b'0' + (digit - 26))
        }
    };
    let adapt = |delta: i32, num_points: i32, first_time: bool| -> i32 {
        let mut delta = if first_time { delta / 700 } else { delta / 2 };
        delta += delta / num_points;
        let mut step = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            step += BASE;
        }
        step + (BASE - TMIN + 1) * delta / (delta + 38)
    };
    let runes: Vec<i32> = label
        .chars()
        .map(|c| i32::try_from(u32::from(c)).unwrap_or(0))
        .collect();
    let mut output = String::from(prefix);
    let mut basic = 0_i32;
    let mut remaining = 0_i32;
    for &rune in &runes {
        if rune < 0x80 {
            basic += 1;
            output.push(char::from(u8::try_from(rune).unwrap_or(0)));
        } else {
            remaining += 1;
        }
    }
    let mut handled = basic;
    if basic > 0 {
        output.push('-');
    }
    let (mut delta, mut point, mut bias) = (0_i32, 128_i32, 72_i32);
    while remaining != 0 {
        let next = runes
            .iter()
            .copied()
            .filter(|&rune| rune >= point)
            .min()
            .unwrap_or(i32::MAX);
        delta = (next - point)
            .checked_mul(handled + 1)
            .and_then(|add| delta.checked_add(add))
            .ok_or_else(puny_err)?;
        point = next;
        for &rune in &runes {
            if rune < point {
                delta = delta.checked_add(1).ok_or_else(puny_err)?;
                continue;
            }
            if rune > point {
                continue;
            }
            let mut rest = delta;
            let mut step = BASE;
            loop {
                let threshold = if step <= bias {
                    TMIN
                } else if step >= bias + TMAX {
                    TMAX
                } else {
                    step - bias
                };
                if rest < threshold {
                    break;
                }
                output.push(encode_digit(
                    threshold + (rest - threshold) % (BASE - threshold),
                ));
                rest = (rest - threshold) / (BASE - threshold);
                step += BASE;
            }
            output.push(encode_digit(rest));
            bias = adapt(delta, handled + 1, handled == basic);
            delta = 0;
            handled += 1;
            remaining -= 1;
        }
        delta += 1;
        point += 1;
    }
    Ok(output)
}

/// Go: `net/http.hexEscapeNonASCII`.
#[must_use]
pub fn hex_escape_non_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b >= 0x80 {
            let _infallible = write!(out, "%{b:x}");
        } else {
            out.push(char::from(b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    #[test]
    fn tokens_and_hosts() -> Result<(), String> {
        let close = ["keep-alive, Close".to_owned()];
        ensure_eq(
            &header_values_contains_token(&close, "close"),
            &true,
            "close",
        )?;
        ensure_eq(&has_token("Upgrade, close", "close"), &true, "hasToken")?;
        ensure_eq(&has_token("closed", "close"), &false, "hasToken word")?;
        ensure_eq(&valid_host_header("a b"), &false, "space host")?;
        ensure_eq(
            &remove_zone("[fe80::1%en0]:8080"),
            &"[fe80::1]:8080".to_owned(),
            "zone",
        )?;
        ensure_eq(
            &punycode_host_port("bücher.example:8080").map_err(|e| e.to_string())?,
            &"xn--bcher-kva.example:8080".to_owned(),
            "punycode",
        )
    }
}
