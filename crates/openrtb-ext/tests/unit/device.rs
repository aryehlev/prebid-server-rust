//! Go: `openrtb_ext/device_test.go`.

use pbs_openrtb_ext::openrtb_ext::{
    ExtDevice, IOS_APP_TRACKING_STATUS_AUTHORIZED, is_known_ios_app_tracking_status,
    parse_device_ext_atts,
};

use super::support::{
    TestResult, all_ok, ensure_eq, serde_unmarshal, unmarshal_error, without_position,
};

const BAD_WIDTH: &str =
    "request.device.ext.prebid.interstitial.minwidthperc must be a number between 0 and 100";
const BAD_HEIGHT: &str =
    "request.device.ext.prebid.interstitial.minheightperc must be a number between 0 and 100";

/// The documents of `TestInvalidDeviceExt`, with Go's error text for each.
const INVALID_DEVICE_EXTS: [(&str, &str); 9] = [
    (
        r#"{"prebid":{"interstitial":{"minheightperc":0}}}"#,
        BAD_WIDTH,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":105}}}"#,
        BAD_WIDTH,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":true,"minheightperc":0}}}"#,
        BAD_WIDTH,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":null,"minheightperc":0}}}"#,
        BAD_WIDTH,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":"75","minheightperc":0}}}"#,
        BAD_WIDTH,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":85}}}"#,
        BAD_HEIGHT,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":85,"minheightperc":-5}}}"#,
        BAD_HEIGHT,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":85,"minheightperc":false}}}"#,
        BAD_HEIGHT,
    ),
    (
        r#"{"prebid":{"interstitial":{"minwidthperc":85,"minheightperc":"75"}}}"#,
        BAD_HEIGHT,
    ),
];

/// Go: `TestInvalidDeviceExt`.
#[test]
fn invalid_device_ext() -> TestResult {
    let mut s = ExtDevice::default();
    for (doc, want) in INVALID_DEVICE_EXTS {
        ensure_eq(&unmarshal_error(doc, &mut s), &Some(want.to_owned()), doc)?;
    }
    Ok(())
}

/// Go: `TestInvalidDeviceExt`, through the serde code: Go's text, which `serde_json` gives with
/// its position after it, the interstitial being inside the document
/// (`json.prebid-message-context`).
#[test]
fn invalid_device_ext_serde() -> TestResult {
    for (doc, want) in INVALID_DEVICE_EXTS {
        let err = serde_unmarshal::<ExtDevice>(doc)
            .err()
            .ok_or_else(|| format!("{doc}: no error"))?;
        ensure_eq(without_position(&err), want, doc)?;
    }
    Ok(())
}

/// Go: `TestValidDeviceExt`, through the serde code.
#[test]
fn valid_device_ext_serde() -> TestResult {
    let empty_prebid: ExtDevice = serde_unmarshal(r#"{"prebid":{}}"#)?;
    ensure_eq(
        &empty_prebid.prebid.interstitial,
        &None,
        "prebid interstitial",
    )?;
    let empty: ExtDevice = serde_unmarshal("{}")?;
    ensure_eq(&empty.prebid.interstitial, &None, "empty interstitial")?;
    let s: ExtDevice =
        serde_unmarshal(r#"{"prebid":{"interstitial":{"minwidthperc":75,"minheightperc":60}}}"#)?;
    let interstitial = s.prebid.interstitial.ok_or("no interstitial")?;
    ensure_eq(&interstitial.min_width_perc, &75, "MinWidthPerc")?;
    ensure_eq(&interstitial.min_height_perc, &60, "MinHeightPerc")
}

/// Go: `TestValidDeviceExt`.
#[test]
fn valid_device_ext() -> TestResult {
    let mut s = ExtDevice::default();
    ensure_eq(
        &unmarshal_error(r#"{"prebid":{}}"#, &mut s),
        &None,
        "prebid",
    )?;
    ensure_eq(&s.prebid.interstitial, &None, "prebid interstitial")?;
    ensure_eq(&unmarshal_error("{}", &mut s), &None, "empty")?;
    ensure_eq(&s.prebid.interstitial, &None, "empty interstitial")?;
    let doc = r#"{"prebid":{"interstitial":{"minwidthperc":75,"minheightperc":60}}}"#;
    ensure_eq(&unmarshal_error(doc, &mut s), &None, doc)?;
    let interstitial = s.prebid.interstitial.ok_or("no interstitial")?;
    ensure_eq(&interstitial.min_width_perc, &75, "MinWidthPerc")?;
    ensure_eq(&interstitial.min_height_perc, &60, "MinHeightPerc")
}

/// Go: `TestIsKnownIOSAppTrackingStatus`.
#[test]
fn known_ios_app_tracking_status() -> TestResult {
    for v in [0, 1, 2, 3] {
        ensure_eq(&is_known_ios_app_tracking_status(v), &true, &v.to_string())?;
    }
    for v in [-1, 4] {
        ensure_eq(&is_known_ios_app_tracking_status(v), &false, &v.to_string())?;
    }
    Ok(())
}

/// Go: `TestParseDeviceExtATTS`. Go's nil and empty `json.RawMessage` are both an empty slice.
/// The ext is read with `serde_json`, so the wrong type fails with its text where Go's is
/// jsonparser's `Value is not a number: 1` (`json.error-texts`).
#[test]
fn parse_device_ext_atts_cases() -> TestResult {
    let authorized = IOS_APP_TRACKING_STATUS_AUTHORIZED;
    let cases = [
        ("Nil", &b""[..], None, ""),
        ("Empty", b"", None, ""),
        ("Empty Object", b"{}", None, ""),
        ("Valid", br#"{"atts":3}"#, Some(authorized), ""),
        ("Invalid Value", br#"{"atts":5}"#, None, "invalid status"),
        // Bytes that do not start an object give no status, as jsonparser finds no `atts` in
        // them (encoding/json would fail).
        ("Malformed - Standard Test Case", b"malformed", None, ""),
        (
            "Malformed - Wrong Type",
            br#"{"atts":"1"}"#,
            None,
            r#"invalid type: string "1", expected i64 at line 1 column 3"#,
        ),
    ];
    all_ok(cases.into_iter().map(
        |(description, given_ext, expected_status, expected_error)| {
            let (status, err) = match parse_device_ext_atts(given_ext) {
                Ok(status) => (status, String::new()),
                Err(e) => (None, e.message().to_owned()),
            };
            let result = ensure_eq(err.as_str(), expected_error, "err")
                .and_then(|()| ensure_eq(&status, &expected_status, "status"));
            (description.to_owned(), result)
        },
    ))
}
