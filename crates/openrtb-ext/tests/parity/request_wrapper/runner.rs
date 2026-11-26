//! The port's side of the wrapper goldens: the steps of the Go generator's scripts (see the
//! comment on `scripts` in `testdata/go/request_wrapper/steps.go`), run on a [`RequestWrapper`]
//! through the crate's public API, with the observation of each.

use std::collections::{BTreeMap, BTreeSet};

use pbs_errortypes::GoError;
use pbs_gojson::RawJson;
use pbs_jsonutil::{marshal, unmarshal};
use pbs_openrtb::openrtb2;
use pbs_openrtb_ext::{
    ConsentedProvidersSettingsIn, ConsentedProvidersSettingsOut, ExtAppPrebid, ExtDevicePrebid,
    ExtImpPrebid, ExtImpPrebidFloors, ExtRegsDSA, ExtRequestPrebid, ExtUserPrebid, ImpWrapper,
    ImpWrapperMut, RequestWrapper, clear26_fields, clear202211_fields, clear202303_fields,
    clear202309_fields, clear202402_fields, clear202409_fields, convert_down_to25, convert_up_to26,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use super::{
    super::support::TestResult,
    obs::{NON_FINITE, Part},
};

/// An ext map's members, as `GetExt` returns them and `SetExt` takes them.
type ExtMembers = BTreeMap<String, Option<Box<RawValue>>>;

/// `json.raw-not-json`: bytes that are not JSON in a `json.RawMessage`, which Go's ext maps hold
/// and the port's do not.
pub(super) const RAW_NOT_JSON: &str = "json.raw-not-json";

/// The text of Go's panic on a write into a nil map, which the port's rebuild returns as an
/// error.
const NIL_MAP_PANIC: &str = "assignment to entry in nil map";

/// What a step did.
#[derive(Debug)]
pub(super) enum Outcome {
    /// Its observation.
    Observed(Vec<Part>),
    /// Go's panic text: the rebuild failed on a write into a nil map.
    Panicked(String),
    /// The step cannot be run with the port's types (an ext member that is not JSON); the
    /// ledger entry that says so.
    Inexpressible(&'static str),
}

/// An observation as the port makes it.
#[derive(Debug, Default)]
struct Obs(Vec<Part>);

impl Obs {
    /// Appends an error's text (empty without one).
    fn err(&mut self, err: Option<&GoError>) {
        self.0.push(Part::Err(
            err.map_or_else(String::new, |e| e.message().to_owned()),
        ));
    }

    /// Appends a value as `pbs_jsonutil::marshal` writes it, or its error.
    fn marshal<T: Serialize + ?Sized>(&mut self, v: &T) {
        self.0.push(match marshal(v) {
            Ok(json) => Part::Json(String::from_utf8_lossy(&json).into_owned()),
            Err(e) => Part::MarshalErr(e.into_message()),
        });
    }

    /// Appends a step's error and the request.
    fn request(&mut self, err: Option<&GoError>, w: &RequestWrapper) {
        self.err(err);
        self.marshal(&w.bid_request());
    }

    /// Appends a flag.
    fn bool(&mut self, b: bool) {
        self.0
            .push(Part::Text(if b { "t" } else { "f" }.to_owned()));
    }

    /// Appends a number.
    fn int(&mut self, n: usize) {
        self.0.push(Part::Text(n.to_string()));
    }

    /// Appends an ext map: each key as JSON and its value's text.
    fn raw_map(&mut self, map: &BTreeMap<String, Option<Box<RawValue>>>) {
        let members = map
            .iter()
            .map(|(key, value)| {
                let key = marshal(key).map_or_else(GoError::into_message, |json| {
                    String::from_utf8_lossy(&json).into_owned()
                });
                (key, value.as_deref().map(|raw| raw.get().to_owned()))
            })
            .collect();
        self.0.push(Part::Map(members));
    }
}

/// A step's argument decoded with `pbs_jsonutil::unmarshal`.
fn decode<T: DeserializeOwned>(arg: &str) -> TestResult<T> {
    unmarshal(arg.as_bytes()).map_err(|e| format!("step argument {arg}: {e}"))
}

/// The objects of the `all` steps, in order.
const OBJECTS: [&str; 9] = [
    "request", "user", "device", "app", "regs", "site", "dooh", "source", "imp",
];

/// The objects of a step's target: all of them for `all`.
fn targets(target: &str) -> Vec<&str> {
    if target == "all" {
        OBJECTS.to_vec()
    } else {
        vec![target]
    }
}

/// Go's panic text when `result` is the nil map error.
fn panic_of(result: &Result<(), GoError>) -> Option<String> {
    result
        .as_ref()
        .err()
        .filter(|e| e.message() == NIL_MAP_PANIC)
        .map(|e| e.message().to_owned())
}

/// Runs the steps of a script on one wrapper, as the Go generator's runner does.
#[derive(Debug)]
pub(super) struct Runner {
    /// The wrapper.
    w: RequestWrapper,
    /// The objects whose ext failed to decode in a step, which Go's wrapper then holds as
    /// jsoniter left it (`json.partial-results`).
    failed: BTreeSet<&'static str>,
    /// The ledger entries of values set that JSON cannot give, which a later step writes.
    tainted: BTreeSet<&'static str>,
}

/// A `get_*_ext_pair`'s ext and error, the error recorded in `failed` for `obj`.
macro_rules! pair {
    ($failed:expr, $obj:literal, $pair:expr) => {{
        let (e, err) = $pair;
        if err.is_some() {
            let _ = $failed.insert($obj);
        }
        (e, err)
    }};
}

impl Runner {
    /// A runner of a new wrapper of `request`.
    pub(super) fn new(request: openrtb2::BidRequest) -> Self {
        Self {
            w: RequestWrapper::new(request),
            failed: BTreeSet::new(),
            tainted: BTreeSet::new(),
        }
    }

    /// The objects whose ext failed to decode so far.
    pub(super) const fn failed(&self) -> &BTreeSet<&'static str> {
        &self.failed
    }

    /// The ledger entries of the values set that JSON cannot give.
    pub(super) const fn tainted(&self) -> &BTreeSet<&'static str> {
        &self.tainted
    }

    /// Runs a step.
    pub(super) fn step(&mut self, step: &str) -> TestResult<Outcome> {
        let mut o = Obs::default();
        let (name, arg) = step.split_once('=').unwrap_or((step, ""));
        let (op, target) = name.split_once(':').unwrap_or((name, ""));
        match op {
            "get" => self.get(target, &mut o)?,
            "set" => self.set(target, arg, &mut o)?,
            "special" => self.special(target, &mut o)?,
            "setext" => {
                let map: Option<BTreeMap<String, Option<Box<RawValue>>>> = decode(arg)?;
                self.set_ext(target, Some(&map), &mut o)?;
            }
            "setrawext" => {
                let (key, raw) = arg.split_once('=').unwrap_or((arg, ""));
                let Ok(raw) = RawValue::from_string(raw.to_owned()) else {
                    return Ok(Outcome::Inexpressible(RAW_NOT_JSON));
                };
                let map = BTreeMap::from([(key.to_owned(), Some(raw))]);
                self.set_ext(target, Some(&Some(map)), &mut o)?;
            }
            "touchext" => {
                for obj in targets(target) {
                    self.set_ext(obj, None, &mut o)?;
                }
            }
            "setimpraw" => {
                for mut iw in self.w.get_imp() {
                    if let Some(imp) = iw.imp_mut() {
                        imp.ext = RawJson::from(arg);
                    }
                }
                self.w.set_imp_from_get_imp();
                o.request(None, &self.w);
            }
            "touch" => {
                for obj in targets(target) {
                    self.touch(obj, &mut o)?;
                }
            }
            _ => {
                if let Some(panic) = self.request_step(op, &mut o)? {
                    return Ok(Outcome::Panicked(panic));
                }
            }
        }
        Ok(Outcome::Observed(o.0))
    }

    /// Runs a step on the request or the impression wrappers; `Some` is Go's panic text.
    fn request_step(&mut self, op: &str, o: &mut Obs) -> TestResult<Option<String>> {
        let w = &mut self.w;
        match op {
            "snap" => o.request(None, w),
            "rebuild" => {
                let result = w.rebuild_request();
                if let Some(panic) = panic_of(&result) {
                    return Ok(Some(panic));
                }
                o.request(result.err().as_ref(), w);
            }
            "up" => {
                let result = convert_up_to26(w);
                if result.is_err() {
                    let _ = self.failed.insert("convert");
                }
                o.request(result.err().as_ref(), &self.w);
            }
            "down" => {
                let result = convert_down_to25(w);
                if result.is_err() {
                    let _ = self.failed.insert("convert");
                }
                o.request(result.err().as_ref(), &self.w);
            }
            "clear26" => clear(w, clear26_fields, o),
            "clear202211" => clear(w, clear202211_fields, o),
            "clear202303" => clear(w, clear202303_fields, o),
            "clear202309" => clear(w, clear202309_fields, o),
            "clear202402" => clear(w, clear202402_fields, o),
            "clear202409" => clear(w, clear202409_fields, o),
            "clone" => {
                *w = w.clone();
                o.int(w.len_imp());
            }
            "cloneclear" => {
                *w = w.clone_and_clear_imp_wrappers();
                o.int(w.len_imp());
            }
            "cloneclearkeep" => {
                let _clone = w.clone_and_clear_imp_wrappers();
                o.int(w.len_imp());
            }
            "exchangecopy" => {
                let bid_request = w.bid_request().cloned().map(Box::new);
                *w = w.clone_and_clear_imp_wrappers_with(bid_request);
                o.int(w.len_imp());
            }
            "partialclone" => {
                let bid_request = w.bid_request().cloned().map(Box::new);
                w.set_bid_request(bid_request);
                o.request(None, w);
            }
            "nilreq" => {
                w.set_bid_request(None);
                o.request(None, w);
            }
            _ => return self.imp_step(op, o),
        }
        Ok(None)
    }

    /// Runs a step on the impressions or their wrappers; `Some` is Go's panic text.
    fn imp_step(&mut self, op: &str, o: &mut Obs) -> TestResult<Option<String>> {
        let w = &mut self.w;
        match op {
            "setimprev" => {
                let rev = w
                    .get_imp()
                    .iter()
                    .rev()
                    .map(ImpWrapperMut::to_imp_wrapper)
                    .collect();
                w.set_imp(Some(rev));
                o.request(None, w);
            }
            "setimpdrop" => {
                let imps: Vec<ImpWrapper> = w
                    .get_imp()
                    .iter()
                    .map(ImpWrapperMut::to_imp_wrapper)
                    .collect();
                if !imps.is_empty() {
                    w.set_imp(Some(imps.into_iter().skip(1).collect()));
                }
                o.request(None, w);
            }
            "setimpnil" => {
                w.set_imp(None);
                o.request(None, w);
            }
            "assignlast" => {
                let last = w
                    .bid_request()
                    .and_then(|req| req.imp.as_ref()?.last().cloned());
                w.assign_imp(last.map(|imp| vec![imp]));
                o.request(None, w);
            }
            "lenimp" => o.int(w.len_imp()),
            "getimp" => {
                let imps = w.get_imp();
                o.int(imps.len());
                for iw in &imps {
                    o.marshal(&iw.imp());
                }
            }
            "setimp" => {
                w.set_imp_from_get_imp();
                o.request(None, w);
            }
            "assignrev" => {
                if let Some(imps) = w.bid_request().and_then(|req| req.imp.clone()) {
                    w.assign_imp(Some(imps.into_iter().rev().collect()));
                }
                o.request(None, w);
            }
            "dropimp" => {
                w.assign_imp(None);
                o.request(None, w);
            }
            "rebuildimp" => {
                for mut iw in w.get_imp() {
                    let result = iw.rebuild_imp();
                    if let Some(panic) = panic_of(&result) {
                        return Ok(Some(panic));
                    }
                    o.err(result.err().as_ref());
                    o.marshal(&iw.imp());
                }
            }
            "getorcreate" => {
                let mut failed = false;
                for mut iw in w.get_imp() {
                    let (e, err) = iw.get_imp_ext_pair();
                    failed |= err.is_some();
                    o.marshal(e.get_or_create_prebid());
                    o.bool(e.dirty());
                }
                if failed {
                    let _ = self.failed.insert("imp");
                }
            }
            _ => return Err(format!("unknown step {op}")),
        }
        Ok(None)
    }

    /// Records an object's `GetXExt` error and every getter.
    fn get(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match obj {
            "request" => {
                let (e, err) = pair!(failed, "request", w.get_request_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.marshal(&e.get_s_chain());
                o.bool(e.dirty());
            }
            "user" => {
                let (e, err) = pair!(failed, "user", w.get_user_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_consent());
                o.marshal(&e.get_prebid());
                o.marshal(&e.get_eid());
                o.marshal(&e.get_consented_providers_settings_in());
                o.marshal(&e.get_consented_providers_settings_out());
                o.bool(e.dirty());
            }
            "device" => {
                let (e, err) = pair!(failed, "device", w.get_device_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.marshal(e.get_c_dep());
                o.bool(e.dirty());
            }
            "app" => {
                let (e, err) = pair!(failed, "app", w.get_app_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_prebid());
                o.bool(e.dirty());
            }
            "regs" => {
                let (e, err) = pair!(failed, "regs", w.get_reg_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_dsa());
                o.marshal(&e.get_gdpr());
                o.marshal(&e.get_gpc());
                o.marshal(e.get_us_privacy());
                o.bool(e.dirty());
            }
            "site" => {
                let (e, err) = pair!(failed, "site", w.get_site_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_amp());
                o.bool(e.dirty());
            }
            "dooh" => {
                let (e, err) = pair!(failed, "dooh", w.get_dooh_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.bool(e.dirty());
            }
            "source" => {
                let (e, err) = pair!(failed, "source", w.get_source_ext_pair());
                o.err(err.as_ref());
                o.raw_map(&e.get_ext());
                o.marshal(&e.get_s_chain());
                o.bool(e.dirty());
            }
            "imp" => {
                let mut imps = w.get_imp();
                o.int(imps.len());
                for iw in &mut imps {
                    let (e, err) = pair!(failed, "imp", iw.get_imp_ext_pair());
                    o.err(err.as_ref());
                    o.raw_map(&e.get_ext());
                    o.marshal(&e.get_prebid());
                    o.marshal(&e.get_data());
                    o.marshal(e.get_tid());
                    o.marshal(e.get_gp_id());
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// Calls a member's setter with `arg` decoded into its parameter.
    fn set(&mut self, target: &str, arg: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match target {
            "request.prebid" => {
                let e = pair!(failed, "request", w.get_request_ext_pair()).0;
                e.set_prebid(decode::<Option<ExtRequestPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "request.schain" => {
                let e = pair!(failed, "request", w.get_request_ext_pair()).0;
                e.set_s_chain(decode::<Option<openrtb2::SupplyChain>>(arg)?);
                o.bool(e.dirty());
            }
            "user.consent" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_consent(decode::<Option<String>>(arg)?);
                o.bool(e.dirty());
            }
            "user.prebid" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_prebid(decode::<Option<ExtUserPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "user.eids" => {
                let eids = if arg == "&nil" {
                    Some(None)
                } else {
                    decode::<Option<Option<Vec<openrtb2::EID>>>>(arg)?
                };
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_eid(eids);
                o.bool(e.dirty());
            }
            "user.cpsin" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_consented_providers_settings_in(decode::<
                    Option<ConsentedProvidersSettingsIn>,
                >(arg)?);
                o.bool(e.dirty());
            }
            "user.cpsout" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_consented_providers_settings_out(decode::<
                    Option<ConsentedProvidersSettingsOut>,
                >(arg)?);
                o.bool(e.dirty());
            }
            "device.prebid" => {
                let e = pair!(failed, "device", w.get_device_ext_pair()).0;
                e.set_prebid(decode::<Option<ExtDevicePrebid>>(arg)?);
                o.bool(e.dirty());
            }
            "device.cdep" => {
                let e = pair!(failed, "device", w.get_device_ext_pair()).0;
                e.set_c_dep(decode::<String>(arg)?);
                o.bool(e.dirty());
            }
            "app.prebid" => {
                let e = pair!(failed, "app", w.get_app_ext_pair()).0;
                e.set_prebid(decode::<Option<ExtAppPrebid>>(arg)?);
                o.bool(e.dirty());
            }
            _ => {
                self.set_other(target, arg, o)?;
                return Ok(());
            }
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// [`Runner::set`] for the members of the regs, site, source and impression exts.
    fn set_other(&mut self, target: &str, arg: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match target {
            "regs.dsa" => {
                let e = pair!(failed, "regs", w.get_reg_ext_pair()).0;
                e.set_dsa(decode::<Option<ExtRegsDSA>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.gdpr" => {
                let e = pair!(failed, "regs", w.get_reg_ext_pair()).0;
                e.set_gdpr(decode::<Option<i8>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.gpc" => {
                let e = pair!(failed, "regs", w.get_reg_ext_pair()).0;
                e.set_gpc(decode::<Option<String>>(arg)?);
                o.bool(e.dirty());
            }
            "regs.usprivacy" => {
                let e = pair!(failed, "regs", w.get_reg_ext_pair()).0;
                e.set_us_privacy(decode::<String>(arg)?);
                o.bool(e.dirty());
            }
            "site.amp" => {
                let e = pair!(failed, "site", w.get_site_ext_pair()).0;
                e.set_amp(decode::<Option<i8>>(arg)?);
                o.bool(e.dirty());
            }
            "source.schain" => {
                let e = pair!(failed, "source", w.get_source_ext_pair()).0;
                e.set_s_chain(decode::<Option<openrtb2::SupplyChain>>(arg)?);
                o.bool(e.dirty());
            }
            "imp.prebid" => {
                for mut iw in w.get_imp() {
                    let e = pair!(failed, "imp", iw.get_imp_ext_pair()).0;
                    e.set_prebid(decode::<Option<ExtImpPrebid>>(arg)?);
                    o.bool(e.dirty());
                }
            }
            "imp.tid" => {
                for mut iw in w.get_imp() {
                    let e = pair!(failed, "imp", iw.get_imp_ext_pair()).0;
                    e.set_tid(decode::<String>(arg)?);
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown member {target}")),
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// Calls a setter with a value that JSON cannot give, as the Go generator's `special` does:
    /// NaN and infinite floats, and raw messages that are not JSON or not compact.
    fn special(&mut self, target: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match target {
            "request.prebid.nan" => {
                let e = pair!(failed, "request", w.get_request_ext_pair()).0;
                e.set_prebid(Some(ExtRequestPrebid {
                    debug: true,
                    bid_adjustment_factors: Some(BTreeMap::from([("a".to_owned(), f64::NAN)])),
                    ..ExtRequestPrebid::default()
                }));
                o.bool(e.dirty());
                let _ = self.tainted.insert(NON_FINITE);
            }
            "imp.prebid.inf" => {
                for mut iw in w.get_imp() {
                    let e = pair!(failed, "imp", iw.get_imp_ext_pair()).0;
                    e.set_prebid(Some(ExtImpPrebid {
                        floors: Some(Box::new(ExtImpPrebidFloors {
                            floor_rule_value: f64::INFINITY,
                            ..ExtImpPrebidFloors::default()
                        })),
                        ..ExtImpPrebid::default()
                    }));
                    o.bool(e.dirty());
                }
                let _ = self.tainted.insert(NON_FINITE);
            }
            "user.eids.badraw" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_eid(Some(Some(vec![openrtb2::EID {
                    source: "s".to_owned(),
                    ext: RawJson::from("{bad"),
                    ..openrtb2::EID::default()
                }])));
                o.bool(e.dirty());
                let _ = self.tainted.insert(RAW_NOT_JSON);
            }
            "source.schain.spacedraw" => {
                let e = pair!(failed, "source", w.get_source_ext_pair()).0;
                e.set_s_chain(Some(openrtb2::SupplyChain {
                    ver: "1".to_owned(),
                    ext: RawJson::from(r#" { "a" : 1 } "#),
                    ..openrtb2::SupplyChain::default()
                }));
                o.bool(e.dirty());
            }
            _ => return Err(format!("unknown special member {target}")),
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// Calls each object's `SetExt` with `map`, or with its `GetExt()` when `map` is `None`.
    fn set_ext(&mut self, obj: &str, map: Option<&Option<ExtMembers>>, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        macro_rules! set_ext_of {
            ($name:literal, $pair:expr) => {{
                let e = pair!(failed, $name, $pair).0;
                let ext = map.cloned().unwrap_or_else(|| Some(e.get_ext()));
                e.set_ext(ext);
                o.bool(e.dirty());
            }};
        }
        match obj {
            "request" => set_ext_of!("request", w.get_request_ext_pair()),
            "user" => set_ext_of!("user", w.get_user_ext_pair()),
            "device" => set_ext_of!("device", w.get_device_ext_pair()),
            "app" => set_ext_of!("app", w.get_app_ext_pair()),
            "regs" => set_ext_of!("regs", w.get_reg_ext_pair()),
            "site" => set_ext_of!("site", w.get_site_ext_pair()),
            "dooh" => set_ext_of!("dooh", w.get_dooh_ext_pair()),
            "source" => set_ext_of!("source", w.get_source_ext_pair()),
            "imp" => {
                for mut iw in w.get_imp() {
                    set_ext_of!("imp", iw.get_imp_ext_pair());
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// Sets every member of an object to what its getter returns.
    fn touch(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match obj {
            "request" => {
                let e = pair!(failed, "request", w.get_request_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                e.set_s_chain(e.get_s_chain().cloned());
                o.bool(e.dirty());
            }
            "user" => {
                let e = pair!(failed, "user", w.get_user_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_consent(e.get_consent().map(str::to_owned));
                e.set_prebid(e.get_prebid().cloned());
                e.set_eid(e.get_eid().map(|eids| eids.map(<[openrtb2::EID]>::to_vec)));
                e.set_consented_providers_settings_in(
                    e.get_consented_providers_settings_in().cloned(),
                );
                e.set_consented_providers_settings_out(
                    e.get_consented_providers_settings_out().cloned(),
                );
                o.bool(e.dirty());
            }
            "device" => {
                let e = pair!(failed, "device", w.get_device_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                e.set_c_dep(e.get_c_dep().to_owned());
                o.bool(e.dirty());
            }
            "app" => {
                let e = pair!(failed, "app", w.get_app_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_prebid(e.get_prebid().cloned());
                o.bool(e.dirty());
            }
            "regs" => {
                let e = pair!(failed, "regs", w.get_reg_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_dsa(e.get_dsa().cloned());
                e.set_gdpr(e.get_gdpr());
                e.set_gpc(e.get_gpc().map(str::to_owned));
                e.set_us_privacy(e.get_us_privacy().to_owned());
                o.bool(e.dirty());
            }
            _ => {
                self.touch_other(obj, o)?;
                return Ok(());
            }
        }
        self.failed.extend(failed);
        Ok(())
    }

    /// [`Runner::touch`] for the site, DOOH, source and impression exts.
    fn touch_other(&mut self, obj: &str, o: &mut Obs) -> TestResult {
        let mut failed = BTreeSet::new();
        let w = &mut self.w;
        match obj {
            "site" => {
                let e = pair!(failed, "site", w.get_site_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_amp(e.get_amp());
                o.bool(e.dirty());
            }
            "dooh" => {
                let e = pair!(failed, "dooh", w.get_dooh_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                o.bool(e.dirty());
            }
            "source" => {
                let e = pair!(failed, "source", w.get_source_ext_pair()).0;
                e.set_ext(Some(e.get_ext()));
                e.set_s_chain(e.get_s_chain().cloned());
                o.bool(e.dirty());
            }
            "imp" => {
                for mut iw in w.get_imp() {
                    let e = pair!(failed, "imp", iw.get_imp_ext_pair()).0;
                    e.set_ext(Some(e.get_ext()));
                    e.set_prebid(e.get_prebid().cloned());
                    e.set_tid(e.get_tid().to_owned());
                    o.bool(e.dirty());
                }
            }
            _ => return Err(format!("unknown object {obj}")),
        }
        self.failed.extend(failed);
        Ok(())
    }
}

/// A `Clear*` step: the function, then an empty error and the request.
fn clear(w: &mut RequestWrapper, f: fn(&mut RequestWrapper), o: &mut Obs) {
    f(w);
    o.request(None, w);
}
