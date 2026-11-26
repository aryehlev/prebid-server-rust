//! Go: `openrtb_ext/request_wrapper_test.go`, on the wrappers' private state as Go's tests are.
//!
//! Go's rebuild tests give each case a request whose ext owner is nil, has no ext, or has one;
//! [`Owner`] says which. Go's clone tests change the original after cloning and compare the
//! clone with a manual copy of the original; here the copy is a second call of the function that
//! builds the original.

use core::fmt::{Debug, Display};
use std::collections::BTreeMap;

use pbs_errortypes::ErrorKind;
use pbs_gojson::RawJson;
use pbs_openrtb::{adcom1, openrtb2};
use serde_json::value::RawValue;

use super::{
    AppExt, DOOHExt, DeviceExt, ExtMap, ImpExt, ImpLoc, ImpWrapper, ImpWrapperMut, ImpWrapperSlot,
    RegExt, RequestExt, RequestWrapper, SiteExt, SourceExt, Tracked, UserExt, WrappedExt as _,
};
use crate::{
    ConsentedProvidersSettingsIn, ConsentedProvidersSettingsOut, ExtAppPrebid, ExtDeviceInt,
    ExtDevicePrebid, ExtImpData, ExtImpDataAdServer, ExtImpPrebid, ExtImpPrebidFloors, ExtRegsDSA,
    ExtRequestPrebid, ExtRequestPrebidChannel, ExtStoredAuctionResponse, ExtStoredBidResponse,
    ExtStoredRequest, ExtUserPrebid, Options, parse_consented_providers_string,
};

/// A test result whose error is the failure message.
type TestResult = Result<(), String>;

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

/// Fails, showing both values, unless they are equal.
fn ensure_eq<T: PartialEq + Debug + ?Sized>(got: &T, want: &T, what: &str) -> TestResult {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

/// Fails with `what` unless `condition` holds.
fn ensure(condition: bool, what: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(what.to_owned())
    }
}

/// Go's member `x` with `xDirty` false.
const fn clean<T>(value: T) -> Tracked<T> {
    Tracked {
        value,
        dirty: false,
    }
}

/// Go's member `x` with `xDirty` true.
const fn dirty<T>(value: T) -> Tracked<T> {
    Tracked { value, dirty: true }
}

/// Go's `ext = make(map[string]json.RawMessage)`, which the rebuild tests give each object.
const fn made() -> Tracked<ExtMap> {
    clean(ExtMap(Some(BTreeMap::new())))
}

/// `json.RawMessage(json)` in an ORTB field.
fn raw(json: &str) -> RawJson {
    RawJson::from(json)
}

/// Go's `map[string]json.RawMessage{...}` of `members` (key, JSON) in an ORTB field.
fn object(members: &[(&str, &str)]) -> BTreeMap<String, RawJson> {
    members
        .iter()
        .map(|&(key, json)| (key.to_owned(), raw(json)))
        .collect()
}

/// `json.RawMessage(json)` in an ext map (nil when `json` is not JSON, which an ext map cannot
/// hold here: the cases of Go's tests that put bytes such as `X` in one put `"X"`).
fn member(json: &str) -> Option<Box<RawValue>> {
    RawValue::from_string(json.to_owned()).ok()
}

/// An ext map's `map[string]json.RawMessage{...}` of `members` (key, JSON).
fn members(members: &[(&str, &str)]) -> BTreeMap<String, Option<Box<RawValue>>> {
    members
        .iter()
        .map(|&(key, json)| (key.to_owned(), member(json)))
        .collect()
}

/// An ext map as texts, which compare (a `RawValue` does not).
fn texts(map: &BTreeMap<String, Option<Box<RawValue>>>) -> BTreeMap<String, Option<String>> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.as_deref().map(|v| v.get().to_owned())))
        .collect()
}

/// Go's `map[string]string{...}` of `members`.
fn string_map(members: &[(&str, &str)]) -> BTreeMap<String, String> {
    members
        .iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// Go's `m[key] = json.RawMessage(json)` on a map that is not nil.
fn put(ext: &mut ExtMap, key: &str, json: &str) {
    if let Some(map) = ext.0.as_mut() {
        let _previous = map.insert(key.to_owned(), member(json));
    }
}

/// Go's `m[key] = json.RawMessage(json)` on an ORTB field's map that is not nil.
fn put_raw(map: &mut Option<BTreeMap<String, RawJson>>, key: &str, json: &str) {
    if let Some(map) = map.as_mut() {
        let _previous = map.insert(key.to_owned(), raw(json));
    }
}

/// Go's `Nil` clone case: the clone of a nil object is nil (like the original).
fn nil_clone<T: Clone + PartialEq + Debug>() -> TestResult {
    let none: Option<T> = None;
    let clone = none.clone();
    ensure(clone.is_none(), "Nil")?;
    ensure_eq(&clone, &none, "Nil")
}

/// Go's clone test loop: the clone of `given()` still equals `given()` after `mutate` changed
/// the original.
fn clone_case<T: Clone + PartialEq + Debug, M: FnOnce(&mut T)>(
    given: fn() -> T,
    mutate: M,
) -> TestResult {
    let mut original = given();
    let clone = original.clone();
    mutate(&mut original);
    ensure_eq(&clone, &given(), "clone")
}

/// The state of the object that owns an ext in Go's rebuild tests.
#[derive(Clone, Copy, Debug)]
enum Owner {
    /// The object is nil.
    Nil,
    /// The object has no ext.
    NoExt,
    /// The object has this ext.
    Ext(&'static str),
}

/// The owner object in the state `owner`, whose ext `ext` returns.
fn owned<T: Default>(owner: Owner, ext: fn(&mut T) -> &mut RawJson) -> Option<Box<T>> {
    match owner {
        Owner::Nil => None,
        Owner::NoExt => Some(Box::default()),
        Owner::Ext(json) => {
            let mut object = Box::<T>::default();
            *ext(&mut object) = raw(json);
            Some(object)
        }
    }
}

/// A case of Go's rebuild tests: name, given owner, ext object, expected owner.
type RebuildCase<T> = (&'static str, Owner, T, Owner);

/// Go's rebuild test loop: the wrapper of the request `request` builds for the given owner,
/// with the ext object `install` stores, is rebuilt and compared with the request of the expected
/// owner (Go ignores the rebuild's error, which is nil in all these cases).
fn rebuild_cases<T, const N: usize>(
    cases: [RebuildCase<T>; N],
    request: fn(Owner) -> openrtb2::BidRequest,
    install: fn(&mut RequestWrapper, T),
) -> TestResult {
    all_ok(cases.into_iter().map(|(name, given, ext, expected)| {
        let mut w = RequestWrapper::new(request(given));
        install(&mut w, ext);
        let result = w.rebuild_request().map_err(|err| err.to_string());
        let want = Some(Box::new(request(expected)));
        (
            name,
            result.and_then(|()| ensure_eq(&w.bid_request, &want, "request")),
        )
    }))
}

/// The `imp` of Go's `ImpWrapper{Imp: &imp}` tests, with this ext (nil for `None`).
fn imp_with_ext(ext: Option<&str>) -> openrtb2::Imp {
    openrtb2::Imp {
        ext: ext.map_or_else(RawJson::nil, raw),
        ..openrtb2::Imp::default()
    }
}

/// An impression with this ID.
fn imp_id(id: &str) -> openrtb2::Imp {
    openrtb2::Imp {
        id: id.to_owned(),
        ..openrtb2::Imp::default()
    }
}

/// Go's `&ImpWrapper{Imp: nil, impExt: impExt}` in a request wrapper's `impWrappers`.
fn nil_imp_slot(imp_ext: ImpExt) -> ImpWrapperSlot {
    ImpWrapperSlot {
        imp: ImpLoc::Detached(None),
        imp_ext: Some(Box::new(imp_ext)),
    }
}

/// Go's `reqWrap` of `TestCloneRequestWrapper`.
fn clone_request_wrapper_given() -> RequestWrapper {
    let fun = ImpExt {
        prebid: dirty(Some(Box::new(ExtImpPrebid {
            options: Some(Box::new(Options {
                echo_video_attrs: true,
            })),
            ..ExtImpPrebid::default()
        }))),
        tid: clean("fun".to_owned()),
        ..ImpExt::default()
    };
    let star = ImpExt {
        tid: clean("star".to_owned()),
        ..ImpExt::default()
    };
    let prebid = ExtRequestPrebid {
        integration: "derivative".to_owned(),
        ..ExtRequestPrebid::default()
    };
    RequestWrapper {
        imp_wrappers: Some(vec![nil_imp_slot(fun), nil_imp_slot(star)]),
        user_ext: Some(Box::new(UserExt {
            consent: dirty(None),
            ..UserExt::default()
        })),
        device_ext: Some(Box::new(DeviceExt {
            ext: dirty(ExtMap(None)),
            ..DeviceExt::default()
        })),
        request_ext: Some(Box::new(RequestExt {
            prebid: clean(Some(Box::new(prebid))),
            ..RequestExt::default()
        })),
        app_ext: Some(Box::new(AppExt {
            prebid: dirty(None),
            ..AppExt::default()
        })),
        reg_ext: Some(Box::new(RegExt {
            us_privacy: clean("foo".to_owned()),
            ..RegExt::default()
        })),
        site_ext: Some(Box::new(SiteExt {
            amp: clean(Some(1)),
            ..SiteExt::default()
        })),
        dooh_ext: Some(Box::default()),
        source_ext: Some(Box::new(SourceExt {
            schain: dirty(None),
            ..SourceExt::default()
        })),
        ..RequestWrapper::default()
    }
}

/// Go: `TestCloneRequestWrapper`.
#[test]
fn clone_request_wrapper() -> TestResult {
    nil_clone::<RequestWrapper>()?;
    let general = |w: &mut RequestWrapper| {
        let slots = w.imp_wrappers.get_or_insert_with(Vec::new);
        if let Some(imp_ext) = slots
            .get_mut(1)
            .and_then(|slot| slot.imp_ext.as_deref_mut())
        {
            imp_ext.prebid.dirty = true;
        }
        if let Some(slot) = slots.first_mut() {
            // Go: reqWrap.impWrappers[0] = nil
            *slot = ImpWrapperSlot::default();
        }
        slots.push(nil_imp_slot(ImpExt {
            tid: clean("star".to_owned()),
            ..ImpExt::default()
        }));
        w.imp_wrappers = None;
        w.user_ext = None;
        w.device_ext = None;
        w.request_ext = None;
        w.app_ext = None;
        w.reg_ext = None;
        w.site_ext = None;
        w.dooh_ext = None;
        w.source_ext = None;
    };
    all_ok([
        ("NoMutate", clone_case(clone_request_wrapper_given, |_| {})),
        ("General", clone_case(clone_request_wrapper_given, general)),
    ])
}

/// Go: `TestUserExt`.
#[test]
fn user_ext() -> TestResult {
    let mut user_ext = UserExt::default();

    user_ext.unmarshal(&[]).map_err(|err| err.to_string())?;
    ensure(!user_ext.dirty(), "New UserExt should not be dirty.")?;
    ensure_eq(&user_ext.get_consent(), &None, "consent")?;
    ensure_eq(&user_ext.get_eid(), &None, "eid")?;
    ensure_eq(&user_ext.get_prebid(), &None, "prebid")?;
    ensure_eq(
        &user_ext.get_consented_providers_settings_in(),
        &None,
        "cps in",
    )?;

    user_ext.set_consent(Some("NewConsent".to_owned()));
    ensure_eq(&user_ext.get_consent(), &Some("NewConsent"), "consent")?;

    let eid = openrtb2::EID {
        source: "source".to_owned(),
        uids: Some(vec![openrtb2::UID {
            id: "id".to_owned(),
            ..openrtb2::UID::default()
        }]),
        ..openrtb2::EID::default()
    };
    user_ext.set_eid(Some(Some(vec![eid.clone()])));
    ensure_eq(&user_ext.get_eid(), &Some(Some(&[eid][..])), "eid")?;

    let prebid = ExtUserPrebid {
        buyer_uids: Some(string_map(&[("buyer", "id")])),
    };
    user_ext.set_prebid(Some(prebid.clone()));
    ensure_eq(&user_ext.get_prebid(), &Some(&prebid), "prebid")?;

    let cps_in = |cps: &str| ConsentedProvidersSettingsIn {
        consented_providers_string: cps.to_owned(),
    };
    user_ext.set_consented_providers_settings_in(Some(cps_in("1~X.X.X")));
    let got = user_ext.get_consented_providers_settings_in().cloned();
    ensure_eq(&got, &Some(cps_in("1~X.X.X")), "cps in")?;
    ensure(
        user_ext.dirty(),
        "UserExt should be dirty after field updates",
    )?;

    let consented_providers_string = "1~1.35.41.101";
    let mut updated = got.unwrap_or_default();
    consented_providers_string.clone_into(&mut updated.consented_providers_string);
    user_ext.set_consented_providers_settings_in(Some(updated));
    let got_string = user_ext
        .get_consented_providers_settings_in()
        .map(|cps| cps.consented_providers_string.as_str());
    ensure_eq(&got_string, &Some(consented_providers_string), "cps in")?;

    let cps_out = ConsentedProvidersSettingsOut {
        consented_providers_list: parse_consented_providers_string(consented_providers_string),
    };
    let list_len = cps_out.consented_providers_list.as_ref().map(Vec::len);
    ensure_eq(&list_len, &Some(4), "consentedProvidersList")?;
    user_ext.set_consented_providers_settings_out(Some(cps_out));

    let updated_user_ext = user_ext.marshal().map_err(|err| err.to_string())?;
    let expected = concat!(
        r#"{"ConsentedProvidersSettings":{"consented_providers":"1~1.35.41.101"},"#,
        r#""consent":"NewConsent","#,
        r#""consented_providers_settings":{"consented_providers":[1,35,41,101]},"#,
        r#""eids":[{"source":"source","uids":[{"id":"id"}]}],"#,
        r#""prebid":{"buyeruids":{"buyer":"id"}}}"#,
    );
    let marshalled = updated_user_ext.as_deref().map(RawValue::get);
    ensure_eq(&marshalled, &Some(expected), "marshalled UserExt")?;
    ensure(
        !user_ext.dirty(),
        "UserExt should not be dirty after marshalling",
    )
}

/// A wrapper of `TestRebuildImp`: Go's `&ImpWrapper{Imp: imp, impExt: &ImpExt{...}}`, with a
/// dirty `prebid` when there is one.
fn rebuild_imp_slot(imp: Option<openrtb2::Imp>, prebid: Option<ExtImpPrebid>) -> ImpWrapperSlot {
    let prebid = prebid.map_or_else(Tracked::default, |prebid| dirty(Some(Box::new(prebid))));
    ImpWrapperSlot {
        imp: ImpLoc::Detached(imp.map(Box::new)),
        imp_ext: Some(Box::new(ImpExt {
            ext: made(),
            prebid,
            ..ImpExt::default()
        })),
    }
}

/// A case of `TestRebuildImp`: the wrapper of `given` with `wrappers` (accessed when there are
/// some) is rebuilt into `expected`, or fails with its error.
fn rebuild_imp_case(
    given: Vec<openrtb2::Imp>,
    wrappers: Option<Vec<ImpWrapperSlot>>,
    expected: Result<Vec<openrtb2::Imp>, &str>,
) -> TestResult {
    let accessed = wrappers.is_some();
    let imps = |imps: Vec<openrtb2::Imp>| Some(imps).filter(|imps| !imps.is_empty());
    let mut w = RequestWrapper {
        bid_request: Some(Box::new(openrtb2::BidRequest {
            imp: imps(given),
            ..openrtb2::BidRequest::default()
        })),
        imp_wrappers: wrappers,
        imp_wrappers_accessed: accessed,
        ..RequestWrapper::default()
    };
    let result = w.rebuild_request();
    let expected_imps = match expected {
        Ok(expected_imps) => expected_imps,
        Err(message) => {
            let got = result.err().map(|err| err.to_string());
            return ensure_eq(&got, &Some(message.to_owned()), "error");
        }
    };
    result.map_err(|err| err.to_string())?;
    let want = openrtb2::BidRequest {
        imp: imps(expected_imps),
        ..openrtb2::BidRequest::default()
    };
    ensure_eq(&w.bid_request, &Some(Box::new(want)), "request")?;
    // Go: each wrapper points at the request's impression (assert.Same).
    let synced = w
        .imp_wrappers
        .iter()
        .flatten()
        .all(|slot| slot.imp == ImpLoc::Request);
    ensure(
        !accessed || synced,
        "the wrappers are not the request's impressions",
    )
}

/// Go: `TestRebuildImp`.
#[test]
fn rebuild_imp() -> TestResult {
    let rewarded = || ExtImpPrebid {
        is_rewarded_inventory: Some(1),
        ..ExtImpPrebid::default()
    };
    let prebid_json = r#"{"prebid":{"is_rewarded_inventory":1}}"#;
    let with_ext = |id: &str, ext: &str| openrtb2::Imp {
        ext: raw(ext),
        ..imp_id(id)
    };
    let ad_unit_code = ExtImpPrebid {
        ad_unit_code: "adunitcode".to_owned(),
        ..ExtImpPrebid::default()
    };
    let ad_unit_code_json = r#"{"prebid":{"adunitcode":"adunitcode"}}"#;
    all_ok([
        (
            "Empty - Never Accessed",
            rebuild_imp_case(Vec::new(), None, Ok(Vec::new())),
        ),
        (
            "One - Never Accessed",
            rebuild_imp_case(vec![imp_id("1")], None, Ok(vec![imp_id("1")])),
        ),
        (
            "One - Accessed - Dirty",
            rebuild_imp_case(
                vec![imp_id("1")],
                Some(vec![rebuild_imp_slot(Some(imp_id("2")), Some(rewarded()))]),
                Ok(vec![with_ext("2", prebid_json)]),
            ),
        ),
        (
            "One - Accessed - Dirty - AdUnitCode",
            rebuild_imp_case(
                vec![imp_id("1")],
                Some(vec![rebuild_imp_slot(
                    Some(imp_id("1")),
                    Some(ad_unit_code),
                )]),
                Ok(vec![with_ext("1", ad_unit_code_json)]),
            ),
        ),
        (
            "One - Accessed - Error",
            rebuild_imp_case(
                vec![imp_id("1")],
                Some(vec![rebuild_imp_slot(None, None)]),
                Err("ImpWrapper RebuildImp called on a nil Imp"),
            ),
        ),
        (
            "Many - Accessed - Dirty / Not Dirty",
            rebuild_imp_case(
                vec![imp_id("1"), imp_id("2")],
                Some(vec![
                    rebuild_imp_slot(Some(imp_id("1")), None),
                    rebuild_imp_slot(Some(imp_id("2")), Some(rewarded())),
                ]),
                Ok(vec![imp_id("1"), with_ext("2", prebid_json)]),
            ),
        ),
    ])
}

/// The request of a user ext rebuild case.
fn user_request(owner: Owner) -> openrtb2::BidRequest {
    openrtb2::BidRequest {
        user: owned(owner, |user: &mut openrtb2::User| &mut user.ext),
        ..openrtb2::BidRequest::default()
    }
}

/// Stores a user ext of a rebuild case, with Go's empty map.
fn install_user_ext(w: &mut RequestWrapper, user_ext: UserExt) {
    w.user_ext = Some(Box::new(UserExt {
        ext: made(),
        ..user_ext
    }));
}

/// Go's `UserExt{consent: consent, consentDirty: true}`.
fn dirty_consent(consent: Option<&str>) -> UserExt {
    UserExt {
        consent: dirty(consent.map(str::to_owned)),
        ..UserExt::default()
    }
}

/// Go's `UserExt{consentedProvidersSettingsIn: cps, consentedProvidersSettingsInDirty: true}`;
/// `Some("")` is `&ConsentedProvidersSettingsIn{}`.
fn dirty_cps_in(cps: Option<&str>) -> UserExt {
    UserExt {
        consented_providers_settings_in: dirty(cps.map(|cps| ConsentedProvidersSettingsIn {
            consented_providers_string: cps.to_owned(),
        })),
        ..UserExt::default()
    }
}

/// Go's `UserExt{consentedProvidersSettingsOut: cps, consentedProvidersSettingsOutDirty: true}`.
fn dirty_cps_out(cps: Option<ConsentedProvidersSettingsOut>) -> UserExt {
    UserExt {
        consented_providers_settings_out: dirty(cps),
        ..UserExt::default()
    }
}

/// Go's `&ConsentedProvidersSettingsOut{ConsentedProvidersList: list}`.
fn providers_out(list: &[isize]) -> ConsentedProvidersSettingsOut {
    ConsentedProvidersSettingsOut {
        consented_providers_list: Some(list.to_vec()),
    }
}

/// Go: `TestRebuildUserExt`, the consent string group.
#[test]
fn rebuild_user_ext_consent() -> TestResult {
    let consent_a = Owner::Ext(r#"{"consent":"a"}"#);
    let consent_b = Owner::Ext(r#"{"consent":"b"}"#);
    let cases = [
        // Nil req.User
        (
            "Nil req.User - UserExt Not Dirty",
            Owner::Nil,
            UserExt::default(),
            Owner::Nil,
        ),
        (
            "Nil req.User - Dirty UserExt",
            Owner::Nil,
            dirty_consent(Some("a")),
            consent_a,
        ),
        (
            "Nil req.User - Dirty UserExt but consent is nil - No Change",
            Owner::Nil,
            dirty_consent(None),
            Owner::Nil,
        ),
        // Nil req.User.Ext
        (
            "Nil req.User.Ext - Not Dirty - No Change",
            Owner::NoExt,
            UserExt::default(),
            Owner::NoExt,
        ),
        (
            concat!(
                "Nil req.User.Ext - Dirty with valid consent string - ",
                "Expect consent string to be added",
            ),
            Owner::NoExt,
            dirty_consent(Some("b")),
            consent_b,
        ),
        (
            "Nil req.User.Ext - Dirty UserExt with nil consent string- No Change",
            Owner::NoExt,
            dirty_consent(None),
            Owner::NoExt,
        ),
        // Not-nil req.User.Ext
        (
            "Populated - Not Dirty - No Change",
            consent_a,
            UserExt::default(),
            consent_a,
        ),
        (
            "Populated - Dirty - Consent string overriden",
            consent_a,
            dirty_consent(Some("b")),
            consent_b,
        ),
        (
            "Populated - Dirty but consent string is equal to the one already set - No Change",
            consent_a,
            dirty_consent(Some("a")),
            consent_a,
        ),
        (
            "Populated - Dirty UserExt with nil consent string - Cleared",
            consent_a,
            dirty_consent(None),
            Owner::NoExt,
        ),
    ];
    rebuild_cases(cases, user_request, install_user_ext)
}

/// The user ext with Google's Additional Consent string `X.X.X.X`.
const CPS_STRING_X: &str = r#"{"ConsentedProvidersSettings":{"consented_providers":"1~X.X.X.X"}}"#;

/// Go: `TestRebuildUserExt`, the consented provider settings string group.
#[test]
fn rebuild_user_ext_consented_providers_string() -> TestResult {
    let set = Owner::Ext(
        r#"{"ConsentedProvidersSettings":{"consented_providers":"ConsentedProvidersString"}}"#,
    );
    let other_key = Owner::Ext(r#"{"1~X.X.X.X":{"consented_providers":"1~X.X.X.X"}}"#);
    let overridden =
        Owner::Ext(r#"{"ConsentedProvidersSettings":{"consented_providers":"1~1.35.41.101"}}"#);
    let x = Owner::Ext(CPS_STRING_X);
    let cases = [
        // Nil req.User
        (
            "Nil req.User - Dirty UserExt with nil consentedProviderSettings - No Change",
            Owner::Nil,
            UserExt::default(),
            Owner::Nil,
        ),
        (
            "Nil req.User - Dirty UserExt with empty consentedProviderSettings - No Change",
            Owner::Nil,
            dirty_cps_in(Some("")),
            Owner::Nil,
        ),
        (
            concat!(
                "Nil req.User - Dirty UserExt with populated consentedProviderSettings - ",
                "ConsentedProvidersSettings are added",
            ),
            Owner::Nil,
            dirty_cps_in(Some("ConsentedProvidersString")),
            set,
        ),
        // Nil req.User.Ext
        (
            "Nil req.User.Ext - Dirty UserExt with nil consentedProviderSettings - No Change",
            Owner::NoExt,
            dirty_cps_in(None),
            Owner::NoExt,
        ),
        (
            "Nil req.User.Ext - Dirty UserExt with empty consentedProviderSettings - No Change",
            Owner::NoExt,
            dirty_cps_in(Some("")),
            Owner::NoExt,
        ),
        (
            concat!(
                "Nil req.User.Ext - Dirty UserExt with populated consentedProviderSettings - ",
                "ConsentedProvidersSettings are added",
            ),
            Owner::NoExt,
            dirty_cps_in(Some("ConsentedProvidersString")),
            set,
        ),
        // Not-nil req.User.Ext
        (
            "Populated req.User.Ext - Not Dirty UserExt - No Change",
            x,
            UserExt::default(),
            x,
        ),
        (
            concat!(
                "Populated req.User.Ext - Dirty UserExt with nil consentedProviderSettings - ",
                "Populated req.User.Ext gets overriden with nil User.Ext",
            ),
            x,
            dirty_cps_in(None),
            Owner::NoExt,
        ),
        (
            concat!(
                "Populated req.User.Ext - Dirty UserExt with empty consentedProviderSettings - ",
                "Populated req.User.Ext gets overriden with nil User.Ext",
            ),
            other_key,
            dirty_cps_in(Some("")),
            Owner::NoExt,
        ),
        (
            concat!(
                "Populated req.User.Ext - ",
                "Dirty UserExt with populated consentedProviderSettings - ",
                "ConsentedProvidersSettings are overriden",
            ),
            x,
            dirty_cps_in(Some("1~1.35.41.101")),
            overridden,
        ),
    ];
    rebuild_cases(cases, user_request, install_user_ext)
}

/// Go: `TestRebuildUserExt`, the consented provider settings array group.
#[test]
fn rebuild_user_ext_consented_providers_list() -> TestResult {
    let list =
        Owner::Ext(r#"{"consented_providers_settings":{"consented_providers":[1,35,41,101]}}"#);
    let overridden =
        Owner::Ext(r#"{"consented_providers_settings":{"consented_providers":[35,36,240]}}"#);
    let cases = [
        // Nil req.User
        (
            "Nil req.User - Dirty UserExt with nil consentedProviderSettings - No Change",
            Owner::Nil,
            UserExt::default(),
            Owner::Nil,
        ),
        (
            "Nil req.User - Dirty UserExt with empty consentedProviderSettings - No Change",
            Owner::Nil,
            dirty_cps_out(Some(ConsentedProvidersSettingsOut::default())),
            Owner::Nil,
        ),
        (
            concat!(
                "Nil req.User - Dirty UserExt with populated consentedProviderSettings - ",
                "ConsentedProvidersSettings are added",
            ),
            Owner::Nil,
            dirty_cps_out(Some(providers_out(&[1, 35, 41, 101]))),
            list,
        ),
        // Nil req.User.Ext
        (
            "Nil req.User.Ext - Dirty UserExt with nil consentedProviderSettings - No Change",
            Owner::NoExt,
            dirty_cps_out(None),
            Owner::NoExt,
        ),
        (
            "Nil req.User.Ext - Dirty UserExt with empty consentedProviderSettings - No Change",
            Owner::NoExt,
            dirty_cps_out(Some(ConsentedProvidersSettingsOut::default())),
            Owner::NoExt,
        ),
        (
            concat!(
                "Nil req.User.Ext - Dirty UserExt with populated consentedProviderSettings - ",
                "ConsentedProvidersSettings are added",
            ),
            Owner::NoExt,
            dirty_cps_out(Some(providers_out(&[1, 35, 41, 101]))),
            list,
        ),
        // Not-nil req.User.Ext
        (
            "Populated req.User.Ext - Not Dirty UserExt - No Change",
            list,
            UserExt::default(),
            list,
        ),
        (
            concat!(
                "Populated req.User.Ext - Dirty UserExt with nil consentedProviderSettingsOut - ",
                "Populated req.User.Ext gets overriden with nil User.Ext",
            ),
            list,
            dirty_cps_out(None),
            Owner::NoExt,
        ),
        (
            concat!(
                "Populated req.User.Ext - Dirty UserExt with empty consentedProviderSettingsOut - ",
                "Populated req.User.Ext gets overriden with nil User.Ext",
            ),
            list,
            dirty_cps_out(Some(ConsentedProvidersSettingsOut::default())),
            Owner::NoExt,
        ),
        (
            concat!(
                "Populated req.User.Ext - ",
                "Dirty UserExt with populated consentedProviderSettingsOut - ",
                "consented_providers list elements are overriden",
            ),
            list,
            dirty_cps_out(Some(providers_out(&[35, 36, 240]))),
            overridden,
        ),
    ];
    rebuild_cases(cases, user_request, install_user_ext)
}

/// Go: `TestUserExtUnmarshal`.
#[test]
fn user_ext_unmarshal() -> TestResult {
    let decoded = UserExt {
        ext: clean(ExtMap(Some(members(&[(
            "eids",
            r#"[{"source":"value"}]"#,
        )])))),
        ..UserExt::default()
    };
    let ext_dirty = UserExt {
        ext: dirty(ExtMap(None)),
        ..UserExt::default()
    };
    let prebid = r#"{"prebid":{"buyeruids":{"elem1":"value1"}}}"#;
    let new = UserExt::default;
    let cases = [
        (
            "UserExt.ext is not nil, don't expect error",
            decoded,
            prebid,
            false,
        ),
        (
            "UserExt.ext is dirty, don't expect error",
            ext_dirty,
            prebid,
            false,
        ),
        // Eids
        (
            "Has eids and it is valid JSON",
            new(),
            r#"{"eids":[{"source":"value"}]}"#,
            false,
        ),
        (
            "Has malformed eids expect error",
            new(),
            r#"{"eids":123}"#,
            true,
        ),
        // prebid
        ("Has prebid and it is valid JSON", new(), prebid, false),
        (
            "Has malformed prebid expect error",
            new(),
            r#"{"prebid":{"buyeruids":123}}"#,
            true,
        ),
        // ConsentedProvidersSettings
        (
            "Has ConsentedProvidersSettings and it is valid JSON",
            new(),
            r#"{"ConsentedProvidersSettings":{"consented_providers":"ConsentedProvidersString"}}"#,
            false,
        ),
        (
            "Has malformed ConsentedProvidersSettings expect error",
            new(),
            r#"{"ConsentedProvidersSettings":{"consented_providers":123}}"#,
            true,
        ),
        // consented_providers_settings
        (
            "Has consented_providers_settings and it is valid JSON",
            new(),
            r#"{"consented_providers_settings":{"consented_providers":[2,25]}}"#,
            false,
        ),
        (
            "Has malformed consented_providers_settings expect error",
            new(),
            r#"{"consented_providers_settings":{"consented_providers":123}}"#,
            true,
        ),
    ];
    all_ok(
        cases
            .into_iter()
            .map(|(name, mut user_ext, json, expect_error)| {
                let failed = user_ext.unmarshal(json.as_bytes()).is_err();
                (name, ensure_eq(&failed, &expect_error, "error"))
            }),
    )
}

/// Go's `userExt` of the `NoMutate` and `General` cases of `TestCloneUserExt`.
fn clone_user_ext_general() -> UserExt {
    UserExt {
        ext: clean(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        consent: dirty(Some("Myconsent".to_owned())),
        prebid: dirty(Some(ExtUserPrebid {
            buyer_uids: Some(string_map(&[("A", "X"), ("B", "Y")])),
        })),
        eids: clean(Some(Some(Vec::new()))),
        ..UserExt::default()
    }
}

/// Go's `userExt` of the `EIDs` case of `TestCloneUserExt`.
fn clone_user_ext_eids() -> UserExt {
    let uid = |id: &str, atype: i64, ext: Option<&str>| openrtb2::UID {
        id: id.to_owned(),
        atype: adcom1::AgentType(atype),
        ext: ext.map_or_else(RawJson::nil, raw),
    };
    let eid = |source: &str, uids: Vec<openrtb2::UID>| openrtb2::EID {
        source: source.to_owned(),
        uids: Some(uids),
        ..openrtb2::EID::default()
    };
    UserExt {
        eids: clean(Some(Some(vec![
            eid(
                "Sauce",
                vec![
                    uid("A", 5, Some("{}")),
                    uid("B", 1, Some(r#"{"extra": "stuff"}"#)),
                ],
            ),
            eid("Moon", vec![uid("G", 3, Some("{}")), uid("D", 1, None)]),
        ]))),
        ..UserExt::default()
    }
}

/// Go's `userExt` of the `ConsentedProviders` case of `TestCloneUserExt`.
fn clone_user_ext_consented_providers() -> UserExt {
    UserExt {
        consented_providers_settings_in: clean(Some(ConsentedProvidersSettingsIn {
            consented_providers_string: "A,B,C".to_owned(),
        })),
        consented_providers_settings_out: clean(Some(ConsentedProvidersSettingsOut {
            consented_providers_list: Some(vec![1, 2, 3, 4]),
        })),
        ..UserExt::default()
    }
}

/// The `uids` of an EID that may be absent.
fn uids_of(eid: Option<&mut openrtb2::EID>) -> Option<&mut Vec<openrtb2::UID>> {
    eid.and_then(|eid| eid.uids.as_mut())
}

/// Go: `TestCloneUserExt`.
#[test]
fn clone_user_ext() -> TestResult {
    nil_clone::<UserExt>()?;
    let general = |user: &mut UserExt| {
        put(&mut user.ext.value, "A", r#""G""#);
        put(&mut user.ext.value, "C", r#""L""#);
        user.ext.dirty = true;
        user.consent = clean(None);
        if let Some(uids) = user
            .prebid
            .value
            .as_mut()
            .and_then(|p| p.buyer_uids.as_mut())
        {
            let _previous = uids.insert("A".to_owned(), "C".to_owned());
            let _added = uids.insert("C".to_owned(), "A".to_owned());
        }
        user.prebid.value = None;
    };
    let eids = |user: &mut UserExt| {
        if let Some(eids) = user.eids.value.as_mut().and_then(Option::as_mut) {
            if let Some(uid) = uids_of(eids.first_mut()).and_then(|uids| uids.get_mut(1)) {
                "G2".clone_into(&mut uid.id);
            }
            if let Some(uid) = uids_of(eids.get_mut(1)).and_then(|uids| uids.first_mut()) {
                uid.atype = adcom1::AgentType(0);
            }
            if let Some(first) = uids_of(eids.first_mut()) {
                first.push(openrtb2::UID {
                    id: "Z".to_owned(),
                    atype: adcom1::AgentType(2),
                    ..openrtb2::UID::default()
                });
            }
            eids.push(openrtb2::EID {
                source: "Blank".to_owned(),
                ..openrtb2::EID::default()
            });
        }
        user.eids.value = None;
    };
    let consented_providers = |user: &mut UserExt| {
        let cps_in = &mut user.consented_providers_settings_in.value;
        if let Some(cps) = cps_in.as_mut() {
            "B,C,D".clone_into(&mut cps.consented_providers_string);
        }
        *cps_in = Some(ConsentedProvidersSettingsIn {
            consented_providers_string: "G,H,I".to_owned(),
        });
        let cps_out = &mut user.consented_providers_settings_out.value;
        if let Some(list) = cps_out
            .as_mut()
            .and_then(|cps| cps.consented_providers_list.as_mut())
        {
            if let Some(second) = list.get_mut(1) {
                *second = 5;
            }
            list.push(7);
        }
        *cps_out = None;
    };
    all_ok([
        ("NoMutate", clone_case(clone_user_ext_general, |_| {})),
        ("General", clone_case(clone_user_ext_general, general)),
        ("EIDs", clone_case(clone_user_ext_eids, eids)),
        (
            "ConsentedProviders",
            clone_case(clone_user_ext_consented_providers, consented_providers),
        ),
    ])
}

/// The device ext of `TestRebuildDeviceExt` with `cdep` 1.
const DEVICE_EXT_1: &str =
    r#"{"cdep":"1","prebid":{"interstitial":{"minwidthperc":1,"minheightperc":0}}}"#;
/// The device ext of `TestRebuildDeviceExt` with `cdep` 2.
const DEVICE_EXT_2: &str =
    r#"{"cdep":"2","prebid":{"interstitial":{"minwidthperc":2,"minheightperc":0}}}"#;

/// Go's `DeviceExt{prebid: prebid, prebidDirty: true, cdep: cdep, cdepDirty: true}`, `prebid`
/// being `&ExtDevicePrebid{Interstitial: &ExtDeviceInt{MinWidthPerc: width}}`.
fn dirty_device(width: Option<i64>, cdep: &str) -> DeviceExt {
    let prebid = width.map(|min_width_perc| ExtDevicePrebid {
        interstitial: Some(Box::new(ExtDeviceInt {
            min_width_perc,
            min_height_perc: 0,
        })),
    });
    DeviceExt {
        prebid: dirty(prebid),
        cdep: dirty(cdep.to_owned()),
        ..DeviceExt::default()
    }
}

/// Go: `TestRebuildDeviceExt`.
#[test]
fn rebuild_device_ext() -> TestResult {
    let device_1 = Owner::Ext(DEVICE_EXT_1);
    let device_2 = Owner::Ext(DEVICE_EXT_2);
    let cases = [
        (
            "Nil - Not Dirty",
            Owner::Nil,
            DeviceExt::default(),
            Owner::Nil,
        ),
        (
            "Nil - Dirty",
            Owner::Nil,
            dirty_device(Some(1), "1"),
            device_1,
        ),
        (
            "Nil - Dirty - No Change",
            Owner::Nil,
            dirty_device(None, ""),
            Owner::Nil,
        ),
        (
            "Empty - Not Dirty",
            Owner::NoExt,
            DeviceExt::default(),
            Owner::NoExt,
        ),
        (
            "Empty - Dirty",
            Owner::NoExt,
            dirty_device(Some(1), "1"),
            device_1,
        ),
        (
            "Empty - Dirty - No Change",
            Owner::NoExt,
            dirty_device(None, ""),
            Owner::NoExt,
        ),
        (
            "Populated - Not Dirty",
            device_1,
            DeviceExt::default(),
            device_1,
        ),
        (
            "Populated - Dirty",
            device_1,
            dirty_device(Some(2), "2"),
            device_2,
        ),
        (
            "Populated - Dirty - No Change",
            device_1,
            dirty_device(Some(1), "1"),
            device_1,
        ),
        (
            "Populated - Dirty - Cleared",
            device_1,
            dirty_device(None, ""),
            Owner::NoExt,
        ),
    ];
    let request = |owner| openrtb2::BidRequest {
        device: owned(owner, |device: &mut openrtb2::Device| &mut device.ext),
        ..openrtb2::BidRequest::default()
    };
    rebuild_cases(cases, request, |w, device_ext| {
        w.device_ext = Some(Box::new(DeviceExt {
            ext: made(),
            ..device_ext
        }));
    })
}

/// Go's `RequestExt{prebid: prebid, prebidDirty: true}`, `prebid` being
/// `&ExtRequestPrebid{Integration: integration}`.
fn dirty_request_prebid(integration: Option<&str>) -> RequestExt {
    RequestExt {
        prebid: dirty(integration.map(|integration| {
            Box::new(ExtRequestPrebid {
                integration: integration.to_owned(),
                ..ExtRequestPrebid::default()
            })
        })),
        ..RequestExt::default()
    }
}

/// Go: `TestRebuildRequestExt`.
#[test]
fn rebuild_request_ext() -> TestResult {
    let prebid_1 = Owner::Ext(r#"{"prebid":{"integration":"1"}}"#);
    let prebid_2 = Owner::Ext(r#"{"prebid":{"integration":"2"}}"#);
    let cases = [
        (
            "Empty - Not Dirty",
            Owner::Nil,
            RequestExt::default(),
            Owner::Nil,
        ),
        (
            "Empty - Dirty",
            Owner::Nil,
            dirty_request_prebid(Some("1")),
            prebid_1,
        ),
        (
            "Empty - Dirty - No Change",
            Owner::Nil,
            dirty_request_prebid(None),
            Owner::Nil,
        ),
        (
            "Populated - Not Dirty",
            prebid_1,
            RequestExt::default(),
            prebid_1,
        ),
        (
            "Populated - Dirty",
            prebid_1,
            dirty_request_prebid(Some("2")),
            prebid_2,
        ),
        (
            "Populated - Dirty - No Change",
            prebid_1,
            dirty_request_prebid(Some("1")),
            prebid_1,
        ),
        (
            "Populated - Dirty - Cleared",
            prebid_1,
            dirty_request_prebid(None),
            Owner::Nil,
        ),
    ];
    // The request is the owner: `Nil` and `NoExt` are a request without ext.
    let request = |owner| match owner {
        Owner::Nil | Owner::NoExt => openrtb2::BidRequest::default(),
        Owner::Ext(json) => openrtb2::BidRequest {
            ext: raw(json),
            ..openrtb2::BidRequest::default()
        },
    };
    rebuild_cases(cases, request, |w, request_ext| {
        w.request_ext = Some(Box::new(RequestExt {
            ext: made(),
            ..request_ext
        }));
    })
}

/// Go's `reqExt` of the `NoMutate` and `General` cases of `TestCloneRequestExt`.
fn clone_request_ext_general() -> RequestExt {
    RequestExt {
        ext: dirty(ExtMap(Some(members(&[
            ("A", "{}"),
            ("B", r#"{"foo":"bar"}"#),
        ])))),
        prebid: clean(Some(Box::new(ExtRequestPrebid {
            bidder_params: raw("{}"),
            ..ExtRequestPrebid::default()
        }))),
        ..RequestExt::default()
    }
}

/// The supply chain of the `SChain` cases of `TestCloneRequestExt` and `TestCloneSourceExt`.
fn clone_supply_chain_given() -> openrtb2::SupplyChain {
    let node = |asi: &str, rid: &str, hp: i8| openrtb2::SupplyChainNode {
        asi: asi.to_owned(),
        rid: rid.to_owned(),
        hp: Some(hp),
        ..openrtb2::SupplyChainNode::default()
    };
    openrtb2::SupplyChain {
        complete: 1,
        ver: "1.1".to_owned(),
        nodes: Some(vec![node("Is a", "off", 1), node("other", "drift", 0)]),
        ..openrtb2::SupplyChain::default()
    }
}

/// The changes of the supply chain mutators of `TestCloneRequestExt` and `TestCloneSourceExt`.
fn mutate_supply_chain(schain: &mut Option<openrtb2::SupplyChain>) {
    if let Some(chain) = schain.as_mut() {
        chain.complete = 0;
        "1.2".clone_into(&mut chain.ver);
        let nodes = chain.nodes.get_or_insert_with(Vec::new);
        if let Some(node) = nodes.first_mut() {
            "some".clone_into(&mut node.asi);
        }
        if let Some(node) = nodes.get_mut(1) {
            node.hp = None;
        }
        nodes.push(openrtb2::SupplyChainNode {
            asi: "added".to_owned(),
            ..openrtb2::SupplyChainNode::default()
        });
    }
    *schain = None;
}

/// Go: `TestCloneRequestExt`.
#[test]
fn clone_request_ext() -> TestResult {
    nil_clone::<RequestExt>()?;
    let general = |req_ext: &mut RequestExt| {
        put(&mut req_ext.ext.value, "A", r#""string""#);
        put(&mut req_ext.ext.value, "C", "{}");
        req_ext.ext.dirty = false;
        if let Some(prebid) = req_ext.prebid.value.as_deref_mut() {
            prebid.channel = Some(Box::new(ExtRequestPrebidChannel {
                name: "Bob".to_owned(),
                ..ExtRequestPrebidChannel::default()
            }));
            prebid.bidder_params = RawJson::nil();
        }
        req_ext.prebid.value = None;
    };
    let schain = || RequestExt {
        schain: clean(Some(clone_supply_chain_given())),
        ..RequestExt::default()
    };
    all_ok([
        ("NoMutate", clone_case(clone_request_ext_general, |_| {})),
        ("General", clone_case(clone_request_ext_general, general)),
        (
            "SChain",
            clone_case(schain, |req_ext| {
                mutate_supply_chain(&mut req_ext.schain.value);
            }),
        ),
    ])
}

/// Go's `devExt` of `TestCloneDeviceExt`.
fn clone_device_ext_given() -> DeviceExt {
    DeviceExt {
        ext: dirty(ExtMap(Some(members(&[
            ("A", "{}"),
            ("B", r#"{"foo":"bar"}"#),
        ])))),
        prebid: clean(Some(ExtDevicePrebid {
            interstitial: Some(Box::new(ExtDeviceInt {
                min_width_perc: 65,
                min_height_perc: 75,
            })),
        })),
        cdep: dirty("1".to_owned()),
    }
}

/// Go: `TestCloneDeviceExt`.
#[test]
fn clone_device_ext() -> TestResult {
    nil_clone::<DeviceExt>()?;
    let general = |dev_ext: &mut DeviceExt| {
        put(&mut dev_ext.ext.value, "A", r#""string""#);
        put(&mut dev_ext.ext.value, "C", "{}");
        dev_ext.ext.dirty = false;
        if let Some(prebid) = dev_ext.prebid.value.as_mut() {
            if let Some(interstitial) = prebid.interstitial.as_deref_mut() {
                interstitial.min_height_perc = 55;
            }
            prebid.interstitial = Some(Box::new(ExtDeviceInt {
                min_width_perc: 80,
                min_height_perc: 0,
            }));
        }
        dev_ext.prebid.value = None;
        dev_ext.cdep = dirty(String::new());
    };
    all_ok([
        ("NoMutate", clone_case(clone_device_ext_given, |_| {})),
        ("General", clone_case(clone_device_ext_given, general)),
    ])
}

/// Go's `AppExt{prebid: prebid, prebidDirty: true}`, `prebid` being
/// `&ExtAppPrebid{Source: source}`.
fn dirty_app_prebid(source: Option<&str>) -> AppExt {
    AppExt {
        prebid: dirty(source.map(|source| ExtAppPrebid {
            source: source.to_owned(),
            ..ExtAppPrebid::default()
        })),
        ..AppExt::default()
    }
}

/// Go: `TestRebuildAppExt`.
#[test]
fn rebuild_app_ext() -> TestResult {
    let source_1 = Owner::Ext(r#"{"prebid":{"source":"1"}}"#);
    let source_2 = Owner::Ext(r#"{"prebid":{"source":"2"}}"#);
    let cases = [
        ("Nil - Not Dirty", Owner::Nil, AppExt::default(), Owner::Nil),
        (
            "Nil - Dirty",
            Owner::Nil,
            dirty_app_prebid(Some("1")),
            source_1,
        ),
        (
            "Nil - Dirty - No Change",
            Owner::Nil,
            dirty_app_prebid(None),
            Owner::Nil,
        ),
        (
            "Empty - Not Dirty",
            Owner::NoExt,
            AppExt::default(),
            Owner::NoExt,
        ),
        (
            "Empty - Dirty",
            Owner::NoExt,
            dirty_app_prebid(Some("1")),
            source_1,
        ),
        (
            "Empty - Dirty - No Change",
            Owner::NoExt,
            dirty_app_prebid(None),
            Owner::NoExt,
        ),
        (
            "Populated - Not Dirty",
            source_1,
            AppExt::default(),
            source_1,
        ),
        (
            "Populated - Dirty",
            source_1,
            dirty_app_prebid(Some("2")),
            source_2,
        ),
        (
            "Populated - Dirty - No Change",
            source_1,
            dirty_app_prebid(Some("1")),
            source_1,
        ),
        (
            "Populated - Dirty - Cleared",
            source_1,
            dirty_app_prebid(None),
            Owner::NoExt,
        ),
    ];
    let request = |owner| openrtb2::BidRequest {
        app: owned(owner, |app: &mut openrtb2::App| &mut app.ext),
        ..openrtb2::BidRequest::default()
    };
    rebuild_cases(cases, request, |w, app_ext| {
        w.app_ext = Some(Box::new(AppExt {
            ext: made(),
            ..app_ext
        }));
    })
}

/// Go's `appExt` of `TestCloneAppExt`.
fn clone_app_ext_given() -> AppExt {
    AppExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        prebid: clean(Some(ExtAppPrebid {
            source: "Sauce".to_owned(),
            version: "2.2".to_owned(),
        })),
    }
}

/// Go: `TestCloneAppExt`.
#[test]
fn clone_app_ext() -> TestResult {
    nil_clone::<AppExt>()?;
    let general = |app_ext: &mut AppExt| {
        put(&mut app_ext.ext.value, "A", r#""string""#);
        put(&mut app_ext.ext.value, "C", "{}");
        app_ext.ext.dirty = false;
        if let Some(prebid) = app_ext.prebid.value.as_mut() {
            "foobar".clone_into(&mut prebid.source);
        }
        app_ext.prebid.value = None;
    };
    all_ok([
        ("NoMutate", clone_case(clone_app_ext_given, |_| {})),
        ("General", clone_case(clone_app_ext_given, general)),
    ])
}

/// Go: `TestRebuildDOOHExt` (`DOOHExt` has no members of its own, so it is never dirty).
#[test]
fn rebuild_dooh_ext() -> TestResult {
    let empty = Owner::Ext("{}");
    let new = DOOHExt::default;
    let cases = [
        ("Nil - Not Dirty", Owner::Nil, new(), Owner::Nil),
        ("Nil - Dirty", Owner::Nil, new(), Owner::Nil),
        ("Nil - Dirty - No Change", Owner::Nil, new(), Owner::Nil),
        ("Empty - Not Dirty", Owner::NoExt, new(), Owner::NoExt),
        ("Empty - Dirty", Owner::NoExt, new(), Owner::NoExt),
        (
            "Empty - Dirty - No Change",
            Owner::NoExt,
            new(),
            Owner::NoExt,
        ),
        ("Populated - Not Dirty", empty, new(), empty),
        ("Populated - Dirty", empty, new(), empty),
        ("Populated - Dirty - No Change", empty, new(), empty),
        ("Populated - Dirty - Cleared", empty, new(), empty),
    ];
    let request = |owner| openrtb2::BidRequest {
        dooh: owned(owner, |dooh: &mut openrtb2::DOOH| &mut dooh.ext),
        ..openrtb2::BidRequest::default()
    };
    // A `DOOHExt` is only its map, which Go's loop replaces.
    rebuild_cases(cases, request, |w, _: DOOHExt| {
        w.dooh_ext = Some(Box::new(DOOHExt { ext: made() }));
    })
}

/// Go's `DOOHExt` of `TestCloneDOOHExt`.
fn clone_dooh_ext_given() -> DOOHExt {
    DOOHExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
    }
}

/// Go: `TestCloneDOOHExt`.
#[test]
fn clone_dooh_ext() -> TestResult {
    nil_clone::<DOOHExt>()?;
    let general = |dooh_ext: &mut DOOHExt| {
        put(&mut dooh_ext.ext.value, "A", r#""string""#);
        put(&mut dooh_ext.ext.value, "C", "{}");
        dooh_ext.ext.dirty = false;
    };
    all_ok([
        ("NoMutate", clone_case(clone_dooh_ext_given, |_| {})),
        ("General", clone_case(clone_dooh_ext_given, general)),
    ])
}

/// Go's `regExt` of `TestCloneRegExt`.
fn clone_reg_ext_given() -> RegExt {
    RegExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        gdpr: clean(Some(1)),
        us_privacy: dirty("priv".to_owned()),
        ..RegExt::default()
    }
}

/// Go: `TestCloneRegExt`.
#[test]
fn clone_reg_ext() -> TestResult {
    nil_clone::<RegExt>()?;
    let general = |reg_ext: &mut RegExt| {
        put(&mut reg_ext.ext.value, "A", r#""string""#);
        put(&mut reg_ext.ext.value, "C", "{}");
        reg_ext.ext.dirty = false;
        reg_ext.gdpr = dirty(None);
        "Other".clone_into(&mut reg_ext.us_privacy.value);
    };
    all_ok([
        ("NoMutate", clone_case(clone_reg_ext_given, |_| {})),
        ("General", clone_case(clone_reg_ext_given, general)),
    ])
}

/// Go's `SiteExt{amp: amp, ampDirty: true}`.
fn dirty_amp(amp: Option<i8>) -> SiteExt {
    SiteExt {
        amp: dirty(amp),
        ..SiteExt::default()
    }
}

/// Go: `TestRebuildSiteExt`.
#[test]
fn rebuild_site_ext() -> TestResult {
    let amp_1 = Owner::Ext(r#"{"amp":1}"#);
    let amp_2 = Owner::Ext(r#"{"amp":2}"#);
    let cases = [
        (
            "Nil - Not Dirty",
            Owner::Nil,
            SiteExt::default(),
            Owner::Nil,
        ),
        ("Nil - Dirty", Owner::Nil, dirty_amp(Some(1)), amp_1),
        (
            "Nil - Dirty - No Change",
            Owner::Nil,
            dirty_amp(None),
            Owner::Nil,
        ),
        (
            "Empty - Not Dirty",
            Owner::NoExt,
            SiteExt::default(),
            Owner::NoExt,
        ),
        ("Empty - Dirty", Owner::NoExt, dirty_amp(Some(1)), amp_1),
        (
            "Empty - Dirty - No Change",
            Owner::NoExt,
            dirty_amp(None),
            Owner::NoExt,
        ),
        ("Populated - Not Dirty", amp_1, SiteExt::default(), amp_1),
        ("Populated - Dirty", amp_1, dirty_amp(Some(2)), amp_2),
        (
            "Populated - Dirty - No Change",
            amp_1,
            dirty_amp(Some(1)),
            amp_1,
        ),
        (
            "Populated - Dirty - Cleared",
            amp_1,
            dirty_amp(None),
            Owner::NoExt,
        ),
    ];
    let request = |owner| openrtb2::BidRequest {
        site: owned(owner, |site: &mut openrtb2::Site| &mut site.ext),
        ..openrtb2::BidRequest::default()
    };
    rebuild_cases(cases, request, |w, site_ext| {
        w.site_ext = Some(Box::new(SiteExt {
            ext: made(),
            ..site_ext
        }));
    })
}

/// Go's `siteExt` of `TestCloneSiteExt`.
fn clone_site_ext_given() -> SiteExt {
    SiteExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        amp: clean(Some(1)),
    }
}

/// Go: `TestCloneSiteExt`.
#[test]
fn clone_site_ext() -> TestResult {
    nil_clone::<SiteExt>()?;
    let general = |site_ext: &mut SiteExt| {
        put(&mut site_ext.ext.value, "A", r#""string""#);
        put(&mut site_ext.ext.value, "C", "{}");
        site_ext.ext.dirty = false;
        site_ext.amp = dirty(None);
    };
    all_ok([
        ("NoMutate", clone_case(clone_site_ext_given, |_| {})),
        ("General", clone_case(clone_site_ext_given, general)),
    ])
}

/// Go's `SourceExt{schain: schain, schainDirty: true}`, `schain` being
/// `&openrtb2.SupplyChain{Ver: ver}`.
fn dirty_schain(ver: Option<&str>) -> SourceExt {
    SourceExt {
        schain: dirty(ver.map(|ver| openrtb2::SupplyChain {
            ver: ver.to_owned(),
            ..openrtb2::SupplyChain::default()
        })),
        ..SourceExt::default()
    }
}

/// Go: `TestRebuildSourceExt`.
#[test]
fn rebuild_source_ext() -> TestResult {
    let ver_1 = Owner::Ext(r#"{"schain":{"complete":0,"nodes":null,"ver":"1"}}"#);
    let ver_2 = Owner::Ext(r#"{"schain":{"complete":0,"nodes":null,"ver":"2"}}"#);
    let cases = [
        (
            "Nil - Not Dirty",
            Owner::Nil,
            SourceExt::default(),
            Owner::Nil,
        ),
        ("Nil - Dirty", Owner::Nil, dirty_schain(Some("1")), ver_1),
        (
            "Nil - Dirty - No Change",
            Owner::Nil,
            dirty_schain(None),
            Owner::Nil,
        ),
        (
            "Empty - Not Dirty",
            Owner::NoExt,
            SourceExt::default(),
            Owner::NoExt,
        ),
        (
            "Empty - Dirty",
            Owner::NoExt,
            dirty_schain(Some("1")),
            ver_1,
        ),
        (
            "Empty - Dirty - No Change",
            Owner::NoExt,
            dirty_schain(None),
            Owner::NoExt,
        ),
        ("Populated - Not Dirty", ver_1, SourceExt::default(), ver_1),
        ("Populated - Dirty", ver_1, dirty_schain(Some("2")), ver_2),
        (
            "Populated - Dirty - No Change",
            ver_1,
            dirty_schain(Some("1")),
            ver_1,
        ),
        (
            "Populated - Dirty - Cleared",
            ver_1,
            dirty_schain(None),
            Owner::NoExt,
        ),
    ];
    let request = |owner| openrtb2::BidRequest {
        source: owned(owner, |source: &mut openrtb2::Source| &mut source.ext),
        ..openrtb2::BidRequest::default()
    };
    rebuild_cases(cases, request, |w, source_ext| {
        w.source_ext = Some(Box::new(SourceExt {
            ext: made(),
            ..source_ext
        }));
    })
}

/// Go's `sourceExt` of `TestCloneSourceExt`.
fn clone_source_ext_given() -> SourceExt {
    SourceExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        schain: clean(Some(clone_supply_chain_given())),
    }
}

/// Go: `TestCloneSourceExt`.
#[test]
fn clone_source_ext() -> TestResult {
    nil_clone::<SourceExt>()?;
    let no_mutate = |source_ext: &mut SourceExt| {
        put(&mut source_ext.ext.value, "A", r#""string""#);
        put(&mut source_ext.ext.value, "C", "{}");
        source_ext.ext.dirty = false;
        mutate_supply_chain(&mut source_ext.schain.value);
    };
    // Go's `NoMutate` case has a mutator.
    clone_case(clone_source_ext_given, no_mutate).map_err(|err| format!("NoMutate: {err}"))
}

/// Go's `ImpExt{prebid: prebid, prebidDirty: true}`, `prebid` being
/// `&ExtImpPrebid{IsRewardedInventory: rewarded}`.
fn dirty_rewarded(rewarded: Option<i8>) -> ImpExt {
    ImpExt {
        prebid: dirty(Some(Box::new(ExtImpPrebid {
            is_rewarded_inventory: rewarded,
            ..ExtImpPrebid::default()
        }))),
        ..ImpExt::default()
    }
}

/// Go's `ImpExt{tid: tid, tidDirty: true}`.
fn dirty_tid(tid: &str) -> ImpExt {
    ImpExt {
        tid: dirty(tid.to_owned()),
        ..ImpExt::default()
    }
}

/// Go: `TestImpWrapperRebuildImp`.
#[test]
fn imp_wrapper_rebuild_imp() -> TestResult {
    let rewarded_1 = Some(r#"{"prebid":{"is_rewarded_inventory":1}}"#);
    let rewarded_2 = Some(r#"{"prebid":{"is_rewarded_inventory":2}}"#);
    let nil_prebid = ImpExt {
        prebid: dirty(None),
        ..ImpExt::default()
    };
    let cases = [
        ("Empty - Not Dirty", None, ImpExt::default(), None),
        ("Empty - Dirty", None, dirty_rewarded(Some(1)), rewarded_1),
        ("Empty - Dirty - No Change", None, nil_prebid.clone(), None),
        (
            "Populated - Not Dirty",
            rewarded_1,
            ImpExt::default(),
            rewarded_1,
        ),
        (
            "Populated - Dirty",
            rewarded_1,
            dirty_rewarded(Some(2)),
            rewarded_2,
        ),
        (
            "Populated - Dirty - No Change",
            rewarded_1,
            dirty_rewarded(Some(1)),
            rewarded_1,
        ),
        ("Populated - Dirty - Cleared", rewarded_1, nil_prebid, None),
        (
            "Populated - Dirty - Empty Prebid Object",
            rewarded_1,
            dirty_rewarded(None),
            None,
        ),
        (
            "Populated Tid - Dirty",
            Some(r#"{"tid": "some-tid"}"#),
            dirty_tid("12345"),
            Some(r#"{"tid":"12345"}"#),
        ),
        (
            "Populated Tid - Dirty - No Change",
            Some(r#"{"tid": "some-tid"}"#),
            dirty_tid("some-tid"),
            Some(r#"{"tid":"some-tid"}"#),
        ),
        (
            "Populated Tid - Dirty - Cleared",
            Some(r#"{"tid":"some-tid"}"#),
            dirty_tid(""),
            None,
        ),
    ];
    all_ok(cases.into_iter().map(|(name, given, imp_ext, expected)| {
        let mut w = ImpWrapper {
            imp: Some(Box::new(imp_with_ext(given))),
            imp_ext: Some(Box::new(ImpExt {
                ext: made(),
                ..imp_ext
            })),
        };
        let result = w.rebuild_imp().map_err(|err| err.to_string());
        let want = Some(Box::new(imp_with_ext(expected)));
        (name, result.and_then(|()| ensure_eq(&w.imp, &want, "imp")))
    }))
}

/// Go: `TestImpWrapperGetImpExt`.
#[test]
fn imp_wrapper_get_imp_ext() -> TestResult {
    let populated = concat!(
        r#"{"prebid":{"is_rewarded_inventory":1},"other":42,"tid":"test-tid","gpid":"test-gpid","#,
        r#""data":{"adserver":{"name":"ads","adslot":"adslot123"},"pbadslot":"pbadslot123"}}"#,
    );
    let populated_ext = ImpExt {
        ext: clean(ExtMap(Some(members(&[
            ("prebid", r#"{"is_rewarded_inventory":1}"#),
            ("other", "42"),
            ("tid", r#""test-tid""#),
            ("gpid", r#""test-gpid""#),
            (
                "data",
                r#"{"adserver":{"name":"ads","adslot":"adslot123"},"pbadslot":"pbadslot123"}"#,
            ),
        ])))),
        prebid: clean(Some(Box::new(ExtImpPrebid {
            is_rewarded_inventory: Some(1),
            ..ExtImpPrebid::default()
        }))),
        data: Some(ExtImpData {
            pb_adslot: "pbadslot123".to_owned(),
            ad_server: Some(Box::new(ExtImpDataAdServer {
                name: "ads".to_owned(),
                ad_slot: "adslot123".to_owned(),
            })),
        }),
        tid: clean("test-tid".to_owned()),
        gp_id: "test-gpid".to_owned(),
    };
    let retrieved = || ImpExt {
        ext: clean(ExtMap(Some(members(&[("foo", r#""bar""#)])))),
        ..ImpExt::default()
    };
    let ok_cases = [
        (
            "Empty",
            ImpWrapper::default(),
            ImpExt {
                ext: made(),
                ..ImpExt::default()
            },
        ),
        (
            "Populated - Ext",
            ImpWrapper::new(imp_with_ext(Some(populated))),
            populated_ext,
        ),
        (
            "Already Retrieved - Dirty - Not Unmarshalled Again",
            ImpWrapper {
                imp: Some(Box::new(imp_with_ext(Some(r#"{"will":"be ignored"}"#)))),
                imp_ext: Some(Box::new(retrieved())),
            },
            retrieved(),
        ),
    ];
    let ok = ok_cases.into_iter().map(|(name, mut wrapper, expected)| {
        let got = wrapper.get_imp_ext().map(|imp_ext| imp_ext.clone());
        (name, ensure_eq(&got, &Ok(expected), "imp ext"))
    });
    let error_cases = [
        ("Error - Ext", "malformed"),
        ("Error - Ext - Prebid", r#"{"prebid":malformed}"#),
    ];
    let errors = error_cases.into_iter().map(|(name, ext)| {
        let mut wrapper = ImpWrapper::new(imp_with_ext(Some(ext)));
        let kind = wrapper.get_imp_ext().err().map(|err| err.kind().clone());
        (
            name,
            ensure_eq(&kind, &Some(ErrorKind::FailedToUnmarshal), "error type"),
        )
    });
    all_ok(ok.chain(errors))
}

/// An impression with this ID and tag ID.
fn tagged(id: &str, tag_id: &str) -> openrtb2::Imp {
    openrtb2::Imp {
        tagid: tag_id.to_owned(),
        ..imp_id(id)
    }
}

/// Go: `TestImpWrapperSetImp`: wrappers changed, replaced and added after `GetImp` become the
/// request's impressions, and the request's wrappers point at them.
#[test]
fn imp_wrapper_set_imp() -> TestResult {
    let orig_imps = vec![
        tagged("imp1", "tag1"),
        tagged("imp2", "tag2"),
        tagged("imp3", "tag3"),
    ];
    let expected_imps = vec![
        openrtb2::Imp {
            bidfloor: 0.5,
            ..tagged("imp1", "tag4")
        },
        openrtb2::Imp {
            bidfloor: 0.6,
            ..tagged("imp1.1", "tag2")
        },
        tagged("imp2", "notag"),
        tagged("imp3", "tag3"),
    ];
    let mut rw = RequestWrapper::new(openrtb2::BidRequest {
        imp: Some(orig_imps),
        ..openrtb2::BidRequest::default()
    });
    let mut views = rw.get_imp();
    // Go: rw.Imp[0].TagID = "tag4"; rw.Imp[0].BidFloor = 0.5 (iw[0] points at it).
    if let Some(imp) = views.first_mut().and_then(ImpWrapperMut::imp_mut) {
        "tag4".clone_into(&mut imp.tagid);
        imp.bidfloor = 0.5;
    }
    let mut iw: Vec<ImpWrapper> = views.iter().map(ImpWrapperMut::to_imp_wrapper).collect();
    let mut replacements = expected_imps.iter().skip(1).cloned().map(ImpWrapper::new);
    for wrapper in iw.iter_mut().skip(1) {
        // Go: iw[1] = &ImpWrapper{Imp: &expectedImps[1]}; *iw[2] = ImpWrapper{Imp: ...}
        *wrapper = replacements.next().unwrap_or_default();
    }
    // Go: iw = append(iw, &ImpWrapper{Imp: &expectedImps[3]})
    iw.extend(replacements);

    rw.set_imp(Some(iw));
    let got = rw.bid_request().and_then(|req| req.imp.clone());
    ensure_eq(&got, &Some(expected_imps), "imps")?;
    let synced_views = rw.get_imp().len();
    let slots = rw.imp_wrappers.as_deref().unwrap_or_default();
    ensure_eq(&synced_views, &4, "wrappers")?;
    // Go: assert.Same(&rw.Imp[i], iw[i].Imp)
    ensure(
        slots.iter().all(|slot| slot.imp == ImpLoc::Request),
        "the wrappers are not the request's impressions",
    )
}

/// Go: `TestImpExtTid`.
#[test]
fn imp_ext_tid() -> TestResult {
    let mut imp_ext = ImpExt::default();

    imp_ext.unmarshal(&[]).map_err(|err| err.to_string())?;
    ensure(!imp_ext.dirty(), "New impext should not be dirty.")?;
    ensure_eq(imp_ext.get_tid(), "", "Empty ImpExt should have  empty tid")?;

    imp_ext.set_tid("tid".to_owned());
    ensure_eq(imp_ext.get_tid(), "tid", "ImpExt tid is incorrect")?;
    ensure(imp_ext.dirty(), "New impext should be dirty.")
}

/// Go's `impWrapper` of `TestCloneImpWrapper`.
fn clone_imp_wrapper_given() -> ImpWrapper {
    ImpWrapper {
        imp: None,
        imp_ext: Some(Box::new(ImpExt {
            tid: clean("occupied".to_owned()),
            ..ImpExt::default()
        })),
    }
}

/// Go: `TestCloneImpWrapper` (and Go's `Clone` of a request's wrapper,
/// [`ImpWrapperMut::to_imp_wrapper`], which gives the same copy).
#[test]
fn clone_imp_wrapper() -> TestResult {
    nil_clone::<ImpWrapper>()?;
    let general = |wrapper: &mut ImpWrapper| {
        if let Some(imp_ext) = wrapper.imp_ext.as_deref_mut() {
            imp_ext.ext.dirty = true;
            "Something".clone_into(&mut imp_ext.tid.value);
        }
        wrapper.imp_ext = None;
    };
    let mut original = clone_imp_wrapper_given();
    let view_clone = original.view().to_imp_wrapper();
    all_ok([
        ("NoMutate", clone_case(clone_imp_wrapper_given, |_| {})),
        ("General", clone_case(clone_imp_wrapper_given, general)),
        (
            "View",
            ensure_eq(&view_clone, &clone_imp_wrapper_given(), "clone"),
        ),
    ])
}

/// Go's `impExt` of the `NoMutate` and `General` cases of `TestCloneImpExt`.
fn clone_imp_ext_general() -> ImpExt {
    ImpExt {
        ext: dirty(ExtMap(Some(members(&[("A", r#""X""#), ("B", r#""Y""#)])))),
        tid: clean("TID".to_owned()),
        ..ImpExt::default()
    }
}

/// Go's `impExt` of the `Prebid` case of `TestCloneImpExt`.
fn clone_imp_ext_prebid() -> ImpExt {
    let stored_bid_response = |id: &str, bidder: &str, replace: bool| ExtStoredBidResponse {
        id: id.to_owned(),
        bidder: bidder.to_owned(),
        replace_imp_id: Some(replace),
    };
    ImpExt {
        prebid: clean(Some(Box::new(ExtImpPrebid {
            stored_request: Some(Box::new(ExtStoredRequest {
                id: "abc123".to_owned(),
            })),
            stored_auction_response: Some(Box::new(ExtStoredAuctionResponse {
                id: "123abc".to_owned(),
            })),
            stored_bid_response: Some(vec![
                stored_bid_response("foo", "bar", true),
                stored_bid_response("def", "xyz", false),
            ]),
            is_rewarded_inventory: Some(1),
            bidder: Some(object(&[("abc", "{}"), ("def", r#"{"alpha":"beta"}"#)])),
            options: Some(Box::new(Options {
                echo_video_attrs: true,
            })),
            passthrough: raw(r#"{"foo":"bar"}"#),
            floors: Some(Box::new(ExtImpPrebidFloors {
                floor_rule: "Rule 16".to_owned(),
                floor_rule_value: 16.17,
                floor_value: 6.7,
                ..ExtImpPrebidFloors::default()
            })),
            ..ExtImpPrebid::default()
        }))),
        ..ImpExt::default()
    }
}

/// The mutator of the `Prebid` case of `TestCloneImpExt`.
fn mutate_imp_ext_prebid(imp_ext: &mut ImpExt) {
    if let Some(prebid) = imp_ext.prebid.value.as_deref_mut() {
        if let Some(stored) = prebid.stored_request.as_deref_mut() {
            "seventy".clone_into(&mut stored.id);
        }
        prebid.stored_request = None;
        if let Some(stored) = prebid.stored_auction_response.as_deref_mut() {
            "xyz".clone_into(&mut stored.id);
        }
        prebid.stored_auction_response = None;
        let responses = prebid.stored_bid_response.get_or_insert_with(Vec::new);
        if let Some(first) = responses.first_mut() {
            "alpha".clone_into(&mut first.id);
        }
        if let Some(second) = responses.get_mut(1) {
            second.replace_imp_id = None;
        }
        if let Some(first) = responses.first_mut() {
            *first = ExtStoredBidResponse {
                id: "o".to_owned(),
                bidder: "k".to_owned(),
                replace_imp_id: Some(false),
            };
        }
        responses.push(ExtStoredBidResponse {
            id: "jay".to_owned(),
            bidder: "walk".to_owned(),
            replace_imp_id: None,
        });
        prebid.is_rewarded_inventory = None;
        put_raw(&mut prebid.bidder, "def", "");
        if let Some(bidder) = prebid.bidder.as_mut() {
            let _removed = bidder.remove("abc");
        }
        put_raw(&mut prebid.bidder, "xyz", r#"{"jar":5}"#);
        if let Some(options) = prebid.options.as_deref_mut() {
            options.echo_video_attrs = false;
        }
        prebid.options = None;
        prebid.passthrough = raw("{}");
        if let Some(floors) = prebid.floors.as_deref_mut() {
            "Friday".clone_into(&mut floors.floor_rule);
            "EUR".clone_into(&mut floors.floor_min_cur);
        }
        prebid.floors = None;
    }
}

/// Go: `TestCloneImpExt`.
#[test]
fn clone_imp_ext() -> TestResult {
    nil_clone::<ImpExt>()?;
    let general = |imp_ext: &mut ImpExt| {
        put(&mut imp_ext.ext.value, "A", r#""string""#);
        put(&mut imp_ext.ext.value, "C", "{}");
        imp_ext.ext.dirty = false;
        imp_ext.tid = dirty("other".to_owned());
    };
    all_ok([
        ("NoMutate", clone_case(clone_imp_ext_general, |_| {})),
        ("General", clone_case(clone_imp_ext_general, general)),
        (
            "Prebid",
            clone_case(clone_imp_ext_prebid, mutate_imp_ext_prebid),
        ),
    ])
}

/// Go's `RegExt{dsa: &ExtRegsDSA{Required: required}, dsaDirty: true}`.
fn dirty_dsa(required: Option<i8>) -> RegExt {
    RegExt {
        dsa: dirty(required.map(dsa_required)),
        ..RegExt::default()
    }
}

/// Go: `TestRebuildRegExt`, the cases of a nil regs or regs ext and the `dsa` cases.
#[test]
fn rebuild_reg_ext() -> TestResult {
    let all = Owner::Ext(r#"{"dsa":{"dsarequired":1},"gdpr":1,"us_privacy":"a"}"#);
    let all_dirty = || RegExt {
        gdpr: dirty(Some(1)),
        us_privacy: dirty("a".to_owned()),
        ..dirty_dsa(Some(1))
    };
    let dsa_1 = Owner::Ext(r#"{"dsa":{"dsarequired":1}}"#);
    let cases = [
        (
            "req_regs_nil_-_not_dirty_-_no_change",
            Owner::Nil,
            RegExt::default(),
            Owner::Nil,
        ),
        (
            "req_regs_nil_-_dirty_and_different_-_change",
            Owner::Nil,
            all_dirty(),
            all,
        ),
        (
            "req_regs_ext_nil_-_not_dirty_-_no_change",
            Owner::NoExt,
            RegExt::default(),
            Owner::NoExt,
        ),
        (
            "req_regs_ext_nil_-_dirty_and_different_-_change",
            Owner::NoExt,
            all_dirty(),
            all,
        ),
        (
            "req_regs_dsa_populated_-_not_dirty_-_no_change",
            dsa_1,
            RegExt::default(),
            dsa_1,
        ),
        (
            "req_regs_dsa_populated_-_dirty_and_different-_change",
            dsa_1,
            dirty_dsa(Some(2)),
            Owner::Ext(r#"{"dsa":{"dsarequired":2}}"#),
        ),
        (
            "req_regs_dsa_populated_-_dirty_and_same_-_no_change",
            dsa_1,
            dirty_dsa(Some(1)),
            dsa_1,
        ),
        (
            "req_regs_dsa_populated_-_dirty_and_nil_-_cleared",
            Owner::Ext("{}"),
            dirty_dsa(None),
            Owner::NoExt,
        ),
    ];
    rebuild_cases(cases, regs_request, install_reg_ext)
}

/// Go: `TestRebuildRegExt`, the `gdpr` and `us_privacy` cases.
#[test]
fn rebuild_reg_ext_gdpr_us_privacy() -> TestResult {
    let gdpr = |gdpr| RegExt {
        gdpr: dirty(gdpr),
        ..RegExt::default()
    };
    let gdpr_1 = Owner::Ext(r#"{"gdpr":1}"#);
    let usp = |us_privacy: &str| RegExt {
        us_privacy: dirty(us_privacy.to_owned()),
        ..RegExt::default()
    };
    let usp_a = Owner::Ext(r#"{"us_privacy":"a"}"#);
    let cases = [
        (
            "req_regs_gdpr_populated_-_not_dirty_-_no_change",
            gdpr_1,
            RegExt::default(),
            gdpr_1,
        ),
        (
            "req_regs_gdpr_populated_-_dirty_and_different-_change",
            gdpr_1,
            gdpr(Some(0)),
            Owner::Ext(r#"{"gdpr":0}"#),
        ),
        (
            "req_regs_gdpr_populated_-_dirty_and_same_-_no_change",
            gdpr_1,
            gdpr(Some(1)),
            gdpr_1,
        ),
        (
            "req_regs_gdpr_populated_-_dirty_and_nil_-_cleared",
            Owner::Ext("{}"),
            gdpr(None),
            Owner::NoExt,
        ),
        (
            "req_regs_usprivacy_populated_-_not_dirty_-_no_change",
            usp_a,
            RegExt::default(),
            usp_a,
        ),
        (
            "req_regs_usprivacy_populated_-_dirty_and_different-_change",
            usp_a,
            usp("b"),
            Owner::Ext(r#"{"us_privacy":"b"}"#),
        ),
        (
            "req_regs_usprivacy_populated_-_dirty_and_same_-_no_change",
            usp_a,
            usp("a"),
            usp_a,
        ),
        (
            "req_regs_usprivacy_populated_-_dirty_and_nil_-_cleared",
            usp_a,
            usp(""),
            Owner::NoExt,
        ),
    ];
    rebuild_cases(cases, regs_request, install_reg_ext)
}

/// The request of a regs ext rebuild case.
fn regs_request(owner: Owner) -> openrtb2::BidRequest {
    openrtb2::BidRequest {
        regs: owned(owner, |regs: &mut openrtb2::Regs| &mut regs.ext),
        ..openrtb2::BidRequest::default()
    }
}

/// Stores a regs ext of a rebuild case, with Go's empty map.
fn install_reg_ext(w: &mut RequestWrapper, reg_ext: RegExt) {
    w.reg_ext = Some(Box::new(RegExt {
        ext: made(),
        ..reg_ext
    }));
}

/// Go: `TestRebuildRegExt`, the `gpc` cases.
#[test]
fn rebuild_reg_ext_gpc() -> TestResult {
    let gpc = |gpc: Option<&str>| RegExt {
        gpc: dirty(gpc.map(str::to_owned)),
        ..RegExt::default()
    };
    let gpc_a = Owner::Ext(r#"{"gpc":"a"}"#);
    let cases = [
        (
            "req_regs_gpc_populated_-_not_dirty_-_no_change",
            gpc_a,
            RegExt::default(),
            gpc_a,
        ),
        (
            "req_regs_gpc_populated_-_dirty_and_different-_change",
            gpc_a,
            gpc(Some("b")),
            Owner::Ext(r#"{"gpc":"b"}"#),
        ),
        (
            "req_regs_gpc_populated_-_dirty_and_same_-_no_change",
            gpc_a,
            gpc(Some("a")),
            gpc_a,
        ),
        (
            "req_regs_gpc_populated_-_dirty_and_nil_-_cleared",
            gpc_a,
            gpc(None),
            Owner::NoExt,
        ),
    ];
    rebuild_cases(cases, regs_request, install_reg_ext)
}

/// What `TestRegExtUnmarshal` checks of a decoded regs ext: `dsa`, `gdpr`, `gpc`, `usPrivacy`.
type RegExtDecoded = (Option<ExtRegsDSA>, Option<i8>, Option<String>, String);

/// The cases of `TestRegExtUnmarshal`: name, object, ext, what is decoded, whether it fails.
fn reg_ext_unmarshal_cases() -> Vec<(&'static str, RegExt, &'static str, RegExtDecoded, bool)> {
    let nothing = || (None, None, None, String::new());
    let dsa = |required| (Some(dsa_required(required)), None, None, String::new());
    let gdpr = |gdpr| (None, Some(gdpr), None, String::new());
    let gpc = |gpc: &str| (None, None, Some(gpc.to_owned()), String::new());
    let decoded = RegExt {
        ext: clean(ExtMap(Some(members(&[("dsa", "{}")])))),
        ..RegExt::default()
    };
    let ext_dirty = RegExt {
        ext: dirty(ExtMap(None)),
        ..RegExt::default()
    };
    let new = RegExt::default;
    vec![
        (
            "RegExt.ext_not_empty_and_not_dirtyr",
            decoded,
            "",
            nothing(),
            false,
        ),
        (
            "RegExt.ext_empty_and_dirty",
            ext_dirty,
            r#"{"dsa":{"dsarequired":1}}"#,
            nothing(),
            false,
        ),
        (
            "nothing_to_unmarshal",
            RegExt {
                ext: made(),
                ..new()
            },
            "",
            nothing(),
            false,
        ),
        // DSA
        (
            "valid_dsa_json",
            new(),
            r#"{"dsa":{"dsarequired":1}}"#,
            dsa(1),
            false,
        ),
        // Go: dsa(0), the `dsarequired` jsoniter allocated before the fault; the port keeps the
        // empty `dsa` it decoded into (json.partial-results).
        (
            "malformed_dsa_json",
            new(),
            r#"{"dsa":{"dsarequired":""}}"#,
            (Some(ExtRegsDSA::default()), None, None, String::new()),
            true,
        ),
        // GDPR
        ("valid_gdpr_json", new(), r#"{"gdpr":1}"#, gdpr(1), false),
        // Go: gdpr(0), the value jsoniter allocated before the fault; the port keeps nil
        // (json.partial-results).
        (
            "malformed_gdpr_json",
            new(),
            r#"{"gdpr":""}"#,
            nothing(),
            true,
        ),
        // GPC
        (
            "valid_gpc_json",
            new(),
            r#"{"gpc":"some_value"}"#,
            gpc("some_value"),
            false,
        ),
        (
            r#"valid_gpc_json_"1""#,
            new(),
            r#"{"gpc": "1"}"#,
            gpc("1"),
            false,
        ),
        ("valid_gpc_json_1", new(), r#"{"gpc": 1}"#, gpc("1"), false),
        (
            "malformed_gpc_json",
            new(),
            r#"{"gpc":nill}"#,
            nothing(),
            true,
        ),
        // us_privacy
        (
            "valid_usprivacy_json",
            new(),
            r#"{"us_privacy":"consent"}"#,
            (None, None, None, "consent".to_owned()),
            false,
        ),
        (
            "malformed_usprivacy_json",
            new(),
            r#"{"us_privacy":1}"#,
            nothing(),
            true,
        ),
    ]
}

/// `ExtRegsDSA{Required: &required}`.
fn dsa_required(required: i8) -> ExtRegsDSA {
    ExtRegsDSA {
        required: Some(required),
        ..ExtRegsDSA::default()
    }
}

/// Go: `TestRegExtUnmarshal`.
#[test]
fn reg_ext_unmarshal() -> TestResult {
    all_ok(reg_ext_unmarshal_cases().into_iter().map(
        |(name, mut reg_ext, json, expected, expect_error)| {
            let failed = reg_ext.unmarshal(json.as_bytes()).is_err();
            let got = (
                reg_ext.dsa.value,
                reg_ext.gdpr.value,
                reg_ext.gpc.value,
                reg_ext.us_privacy.value,
            );
            let result = ensure_eq(&failed, &expect_error, "error")
                .and_then(|()| ensure_eq(&got, &expected, "dsa, gdpr, gpc, us_privacy"));
            (name, result)
        },
    ))
}

/// Go: `TestRegExtGetExtSetExt`.
#[test]
fn reg_ext_get_ext_set_ext() -> TestResult {
    let mut reg_ext = RegExt::default();
    ensure_eq(&texts(&reg_ext.get_ext()), &BTreeMap::new(), "GetExt")?;
    ensure(!reg_ext.dirty(), "dirty")?;

    let raw_json = members(&[("dsa", "{}"), ("gdpr", "1"), ("usprivacy", r#""consent""#)]);
    reg_ext.set_ext(Some(raw_json.clone()));
    ensure(reg_ext.dirty(), "not dirty")?;
    ensure_eq(&texts(&reg_ext.get_ext()), &texts(&raw_json), "GetExt")
}

/// Go: `TestRegExtGetDSASetDSA`.
#[test]
fn reg_ext_get_dsa_set_dsa() -> TestResult {
    let mut reg_ext = RegExt::default();
    ensure_eq(&reg_ext.get_dsa(), &None, "GetDSA")?;
    ensure(!reg_ext.dirty(), "dirty")?;

    let dsa = Some(dsa_required(2));
    reg_ext.set_dsa(dsa.clone());
    ensure(reg_ext.dirty(), "not dirty")?;
    ensure_eq(&reg_ext.get_dsa(), &dsa.as_ref(), "GetDSA")
}

/// Go: `TestRegExtGetUSPrivacySetUSPrivacy`.
#[test]
fn reg_ext_get_us_privacy_set_us_privacy() -> TestResult {
    let mut reg_ext = RegExt::default();
    ensure_eq(reg_ext.get_us_privacy(), "", "GetUSPrivacy")?;
    ensure(!reg_ext.dirty(), "dirty")?;

    reg_ext.set_us_privacy("consent".to_owned());
    ensure(reg_ext.dirty(), "not dirty")?;
    ensure_eq(reg_ext.get_us_privacy(), "consent", "GetUSPrivacy")
}

/// Go: `TestRegExtGetGDPRSetGDPR`.
#[test]
fn reg_ext_get_gdpr_set_gdpr() -> TestResult {
    let mut reg_ext = RegExt::default();
    ensure_eq(&reg_ext.get_gdpr(), &None, "GetGDPR")?;
    ensure(!reg_ext.dirty(), "dirty")?;

    reg_ext.set_gdpr(Some(1));
    ensure(reg_ext.dirty(), "not dirty")?;
    ensure_eq(&reg_ext.get_gdpr(), &Some(1), "GetGDPR")
}

/// Go: `TestRegExtGetGPCSetGPC`.
#[test]
fn reg_ext_get_gpc_set_gpc() -> TestResult {
    let mut reg_ext = RegExt::default();
    ensure_eq(&reg_ext.get_gpc(), &None, "GetGPC")?;
    ensure(!reg_ext.dirty(), "dirty")?;

    reg_ext.set_gpc(Some("Gpc".to_owned()));
    ensure(reg_ext.dirty(), "not dirty")?;
    ensure_eq(&reg_ext.get_gpc(), &Some("Gpc"), "GetGPC")
}
