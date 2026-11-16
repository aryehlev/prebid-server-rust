//! Go: `parse_test.go`, plus cases for the partial-parse and stale-error behaviour.

use crate::{
    GenericSection, GppContainer, GppError, Section,
    constants::{SECTION_USPCA, SECTION_USPVA, SectionId},
    parse,
    sections::{
        CommonUsCoreSegment, CommonUsGpcSegment,
        uspca::{Uspca, UspcaCoreSegment},
        uspva::Uspva,
    },
    util::BitStream,
};

const TCF: &str = "CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA";

fn generic(id: i64, value: &str) -> Section {
    Section::Generic(GenericSection::new(SectionId(id), value.to_owned()))
}

fn ids(list: &[i64]) -> Vec<SectionId> {
    list.iter().copied().map(SectionId).collect()
}

fn messages(errs: &[GppError]) -> Vec<&str> {
    errs.iter().map(GppError::message).collect()
}

/// Go: `TestParse` success rows.
#[test]
fn parse_valid() {
    let cases = vec![
        (
            "gpp-tcf",
            "DBABM~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
            GppContainer {
                version: 1,
                section_types: ids(&[2]),
                sections: vec![generic(2, TCF)],
            },
        ),
        (
            "gpp-tcf-valid-quantum",
            "DBABMA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
            GppContainer {
                version: 1,
                section_types: ids(&[2]),
                sections: vec![generic(2, TCF)],
            },
        ),
        (
            "gpp-tcf-usp",
            "DBACNY~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1YNN",
            GppContainer {
                version: 1,
                section_types: ids(&[2, 6]),
                sections: vec![generic(2, TCF), generic(6, "1YNN")],
            },
        ),
        (
            "gpp-tcfca-usp",
            "DBABjw~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1YNN",
            GppContainer {
                version: 1,
                section_types: ids(&[5, 6]),
                sections: vec![generic(5, TCF), generic(6, "1YNN")],
            },
        ),
        (
            "gpp-uspca",
            "DBABBgA~xlgWEYCZAA",
            GppContainer {
                version: 1,
                section_types: ids(&[8]),
                sections: vec![Section::Uspca(Uspca {
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
                        gpc: false,
                    },
                    section_id: SECTION_USPCA,
                    value: "xlgWEYCZAA".to_owned(),
                })],
            },
        ),
        (
            "gpp-uspva",
            "DBABRgA~bSFgmiU",
            GppContainer {
                version: 1,
                section_types: ids(&[9]),
                sections: vec![Section::Uspva(Uspva {
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
                    value: "bSFgmiU".to_owned(),
                })],
            },
        ),
    ];
    for (name, gpp_string, expected) in cases {
        let (result, errs) = parse(gpp_string);
        assert!(errs.is_empty(), "{name}: unexpected errors {errs:?}");
        assert_eq!(result, expected, "{name}");
    }
}

/// Go: `TestParse` error rows.
#[test]
fn parse_errors() {
    let cases = [
        (
            "gpp-tcf-error",
            "DBGBM~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA",
            "error parsing GPP header, section identifiers: error reading an int offset value in \
             a Range(Fibonacci) entry(1): error reading bit 4 of Integer(Fibonacci): expected 1 \
             bit at bit 32, but the byte array was only 4 bytes long",
        ),
        (
            "gpp-uspca-error",
            "DBABBgA~xlgWE",
            "error parsing uspca consent string: unable to set field \
             CoreSegment.SensitiveDataProcessing due to parse error: expected 2 bits to start at \
             bit 32, but the byte array was only 4 bytes long",
        ),
    ];
    for (name, gpp_string, expected) in cases {
        let (_, errs) = parse(gpp_string);
        assert_eq!(messages(&errs), [expected], "{name}");
    }
}

#[test]
fn header_errors() {
    let cases = [
        (
            "",
            "error parsing GPP header, should be at least 3 bytes long",
        ),
        (
            "DB",
            "error parsing GPP header, should be at least 3 bytes long",
        ),
        (
            "D=AB",
            "error parsing GPP header, base64 decoding: illegal base64 data at input byte 1",
        ),
        (
            "CBABMA~x",
            "error parsing GPP header, header must have type=3",
        ),
        (
            "DBABMA",
            "error parsing GPP header, section IDs do not match the number of sections: found 1 \
             IDs, have 0 sections",
        ),
        (
            "DBABMA~a~b",
            "error parsing GPP header, section IDs do not match the number of sections: found 1 \
             IDs, have 2 sections",
        ),
    ];
    for (input, expected) in cases {
        let (_, errs) = parse(input);
        assert_eq!(messages(&errs), [expected], "{input:?}");
    }
}

#[test]
fn header_error_keeps_version() {
    let (gpp, errs) = parse("DBABMA~a~b");
    assert_eq!(errs.len(), 1, "one error");
    assert_eq!(
        gpp,
        GppContainer {
            version: 1,
            section_types: Vec::new(),
            sections: Vec::new(),
        },
        "partial container"
    );
}

#[test]
fn failed_section_is_zero_value_and_parsing_continues() {
    // usca (8) fails, then usva (9) succeeds.
    let (gpp, errs) = parse("DBABh4A~xlgWE~bSFgmiU");
    assert_eq!(
        messages(&errs),
        ["error parsing uspca consent string: unable to set field \
          CoreSegment.SensitiveDataProcessing due to parse error: expected 2 bits to start at bit \
          32, but the byte array was only 4 bytes long"],
        "errors"
    );
    assert_eq!(gpp.section_types, ids(&[8, 9]), "types");
    assert_eq!(
        gpp.sections.first(),
        Some(&Section::Uspca(Uspca::default())),
        "zero value"
    );
    assert_eq!(
        gpp.sections.first().map(Section::id),
        Some(SectionId(0)),
        "zero id"
    );
    assert_eq!(
        gpp.sections.get(1).map(Section::value),
        Some("bSFgmiU"),
        "decoded"
    );
}

/// Encodes `(id, value)` generic sections with [`crate::encode`] and parses the result; an encode
/// error is returned as the only parse error so that assertions fail visibly.
fn parse_pairs(pairs: &[(i64, &str)]) -> (GppContainer, Vec<GppError>) {
    let mut sections: Vec<Section> = pairs
        .iter()
        .map(|&(id, value)| generic(id, value))
        .collect();
    match crate::encode(&mut sections) {
        Ok(encoded) => parse(&encoded),
        Err(err) => (GppContainer::default(), vec![err]),
    }
}

#[test]
fn stale_error_is_repeated_for_generic_sections() {
    // Go's default branch re-reports the uspnat (7) error for every later generic section.
    let (gpp, errs) = parse_pairs(&[(7, "!"), (13, "thirteen"), (14, "fourteen")]);
    let base = "illegal base64 data at input byte 0";
    assert_eq!(
        messages(&errs),
        [
            format!("error parsing uspnat consent string: {base}"),
            format!("error parsing unsupported (section 13) consent string: {base}"),
            format!("error parsing unsupported (section 14) consent string: {base}"),
        ],
        "stale errors"
    );
    assert_eq!(
        gpp.sections.get(1),
        Some(&generic(13, "thirteen")),
        "generic kept"
    );

    // A successful US section in between resets the stale error.
    let (_, reset_errs) = parse_pairs(&[(7, "!"), (9, "bSFgmiU"), (13, "thirteen")]);
    assert_eq!(
        messages(&reset_errs),
        [format!("error parsing uspnat consent string: {base}")],
        "reset by success"
    );
}

/// A header with ten single IDs 6500 apart, then the range [65001, 65535].
fn wrapping_range_header() -> Result<String, GppError> {
    let mut bs = BitStream::new_for_write();
    bs.write_byte6(3);
    bs.write_byte6(1);
    bs.write_uint12(11);
    for _ in 0..10 {
        bs.write_byte1(0);
        bs.write_fibonacci_int(6500)?;
    }
    bs.write_byte1(1);
    bs.write_fibonacci_int(1)?;
    bs.write_fibonacci_int(534)?;
    Ok(String::from_utf8_lossy(&bs.base64_encode()).into_owned())
}

#[test]
fn infinite_go_loops_are_header_errors() {
    // Go's uint16 loop over a range ending at 65535 never ends.
    let parsed = wrapping_range_header().map(|header| parse(&header));
    assert_eq!(
        parsed.as_ref().map(|(_, errs)| messages(errs)),
        Ok(vec![
            "error parsing GPP header, section identifiers: range 65001-65535 never ends"
        ]),
        "Rust-only error"
    );
    assert_eq!(parsed.map(|(gpp, _)| gpp.version), Ok(1), "version kept");
}

/// Header "DBADAxVFBagSgsA": three single entries with Fibonacci offsets 13, 65530 and 65536.
/// Go adds them in uint16, so the IDs are 13, then 7 (13 + 65530 wraps, the maximum stays 13),
/// then 13 again (65536 wraps to 0). Expected values recorded from go-gpp v0.2.0.
const WRAPPING_IDS_HEADER: &str = "DBADAxVFBagSgsA";

#[test]
fn wrapping_offsets_give_backwards_and_repeated_ids() {
    let (gpp, errs) = parse(&format!(
        "{WRAPPING_IDS_HEADER}~thirteen~BVQqAAAAAgA.QA~again"
    ));
    assert_eq!(messages(&errs), Vec::<&str>::new(), "no errors");
    assert_eq!(gpp.section_types, ids(&[13, 7, 13]), "types");
    let got: Vec<(SectionId, &str)> = gpp.sections.iter().map(|s| (s.id(), s.value())).collect();
    assert_eq!(
        got,
        [
            (SectionId(13), "thirteen"),
            (SectionId(7), "BVQqAAAAAgA.QA"),
            (SectionId(13), "again"),
        ],
        "sections"
    );
    assert!(
        matches!(gpp.sections.get(1), Some(Section::Uspnat(_))),
        "uspnat decoded"
    );
    let mut sections = gpp.sections;
    assert_eq!(
        crate::encode(&mut sections).map_err(|e| e.to_string()),
        Err("duplicated sections".to_owned()),
        "encode"
    );
}

#[test]
fn stale_error_after_backwards_id() {
    let (gpp, errs) = parse(&format!("{WRAPPING_IDS_HEADER}~thirteen~!~again"));
    let base = "illegal base64 data at input byte 0";
    assert_eq!(
        messages(&errs),
        [
            format!("error parsing uspnat consent string: {base}"),
            format!("error parsing unsupported (section 13) consent string: {base}"),
        ],
        "errors"
    );
    let got: Vec<(SectionId, &str)> = gpp.sections.iter().map(|s| (s.id(), s.value())).collect();
    assert_eq!(
        got,
        [
            (SectionId(13), "thirteen"),
            (SectionId(0), ""),
            (SectionId(13), "again"),
        ],
        "sections"
    );
    let mut sections = gpp.sections;
    assert_eq!(
        crate::encode(&mut sections).map_err(|e| e.to_string()),
        Err("section ID out of range".to_owned()),
        "encode sees the zero-value section"
    );
}

#[test]
fn offset_wrapping_to_zero_is_section_id_zero() {
    // "DBABUCUFgA": one entry whose Fibonacci offset is 65536, which go-gpp reads as 0.
    let (gpp, errs) = parse("DBABUCUFgA~zero");
    assert!(errs.is_empty(), "no errors: {errs:?}");
    assert_eq!(gpp.section_types, ids(&[0]), "types");
    assert_eq!(gpp.sections, [generic(0, "zero")], "generic section 0");
}

#[test]
fn zero_sections() {
    // "DBA" pads to "DBAA": a 3-byte header with an empty range and no sections.
    assert_eq!(
        parse("DBA"),
        (
            GppContainer {
                version: 1,
                section_types: Vec::new(),
                sections: Vec::new(),
            },
            Vec::new()
        ),
        "empty"
    );
}
