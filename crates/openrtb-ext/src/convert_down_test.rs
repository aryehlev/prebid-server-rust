//! Go: `openrtb_ext/convert_down_test.go`. The requests and impressions of Go's struct literals
//! are written as their JSON, decoded with `jsonutil.Unmarshal`; an invalid ext, which JSON
//! cannot hold, is set on the decoded value.

use core::fmt::{Debug, Display};

use pbs_errortypes::{ErrorKind, GoError};
use pbs_gojson::{JsoniterDecode, RawJson, jsonutil};
use pbs_openrtb::openrtb2;

use super::{
    clear26_fields, clear202211_fields, clear202303_fields, clear202309_fields, clear202402_fields,
    clear202409_fields, convert_down_to25, move_consent_from26_to25, move_eid_from26_to25,
    move_gdpr_from26_to25, move_rewarded_from26_to_prebid_ext, move_supply_chain_from26_to25,
    move_us_privacy_from26_to25,
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

/// Gives a request an invalid ext.
type Place = fn(&mut openrtb2::BidRequest);

/// Go's `json.RawMessage("malformed")`.
fn malformed() -> RawJson {
    RawJson::from("malformed")
}

/// What a case of a move function's test expects: the rebuilt request (as JSON), or that the
/// move fails with a `FailedToUnmarshal` (Go's `assert.IsType(&errortypes.FailedToUnmarshal{})`).
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// The rebuilt request.
    Request(&'static str),
    /// A `FailedToUnmarshal`.
    FailedToUnmarshal,
}

/// Checks that `result` is a `FailedToUnmarshal`.
fn ensure_failed_to_unmarshal(result: Result<(), GoError>) -> TestResult {
    let kind = result.err().map(|err| err.kind().clone());
    ensure_eq(&kind, &Some(ErrorKind::FailedToUnmarshal), "error type")
}

/// Go's test loop of the move functions: the wrapper of `given`, changed by `step`, fails as
/// expected or is rebuilt into the expected request.
fn move_case(
    given: openrtb2::BidRequest,
    step: fn(&mut RequestWrapper) -> Result<(), GoError>,
    expected: Expect,
) -> TestResult {
    let mut w = RequestWrapper::new(given);
    let result = step(&mut w);
    let Expect::Request(expected) = expected else {
        return ensure_failed_to_unmarshal(result);
    };
    result.map_err(|err| err.to_string())?;
    w.rebuild_request().map_err(|err| err.to_string())?;
    ensure_eq(&w.bid_request(), &Some(&decode(expected)?), "result")
}

/// The cases of a move function's test (name, given request as JSON, expected), and its
/// `malformed` case: the request `malformed_json` whose ext `place` makes invalid.
fn move_cases<const N: usize>(
    cases: [(&str, &str, &'static str); N],
    malformed_json: &str,
    place: Place,
    step: fn(&mut RequestWrapper) -> Result<(), GoError>,
) -> TestResult {
    let mut results: Vec<(&str, TestResult)> = cases
        .into_iter()
        .map(|(name, given, expected)| {
            let result =
                decode(given).and_then(|req| move_case(req, step, Expect::Request(expected)));
            (name, result)
        })
        .collect();
    let malformed_result = decode(malformed_json).and_then(|mut req| {
        place(&mut req);
        move_case(req, step, Expect::FailedToUnmarshal)
    });
    results.push(("malformed", malformed_result));
    all_ok(results)
}

/// Go: `TestConvertDownTo25`, the cases that convert.
#[test]
fn convert_down_to25_cases() -> TestResult {
    let given = concat!(
        r#"{"id":"anyID","imp":[{"rwdd":1}],"#,
        r#""source":{"schain":{"complete":1,"nodes":[],"ver":"2"}},"#,
        r#""regs":{"gdpr":1,"us_privacy":"3"},"#,
        r#""user":{"consent":"1","eids":[{"source":"42"}]}}"#,
    );
    let expected = concat!(
        r#"{"id":"anyID","imp":[{"ext":{"prebid":{"is_rewarded_inventory":1}}}],"#,
        r#""source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}},"#,
        r#""regs":{"ext":{"gdpr":1,"us_privacy":"3"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}]}}}"#,
    );
    let given_other = concat!(
        r#"{"id":"anyID","imp":[{"rwdd":1,"ext":{"other":"otherImp"}}],"#,
        r#""ext":{"other":"otherExt"},"#,
        r#""source":{"schain":{"complete":1,"nodes":[],"ver":"2"},"ext":{"other":"otherSource"}},"#,
        r#""regs":{"gdpr":1,"us_privacy":"3","ext":{"other":"otherRegs"}},"#,
        r#""user":{"consent":"1","eids":[{"source":"42"}],"ext":{"other":"otherUser"}}}"#,
    );
    let expected_other = concat!(
        r#"{"id":"anyID","#,
        r#""imp":[{"ext":{"other":"otherImp","prebid":{"is_rewarded_inventory":1}}}],"#,
        r#""ext":{"other":"otherExt"},"#,
        r#""source":{"ext":{"other":"otherSource","schain":{"complete":1,"nodes":[],"ver":"2"}}},"#,
        r#""regs":{"ext":{"gdpr":1,"other":"otherRegs","us_privacy":"3"}},"#,
        r#""user":{"ext":{"consent":"1","eids":[{"source":"42"}],"other":"otherUser"}}}"#,
    );
    all_ok(
        [
            ("2.6-to-2.5", given, expected),
            ("2.6-to-2.5-OtherExtFields", given_other, expected_other),
        ]
        .map(|(name, given, expected)| {
            let result = decode(given)
                .and_then(|req| move_case(req, convert_down_to25, Expect::Request(expected)));
            (name, result)
        }),
    )
}

/// Go: `TestConvertDownTo25`, the malformed cases.
#[test]
fn convert_down_to25_malformed() -> TestResult {
    let cases: [(&str, &str, Place); 6] = [
        (
            "malformed-schain",
            r#"{"id":"anyID","source":{"schain":{"complete":1,"nodes":[],"ver":"2"}}}"#,
            |req| malformed_source_ext(req),
        ),
        (
            "malformed-gdpr",
            r#"{"id":"anyID","regs":{"gdpr":1}}"#,
            |req| {
                malformed_regs_ext(req);
            },
        ),
        (
            "malformed-consent",
            r#"{"id":"anyID","user":{"consent":"1"}}"#,
            |req| {
                malformed_user_ext(req);
            },
        ),
        (
            "malformed-usprivacy",
            r#"{"id":"anyID","regs":{"us_privacy":"3"}}"#,
            |req| {
                malformed_regs_ext(req);
            },
        ),
        (
            "malformed-eid",
            r#"{"id":"anyID","user":{"eids":[{"source":"42"}]}}"#,
            |req| {
                malformed_user_ext(req);
            },
        ),
        (
            "malformed-imp",
            r#"{"id":"anyID","imp":[{"rwdd":1}]}"#,
            |req| {
                malformed_imp_ext(req);
            },
        ),
    ];
    all_ok(cases.into_iter().map(|(name, given, place)| {
        let result = decode(given).and_then(|mut req| {
            place(&mut req);
            move_case(req, convert_down_to25, Expect::FailedToUnmarshal)
        });
        (name, result)
    }))
}

/// Makes the request's `source.ext` invalid.
fn malformed_source_ext(req: &mut openrtb2::BidRequest) {
    if let Some(source) = req.source.as_deref_mut() {
        source.ext = malformed();
    }
}

/// Makes the request's `regs.ext` invalid.
fn malformed_regs_ext(req: &mut openrtb2::BidRequest) {
    if let Some(regs) = req.regs.as_deref_mut() {
        regs.ext = malformed();
    }
}

/// Makes the request's `user.ext` invalid.
fn malformed_user_ext(req: &mut openrtb2::BidRequest) {
    if let Some(user) = req.user.as_deref_mut() {
        user.ext = malformed();
    }
}

/// Makes the `ext` of the request's impressions invalid.
fn malformed_imp_ext(req: &mut openrtb2::BidRequest) {
    for imp in req.imp.iter_mut().flatten() {
        imp.ext = malformed();
    }
}

/// Go: `TestMoveSupplyChainFrom26To25`.
#[test]
fn move_supply_chain_from26_to25_cases() -> TestResult {
    let schain_1 = r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}"#;
    let ext_1 = r#"{"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"1"}}}}"#;
    let overwritten = concat!(
        r#"{"source":{"schain":{"complete":1,"nodes":[],"ver":"1"},"#,
        r#""ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}}}"#,
    );
    move_cases(
        [
            ("notpresent-source", "{}", "{}"),
            (
                "notpresent-source-schain",
                r#"{"source":{}}"#,
                r#"{"source":{}}"#,
            ),
            ("2.6-migratedto-2.5", schain_1, ext_1),
            ("2.5-overwritten", overwritten, ext_1),
        ],
        schain_1,
        malformed_source_ext,
        move_supply_chain_from26_to25,
    )
}

/// Go: `TestMoveGDPRFrom26To25`.
#[test]
fn move_gdpr_from26_to25_cases() -> TestResult {
    let gdpr_0 = r#"{"regs":{"gdpr":0}}"#;
    move_cases(
        [
            ("notpresent-regs", "{}", "{}"),
            ("notpresent-regs-gdpr", r#"{"regs":{}}"#, r#"{"regs":{}}"#),
            (
                "2.6-migratedto-2.5",
                gdpr_0,
                r#"{"regs":{"ext":{"gdpr":0}}}"#,
            ),
            (
                "2.5-overwritten",
                r#"{"regs":{"gdpr":0,"ext":{"gdpr":1}}}"#,
                r#"{"regs":{"ext":{"gdpr":0}}}"#,
            ),
        ],
        gdpr_0,
        malformed_regs_ext,
        move_gdpr_from26_to25,
    )
}

/// Go: `TestMoveConsentFrom26To25`.
#[test]
fn move_consent_from26_to25_cases() -> TestResult {
    let consent_1 = r#"{"user":{"consent":"1"}}"#;
    move_cases(
        [
            ("notpresent-user", "{}", "{}"),
            (
                "notpresent-user-consent",
                r#"{"user":{}}"#,
                r#"{"user":{}}"#,
            ),
            (
                "2.6-migratedto-2.5",
                consent_1,
                r#"{"user":{"ext":{"consent":"1"}}}"#,
            ),
            (
                "2.5-overwritten",
                r#"{"user":{"consent":"1","ext":{"consent":"2"}}}"#,
                r#"{"user":{"ext":{"consent":"1"}}}"#,
            ),
        ],
        consent_1,
        malformed_user_ext,
        move_consent_from26_to25,
    )
}

/// Go: `TestMoveUSPrivacyFrom26To25`.
#[test]
fn move_us_privacy_from26_to25_cases() -> TestResult {
    let us_privacy_1 = r#"{"regs":{"us_privacy":"1"}}"#;
    move_cases(
        [
            ("notpresent-regs", "{}", "{}"),
            (
                "notpresent-regs-usprivacy",
                r#"{"regs":{}}"#,
                r#"{"regs":{}}"#,
            ),
            (
                "2.6-migratedto-2.5",
                us_privacy_1,
                r#"{"regs":{"ext":{"us_privacy":"1"}}}"#,
            ),
            (
                "2.5-overwritten",
                r#"{"regs":{"us_privacy":"1","ext":{"us_privacy":"2"}}}"#,
                r#"{"regs":{"ext":{"us_privacy":"1"}}}"#,
            ),
        ],
        us_privacy_1,
        malformed_regs_ext,
        move_us_privacy_from26_to25,
    )
}

/// Go: `TestMoveEIDFrom26To25`.
#[test]
fn move_eid_from26_to25_cases() -> TestResult {
    let eid_1 = r#"{"user":{"eids":[{"source":"1"}]}}"#;
    let ext_1 = r#"{"user":{"ext":{"eids":[{"source":"1"}]}}}"#;
    move_cases(
        [
            ("notpresent-user", "{}", "{}"),
            ("notpresent-user-eids", r#"{"user":{}}"#, r#"{"user":{}}"#),
            ("2.6-migratedto-2.5", eid_1, ext_1),
            (
                "2.6-migratedto-2.5-empty",
                r#"{"user":{"eids":[]}}"#,
                r#"{"user":{}}"#,
            ),
            (
                "2.5-overwritten",
                r#"{"user":{"eids":[{"source":"1"}],"ext":{"eids":[{"source":"2"}]}}}"#,
                ext_1,
            ),
        ],
        eid_1,
        malformed_user_ext,
        move_eid_from26_to25,
    )
}

/// Go: `TestMoveRewardedFrom26ToPrebidExt`.
#[test]
fn move_rewarded_from26_to_prebid_ext_cases() -> TestResult {
    let rewarded_1 = r#"{"ext":{"prebid":{"is_rewarded_inventory":1}}}"#;
    let cases = [
        ("notpresent-prebid", "{}", Expect::Request("{}")),
        (
            "2.6-migratedto-2.5",
            r#"{"rwdd":1}"#,
            Expect::Request(rewarded_1),
        ),
        (
            "2.5-overwritten",
            r#"{"rwdd":1,"ext":{"prebid":{"is_rewarded_inventory":2}}}"#,
            Expect::Request(rewarded_1),
        ),
        ("Malformed", r#"{"rwdd":1}"#, Expect::FailedToUnmarshal),
    ];
    all_ok(cases.into_iter().map(|(name, given, expected)| {
        let result = decode::<openrtb2::Imp>(given).and_then(|mut imp| {
            if matches!(expected, Expect::FailedToUnmarshal) {
                imp.ext = malformed();
            }
            let mut w = ImpWrapper::new(imp);
            let moved = move_rewarded_from26_to_prebid_ext(&mut w.view());
            let Expect::Request(expected_imp) = expected else {
                return ensure_failed_to_unmarshal(moved);
            };
            moved.map_err(|err| err.to_string())?;
            w.rebuild_imp().map_err(|err| err.to_string())?;
            ensure_eq(&w.imp(), &Some(&decode(expected_imp)?), "result")
        });
        (name, result)
    }))
}

/// A case of the `clear*_fields` tests: the request `given`, cleared by `clear`, is `expected`
/// (Go compares the request without rebuilding it).
fn clear_case(given: &str, clear: fn(&mut RequestWrapper), expected: &str) -> TestResult {
    let mut r = RequestWrapper::new(decode(given)?);
    clear(&mut r);
    ensure_eq(&r.bid_request(), &Some(&decode(expected)?), "request")
}

/// The `app` and `site` of `TestClear26Fields`'s given request (`cattax` 4 is
/// `adcom1.CatTaxIABAudience11`).
const CLEAR26_DISTRIBUTION: &str = concat!(
    r#"{"cattax":4,"kwarray":["anyKeyword"],"#,
    r#""content":{"id":"anyContent","cattax":4,"kwarray":["anyKeyword"],"langb":"anyLang","#,
    r#""network":{"id":"anyNetwork"},"channel":{"id":"anyChannel"},"#,
    r#""producer":{"id":"anyProcedure","cattax":4}},"#,
    r#""publisher":{"id":"anyPublisher","cattax":4}}"#,
);

/// The `app` and `site` of `TestClear26Fields`'s expected request.
const CLEAR26_DISTRIBUTION_CLEARED: &str = concat!(
    r#"{"content":{"id":"anyContent","producer":{"id":"anyProcedure"}},"#,
    r#""publisher":{"id":"anyPublisher"}}"#,
);

/// Go: `TestClear26Fields` (`ssai` 3 is `openrtb2.AdInsertServer`, `podseq` and `slotinpod` 1
/// are `adcom1.PodSeqFirst` and `adcom1.SlotPosFirst`).
#[test]
fn clear26_fields_case() -> TestResult {
    let given = [
        r#"{"id":"anyID","wlangb":["anyLang"],"cattax":4,"#,
        &format!(r#""app":{CLEAR26_DISTRIBUTION},"site":{CLEAR26_DISTRIBUTION},"#),
        r#""device":{"ip":"1.2.3.4","langb":"anyLang","sua":{"model":"PBS 2000"}},"#,
        r#""regs":{"coppa":1,"gdpr":1,"us_privacy":"anyCCPA"},"#,
        r#""source":{"tid":"anyTransactionID","schain":{"complete":1}},"#,
        r#""user":{"id":"anyUser","kwarray":["anyKeyword"],"consent":"anyConsent","#,
        r#""eids":[{"source":"anySource"}]},"#,
        r#""imp":[{"id":"imp1","rwdd":1,"ssai":3,"#,
        r#""audio":{"mimes":["any/audio"],"poddur":30,"rqddurs":[15,60],"podid":"1","#,
        r#""podseq":1,"slotinpod":1,"mincpmpersec":100},"#,
        r#""video":{"mimes":["any/video"],"maxseq":30,"poddur":30,"podid":"1","podseq":1,"#,
        r#""rqddurs":[15,60],"slotinpod":1,"mincpmpersec":100}}]}"#,
    ]
    .concat();
    let expected = [
        r#"{"id":"anyID","#,
        &format!(r#""app":{CLEAR26_DISTRIBUTION_CLEARED},"#),
        &format!(r#""site":{CLEAR26_DISTRIBUTION_CLEARED},"#),
        r#""device":{"ip":"1.2.3.4"},"regs":{"coppa":1},"source":{"tid":"anyTransactionID"},"#,
        r#""user":{"id":"anyUser"},"#,
        r#""imp":[{"id":"imp1","audio":{"mimes":["any/audio"]},"video":{"mimes":["any/video"]}}]}"#,
    ]
    .concat();
    clear_case(&given, clear26_fields, &expected)
}

/// Go: `TestClear202211Fields`.
#[test]
fn clear202211_fields_cases() -> TestResult {
    let tail = concat!(
        r#""imp":[{"id":"imp1","qty":{"multiplier":2},"dt":42}],"#,
        r#""regs":{"gpp":"anyGPP","gpp_sid":[1,2,3]}}"#,
    );
    let cleared_tail = r#""imp":[{"id":"imp1"}],"regs":{}}"#;
    let cases = [
        (
            "app",
            format!(r#"{{"id":"anyID","app":{{"inventorypartnerdomain":"anyDomain"}},{tail}"#),
            format!(r#"{{"id":"anyID","app":{{}},{cleared_tail}"#),
        ),
        (
            "site",
            format!(r#"{{"id":"anyID","site":{{"inventorypartnerdomain":"anyDomain"}},{tail}"#),
            format!(r#"{{"id":"anyID","site":{{}},{cleared_tail}"#),
        ),
        (
            "dooh",
            format!(r#"{{"id":"anyID","dooh":{{"id":"anyDOOH"}},{tail}"#),
            format!(r#"{{"id":"anyID",{cleared_tail}"#),
        ),
    ];
    all_ok(
        cases.into_iter().map(|(name, given, expected)| {
            (name, clear_case(&given, clear202211_fields, &expected))
        }),
    )
}

/// Go: `TestClear202303Fields` (`plcmt` 1 is `adcom1.VideoPlcmtInstream`).
#[test]
fn clear202303_fields_case() -> TestResult {
    clear_case(
        concat!(
            r#"{"id":"anyID","imp":[{"id":"imp1","video":{"podid":"1","plcmt":1},"#,
            r#""refresh":{"count":1}}]}"#,
        ),
        clear202303_fields,
        r#"{"id":"anyID","imp":[{"id":"imp1","video":{"podid":"1"}}]}"#,
    )
}

/// Go: `TestClear202309Fields`.
#[test]
fn clear202309_fields_case() -> TestResult {
    let dur_floors = r#"[{"mindur":15,"maxdur":30,"bidfloor":100}]"#;
    let given = format!(
        concat!(
            r#"{{"id":"anyID","acat":["acat1","acat2"],"imp":["#,
            r#"{{"id":"imp1","audio":{{"podid":"1","durfloors":{dur_floors}}}}},"#,
            r#"{{"id":"imp2","video":{{"podid":"2","durfloors":{dur_floors}}},"#,
            r#""pmp":{{"private_auction":1,"deals":[{{"id":"deal1","bidfloor":200,"guar":1,"#,
            r#""mincpmpersec":2,"durfloors":{dur_floors}}}]}}}}]}}"#,
        ),
        dur_floors = dur_floors,
    );
    let expected = concat!(
        r#"{"id":"anyID","imp":[{"id":"imp1","audio":{"podid":"1"}},"#,
        r#"{"id":"imp2","video":{"podid":"2"},"#,
        r#""pmp":{"private_auction":1,"deals":[{"id":"deal1","bidfloor":200}]}}]}"#,
    );
    clear_case(&given, clear202309_fields, expected)
}

/// Go: `TestClear202402Fields` (`poddedupe` 1 is `adcom1.PodDedupeADomain`).
#[test]
fn clear202402_fields_case() -> TestResult {
    clear_case(
        r#"{"id":"anyID","imp":[{"id":"imp2","video":{"podid":"1","poddedupe":[1]}}]}"#,
        clear202402_fields,
        r#"{"id":"anyID","imp":[{"id":"imp2","video":{"podid":"1"}}]}"#,
    )
}

/// Go: `TestClear202409Fields` (`mm` 2 is `adcom1.MatchMethodBrowserCookieSync`).
#[test]
fn clear202409_fields_cases() -> TestResult {
    let cleared = concat!(
        r#"{"user":{"eids":[{"source":"anySource","inserter":"anyInserter","#,
        r#""matcher":"anyMatcher","mm":2,"uids":[{"id":"anyID"}]}]}}"#,
    );
    let cases = [
        ("user-nil", "{}", "{}"),
        ("eids-nil", r#"{"user":{}}"#, r#"{"user":{}}"#),
        (
            "cleared",
            cleared,
            r#"{"user":{"eids":[{"source":"anySource","uids":[{"id":"anyID"}]}]}}"#,
        ),
    ];
    all_ok(
        cases
            .into_iter()
            .map(|(name, given, expected)| (name, clear_case(given, clear202409_fields, expected))),
    )
}
