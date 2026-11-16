//! Go: `vendorconsent/consent_test.go` and `vendorconsent/consent20_test.go`.

use super::{ParsedConsent, common::is_set, parse_string, tcf1, tcf2};
use crate::{
    ConsentError,
    api::VendorConsents as _,
    base64::{CorruptInputError, RAW_URL_ENCODING},
    test_support::TestResult,
};

/// Go: `TestIsSet`.
#[test]
fn test_is_set() -> TestResult {
    let data = [0xF6, 0x37];
    let expected = [
        true, true, true, true, false, true, true, false, false, false, true, true, false, true,
        true, true,
    ];
    for (bit, want) in expected.into_iter().enumerate() {
        check_eq!(is_set(&data, bit)?, want, "bit {bit}");
    }
    check_eq!(
        is_set(&data, 16),
        Err(ConsentError::IndexOutOfRange {
            index: 2,
            length: 2
        }),
        "past the end"
    );
    Ok(())
}

/// Go: `assertInvalid` / `assertInvalid20`: the base64url string decodes, and `parse` rejects it
/// with exactly `expect_error`.
fn assert_invalid<T>(
    encoded: &str,
    parse: fn(Vec<u8>) -> Result<T, ConsentError>,
    expect_error: &str,
) -> TestResult {
    let data = RAW_URL_ENCODING.decode(encoded.as_bytes())?;
    let err = parse(data)
        .err()
        .ok_or_else(|| format!("{encoded} was considered valid"))?;
    check_eq!(
        err.to_string(),
        expect_error,
        "error messages did not match for {encoded}"
    );
    Ok(())
}

/// Go: `TestInvalidConsentStrings` (TCF 1.1).
#[test]
fn invalid_consent_strings() -> TestResult {
    let cases = [
        // Bad metadata
        (
            "BONciguONcjGKADACHENAOLS1rAA",
            "vendor consent strings are at least 22 bytes long. This one was 21",
        ),
        (
            "AONciguONcjGKADACHENAOLS1rAAMA",
            "the consent string encoded a Version of 0, but this value must be greater than or \
             equal to 1",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAAA",
            "the consent string encoded a MaxVendorID of 0, but this value must be greater than \
             or equal to 1",
        ),
        (
            "BONciguONcjGKADACHENAALS1rAAMA",
            "the consent string encoded a VendorListVersion of 0, but this value must be greater \
             than or equal to 1",
        ),
        // Bad BitFields
        (
            "BONciguONcjGKADACHENAOLS1rAAQA",
            "a BitField for 4 vendors requires a consent string of 23 bytes. This consent string \
             had 22",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAwAA",
            "a BitField for 12 vendors requires a consent string of 24 bytes. This consent string \
             had 23",
        ),
        // Bad RangeSections
        (
            "BONciguONcjGKADACHENAOLS1rAAOAA",
            "vendor consent strings using RangeSections require at least 24 bytes. Got 23",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABAAA",
            "rangeSection expected a 16-bit vendorID to start at bit 187, but the consent string \
             was only 25 bytes long",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABgACA",
            "rangeSection expected a 16-bit vendorID to start at bit 203, but the consent string \
             was only 26 bytes long",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPACAACA",
            "rangeSection expected a 16-bit vendorID to start at bit 204, but the consent string \
             was only 26 bytes long",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABAAAA",
            "bit 186 range entry excludes vendor 0, but only vendors [1, 3] are valid",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABAAIA",
            "bit 186 range entry excludes vendor 4, but only vendors [1, 3] are valid",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABgAAAAIA",
            "bit 186 range entry exclusion starts at 0, but the min vendor ID is 1",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABgACAAgA",
            "bit 186 range entry exclusion ends at 4, but the max vendor ID is 3",
        ),
        (
            "BONciguONcjGKADACHENAOLS1rAAPABgAEAAIA",
            "bit 186 range entry excludes vendors [2, 1]. The start should be less than the end",
        ),
    ];
    for (encoded, expect_error) in cases {
        assert_invalid(encoded, tcf1::parse, expect_error)?;
    }
    Ok(())
}

/// Go: `TestInvalidConsentStrings20` (TCF 2).
#[test]
fn invalid_consent_strings20() -> TestResult {
    let cases = [
        // Bad metadata
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQAA",
            "vendor consent strings are at least 29 bytes long. This one was 28",
        ),
        (
            "BONciguONcjGKADACHENAOCIAC0ta__AACiQABgAAYA",
            "the consent string encoded a Version of 1, but this value must be greater than or \
             equal to 2",
        ),
        (
            "CONciguONcjGKADACHENAACIAC0ta__AACiQABgAAYA",
            "the consent string encoded a VendorListVersion of 0, but this value must be greater \
             than or equal to 1",
        ),
        // Bad BitFields
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQAeAA",
            "a BitField for 60 vendors requires a consent string of 36 bytes. This consent string \
             had 30",
        ),
        // Bad RangeSections
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwA",
            "vendor consent strings using RangeSections require at least 31 bytes. Got 30",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAQQ",
            "ParseUInt16 expected a 16-bit int to start at bit 243, but the consent string was \
             only 31 bytes long",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAYQAC",
            "ParseUInt16 expected a 16-bit int to start at bit 259, but the consent string was \
             only 33 bytes long",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAgABA",
            "ParseUInt16 expected a 16-bit int to start at bit 260, but the consent string was \
             only 33 bytes long",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAgAAAAA",
            "bit 242 range entry excludes vendor 0, but only vendors [1, 3] are valid",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAgACAAA",
            "bit 242 range entry excludes vendor 4, but only vendors [1, 3] are valid",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAgABAAAA",
            "bit 259 range entry excludes vendor 0, but only vendors [1, 3] are valid",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAgABAAEA",
            "bit 259 range entry excludes vendor 4, but only vendors [1, 3] are valid",
        ),
        (
            "CONciguONcjGKADACHENAOCIAC0ta__AACiQABwAoABAACA",
            "bit 242 range entry excludes vendors [2, 1]. The start should be less than the end",
        ),
    ];
    for (encoded, expect_error) in cases {
        assert_invalid(encoded, tcf2::parse, expect_error)?;
    }
    Ok(())
}

/// Go: `TestParseInvalidString`.
#[test]
fn parse_invalid_string() -> TestResult {
    check_eq!(
        parse_string("/"),
        Err(ConsentError::CorruptBase64(CorruptInputError(0))),
        "strings with invalid base64 characters should be invalid"
    );
    Ok(())
}

/// Go: `TestParseValidString`.
#[test]
fn parse_valid_string() -> TestResult {
    let parsed = parse_string("BONV8oqONXwgmADACHENAO7pqzAAppY")?;
    check!(matches!(parsed, ParsedConsent::Tcf1(_)), "parsed as TCF 1");
    check_eq!(parsed.vendor_list_version(), 14, "vendor list version");
    Ok(())
}

/// Go: `TestParseValidString20`.
#[test]
fn parse_valid_string20() -> TestResult {
    let parsed =
        parse_string("COyiILmOyiILmADACHENAPCAAAAAAAAAAAAAE5QBgALgAqgD8AQACSwEygJyAAAAAA")?;
    check!(matches!(parsed, ParsedConsent::Tcf2(_)), "parsed as TCF 2");
    check_eq!(parsed.vendor_list_version(), 15, "vendor list version");
    Ok(())
}

/// Go: `TestParseValidString20MaxVendorID0`.
#[test]
fn parse_valid_string20_max_vendor_id0() -> TestResult {
    let parsed = parse_string("COwJz-rOwJz-rMLAEAFRAPCgAAAAAAAAAAqIAAAAAAAA")?;
    // if vendor consent is empty, max vendor ID may be 0. See
    // https://github.com/InteractiveAdvertisingBureau/iabtcf-es/issues/121
    check_eq!(parsed.max_vendor_id(), 0, "max vendor id");
    check_eq!(parsed.vendor_list_version(), 15, "vendor list version");
    Ok(())
}

/// Go: `TestParseEmptyString` (`ParseVersion` is not ported).
#[test]
fn parse_empty_string() -> TestResult {
    check_eq!(
        parse_string(""),
        Err(ConsentError::EmptyDecodedConsent),
        "vendorconsent"
    );
    check_eq!(
        tcf2::parse_string(""),
        Err(ConsentError::EmptyDecodedConsent),
        "tcf2"
    );
    check_eq!(
        tcf1::parse_string(""),
        Err(ConsentError::EmptyDecodedConsent),
        "tcf1"
    );
    check_eq!(
        ConsentError::EmptyDecodedConsent.to_string(),
        "decoded consent cannot be empty",
        "message"
    );
    Ok(())
}

/// `tcf2.ParseString` only decodes the core segment.
#[test]
fn tcf2_segments_are_ignored() -> TestResult {
    let core = "COyiILmOyiILmADACHENAPCAAAAAAAAAAAAAE5QBgALgAqgD8AQACSwEygJyAAAAAA";
    // A disclosed-vendors and a publisher segment.
    let segment = "IFoEUQQgAIQwgIwQABAEAAAAOIAACAIAAAAQAIAgEAACEAAAAAgAQBAAAAAAAGBAAgAAAAAAAFAAEC";
    let with_segments = format!("{core}.{segment}.Q{segment}");
    check_eq!(
        tcf2::parse_string(&with_segments)?,
        tcf2::parse_string(core)?,
        "segments"
    );
    check!(tcf2::is_consent_v2(core), "is TCF 2");
    check!(
        !tcf2::is_consent_v2("BONV8oqONXwgmADACHENAO7pqzAAppY"),
        "is TCF 1"
    );
    check!(!tcf2::is_consent_v2(""), "empty");
    Ok(())
}
