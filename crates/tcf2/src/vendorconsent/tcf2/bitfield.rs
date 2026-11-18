//! Go: `vendorconsent/tcf2/bitfield.go`.

use super::metadata::VendorSection;
use crate::ConsentError;

/// Go: `parseBitField`: a section with one bit per vendor starting at `start_bit`. Returns the
/// section and the bit after it.
///
/// As in Go the length check rounds down, so it can accept data that ends inside the field; the
/// caller's next read then fails.
pub(super) fn parse_bit_field(
    data: &[u8],
    vendor_bits_required: u16,
    start_bit: usize,
) -> Result<(VendorSection, usize), ConsentError> {
    let bytes_required = (usize::from(vendor_bits_required) + start_bit) / 8;
    if data.len() < bytes_required {
        return Err(ConsentError::Malformed(format!(
            "a BitField for {vendor_bits_required} vendors requires a consent string of \
             {bytes_required} bytes. This consent string had {}",
            data.len()
        )));
    }

    Ok((
        VendorSection::BitField {
            start_bit,
            max_vendor_id: vendor_bits_required,
        },
        start_bit + usize::from(vendor_bits_required),
    ))
}
