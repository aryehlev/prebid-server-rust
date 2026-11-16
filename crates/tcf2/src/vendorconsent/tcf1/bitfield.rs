//! Go: `vendorconsent/tcf1/bitfield.go`.

use super::{Section, metadata::max_vendor_id};
use crate::{
    ConsentError,
    vendorconsent::common::{bit, byte},
};

/// Go: `parseBitField`: checks that the data holds one bit per vendor after bit 172.
pub(super) fn parse_bit_field(data: &[u8]) -> Result<Section, ConsentError> {
    let vendor_bits_required = max_vendor_id(data);

    // BitFields start at bit 173. This means the last three bits of byte 21 are part of the
    // bitfield. In this case "others" will never be used.
    if vendor_bits_required <= 3 {
        return Ok(Section::BitField);
    }

    let mut other_bytes_required = (vendor_bits_required - 3) / 8;
    if !(vendor_bits_required - 3).is_multiple_of(8) {
        other_bytes_required += 1;
    }
    let data_length_required = 22 + usize::from(other_bytes_required);
    if data.len() < data_length_required {
        return Err(ConsentError::Malformed(format!(
            "a BitField for {vendor_bits_required} vendors requires a consent string of \
             {data_length_required} bytes. This consent string had {}",
            data.len()
        )));
    }
    Ok(Section::BitField)
}

/// Go: `consentBitField.VendorConsent(id)`.
pub(super) fn vendor_consent(data: &[u8], id: u16) -> bool {
    if id < 1 || id > max_vendor_id(data) {
        return false;
    }
    // Careful here... vendor IDs start at index 1...
    if id <= 3 {
        // Go: `firstThree` is data[21].
        return byte(data, 21) & (0x08 >> id) != 0;
    }
    // Go: `others` is data[22:], which parse_bit_field checked covers every vendor.
    bit(data, 22 * 8 + usize::from(id - 4))
}
