//! US national section (ID 7). Go: `sections/uspnat/uspnat.go`.

use super::section_util::{
    CommonUsGpcSegment, create_bit_streams, encode_with_gpc, error_helper, gpc_segment_or_default,
};
use crate::{
    constants::{SECTION_USPNAT, SectionId},
    error::GppError,
    util::BitStream,
};

/// Go: `uspnat.USPNATCoreSegment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UspnatCoreSegment {
    /// Go: `Version`.
    pub version: u8,
    /// Go: `SharingNotice`.
    pub sharing_notice: u8,
    /// Go: `SaleOptOutNotice`.
    pub sale_opt_out_notice: u8,
    /// Go: `SharingOptOutNotice`.
    pub sharing_opt_out_notice: u8,
    /// Go: `TargetedAdvertisingOptOutNotice`.
    pub targeted_advertising_opt_out_notice: u8,
    /// Go: `SensitiveDataProcessingOptOutNotice`.
    pub sensitive_data_processing_opt_out_notice: u8,
    /// Go: `SensitiveDataLimitUseNotice`.
    pub sensitive_data_limit_use_notice: u8,
    /// Go: `SaleOptOut`.
    pub sale_opt_out: u8,
    /// Go: `SharingOptOut`.
    pub sharing_opt_out: u8,
    /// Go: `TargetedAdvertisingOptOut`.
    pub targeted_advertising_opt_out: u8,
    /// Go: `SensitiveDataProcessing` (12 fields).
    pub sensitive_data_processing: Vec<u8>,
    /// Go: `KnownChildSensitiveDataConsents` (2 fields).
    pub known_child_sensitive_data_consents: Vec<u8>,
    /// Go: `PersonalDataConsents`.
    pub personal_data_consents: u8,
    /// Go: `MspaCoveredTransaction`.
    pub mspa_covered_transaction: u8,
    /// Go: `MspaOptOutOptionMode`.
    pub mspa_opt_out_option_mode: u8,
    /// Go: `MspaServiceProviderMode`.
    pub mspa_service_provider_mode: u8,
}

/// Go: `uspnat.USPNAT`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Uspnat {
    /// Go: `SectionID`.
    pub section_id: SectionId,
    /// The section string as given. Go: `Value`.
    pub value: String,
    /// Go: `CoreSegment`.
    pub core_segment: UspnatCoreSegment,
    /// Go: `GPCSegment`.
    pub gpc_segment: CommonUsGpcSegment,
}

impl UspnatCoreSegment {
    /// Decodes the core segment. Go: `uspnat.NewUSPNATCoreSegment`.
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
            sharing_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SharingOptOutNotice"))?,
            targeted_advertising_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.TargetedAdvertisingOptOutNotice"))?,
            sensitive_data_processing_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SensitiveDataProcessingOptOutNotice"))?,
            sensitive_data_limit_use_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SensitiveDataLimitUseNotice"))?,
            sale_opt_out: bs.read_byte2().map_err(field("CoreSegment.SaleOptOut"))?,
            sharing_opt_out: bs
                .read_byte2()
                .map_err(field("CoreSegment.SharingOptOut"))?,
            targeted_advertising_opt_out: bs
                .read_byte2()
                .map_err(field("CoreSegment.TargetedAdvertisingOptOut"))?,
            sensitive_data_processing: bs
                .read_two_bit_field(12)
                .map_err(field("CoreSegment.SensitiveDataProcessing"))?,
            known_child_sensitive_data_consents: bs
                .read_two_bit_field(2)
                .map_err(field("CoreSegment.KnownChildSensitiveDataConsents"))?,
            personal_data_consents: bs
                .read_byte2()
                .map_err(field("CoreSegment.PersonalDataConsents"))?,
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

    /// Writes the segment. Go: `USPNATCoreSegment.Encode`.
    pub fn encode(&self, bs: &mut BitStream) {
        bs.write_byte6(self.version);
        bs.write_byte2(self.sharing_notice);
        bs.write_byte2(self.sale_opt_out_notice);
        bs.write_byte2(self.sharing_opt_out_notice);
        bs.write_byte2(self.targeted_advertising_opt_out_notice);
        bs.write_byte2(self.sensitive_data_processing_opt_out_notice);
        bs.write_byte2(self.sensitive_data_limit_use_notice);
        bs.write_byte2(self.sale_opt_out);
        bs.write_byte2(self.sharing_opt_out);
        bs.write_byte2(self.targeted_advertising_opt_out);
        bs.write_two_bit_field(&self.sensitive_data_processing);
        bs.write_two_bit_field(&self.known_child_sensitive_data_consents);
        bs.write_byte2(self.personal_data_consents);
        bs.write_byte2(self.mspa_covered_transaction);
        bs.write_byte2(self.mspa_opt_out_option_mode);
        bs.write_byte2(self.mspa_service_provider_mode);
    }
}

impl Uspnat {
    /// Decodes a US national section string. Go: `uspnat.NewUSPNAT`.
    ///
    /// # Errors
    ///
    /// Returns the base64 error of either segment, then the first core field error, then the
    /// GPC segment error.
    pub fn new(encoded: &str) -> Result<Self, GppError> {
        let (mut core_bit_stream, gpc_bit_stream) = create_bit_streams(encoded, true)?;
        let core_segment = UspnatCoreSegment::new(&mut core_bit_stream)?;
        let gpc_segment = gpc_segment_or_default(gpc_bit_stream)?;
        Ok(Self {
            section_id: SECTION_USPNAT,
            value: encoded.to_owned(),
            core_segment,
            gpc_segment,
        })
    }

    /// Encodes the section, appending `.` and the GPC segment when `gpc_included`.
    /// Go: `USPNAT.Encode`.
    #[must_use]
    pub fn encode(&self, gpc_included: bool) -> Vec<u8> {
        encode_with_gpc(
            |bs| self.core_segment.encode(bs),
            self.gpc_segment,
            gpc_included,
        )
    }

    /// Go: `USPNAT.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `USPNAT.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    //! Go: `sections/uspnat/uspnat_test.go`.

    use super::{Uspnat, UspnatCoreSegment};
    use crate::{constants::SECTION_USPNAT, sections::CommonUsGpcSegment};

    #[test]
    fn uspnat() {
        let gpp_string = "DSJgmkoZJSA.YA";
        // 000011 01 00 10 00 10 01 10 00 00 100110100100101000011001 0010 01 01 00 10 01 1 011
        let expected = Uspnat {
            core_segment: UspnatCoreSegment {
                version: 3,
                sharing_notice: 1,
                sale_opt_out_notice: 0,
                sharing_opt_out_notice: 2,
                targeted_advertising_opt_out_notice: 0,
                sensitive_data_processing_opt_out_notice: 2,
                sensitive_data_limit_use_notice: 1,
                sale_opt_out: 2,
                sharing_opt_out: 0,
                targeted_advertising_opt_out: 0,
                sensitive_data_processing: vec![2, 1, 2, 2, 1, 0, 2, 2, 0, 1, 2, 1],
                known_child_sensitive_data_consents: vec![0, 2],
                personal_data_consents: 1,
                mspa_covered_transaction: 1,
                mspa_opt_out_option_mode: 0,
                mspa_service_provider_mode: 2,
            },
            gpc_segment: CommonUsGpcSegment {
                subsection_type: 1,
                gpc: true,
            },
            section_id: SECTION_USPNAT,
            value: gpp_string.to_owned(),
        };

        let result = Uspnat::new(gpp_string);
        assert_eq!(result.as_ref().map(Uspnat::id), Ok(SECTION_USPNAT), "id");
        assert_eq!(result.as_ref().map(Uspnat::value), Ok(gpp_string), "value");
        assert_eq!(expected.encode(true), gpp_string.as_bytes(), "encoded");
        assert_eq!(result, Ok(expected), "decoded");
    }
}
