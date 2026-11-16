//! Go: `vendorconsent/tcf1/metadata.go`.

use crate::{
    ConsentError,
    vendorconsent::common::{self, byte},
};

/// Go: `parseMetadata`, which validates the fixed-size header of a decoded TCF 1 string.
pub(super) fn parse_metadata(data: &[u8]) -> Result<(), ConsentError> {
    if data.len() < 22 {
        return Err(ConsentError::Malformed(format!(
            "vendor consent strings are at least 22 bytes long. This one was {}",
            data.len()
        )));
    }
    let max_vendor_id = max_vendor_id(data);
    if max_vendor_id < 1 {
        return Err(ConsentError::Malformed(format!(
            "the consent string encoded a MaxVendorID of {max_vendor_id}, but this value must be \
             greater than or equal to 1"
        )));
    }
    let version = common::version(data);
    if version < 1 {
        return Err(ConsentError::Malformed(format!(
            "the consent string encoded a Version of {version}, but this value must be greater \
             than or equal to 1"
        )));
    }
    if common::vendor_list_version(data) == 0 {
        return Err(ConsentError::InvalidVendorListVersion);
    }
    Ok(())
}

/// Go: `consentMetadata.MaxVendorID()`, bits 156-171.
pub(super) fn max_vendor_id(data: &[u8]) -> u16 {
    // The nibbles do not overlap, so Go's byte addition never carries.
    let left_byte = ((byte(data, 19) & 0x0F) << 4) | ((byte(data, 20) & 0xF0) >> 4);
    let right_byte = ((byte(data, 20) & 0x0F) << 4) | ((byte(data, 21) & 0xF0) >> 4);
    u16::from_be_bytes([left_byte, right_byte])
}
