//! Connecticut section (ID 12). Go: `sections/uspct/uspct.go`.

use super::section_util::{
    CommonUsCoreSegment, CommonUsGpcSegment, create_bit_streams, encode_with_gpc,
    gpc_segment_or_default, new_common_us_core_segment,
};
use crate::{
    constants::{SECTION_USPCT, SectionId},
    error::GppError,
};

/// Go: `uspct.USPCT`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Uspct {
    /// Go: `SectionID`.
    pub section_id: SectionId,
    /// The section string as given. Go: `Value`.
    pub value: String,
    /// Go: `CoreSegment`.
    pub core_segment: CommonUsCoreSegment,
    /// Go: `GPCSegment`.
    pub gpc_segment: CommonUsGpcSegment,
}

impl Uspct {
    /// Decodes a Connecticut section string (8 sensitive data fields, 3 known child fields).
    /// Go: `uspct.NewUSPCT`.
    ///
    /// # Errors
    ///
    /// Returns the base64 error of either segment, then the first core field error, then the
    /// GPC segment error.
    pub fn new(encoded: &str) -> Result<Self, GppError> {
        let (mut core_bit_stream, gpc_bit_stream) = create_bit_streams(encoded, true)?;
        let core_segment = new_common_us_core_segment(8, 3, &mut core_bit_stream)?;
        let gpc_segment = gpc_segment_or_default(gpc_bit_stream)?;
        Ok(Self {
            section_id: SECTION_USPCT,
            value: encoded.to_owned(),
            core_segment,
            gpc_segment,
        })
    }

    /// Encodes the section, appending `.` and the GPC segment when `gpc_included`.
    /// Go: `USPCT.Encode`.
    #[must_use]
    pub fn encode(&self, gpc_included: bool) -> Vec<u8> {
        encode_with_gpc(
            |bs| self.core_segment.encode(bs),
            self.gpc_segment,
            gpc_included,
        )
    }

    /// Go: `USPCT.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `USPCT.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    //! Go: `sections/uspct/uspct_test.go`.

    use super::Uspct;
    use crate::{
        constants::SECTION_USPCT,
        sections::{CommonUsCoreSegment, CommonUsGpcSegment},
    };

    #[test]
    fn uspct() {
        let gpp_string = "bSFgmSZQ.YA";
        // 011011 01 00 10 00 01 0110000010011001 001001 10 01 01 01 1 011
        let expected = Uspct {
            core_segment: CommonUsCoreSegment {
                version: 27,
                sharing_notice: 1,
                sale_opt_out_notice: 0,
                targeted_advertising_opt_out_notice: 2,
                sale_opt_out: 0,
                targeted_advertising_opt_out: 1,
                sensitive_data_processing: vec![1, 2, 0, 0, 2, 1, 2, 1],
                known_child_sensitive_data_consents: vec![0, 2, 1],
                mspa_covered_transaction: 2,
                mspa_opt_out_option_mode: 1,
                mspa_service_provider_mode: 1,
            },
            gpc_segment: CommonUsGpcSegment {
                subsection_type: 1,
                gpc: true,
            },
            section_id: SECTION_USPCT,
            value: gpp_string.to_owned(),
        };

        let result = Uspct::new(gpp_string);
        assert_eq!(result.as_ref().map(Uspct::id), Ok(SECTION_USPCT), "id");
        assert_eq!(result.as_ref().map(Uspct::value), Ok(gpp_string), "value");
        assert_eq!(expected.encode(true), gpp_string.as_bytes(), "encoded");
        assert_eq!(result, Ok(expected), "decoded");
    }
}
