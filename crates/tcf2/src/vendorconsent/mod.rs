//! Port of go-gdpr `vendorconsent`: consent string parsing for TCF 1.1 and TCF 2.
//!
//! Go: `vendorconsent/consent.go`. `ParseVersion` is not ported because prebid-server never calls
//! it.

mod common;
pub mod tcf1;
pub mod tcf2;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tests;

use crate::{
    ConsentError,
    api::{ConsentTime, VendorConsents},
    consentconstants::Purpose,
};

/// The `api.VendorConsents` value `vendorconsent.ParseString` returns, with its dynamic type.
///
/// prebid-server's `pc.(tcf2.ConsentMetadata)` type assertion (`gdpr/consent_parser.go`) is a
/// match on [`ParsedConsent::Tcf2`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedConsent {
    /// A TCF 1.1 string (Go `*consentBitField` or `*rangeSection` from `vendorconsent/tcf1`).
    Tcf1(tcf1::Consent),
    /// A TCF 2 string (Go `tcf2.ConsentMetadata`).
    Tcf2(tcf2::ConsentMetadata),
}

/// Go: `vendorconsent.ParseString`: parses an unpadded base64url consent string.
///
/// Strings starting with `C` are parsed as TCF 2 (and split at the first `.`); everything else is
/// parsed as TCF 1.1.
///
/// # Errors
///
/// Returns go-gdpr's error for an empty string, invalid base64 or a malformed consent string. See
/// [`ConsentError::IndexOutOfRange`] for the one TCF 2 input shape that makes Go panic instead.
pub fn parse_string<S: AsRef<[u8]> + ?Sized>(consent: &S) -> Result<ParsedConsent, ConsentError> {
    let consent = consent.as_ref();
    if consent.is_empty() {
        return Err(ConsentError::EmptyDecodedConsent);
    }

    if tcf2::is_consent_v2(consent) {
        return tcf2::parse_string(consent).map(ParsedConsent::Tcf2);
    }

    tcf1::parse_string(consent).map(ParsedConsent::Tcf1)
}

/// Go: `vendorconsent.Parse`, kept by go-gdpr for backwards compatibility: parses decoded data as
/// TCF 1.1 (`tcf1.Parse`). `adapters/flipp` uses it.
///
/// # Errors
///
/// Returns go-gdpr's error for malformed data.
pub fn parse(data: Vec<u8>) -> Result<tcf1::Consent, ConsentError> {
    tcf1::parse(data)
}

impl VendorConsents for ParsedConsent {
    fn version(&self) -> u8 {
        match self {
            Self::Tcf1(consent) => consent.version(),
            Self::Tcf2(consent) => consent.version(),
        }
    }

    fn created(&self) -> ConsentTime {
        match self {
            Self::Tcf1(consent) => consent.created(),
            Self::Tcf2(consent) => consent.created(),
        }
    }

    fn last_updated(&self) -> ConsentTime {
        match self {
            Self::Tcf1(consent) => consent.last_updated(),
            Self::Tcf2(consent) => consent.last_updated(),
        }
    }

    fn cmp_id(&self) -> u16 {
        match self {
            Self::Tcf1(consent) => consent.cmp_id(),
            Self::Tcf2(consent) => consent.cmp_id(),
        }
    }

    fn cmp_version(&self) -> u16 {
        match self {
            Self::Tcf1(consent) => consent.cmp_version(),
            Self::Tcf2(consent) => consent.cmp_version(),
        }
    }

    fn consent_screen(&self) -> u8 {
        match self {
            Self::Tcf1(consent) => consent.consent_screen(),
            Self::Tcf2(consent) => consent.consent_screen(),
        }
    }

    fn consent_language(&self) -> [u8; 2] {
        match self {
            Self::Tcf1(consent) => consent.consent_language(),
            Self::Tcf2(consent) => consent.consent_language(),
        }
    }

    fn vendor_list_version(&self) -> u16 {
        match self {
            Self::Tcf1(consent) => consent.vendor_list_version(),
            Self::Tcf2(consent) => consent.vendor_list_version(),
        }
    }

    fn tcf_policy_version(&self) -> u8 {
        match self {
            Self::Tcf1(consent) => consent.tcf_policy_version(),
            Self::Tcf2(consent) => consent.tcf_policy_version(),
        }
    }

    fn max_vendor_id(&self) -> u16 {
        match self {
            Self::Tcf1(consent) => consent.max_vendor_id(),
            Self::Tcf2(consent) => consent.max_vendor_id(),
        }
    }

    fn purpose_allowed(&self, id: Purpose) -> bool {
        match self {
            Self::Tcf1(consent) => consent.purpose_allowed(id),
            Self::Tcf2(consent) => consent.purpose_allowed(id),
        }
    }

    fn vendor_consent(&self, id: u16) -> bool {
        match self {
            Self::Tcf1(consent) => consent.vendor_consent(id),
            Self::Tcf2(consent) => consent.vendor_consent(id),
        }
    }
}
