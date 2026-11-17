//! Port of go-gdpr `vendorconsent/tcf1`: the TCF 1.1 consent string parser.
//!
//! prebid-server reaches it through `vendorconsent.ParseString` for strings that do not start
//! with `C`, and through `vendorconsent.Parse` in `adapters/flipp`.
//!
//! Go: `vendorconsent/tcf1/consent.go`.

mod bitfield;
mod metadata;
mod rangesection;

#[cfg(test)]
mod tests;

use self::metadata::parse_metadata;
use crate::{
    ConsentError,
    api::{ConsentTime, VendorConsents},
    base64::RAW_URL_ENCODING,
    consentconstants::Purpose,
    vendorconsent::common::{self, RangeEntry, bit, is_set},
};

/// Go: `tcf1.ParseString`: parses a TCF 1.1 consent string, unpadded base64url encoded.
///
/// Unlike TCF 2 there is no `.` segment handling.
///
/// # Errors
///
/// Returns go-gdpr's error for an empty string, invalid base64 or a malformed consent string.
pub fn parse_string<S: AsRef<[u8]> + ?Sized>(consent: &S) -> Result<Consent, ConsentError> {
    let consent = consent.as_ref();
    if consent.is_empty() {
        return Err(ConsentError::EmptyDecodedConsent);
    }
    // Go decodes in place; the result is identical to a separate output buffer.
    let decoded = RAW_URL_ENCODING.decode(consent)?;
    parse(decoded)
}

/// Go: `tcf1.Parse`: parses decoded (not base64 encoded) TCF 1.1 consent data.
///
/// # Errors
///
/// Returns go-gdpr's error for malformed data.
pub fn parse(data: Vec<u8>) -> Result<Consent, ConsentError> {
    parse_metadata(&data)?;

    // Bit 172 determines whether or not the consent string encodes Vendor data in a RangeSection
    // or BitField.
    let section = if bit(&data, 172) {
        rangesection::parse_range_section(&data)?
    } else {
        bitfield::parse_bit_field(&data)?
    };
    Ok(Consent { data, section })
}

/// The vendor section of a TCF 1.1 string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Section {
    /// Go: `*consentBitField`.
    BitField,
    /// Go: `*rangeSection`.
    Range {
        /// The consent value of every vendor not covered by an exception.
        default_value: bool,
        /// The vendor ranges whose consent is the opposite of `default_value`.
        exceptions: Vec<RangeEntry>,
    },
}

/// A parsed TCF 1.1 consent string: Go's `*consentBitField` or `*rangeSection`, both of which
/// embed `consentMetadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consent {
    data: Vec<u8>,
    pub(super) section: Section,
}

impl Consent {
    /// Go: `consentMetadata.PurposeAllowed(id)`, which reads bit `id + 131` without a bounds
    /// check.
    ///
    /// For ids whose bit lies past the end of the data Go panics; this returns that panic as
    /// [`ConsentError::IndexOutOfRange`]. [`VendorConsents::purpose_allowed`] returns false
    /// there instead. prebid-server only asks for purposes 4 and 9, which always fit.
    ///
    /// # Errors
    ///
    /// Returns [`ConsentError::IndexOutOfRange`] where Go panics.
    pub fn checked_purpose_allowed(&self, id: Purpose) -> Result<bool, ConsentError> {
        // Purposes are stored in bits 132 - 155.
        is_set(&self.data, usize::from(id.0) + 131)
    }
}

impl VendorConsents for Consent {
    fn version(&self) -> u8 {
        common::version(&self.data)
    }

    fn created(&self) -> ConsentTime {
        common::created(&self.data)
    }

    fn last_updated(&self) -> ConsentTime {
        common::last_updated(&self.data)
    }

    fn cmp_id(&self) -> u16 {
        common::cmp_id(&self.data)
    }

    fn cmp_version(&self) -> u16 {
        common::cmp_version(&self.data)
    }

    fn consent_screen(&self) -> u8 {
        common::consent_screen(&self.data)
    }

    fn consent_language(&self) -> [u8; 2] {
        common::consent_language(&self.data)
    }

    fn vendor_list_version(&self) -> u16 {
        common::vendor_list_version(&self.data)
    }

    /// `TCFPolicyVersion` is not implemented in TCF1; Go returns 0.
    fn tcf_policy_version(&self) -> u8 {
        0
    }

    fn max_vendor_id(&self) -> u16 {
        metadata::max_vendor_id(&self.data)
    }

    /// See [`Consent::checked_purpose_allowed`]; returns false where Go panics.
    fn purpose_allowed(&self, id: Purpose) -> bool {
        matches!(self.checked_purpose_allowed(id), Ok(true))
    }

    fn vendor_consent(&self, id: u16) -> bool {
        match &self.section {
            Section::BitField => bitfield::vendor_consent(&self.data, id),
            Section::Range {
                default_value,
                exceptions,
            } => rangesection::vendor_consent(&self.data, *default_value, exceptions, id),
        }
    }
}
