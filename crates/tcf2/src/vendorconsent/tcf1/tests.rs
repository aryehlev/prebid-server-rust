//! Go: `vendorconsent/tcf1/{bitfield,metadata,rangesection}_test.go`.

use std::collections::BTreeSet;

use super::{Consent, parse, rangesection::parse_uint16};
use crate::{
    ConsentError, api::VendorConsents as _, base64::RAW_URL_ENCODING, consentconstants::Purpose,
    test_support::TestResult,
};

/// Go: `decode` in `test_utils.go`.
fn decode(encoded: &str) -> Result<Vec<u8>, ConsentError> {
    Ok(RAW_URL_ENCODING.decode(encoded.as_bytes())?)
}

fn parse_encoded(encoded: &str) -> Result<Consent, ConsentError> {
    parse(decode(encoded)?)
}

/// Go: `TestBitField`.
#[test]
fn bit_field() -> TestResult {
    // String built using http://acdn.adnxs.com/cmp/docs/#/tools/vendor-cookie-encoder
    // This sample includes a BitField.
    let consent = parse_encoded("BONV8oqONXwgmADACHENAO7pqzAAppY")?;
    check_eq!(consent.version(), 1, "version");
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

/// Go: `TestCreatedDate`: 1998-02-15T07:24:54Z.
#[test]
fn created_date() -> TestResult {
    let consent = parse_encoded("BIRAfK8OOHsDFABABBAAABAAAAAAEA")?;
    check_eq!(consent.created().unix(), 887_527_494, "created");
    Ok(())
}

/// Go: `TestLastUpdated`: 2018-05-21T18:43:18Z.
#[test]
fn last_updated() -> TestResult {
    let consent = parse_encoded("BIRAfK8OOHsDFABABBAAABAAAAAAEA")?;
    check_eq!(consent.last_updated().unix(), 1_526_928_198, "last updated");
    Ok(())
}

/// Go: `TestLargeVendorListVersion`.
#[test]
fn large_vendor_list_version() -> TestResult {
    let consent = parse_encoded("BON96hFON96hFABABBAA4yAAAAAAEA")?;
    check_eq!(consent.vendor_list_version(), 3634, "vendor list version");
    Ok(())
}

/// Go: `TestLargeCmpID`.
#[test]
fn large_cmp_id() -> TestResult {
    let consent = parse_encoded("BOOG3gbOOG3gbFZABBAAABAAAAAAEA")?;
    check_eq!(consent.cmp_id(), 345, "cmp id");
    Ok(())
}

/// Go: `TestLargeCmpVersion`.
#[test]
fn large_cmp_version() -> TestResult {
    let consent = parse_encoded("BOOG4uyOOG4uyABFZBAAABAAAAAAEA")?;
    check_eq!(consent.cmp_version(), 345, "cmp version");
    Ok(())
}

/// Go: `TestLargeConsentScreen`.
#[test]
fn large_consent_screen() -> TestResult {
    let consent = parse_encoded("BOOG4uyOOG4uyABFZTAAABAAAAAAEA")?;
    check_eq!(consent.consent_screen(), 19, "consent screen");
    Ok(())
}

/// Go: `TestLanguageExtremes`.
#[test]
fn language_extremes() -> TestResult {
    for (encoded, language) in [
        ("BOOG9-6OOG9-6ABABBAZABAAAAAAEA", b"AZ"),
        ("BOOG9-6OOG9-6ABABBZAABAAAAAAEA", b"ZA"),
    ] {
        check_eq!(
            &parse_encoded(encoded)?.consent_language(),
            language,
            "{encoded}"
        );
    }
    Ok(())
}

/// Go: `TestRangeSectionConsent`.
#[test]
fn range_section_consent() -> TestResult {
    // String built using http://acdn.adnxs.com/cmp/docs/#/tools/vendor-cookie-encoder
    // This sample encodes a mix of Single- and Range-typed consent exceptions.
    let consent = parse_encoded("BONciguONcjGKADACHENAOLS1rAHDAFAAEAASABQAMwAeACEAFw")?;
    check_eq!(consent.version(), 1, "version");
    check_eq!(consent.cmp_id(), 3, "cmp id");
    check_eq!(consent.cmp_version(), 2, "cmp version");
    check_eq!(consent.consent_screen(), 7, "consent screen");
    check_eq!(&consent.consent_language(), b"EN", "consent language");
    check_eq!(consent.vendor_list_version(), 14, "vendor list version");
    check_eq!(consent.max_vendor_id(), 112, "max vendor id");

    let purposes_with_consent = BTreeSet::from([3, 5, 6, 8, 11, 13, 14, 16, 18, 19, 21, 23, 24]);
    for i in 1..=24 {
        check_eq!(
            consent.purpose_allowed(Purpose(i)),
            purposes_with_consent.contains(&i),
            "purpose {i}"
        );
    }

    let vendors_lacking_consent = BTreeSet::from([
        2, 4, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 46,
    ]);
    for i in 1..=consent.max_vendor_id() {
        check_eq!(
            consent.vendor_consent(i),
            !vendors_lacking_consent.contains(&i),
            "vendor {i}"
        );
    }
    Ok(())
}

/// Go: `TestInvalidRangeEdgeCase` (prevents go-gdpr issue #10).
#[test]
fn invalid_range_edge_case() -> TestResult {
    let data = decode("BOQA9AtOQA9AtABABBAAABAAAAAGSAHAACAAMAAoABwAEgALAAaA")?;
    let truncated = data.get(..36).ok_or("test vector shorter than 36 bytes")?;
    let err = parse(truncated.to_vec())
        .err()
        .ok_or("data was considered valid")?;
    check_eq!(
        err.to_string(),
        "bit 288 was supposed to start a new RangeEntry, but the consent string was only 36 bytes \
         long",
        "error messages did not match"
    );
    Ok(())
}

/// Go: `TestParseUInt16`.
#[test]
fn test_parse_uint16() -> TestResult {
    // Start with 01100000 00000000 00100000, expect 00000000 00000001
    check_eq!(
        parse_uint16(&[0x60, 0x00, 0x20], 3)?,
        0x1,
        "unaligned low bit"
    );
    // Start with 00100000 00001110 00000000, expect 00000000 01110000
    check_eq!(
        parse_uint16(&[0x20, 0x0E, 0x00], 3)?,
        0x70,
        "unaligned middle bits"
    );
    // Start with 11110100 00010011, expect 11110100 00010011
    check_eq!(parse_uint16(&[0xF4, 0x13], 0)?, 0xF413, "aligned");
    Ok(())
}

/// Go panics in `PurposeAllowed` when `id + 131` lies past the end of the data.
#[test]
fn purpose_allowed_past_the_end() -> TestResult {
    let consent = parse_encoded("BONV8oqONXwgmADACHENAO7pqzAAppY")?;
    // 23 bytes hold bits 0-183, so purpose 52 (bit 183) is the last readable one.
    check!(
        consent.checked_purpose_allowed(Purpose(52)).is_ok(),
        "purpose 52"
    );
    check_eq!(
        consent.checked_purpose_allowed(Purpose(53)),
        Err(ConsentError::IndexOutOfRange {
            index: 23,
            length: 23
        }),
        "purpose 53"
    );
    check!(
        !consent.purpose_allowed(Purpose(53)),
        "unchecked purpose 53"
    );
    Ok(())
}
