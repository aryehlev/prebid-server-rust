//! Glue (Rule B (a) and (d)): consent strings decode as Go's `encoding/base64` decodes them.
//!
//! go-gdpr decodes a consent string with `base64.RawURLEncoding` (`ParseString` in
//! vendorconsent/tcf2/consent.go:27-33 and vendorconsent/tcf1/consent.go:16-22), and the flipp
//! adapter decodes one with `RawURLEncoding.DecodeString` (adapters/flipp/flipp.go:291). The
//! base64 crate decodes here, and two things stay as Go has them:
//!
//! - which strings decode, case (a): Go's decoder skips `\r` and `\n` anywhere in the input, and
//!   its encodings are not strict, so a last digit with unused bits set is accepted; the crate's
//!   `URL_SAFE_NO_PAD` refuses both. The line breaks are removed before the crate reads the input,
//!   and the engine allows trailing bits. Go's `NoPadding` makes `=` an illegal byte, as the
//!   crate's [`DecodePaddingMode::RequireNone`] does.
//! - the error, case (d): go-gdpr returns Go's `illegal base64 data at input byte N`, and Prebid
//!   writes that text into its own (the AMP consent warning, amp/parse.go:143-152). The crate
//!   reports another offset for some inputs (it looks at the last byte first), so the offset is
//!   Go's: that of the first byte that is neither a digit nor a line break, or, when the digits end
//!   in a lone one, that of the last byte.
//!
//! The bytes a string decodes to are the crate's.

use core::fmt;
use std::{borrow::Cow, error::Error};

use base64::{
    DecodeError, Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};

/// Go `base64.CorruptInputError`: `illegal base64 data at input byte N`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CorruptInputError(pub usize);

impl fmt::Display for CorruptInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Go: $GOROOT/src/encoding/base64/base64.go:303-305.
        write!(f, "illegal base64 data at input byte {}", self.0)
    }
}

impl Error for CorruptInputError {}

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

/// Go `base64.RawURLEncoding.DecodeString`, and `RawURLEncoding.Decode` as go-gdpr calls it, in
/// place over the input: both give the same bytes and the same errors.
///
/// Go also returns the bytes decoded before an error; no caller in go-gdpr or prebid-server reads
/// them, so they are dropped.
///
/// # Errors
///
/// Returns Go's [`CorruptInputError`] when `src` is not unpadded base64url.
pub fn raw_url_decode(src: &[u8]) -> Result<Vec<u8>, CorruptInputError> {
    RAW_URL
        .decode(without_line_breaks(src))
        .map_err(|err| go_error(src, &err))
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
    use super::{CorruptInputError, raw_url_decode};
    use crate::test_support::TestResult;

    // Go: testdata/go/b64.go (the hand-picked inputs of buildBase64Cases), whose results
    // testdata/base64_goldens.json records from RawURLEncoding.
    #[test]
    fn raw_url_values() -> TestResult {
        check_eq!(raw_url_decode(b"")?, Vec::<u8>::new(), "empty input");
        check_eq!(raw_url_decode(b"Zm9vYmFy")?, b"foobar", "full quanta");
        check_eq!(raw_url_decode(b"Zm9vYg")?, b"foob", "partial quantum");
        check_eq!(
            raw_url_decode(b"A\nA\nA\nA")?,
            [0, 0, 0],
            "line breaks are skipped"
        );
        check_eq!(raw_url_decode(b"AA\n\n")?, [0], "after a partial quantum");
        check_eq!(raw_url_decode(b"QR")?, b"A", "trailing bits are ignored");
        check_eq!(raw_url_decode(b"-_-_")?, [0xFB, 0xFF, 0xBF], "URL alphabet");
        Ok(())
    }

    // Go: testdata/go/b64.go (the hand-picked inputs of buildBase64Cases), whose errors
    // testdata/base64_goldens.json records from RawURLEncoding.
    #[test]
    fn raw_url_errors() -> TestResult {
        for (input, offset) in [
            (&b"A"[..], 0),
            (b"=", 0),
            (b"AA==", 2),
            (b"AA=A", 2),
            (b"/+/+", 0),
            (b"A\n", 1),
            (b"\xff", 0),
        ] {
            check_eq!(
                raw_url_decode(input),
                Err(CorruptInputError(offset)),
                "{input:?}"
            );
        }
        check_eq!(
            CorruptInputError(3).to_string(),
            "illegal base64 data at input byte 3",
            "Go's text"
        );
        Ok(())
    }
}
