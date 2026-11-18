//! Port of go-gdpr `vendorlist2`: parsing of IAB TCF 2 global vendor lists (GVL specification
//! versions 2 and 3).
//!
//! Go: `vendorlist2/eager-parsing.go`. `ParseLazily` is not ported because prebid-server never
//! calls it. Go's `json.Unmarshal` into `vendorListContract` is reproduced by the private
//! `scanner` (Go `encoding/json/scanner.go`) and `decode` (Go `encoding/json/decode.go`)
//! modules, so that the accepted documents, the decoded values and the error messages all match.

mod decode;
mod error;
mod scanner;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use self::decode::{VendorContract, unmarshal};
pub use self::error::VendorListError;
use crate::{
    api::{Vendor, VendorList},
    consentconstants::{Purpose, SpecialFeature},
};

/// Go: `vendorlist2.ParseEagerly`: parses and validates a vendor list up front.
///
/// The document is decoded as Go's `encoding/json` decodes it into go-gdpr's contract: unknown
/// fields are ignored, keys match case-insensitively, `purposes`-style fields may also be base64
/// strings, and so on (see the `decode` module).
///
/// When several entries of `vendors` carry the same `id`, Go keeps whichever its randomised map
/// iteration visits last. This port is deterministic: it keeps the entry of the map key that
/// first appears latest in the document (a repeated key replaces its earlier value in place, as
/// in Go's map).
///
/// # Errors
///
/// Returns Go's error: a JSON syntax error, the first type mismatch or base64 error met while
/// decoding, or [`VendorListError::InvalidVersion`] when `vendorListVersion` is 0 or missing.
pub fn parse_eagerly(data: &[u8]) -> Result<ParsedVendorList, VendorListError> {
    let contract = unmarshal(data)?;

    if contract.version == 0 {
        return Err(VendorListError::InvalidVersion);
    }

    let mut vendors = HashMap::with_capacity(contract.vendors.values().len());
    for vendor in contract.vendors.values() {
        let _previous = vendors.insert(vendor.id, ParsedVendor::new(vendor));
    }

    Ok(ParsedVendorList {
        spec_version: contract.gvl_specification_version,
        version: contract.version,
        vendors,
    })
}

/// Go: `parsedVendorList`, the [`VendorList`] that [`parse_eagerly`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVendorList {
    spec_version: u16,
    version: u16,
    vendors: HashMap<u16, ParsedVendor>,
}

impl VendorList for ParsedVendorList {
    fn spec_version(&self) -> u16 {
        self.spec_version
    }

    fn version(&self) -> u16 {
        self.version
    }

    fn vendor(&self, vendor_id: u16) -> Option<&dyn Vendor> {
        let vendor: &dyn Vendor = self.vendors.get(&vendor_id)?;
        Some(vendor)
    }
}

/// A set of `u8` ids (Go `map[consentconstants.Purpose]struct{}` and the special feature
/// equivalent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct IdSet([u64; 4]);

impl IdSet {
    /// Go: `mapifyPurpose` and `mapifySpecialFeature`.
    fn new(ids: &[u8]) -> Self {
        let mut set = Self::default();
        for &id in ids {
            if let Some(word) = set.0.get_mut(usize::from(id >> 6)) {
                *word |= 1 << (id & 63);
            }
        }
        set
    }

    fn contains(&self, id: u8) -> bool {
        self.0
            .get(usize::from(id >> 6))
            .is_some_and(|word| (word >> (id & 63)) & 1 == 1)
    }
}

/// Go: `parsedVendor`, a vendor of a [`ParsedVendorList`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedVendor {
    purposes: IdSet,
    legitimate_interests: IdSet,
    flexible_purposes: IdSet,
    special_purposes: IdSet,
    special_features: IdSet,
}

impl ParsedVendor {
    /// Go: `parseVendor`.
    fn new(contract: &VendorContract) -> Self {
        Self {
            purposes: IdSet::new(contract.purposes.values()),
            legitimate_interests: IdSet::new(contract.legitimate_interests.values()),
            flexible_purposes: IdSet::new(contract.flexible_purposes.values()),
            special_purposes: IdSet::new(contract.special_purposes.values()),
            special_features: IdSet::new(contract.special_features.values()),
        }
    }
}

impl Vendor for ParsedVendor {
    fn purpose(&self, purpose_id: Purpose) -> bool {
        self.purposes.contains(purpose_id.0) || self.flexible_purposes.contains(purpose_id.0)
    }

    fn purpose_strict(&self, purpose_id: Purpose) -> bool {
        self.purposes.contains(purpose_id.0)
    }

    fn legitimate_interest(&self, purpose_id: Purpose) -> bool {
        self.legitimate_interests.contains(purpose_id.0)
            || self.flexible_purposes.contains(purpose_id.0)
    }

    fn legitimate_interest_strict(&self, purpose_id: Purpose) -> bool {
        self.legitimate_interests.contains(purpose_id.0)
    }

    fn special_purpose(&self, purpose_id: Purpose) -> bool {
        self.special_purposes.contains(purpose_id.0)
    }

    fn special_feature(&self, feature_id: SpecialFeature) -> bool {
        self.special_features.contains(feature_id.0)
    }
}
