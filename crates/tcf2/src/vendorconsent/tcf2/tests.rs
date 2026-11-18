//! Go: `vendorconsent/tcf2/{bitfield,rangesection,pubrestrict,metadata}_test.go`.

use std::collections::BTreeSet;

use super::{ConsentMetadata, parse};
use crate::{
    ConsentError, api::VendorConsents as _, base64::RAW_URL_ENCODING, consentconstants::Purpose,
    test_support::TestResult,
};

/// Go: `decode` in `test_utils.go`.
fn decode(encoded: &str) -> Result<Vec<u8>, ConsentError> {
    Ok(RAW_URL_ENCODING.decode(encoded.as_bytes())?)
}

fn parse_encoded(encoded: &str) -> Result<ConsentMetadata, ConsentError> {
    parse(decode(encoded)?)
}

/// Go: `assertInvalidBytes` in `test_utils.go`.
fn assert_invalid_bytes(data: &[u8], expect_error: &str) -> TestResult {
    match parse(data.to_vec()) {
        Ok(consent) => Err(format!(
            "data {data:?} was considered valid, but shouldn't be. MaxVendorID: {}",
            consent.max_vendor_id()
        )
        .into()),
        Err(err) => {
            check_eq!(
                err.to_string(),
                expect_error,
                "error messages did not match"
            );
            Ok(())
        }
    }
}

/// Go: `TestBitField`.
#[test]
fn bit_field() -> TestResult {
    // String built using http://gdpr-demo.labs.quantcast.com/user-examples/cookie-workshop.html
    // This sample includes a BitField.
    let consent = parse_encoded("COwGVJOOwGVJOADACHENAOCAAO6as_-AAAhoAFNLAAoAAAA")?;
    check_eq!(consent.version(), 2, "version");
    check_eq!(consent.cmp_id(), 3, "cmp id");
    check_eq!(consent.cmp_version(), 2, "cmp version");
    check_eq!(consent.consent_screen(), 7, "consent screen");
    check_eq!(&consent.consent_language(), b"EN", "consent language");
    check_eq!(consent.vendor_list_version(), 14, "vendor list version");
    check_eq!(consent.max_vendor_id(), 10, "max vendor id");

    let purposes_allowed = BTreeSet::from([1, 2, 3, 5, 6, 7, 9, 12, 13, 15, 17, 19, 20, 23, 24]);
    for i in 1..=24 {
        check_eq!(
            consent.purpose_allowed(Purpose(i)),
            purposes_allowed.contains(&i),
            "purpose {i}"
        );
    }

    let vendors_with_consent = BTreeSet::from([1, 2, 4, 7, 9, 10]);
    for i in 1..=consent.max_vendor_id() {
        check_eq!(
            consent.vendor_consent(i),
            vendors_with_consent.contains(&i),
            "vendor {i}"
        );
    }
    Ok(())
}

/// Go: `TestRangeSectionConsent`.
#[test]
fn range_section_consent() -> TestResult {
    // String built using http://iabtcf.com/#/encode
    // This sample encodes a mix of Single- and Range-typed consent exceptions.
    let consent = parse_encoded(
        "COyfVVoOyfVVoADACHENAwCAAAAAAAAAAAAAE5QBgALgAqgD8AQACSwEygJyAnSAMABgAFkAgQCDASeAmYBOgAA",
    )?;
    check_eq!(consent.version(), 2, "version");
    check_eq!(consent.cmp_id(), 3, "cmp id");
    check_eq!(consent.cmp_version(), 2, "cmp version");
    check_eq!(consent.consent_screen(), 7, "consent screen");
    check_eq!(&consent.consent_language(), b"EN", "consent language");
    check_eq!(consent.vendor_list_version(), 48, "vendor list version");
    check_eq!(consent.max_vendor_id(), 626, "max vendor id");

    let vendors_with_consent = BTreeSet::from([23, 42, 126, 127, 128, 587, 613, 626]);
    for i in 1..=consent.max_vendor_id() {
        check_eq!(
            consent.vendor_consent(i),
            vendors_with_consent.contains(&i),
            "vendor {i}"
        );
    }

    let vendors_legitimate_interest = BTreeSet::from([24, 44, 129, 130, 131, 591, 614, 628]);
    for i in 1..=consent.vendor_legit_interest_max_id() {
        check_eq!(
            consent.vendor_legit_interest(i),
            vendors_legitimate_interest.contains(&i),
            "legitimate interest {i}"
        );
    }
    Ok(())
}

/// Go: `TestInvalidRangeEdgeCase` (prevents go-gdpr issue #10).
#[test]
fn invalid_range_edge_case() -> TestResult {
    let data = decode("COwDzqZOwDzqZN4ABMENAPCAAP4AAP-AAAhoAFQAYABgAOABQAAAAA")?;
    let truncated = data.get(..31).ok_or("test vector shorter than 31 bytes")?;
    assert_invalid_bytes(
        truncated,
        "ParseUInt16 expected a 16-bit int to start at bit 243, but the consent string was only \
         31 bytes long",
    )
}

/// Go: `TestPubRestrictions`.
#[test]
fn pub_restrictions() -> TestResult {
    let consent = parse_encoded("COwAdDhOwAdDhN4ABAENAPCgAAQAAv___wAAAFP_AAp_4AI6ACACAA")?;
    // Extra random verification to ensure the basics are solid
    check_eq!(consent.version(), 2, "version");
    check_eq!(consent.cmp_id(), 888, "cmp id");
    check_eq!(consent.cmp_version(), 1, "cmp version");
    check_eq!(consent.consent_screen(), 0, "consent screen");
    check_eq!(&consent.consent_language(), b"EN", "consent language");
    check_eq!(consent.vendor_list_version(), 15, "vendor list version");
    check_eq!(consent.max_vendor_id(), 10, "max vendor id");

    // A pub restriction was set on Purpose 7, type 1, for vendor 32
    check!(
        consent.check_pub_restriction(7, 1, 32),
        "purpose 7 type 1 vendor 32"
    );
    // Verify a that a different vendor is not flagged
    check!(
        !consent.check_pub_restriction(7, 1, 7),
        "purpose 7 type 1 vendor 7"
    );
    // Verify that a different purpose is not flagged for the same vendor
    check!(
        !consent.check_pub_restriction(5, 1, 32),
        "purpose 5 type 1 vendor 32"
    );
    Ok(())
}

/// Go: `TestPubRestrictions2`.
#[test]
fn pub_restrictions2() -> TestResult {
    let consent = parse_encoded(concat!(
        "COxPe2TOxPe2TALABAENAPCgAAAAAAAAAAAAAFAAAAoAAA4IACACAIABgACAFA4ADACAAIygAGADwAQBIAIAIB",
        "0AEAEBSACACAA",
    ))?;

    check!(!consent.purpose_one_treatment(), "purpose one treatment");
    check!(!consent.special_feature_opt_in(3), "special feature 3");

    // Pub restriction 0 set on purpose 1
    check!(
        consent.check_pub_restriction(1, 0, 32),
        "purpose 1 type 0 vendor 32"
    );
    // Verify that restriction type 1 isn't erroneously flagged
    check!(
        !consent.check_pub_restriction(1, 1, 32),
        "purpose 1 type 1 vendor 32"
    );
    // Pub restriction 0 for purpose 2 is set on several vendors, but not all
    for (vendor, expected) in [(32, true), (5, true), (11, true), (44, false), (500, false)] {
        check_eq!(
            consent.check_pub_restriction(2, 0, vendor),
            expected,
            "vendor {vendor}"
        );
    }
    // Pub restriction 1 on purpose 2 also set for vendor 32. This does not make sense, but not
    // explicitly disallowed by the spec. Verifying that the code handles it
    check!(
        consent.check_pub_restriction(2, 1, 32),
        "purpose 2 type 1 vendor 32"
    );
    // Verifying that this special case doesn't bleed over to other vendors.
    check!(
        !consent.check_pub_restriction(2, 1, 42),
        "purpose 2 type 1 vendor 42"
    );
    Ok(())
}

/// Go: `TestPubRestrictions3`.
#[test]
fn pub_restrictions3() -> TestResult {
    let _consent = parse_encoded("COzSDo9OzSDo9B9AAAENAiCAALAAAAAAAAAACOQAQCOAAAAA")?;
    Ok(())
}

/// 2020-02-27T19:51:49Z, the creation and update time of the `TestCreatedDate` vector.
const FEB_27_2020_19_51_49: u64 = 1_582_833_109;

/// Go: `TestCreatedDate`.
#[test]
fn created_date() -> TestResult {
    let consent = parse_encoded(
        "COvcSpYOvcSpYC9AAAENAPCAAAAAAAAAAAAACvwDQABAAIAAYABIAC4AJQAagA9ACEAPgAjIBJoCvAK-AAAAAA",
    )?;
    check_eq!(consent.created().unix(), FEB_27_2020_19_51_49, "created");
    Ok(())
}

/// Go: `TestLastUpdate`.
#[test]
fn last_update() -> TestResult {
    let consent = parse_encoded(
        "COvcSpYOvcSpYC9AAAENAPCAAAAAAAAAAAAACvwDQABAAIAAYABIAC4AJQAagA9ACEAPgAjIBJoCvAK-AAAAAA",
    )?;
    check_eq!(
        consent.last_updated().unix(),
        FEB_27_2020_19_51_49,
        "last updated"
    );
    Ok(())
}

/// Go: `TestLargeCmpID`.
#[test]
fn large_cmp_id() -> TestResult {
    let consent = parse_encoded(concat!(
        "COyiBqdOyiBqdObAAAENAfCIAP8AAH-AAAAAB4AXQQgEAAAgoAAAAABAIYQUAAAAAAAAAAAAAAAIQIQCxIvkgQ",
        "MAAAABgAIAAAAAAAAAAABAZAkAAA",
    ))?;
    check_eq!(consent.cmp_id(), 923, "cmp id");
    Ok(())
}

/// Go: `TestLargeCmpVersion`.
#[test]
fn large_cmp_version() -> TestResult {
    let consent = parse_encoded("COyiCPlOyiCPlKxMIAENAfCAAAAAAAAAAAAAAAAAAAAA")?;
    check_eq!(consent.cmp_version(), 776, "cmp version");
    Ok(())
}

/// Go: `TestLargeConsentScreen`.
#[test]
fn large_consent_screen() -> TestResult {
    let consent = parse_encoded("COyiFYuOyiFYuDKAA4ENAfCAAAAAAAAAAAAAAAAAAAAA")?;
    check_eq!(consent.consent_screen(), 56, "consent screen");
    Ok(())
}

/// Go: `TestLanguageExtremes`.
#[test]
fn language_extremes() -> TestResult {
    for (encoded, language) in [
        ("COyiHgFOyiHgFN4ABABGAPCAAAAAAAAAAAAAAFAAAAoAAAA", b"BG"),
        ("COyiHgFOyiHgFN4ABASVAPCAAAAAAAAAAAAAAFAAAAoAAAA", b"SV"),
    ] {
        check_eq!(
            &parse_encoded(encoded)?.consent_language(),
            language,
            "{encoded}"
        );
    }
    Ok(())
}

/// Go: `TestTCFPolicyVersion`.
#[test]
fn tcf_policy_version() -> TestResult {
    let base_consent = "CPtGDMAPtGDMALMAAAENA_C_AAAAAAAAACiQAAAAAAAA";
    // policy version is at the 23rd 6-bit base64 position
    let (prefix, rest) = base_consent.split_at(22);
    let suffix = rest.get(1..).ok_or("base consent too short")?;
    for (base64_char, expected) in [
        ("A", 0),
        ("B", 1),
        ("C", 2),
        ("E", 4),
        ("I", 8),
        ("Q", 16),
        ("g", 32),
        ("_", 63),
    ] {
        let consent = parse_encoded(&format!("{prefix}{base64_char}{suffix}"))?;
        check_eq!(consent.tcf_policy_version(), expected, "char {base64_char}");
    }
    Ok(())
}

/// Go: `TestTCF2Fields`.
#[test]
fn tcf2_fields() -> TestResult {
    let consent = parse_encoded("COx3XOeOx3XOeLkAAAENAfCIAAAAAHgAAIAAAAAAAAAA")?;
    check!(consent.purpose_one_treatment(), "purpose one treatment");
    check!(consent.special_feature_opt_in(1), "special feature 1");
    check!(!consent.special_feature_opt_in(2), "special feature 2");
    Ok(())
}

/// Go: `TestLITransparency`.
#[test]
fn li_transparency() -> TestResult {
    let consent = parse_encoded("COx3XOeOx3XOeLkAAAENAfCIAAAAAHgAAIAAAAAAAAAA")?;
    for (purpose, expected) in [
        (1, false),
        (2, true),
        (3, true),
        (4, true),
        (5, true),
        (6, false),
        (7, false),
        (28, false),
    ] {
        check_eq!(
            consent.purpose_li_transparency(Purpose(purpose)),
            expected,
            "{purpose}"
        );
    }
    Ok(())
}

/// The one input shape where Go panics: the legitimate-interest section starts on a byte
/// boundary right at the end of the data (Go: `consent.go:68`).
#[test]
fn aligned_legitimate_interest_panic() -> TestResult {
    // A TCF 2 header with MaxVendorID 2 (bits 213-228) and a bitfield (bit 229 clear): the LI
    // section starts at bit 232, byte 29, and the data ends right after its 16-bit max vendor ID.
    let mut data = decode("CPtGDMAPtGDMALMAAAENA_C_AAAAAAAAACiQAAAAAAAA")?;
    data.truncate(26);
    data.extend_from_slice(&[0, 0, 0x10, 0, 0]);
    let err = parse(data).err().ok_or("expected an error")?;
    check_eq!(
        err,
        ConsentError::IndexOutOfRange {
            index: 31,
            length: 31
        },
        "error"
    );
    check!(err.is_go_panic(), "is a Go panic");
    check_eq!(
        err.to_string(),
        "runtime error: index out of range [31] with length 31",
        "text"
    );
    Ok(())
}
