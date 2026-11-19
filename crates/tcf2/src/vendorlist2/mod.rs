//! Port of go-gdpr `vendorlist2`: parsing of IAB TCF 2 global vendor lists (GVL specification
//! versions 2 and 3).
//!
//! Go: `vendorlist2/eager-parsing.go`. `ParseLazily` is not ported because prebid-server never
//! calls it. Go reads the document with `json.Unmarshal` into go-gdpr's `vendorListContract`;
//! here `serde_json` reads it into the same contract, a plain serde type, so documents that only
//! `encoding/json`'s leniency accepts read otherwise (`gvl.json-leniency` in the differences
//! ledger, crates/tcf2/differences.toml). What go-gdpr does with the contract is ported as is.

mod error;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

pub use self::error::VendorListError;
use crate::{
    api::{Vendor, VendorList},
    consentconstants::{Purpose, SpecialFeature},
};

/// Go: `vendorlist2.ParseEagerly`: parses and validates a vendor list up front.
///
/// `serde_json` reads the document into go-gdpr's contract: unknown fields are ignored, and a
/// `null` (or the `null` document) leaves the zero value, as in Go; keys match exactly, and
/// `encoding/json`'s other leniencies are not reproduced (see the module documentation).
///
/// When several entries of `vendors` carry the same `id`, Go keeps whichever its randomised map
/// iteration visits last. Here the entries are visited in ascending key order, so the entry
/// under the greatest key wins.
///
/// # Errors
///
/// Returns [`VendorListError::Json`] when `serde_json` cannot read the document into the contract,
/// and [`VendorListError::InvalidVersion`] when `vendorListVersion` is 0 or missing.
pub fn parse_eagerly(data: &[u8]) -> Result<ParsedVendorList, VendorListError> {
    // Go: vendorlist2/eager-parsing.go:20-23.
    let contract = serde_json::from_slice::<Option<VendorListContract>>(data)
        .map_err(VendorListError::Json)?
        .unwrap_or_default();

    // Go: vendorlist2/eager-parsing.go:25-27.
    let version = contract.version.unwrap_or_default();
    if version == 0 {
        return Err(VendorListError::InvalidVersion);
    }

    // Go: vendorlist2/eager-parsing.go:29-37.
    let contracts = contract.vendors.unwrap_or_default();
    let mut vendors = HashMap::with_capacity(contracts.len());
    for vendor in contracts.into_values().map(Option::unwrap_or_default) {
        let _previous = vendors.insert(vendor.id.unwrap_or_default(), ParsedVendor::new(&vendor));
    }

    Ok(ParsedVendorList {
        spec_version: contract.gvl_specification_version.unwrap_or_default(),
        version,
        vendors,
    })
}

/// Go: `vendorListContract` (vendorlist2/eager-parsing.go:146-150). Every member, map value and
/// element that Go reads `null` into keeps its zero value there, so each is an `Option` here.
#[derive(Debug, Default, Deserialize)]
struct VendorListContract {
    #[serde(rename = "gvlSpecificationVersion")]
    gvl_specification_version: Option<u16>,
    #[serde(rename = "vendorListVersion")]
    version: Option<u16>,
    vendors: Option<BTreeMap<String, Option<VendorListVendorContract>>>,
}

/// Go: `vendorListVendorContract` (vendorlist2/eager-parsing.go:152-159).
#[derive(Debug, Default, Deserialize)]
struct VendorListVendorContract {
    id: Option<u16>,
    purposes: Ids,
    #[serde(rename = "legIntPurposes")]
    legitimate_interests: Ids,
    #[serde(rename = "flexiblePurposes")]
    flexible_purposes: Ids,
    #[serde(rename = "specialPurposes")]
    special_purposes: Ids,
    #[serde(rename = "specialFeatures")]
    special_features: Ids,
}

/// A `[]uint8` member of [`VendorListVendorContract`].
type Ids = Option<Vec<Option<u8>>>;

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
    /// Go: `mapifyPurpose` and `mapifySpecialFeature` (vendorlist2/eager-parsing.go:54-70); a
    /// `null` element is Go's zero.
    fn new(ids: &Ids) -> Self {
        let mut set = Self::default();
        for id in ids.iter().flatten().map(|id| id.unwrap_or_default()) {
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
    /// Go: `parseVendor` (vendorlist2/eager-parsing.go:42-52).
    fn new(contract: &VendorListVendorContract) -> Self {
        Self {
            purposes: IdSet::new(&contract.purposes),
            legitimate_interests: IdSet::new(&contract.legitimate_interests),
            flexible_purposes: IdSet::new(&contract.flexible_purposes),
            special_purposes: IdSet::new(&contract.special_purposes),
            special_features: IdSet::new(&contract.special_features),
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
