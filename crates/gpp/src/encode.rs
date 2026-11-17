//! GPP string encoding. Go: `encode.go`.
//!
//! Prebid Server never calls `gpp.Encode` in production; it is ported because it is part of
//! go-gpp's public API, and go-gpp's own tests (and the Rust golden tests) use it to round-trip
//! parsed strings.

use crate::{
    constants::SectionId,
    error::GppError,
    parse::Section,
    util::{BitStream, IRange, IntRange},
};

/// The first 6 bits of the header must always evaluate to the integer '3'. Go: `gppType`.
const GPP_TYPE: u8 = 0x3;
/// Go: `gppVersion`.
const GPP_VERSION: u8 = 0x1;
/// The range of section IDs starts at 1. Go: `minSectionId`.
const MIN_SECTION_ID: SectionId = SectionId(1);
/// ...and ends at the maximum `uint16`. Go: `maxSectionId`.
const MAX_SECTION_ID: SectionId = SectionId(0xFFFF);

/// Go: `sectionIdOutOfRangeErr`.
const SECTION_ID_OUT_OF_RANGE: &str = "section ID out of range";
/// Go: `duplicatedSectionErr`.
const DUPLICATED_SECTIONS: &str = "duplicated sections";

/// Encodes sections into a GPP string with header version 1. Go: `gpp.Encode`.
///
/// Like Go, this sorts `sections` in place by ID before encoding (Go uses the unstable
/// `sort.Slice`; the order of equal IDs only matters when encoding fails with
/// `duplicated sections`).
///
/// # Errors
///
/// Returns `section ID out of range` when an ID is outside `1..=65535`, `duplicated sections`
/// when an ID repeats, or `write int range error: ...` when the Fibonacci header cannot encode
/// an ID offset (offsets of 6765 or more).
pub fn encode(sections: &mut [Section]) -> Result<String, GppError> {
    let mut bs = BitStream::new_for_write();

    bs.write_byte6(GPP_TYPE);
    bs.write_byte6(GPP_VERSION);

    sections.sort_by_key(Section::id);

    if let (Some(first), Some(last)) = (sections.first(), sections.last())
        && (first.id() < MIN_SECTION_ID || last.id() > MAX_SECTION_ID)
    {
        return Err(GppError::new(SECTION_ID_OUT_OF_RANGE.to_owned()));
    }
    // Generate the int range. The previous ID starts at -1 so that ID 1 is not continuous.
    let mut ranges: Vec<IRange> = Vec::new();
    let mut prev_id = SectionId(-1);
    for sec in sections.iter() {
        let id = sec.id();
        if id == prev_id {
            return Err(GppError::new(DUPLICATED_SECTIONS.to_owned()));
        }
        // In range after the check above, so Go's `uint16(id)` does not truncate.
        let id16 = u16::try_from(id.0).unwrap_or_default();
        match ranges.last_mut() {
            Some(last) if prev_id.0 + 1 == id.0 => last.end_id = id16,
            _ => ranges.push(IRange {
                start_id: id16,
                end_id: id16,
            }),
        }
        prev_id = id;
    }
    // Go: `uint16(len(intRange.Range))`, which wraps.
    let size = u16::try_from(ranges.len() % 65_536).unwrap_or_default();
    let int_range = IntRange {
        size,
        range: ranges,
        max: 0,
    };

    bs.write_int_range(&int_range)
        .map_err(|err| GppError::new(format!("write int range error: {err}")))?;

    let mut out = bs.base64_encode();
    for sec in sections.iter() {
        out.push(b'~');
        // By default, GPC is included.
        out.extend_from_slice(&sec.encode(true));
    }

    // Every byte is either base64url or comes from a section value, which is a `String`.
    Ok(String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    //! Go: `encode_test.go`.

    use super::encode;
    use crate::{
        constants::{
            SECTION_USPCA, SECTION_USPCO, SECTION_USPCT, SECTION_USPNAT, SECTION_USPUT,
            SECTION_USPVA, SectionId,
        },
        error::GppError,
        parse::{GenericSection, Section, parse},
        sections::{
            CommonUsCoreSegment, CommonUsGpcSegment,
            uspca::{Uspca, UspcaCoreSegment},
            uspco::Uspco,
            uspct::Uspct,
            uspnat::{Uspnat, UspnatCoreSegment},
            usput::{Usput, UsputCoreSegment},
            uspva::Uspva,
        },
    };

    const GPC_TRUE: CommonUsGpcSegment = CommonUsGpcSegment {
        subsection_type: 1,
        gpc: true,
    };

    fn common_core(version: u8, sdp: &[u8], kc: &[u8]) -> CommonUsCoreSegment {
        CommonUsCoreSegment {
            version,
            sharing_notice: 1,
            sale_opt_out_notice: 0,
            targeted_advertising_opt_out_notice: 2,
            sale_opt_out: 0,
            targeted_advertising_opt_out: 1,
            sensitive_data_processing: sdp.to_vec(),
            known_child_sensitive_data_consents: kc.to_vec(),
            mspa_covered_transaction: 2,
            mspa_opt_out_option_mode: 1,
            mspa_service_provider_mode: 1,
        }
    }

    fn uspva(version: u8, value: &str) -> Section {
        Section::Uspva(Uspva {
            core_segment: common_core(version, &[1, 2, 0, 0, 2, 1, 2, 2], &[0]),
            section_id: SECTION_USPVA,
            value: value.to_owned(),
        })
    }

    fn uspct(version: u8, value: &str) -> Section {
        Section::Uspct(Uspct {
            core_segment: common_core(version, &[1, 2, 0, 0, 2, 1, 2, 1], &[0, 2, 1]),
            gpc_segment: GPC_TRUE,
            section_id: SECTION_USPCT,
            value: value.to_owned(),
        })
    }

    fn uspnat(version: u8, value: &str) -> Section {
        Section::Uspnat(Uspnat {
            core_segment: UspnatCoreSegment {
                version,
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
            gpc_segment: GPC_TRUE,
            section_id: SECTION_USPNAT,
            value: value.to_owned(),
        })
    }

    fn uspca() -> Section {
        Section::Uspca(Uspca {
            core_segment: UspcaCoreSegment {
                version: 1,
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
                gpc: false,
            },
            section_id: SECTION_USPCA,
            value: "BlgWEYCY.QA".to_owned(),
        })
    }

    fn usput() -> Section {
        Section::Usput(Usput {
            core_segment: UsputCoreSegment {
                version: 1,
                sharing_notice: 1,
                sale_opt_out_notice: 1,
                targeted_advertising_opt_out_notice: 2,
                sensitive_data_processing_opt_out_notice: 0,
                sale_opt_out: 2,
                targeted_advertising_opt_out: 1,
                sensitive_data_processing: vec![1, 2, 0, 0, 2, 1, 2, 1],
                known_child_sensitive_data_consents: 1,
                mspa_covered_transaction: 2,
                mspa_opt_out_option_mode: 1,
                mspa_service_provider_mode: 1,
            },
            section_id: SECTION_USPUT,
            value: "BWJYJllA".to_owned(),
        })
    }

    /// Description, sections, expected string, expected error.
    type EncodeCase = (
        &'static str,
        Vec<Section>,
        &'static str,
        Option<&'static str>,
    );

    /// Go: `testData` in `encode_test.go`.
    fn test_data() -> Vec<EncodeCase> {
        vec![
            (
                "USPCA GPP string encoding",
                vec![uspca(), uspva(1, "BSFgmiU")],
                "DBABh4A~BlgWEYCY.QA~BSFgmiU",
                None,
            ),
            (
                "USPVA GPP string encoding",
                vec![uspva(27, "bSFgmiU")],
                "DBABRg~bSFgmiU",
                None,
            ),
            (
                "USPCO GPP string encoding",
                vec![Section::Uspco(Uspco {
                    core_segment: common_core(27, &[1, 2, 0, 0, 2, 1, 2], &[0]),
                    gpc_segment: GPC_TRUE,
                    section_id: SECTION_USPCO,
                    value: "bSFgmJQ.YA".to_owned(),
                })],
                "DBABJg~bSFgmJQ.YA",
                None,
            ),
            (
                "USPCT GPP string encoding",
                vec![uspct(27, "bSFgmSZQ.YA")],
                "DBABVg~bSFgmSZQ.YA",
                None,
            ),
            (
                "USPNAT GPP string encoding",
                vec![uspnat(3, "DSJgmkoZJSA.YA")],
                "DBABLA~DSJgmkoZJSA.YA",
                None,
            ),
            (
                "GPP string encoding for multiple sections",
                vec![
                    usput(),
                    uspnat(1, "BSJgmkoZJSA.YA"),
                    uspct(1, "BSFgmSZQ.YA"),
                    uspva(1, "BSFgmiU"),
                ],
                "DBADLO8~BSJgmkoZJSA.YA~BSFgmiU~BWJYJllA~BSFgmSZQ.YA",
                None,
            ),
            (
                "GPP string encoding minimum section ID",
                vec![Section::Generic(GenericSection::new(
                    SectionId(1),
                    "test_minimum".to_owned(),
                ))],
                "DBABYA~test_minimum",
                None,
            ),
            (
                "GPP string encoding section ID zero",
                vec![Section::Generic(GenericSection::new(
                    SectionId(0),
                    "section_with_id_zero".to_owned(),
                ))],
                "",
                Some("section ID out of range"),
            ),
        ]
    }

    #[test]
    fn encode_cases() {
        for (description, sections, expected, err) in test_data() {
            let mut to_encode = sections.clone();
            let result = encode(&mut to_encode);
            assert_eq!(
                result.as_ref().err().map(GppError::message),
                err,
                "{description}: error"
            );
            assert_eq!(
                result.as_deref().unwrap_or_default(),
                expected,
                "{description}: encoded"
            );
            if result.is_err() {
                continue;
            }

            // The GPP string must parse back into the original sections.
            let (container, errs) = parse(expected);
            assert!(errs.is_empty(), "{description}: parse errors {errs:?}");
            for section in &sections {
                let parsed = container.sections.iter().find(|s| s.id() == section.id());
                assert_eq!(
                    parsed,
                    Some(section),
                    "{description}: section {}",
                    section.id()
                );
            }
        }
    }

    /// Go: `TestEncode2`. Decoding and re-encoding gives back the original strings.
    #[test]
    fn encode_round_trip() {
        let gpp_strings = [
            "DBABh4A~BlgWEYCY.QA~BSFgmiU",
            "DBABRg~bSFgmiU",
            "DBABJg~bSFgmJQ.YA",
            "DBABVg~bSFgmSZQ.YA",
            "DBABLA~DSJgmkoZJSA.YA",
            "DBADLO8~BSJgmkoZJSA.YA~BSFgmiU~BWJYJllA~BSFgmSZQ.YA",
            "DBABrGA~DSJgmkoZJSA.YA~BlgWEYCY.QA~BSFgmiU~bSFgmJQ.YA~BWJYJllA~bSFgmSZQ.YA",
            "DBACLMA~BAAAAAAAAAA.QA~BaAAAAA",
            "DBACTjw~1YYN~BSZZYgkA~BaRlkCSA.QA",
            "DBACMYA~CPpcCoAPpcCoAPoABABGCyCUACAAACAAAAAAAVQAQAVABZABABYAAAAA.QADgIAAA.IABE~\
             CPpcCoAPpcCoAPoABABGCyCQAEAAAEAAAAEFABAEEAN8AEAN4A.YAAAAAAAAAA",
        ];
        for s in gpp_strings {
            let (mut container, errs) = parse(s);
            assert!(errs.is_empty(), "{s}: parse errors {errs:?}");
            assert_eq!(encode(&mut container.sections).as_deref(), Ok(s), "{s}");
        }
    }

    #[test]
    fn duplicated_and_out_of_range_ids() {
        let generic = |id: i64| Section::Generic(GenericSection::new(SectionId(id), String::new()));
        let cases: &[(&[i64], &str)] = &[
            (&[3, 3], "duplicated sections"),
            (&[2, 65_536], "section ID out of range"),
            (&[-1, 4], "section ID out of range"),
            (
                &[1, 7000],
                "write int range error: the number to be encoded is out of range",
            ),
        ];
        for &(ids, want) in cases {
            let mut sections: Vec<Section> = ids.iter().map(|&id| generic(id)).collect();
            let got = encode(&mut sections).err();
            assert_eq!(got.as_ref().map(GppError::message), Some(want), "{ids:?}");
        }
    }

    #[test]
    fn sorts_sections_in_place() {
        let mut sections = vec![
            Section::Generic(GenericSection::new(SectionId(6), "1YNN".to_owned())),
            Section::Generic(GenericSection::new(SectionId(2), "tcf".to_owned())),
        ];
        assert_eq!(
            encode(&mut sections).as_deref(),
            Ok("DBACNYA~tcf~1YNN"),
            "encoded"
        );
        let ids: Vec<SectionId> = sections.iter().map(Section::id).collect();
        assert_eq!(ids, [SectionId(2), SectionId(6)], "sorted");
    }
}
