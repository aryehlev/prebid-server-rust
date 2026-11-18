//! Virginia section (ID 9). Go: `sections/uspva/uspva.go`.

use super::section_util::{CommonUsCoreSegment, encode_core, new_common_us_core_segment};
use crate::{
    constants::{SECTION_USPVA, SectionId},
    error::GppError,
    util::BitStream,
};

/// Go: `uspva.USPVA`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Uspva {
    /// Go: `SectionID`.
    pub section_id: SectionId,
    /// The section string as given. Go: `Value`.
    pub value: String,
    /// Go: `CoreSegment`.
    pub core_segment: CommonUsCoreSegment,
}

impl Uspva {
    /// Decodes a Virginia section string. Go: `uspva.NewUSPVA`.
    ///
    /// The whole string is decoded as one base64 segment (Virginia has no GPC segment, so a `.`
    /// is a base64 error). `KnownChildSensitiveDataConsents` is a one-element vector.
    ///
    /// # Errors
    ///
    /// Returns the base64 error or the first core field error.
    pub fn new(encoded: &str) -> Result<Self, GppError> {
        let mut bit_stream = BitStream::from_base64(encoded)?;
        let core_segment = new_common_us_core_segment(8, 1, &mut bit_stream)?;
        Ok(Self {
            section_id: SECTION_USPVA,
            value: encoded.to_owned(),
            core_segment,
        })
    }

    /// Encodes the section; the argument is ignored. Go: `USPVA.Encode`.
    #[must_use]
    pub fn encode(&self, _gpc_included: bool) -> Vec<u8> {
        encode_core(|bs| self.core_segment.encode(bs))
    }

    /// Go: `USPVA.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `USPVA.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    //! Go: `sections/uspva/uspva_test.go`.

    use super::Uspva;
    use crate::{constants::SECTION_USPVA, sections::CommonUsCoreSegment};

    #[test]
    fn uspva() {
        let gpp_string = "bSFgmiU";
        // 011011 01 00 10 00 01 0110000010011010 00 10 01 01
        let expected = Uspva {
            core_segment: CommonUsCoreSegment {
                version: 27,
                sharing_notice: 1,
                sale_opt_out_notice: 0,
                targeted_advertising_opt_out_notice: 2,
                sale_opt_out: 0,
                targeted_advertising_opt_out: 1,
                sensitive_data_processing: vec![1, 2, 0, 0, 2, 1, 2, 2],
                known_child_sensitive_data_consents: vec![0],
                mspa_covered_transaction: 2,
                mspa_opt_out_option_mode: 1,
                mspa_service_provider_mode: 1,
            },
            section_id: SECTION_USPVA,
            value: gpp_string.to_owned(),
        };

        let result = Uspva::new(gpp_string);
        assert_eq!(result.as_ref().map(Uspva::id), Ok(SECTION_USPVA), "id");
        assert_eq!(result.as_ref().map(Uspva::value), Ok(gpp_string), "value");
        assert_eq!(expected.encode(true), gpp_string.as_bytes(), "encoded");
        assert_eq!(result, Ok(expected), "decoded");
    }
}
