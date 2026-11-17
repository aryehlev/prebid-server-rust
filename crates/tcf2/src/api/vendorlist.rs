//! Go: `api/vendorlist.go`.

use crate::consentconstants::{Purpose, SpecialFeature};

/// Go: `api.VendorList`, an IAB global vendor list.
pub trait VendorList {
    /// The version of the global vendor list specification the list adheres to.
    fn spec_version(&self) -> u16;

    /// The version of the vendor list.
    fn version(&self) -> u16;

    /// The vendor with the given ID, or `None` (Go `nil`) if the list does not contain it.
    fn vendor(&self, vendor_id: u16) -> Option<&dyn Vendor>;
}

/// Go: `api.Vendor`, the purposes a vendor declares in a vendor list.
pub trait Vendor {
    /// Whether the vendor declares the purpose, either as a consent purpose or as a flexible one.
    fn purpose(&self, purpose_id: Purpose) -> bool;

    /// Whether the vendor declares the purpose as a consent purpose, ignoring flexible purposes.
    fn purpose_strict(&self, purpose_id: Purpose) -> bool;

    /// Whether the vendor claims a legitimate interest for the purpose, either directly or
    /// through a flexible purpose.
    fn legitimate_interest(&self, purpose_id: Purpose) -> bool;

    /// Whether the vendor claims a legitimate interest for the purpose, ignoring flexible
    /// purposes.
    fn legitimate_interest_strict(&self, purpose_id: Purpose) -> bool;

    /// Whether the vendor declares the special purpose.
    fn special_purpose(&self, purpose_id: Purpose) -> bool;

    /// Whether the vendor declares the special feature.
    fn special_feature(&self, feature_id: SpecialFeature) -> bool;
}
