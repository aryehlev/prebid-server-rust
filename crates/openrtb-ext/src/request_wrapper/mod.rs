//! Go: `openrtb_ext/request_wrapper.go`: [`RequestWrapper`], [`ImpWrapper`] and the lazily
//! decoded ext objects of a bid request.
//!
//! The ext objects are [`UserExt`], [`DeviceExt`], [`RequestExt`], [`AppExt`], [`RegExt`],
//! [`SiteExt`], [`DOOHExt`], [`SourceExt`] and [`ImpExt`]. Each is decoded on first access
//! (`get_*_ext`) into a map of its JSON members plus the members Prebid Server reads on their own
//! (`prebid`, `consent`, `gdpr`, ...). Setters mark their member dirty;
//! [`RequestWrapper::rebuild_request`] writes the dirty members back into the map and encodes the
//! map into the request, as Go does: an ext whose map ends up empty becomes absent (nil), a member
//! set to its zero value is deleted, and a member that was never touched keeps its original
//! bytes.
//!
//! The JSON is `serde_json`'s, through pbs-jsonutil where Go calls `util/jsonutil`:
//! `pbs_jsonutil::unmarshal` decodes the ext and its members (Go: `jsonutil.Unmarshal`),
//! `pbs_jsonutil::marshal_raw` encodes them (`jsonutil.Marshal`: keys sorted, raw values
//! compacted) and `pbs_jsonutil::parse_into_string` reads `regs.ext.gpc`
//! (`jsonutil.ParseIntoString`). A member of an ext map is Go's `json.RawMessage`, an
//! `Option<Box<RawValue>>` whose `None` is nil (JSON null). What `serde_json` reads or writes
//! otherwise than jsoniter is in the ledger: the library's error texts inside Prebid's wrappers
//! (`json.error-texts`), number spelling and escaping (`json.float-format`, `json.html-escape`,
//! `json.control-escapes`), null for a nil member where Go writes no value
//! (`json.nil-raw-written`; an ext map whose member is null is written as JSON where Go's rebuild
//! writes bytes that are not: `json.ext-null-member`), NaN and infinities written as null where Go
//! fails
//! (`json.non-finite`), and the decodes `serde_json` fails where jsoniter reads on
//! (`json.duplicate-keys`, `json.key-case`, `json.negative-zero-int`, `json.invalid-utf8`).
//! Prebid's own messages from `UnmarshalJSON` methods reach the caller bare, as Go gives them
//! (`prebid_messages`).
//!
//! # Impressions
//!
//! Go's `ImpWrapper` holds a pointer to its `openrtb2.Imp`. `GetImp` points each wrapper at an
//! element of `rw.Imp`, so changes through a wrapper are changes of the request, and `SetImp` and
//! `RebuildRequest` copy the wrappers' impressions into a new `rw.Imp` and point the wrappers at
//! it. When Go code replaces `rw.Imp` (or the whole `BidRequest`), the wrappers keep pointing at
//! the old impressions, and the next `RebuildRequest` copies those back into the request.
//!
//! The Rust wrapper keeps that model: a wrapper of the request either refers to the request's
//! impression at its own index or owns the impression it was detached with.
//! [`RequestWrapper::get_imp`] returns views ([`ImpWrapperMut`]) that borrow the request, and
//! [`RequestWrapper::assign_imp`] and [`RequestWrapper::set_bid_request`] are Go's `rw.Imp = imps`
//! and `rw.BidRequest = req`, which detach the wrappers from the replaced impressions. Go's
//! `rw.SetImp(rw.GetImp())` is [`RequestWrapper::set_imp_from_get_imp`].
//!
//! # Differences from Go
//!
//! * Go embeds `*openrtb2.BidRequest`; here it is reached through [`RequestWrapper::bid_request`]
//!   and [`RequestWrapper::bid_request_mut`]. Changing the length of the request's `imp` through
//!   the latter keeps the wrappers at their positions (Go's wrappers keep the old slice); replace
//!   the slice with [`RequestWrapper::assign_imp`] instead.
//! * The `get_*_ext` methods return `Err` where Go returns the ext object together with an error.
//!   Go stores the object before decoding it, and later calls return it without an error; the
//!   `get_*_ext_pair` methods return both, for the Go code that uses the object whatever the error.
//!   A member that fails to decode keeps the value it had before the decode (Go's keeps what
//!   jsoniter read before the fault: `json.partial-results`).
//! * The getters of the ext objects return borrows where Go returns shallow copies, and setters
//!   take values. Go's copies share their maps, slices and pointers with the stored value, so Go
//!   code that changes those in place changes the stored value without marking it dirty; Rust code
//!   works on its own copy and sets it back.
//! * Clones own their data. Go's `Clone` and `CloneAndClearImpWrappers` share the `BidRequest` (and
//!   the wrappers' impressions, and some ext pointers) between the original and the clone; here the
//!   clone has its own copies, which differs only when one side is changed later. What Go's clones
//!   change is kept: a nil wrapper slice becomes empty, a nil `eids` slice of the user ext and nil
//!   supply chain nodes become empty, and the request ext's `prebid` is copied with
//!   [`ExtRequestPrebid::clone_go`].
//! * Where Go panics, Rust does not: a nil `BidRequest` counts as one without impressions (and
//!   [`RequestWrapper::set_imp`] then keeps the wrappers detached), a write into a nil ext map
//!   (after `set_ext(None)`, or an ext that is the JSON `null`) makes the rebuild fail with Go's
//!   panic text (`assignment to entry in nil map`), and `set_imp` with a wrapper without an
//!   impression stores the wrappers detached and leaves the request's impressions alone. Each
//!   function documents what it does instead.
//! * Errors are [`GoError`]s: `jsonutil`'s `FailedToUnmarshal` and `FailedToMarshal`, or Go's
//!   `errors.New` texts.
//! * A member of an ext map holds JSON (a `RawValue` cannot hold other bytes), where Go's
//!   `json.RawMessage` holds any bytes and writes no value for bytes that are not JSON
//!   (`json.raw-not-json`).

mod prebid_messages;

use core::mem;
use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_gojson::RawJson;
use pbs_jsonutil::{marshal_raw, parse_into_string, unmarshal};
use pbs_openrtb::openrtb2;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

use crate::{
    generated::{
        ConsentedProvidersSettingsIn, ConsentedProvidersSettingsOut, ExtAppPrebid, ExtDevicePrebid,
        ExtImpData, ExtImpPrebid, ExtRegsDSA, ExtRequestPrebid, ExtUserPrebid,
    },
    supply_chain::clone_supply_chain,
};

#[cfg(test)]
mod tests;

/// Go: `jsonEmptyObjectLength`: the length of `{}`; typed members that encode no longer than this
/// are deleted from the ext.
const JSON_EMPTY_OBJECT_LENGTH: usize = 2;
/// Go: `consentedProvidersSettingsStringKey`.
const CONSENTED_PROVIDERS_SETTINGS_STRING_KEY: &str = "ConsentedProvidersSettings";
/// Go: `consentedProvidersSettingsListKey`.
const CONSENTED_PROVIDERS_SETTINGS_LIST_KEY: &str = "consented_providers_settings";
/// Go: `consentKey`.
const CONSENT_KEY: &str = "consent";
/// Go: `ampKey`.
const AMP_KEY: &str = "amp";
/// Go: `dsaKey`.
const DSA_KEY: &str = "dsa";
/// Go: `eidsKey`.
const EIDS_KEY: &str = "eids";
/// Go: `gdprKey`.
const GDPR_KEY: &str = "gdpr";
/// Go: `prebidKey`.
const PREBID_KEY: &str = "prebid";
/// Go: `dataKey`.
const DATA_KEY: &str = "data";
/// Go: `schainKey`.
const SCHAIN_KEY: &str = "schain";
/// Go: `us_privacyKey`.
const US_PRIVACY_KEY: &str = "us_privacy";
/// Go: `cdepKey`.
const CDEP_KEY: &str = "cdep";
/// Go: `gpcKey`.
const GPC_KEY: &str = "gpc";
/// The `tid` key that `ImpExt` reads and writes (a literal in Go).
const TID_KEY: &str = "tid";
/// The `gpid` key that `ImpExt` reads (a literal in Go).
const GPID_KEY: &str = "gpid";

/// The text of Go's runtime panic on a write into a nil map, which a rebuild returns instead.
const NIL_MAP_WRITE: &str = "assignment to entry in nil map";

/// Go's `map[string]json.RawMessage`: an ext object's members, each `None` when nil.
type Members = BTreeMap<String, Option<Box<RawValue>>>;

/// An ext object's map: Go's `map[string]json.RawMessage`, `None` being nil.
///
/// Two maps are equal when they hold the same keys with the same bytes (a `RawValue` has no
/// `PartialEq` of its own).
#[derive(Clone, Debug, Default)]
struct ExtMap(Option<Members>);

impl PartialEq for ExtMap {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|((key_a, a), (key_b, b))| {
                        key_a == key_b
                            && a.as_deref().map(RawValue::get) == b.as_deref().map(RawValue::get)
                    })
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl Eq for ExtMap {}

/// Go's `[]openrtb2.EID`, `None` being nil (the user ext keeps a pointer to one).
type EidSlice = Option<Vec<openrtb2::EID>>;

/// A member of an ext object with the Go `...Dirty` flag that goes with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Tracked<T> {
    /// The member (Go's `ext`, `prebid`, `consent`, ...).
    value: T,
    /// Go's `extDirty`, `prebidDirty`, `consentDirty`, ...: set since decoded or rebuilt.
    dirty: bool,
}

impl<T> Tracked<T> {
    /// A Go setter: `x.member = value; x.memberDirty = true`.
    fn set(&mut self, value: T) {
        self.value = value;
        self.dirty = true;
    }
}

/// Go: `jsonutil.Unmarshal(data, &v)` into a new value: `pbs_jsonutil::unmarshal`, whose
/// `FailedToUnmarshal` keeps Prebid's own messages bare ([`prebid_messages::bare`]).
fn decode<T: DeserializeOwned>(data: &[u8]) -> Result<T, GoError> {
    unmarshal(data).map_err(prebid_messages::bare)
}

/// Go: `jsonutil.Unmarshal(data, v)` into the value `v` points at, which keeps what it held when
/// `data` does not decode (Go's holds what jsoniter read before the fault:
/// `json.partial-results`).
fn decode_into<T: DeserializeOwned>(data: &[u8], v: &mut T) -> Result<(), GoError> {
    *v = decode(data)?;
    Ok(())
}

/// Go: `jsonutil.Marshal(v)`, as the `json.RawMessage` Prebid stores.
fn encode<T: Serialize + ?Sized>(v: &T) -> Result<Box<RawValue>, GoError> {
    marshal_raw(v)
}

/// The bytes of an ORTB object's `ext` (none for nil). The field is pbs-gojson's `RawJson` until
/// the ORTB types take pbs-jsonutil's `Option<Box<RawValue>>`.
fn field_bytes(field: &RawJson) -> &[u8] {
    field.as_bytes()
}

/// An ORTB object's `ext` holding `json`: Go's `json.RawMessage(extJson)`, nil for `None`.
fn field(json: Option<&RawValue>) -> RawJson {
    json.map_or_else(RawJson::nil, |json| RawJson::from(json.get()))
}

/// Go's `ext[key]` on a map that may be nil: `Some(None)` is a nil member.
fn ext_get<'a>(ext: &'a ExtMap, key: &str) -> Option<&'a Option<Box<RawValue>>> {
    ext.0.as_ref().and_then(|map| map.get(key))
}

/// Go's `if json != nil` on a member that may be missing: its bytes when present and not nil.
fn present(member: Option<&Option<Box<RawValue>>>) -> Option<&[u8]> {
    member
        .and_then(Option::as_deref)
        .map(|raw| raw.get().as_bytes())
}

/// Go's `ext[key] = value`, which panics on a nil map.
fn ext_insert(ext: &mut ExtMap, key: &str, value: Box<RawValue>) -> Result<(), GoError> {
    let Some(map) = ext.0.as_mut() else {
        return Err(GoError::generic(NIL_MAP_WRITE));
    };
    let _replaced = map.insert(key.to_owned(), Some(value));
    Ok(())
}

/// Go's `delete(ext, key)` (a no-op on a nil map).
fn ext_delete(ext: &mut ExtMap, key: &str) {
    if let Some(map) = ext.0.as_mut() {
        let _removed = map.remove(key);
    }
}

/// The start of every ext's `unmarshal`: whether the object was already decoded (or changed),
/// in which case Go returns without doing anything. Otherwise the map becomes Go's
/// `make(map[string]json.RawMessage)`.
fn start_unmarshal(dirty: bool, ext: &mut ExtMap) -> bool {
    if ext.0.as_ref().is_some_and(|map| !map.is_empty()) || dirty {
        return true;
    }
    *ext = ExtMap(Some(BTreeMap::new()));
    false
}

/// Go's `jsonutil.Unmarshal(extJson, &x.ext)` into the map `start_unmarshal` made: JSON null makes
/// it nil, and a failed decode leaves it empty (Go's holds the members jsoniter read before the
/// fault: `json.partial-results`).
fn decode_map(ext_json: &[u8], ext: &mut ExtMap) -> Result<(), GoError> {
    ext.0 = decode(ext_json)?;
    Ok(())
}

/// The end of every ext's `marshal`: `extDirty = false`, then nil for an empty map and the
/// `jsonutil.Marshal` of the map otherwise (a nil member written as null, where Go writes no
/// value: `json.nil-raw-written`).
fn finish_marshal(ext: &mut Tracked<ExtMap>) -> Result<Option<Box<RawValue>>, GoError> {
    ext.dirty = false;
    match ext.value.0.as_ref() {
        Some(map) if !map.is_empty() => encode(map).map(Some),
        Some(_) | None => Ok(None),
    }
}

/// Go's write-back of a typed object member: set when `value` encodes longer than `{}`
/// ([`JSON_EMPTY_OBJECT_LENGTH`]), deleted when it does not or is nil.
fn marshal_object_member<T: Serialize + ?Sized>(
    ext: &mut ExtMap,
    key: &str,
    value: Option<&T>,
) -> Result<(), GoError> {
    if let Some(value) = value {
        let json = encode(value)?;
        if json.get().len() > JSON_EMPTY_OBJECT_LENGTH {
            return ext_insert(ext, key, json);
        }
    }
    ext_delete(ext, key);
    Ok(())
}

/// Go's write-back of any other member: set to its encoding when `value` is `Some`, deleted
/// otherwise.
fn marshal_member<T: Serialize + ?Sized>(
    ext: &mut ExtMap,
    key: &str,
    value: Option<&T>,
) -> Result<(), GoError> {
    if let Some(value) = value {
        return ext_insert(ext, key, encode(value)?);
    }
    ext_delete(ext, key);
    Ok(())
}

/// What [`RequestWrapper`] needs of each ext object: Go's unexported `unmarshal` and `marshal`
/// methods, and `Dirty`.
trait WrappedExt: Default {
    /// Go: `unmarshal(extJson)`.
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError>;
    /// Go: `marshal()`: nil when the map ends up empty.
    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError>;
    /// Go: `Dirty()`.
    fn is_dirty(&self) -> bool;
}

/// Go's `GetXExt` body: the stored object, or a new one decoded from `raw` (an absent ext
/// decodes as empty), stored also when decoding fails.
fn ext_pair<'a, T: WrappedExt>(
    slot: &'a mut Option<Box<T>>,
    raw: Option<&RawJson>,
) -> (&'a mut T, Option<GoError>) {
    let mut err = None;
    if slot.is_none() {
        let mut ext = Box::<T>::default();
        err = ext.unmarshal(raw.map_or(&[], field_bytes)).err();
        *slot = Some(ext);
    }
    (slot.get_or_insert_with(Box::default), err)
}

/// Go's `(ext, err)` pair as a `Result`.
fn pair_result<T>((ext, err): (T, Option<GoError>)) -> Result<T, GoError> {
    err.map_or(Ok(ext), Err)
}

/// What Go's `rebuildXExt` does with the owner's `ext` field.
enum Rebuilt {
    /// The ext object is absent or clean: the field keeps its bytes.
    Kept,
    /// The ext object was dirty: the field becomes its `marshal`, `None` for Go's nil.
    Written(Option<Box<RawValue>>),
}

/// Go's `rebuildXExt` test: [`Rebuilt::Kept`] when the object is absent or clean, otherwise
/// its `marshal`.
fn rebuild_ext<T: WrappedExt>(ext: Option<&mut T>) -> Result<Rebuilt, GoError> {
    match ext {
        Some(ext) if ext.is_dirty() => ext.marshal().map(Rebuilt::Written),
        Some(_) | None => Ok(Rebuilt::Kept),
    }
}

/// Go's `if json != nil && owner == nil { owner = &T{Ext: json} } else if owner != nil {
/// owner.Ext = json }`, for an owner object whose `ext` field `ext` returns.
fn set_owner_ext<T: Default>(
    owner: &mut Option<Box<T>>,
    json: Option<&RawValue>,
    ext: impl FnOnce(&mut T) -> &mut RawJson,
) {
    match owner.as_deref_mut() {
        Some(owner) => *ext(owner) = field(json),
        None if json.is_some() => {
            let mut created = T::default();
            *ext(&mut created) = field(json);
            *owner = Some(Box::new(created));
        }
        None => {}
    }
}

/// Defines Go's `GetExt` and `SetExt` of an ext object.
macro_rules! ext_map_accessors {
    ($go:literal) => {
        #[doc = concat!(
            "Go: `", $go, ".GetExt()`: a copy of the ext's members (empty, never nil, when the ",
            "map is nil). A member is `None` when it is nil (JSON null)."
        )]
        #[must_use]
        pub fn get_ext(&self) -> BTreeMap<String, Option<Box<RawValue>>> {
            self.ext.value.0.clone().unwrap_or_default()
        }

        #[doc = concat!(
            "Go: `", $go, ".SetExt(ext)`: replaces the ext's members and marks them dirty.\n\n",
            "Members that have their own setter are overwritten by it when the ext is rebuilt. ",
            "`None` is Go's nil map, which a later write of such a member cannot go into (the ",
            "rebuild then fails, where Go panics)."
        )]
        pub fn set_ext(&mut self, ext: Option<BTreeMap<String, Option<Box<RawValue>>>>) {
            self.ext.set(ExtMap(ext));
        }
    };
}

/// Where the impression of a request wrapper's [`ImpWrapperSlot`] is.
#[derive(Clone, Debug, Default, PartialEq)]
enum ImpLoc {
    /// Go's `&rw.Imp[i]`, `i` being the slot's index.
    #[default]
    Request,
    /// An impression the request no longer holds (or nil): Go's wrapper still points at the
    /// element of a replaced `rw.Imp`, or at an impression it was given.
    Detached(Option<Box<openrtb2::Imp>>),
}

/// One of a request wrapper's Go `*ImpWrapper`s.
#[derive(Clone, Debug, Default, PartialEq)]
struct ImpWrapperSlot {
    /// Go's `Imp` pointer.
    imp: ImpLoc,
    /// Go's `impExt`.
    imp_ext: Option<Box<ImpExt>>,
}

impl ImpWrapperSlot {
    /// The view of the slot, given the request's impression at the slot's index.
    fn view<'a>(&'a mut self, request_imp: Option<&'a mut openrtb2::Imp>) -> ImpWrapperMut<'a> {
        let Self { imp, imp_ext } = self;
        let imp = match imp {
            ImpLoc::Request => request_imp,
            ImpLoc::Detached(imp) => imp.as_deref_mut(),
        };
        ImpWrapperMut { imp, imp_ext }
    }

    /// Whether the slot has an impression, given whether the request has one at its index.
    const fn has_imp(&self, request_has_imp: bool) -> bool {
        match &self.imp {
            ImpLoc::Request => request_has_imp,
            ImpLoc::Detached(imp) => imp.is_some(),
        }
    }
}

/// Points the wrappers that refer to the request's impressions at `old`, the impressions the
/// request no longer holds (Go's wrappers keep pointing at a replaced `rw.Imp`).
fn detach_request_slots(
    wrappers: &mut Option<Vec<ImpWrapperSlot>>,
    old: Option<Vec<openrtb2::Imp>>,
) {
    let Some(slots) = wrappers.as_mut() else {
        return;
    };
    let mut old_imps = old.into_iter().flatten();
    for slot in slots {
        let old_imp = old_imps.next();
        if slot.imp == ImpLoc::Request {
            slot.imp = ImpLoc::Detached(old_imp.map(Box::new));
        }
    }
}

/// Go's `RebuildImp` of a wrapper: an error for a nil impression, otherwise the ext object, when
/// dirty, encoded into the impression's `ext`.
fn rebuild_imp_parts(
    imp: Option<&mut openrtb2::Imp>,
    imp_ext: &mut Option<Box<ImpExt>>,
) -> Result<(), GoError> {
    // Go: openrtb_ext/request_wrapper.go:1644
    let Some(imp) = imp else {
        return Err(GoError::generic(
            "ImpWrapper RebuildImp called on a nil Imp",
        ));
    };
    // Go: openrtb_ext/request_wrapper.go:1656
    if let Rebuilt::Written(json) = rebuild_ext(imp_ext.as_deref_mut())? {
        imp.ext = field(json.as_deref());
    }
    Ok(())
}

/// Go's `GetImpExt` of a wrapper whose impression is `imp` (a nil impression decodes as an
/// absent ext, as in Go).
fn imp_ext_pair<'a>(
    imp: Option<&openrtb2::Imp>,
    imp_ext: &'a mut Option<Box<ImpExt>>,
) -> (&'a mut ImpExt, Option<GoError>) {
    // Go: openrtb_ext/request_wrapper.go:1633
    ext_pair(imp_ext, imp.map(|imp| &imp.ext))
}

/// Go: `RequestWrapper`: an ORTB request whose ext objects are decoded once, on first use.
///
/// Wrap a request with [`RequestWrapper::new`], get an ext object with one of the `get_*_ext`
/// methods, read and change it with its getters and setters, and write the changes back into the
/// request with [`RequestWrapper::rebuild_request`]. Members with their own setter should be
/// changed with it rather than through `set_ext`, which the setter overwrites on rebuild.
///
/// The methods are not thread safe, as in Go.
#[derive(Debug, Default, PartialEq)]
pub struct RequestWrapper {
    /// Go's embedded `*openrtb2.BidRequest`.
    bid_request: Option<Box<openrtb2::BidRequest>>,
    /// Go's `impWrappers`.
    imp_wrappers: Option<Vec<ImpWrapperSlot>>,
    /// Go's `impWrappersAccessed`.
    imp_wrappers_accessed: bool,
    /// Go's `userExt`.
    user_ext: Option<Box<UserExt>>,
    /// Go's `deviceExt`.
    device_ext: Option<Box<DeviceExt>>,
    /// Go's `requestExt`.
    request_ext: Option<Box<RequestExt>>,
    /// Go's `appExt`.
    app_ext: Option<Box<AppExt>>,
    /// Go's `regExt`.
    reg_ext: Option<Box<RegExt>>,
    /// Go's `siteExt`.
    site_ext: Option<Box<SiteExt>>,
    /// Go's `doohExt`.
    dooh_ext: Option<Box<DOOHExt>>,
    /// Go's `sourceExt`.
    source_ext: Option<Box<SourceExt>>,
}

impl Clone for RequestWrapper {
    /// Go: `(*RequestWrapper).Clone()`: the ext objects and the impression wrappers are copied
    /// with their Go `Clone`, and a nil wrapper slice becomes an empty one, as in Go. The request
    /// is copied too (Go's clone shares it with the original).
    fn clone(&self) -> Self {
        // Go: openrtb_ext/request_wrapper.go:404
        Self {
            bid_request: self.bid_request.clone(),
            imp_wrappers: Some(self.imp_wrappers.iter().flatten().cloned().collect()),
            imp_wrappers_accessed: self.imp_wrappers_accessed,
            user_ext: self.user_ext.clone(),
            device_ext: self.device_ext.clone(),
            request_ext: self.request_ext.clone(),
            app_ext: self.app_ext.clone(),
            reg_ext: self.reg_ext.clone(),
            site_ext: self.site_ext.clone(),
            dooh_ext: self.dooh_ext.clone(),
            source_ext: self.source_ext.clone(),
        }
    }
}

impl RequestWrapper {
    /// Go: `&RequestWrapper{BidRequest: req}`.
    #[must_use]
    pub fn new(bid_request: openrtb2::BidRequest) -> Self {
        Self::from_bid_request(Some(Box::new(bid_request)))
    }

    /// Go: `&RequestWrapper{BidRequest: req}` for a request that may be nil.
    #[must_use]
    pub fn from_bid_request(bid_request: Option<Box<openrtb2::BidRequest>>) -> Self {
        Self {
            bid_request,
            ..Self::default()
        }
    }

    /// Go's embedded `BidRequest` (`None` is nil).
    #[must_use]
    pub fn bid_request(&self) -> Option<&openrtb2::BidRequest> {
        self.bid_request.as_deref()
    }

    /// Go's embedded `BidRequest`, to change in place.
    ///
    /// Changes of an impression are seen by the impression wrappers that refer to it. To replace
    /// the `imp` slice use [`RequestWrapper::assign_imp`]: a slice whose length changes in place
    /// keeps the wrappers at their positions, where Go's keep the old slice.
    pub fn bid_request_mut(&mut self) -> Option<&mut openrtb2::BidRequest> {
        self.bid_request.as_deref_mut()
    }

    /// Go: `rw.BidRequest = req`.
    ///
    /// Wrappers that referred to the replaced request's impressions keep them (detached), as Go's
    /// keep pointing at the old request.
    pub fn set_bid_request(&mut self, bid_request: Option<Box<openrtb2::BidRequest>>) {
        let old = mem::replace(&mut self.bid_request, bid_request);
        detach_request_slots(
            &mut self.imp_wrappers,
            old.and_then(|mut old| old.imp.take()),
        );
    }

    /// The wrapped request (`None` is nil), dropping the wrapper.
    #[must_use]
    pub fn into_bid_request(self) -> Option<Box<openrtb2::BidRequest>> {
        self.bid_request
    }

    /// Go: `rw.Imp = imps`.
    ///
    /// Wrappers that referred to the replaced impressions keep them (detached), as Go's keep
    /// pointing at the old slice; the next [`RequestWrapper::rebuild_request`] or
    /// [`RequestWrapper::set_imp_from_get_imp`] copies them back into the request. Nothing
    /// happens on a nil request (where Go panics).
    pub fn assign_imp(&mut self, imps: Option<Vec<openrtb2::Imp>>) {
        let Some(req) = self.bid_request.as_deref_mut() else {
            return;
        };
        let old = mem::replace(&mut req.imp, imps);
        detach_request_slots(&mut self.imp_wrappers, old);
    }

    /// Go: `LenImp()`: the number of impressions, without creating the impression wrappers.
    ///
    /// That is the number of wrappers once they exist, otherwise of the request's impressions (0
    /// for a nil request, where Go panics).
    #[must_use]
    pub fn len_imp(&self) -> usize {
        // Go: openrtb_ext/request_wrapper.go:68
        if self.imp_wrappers_accessed {
            return self.imp_wrappers.as_ref().map_or(0, Vec::len);
        }
        self.bid_request
            .as_deref()
            .and_then(|req| req.imp.as_ref())
            .map_or(0, Vec::len)
    }

    /// Go's `GetImp` without its result: creates a wrapper for each of the request's impressions
    /// the first time (none for a nil `imp`, or a nil request, where Go panics).
    fn access_imp_wrappers(&mut self) {
        // Go: openrtb_ext/request_wrapper.go:76
        if self.imp_wrappers_accessed {
            return;
        }
        if let Some(imps) = self.bid_request.as_deref().and_then(|req| req.imp.as_ref()) {
            self.imp_wrappers = Some(vec![ImpWrapperSlot::default(); imps.len()]);
        }
        self.imp_wrappers_accessed = true;
    }

    /// Go: `GetImp()`: the impression wrappers, created for the request's impressions the first
    /// time.
    ///
    /// Each wrapper refers to the request's impression at its index (so that changes through it
    /// change the request), or to the impression it was detached with.
    pub fn get_imp(&mut self) -> Vec<ImpWrapperMut<'_>> {
        self.access_imp_wrappers();
        let request_imps = self
            .bid_request
            .as_deref_mut()
            .and_then(|req| req.imp.as_mut());
        let mut request_imps = request_imps.into_iter().flatten();
        self.imp_wrappers
            .iter_mut()
            .flatten()
            .map(|slot| slot.view(request_imps.next()))
            .collect()
    }

    /// Go: `SetImp(imps)`: the wrappers become the request's wrappers, and their impressions
    /// (moved into a new slice) the request's impressions.
    ///
    /// `None` (Go's nil) leaves no wrappers and an empty, non-nil impression slice. Where Go
    /// panics (a nil request, or a wrapper without an impression), the wrappers are stored as Go
    /// stores them before panicking, keeping their impressions, and the request's impressions are
    /// left alone.
    pub fn set_imp(&mut self, imps: Option<Vec<ImpWrapper>>) {
        // Go: openrtb_ext/request_wrapper.go:96
        let Some(wrappers) = imps else {
            self.imp_wrappers = None;
            if let Some(req) = self.bid_request.as_deref_mut() {
                req.imp = Some(Vec::new());
                self.imp_wrappers_accessed = true;
            }
            return;
        };
        let Some(req) = self
            .bid_request
            .as_deref_mut()
            .filter(|_| wrappers.iter().all(|wrapper| wrapper.imp.is_some()))
        else {
            self.imp_wrappers = Some(
                wrappers
                    .into_iter()
                    .map(|wrapper| ImpWrapperSlot {
                        imp: ImpLoc::Detached(wrapper.imp),
                        imp_ext: wrapper.imp_ext,
                    })
                    .collect(),
            );
            return;
        };
        let mut imp_arr = Vec::with_capacity(wrappers.len());
        let mut slots = Vec::with_capacity(wrappers.len());
        for ImpWrapper { imp, imp_ext } in wrappers {
            imp_arr.extend(imp.map(|imp| *imp));
            slots.push(ImpWrapperSlot {
                imp: ImpLoc::Request,
                imp_ext,
            });
        }
        req.imp = Some(imp_arr);
        self.imp_wrappers = Some(slots);
        self.imp_wrappers_accessed = true;
    }

    /// Go: `rw.SetImp(rw.GetImp())`: the wrappers' impressions (the request's own, or those they
    /// were detached with) become a new impression slice of the request, which the wrappers then
    /// refer to.
    ///
    /// Nothing changes on a nil request or when a wrapper has no impression (where Go panics).
    pub fn set_imp_from_get_imp(&mut self) {
        self.access_imp_wrappers();
        let Some(req) = self.bid_request.as_deref_mut() else {
            return;
        };
        let Some(slots) = self.imp_wrappers.as_mut() else {
            // Go: SetImp(nil)
            req.imp = Some(Vec::new());
            return;
        };
        let request_len = req.imp.as_ref().map_or(0, Vec::len);
        let complete = slots
            .iter()
            .enumerate()
            .all(|(i, slot)| slot.has_imp(i < request_len));
        if !complete {
            return;
        }
        let mut old_imps = req.imp.take().into_iter().flatten();
        let mut imp_arr = Vec::with_capacity(slots.len());
        for slot in slots.iter_mut() {
            let old_imp = old_imps.next();
            let imp = match mem::take(&mut slot.imp) {
                ImpLoc::Request => old_imp,
                ImpLoc::Detached(imp) => imp.map(|imp| *imp),
            };
            imp_arr.extend(imp);
        }
        req.imp = Some(imp_arr);
    }

    /// Go: `GetUserExt()`: the `user.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call (`FailedToUnmarshal`); the object is stored anyway,
    /// and later calls return it (see [`RequestWrapper::get_user_ext_pair`]).
    pub fn get_user_ext(&mut self) -> Result<&mut UserExt, GoError> {
        pair_result(self.get_user_ext_pair())
    }

    /// Go: `GetUserExt()` with both results: the object, also when decoding it failed, and the
    /// error of the first call.
    pub fn get_user_ext_pair(&mut self) -> (&mut UserExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:107
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.user.as_deref())
            .map(|user| &user.ext);
        ext_pair(&mut self.user_ext, raw)
    }

    /// Go: `GetDeviceExt()`: the `device.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`RequestWrapper::get_user_ext`].
    pub fn get_device_ext(&mut self) -> Result<&mut DeviceExt, GoError> {
        pair_result(self.get_device_ext_pair())
    }

    /// Go: `GetDeviceExt()` with both results.
    pub fn get_device_ext_pair(&mut self) -> (&mut DeviceExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:119
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.device.as_deref())
            .map(|device| &device.ext);
        ext_pair(&mut self.device_ext, raw)
    }

    /// Go: `GetRequestExt()`: the request's `ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`RequestWrapper::get_user_ext`].
    pub fn get_request_ext(&mut self) -> Result<&mut RequestExt, GoError> {
        pair_result(self.get_request_ext_pair())
    }

    /// Go: `GetRequestExt()` with both results.
    pub fn get_request_ext_pair(&mut self) -> (&mut RequestExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:130
        let raw = self.bid_request.as_deref().map(|req| &req.ext);
        ext_pair(&mut self.request_ext, raw)
    }

    /// Go: `GetAppExt()`: the `app.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`RequestWrapper::get_user_ext`].
    pub fn get_app_ext(&mut self) -> Result<&mut AppExt, GoError> {
        pair_result(self.get_app_ext_pair())
    }

    /// Go: `GetAppExt()` with both results.
    pub fn get_app_ext_pair(&mut self) -> (&mut AppExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:141
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.app.as_deref())
            .map(|app| &app.ext);
        ext_pair(&mut self.app_ext, raw)
    }

    /// Go: `GetRegExt()`: the `regs.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call (`FailedToUnmarshal`, or `gdpr must be an
    /// integer`); see [`RequestWrapper::get_user_ext`].
    pub fn get_reg_ext(&mut self) -> Result<&mut RegExt, GoError> {
        pair_result(self.get_reg_ext_pair())
    }

    /// Go: `GetRegExt()` with both results.
    pub fn get_reg_ext_pair(&mut self) -> (&mut RegExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:152
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.regs.as_deref())
            .map(|regs| &regs.ext);
        ext_pair(&mut self.reg_ext, raw)
    }

    /// Go: `GetSiteExt()`: the `site.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call (`FailedToUnmarshal`, or `request.site.ext.amp must
    /// be either 1, 0, or undefined`); see [`RequestWrapper::get_user_ext`].
    pub fn get_site_ext(&mut self) -> Result<&mut SiteExt, GoError> {
        pair_result(self.get_site_ext_pair())
    }

    /// Go: `GetSiteExt()` with both results.
    pub fn get_site_ext_pair(&mut self) -> (&mut SiteExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:163
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.site.as_deref())
            .map(|site| &site.ext);
        ext_pair(&mut self.site_ext, raw)
    }

    /// Go: `GetDOOHExt()`: the `dooh.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`RequestWrapper::get_user_ext`].
    pub fn get_dooh_ext(&mut self) -> Result<&mut DOOHExt, GoError> {
        pair_result(self.get_dooh_ext_pair())
    }

    /// Go: `GetDOOHExt()` with both results.
    pub fn get_dooh_ext_pair(&mut self) -> (&mut DOOHExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:174
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.dooh.as_deref())
            .map(|dooh| &dooh.ext);
        ext_pair(&mut self.dooh_ext, raw)
    }

    /// Go: `GetSourceExt()`: the `source.ext` object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`RequestWrapper::get_user_ext`].
    pub fn get_source_ext(&mut self) -> Result<&mut SourceExt, GoError> {
        pair_result(self.get_source_ext_pair())
    }

    /// Go: `GetSourceExt()` with both results.
    pub fn get_source_ext_pair(&mut self) -> (&mut SourceExt, Option<GoError>) {
        // Go: openrtb_ext/request_wrapper.go:185
        let raw = self
            .bid_request
            .as_deref()
            .and_then(|req| req.source.as_deref())
            .map(|source| &source.ext);
        ext_pair(&mut self.source_ext, raw)
    }

    /// Go: `RebuildRequest()`: writes the impression wrappers and every dirty ext object back
    /// into the request.
    ///
    /// The order is Go's: impressions, user, device, request, app, regs, site, DOOH, source. An
    /// ext that ends up empty is removed (nil); an object that is absent from the request is
    /// created for an ext that is not.
    ///
    /// # Errors
    ///
    /// `Requestwrapper RebuildRequest called on a nil BidRequest`; the first wrapper's or
    /// object's error, after which nothing else is rebuilt: `ImpWrapper RebuildImp called on a
    /// nil Imp`, a `FailedToMarshal`, or Go's nil map panic text.
    pub fn rebuild_request(&mut self) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:196
        let Some(req) = self.bid_request.as_deref_mut() else {
            return Err(GoError::generic(
                "Requestwrapper RebuildRequest called on a nil BidRequest",
            ));
        };
        rebuild_imp(req, self.imp_wrappers_accessed, &mut self.imp_wrappers)?;
        // Go: openrtb_ext/request_wrapper.go:254
        if let Rebuilt::Written(json) = rebuild_ext(self.user_ext.as_deref_mut())? {
            set_owner_ext(&mut req.user, json.as_deref(), |user| &mut user.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:273
        if let Rebuilt::Written(json) = rebuild_ext(self.device_ext.as_deref_mut())? {
            set_owner_ext(&mut req.device, json.as_deref(), |device| &mut device.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:292
        if let Rebuilt::Written(json) = rebuild_ext(self.request_ext.as_deref_mut())? {
            req.ext = field(json.as_deref());
        }
        // Go: openrtb_ext/request_wrapper.go:307
        if let Rebuilt::Written(json) = rebuild_ext(self.app_ext.as_deref_mut())? {
            set_owner_ext(&mut req.app, json.as_deref(), |app| &mut app.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:345
        if let Rebuilt::Written(json) = rebuild_ext(self.reg_ext.as_deref_mut())? {
            set_owner_ext(&mut req.regs, json.as_deref(), |regs| &mut regs.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:364
        if let Rebuilt::Written(json) = rebuild_ext(self.site_ext.as_deref_mut())? {
            set_owner_ext(&mut req.site, json.as_deref(), |site| &mut site.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:326
        if let Rebuilt::Written(json) = rebuild_ext(self.dooh_ext.as_deref_mut())? {
            set_owner_ext(&mut req.dooh, json.as_deref(), |dooh| &mut dooh.ext);
        }
        // Go: openrtb_ext/request_wrapper.go:383
        if let Rebuilt::Written(json) = rebuild_ext(self.source_ext.as_deref_mut())? {
            set_owner_ext(&mut req.source, json.as_deref(), |source| &mut source.ext);
        }
        Ok(())
    }

    /// Go: `CloneAndClearImpWrappers()`: a copy without impression wrappers, whose ext objects are
    /// Go clones of this wrapper's.
    ///
    /// This wrapper's impression wrappers are marked as not accessed, so that the next
    /// [`RequestWrapper::get_imp`] creates them anew from the request's impressions. The copy has
    /// its own copy of the request (Go's shares it).
    #[must_use]
    pub fn clone_and_clear_imp_wrappers(&mut self) -> Self {
        let bid_request = self.bid_request.clone();
        self.clone_and_clear_imp_wrappers_with(bid_request)
    }

    /// Go: `c := rw.CloneAndClearImpWrappers(); c.BidRequest = bidRequest`, without copying the
    /// request that is then replaced.
    #[must_use]
    pub fn clone_and_clear_imp_wrappers_with(
        &mut self,
        bid_request: Option<Box<openrtb2::BidRequest>>,
    ) -> Self {
        // Go: openrtb_ext/request_wrapper.go:426
        self.imp_wrappers_accessed = false;
        Self {
            bid_request,
            imp_wrappers: None,
            imp_wrappers_accessed: false,
            user_ext: self.user_ext.clone(),
            device_ext: self.device_ext.clone(),
            request_ext: self.request_ext.clone(),
            app_ext: self.app_ext.clone(),
            reg_ext: self.reg_ext.clone(),
            site_ext: self.site_ext.clone(),
            dooh_ext: self.dooh_ext.clone(),
            source_ext: self.source_ext.clone(),
        }
    }
}

/// Go's `rebuildImp`: when the wrappers were accessed, the request's impressions become a new
/// slice of the wrappers' rebuilt impressions (nil for nil wrappers), which the wrappers then
/// refer to.
///
/// On a wrapper's error the new slice is already in place, with the impressions before it and
/// zero impressions from it on; the wrappers from it on keep their impressions (detached), as
/// Go's keep pointing at the old ones.
fn rebuild_imp(
    req: &mut openrtb2::BidRequest,
    accessed: bool,
    wrappers: &mut Option<Vec<ImpWrapperSlot>>,
) -> Result<(), GoError> {
    // Go: openrtb_ext/request_wrapper.go:232
    if !accessed {
        return Ok(());
    }
    let Some(slots) = wrappers.as_mut() else {
        req.imp = None;
        return Ok(());
    };
    let mut old = req.imp.take().unwrap_or_default();
    let mut imp_arr = Vec::with_capacity(slots.len());
    let mut failure = None;
    for (i, slot) in slots.iter_mut().enumerate() {
        let ImpWrapperSlot { imp, imp_ext } = slot;
        let current = match imp {
            ImpLoc::Request => old.get_mut(i),
            ImpLoc::Detached(imp) => imp.as_deref_mut(),
        };
        if let Err(err) = rebuild_imp_parts(current, imp_ext) {
            failure = Some((i, err));
            break;
        }
        let rebuilt = match mem::take(imp) {
            ImpLoc::Request => old.get_mut(i).map(mem::take),
            ImpLoc::Detached(imp) => imp.map(|imp| *imp),
        };
        imp_arr.push(rebuilt.unwrap_or_default());
    }
    if let Some((failed, err)) = failure {
        for (i, slot) in slots.iter_mut().enumerate().skip(failed) {
            if slot.imp == ImpLoc::Request {
                slot.imp = ImpLoc::Detached(old.get_mut(i).map(mem::take).map(Box::new));
            }
        }
        imp_arr.resize_with(slots.len(), openrtb2::Imp::default);
        req.imp = Some(imp_arr);
        return Err(err);
    }
    req.imp = Some(imp_arr);
    Ok(())
}

/// Go: `ImpWrapper`: an impression whose `ext` is decoded once, on first use.
///
/// This is a wrapper of its own (Go's `&ImpWrapper{Imp: &imp}`); a request's wrappers are
/// [`ImpWrapperMut`] views (see [`RequestWrapper::get_imp`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImpWrapper {
    /// Go's embedded `*openrtb2.Imp`.
    imp: Option<Box<openrtb2::Imp>>,
    /// Go's `impExt`.
    imp_ext: Option<Box<ImpExt>>,
}

impl ImpWrapper {
    /// Go: `&ImpWrapper{Imp: &imp}`.
    #[must_use]
    pub fn new(imp: openrtb2::Imp) -> Self {
        Self::from_imp(Some(Box::new(imp)))
    }

    /// Go: `&ImpWrapper{Imp: imp}` for an impression that may be nil.
    #[must_use]
    pub const fn from_imp(imp: Option<Box<openrtb2::Imp>>) -> Self {
        Self { imp, imp_ext: None }
    }

    /// Go's embedded `Imp` (`None` is nil).
    #[must_use]
    pub fn imp(&self) -> Option<&openrtb2::Imp> {
        self.imp.as_deref()
    }

    /// Go's embedded `Imp`, to change in place.
    pub fn imp_mut(&mut self) -> Option<&mut openrtb2::Imp> {
        self.imp.as_deref_mut()
    }

    /// The impression, dropping the wrapper.
    #[must_use]
    pub fn into_imp(self) -> Option<Box<openrtb2::Imp>> {
        self.imp
    }

    /// The wrapper as the view that functions taking Go's `*ImpWrapper` take.
    pub fn view(&mut self) -> ImpWrapperMut<'_> {
        ImpWrapperMut {
            imp: self.imp.as_deref_mut(),
            imp_ext: &mut self.imp_ext,
        }
    }

    /// Go: `GetImpExt()`: the impression's ext object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call; see [`ImpWrapperMut::get_imp_ext`].
    pub fn get_imp_ext(&mut self) -> Result<&mut ImpExt, GoError> {
        pair_result(self.get_imp_ext_pair())
    }

    /// Go: `GetImpExt()` with both results.
    pub fn get_imp_ext_pair(&mut self) -> (&mut ImpExt, Option<GoError>) {
        imp_ext_pair(self.imp.as_deref(), &mut self.imp_ext)
    }

    /// Go: `RebuildImp()`: the ext object, when dirty, encoded into the impression's `ext`.
    ///
    /// # Errors
    ///
    /// `ImpWrapper RebuildImp called on a nil Imp`, or the ext object's encoding error.
    pub fn rebuild_imp(&mut self) -> Result<(), GoError> {
        rebuild_imp_parts(self.imp.as_deref_mut(), &mut self.imp_ext)
    }
}

/// Go's `*ImpWrapper` of a request ([`RequestWrapper::get_imp`]) or of an [`ImpWrapper`]
/// ([`ImpWrapper::view`]): the wrapper's impression and its ext object, borrowed.
#[derive(Debug)]
pub struct ImpWrapperMut<'a> {
    /// The wrapper's impression (`None` is Go's nil `Imp`).
    imp: Option<&'a mut openrtb2::Imp>,
    /// Go's `impExt`.
    imp_ext: &'a mut Option<Box<ImpExt>>,
}

impl ImpWrapperMut<'_> {
    /// Go's embedded `Imp` (`None` is nil).
    #[must_use]
    pub fn imp(&self) -> Option<&openrtb2::Imp> {
        self.imp.as_deref()
    }

    /// Go's embedded `Imp`, to change in place.
    pub fn imp_mut(&mut self) -> Option<&mut openrtb2::Imp> {
        self.imp.as_deref_mut()
    }

    /// Go: `GetImpExt()`: the impression's ext object, decoded the first time.
    ///
    /// # Errors
    ///
    /// The decoding error of the first call (`FailedToUnmarshal`); the object is stored anyway,
    /// and later calls return it (see [`ImpWrapperMut::get_imp_ext_pair`]).
    pub fn get_imp_ext(&mut self) -> Result<&mut ImpExt, GoError> {
        pair_result(self.get_imp_ext_pair())
    }

    /// Go: `GetImpExt()` with both results: the object, also when decoding it failed, and the
    /// error of the first call.
    pub fn get_imp_ext_pair(&mut self) -> (&mut ImpExt, Option<GoError>) {
        imp_ext_pair(self.imp.as_deref(), self.imp_ext)
    }

    /// Go: `RebuildImp()`: the ext object, when dirty, encoded into the impression's `ext`.
    ///
    /// # Errors
    ///
    /// `ImpWrapper RebuildImp called on a nil Imp`, or the ext object's encoding error.
    pub fn rebuild_imp(&mut self) -> Result<(), GoError> {
        rebuild_imp_parts(self.imp.as_deref_mut(), self.imp_ext)
    }

    /// Go: `Clone()`: a wrapper of its own with a copy of the impression (Go's shares it) and a
    /// Go clone of the ext object.
    #[must_use]
    pub fn to_imp_wrapper(&self) -> ImpWrapper {
        // Go: openrtb_ext/request_wrapper.go:1671
        ImpWrapper {
            imp: self.imp.as_deref().cloned().map(Box::new),
            imp_ext: self.imp_ext.clone(),
        }
    }
}

/// Go: `UserExt`: the decoded `user.ext`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UserExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `consent` and `consentDirty`.
    consent: Tracked<Option<String>>,
    /// Go's `prebid` and `prebidDirty`.
    prebid: Tracked<Option<ExtUserPrebid>>,
    /// Go's `eids` (`*[]openrtb2.EID`) and `eidsDirty`.
    eids: Tracked<Option<EidSlice>>,
    /// Go's `consentedProvidersSettingsIn` and `consentedProvidersSettingsInDirty`.
    consented_providers_settings_in: Tracked<Option<ConsentedProvidersSettingsIn>>,
    /// Go's `consentedProvidersSettingsOut` and `consentedProvidersSettingsOutDirty`.
    consented_providers_settings_out: Tracked<Option<ConsentedProvidersSettingsOut>>,
}

impl WrappedExt for UserExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:465
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        let ext = &self.ext.value;
        if let Some(consent_json) = present(ext_get(ext, CONSENT_KEY)) {
            decode_into(consent_json, &mut self.consent.value)?;
        }
        if let Some(prebid_json) = ext_get(ext, PREBID_KEY) {
            let prebid = self.prebid.value.insert(ExtUserPrebid::default());
            if let Some(prebid_json) = present(Some(prebid_json)) {
                decode_into(prebid_json, prebid)?;
            }
        }
        if let Some(eids_json) = ext_get(ext, EIDS_KEY) {
            let eids = self.eids.value.insert(Some(Vec::new()));
            if let Some(eids_json) = present(Some(eids_json)) {
                decode_into(eids_json, eids)?;
            }
        }
        if let Some(cps_json) = ext_get(ext, CONSENTED_PROVIDERS_SETTINGS_STRING_KEY) {
            let cps = self
                .consented_providers_settings_in
                .value
                .insert(ConsentedProvidersSettingsIn::default());
            if let Some(cps_json) = present(Some(cps_json)) {
                decode_into(cps_json, cps)?;
            }
        }
        if let Some(cps_json) = ext_get(ext, CONSENTED_PROVIDERS_SETTINGS_LIST_KEY) {
            let cps = self
                .consented_providers_settings_out
                .value
                .insert(ConsentedProvidersSettingsOut::default());
            if let Some(cps_json) = present(Some(cps_json)) {
                decode_into(cps_json, cps)?;
            }
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:530
        let ext = &mut self.ext.value;
        if self.consent.dirty {
            let consent = self
                .consent
                .value
                .as_ref()
                .filter(|consent| !consent.is_empty());
            marshal_member(ext, CONSENT_KEY, consent)?;
            self.consent.dirty = false;
        }
        if self.prebid.dirty {
            marshal_object_member(ext, PREBID_KEY, self.prebid.value.as_ref())?;
            self.prebid.dirty = false;
        }
        if self.consented_providers_settings_in.dirty {
            let cps = self.consented_providers_settings_in.value.as_ref();
            marshal_object_member(ext, CONSENTED_PROVIDERS_SETTINGS_STRING_KEY, cps)?;
            self.consented_providers_settings_in.dirty = false;
        }
        if self.consented_providers_settings_out.dirty {
            let cps = self.consented_providers_settings_out.value.as_ref();
            marshal_object_member(ext, CONSENTED_PROVIDERS_SETTINGS_LIST_KEY, cps)?;
            self.consented_providers_settings_out.dirty = false;
        }
        if self.eids.dirty {
            let eids = self
                .eids
                .value
                .as_ref()
                .and_then(Option::as_ref)
                .filter(|eids| !eids.is_empty());
            marshal_member(ext, EIDS_KEY, eids)?;
            self.eids.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl Clone for UserExt {
    /// Go: `(*UserExt).Clone()`, which turns a nil `eids` slice into an empty one.
    fn clone(&self) -> Self {
        // Go: openrtb_ext/request_wrapper.go:710
        Self {
            ext: self.ext.clone(),
            consent: self.consent.clone(),
            prebid: self.prebid.clone(),
            eids: Tracked {
                value: self
                    .eids
                    .value
                    .as_ref()
                    .map(|eids| Some(eids.clone().unwrap_or_default())),
                dirty: self.eids.dirty,
            },
            consented_providers_settings_in: self.consented_providers_settings_in.clone(),
            consented_providers_settings_out: self.consented_providers_settings_out.clone(),
        }
    }
}

impl UserExt {
    ext_map_accessors!("UserExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:615
        self.ext.dirty
            || self.eids.dirty
            || self.prebid.dirty
            || self.consent.dirty
            || self.consented_providers_settings_in.dirty
            || self.consented_providers_settings_out.dirty
    }

    /// Go: `GetConsent()`: `user.ext.consent` (`None` is nil).
    #[must_use]
    pub fn get_consent(&self) -> Option<&str> {
        self.consent.value.as_deref()
    }

    /// Go: `SetConsent(consent)`. A nil or empty consent is deleted on rebuild.
    pub fn set_consent(&mut self, consent: Option<String>) {
        self.consent.set(consent);
    }

    /// Go: `GetConsentedProvidersSettingsIn()`: `user.ext.ConsentedProvidersSettings`, Google's
    /// Additional Consent string.
    #[must_use]
    pub const fn get_consented_providers_settings_in(
        &self,
    ) -> Option<&ConsentedProvidersSettingsIn> {
        self.consented_providers_settings_in.value.as_ref()
    }

    /// Go: `SetConsentedProvidersSettingsIn(cpSettings)`.
    ///
    /// A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_consented_providers_settings_in(
        &mut self,
        cp_settings: Option<ConsentedProvidersSettingsIn>,
    ) {
        self.consented_providers_settings_in.set(cp_settings);
    }

    /// Go: `GetConsentedProvidersSettingsOut()`: `user.ext.consented_providers_settings`, the
    /// elements of Google's Additional Consent string.
    #[must_use]
    pub const fn get_consented_providers_settings_out(
        &self,
    ) -> Option<&ConsentedProvidersSettingsOut> {
        self.consented_providers_settings_out.value.as_ref()
    }

    /// Go: `SetConsentedProvidersSettingsOut(cpSettings)`: replaces the value.
    ///
    /// `None` (Go's nil) changes nothing. A value that encodes as `{}` is deleted on rebuild.
    pub fn set_consented_providers_settings_out(
        &mut self,
        cp_settings: Option<ConsentedProvidersSettingsOut>,
    ) {
        // Go: openrtb_ext/request_wrapper.go:675
        if cp_settings.is_none() {
            return;
        }
        self.consented_providers_settings_out.set(cp_settings);
    }

    /// Go: `GetPrebid()`: `user.ext.prebid`.
    #[must_use]
    pub const fn get_prebid(&self) -> Option<&ExtUserPrebid> {
        self.prebid.value.as_ref()
    }

    /// Go: `SetPrebid(prebid)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_prebid(&mut self, prebid: Option<ExtUserPrebid>) {
        self.prebid.set(prebid);
    }

    /// Go: `GetEid()`: `user.ext.eids`.
    ///
    /// Go's `*[]openrtb2.EID`: `None` is a nil pointer, `Some(None)` a pointer to a nil slice.
    #[must_use]
    pub fn get_eid(&self) -> Option<Option<&[openrtb2::EID]>> {
        self.eids.value.as_ref().map(Option::as_deref)
    }

    /// Go: `SetEid(eid)`. Nil or empty identifiers are deleted on rebuild.
    pub fn set_eid(&mut self, eid: Option<Option<Vec<openrtb2::EID>>>) {
        self.eids.set(eid);
    }
}

/// Go: `RequestExt`: the decoded request `ext`.
#[derive(Debug, Default, PartialEq)]
pub struct RequestExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `prebid` and `prebidDirty`.
    prebid: Tracked<Option<Box<ExtRequestPrebid>>>,
    /// Go's `schain` (the ORTB 2.4 location) and `schainDirty`.
    schain: Tracked<Option<openrtb2::SupplyChain>>,
}

impl WrappedExt for RequestExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:760
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        let ext = &self.ext.value;
        if let Some(prebid_json) = ext_get(ext, PREBID_KEY) {
            let prebid = self.prebid.value.insert(Box::default());
            if let Some(prebid_json) = present(Some(prebid_json)) {
                decode_into(prebid_json, &mut **prebid)?;
            }
        }
        if let Some(schain_json) = ext_get(ext, SCHAIN_KEY) {
            let schain = self.schain.value.insert(openrtb2::SupplyChain::default());
            if let Some(schain_json) = present(Some(schain_json)) {
                decode_into(schain_json, schain)?;
            }
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:798
        let ext = &mut self.ext.value;
        if self.prebid.dirty {
            marshal_object_member(ext, PREBID_KEY, self.prebid.value.as_deref())?;
            self.prebid.dirty = false;
        }
        if self.schain.dirty {
            marshal_object_member(ext, SCHAIN_KEY, self.schain.value.as_ref())?;
            self.schain.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl Clone for RequestExt {
    /// Go: `(*RequestExt).Clone()`: `prebid` is copied with [`ExtRequestPrebid::clone_go`]
    /// (which leaves some targeting fields out), and the supply chain's nil nodes become empty.
    fn clone(&self) -> Self {
        // Go: openrtb_ext/request_wrapper.go:886
        Self {
            ext: self.ext.clone(),
            prebid: Tracked {
                value: self
                    .prebid
                    .value
                    .as_deref()
                    .map(|prebid| Box::new(prebid.clone_go())),
                dirty: self.prebid.dirty,
            },
            schain: Tracked {
                value: self.schain.value.as_ref().map(clone_supply_chain),
                dirty: self.schain.dirty,
            },
        }
    }
}

impl RequestExt {
    ext_map_accessors!("RequestExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:840
        self.ext.dirty || self.prebid.dirty || self.schain.dirty
    }

    /// Go: `GetPrebid()`: `ext.prebid`.
    #[must_use]
    pub fn get_prebid(&self) -> Option<&ExtRequestPrebid> {
        self.prebid.value.as_deref()
    }

    /// Go: `SetPrebid(prebid)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_prebid(&mut self, prebid: Option<ExtRequestPrebid>) {
        self.prebid.set(prebid.map(Box::new));
    }

    /// Go: `GetSChain()`: `ext.schain`, the ORTB 2.4 location of the supply chain.
    ///
    /// It is only for backwards compatibility: see [`SourceExt::get_s_chain`] and
    /// `ext.prebid.schains`.
    #[must_use]
    pub const fn get_s_chain(&self) -> Option<&openrtb2::SupplyChain> {
        self.schain.value.as_ref()
    }

    /// Go: `SetSChain(schain)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_s_chain(&mut self, schain: Option<openrtb2::SupplyChain>) {
        self.schain.set(schain);
    }
}

/// Go: `DeviceExt`: the decoded `device.ext`.
///
/// (`atts` stays in the map: Go reads it with `ParseDeviceExtATTS`.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `prebid` and `prebidDirty`.
    prebid: Tracked<Option<ExtDevicePrebid>>,
    /// Go's `cdep` and `cdepDirty`.
    cdep: Tracked<String>,
}

impl WrappedExt for DeviceExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:920
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        let ext = &self.ext.value;
        if let Some(prebid_json) = ext_get(ext, PREBID_KEY) {
            let prebid = self.prebid.value.insert(ExtDevicePrebid::default());
            if let Some(prebid_json) = present(Some(prebid_json)) {
                decode_into(prebid_json, prebid)?;
            }
        }
        if let Some(cdep_json) = present(ext_get(ext, CDEP_KEY)) {
            decode_into(cdep_json, &mut self.cdep.value)?;
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:955
        let ext = &mut self.ext.value;
        if self.prebid.dirty {
            marshal_object_member(ext, PREBID_KEY, self.prebid.value.as_ref())?;
            self.prebid.dirty = false;
        }
        if self.cdep.dirty {
            let cdep = Some(&self.cdep.value).filter(|cdep| !cdep.is_empty());
            marshal_member(ext, CDEP_KEY, cdep)?;
            self.cdep.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl DeviceExt {
    ext_map_accessors!("DeviceExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:993
        self.ext.dirty || self.prebid.dirty || self.cdep.dirty
    }

    /// Go: `GetPrebid()`: `device.ext.prebid`.
    #[must_use]
    pub const fn get_prebid(&self) -> Option<&ExtDevicePrebid> {
        self.prebid.value.as_ref()
    }

    /// Go: `SetPrebid(prebid)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_prebid(&mut self, prebid: Option<ExtDevicePrebid>) {
        self.prebid.set(prebid);
    }

    /// Go: `GetCDep()`: `device.ext.cdep`, the Chrome deprecation label.
    #[must_use]
    pub fn get_c_dep(&self) -> &str {
        &self.cdep.value
    }

    /// Go: `SetCDep(cdep)`. An empty label is deleted on rebuild.
    pub fn set_c_dep(&mut self, cdep: String) {
        self.cdep.set(cdep);
    }
}

/// Go: `AppExt`: the decoded `app.ext`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `prebid` and `prebidDirty`.
    prebid: Tracked<Option<ExtAppPrebid>>,
}

impl WrappedExt for AppExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1063
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        if let Some(prebid_json) = ext_get(&self.ext.value, PREBID_KEY) {
            let prebid = self.prebid.value.insert(ExtAppPrebid::default());
            if let Some(prebid_json) = present(Some(prebid_json)) {
                decode_into(prebid_json, prebid)?;
            }
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1091
        if self.prebid.dirty {
            let prebid = self.prebid.value.as_ref();
            marshal_object_member(&mut self.ext.value, PREBID_KEY, prebid)?;
            self.prebid.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl AppExt {
    ext_map_accessors!("AppExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1116
        self.ext.dirty || self.prebid.dirty
    }

    /// Go: `GetPrebid()`: `app.ext.prebid`.
    #[must_use]
    pub const fn get_prebid(&self) -> Option<&ExtAppPrebid> {
        self.prebid.value.as_ref()
    }

    /// Go: `SetPrebid(prebid)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_prebid(&mut self, prebid: Option<ExtAppPrebid>) {
        self.prebid.set(prebid);
    }
}

/// Go: `DOOHExt`: the decoded `dooh.ext` (no members of its own yet, as in Go).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DOOHExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
}

impl WrappedExt for DOOHExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1169
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1187
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl DOOHExt {
    ext_map_accessors!("DOOHExt");

    /// Go: `Dirty()`: whether the members were set since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1195
        self.ext.dirty
    }
}

/// Go: `RegExt`: the decoded `regs.ext`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `dsa` and `dsaDirty`.
    dsa: Tracked<Option<ExtRegsDSA>>,
    /// Go's `gdpr` and `gdprDirty`.
    gdpr: Tracked<Option<i8>>,
    /// Go's `gpc` and `gpcDirty`.
    gpc: Tracked<Option<String>>,
    /// Go's `usPrivacy` and `usPrivacyDirty`.
    us_privacy: Tracked<String>,
}

impl WrappedExt for RegExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1240
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        let ext = &self.ext.value;
        if let Some(dsa_json) = ext_get(ext, DSA_KEY) {
            let dsa = self.dsa.value.insert(ExtRegsDSA::default());
            if let Some(dsa_json) = present(Some(dsa_json)) {
                decode_into(dsa_json, dsa)?;
            }
        }
        if let Some(gdpr_json) = present(ext_get(ext, GDPR_KEY))
            && decode_into(gdpr_json, &mut self.gdpr.value).is_err()
        {
            return Err(GoError::generic("gdpr must be an integer"));
        }
        if let Some(usp_json) = present(ext_get(ext, US_PRIVACY_KEY)) {
            decode_into(usp_json, &mut self.us_privacy.value)?;
        }
        if let Some(gpc_json) = present(ext_get(ext, GPC_KEY)) {
            // Go: return jsonutil.ParseIntoString(gpcJson, &re.gpc), which never fails here.
            parse_into_string(gpc_json, Some(&mut self.gpc.value))?;
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1287
        let ext = &mut self.ext.value;
        if self.dsa.dirty {
            marshal_member(ext, DSA_KEY, self.dsa.value.as_ref())?;
            self.dsa.dirty = false;
        }
        if self.gdpr.dirty {
            marshal_member(ext, GDPR_KEY, self.gdpr.value.as_ref())?;
            self.gdpr.dirty = false;
        }
        if self.us_privacy.dirty {
            let us_privacy = Some(&self.us_privacy.value).filter(|usp| !usp.is_empty());
            marshal_member(ext, US_PRIVACY_KEY, us_privacy)?;
            self.us_privacy.dirty = false;
        }
        if self.gpc.dirty {
            marshal_member(ext, GPC_KEY, self.gpc.value.as_ref())?;
            self.gpc.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl RegExt {
    ext_map_accessors!("RegExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1347
        self.ext.dirty
            || self.dsa.dirty
            || self.gdpr.dirty
            || self.us_privacy.dirty
            || self.gpc.dirty
    }

    /// Go: `GetDSA()`: `regs.ext.dsa`.
    #[must_use]
    pub const fn get_dsa(&self) -> Option<&ExtRegsDSA> {
        self.dsa.value.as_ref()
    }

    /// Go: `SetDSA(dsa)`. A nil value is deleted on rebuild.
    pub fn set_dsa(&mut self, dsa: Option<ExtRegsDSA>) {
        self.dsa.set(dsa);
    }

    /// Go: `GetGDPR()`: `regs.ext.gdpr`.
    #[must_use]
    pub const fn get_gdpr(&self) -> Option<i8> {
        self.gdpr.value
    }

    /// Go: `SetGDPR(gdpr)`. A nil value is deleted on rebuild.
    pub fn set_gdpr(&mut self, gdpr: Option<i8>) {
        self.gdpr.set(gdpr);
    }

    /// Go: `GetGPC()`: `regs.ext.gpc`, a string whatever its JSON type.
    #[must_use]
    pub fn get_gpc(&self) -> Option<&str> {
        self.gpc.value.as_deref()
    }

    /// Go: `SetGPC(gpc)`. A nil value is deleted on rebuild.
    pub fn set_gpc(&mut self, gpc: Option<String>) {
        self.gpc.set(gpc);
    }

    /// Go: `GetUSPrivacy()`: `regs.ext.us_privacy`.
    #[must_use]
    pub fn get_us_privacy(&self) -> &str {
        &self.us_privacy.value
    }

    /// Go: `SetUSPrivacy(usPrivacy)`. An empty string is deleted on rebuild.
    pub fn set_us_privacy(&mut self, us_privacy: String) {
        self.us_privacy.set(us_privacy);
    }
}

/// Go: `SiteExt`: the decoded `site.ext`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `amp` and `ampDirty`.
    amp: Tracked<Option<i8>>,
}

impl WrappedExt for SiteExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1437
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        if let Some(amp_json) = present(ext_get(&self.ext.value, AMP_KEY))
            && decode_into(amp_json, &mut self.amp.value).is_err()
        {
            return Err(GoError::generic(
                "request.site.ext.amp must be either 1, 0, or undefined",
            ));
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1462
        if self.amp.dirty {
            marshal_member(&mut self.ext.value, AMP_KEY, self.amp.value.as_ref())?;
            self.amp.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl SiteExt {
    ext_map_accessors!("SiteExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1483
        self.ext.dirty || self.amp.dirty
    }

    /// Go: `GetAmp()`: `site.ext.amp`.
    #[must_use]
    pub const fn get_amp(&self) -> Option<i8> {
        self.amp.value
    }

    /// Go: `SetAmp(amp)`. A nil value is deleted on rebuild.
    pub fn set_amp(&mut self, amp: Option<i8>) {
        self.amp.set(amp);
    }
}

/// Go: `SourceExt`: the decoded `source.ext`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `schain` (the ORTB 2.5 location) and `schainDirty`.
    schain: Tracked<Option<openrtb2::SupplyChain>>,
}

impl WrappedExt for SourceExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1532
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        if let Some(schain_json) = present(ext_get(&self.ext.value, SCHAIN_KEY)) {
            decode_into(schain_json, &mut self.schain.value)?;
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1557
        if self.schain.dirty {
            let schain = self.schain.value.as_ref();
            marshal_object_member(&mut self.ext.value, SCHAIN_KEY, schain)?;
            self.schain.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl Clone for SourceExt {
    /// Go: `(*SourceExt).Clone()`: the supply chain's nil nodes become empty.
    fn clone(&self) -> Self {
        // Go: openrtb_ext/request_wrapper.go:1612
        Self {
            ext: self.ext.clone(),
            schain: Tracked {
                value: self.schain.value.as_ref().map(clone_supply_chain),
                dirty: self.schain.dirty,
            },
        }
    }
}

impl SourceExt {
    ext_map_accessors!("SourceExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1582
        self.ext.dirty || self.schain.dirty
    }

    /// Go: `GetSChain()`: `source.ext.schain`, the ORTB 2.5 location of the supply chain.
    #[must_use]
    pub const fn get_s_chain(&self) -> Option<&openrtb2::SupplyChain> {
        self.schain.value.as_ref()
    }

    /// Go: `SetSChain(schain)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_s_chain(&mut self, schain: Option<openrtb2::SupplyChain>) {
        self.schain.set(schain);
    }
}

/// Go: `ImpExt`: the decoded `imp.ext`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImpExt {
    /// Go's `ext` and `extDirty`.
    ext: Tracked<ExtMap>,
    /// Go's `prebid` and `prebidDirty`.
    prebid: Tracked<Option<Box<ExtImpPrebid>>>,
    /// Go's `data` (read only).
    data: Option<ExtImpData>,
    /// Go's `tid` and `tidDirty`.
    tid: Tracked<String>,
    /// Go's `gpId` (read only).
    gp_id: String,
}

impl WrappedExt for ImpExt {
    fn unmarshal(&mut self, ext_json: &[u8]) -> Result<(), GoError> {
        // Go: openrtb_ext/request_wrapper.go:1697
        if start_unmarshal(self.dirty(), &mut self.ext.value) || ext_json.is_empty() {
            return Ok(());
        }
        decode_map(ext_json, &mut self.ext.value)?;
        let ext = &self.ext.value;
        if let Some(prebid_json) = ext_get(ext, PREBID_KEY) {
            let prebid = self.prebid.value.insert(Box::default());
            if let Some(prebid_json) = present(Some(prebid_json)) {
                decode_into(prebid_json, &mut **prebid)?;
            }
        }
        if let Some(data_json) = ext_get(ext, DATA_KEY) {
            let data = self.data.insert(ExtImpData::default());
            if let Some(data_json) = present(Some(data_json)) {
                decode_into(data_json, data)?;
            }
        }
        if let Some(tid_json) = present(ext_get(ext, TID_KEY)) {
            decode_into(tid_json, &mut self.tid.value)?;
        }
        if let Some(gp_id_json) = present(ext_get(ext, GPID_KEY)) {
            decode_into(gp_id_json, &mut self.gp_id)?;
        }
        Ok(())
    }

    fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
        // Go: openrtb_ext/request_wrapper.go:1749
        let ext = &mut self.ext.value;
        if self.prebid.dirty {
            marshal_object_member(ext, PREBID_KEY, self.prebid.value.as_deref())?;
            self.prebid.dirty = false;
        }
        if self.tid.dirty {
            let tid = Some(&self.tid.value).filter(|tid| !tid.is_empty());
            marshal_member(ext, TID_KEY, tid)?;
            self.tid.dirty = false;
        }
        finish_marshal(&mut self.ext)
    }

    fn is_dirty(&self) -> bool {
        self.dirty()
    }
}

impl ImpExt {
    ext_map_accessors!("ImpExt");

    /// Go: `Dirty()`: whether anything changed since the ext was decoded or rebuilt.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        // Go: openrtb_ext/request_wrapper.go:1787
        self.ext.dirty || self.prebid.dirty || self.tid.dirty
    }

    /// Go: `GetPrebid()`: `imp.ext.prebid`.
    #[must_use]
    pub fn get_prebid(&self) -> Option<&ExtImpPrebid> {
        self.prebid.value.as_deref()
    }

    /// Go: `GetOrCreatePrebid()`: `imp.ext.prebid`, created empty when absent (which does not
    /// mark it dirty).
    pub fn get_or_create_prebid(&mut self) -> &ExtImpPrebid {
        // Go: openrtb_ext/request_wrapper.go:1812
        self.prebid.value.get_or_insert_with(Box::default)
    }

    /// Go: `SetPrebid(prebid)`. A nil value, or one that encodes as `{}`, is deleted on rebuild.
    pub fn set_prebid(&mut self, prebid: Option<ExtImpPrebid>) {
        self.prebid.set(prebid.map(Box::new));
    }

    /// Go: `GetData()`: `imp.ext.data`.
    #[must_use]
    pub const fn get_data(&self) -> Option<&ExtImpData> {
        self.data.as_ref()
    }

    /// Go: `GetTid()`: `imp.ext.tid`.
    #[must_use]
    pub fn get_tid(&self) -> &str {
        &self.tid.value
    }

    /// Go: `SetTid(tid)`. An empty value is deleted on rebuild.
    pub fn set_tid(&mut self, tid: String) {
        self.tid.set(tid);
    }

    /// Go: `GetGpId()`: `imp.ext.gpid`.
    #[must_use]
    pub fn get_gp_id(&self) -> &str {
        &self.gp_id
    }
}

/// Go: `CreateImpExtForTesting(ext, prebid)`: an ext object with the given members and `prebid`,
/// as if decoded (nothing dirty).
#[must_use]
pub fn create_imp_ext_for_testing(
    ext: Option<BTreeMap<String, Option<Box<RawValue>>>>,
    prebid: Option<ExtImpPrebid>,
) -> ImpExt {
    // Go: openrtb_ext/request_wrapper.go:1847
    ImpExt {
        ext: Tracked {
            value: ExtMap(ext),
            dirty: false,
        },
        prebid: Tracked {
            value: prebid.map(Box::new),
            dirty: false,
        },
        ..ImpExt::default()
    }
}
