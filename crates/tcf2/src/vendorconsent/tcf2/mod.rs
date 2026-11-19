//! Port of go-gdpr `vendorconsent/tcf2`: the TCF 2 consent string parser.
//!
//! Go: `vendorconsent/tcf2/consent.go`.

mod bitfield;
mod metadata;
mod pubrestrict;
mod rangesection;

#[cfg(test)]
mod tests;

pub use metadata::ConsentMetadata;
#[cfg(test)]
pub(in crate::vendorconsent) use metadata::VendorSection;

use self::{
    bitfield::parse_bit_field,
    metadata::{max_vendor_id, parse_metadata},
    pubrestrict::parse_pub_restriction,
    rangesection::parse_range_section,
};
use crate::{ConsentError, base64, bitutils, vendorconsent::common::is_set};

const CONSENT_STRING_TCF2_SEPARATOR: u8 = b'.';
const CONSENT_STRING_TCF2_PREFIX: u8 = b'C';

/// Go: `tcf2.ParseString`: parses a TCF 2 consent string, unpadded base64url encoded.
///
/// Only the core segment (everything before the first `.`) is decoded. The input may be any Go
/// string, so it is taken as bytes; `&str` works as well.
///
/// # Errors
///
/// Returns go-gdpr's error for an empty string, invalid base64 or a malformed consent string. See
/// [`ConsentError::IndexOutOfRange`] for the one input shape that makes Go panic instead.
pub fn parse_string<S: AsRef<[u8]> + ?Sized>(consent: &S) -> Result<ConsentMetadata, ConsentError> {
    let mut consent = consent.as_ref();
    if consent.is_empty() {
        return Err(ConsentError::EmptyDecodedConsent);
    }
    // split TCF 2.0 segments
    if let Some(index) = consent
        .iter()
        .position(|&c| c == CONSENT_STRING_TCF2_SEPARATOR)
    {
        consent = consent.get(..index).unwrap_or_default();
    }

    // Go decodes in place; the result is identical to a separate output buffer.
    let decoded = base64::raw_url_decode(consent)?;
    parse(decoded)
}

/// Go: `tcf2.Parse`: parses decoded (not base64 encoded) TCF 2 consent data.
///
/// # Errors
///
/// Returns go-gdpr's error for malformed data. See [`ConsentError::IndexOutOfRange`] for the one
/// input shape that makes Go panic instead.
pub fn parse(data: Vec<u8>) -> Result<ConsentMetadata, ConsentError> {
    parse_metadata(&data)?;
    let max_vendor_id = max_vendor_id(&data);

    // Bit 229 determines whether or not the consent string encodes Vendor data in a RangeSection
    // or BitField.
    let (vendor_consents, legit_int_start) = if is_set(&data, 229)? {
        parse_range_section(&data, max_vendor_id, 230)?
    } else {
        parse_bit_field(&data, max_vendor_id, 230)?
    };

    let vendor_legitimate_interest_start = legit_int_start + 17;
    let leg_int_max_vend = bitutils::parse_uint16(&data, legit_int_start)?;

    // Go: consent.go:68. ParseUInt16 has only checked the two bytes it read when legit_int_start
    // is byte aligned, so this read can run past the end; Go panics there.
    let (vendor_legit_ints, pub_restricts_start) = if is_set(&data, legit_int_start + 16)? {
        parse_range_section(&data, leg_int_max_vend, vendor_legitimate_interest_start)?
    } else {
        parse_bit_field(&data, leg_int_max_vend, vendor_legitimate_interest_start)?
    };

    let pub_restrictions = parse_pub_restriction(&data, pub_restricts_start)?;
    Ok(ConsentMetadata::new(
        data,
        vendor_consents,
        vendor_legit_ints,
        pub_restrictions,
    ))
}

/// Go: `tcf2.IsConsentV2`: whether the consent string looks like a TCF 2 string (starts with
/// `C`).
#[must_use]
pub fn is_consent_v2<S: AsRef<[u8]> + ?Sized>(consent: &S) -> bool {
    consent.as_ref().first() == Some(&CONSENT_STRING_TCF2_PREFIX)
}
