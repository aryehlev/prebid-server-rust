//! Go: `openrtb_ext/convert_up_test.go`. The requests and impressions of Go's struct literals
//! are written as their JSON, decoded with `jsonutil.Unmarshal`.

use core::{
    fmt::{Debug, Display},
    iter,
};

use pbs_gojson::{JsoniterDecode, RawJson, jsonutil};
use pbs_openrtb::openrtb2;

use super::{
    convert_up_ensure_ext, convert_up_to26, move_consent_from25_to26, move_eid_from25_to26,
    move_gdpr_from25_to26, move_rewarded_from_prebid_ext_to26, move_supply_chain_from24_to25,
    move_supply_chain_from25_to26, move_us_privacy_from25_to26,
};
use crate::request_wrapper::{ImpWrapper, RequestWrapper};

/// A test result whose error is the failure message.
type TestResult = Result<(), String>;

/// Fails, showing both values, unless they are equal.
fn ensure_eq<T: PartialEq + Debug + ?Sized>(got: &T, want: &T, what: &str) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

/// Fails with every failure of `cases` (name, result).
fn all_ok<N: Display, I: IntoIterator<Item = (N, TestResult)>>(cases: I) -> TestResult {
    let failures: Vec<String> = cases
        .into_iter()
        .filter_map(|(name, result)| result.err().map(|err| format!("{name}: {err}")))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// The value `json` decodes into.
fn decode<T: JsoniterDecode + Default>(json: &str) -> Result<T, String> {
    let mut value = T::default();
    jsonutil::unmarshal(json.as_bytes(), &mut value).map_err(|err| format!("{json}: {err}"))?;
    Ok(value)
}

/// Go's test loop of the move functions: the wrapper of the request `given`, changed by `step`
/// and rebuilt, is the request `expected`.
fn move_case(given: &str, step: fn(&mut RequestWrapper), expected: &str) -> TestResult {
    let mut w = RequestWrapper::new(decode(given)?);
    step(&mut w);
    w.rebuild_request().map_err(|err| err.to_string())?;
    ensure_eq(&w.bid_request(), &Some(&decode(expected)?), "request")
}

/// The cases of a move function's test (name, given request, expected request).
fn move_cases<const N: usize>(
    cases: [(&str, &str, &str); N],
    step: fn(&mut RequestWrapper),
) -> TestResult {
    all_ok(
        cases
            .into_iter()
            .map(|(name, given, expected)| (name, move_case(given, step, expected))),
    )
}

/// Go: `TestConvertUpTo26`.
#[test]
fn convert_up_to26_cases() -> TestResult {
    let given_24 = concat!(
        r#"{"id":"anyID","imp":[{"ext":{"prebid":{"is_rewarded_inventory":1}}}],"#,
        r#""ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}},"#,
        r#""regs":{"ext":{"gdpr":1,"us_privacy":"3"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}]}}}"#,
    );
    let expected_26 = concat!(
        r#"{"id":"anyID","imp":[{"rwdd":1}],"#,
        r#""source":{"schain":{"complete":1,"nodes":[],"ver":"2"}},"#,
        r#""regs":{"gdpr":1,"us_privacy":"3"},"#,
        r#""user":{"consent":"1","eids":[{"source":"42"}]}}"#,
    );
    let given_24_other = concat!(
        r#"{"id":"anyID","#,
        r#""imp":[{"ext":{"prebid":{"is_rewarded_inventory":1},"other":"otherImp"}}],"#,
        r#""ext":{"schain":{"complete":1,"nodes":[],"ver":"2"},"other":"otherExt"},"#,
        r#""regs":{"ext":{"gdpr":1,"other":"otherRegs","us_privacy":"3"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}],"other":"otherUser"}}}"#,
    );
    let expected_24_other = concat!(
        r#"{"id":"anyID","imp":[{"rwdd":1,"ext":{"other":"otherImp"}}],"#,
        r#""ext":{"other":"otherExt"},"#,
        r#""source":{"schain":{"complete":1,"nodes":[],"ver":"2"}},"#,
        r#""regs":{"gdpr":1,"us_privacy":"3","ext":{"other":"otherRegs"}},"#,
        r#""user":{"consent":"1","eids":[{"source":"42"}],"ext":{"other":"otherUser"}}}"#,
    );
    let given_25 = concat!(
        r#"{"id":"anyID","imp":[{"ext":{"prebid":{"is_rewarded_inventory":1}}}],"#,
        r#""source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}},"#,
        r#""regs":{"ext":{"gdpr":1,"us_privacy":"3"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}]}}}"#,
    );
    let given_25_other = concat!(
        r#"{"id":"anyID","#,
        r#""imp":[{"ext":{"prebid":{"is_rewarded_inventory":1},"other":"otherImp"}}],"#,
        r#""ext":{"other":"otherExt"},"#,
        r#""source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"2"},"other":"otherSource"}},"#,
        r#""regs":{"ext":{"gdpr":1,"us_privacy":"3","other":"otherRegs"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}],"other":"otherUser"}}}"#,
    );
    let expected_25_other = concat!(
        r#"{"id":"anyID","imp":[{"rwdd":1,"ext":{"other":"otherImp"}}],"#,
        r#""ext":{"other":"otherExt"},"#,
        r#""source":{"schain":{"complete":1,"nodes":[],"ver":"2"},"ext":{"other":"otherSource"}},"#,
        r#""regs":{"gdpr":1,"us_privacy":"3","ext":{"other":"otherRegs"}},"#,
        r#""user":{"consent":"1","eids":[{"source":"42"}],"ext":{"other":"otherUser"}}}"#,
    );
    let mut malformed_ext = RequestWrapper::new(malformed_request(|req| req.ext = malformed()));
    let got = convert_up_to26(&mut malformed_ext).map_err(|err| err.to_string());
    let want = Err("req.ext is invalid: expect { or n, but found m".to_owned());
    let cases = [
        ("2.4 -> 2.6", given_24, expected_26),
        (
            "2.4 -> 2.6 + Other Ext Fields",
            given_24_other,
            expected_24_other,
        ),
        ("2.5 -> 2.6", given_25, expected_26),
        (
            "2.5 -> 2.6 + Other Ext Fields",
            given_25_other,
            expected_25_other,
        ),
    ];
    let converted = cases.into_iter().map(|(name, given, expected)| {
        let result = decode(given).and_then(|req| {
            let mut w = RequestWrapper::new(req);
            convert_up_to26(&mut w).map_err(|err| err.to_string())?;
            w.rebuild_request().map_err(|err| err.to_string())?;
            ensure_eq(&w.bid_request(), &Some(&decode(expected)?), "request")
        });
        (name, result)
    });
    all_ok(iter::once(("Malformed", ensure_eq(&got, &want, "error"))).chain(converted))
}

/// Gives a request an invalid ext.
type Place = fn(&mut openrtb2::BidRequest);

/// A request that `place` gives an invalid ext, which JSON cannot hold (Go's
/// `json.RawMessage("malformed")`).
fn malformed_request(place: Place) -> openrtb2::BidRequest {
    let mut req = openrtb2::BidRequest::default();
    place(&mut req);
    req
}

/// The invalid ext of the malformed cases.
fn malformed() -> RawJson {
    RawJson::from("malformed")
}

/// Go: `TestConvertUpEnsureExt`.
#[test]
fn convert_up_ensure_ext_cases() -> TestResult {
    let cases: [(&str, Place, Option<&str>); 6] = [
        ("Empty", |_| {}, None),
        (
            "Ext",
            |req| req.ext = malformed(),
            Some("req.ext is invalid: expect { or n, but found m"),
        ),
        (
            "Source.Ext",
            |req| {
                req.source = Some(Box::new(openrtb2::Source {
                    ext: malformed(),
                    ..openrtb2::Source::default()
                }));
            },
            Some("req.source.ext is invalid: expect { or n, but found m"),
        ),
        (
            "Regs.Ext",
            |req| {
                req.regs = Some(Box::new(openrtb2::Regs {
                    ext: malformed(),
                    ..openrtb2::Regs::default()
                }));
            },
            Some("req.regs.ext is invalid: expect { or n, but found m"),
        ),
        (
            "User.Ext",
            |req| {
                req.user = Some(Box::new(openrtb2::User {
                    ext: malformed(),
                    ..openrtb2::User::default()
                }));
            },
            Some("req.user.ext is invalid: expect { or n, but found m"),
        ),
        (
            "Imp.Ext",
            |req| {
                req.imp = Some(vec![openrtb2::Imp {
                    ext: malformed(),
                    ..openrtb2::Imp::default()
                }]);
            },
            Some("imp[0].imp.ext is invalid: expect { or n, but found m"),
        ),
    ];
    all_ok(cases.into_iter().map(|(name, place, expected)| {
        let mut w = RequestWrapper::new(malformed_request(place));
        let got = convert_up_ensure_ext(&mut w)
            .err()
            .map(|err| err.to_string());
        (name, ensure_eq(&got, &expected.map(str::to_owned), "error"))
    }))
}

/// Go: `TestMoveSupplyChainFrom24To25`.
#[test]
fn move_supply_chain_from24_to25_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.4 Migrated To 2.5 - Source Doesn't Exist",
                r#"{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
            ),
            (
                "2.4 Migrated To 2.5 - Source Exists",
                r#"{"source":{},"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
            ),
            (
                "2.4 Dropped",
                concat!(
                    r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}},"#,
                    r#""ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}}"#,
                ),
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
            ),
            (
                "2.5 Left Alone",
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
            ),
        ],
        move_supply_chain_from24_to25,
    )
}

/// Go: `TestConvertSupplyChainFrom25To26`.
#[test]
fn move_supply_chain_from25_to26_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.5 Migrated To 2.6",
                r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#,
                r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
            ),
            (
                "2.5 Dropped",
                concat!(
                    r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"},"#,
                    r#""ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}}}"#,
                ),
                r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
            ),
            (
                "2.6 Left Alone",
                r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
                r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#,
            ),
        ],
        move_supply_chain_from25_to26,
    )
}

/// Go: `TestMoveGDPRFrom25To26`.
#[test]
fn move_gdpr_from25_to26_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.5 Migrated To 2.6",
                r#"{"regs":{"ext":{"gdpr":0}}}"#,
                r#"{"regs":{"gdpr":0}}"#,
            ),
            (
                "2.5 Dropped",
                r#"{"regs":{"gdpr":0,"ext":{"gdpr":1}}}"#,
                r#"{"regs":{"gdpr":0}}"#,
            ),
            (
                "2.6 Left Alone",
                r#"{"regs":{"gdpr":0}}"#,
                r#"{"regs":{"gdpr":0}}"#,
            ),
        ],
        move_gdpr_from25_to26,
    )
}

/// Go: `TestMoveConsentFrom25To26`.
#[test]
fn move_consent_from25_to26_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.5 Migrated To 2.6",
                r#"{"user":{"ext":{"consent":"1"}}}"#,
                r#"{"user":{"consent":"1"}}"#,
            ),
            (
                "2.5 Dropped",
                r#"{"user":{"consent":"1","ext":{"consent":2}}}"#,
                r#"{"user":{"consent":"1"}}"#,
            ),
            (
                "2.6 Left Alone",
                r#"{"user":{"consent":"1"}}"#,
                r#"{"user":{"consent":"1"}}"#,
            ),
        ],
        move_consent_from25_to26,
    )
}

/// Go: `TestMoveUSPrivacyFrom25To26`.
#[test]
fn move_us_privacy_from25_to26_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.5 Migrated To 2.6",
                r#"{"regs":{"ext":{"us_privacy":"1"}}}"#,
                r#"{"regs":{"us_privacy":"1"}}"#,
            ),
            (
                "2.5 Dropped",
                r#"{"regs":{"us_privacy":"1","ext":{"us_privacy":"2"}}}"#,
                r#"{"regs":{"us_privacy":"1"}}"#,
            ),
            (
                "2.6 Left Alone",
                r#"{"regs":{"us_privacy":"1"}}"#,
                r#"{"regs":{"us_privacy":"1"}}"#,
            ),
        ],
        move_us_privacy_from25_to26,
    )
}

/// Go: `TestMoveEIDFrom25To26`.
#[test]
fn move_eid_from25_to26_cases() -> TestResult {
    move_cases(
        [
            ("Not Present", "{}", "{}"),
            (
                "2.5 Migrated To 2.6",
                r#"{"user":{"ext":{"eids":[{"source":"1"}]}}}"#,
                r#"{"user":{"eids":[{"source":"1"}]}}"#,
            ),
            (
                "2.5 Dropped",
                r#"{"user":{"eids":[{"source":"1"}],"ext":{"eids":[{"source":"2"}]}}}"#,
                r#"{"user":{"eids":[{"source":"1"}]}}"#,
            ),
            (
                "2.6 Left Alone",
                r#"{"user":{"eids":[{"source":"1"}]}}"#,
                r#"{"user":{"eids":[{"source":"1"}]}}"#,
            ),
        ],
        move_eid_from25_to26,
    )
}

/// Go: `TestMoveRewardedFromPrebidExtTo26`.
#[test]
fn move_rewarded_from_prebid_ext_to26_cases() -> TestResult {
    let rwdd_1 = r#"{"ext":{"prebid":{"is_rewarded_inventory":1}}}"#;
    let cases = [
        ("Not Present - No Ext", "{}", "{}"),
        ("Not Present - Empty Ext", r#"{"ext":{}}"#, r#"{"ext":{}}"#),
        (
            "Not Present - Null Prebid Ext",
            r#"{"ext":{"prebid":null}}"#,
            r#"{"ext":{"prebid":null}}"#,
        ),
        (
            "Not Present - Empty Prebid Ext",
            r#"{"ext":{"prebid":{}}}"#,
            r#"{"ext":{"prebid":{}}}"#,
        ),
        ("Prebid Ext Migrated To 2.6", rwdd_1, r#"{"rwdd":1}"#),
        (
            "2.5 Dropped",
            r#"{"rwdd":1,"ext":{"prebid":{"is_rewarded_inventory":2}}}"#,
            r#"{"rwdd":1}"#,
        ),
        ("2.6 Left Alone", r#"{"rwdd":1}"#, r#"{"rwdd":1}"#),
    ];
    all_ok(cases.into_iter().map(|(name, given, expected)| {
        let result = decode::<openrtb2::Imp>(given).and_then(|imp| {
            let mut w = ImpWrapper::new(imp);
            move_rewarded_from_prebid_ext_to26(&mut w.view());
            w.rebuild_imp().map_err(|err| err.to_string())?;
            ensure_eq(&w.imp(), &Some(&decode(expected)?), "imp")
        });
        (name, result)
    }))
}
