//! Go: `net/netip.ParseAddr`, as far as `url.parseHost` needs it: whether a bracketed host is
//! an IP address, whether it is IPv4, and the error text when it is neither.

use std::{error::Error as StdError, fmt};

use crate::strconv::quote_bytes;

/// Go: `netip.parseAddrError`: `ParseAddr("in"): msg` with an optional `(at "rest")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseAddrError {
    input: Vec<u8>,
    msg: &'static str,
    at: Vec<u8>,
}

impl ParseAddrError {
    fn new(input: &[u8], msg: &'static str, at: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            msg,
            at: at.to_vec(),
        }
    }
}

impl fmt::Display for ParseAddrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParseAddr({}): {}", quote_bytes(&self.input), self.msg)?;
        if !self.at.is_empty() {
            write!(f, " (at {})", quote_bytes(&self.at))?;
        }
        Ok(())
    }
}

impl StdError for ParseAddrError {}

/// Go: `netip.ParseAddr`. Returns whether the address is IPv4 (`Addr.Is4`).
pub(super) fn parse_addr(s: &[u8]) -> Result<bool, ParseAddrError> {
    for &c in s {
        match c {
            b'.' => return parse_ipv4_fields(s, 0, s.len()).map(|()| true),
            b':' => return parse_ipv6(s).map(|()| false),
            // Assume an IPv6 address with a zone specifier whose address is missing.
            b'%' => return Err(ParseAddrError::new(s, "missing IPv6 address", b"")),
            _ => {}
        }
    }
    Err(ParseAddrError::new(s, "unable to parse IP", b""))
}

/// Go: `parseIPv4Fields(in, off, end, fields)` (the field values are not needed).
fn parse_ipv4_fields(input: &[u8], off: usize, end: usize) -> Result<(), ParseAddrError> {
    let s = input.get(off..end).unwrap_or_default();
    let err = |msg, at: &[u8]| ParseAddrError::new(input, msg, at);
    let mut val = 0_u32;
    let mut pos = 0;
    let mut dig_len = 0;
    for (i, &c) in s.iter().enumerate() {
        let rest = s.get(i..).unwrap_or_default();
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(err("IPv4 field has octet with leading zero", b""));
            }
            val = val * 10 + u32::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(err("IPv4 field has value >255", b""));
            }
        } else if c == b'.' {
            // .1.2.3 or 1.2.3. or 1..2.3
            if i == 0 || i + 1 == s.len() || i.checked_sub(1).and_then(|p| s.get(p)) == Some(&b'.')
            {
                return Err(err("IPv4 field must have at least one digit", rest));
            }
            // 1.2.3.4.5
            if pos == 3 {
                return Err(err("IPv4 address too long", b""));
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(err("unexpected character", rest));
        }
    }
    if pos < 3 {
        return Err(err("IPv4 address too short", b""));
    }
    Ok(())
}

/// The hex digits of one IPv6 group at the start of `s` (Go: the inner loop of `parseIPv6`).
/// Returns how many there are.
fn hex_group(s: &[u8]) -> Result<usize, &'static str> {
    let mut off = 0;
    let mut acc = 0_u32;
    while let Some(&c) = s.get(off) {
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        };
        acc = (acc << 4) + u32::from(digit);
        if off > 3 {
            return Err("each group must have 4 or less digits");
        }
        if acc > u32::from(u16::MAX) {
            return Err("IPv6 field has value >=2^16");
        }
        off += 1;
    }
    if off == 0 {
        return Err("each colon-separated field must have at least one digit");
    }
    Ok(off)
}

/// Go: `parseIPv6`.
fn parse_ipv6(input: &[u8]) -> Result<(), ParseAddrError> {
    let err = |msg, at: &[u8]| ParseAddrError::new(input, msg, at);
    // Split off the zone.
    let (mut s, zone_len) = match input.iter().position(|&c| c == b'%') {
        Some(i) => {
            let zone = input.get(i + 1..).unwrap_or_default();
            if zone.is_empty() {
                // Not allowed to have an empty zone if explicitly specified.
                return Err(err("zone must be a non-empty string", b""));
            }
            (input.get(..i).unwrap_or_default(), zone.len() + 1)
        }
        None => (input, 0),
    };
    let mut ellipsis: Option<usize> = None;
    // Might have leading ellipsis, or be only an ellipsis.
    if let Some(after) = s.strip_prefix(b"::") {
        ellipsis = Some(0);
        s = after;
        if s.is_empty() {
            return Ok(());
        }
    }
    // Loop, parsing hex numbers followed by colon.
    let mut i = 0;
    while i < 16 {
        let off = hex_group(s).map_err(|msg| err(msg, s))?;
        // If followed by dot, might be in trailing IPv4.
        if s.get(off) == Some(&b'.') {
            if ellipsis.is_none() && i != 12 {
                return Err(err(
                    "embedded IPv4 address must replace the final 2 fields of the address",
                    s,
                ));
            }
            if i + 4 > 16 {
                return Err(err(
                    "too many hex fields to fit an embedded IPv4 at the end of the address",
                    s,
                ));
            }
            let end = input.len() - zone_len;
            parse_ipv4_fields(input, end - s.len(), end)?;
            s = b"";
            i += 4;
            break;
        }
        i += 2;
        s = s.get(off..).unwrap_or_default();
        let Some((&first, after)) = s.split_first() else {
            break;
        };
        // Otherwise must be followed by colon and more.
        if first != b':' {
            return Err(err("unexpected character, want colon", s));
        }
        if after.is_empty() {
            return Err(err("colon must be followed by more characters", s));
        }
        s = after;
        if let Some(rest) = s.strip_prefix(b":") {
            if ellipsis.is_some() {
                return Err(err("multiple :: in address", s));
            }
            ellipsis = Some(i);
            s = rest;
            if s.is_empty() {
                break;
            }
        }
    }
    if !s.is_empty() {
        return Err(err("trailing garbage after address", s));
    }
    if i < 16 {
        if ellipsis.is_none() {
            return Err(err("address string too short", b""));
        }
    } else if ellipsis.is_some() {
        // Ellipsis must represent at least one 0 group.
        return Err(err(
            "the :: must expand to at least one field of zeros",
            b"",
        ));
    }
    Ok(())
}
