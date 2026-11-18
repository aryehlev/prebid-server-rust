//! Go: `vendorconsent/tcf2/metadata.go`.

use super::{pubrestrict::PubRestrictions, rangesection::RangeSection};
use crate::{
    ConsentError,
    api::{ConsentTime, VendorConsents},
    consentconstants::Purpose,
    vendorconsent::common::{self, bit, byte},
};

/// Go: `parseMetadata`, which validates the fixed-size header of a decoded TCF 2 string.
pub(super) fn parse_metadata(data: &[u8]) -> Result<(), ConsentError> {
    if data.len() < 29 {
        return Err(ConsentError::Malformed(format!(
            "vendor consent strings are at least 29 bytes long. This one was {}",
            data.len()
        )));
    }
    let version = common::version(data);
    if version < 2 {
        return Err(ConsentError::Malformed(format!(
            "the consent string encoded a Version of {version}, but this value must be greater \
             than or equal to 2"
        )));
    }
    if common::vendor_list_version(data) == 0 {
        return Err(ConsentError::InvalidVendorListVersion);
    }
    Ok(())
}

/// Go: `vendorConsentsResolver`, the vendor consent or legitimate-interest section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vendorconsent) enum VendorSection {
    /// Go: `*consentBitField`: one bit per vendor, starting at `start_bit`.
    BitField {
        /// The bit holding vendor 1.
        start_bit: usize,
        /// The highest vendor ID in the section.
        max_vendor_id: u16,
    },
    /// Go: `*rangeSection`.
    Range(RangeSection),
}

impl VendorSection {
    /// Go: `MaxVendorID()`.
    const fn max_vendor_id(&self) -> u16 {
        match self {
            Self::BitField { max_vendor_id, .. } => *max_vendor_id,
            Self::Range(range) => range.max_vendor_id,
        }
    }

    /// Go: `VendorConsent(id)`.
    fn vendor_consent(&self, data: &[u8], id: u16) -> bool {
        match self {
            Self::BitField {
                start_bit,
                max_vendor_id,
            } => {
                if id < 1 || id > *max_vendor_id {
                    return false;
                }
                // Careful here... vendor IDs start at index 1. Parse has checked that the whole
                // field lies inside data (the section after it is read successfully).
                bit(data, *start_bit + usize::from(id) - 1)
            }
            Self::Range(range) => range.vendor_consent(id),
        }
    }
}

/// Go: `vendorconsent.ConsentMetadata`, a parsed TCF 2 consent string.
///
/// This is the type prebid-server type-asserts `api.VendorConsents` to (`gdpr/consent_parser.go`);
/// match [`ParsedConsent::Tcf2`](crate::vendorconsent::ParsedConsent::Tcf2) to do the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentMetadata {
    data: Vec<u8>,
    pub(in crate::vendorconsent) vendor_consents: VendorSection,
    pub(in crate::vendorconsent) vendor_legitimate_interests: VendorSection,
    pub(in crate::vendorconsent) publisher_restrictions: PubRestrictions,
}

impl ConsentMetadata {
    pub(super) const fn new(
        data: Vec<u8>,
        vendor_consents: VendorSection,
        vendor_legitimate_interests: VendorSection,
        publisher_restrictions: PubRestrictions,
    ) -> Self {
        Self {
            data,
            vendor_consents,
            vendor_legitimate_interests,
            publisher_restrictions,
        }
    }

    /// Go: `VendorLegitInterestMaxID()`, the highest vendor ID in the legitimate-interest
    /// section.
    #[must_use]
    pub const fn vendor_legit_interest_max_id(&self) -> u16 {
        self.vendor_legitimate_interests.max_vendor_id()
    }

    /// Go: `PurposeLITransparency(id)`, bits 176-199: whether legitimate-interest transparency
    /// was established for the purpose. Always false for ids above 24.
    #[must_use]
    pub fn purpose_li_transparency(&self, id: Purpose) -> bool {
        if id.0 > 24 {
            return false;
        }
        bit(&self.data, usize::from(id.0) + 175)
    }

    /// Go: `PurposeOneTreatment()`, bit 200.
    #[must_use]
    pub fn purpose_one_treatment(&self) -> bool {
        bit(&self.data, 200)
    }

    /// Go: `SpecialFeatureOptIn(id)`, bits 140-151. Always false for ids above 12; id 0 reads bit
    /// 139 (the "non-standard stacks" flag), as in Go.
    #[must_use]
    pub fn special_feature_opt_in(&self, id: u16) -> bool {
        if id > 12 {
            return false;
        }
        bit(&self.data, 140 + usize::from(id) - 1)
    }

    /// Go: `VendorLegitInterest(id)`: whether legitimate interest was established for the vendor.
    #[must_use]
    pub fn vendor_legit_interest(&self, id: u16) -> bool {
        self.vendor_legitimate_interests
            .vendor_consent(&self.data, id)
    }

    /// Go: `CheckPubRestriction(purposeID, restrictType, vendor)`: whether the publisher
    /// restricts the purpose with the given restriction type for the vendor.
    ///
    /// As in Go, the lookup key is the `uint8` value `purpose_id<<2 | restrict_type&3`, so
    /// purpose ids of 64 and above wrap around.
    #[must_use]
    pub fn check_pub_restriction(&self, purpose_id: u8, restrict_type: u8, vendor: u16) -> bool {
        self.publisher_restrictions
            .check_pub_restriction(purpose_id, restrict_type, vendor)
    }
}

impl VendorConsents for ConsentMetadata {
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

    /// Go: `TCFPolicyVersion()`, bits 132-137.
    fn tcf_policy_version(&self) -> u8 {
        ((byte(&self.data, 16) & 0x0F) << 2) | (byte(&self.data, 17) & 0xC0) >> 6
    }

    /// Go: `MaxVendorID()`, bits 213-228 (read from the header, not from the parsed section).
    fn max_vendor_id(&self) -> u16 {
        max_vendor_id(&self.data)
    }

    /// Go: `PurposeAllowed(id)`, bits 152-175. Always false for ids above 24.
    fn purpose_allowed(&self, id: Purpose) -> bool {
        if id.0 > 24 {
            return false;
        }
        bit(&self.data, usize::from(id.0) + 151)
    }

    fn vendor_consent(&self, id: u16) -> bool {
        self.vendor_consents.vendor_consent(&self.data, id)
    }
}

/// Go: `ConsentMetadata.MaxVendorID()`.
pub(super) fn max_vendor_id(data: &[u8]) -> u16 {
    let left_byte = ((byte(data, 26) & 0x07) << 5) | ((byte(data, 27) & 0xF8) >> 3);
    let right_byte = ((byte(data, 27) & 0x07) << 5) | ((byte(data, 28) & 0xF8) >> 3);
    u16::from_be_bytes([left_byte, right_byte])
}
