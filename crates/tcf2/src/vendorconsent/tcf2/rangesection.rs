//! Go: `vendorconsent/tcf2/rangesection.go`.

use super::metadata::VendorSection;
use crate::{
    ConsentError, bitutils,
    vendorconsent::common::{RangeEntry, is_set},
};

/// Go: `parseRangeSection`: a section listing the vendor ID ranges that have consent. Returns the
/// section and the bit after it.
pub(super) fn parse_range_section(
    data: &[u8],
    max_vendor_id: u16,
    start_bit: usize,
) -> Result<(VendorSection, usize), ConsentError> {
    if data.len() < 31 {
        return Err(ConsentError::Malformed(format!(
            "vendor consent strings using RangeSections require at least 31 bytes. Got {}",
            data.len()
        )));
    }

    // This makes an int from bits [startBit, startBit + 12)
    let num_entries = bitutils::parse_uint12(data, start_bit)?;

    // Parse out the "exceptions" here.
    let mut current_offset = start_bit + 12;
    let mut consents = Vec::with_capacity(usize::from(num_entries));
    for _ in 0..num_entries {
        let (entry, bits_consumed) = parse_range_consent(data, current_offset, max_vendor_id)?;
        consents.push(entry);
        current_offset += bits_consumed;
    }

    Ok((
        VendorSection::Range(RangeSection {
            consents,
            max_vendor_id,
        }),
        current_offset,
    ))
}

/// Go: `parseRangeConsent`: one range entry starting at `initial_bit`, and the number of bits it
/// used.
pub(super) fn parse_range_consent(
    data: &[u8],
    initial_bit: usize,
    max_vendor_id: u16,
) -> Result<(RangeEntry, usize), ConsentError> {
    // Fixes #10
    if data.len() <= initial_bit / 8 {
        return Err(ConsentError::Malformed(format!(
            "bit {initial_bit} was supposed to start a new RangeEntry, but the consent string was \
             only {} bytes long",
            data.len()
        )));
    }
    // If the first bit is set, it's a Range of IDs
    if is_set(data, initial_bit)? {
        let start = bitutils::parse_uint16(data, initial_bit + 1)?;
        let end = bitutils::parse_uint16(data, initial_bit + 17)?;
        if start == 0 {
            return Err(ConsentError::Malformed(format!(
                "bit {initial_bit} range entry exclusion starts at 0, but the min vendor ID is 1"
            )));
        }
        if end > max_vendor_id {
            return Err(ConsentError::Malformed(format!(
                "bit {initial_bit} range entry exclusion ends at {end}, but the max vendor ID is \
                 {max_vendor_id}"
            )));
        }
        if end <= start {
            return Err(ConsentError::Malformed(format!(
                "bit {initial_bit} range entry excludes vendors [{start}, {end}]. The start should \
                 be less than the end"
            )));
        }
        return Ok((
            RangeEntry {
                start_id: start,
                end_id: end,
            },
            33,
        ));
    }

    let vendor_id = bitutils::parse_uint16(data, initial_bit + 1)?;
    if vendor_id == 0 || vendor_id > max_vendor_id {
        return Err(ConsentError::Malformed(format!(
            "bit {initial_bit} range entry excludes vendor {vendor_id}, but only vendors [1, \
             {max_vendor_id}] are valid"
        )));
    }

    Ok((
        RangeEntry {
            start_id: vendor_id,
            end_id: vendor_id,
        },
        17,
    ))
}

/// Go: `rangeSection`: the vendor ID ranges that have consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vendorconsent) struct RangeSection {
    pub(in crate::vendorconsent) consents: Vec<RangeEntry>,
    pub(in crate::vendorconsent) max_vendor_id: u16,
}

impl RangeSection {
    /// Go: `VendorConsent(id)`.
    pub(super) fn vendor_consent(&self, id: u16) -> bool {
        if id < 1 || id > self.max_vendor_id {
            return false;
        }
        self.consents.iter().any(|entry| entry.contains(id))
    }
}
