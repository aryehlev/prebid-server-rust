//! Go: `vendorconsent/tcf2/pubrestrict.go`.

use std::collections::BTreeMap;

use super::rangesection::parse_range_consent;
use crate::{ConsentError, bitutils, vendorconsent::common::RangeEntry};

/// IAB spec does not specify a max vendorID for the publisher restrictions. This should be one
/// bit short of the max possible.
const ASSUMED_MAX_VENDOR_ID: u16 = 32767;

/// Go: `parsePubRestriction`: the publisher restrictions section starting at `start_bit`.
///
/// Go also returns the bit after the section, which no caller uses.
pub(super) fn parse_pub_restriction(
    data: &[u8],
    start_bit: usize,
) -> Result<PubRestrictions, ConsentError> {
    let num_restrictions = bitutils::parse_uint12(data, start_bit).map_err(|err| {
        ConsentError::Malformed(format!(
            "Error on parsing the number of publisher restrictions: {err}"
        ))
    })?;

    // Parse out the "exceptions" here.
    let mut current_offset = start_bit + 12;
    let mut restrictions = BTreeMap::new();
    for _ in 0..num_restrictions {
        let restrict_data = bitutils::parse_byte8(data, current_offset).map_err(|err| {
            ConsentError::Malformed(format!(
                "Error on parsing the publisher restriction purpose/type: {err}"
            ))
        })?;
        current_offset += 8;
        let num_entries = bitutils::parse_uint12(data, current_offset).map_err(|err| {
            ConsentError::Malformed(format!(
                "Error on parsing the number of publisher restriction vendor ranges: {err}"
            ))
        })?;
        current_offset += 12;
        let mut vendors = Vec::with_capacity(usize::from(num_entries));
        for _ in 0..num_entries {
            let (entry, bits_consumed) =
                parse_range_consent(data, current_offset, ASSUMED_MAX_VENDOR_ID)?;
            vendors.push(entry);
            current_offset += bits_consumed;
        }
        // Go keys a map by the raw purpose/type byte, so a repeated byte replaces the earlier
        // restriction. Go also stores purposeID and restrictType, which nothing reads.
        let _previous = restrictions.insert(restrict_data, vendors);
    }
    Ok(PubRestrictions { restrictions })
}

/// Go: `pubRestrictions`, keyed by the purpose/type byte `purposeID<<2 | restrictType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vendorconsent) struct PubRestrictions {
    pub(in crate::vendorconsent) restrictions: BTreeMap<u8, Vec<RangeEntry>>,
}

impl PubRestrictions {
    /// Go: `CheckPubRestriction`.
    pub(super) fn check_pub_restriction(
        &self,
        purpose_id: u8,
        restrict_type: u8,
        vendor: u16,
    ) -> bool {
        // Go computes this in uint8, so the purpose's top two bits are shifted out.
        let key = purpose_id << 2 | (restrict_type & 0x03);
        self.restrictions
            .get(&key)
            .is_some_and(|vendors| vendors.iter().any(|entry| entry.contains(vendor)))
    }
}
