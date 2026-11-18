//! Port of go-gdpr `consentconstants`: the purpose and special-feature identifiers shared by the
//! consent string parsers and the vendor lists.
//!
//! `consentconstants.ErrEmptyDecodedConsent` is [`ConsentError::EmptyDecodedConsent`].
//!
//! [`ConsentError::EmptyDecodedConsent`]: crate::ConsentError::EmptyDecodedConsent

pub mod tcf2;

/// Go: `consentconstants.Purpose` (`purposes.go`), one of the IAB GDPR purposes.
///
/// Like the Go `uint8` type it accepts every value; ids outside the TCF range are simply never
/// set in a consent string or vendor list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Purpose(pub u8);

/// Go: `consentconstants.SpecialFeature` (`features.go`), one of the IAB GDPR special features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SpecialFeature(pub u8);

/// TCF 1 purpose 1: storage and access of information on the device.
pub const INFO_STORAGE_ACCESS: Purpose = Purpose(1);

/// TCF 1 purpose 2: personalisation.
pub const PERSONALIZATION: Purpose = Purpose(2);

/// TCF 1 purpose 3: ad selection, delivery and reporting.
pub const AD_SELECTION_DELIVERY_REPORTING: Purpose = Purpose(3);

/// TCF 1 purpose 4: content selection, delivery and reporting.
pub const CONTENT_SELECTION_DELIVERY_REPORTING: Purpose = Purpose(4);

/// TCF 1 purpose 5: measurement.
pub const MEASUREMENT: Purpose = Purpose(5);
