//! California section (ID 8). Go: `sections/uspca/uspca.go`.

use super::section_util::{
    CommonUsGpcSegment, create_bit_streams, encode_with_gpc, error_helper, gpc_segment_or_default,
};
use crate::{
    constants::{SECTION_USPCA, SectionId},
    error::GppError,
    util::BitStream,
};

/// Go: `uspca.USPCACoreSegment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UspcaCoreSegment {
    /// Go: `Version`.
    pub version: u8,
    /// Go: `SaleOptOutNotice`.
    pub sale_opt_out_notice: u8,
    /// Go: `SharingOptOutNotice`.
    pub sharing_opt_out_notice: u8,
    /// Go: `SensitiveDataLimitUseNotice`.
    pub sensitive_data_limit_use_notice: u8,
    /// Go: `SaleOptOut`.
    pub sale_opt_out: u8,
    /// Go: `SharingOptOut`.
    pub sharing_opt_out: u8,
    /// Go: `SensitiveDataProcessing` (9 fields).
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

/// Go: `uspca.USPCA`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Uspca {
    /// Go: `SectionID`.
    pub section_id: SectionId,
    /// The section string as given. Go: `Value`.
    pub value: String,
    /// Go: `CoreSegment`.
    pub core_segment: UspcaCoreSegment,
    /// Go: `GPCSegment`.
    pub gpc_segment: CommonUsGpcSegment,
}

impl UspcaCoreSegment {
    /// Decodes the core segment. Go: `uspca.NewUSPCACoreSegment`.
    ///
    /// Go's field labels are reproduced verbatim, including `CoreSegment.SalesOptOutNotice`,
    /// `CoreSegment.SalesOptOut`, and `CoreSegment.Version` for `SensitiveDataLimitUseNotice`.
    ///
    /// # Errors
    ///
    /// Returns `unable to set field CoreSegment.<Field> due to parse error: ...` for the first
    /// field that cannot be read.
    pub fn new(bs: &mut BitStream) -> Result<Self, GppError> {
        let field = |name: &'static str| move |err: GppError| error_helper(name, &err);
        Ok(Self {
            version: bs.read_byte6().map_err(field("CoreSegment.Version"))?,
            sale_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SalesOptOutNotice"))?,
            sharing_opt_out_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.SharingOptOutNotice"))?,
            sensitive_data_limit_use_notice: bs
                .read_byte2()
                .map_err(field("CoreSegment.Version"))?,
            sale_opt_out: bs.read_byte2().map_err(field("CoreSegment.SalesOptOut"))?,
            sharing_opt_out: bs
                .read_byte2()
                .map_err(field("CoreSegment.SharingOptOut"))?,
            sensitive_data_processing: bs
                .read_two_bit_field(9)
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

    /// Writes the segment. Go: `USPCACoreSegment.Encode`.
    pub fn encode(&self, bs: &mut BitStream) {
        bs.write_byte6(self.version);
        bs.write_byte2(self.sale_opt_out_notice);
        bs.write_byte2(self.sharing_opt_out_notice);
        bs.write_byte2(self.sensitive_data_limit_use_notice);
        bs.write_byte2(self.sale_opt_out);
        bs.write_byte2(self.sharing_opt_out);
        bs.write_two_bit_field(&self.sensitive_data_processing);
        bs.write_two_bit_field(&self.known_child_sensitive_data_consents);
        bs.write_byte2(self.personal_data_consents);
        bs.write_byte2(self.mspa_covered_transaction);
        bs.write_byte2(self.mspa_opt_out_option_mode);
        bs.write_byte2(self.mspa_service_provider_mode);
    }
}

impl Uspca {
    /// Decodes a California section string. Go: `uspca.NewUSPCA`.
    ///
    /// # Errors
    ///
    /// Returns the base64 error of either segment, then the first core field error, then the
    /// GPC segment error.
    pub fn new(encoded: &str) -> Result<Self, GppError> {
        let (mut core_bit_stream, gpc_bit_stream) = create_bit_streams(encoded, true)?;
        let core_segment = UspcaCoreSegment::new(&mut core_bit_stream)?;
        let gpc_segment = gpc_segment_or_default(gpc_bit_stream)?;
        Ok(Self {
            section_id: SECTION_USPCA,
            value: encoded.to_owned(),
            core_segment,
            gpc_segment,
        })
    }

    /// Encodes the section, appending `.` and the GPC segment when `gpc_included`.
    /// Go: `USPCA.Encode`.
    #[must_use]
    pub fn encode(&self, gpc_included: bool) -> Vec<u8> {
        encode_with_gpc(
            |bs| self.core_segment.encode(bs),
            self.gpc_segment,
            gpc_included,
        )
    }

    /// Go: `USPCA.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `USPCA.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    //! Go: `sections/uspca/uspca_test.go`.

    use super::{Uspca, UspcaCoreSegment};
    use crate::{constants::SECTION_USPCA, sections::CommonUsGpcSegment};

    #[test]
    fn uspca() {
        let gpp_string = "xlgWEYCY.YA";
        // 110001 10 01 01 10 00 000101100001000110 0000 00 10 01 10 01 0 011
        let expected = Uspca {
            core_segment: UspcaCoreSegment {
                version: 49,
                sale_opt_out_notice: 2,
                sharing_opt_out_notice: 1,
                sensitive_data_limit_use_notice: 1,
                sale_opt_out: 2,
                sharing_opt_out: 0,
                sensitive_data_processing: vec![0, 1, 1, 2, 0, 1, 0, 1, 2],
                known_child_sensitive_data_consents: vec![0, 0],
                personal_data_consents: 0,
                mspa_covered_transaction: 2,
                mspa_opt_out_option_mode: 1,
                mspa_service_provider_mode: 2,
            },
            gpc_segment: CommonUsGpcSegment {
                subsection_type: 1,
                gpc: true,
            },
            section_id: SECTION_USPCA,
            value: gpp_string.to_owned(),
        };

        let result = Uspca::new(gpp_string);
        assert_eq!(result.as_ref().map(Uspca::id), Ok(SECTION_USPCA), "id");
        assert_eq!(result.as_ref().map(Uspca::value), Ok(gpp_string), "value");
        assert_eq!(expected.encode(true), gpp_string.as_bytes(), "encoded");
        assert_eq!(result, Ok(expected), "decoded");
    }
}
