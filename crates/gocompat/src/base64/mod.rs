//! Go: `encoding/base64`.
//!
//! The four standard encodings ([`STD_ENCODING`], [`URL_ENCODING`], [`RAW_STD_ENCODING`],
//! [`RAW_URL_ENCODING`]) with Go's decoder rules: `\r` and `\n` are skipped anywhere, padded
//! encodings require exact padding, unpadded ones reject `=`, non-zero trailing bits are accepted
//! (Go's default, non-strict mode), and errors are [`CorruptInputError`]:
//! `illegal base64 data at input byte N`.

use std::{error::Error as StdError, fmt};

/// Go: `base64.CorruptInputError`: the offset of the first byte that could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorruptInputError(pub usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

impl StdError for CorruptInputError {}

/// Go: `base64.Encoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    alphabet: &'static [u8; 64],
    pad: Option<u8>,
}

const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Go: `base64.StdEncoding` (RFC 4648, padded).
pub const STD_ENCODING: Encoding = Encoding {
    alphabet: STD_ALPHABET,
    pad: Some(b'='),
};
/// Go: `base64.URLEncoding` (URL-safe alphabet, padded).
pub const URL_ENCODING: Encoding = Encoding {
    alphabet: URL_ALPHABET,
    pad: Some(b'='),
};
/// Go: `base64.RawStdEncoding` (no padding).
pub const RAW_STD_ENCODING: Encoding = Encoding {
    alphabet: STD_ALPHABET,
    pad: None,
};
/// Go: `base64.RawURLEncoding` (URL-safe alphabet, no padding).
pub const RAW_URL_ENCODING: Encoding = Encoding {
    alphabet: URL_ALPHABET,
    pad: None,
};

impl Encoding {
    fn decode_byte(&self, c: u8) -> Option<u8> {
        self.alphabet
            .iter()
            .position(|&a| a == c)
            .and_then(|p| u8::try_from(p).ok())
    }

    fn symbol(&self, v: u32) -> char {
        let i = usize::try_from(v & 0x3F).unwrap_or(0);
        char::from(self.alphabet.get(i).copied().unwrap_or(b'A'))
    }

    /// Go: `Encoding.EncodeToString`.
    #[must_use]
    pub fn encode_to_string(&self, src: &[u8]) -> String {
        let mut out = String::with_capacity(src.len().div_ceil(3) * 4);
        for chunk in src.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| u32::from(chunk.get(i).copied().unwrap_or(0)));
            let val = (a << 16) | (b << 8) | c;
            out.push(self.symbol(val >> 18));
            out.push(self.symbol(val >> 12));
            if chunk.len() > 1 {
                out.push(self.symbol(val >> 6));
            }
            if chunk.len() > 2 {
                out.push(self.symbol(val));
            }
            if let Some(pad) = self.pad {
                for _ in chunk.len()..3 {
                    out.push(char::from(pad));
                }
            }
        }
        out
    }

    /// Go: `decodeQuantum`: decodes one quantum starting at `si` into `dst` and returns the next
    /// index; on error, Go's offset (bytes decoded before trailing garbage are still written).
    fn decode_quantum(
        &self,
        dst: &mut Vec<u8>,
        src: &[u8],
        mut si: usize,
    ) -> Result<usize, CorruptInputError> {
        let is_newline = |c: u8| c == b'\n' || c == b'\r';
        let mut dbuf = [0_u8; 4];
        let mut dlen = 4;
        let mut trailing = None;
        let mut j = 0;
        while j < dbuf.len() {
            let Some(&input) = src.get(si) else {
                if j == 0 {
                    return Ok(si);
                }
                if j == 1 || self.pad.is_some() {
                    return Err(CorruptInputError(si - j));
                }
                dlen = j;
                break;
            };
            si += 1;
            if let Some(out) = self.decode_byte(input) {
                if let Some(slot) = dbuf.get_mut(j) {
                    *slot = out;
                }
                j += 1;
                continue;
            }
            if is_newline(input) {
                continue;
            }
            if Some(input) != self.pad {
                return Err(CorruptInputError(si - 1));
            }
            // We've reached the end and there's padding.
            match j {
                // incorrect padding
                0 | 1 => return Err(CorruptInputError(si - 1)),
                2 => {
                    // "==" is expected, the first "=" is already consumed.
                    while src.get(si).copied().is_some_and(is_newline) {
                        si += 1;
                    }
                    match src.get(si) {
                        // not enough padding
                        None => return Err(CorruptInputError(src.len())),
                        // incorrect padding
                        Some(&c) if Some(c) != self.pad => {
                            return Err(CorruptInputError(si - 1));
                        }
                        Some(_) => si += 1,
                    }
                }
                _ => {}
            }
            while src.get(si).copied().is_some_and(is_newline) {
                si += 1;
            }
            if si < src.len() {
                // trailing garbage
                trailing = Some(CorruptInputError(si));
            }
            dlen = j;
            break;
        }
        // Convert 4x 6bit source bytes into 3 bytes.
        let val = dbuf.iter().fold(0_u32, |acc, &d| (acc << 6) | u32::from(d));
        let bytes = val.to_be_bytes();
        dst.extend(bytes.get(1..dlen).unwrap_or_default());
        trailing.map_or(Ok(si), Err)
    }

    /// Go: `Encoding.DecodeString`, keeping the bytes decoded before an error.
    #[must_use]
    pub fn decode_string_pair(&self, s: &str) -> (Vec<u8>, Option<CorruptInputError>) {
        let src = s.as_bytes();
        let mut dst = Vec::with_capacity(src.len() / 4 * 3 + 3);
        let mut si = 0;
        while si < src.len() {
            match self.decode_quantum(&mut dst, src, si) {
                Ok(next) => si = next,
                Err(e) => return (dst, Some(e)),
            }
        }
        (dst, None)
    }

    /// Go: `Encoding.DecodeString`.
    ///
    /// # Errors
    ///
    /// [`CorruptInputError`] at the first offset Go reports.
    pub fn decode_string(&self, s: &str) -> Result<Vec<u8>, CorruptInputError> {
        match self.decode_string_pair(s) {
            (out, None) => Ok(out),
            (_, Some(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests;
