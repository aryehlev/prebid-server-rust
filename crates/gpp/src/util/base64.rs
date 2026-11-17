//! The subset of Go's `encoding/base64` that go-gpp uses: `base64.RawURLEncoding` (URL alphabet,
//! no padding, non-strict).
//!
//! The decoder reproduces Go's `Encoding.Decode` exactly, including the quirks that are visible
//! through go-gpp's error messages:
//! - `\r` and `\n` are skipped anywhere in the input;
//! - `=` is an illegal character, because the encoding has no padding;
//! - non-zero trailing bits in the final quantum are accepted (the encoding is not strict);
//! - a final quantum with a single character is illegal, reported at Go's computed offset.
//!
//! Go: `$GOROOT/src/encoding/base64/base64.go` (`decodeQuantum`, `Decode`, `Encode`).

use std::fmt;

/// The URL-safe alphabet. Go: `encodeURL`.
const ENCODE_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Marker for bytes outside the alphabet in [`decode_value`].
const INVALID: u8 = 0xFF;

/// Go's `base64.CorruptInputError`: the input byte offset at which decoding failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CorruptInputError(usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

/// Maps an input byte to its 6-bit value, or [`INVALID`]. Go: `Encoding.decodeMap`.
const fn decode_value(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => INVALID,
    }
}

/// Decodes `src` like Go's `base64.RawURLEncoding.Decode`, returning the decoded bytes or the
/// `CorruptInputError` Go would report.
///
/// Go's fast paths (`assemble64`/`assemble32`) only handle runs of valid characters and produce the
/// same bytes as `decodeQuantum`, so this port runs `decodeQuantum` for every quantum.
pub(super) fn raw_url_decode(src: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    let mut out = Vec::with_capacity(src.len() / 4 * 3 + src.len() % 4 * 6 / 8);
    let mut si = 0usize;
    while si < src.len() {
        // Go: decodeQuantum.
        let mut dbuf = [0u8; 4];
        let mut j = 0usize;
        let mut dlen = 4usize;
        while j < dbuf.len() {
            let Some(&input) = src.get(si) else {
                match j {
                    0 => return Ok(out),
                    // Go: `return si, 0, CorruptInputError(si - j)`.
                    1 => return Err(CorruptInputError(si - j)),
                    _ => {}
                }
                dlen = j;
                break;
            };
            si += 1;

            let value = decode_value(input);
            if value != INVALID {
                if let Some(slot) = dbuf.get_mut(j) {
                    *slot = value;
                }
                j += 1;
                continue;
            }

            if input == b'\n' || input == b'\r' {
                continue;
            }

            // The encoding has no padding character, so every other byte (including '=') is
            // illegal. Go: `if rune(in) != enc.padChar { return si, 0, CorruptInputError(si - 1)
            // }`.
            return Err(CorruptInputError(si - 1));
        }

        let [d0, d1, d2, d3] = dbuf;
        let val = u32::from(d0) << 18 | u32::from(d1) << 12 | u32::from(d2) << 6 | u32::from(d3);
        let [_, b0, b1, b2] = val.to_be_bytes();
        match dlen {
            4 => out.extend_from_slice(&[b0, b1, b2]),
            3 => out.extend_from_slice(&[b0, b1]),
            2 => out.push(b0),
            _ => {}
        }
    }
    Ok(out)
}

/// Encodes `src` like Go's `base64.RawURLEncoding.Encode`.
pub(super) fn raw_url_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len().div_ceil(3) * 4);
    let (chunks, remainder) = src.as_chunks::<3>();
    for &[a, b, c] in chunks {
        push_quantum(&mut out, a, b, c, 4);
    }
    match *remainder {
        [a] => push_quantum(&mut out, a, 0, 0, 2),
        [a, b] => push_quantum(&mut out, a, b, 0, 3),
        _ => {}
    }
    out
}

/// Appends the first `chars` characters of the encoding of the 24-bit group `a b c`.
fn push_quantum(out: &mut Vec<u8>, a: u8, b: u8, c: u8, chars: usize) {
    let val = u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c);
    for shift in [18u32, 12, 6, 0].into_iter().take(chars) {
        let index = usize::try_from((val >> shift) & 0x3F).unwrap_or_default();
        if let Some(&ch) = ENCODE_URL.get(index) {
            out.push(ch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CorruptInputError, raw_url_decode, raw_url_encode};

    #[test]
    fn decode_vectors() {
        let cases: &[(&str, Result<&[u8], usize>)] = &[
            ("", Ok(b"")),
            ("AA", Ok(&[0])),
            ("AAA", Ok(&[0, 0])),
            ("AAAA", Ok(&[0, 0, 0])),
            ("A", Err(0)),
            ("AAAAA", Err(4)),
            ("AA=A", Err(2)),
            ("A\nA", Ok(&[0])),
            ("\nA", Err(1)),
            ("AAAA\n", Ok(&[0, 0, 0])),
            ("AAAAA\n", Err(5)),
            ("_-//", Err(2)),
            ("D_", Ok(&[0x0F])),
            ("ab+c", Err(2)),
        ];
        for (input, want) in cases {
            let got = raw_url_decode(input.as_bytes());
            let want = want.map(<[u8]>::to_vec).map_err(CorruptInputError);
            assert_eq!(got, want, "decode {input:?}");
        }
    }

    #[test]
    fn decode_error_text() {
        let err = raw_url_decode(b"DBA=").err().map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some("illegal base64 data at input byte 3"),
            "error text"
        );
    }

    #[test]
    fn encode_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (&[0xFB], "-w"),
            (&[0xFB, 0xFF], "-_8"),
            (&[0xFB, 0xFF, 0xBF], "-_-_"),
            (b"GPP framework", "R1BQIGZyYW1ld29yaw"),
        ];
        for (input, want) in cases {
            assert_eq!(raw_url_encode(input), want.as_bytes(), "encode {input:?}");
        }
    }
}
