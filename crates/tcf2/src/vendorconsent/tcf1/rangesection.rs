//! Go: `vendorconsent/tcf1/rangesection.go`.

use super::{Section, metadata::max_vendor_id};
use crate::{
    ConsentError,
    vendorconsent::common::{RangeEntry, bit, byte, is_set},
};

/// Go: `parseRangeSection`: the default consent value and the vendor ranges that invert it.
pub(super) fn parse_range_section(data: &[u8]) -> Result<Section, ConsentError> {
    let default_value = bit(data, 173);

    // This makes an int from bits 174-185
    if data.len() < 24 {
        return Err(ConsentError::Malformed(format!(
            "vendor consent strings using RangeSections require at least 24 bytes. Got {}",
            data.len()
        )));
    }
    let num_entries = parse_num_entries(data);

    // Parse out the "exceptions" here.
    let mut current_offset = 186;
    let mut exceptions = Vec::with_capacity(usize::from(num_entries));
    for _ in 0..num_entries {
        let (entry, bits_consumed) = parse_exception(data, current_offset)?;
        exceptions.push(entry);
        current_offset += bits_consumed;
    }

    Ok(Section::Range {
        default_value,
        exceptions,
    })
}

/// Go: `parseNumEntries`, bits 174-185.
fn parse_num_entries(data: &[u8]) -> u16 {
    let left_byte = ((byte(data, 21) & 0x03) << 2) | (byte(data, 22) >> 6);
    let right_byte = (byte(data, 22) << 2) | (byte(data, 23) >> 6);
    u16::from_be_bytes([left_byte, right_byte])
}

/// Go: `parseException`: one range entry starting at `initial_bit`, and the number of bits it
/// used. Unlike TCF 2, the bounds come from the header's `MaxVendorID`.
fn parse_exception(data: &[u8], initial_bit: usize) -> Result<(RangeEntry, usize), ConsentError> {
    // Fixes #10
    if data.len() <= initial_bit / 8 {
        return Err(ConsentError::Malformed(format!(
            "bit {initial_bit} was supposed to start a new RangeEntry, but the consent string was \
             only {} bytes long",
            data.len()
        )));
    }
    let max_vendor_id = max_vendor_id(data);
    // If the first bit is set, it's a Range of IDs
    if is_set(data, initial_bit)? {
        let start = parse_uint16(data, initial_bit + 1)?;
        let end = parse_uint16(data, initial_bit + 17)?;
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

    let vendor_id = parse_uint16(data, initial_bit + 1)?;
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

/// Go: the package-local `parseUInt16`, which has its own error text.
pub(super) fn parse_uint16(data: &[u8], bit_start_index: usize) -> Result<u16, ConsentError> {
    let start_byte = bit_start_index / 8;
    let bit_start_offset = bit_start_index % 8;
    let required = if bit_start_offset == 0 {
        start_byte + 2
    } else {
        start_byte + 3
    };
    if data.len() < required {
        return Err(ConsentError::Malformed(format!(
            "rangeSection expected a 16-bit vendorID to start at bit {bit_start_index}, but the \
             consent string was only {} bytes long",
            data.len()
        )));
    }
    if bit_start_offset == 0 {
        return Ok(u16::from_be_bytes([
            byte(data, start_byte),
            byte(data, start_byte + 1),
        ]));
    }

    let shift_complement = 8 - bit_start_offset;

    // Take the rightmost bits of the left byte, and the leftmost bits of the middle byte
    let mut left_byte = (byte(data, start_byte) & (0xFF >> bit_start_offset)) << bit_start_offset;
    left_byte |= byte(data, start_byte + 1) >> shift_complement;

    // Take the rightmost bits of the middle byte, and the leftmost bits of the right byte
    let mut right_byte = byte(data, start_byte + 2) & (0xFF << shift_complement);
    right_byte =
        (right_byte >> shift_complement) | (byte(data, start_byte + 1) << bit_start_offset);

    Ok(u16::from_be_bytes([left_byte, right_byte]))
}

/// Go: `rangeSection.VendorConsent(id)`.
pub(super) fn vendor_consent(
    data: &[u8],
    default_value: bool,
    exceptions: &[RangeEntry],
    id: u16,
) -> bool {
    if id < 1 || id > max_vendor_id(data) {
        return false;
    }
    if exceptions.iter().any(|entry| entry.contains(id)) {
        return !default_value;
    }
    default_value
}
