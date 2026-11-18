//! Go `encoding/base64` decoding, as used by go-gdpr and by `encoding/json` for `[]byte` fields.
//!
//! Go's decoder differs from most Rust base64 decoders in ways that are observable here:
//! `\r` and `\n` are skipped anywhere in the input, non-zero trailing bits are accepted (the
//! standard encodings are not strict), and errors report a specific byte offset through
//! [`CorruptInputError`]. This module reproduces `Encoding.Decode` exactly for the two encodings
//! prebid-server needs.

use core::fmt;
use std::error::Error;

/// Go `base64.CorruptInputError`: `illegal base64 data at input byte N`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CorruptInputError(pub usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

impl Error for CorruptInputError {}

/// A base64 encoding with Go's decoding semantics.
///
/// Both supported alphabets share their first 62 digits (`A-Z`, `a-z`, `0-9`) and differ in the
/// last two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    digit62: u8,
    digit63: u8,
    pad_char: Option<u8>,
}

/// Go `base64.StdEncoding` (padded, standard alphabet).
pub const STD_ENCODING: Encoding = Encoding {
    digit62: b'+',
    digit63: b'/',
    pad_char: Some(b'='),
};

/// Go `base64.RawURLEncoding` (unpadded, URL-safe alphabet).
pub const RAW_URL_ENCODING: Encoding = Encoding {
    digit62: b'-',
    digit63: b'_',
    pad_char: None,
};

impl Encoding {
    /// Go: `Encoding.decodeMap`: the 6-bit value of an alphabet byte.
    const fn decode_digit(self, c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            _ if c == self.digit62 => Some(62),
            _ if c == self.digit63 => Some(63),
            _ => None,
        }
    }

    /// Go `Encoding.DecodedLen`: the maximum length of the decoded data for `n` input bytes.
    #[must_use]
    pub const fn decoded_len(&self, n: usize) -> usize {
        if self.pad_char.is_none() {
            // Unpadded data may end with a partial quantum.
            return n / 4 * 3 + n % 4 * 6 / 8;
        }
        n / 4 * 3
    }

    /// Go `Encoding.DecodeString` / `Encoding.Decode`.
    ///
    /// Go also returns the bytes decoded before an error; no caller in go-gdpr or prebid-server
    /// uses them, so they are dropped.
    ///
    /// # Errors
    ///
    /// Returns the [`CorruptInputError`] Go reports for the first invalid input byte.
    pub fn decode(&self, src: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
        let mut out = Vec::with_capacity(self.decoded_len(src.len()));
        let mut si = 0;
        while si < src.len() {
            si = self.decode_quantum(&mut out, src, si)?;
        }
        Ok(out)
    }

    /// Port of `Encoding.decodeQuantum`: decodes up to 4 base64 digits starting at `si` and
    /// returns the index of the next unread input byte.
    ///
    /// Go's `Decode` also has 8- and 4-digit fast paths, but they fall back to this function on
    /// the first invalid digit and produce the same output and errors.
    fn decode_quantum(
        self,
        out: &mut Vec<u8>,
        src: &[u8],
        start: usize,
    ) -> Result<usize, CorruptInputError> {
        let mut dbuf = [0u8; 4];
        let mut dlen = 4;
        let mut si = start;
        let mut trailing_garbage = None;
        let mut j = 0;
        while j < 4 {
            let Some(&input) = src.get(si) else {
                if j == 0 {
                    return Ok(si);
                }
                if j == 1 || self.pad_char.is_some() {
                    return Err(CorruptInputError(si.saturating_sub(j)));
                }
                dlen = j;
                break;
            };
            si += 1;

            if let Some(decoded) = self.decode_digit(input) {
                if let Some(slot) = dbuf.get_mut(j) {
                    *slot = decoded;
                }
                j += 1;
                continue;
            }

            if input == b'\n' || input == b'\r' {
                continue;
            }

            if Some(input) != self.pad_char {
                return Err(CorruptInputError(si - 1));
            }

            // We've reached the end and there's padding.
            match j {
                0 | 1 => return Err(CorruptInputError(si - 1)),
                2 => {
                    // "==" is expected, the first "=" is already consumed.
                    si = skip_newlines(src, si);
                    match src.get(si) {
                        None => return Err(CorruptInputError(src.len())),
                        Some(&next) if Some(next) != self.pad_char => {
                            return Err(CorruptInputError(si - 1));
                        }
                        Some(_) => si += 1,
                    }
                }
                _ => {}
            }

            si = skip_newlines(src, si);
            if si < src.len() {
                trailing_garbage = Some(CorruptInputError(si));
            }
            dlen = j;
            break;
        }

        let val = u32::from(dbuf[0]) << 18
            | u32::from(dbuf[1]) << 12
            | u32::from(dbuf[2]) << 6
            | u32::from(dbuf[3]);
        let [_, b0, b1, b2] = val.to_be_bytes();
        match dlen {
            4 => out.extend_from_slice(&[b0, b1, b2]),
            3 => out.extend_from_slice(&[b0, b1]),
            2 => out.push(b0),
            _ => {}
        }
        trailing_garbage.map_or(Ok(si), Err)
    }
}

fn skip_newlines(src: &[u8], mut si: usize) -> usize {
    while matches!(src.get(si), Some(b'\n' | b'\r')) {
        si += 1;
    }
    si
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{CorruptInputError, Encoding, RAW_URL_ENCODING, STD_ENCODING};
    use crate::test_support::{TestResult, hex_encode, input_bytes, load_cases, str_field};

    #[test]
    fn raw_url_values() -> TestResult {
        check_eq!(
            RAW_URL_ENCODING.decode(b"")?,
            Vec::<u8>::new(),
            "empty input"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"Zm9vYmFy")?,
            b"foobar",
            "full quanta"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"Zm9vYg")?,
            b"foob",
            "partial quantum"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"Zm9v\nYg\r")?,
            b"foob",
            "newlines are skipped"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"QR")?,
            b"A",
            "trailing bits are ignored"
        );
        Ok(())
    }

    #[test]
    fn raw_url_errors() -> TestResult {
        check_eq!(
            RAW_URL_ENCODING.decode(b"A"),
            Err(CorruptInputError(0)),
            "single digit"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"AAAAA"),
            Err(CorruptInputError(4)),
            "dangling digit"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"AA=="),
            Err(CorruptInputError(2)),
            "padding"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"/"),
            Err(CorruptInputError(0)),
            "std alphabet"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"AB\n"),
            Ok(vec![0]),
            "newline after quantum"
        );
        check_eq!(
            RAW_URL_ENCODING.decode(b"AAAAB\n"),
            Err(CorruptInputError(5)),
            "si - j"
        );
        Ok(())
    }

    #[test]
    fn std_errors() -> TestResult {
        check_eq!(
            STD_ENCODING.decode(b"AQI"),
            Err(CorruptInputError(0)),
            "missing padding"
        );
        check_eq!(STD_ENCODING.decode(b"AQ=="), Ok(vec![1]), "padded");
        check_eq!(
            STD_ENCODING.decode(b"AQ="),
            Err(CorruptInputError(3)),
            "short padding"
        );
        check_eq!(
            STD_ENCODING.decode(b"AQ==A"),
            Err(CorruptInputError(4)),
            "trailing data"
        );
        check_eq!(
            STD_ENCODING.decode(b"AQ=A"),
            Err(CorruptInputError(2)),
            "bad padding"
        );
        check_eq!(
            STD_ENCODING.decode(b"A-I="),
            Err(CorruptInputError(1)),
            "url alphabet"
        );
        Ok(())
    }

    /// Renders a decode result the way the Go generator records it.
    fn render(result: Result<Vec<u8>, CorruptInputError>) -> (String, String, usize) {
        match result {
            Ok(bytes) => (String::new(), hex_encode(&bytes), bytes.len()),
            Err(err) => (err.to_string(), String::new(), 0),
        }
    }

    fn expected(case: &Value, key: &str) -> (String, String, usize) {
        let result = case.get(key).unwrap_or(&Value::Null);
        (
            str_field(result, "err").unwrap_or_default().to_owned(),
            str_field(result, "hex").unwrap_or_default().to_owned(),
            result
                .get("len")
                .and_then(Value::as_u64)
                .and_then(|len| usize::try_from(len).ok())
                .unwrap_or_default(),
        )
    }

    /// Replays `testdata/base64_goldens.json`: `RawURLEncoding.DecodeString`, the in-place
    /// `RawURLEncoding.Decode` of go-gdpr's `ParseString`, and `StdEncoding.Decode` as used by
    /// `encoding/json`.
    #[test]
    fn go_goldens() -> TestResult {
        let cases = load_cases("base64_goldens.json")?;
        check!(cases.len() > 1000, "only {} base64 cases", cases.len());
        let encodings: [(&str, Encoding); 3] = [
            ("raw_url", RAW_URL_ENCODING),
            ("raw_url_in_place", RAW_URL_ENCODING),
            ("std", STD_ENCODING),
        ];
        for case in &cases {
            let input = input_bytes(case)?;
            for (key, encoding) in encodings {
                check_eq!(
                    render(encoding.decode(&input)),
                    expected(case, key),
                    "{key} {input:?}"
                );
            }
        }
        Ok(())
    }
}
