//! Utah section (ID 11). Go: `sections/usput/usput.go`.

use super::section_util::{encode_core, error_helper};
use crate::{
    constants::{SECTION_USPUT, SectionId},
    error::GppError,
    util::BitStream,
};

/// Go: `usput.USPUTCoreSegment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UsputCoreSegment {
    /// Go: `Version`.
    pub version: u8,
    /// Go: `SharingNotice`.
    pub sharing_notice: u8,
    /// Go: `SaleOptOutNotice`.
    pub sale_opt_out_notice: u8,
    /// Go: `TargetedAdvertisingOptOutNotice`.
    pub targeted_advertising_opt_out_notice: u8,
    /// Go: `SensitiveDataProcessingOptOutNotice`.
    pub sensitive_data_processing_opt_out_notice: u8,
    /// Go: `SaleOptOut`.
    pub sale_opt_out: u8,
    /// Go: `TargetedAdvertisingOptOut`.
    pub targeted_advertising_opt_out: u8,
    /// Go: `SensitiveDataProcessing` (8 fields).
    pub sensitive_data_processing: Vec<u8>,
    /// A single 2-bit value in Utah. Go: `KnownChildSensitiveDataConsents`.
    pub known_child_sensitive_data_consents: u8,
    /// Go: `MspaCoveredTransaction`.
    pub mspa_covered_transaction: u8,
    /// Go: `MspaOptOutOptionMode`.
    pub mspa_opt_out_option_mode: u8,
    /// Go: `MspaServiceProviderMode`.
    pub mspa_service_provider_mode: u8,
}

/// Go: `usput.USPUT`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Usput {
    /// Go: `SectionID`.
    pub section_id: SectionId,
    /// The section string as given. Go: `Value`.
    pub value: String,
    /// Go: `CoreSegment`.
    pub core_segment: UsputCoreSegment,
}

impl UsputCoreSegment {
    /// Decodes the core segment. Go: `usput.NewUPSUTCoreSegment` (sic).
    ///
    /// # Errors
    ///
    /// Returns `unable to set field CoreSegment.<Field> due to parse error: ...` for the first
    /// field that cannot be read.
    pub fn new(bs: &mut BitStream) -> Result<Self, GppError> {
        let field = |name: &'static str| move |err: GppError| error_helper(name, &err);
        Ok(Self {
            version: bs.read_byte6().map_err(field("CoreSegment.Version"))?,
            sharing_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SharingNotice"))?,
            sale_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SaleOptOutNotice"))?,
            targeted_advertising_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.TargetedAdvertisingOptOutNotice"))?,
            sensitive_data_processing_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SensitiveDataProcessingOptOutNotice"))?,
            sale_opt_out: bs.read_byte2().map_err(field("CoreSegment.SaleOptOut"))?,
            targeted_advertising_opt_out: bs
                .read_byte2()
                .map_err(field("CoreSegment.TargetedAdvertisingOptOut"))?,
            sensitive_data_processing: bs
                .read_two_bit_field(8)
                .map_err(field("CoreSegment.SensitiveDataProcessing"))?,
            known_child_sensitive_data_consents: bs
                .read_byte2()
                .map_err(field("CoreSegment.KnownChildSensitiveDataConsents"))?,
            mspa_covered_transaction: bs
                .read_byte2()
                .map_err(field("CoreSegment.MspaCoveredTransaction"))?,
            mspa_opt_out_option_mode: bs
                .read_byte2()
                .map_err(field("CoreSegment.MspaOptOutOptionMode"))?,
            mspa_service_provider_mode: bs
                .read_byte2()
                .map_err(field("CoreSegment.MspaServiceProviderMode"))?,
        })
    }

    /// Writes the segment. Go: `USPUTCoreSegment.Encode`.
    pub fn encode(&self, bs: &mut BitStream) {
        bs.write_byte6(self.version);
        bs.write_byte2(self.sharing_notice);
        bs.write_byte2(self.sale_opt_out_notice);
        bs.write_byte2(self.targeted_advertising_opt_out_notice);
        bs.write_byte2(self.sensitive_data_processing_opt_out_notice);
        bs.write_byte2(self.sale_opt_out);
        bs.write_byte2(self.targeted_advertising_opt_out);
        bs.write_two_bit_field(&self.sensitive_data_processing);
        bs.write_byte2(self.known_child_sensitive_data_consents);
        bs.write_byte2(self.mspa_covered_transaction);
        bs.write_byte2(self.mspa_opt_out_option_mode);
        bs.write_byte2(self.mspa_service_provider_mode);
    }
}

impl Usput {
    /// Decodes a Utah section string. Go: `usput.NewUSPUT`.
    ///
    /// The whole string is decoded as one base64 segment (Utah has no GPC segment, so a `.` is a
    /// base64 error).
    ///
    /// # Errors
    ///
    /// Returns the base64 error or the first core field error.
    pub fn new(encoded: &str) -> Result<Self, GppError> {
        let mut bit_stream = BitStream::from_base64(encoded)?;
        let core_segment = UsputCoreSegment::new(&mut bit_stream)?;
        Ok(Self {
            section_id: SECTION_USPUT,
            value: encoded.to_owned(),
            core_segment,
        })
    }

    /// Encodes the section; the argument is ignored. Go: `USPUT.Encode`.
    #[must_use]
    pub fn encode(&self, _gpc_included: bool) -> Vec<u8> {
        encode_core(|bs| self.core_segment.encode(bs))
    }

    /// Go: `USPUT.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `USPUT.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    //! Go: `sections/usput/usput_test.go`.

    use super::{Usput, UsputCoreSegment};
    use crate::constants::SECTION_USPUT;

    #[test]
    fn usput() {
        let gpp_string = "bSRYJllA";
        // 011011 01 00 10 01 00 01 0110000010011001 01 10 01 01
        let expected = Usput {
            core_segment: UsputCoreSegment {
                version: 27,
                sharing_notice: 1,
                sale_opt_out_notice: 0,
                targeted_advertising_opt_out_notice: 2,
                sensitive_data_processing_opt_out_notice: 1,
                sale_opt_out: 0,
                targeted_advertising_opt_out: 1,
                sensitive_data_processing: vec![1, 2, 0, 0, 2, 1, 2, 1],
                known_child_sensitive_data_consents: 1,
                mspa_covered_transaction: 2,
                mspa_opt_out_option_mode: 1,
                mspa_service_provider_mode: 1,
            },
            section_id: SECTION_USPUT,
            value: gpp_string.to_owned(),
        };

        let result = Usput::new(gpp_string);
        assert_eq!(result.as_ref().map(Usput::id), Ok(SECTION_USPUT), "id");
        assert_eq!(result.as_ref().map(Usput::value), Ok(gpp_string), "value");
        assert_eq!(expected.encode(true), gpp_string.as_bytes(), "encoded");
        assert_eq!(result, Ok(expected), "decoded");
    }
}
