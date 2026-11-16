//! The error type shared by the consent string parsers.

use core::fmt;
use std::error::Error;

use crate::base64::CorruptInputError;

/// An error returned by the consent string parsers.
///
/// [`Display`](fmt::Display) renders exactly what Go's `err.Error()` returns for the
/// corresponding go-gdpr error, so callers can embed it in responses and logs the way
/// prebid-server does (for example `amp/parse.go` returns it verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentError {
    /// `consentconstants.ErrEmptyDecodedConsent`: `decoded consent cannot be empty`.
    EmptyDecodedConsent,
    /// A `base64.CorruptInputError` from decoding the consent string.
    CorruptBase64(CorruptInputError),
    /// `errInvalidVendorListVersion` (identical text in the tcf1 and tcf2 packages).
    InvalidVendorListVersion,
    /// Any other go-gdpr error built with `fmt.Errorf`; holds Go's message verbatim.
    Malformed(String),
    /// The input makes Go panic with `runtime error: index out of range [index] with length
    /// length` instead of returning an error.
    ///
    /// Go: `vendorconsent/tcf2/consent.go:68` reads the legitimate-interest encoding bit after a
    /// bounds check that only covered two bytes. In prebid-server the panic unwinds the request
    /// goroutine and `net/http` drops the connection without a response. Callers that need exact
    /// parity must reproduce that at the call site; see [`ConsentError::is_go_panic`].
    IndexOutOfRange {
        /// The byte index Go tried to read.
        index: usize,
        /// The length of the decoded consent data.
        length: usize,
    },
}

impl ConsentError {
    /// Reports whether Go would have panicked instead of returning an error.
    #[must_use]
    pub const fn is_go_panic(&self) -> bool {
        matches!(self, Self::IndexOutOfRange { .. })
    }
}

impl fmt::Display for ConsentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDecodedConsent => f.write_str("decoded consent cannot be empty"),
            Self::CorruptBase64(err) => fmt::Display::fmt(err, f),
            Self::InvalidVendorListVersion => f.write_str(
                "the consent string encoded a VendorListVersion of 0, but this value must be \
                 greater than or equal to 1",
            ),
            Self::Malformed(message) => f.write_str(message),
            Self::IndexOutOfRange { index, length } => {
                write!(
                    f,
                    "runtime error: index out of range [{index}] with length {length}"
                )
            }
        }
    }
}

impl Error for ConsentError {}

impl From<CorruptInputError> for ConsentError {
    fn from(err: CorruptInputError) -> Self {
        Self::CorruptBase64(err)
    }
}
