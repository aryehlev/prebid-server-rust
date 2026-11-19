//! Glue (Rule B (a) and (d)): GPP strings decode as Go's `encoding/base64` decodes them.
//!
//! go-gpp decodes the header and every section segment with `base64.RawURLEncoding.Decode`
//! (util/bitstream.go:20-30, after padding the last quantum) and encodes with
//! `RawURLEncoding.Encode` (util/encoding.go:67-72). The base64 crate decodes and encodes here,
//! and two things stay as Go has them:
//!
//! - which strings decode, case (a): Go's decoder skips `\r` and `\n` anywhere in the input, and
//!   its encodings are not strict, so a last digit with unused bits set is accepted (a line break
//!   leaves the last quantum short after go-gpp's padding); the crate's `URL_SAFE_NO_PAD` refuses
//!   both. The line breaks are removed before the crate reads the input, and the engine allows
//!   trailing bits. Go's `NoPadding` makes `=` an illegal byte, as the crate's
//!   [`DecodePaddingMode::RequireNone`] does.
//! - the error, case (d): go-gpp wraps Go's `illegal base64 data at input byte N` into its own
//!   errors (`error parsing GPP header, base64 decoding: ...`, parse.go:39-42), which Prebid writes
//!   into its GPP warning (endpoints/openrtb2/auction.go:869-874). The crate reports another offset
//!   for some inputs (it looks at the last byte first), so the offset is Go's: that of the first
//!   byte that is neither a digit nor a line break, or, when the digits end in a lone one, that of
//!   the last byte.
//!
//! The bytes are the crate's, both ways.

use std::{borrow::Cow, fmt};

use base64::{
    DecodeError, Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};

/// Go's `base64.CorruptInputError`: the input byte offset at which decoding failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CorruptInputError(usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Go: $GOROOT/src/encoding/base64/base64.go:303-305.
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

/// Go's `base64.RawURLEncoding` (base64.go:133) as an engine of the crate: the URL-safe
/// alphabet and no padding, with trailing bits allowed, since Go's encodings are strict only
/// after `Strict()`.
const RAW_URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

/// Decodes `src` like Go's `base64.RawURLEncoding.Decode`, returning the decoded bytes or the
/// `CorruptInputError` Go would report.
pub(super) fn raw_url_decode(src: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    RAW_URL
        .decode(without_line_breaks(src))
        .map_err(|err| go_error(src, &err))
}

/// Encodes `src` like Go's `base64.RawURLEncoding.Encode`.
pub(super) fn raw_url_encode(src: &[u8]) -> Vec<u8> {
    RAW_URL.encode(src).into_bytes()
}

/// `src` without the `\r` and `\n` bytes Go's decoder skips (base64.go:340-343).
fn without_line_breaks(src: &[u8]) -> Cow<'_, [u8]> {
    if src.iter().copied().any(is_line_break) {
        Cow::Owned(src.iter().copied().filter(|&b| !is_line_break(b)).collect())
    } else {
        Cow::Borrowed(src)
    }
}

const fn is_line_break(b: u8) -> bool {
    b == b'\r' || b == b'\n'
}

/// The error Go reports for `src`, which the crate refused with `err`.
///
/// Go reads `src` left to right and stops at the first byte that is neither a digit nor a line
/// break (base64.go:345-347; `=` is such a byte, the encoding having no padding). When there is
/// none, the digits end in a lone one, which Go reports at `len(src) - 1` once it has read to the
/// end (base64.go:322-327: `si - j` with `si == len(src)` and `j == 1`), and which the crate
/// reports as [`DecodeError::InvalidLength`].
fn go_error(src: &[u8], err: &DecodeError) -> CorruptInputError {
    let illegal = match err {
        DecodeError::InvalidLength(_) => None,
        DecodeError::InvalidByte(..)
        | DecodeError::InvalidLastSymbol { .. }
        | DecodeError::InvalidPadding => src.iter().position(|&b| {
            !is_line_break(b) && !alphabet::URL_SAFE.as_str().as_bytes().contains(&b)
        }),
    };
    CorruptInputError(illegal.unwrap_or_else(|| src.len().saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::{CorruptInputError, raw_url_decode, raw_url_encode};

    // Go: $GOROOT/src/encoding/base64/base64.go:312-407 (decodeQuantum, through
    // RawURLEncoding.Decode at :518); every value checked against go1.26.2.
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

    // Go: $GOROOT/src/encoding/base64/base64.go:303-305 (CorruptInputError.Error).
    #[test]
    fn decode_error_text() {
        let err = raw_url_decode(b"DBA=").err().map(|e| e.to_string());
        assert_eq!(
            err.as_deref(),
            Some("illegal base64 data at input byte 3"),
            "error text"
        );
    }

    // Go: $GOROOT/src/encoding/base64/base64.go:145 (Encode, through RawURLEncoding), which
    // util.BitStream.Base64Encode calls (util/encoding.go:67-72); checked against go1.26.2.
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
