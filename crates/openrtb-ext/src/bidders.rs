//! Go: `openrtb_ext/bidders.go`: bidder names, the core bidder and alias registry, and the
//! bidder params validator.
//!
//! Go keeps the registry in package variables (`coreBidderNames`, `aliasBidderToParent`,
//! `bidderNameLookup`) that `SetAliasBidderName` appends to while the router starts; here they
//! live behind one process-wide lock with the same contents and order.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{LazyLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

pub use pbs_bidder_params::{
    BidderParamValidator, LoadError, ParamsError, StandardBidderParamValidator,
};
use pbs_errortypes::GoError;
use pbs_gocompat::strings;

use crate::cow_str::cow_string_type;

cow_string_type! {
    /// `BidderName` refers to a core bidder id or an alias id.
    ///
    /// Go's `(*BidderName).String()` (which a nil pointer turns into `""`) is
    /// [`BidderName::as_str`] and `Display`.
    pub struct BidderName = "openrtb_ext.BidderName";
}

/// Go: `BidderNameNormalizer`, a `func(name string) (BidderName, bool)`; `None` is Go's
/// `("", false)`.
pub type BidderNameNormalizer = fn(&str) -> Option<BidderName>;

/// The registry behind Go's package variables.
#[derive(Debug)]
struct Registry {
    /// Go: `coreBidderNames`: the core bidders, then every alias in registration order.
    core_bidder_names: Vec<BidderName>,
    /// Go: `aliasBidderToParent`.
    alias_bidder_to_parent: BTreeMap<BidderName, BidderName>,
    /// Go: `bidderNameLookup`: the lower-case name to the exact name.
    bidder_name_lookup: HashMap<String, BidderName>,
}

static REGISTRY: LazyLock<RwLock<Registry>> = LazyLock::new(|| {
    let core_bidder_names = CORE_BIDDER_NAMES.to_vec();
    // Go: openrtb_ext/bidders.go:734 bidderNameLookup (a later name wins a lower-case
    // collision, as the map assignment in slice order does).
    let bidder_name_lookup = core_bidder_names
        .iter()
        .map(|name| (strings::to_lower(name.as_str()), name.clone()))
        .collect();
    RwLock::new(Registry {
        core_bidder_names,
        alias_bidder_to_parent: BTreeMap::new(),
        bidder_name_lookup,
    })
});

fn registry() -> RwLockReadGuard<'static, Registry> {
    REGISTRY.read().unwrap_or_else(PoisonError::into_inner)
}

fn registry_mut() -> RwLockWriteGuard<'static, Registry> {
    REGISTRY.write().unwrap_or_else(PoisonError::into_inner)
}

/// Go: `GetAliasBidderToParent()`: every registered alias and its parent (a copy of the
/// registry's map).
#[must_use]
pub fn get_alias_bidder_to_parent() -> BTreeMap<BidderName, BidderName> {
    registry().alias_bidder_to_parent.clone()
}

/// Go: `SetAliasBidderName(aliasBidderName, parentBidderName)`: registers an alias as a core
/// bidder name (appended again on every call, as Go appends), its parent, and its lower-case
/// lookup.
///
/// The alias text is interned for the life of the process: aliases are registered once while
/// the server starts, and every name the registry hands out then clones without allocating.
///
/// # Errors
///
/// `alias %s is a reserved bidder name and cannot be used` when the alias is reserved.
pub fn set_alias_bidder_name(
    alias_bidder_name: &str,
    parent_bidder_name: BidderName,
) -> Result<(), GoError> {
    // Go: openrtb_ext/bidders.go:309
    if is_bidder_name_reserved(alias_bidder_name) {
        return Err(GoError::generic(format!(
            "alias {alias_bidder_name} is a reserved bidder name and cannot be used"
        )));
    }
    let interned: &'static str = Box::leak(alias_bidder_name.to_owned().into_boxed_str());
    let alias_bidder = BidderName::from_static(interned);
    let lower = strings::to_lower(alias_bidder_name);
    let mut reg = registry_mut();
    reg.core_bidder_names.push(alias_bidder.clone());
    drop(
        reg.alias_bidder_to_parent
            .insert(alias_bidder.clone(), parent_bidder_name),
    );
    drop(reg.bidder_name_lookup.insert(lower, alias_bidder));
    drop(reg);
    Ok(())
}

/// The reserved bidder names, in the order Go's `IsBidderNameReserved` tests them.
const RESERVED: [BidderName; 10] = [
    BIDDER_RESERVED_ALL,
    BIDDER_RESERVED_CONTEXT,
    BIDDER_RESERVED_DATA,
    BIDDER_RESERVED_GENERAL,
    BIDDER_RESERVED_GPID,
    BIDDER_RESERVED_SK_AD_N,
    BIDDER_RESERVED_PREBID,
    BIDDER_RESERVED_TID,
    BIDDER_RESERVED_AE,
    BIDDER_RESERVED_IGS,
];

/// Go: `IsBidderNameReserved(name)`: whether `name` is a case-insensitive
/// (`strings.EqualFold`) match for a reserved bidder name.
#[must_use]
pub fn is_bidder_name_reserved(name: &str) -> bool {
    RESERVED
        .iter()
        .any(|reserved| strings::equal_fold(name, reserved.as_str()))
}

/// Go: `IsPotentialBidder(name)`: whether `name` is not reserved within the `imp[].ext` context
/// (an exact match; `all` and `general` are potential bidders).
#[must_use]
pub fn is_potential_bidder(name: &str) -> bool {
    !matches!(
        name,
        "context" | "data" | "gpid" | "prebid" | "skadn" | "tid" | "ae" | "igs"
    )
}

/// Go: `CoreBidderNames()`: the core bidders, then every registered alias.
#[must_use]
pub fn core_bidder_names() -> Vec<BidderName> {
    registry().core_bidder_names.clone()
}

/// Go: `BuildBidderMap()`: every core bidder and alias by its exact name.
#[must_use]
pub fn build_bidder_map() -> BTreeMap<String, BidderName> {
    registry()
        .core_bidder_names
        .iter()
        .map(|name| (name.as_str().to_owned(), name.clone()))
        .collect()
}

/// Go: `BuildBidderStringSlice()`: [`core_bidder_names`] as strings.
#[must_use]
pub fn build_bidder_string_slice() -> Vec<String> {
    registry()
        .core_bidder_names
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect()
}

/// Go: `BuildBidderNameHashSet()`: the set of [`core_bidder_names`].
#[must_use]
pub fn build_bidder_name_hash_set() -> HashSet<String> {
    registry()
        .core_bidder_names
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect()
}

/// Go: `NormalizeBidderName(name)`: the exact name of the core bidder or alias that `name`
/// matches after `strings.ToLower`.
#[must_use]
pub fn normalize_bidder_name(name: &str) -> Option<BidderName> {
    // Go: openrtb_ext/bidders.go:745
    let reg = registry();
    let found = if name
        .bytes()
        .any(|b| !b.is_ascii() || b.is_ascii_uppercase())
    {
        reg.bidder_name_lookup.get(&strings::to_lower(name))
    } else {
        reg.bidder_name_lookup.get(name)
    };
    found.cloned()
}

/// Go: `NormalizeBidderNameOrUnchanged(name)`: the normalized name of a known bidder, otherwise
/// the name exactly as provided.
#[must_use]
pub fn normalize_bidder_name_or_unchanged(name: &str) -> BidderName {
    normalize_bidder_name(name).unwrap_or_else(|| BidderName::from(name))
}

/// Go: `NewBidderParamsValidator(schemaDirectory)`: compiles every schema in the directory
/// (`static/bidder-params`), each named after a core bidder or alias, and gives every alias its
/// parent's schema.
///
/// The registry is read when it is called, as Go reads its package variables.
///
/// # Errors
///
/// Go's messages: the directory cannot be read, a file name is not a bidder name, or a schema
/// cannot be loaded (see [`pbs_bidder_params::new_bidder_params_validator`]).
pub fn new_bidder_params_validator(
    schema_directory: &str,
) -> Result<StandardBidderParamValidator, LoadError> {
    let reg = registry();
    let bidder_names: BTreeSet<String> = reg
        .core_bidder_names
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect();
    let alias_bidder_to_parent: BTreeMap<String, String> = reg
        .alias_bidder_to_parent
        .iter()
        .map(|(alias, parent)| (alias.as_str().to_owned(), parent.as_str().to_owned()))
        .collect();
    drop(reg);
    pbs_bidder_params::new_bidder_params_validator(
        schema_directory,
        &bidder_names,
        &alias_bidder_to_parent,
    )
}

// Names of reserved bidders (these names may not be used by a core bidder or alias), then the
// names of core bidders. Go: openrtb_ext/bidders.go:328 and :415.
/// Reserved for the /info/bidders/all endpoint.
#[doc(alias = "BidderReservedAll")]
pub const BIDDER_RESERVED_ALL: BidderName = BidderName::from_static("all");
/// Reserved for first party data.
#[doc(alias = "BidderReservedContext")]
pub const BIDDER_RESERVED_CONTEXT: BidderName = BidderName::from_static("context");
/// Reserved for first party data.
#[doc(alias = "BidderReservedData")]
pub const BIDDER_RESERVED_DATA: BidderName = BidderName::from_static("data");
/// Reserved for non-bidder specific messages when using a map keyed on the bidder name.
#[doc(alias = "BidderReservedGeneral")]
pub const BIDDER_RESERVED_GENERAL: BidderName = BidderName::from_static("general");
/// Reserved for Global Placement ID (GPID).
#[doc(alias = "BidderReservedGPID")]
pub const BIDDER_RESERVED_GPID: BidderName = BidderName::from_static("gpid");
/// Reserved for Prebid Server configuration.
#[doc(alias = "BidderReservedPrebid")]
pub const BIDDER_RESERVED_PREBID: BidderName = BidderName::from_static("prebid");
/// Reserved for Apple's `SKAdNetwork` `OpenRTB` extension.
#[doc(alias = "BidderReservedSKAdN")]
pub const BIDDER_RESERVED_SK_AD_N: BidderName = BidderName::from_static("skadn");
/// Reserved for Per-Impression Transactions IDs for Multi-Impression Bid Requests.
#[doc(alias = "BidderReservedTID")]
pub const BIDDER_RESERVED_TID: BidderName = BidderName::from_static("tid");
/// Reserved for PAAPI Auction Environment.
#[doc(alias = "BidderReservedAE")]
pub const BIDDER_RESERVED_AE: BidderName = BidderName::from_static("ae");
/// Reserved for PAAPI Interest Group Seller object.
#[doc(alias = "BidderReservedIGS")]
pub const BIDDER_RESERVED_IGS: BidderName = BidderName::from_static("igs");
/// Go constant `Bidder33Across`.
#[doc(alias = "Bidder33Across")]
pub const BIDDER33_ACROSS: BidderName = BidderName::from_static("33across");
/// Go constant `BidderAax`.
#[doc(alias = "BidderAax")]
pub const BIDDER_AAX: BidderName = BidderName::from_static("aax");
/// Go constant `BidderAceex`.
#[doc(alias = "BidderAceex")]
pub const BIDDER_ACEEX: BidderName = BidderName::from_static("aceex");
/// Go constant `BidderAcuityAds`.
#[doc(alias = "BidderAcuityAds")]
pub const BIDDER_ACUITY_ADS: BidderName = BidderName::from_static("acuityads");
/// Go constant `BidderAdagio`.
#[doc(alias = "BidderAdagio")]
pub const BIDDER_ADAGIO: BidderName = BidderName::from_static("adagio");
/// Go constant `BidderAdelement`.
#[doc(alias = "BidderAdelement")]
pub const BIDDER_ADELEMENT: BidderName = BidderName::from_static("adelement");
/// Go constant `BidderAdf`.
#[doc(alias = "BidderAdf")]
pub const BIDDER_ADF: BidderName = BidderName::from_static("adf");
/// Go constant `BidderAdgeneration`.
#[doc(alias = "BidderAdgeneration")]
pub const BIDDER_ADGENERATION: BidderName = BidderName::from_static("adgeneration");
/// Go constant `BidderAdhese`.
#[doc(alias = "BidderAdhese")]
pub const BIDDER_ADHESE: BidderName = BidderName::from_static("adhese");
/// Go constant `BidderAdkernel`.
#[doc(alias = "BidderAdkernel")]
pub const BIDDER_ADKERNEL: BidderName = BidderName::from_static("adkernel");
/// Go constant `BidderAdkernelAdn`.
#[doc(alias = "BidderAdkernelAdn")]
pub const BIDDER_ADKERNEL_ADN: BidderName = BidderName::from_static("adkernelAdn");
/// Go constant `BidderAdman`.
#[doc(alias = "BidderAdman")]
pub const BIDDER_ADMAN: BidderName = BidderName::from_static("adman");
/// Go constant `BidderAdmatic`.
#[doc(alias = "BidderAdmatic")]
pub const BIDDER_ADMATIC: BidderName = BidderName::from_static("admatic");
/// Go constant `BidderAdmixer`.
#[doc(alias = "BidderAdmixer")]
pub const BIDDER_ADMIXER: BidderName = BidderName::from_static("admixer");
/// Go constant `BidderAdnuntius`.
#[doc(alias = "BidderAdnuntius")]
pub const BIDDER_ADNUNTIUS: BidderName = BidderName::from_static("adnuntius");
/// Go constant `BidderAdot`.
#[doc(alias = "BidderAdot")]
pub const BIDDER_ADOT: BidderName = BidderName::from_static("adot");
/// Go constant `BidderAdpone`.
#[doc(alias = "BidderAdpone")]
pub const BIDDER_ADPONE: BidderName = BidderName::from_static("adpone");
/// Go constant `BidderAdprime`.
#[doc(alias = "BidderAdprime")]
pub const BIDDER_ADPRIME: BidderName = BidderName::from_static("adprime");
/// Go constant `BidderAdquery`.
#[doc(alias = "BidderAdquery")]
pub const BIDDER_ADQUERY: BidderName = BidderName::from_static("adquery");
/// Go constant `BidderAdrino`.
#[doc(alias = "BidderAdrino")]
pub const BIDDER_ADRINO: BidderName = BidderName::from_static("adrino");
/// Go constant `BidderAdtarget`.
#[doc(alias = "BidderAdtarget")]
pub const BIDDER_ADTARGET: BidderName = BidderName::from_static("adtarget");
/// Go constant `BidderAdtrgtme`.
#[doc(alias = "BidderAdtrgtme")]
pub const BIDDER_ADTRGTME: BidderName = BidderName::from_static("adtrgtme");
/// Go constant `BidderAdTonos`.
#[doc(alias = "BidderAdTonos")]
pub const BIDDER_AD_TONOS: BidderName = BidderName::from_static("adtonos");
/// Go constant `BidderAdtelligent`.
#[doc(alias = "BidderAdtelligent")]
pub const BIDDER_ADTELLIGENT: BidderName = BidderName::from_static("adtelligent");
/// Go constant `BidderAdUpTech`.
#[doc(alias = "BidderAdUpTech")]
pub const BIDDER_AD_UP_TECH: BidderName = BidderName::from_static("aduptech");
/// Go constant `BidderAdvangelists`.
#[doc(alias = "BidderAdvangelists")]
pub const BIDDER_ADVANGELISTS: BidderName = BidderName::from_static("advangelists");
/// Go constant `BidderAdverxo`.
#[doc(alias = "BidderAdverxo")]
pub const BIDDER_ADVERXO: BidderName = BidderName::from_static("adverxo");
/// Go constant `BidderAdView`.
#[doc(alias = "BidderAdView")]
pub const BIDDER_AD_VIEW: BidderName = BidderName::from_static("adview");
/// Go constant `BidderAdxcg`.
#[doc(alias = "BidderAdxcg")]
pub const BIDDER_ADXCG: BidderName = BidderName::from_static("adxcg");
/// Go constant `BidderAdyoulike`.
#[doc(alias = "BidderAdyoulike")]
pub const BIDDER_ADYOULIKE: BidderName = BidderName::from_static("adyoulike");
/// Go constant `BidderAfront`.
#[doc(alias = "BidderAfront")]
pub const BIDDER_AFRONT: BidderName = BidderName::from_static("afront");
/// Go constant `BidderAidem`.
#[doc(alias = "BidderAidem")]
pub const BIDDER_AIDEM: BidderName = BidderName::from_static("aidem");
/// Go constant `BidderAJA`.
#[doc(alias = "BidderAJA")]
pub const BIDDER_AJA: BidderName = BidderName::from_static("aja");
/// Go constant `BidderAkcelo`.
#[doc(alias = "BidderAkcelo")]
pub const BIDDER_AKCELO: BidderName = BidderName::from_static("akcelo");
/// Go constant `BidderAlgorix`.
#[doc(alias = "BidderAlgorix")]
pub const BIDDER_ALGORIX: BidderName = BidderName::from_static("algorix");
/// Go constant `BidderAlkimi`.
#[doc(alias = "BidderAlkimi")]
pub const BIDDER_ALKIMI: BidderName = BidderName::from_static("alkimi");
/// Go constant `BidderAllianceGravity`.
#[doc(alias = "BidderAllianceGravity")]
pub const BIDDER_ALLIANCE_GRAVITY: BidderName = BidderName::from_static("alliance_gravity");
/// Go constant `BidderAMX`.
#[doc(alias = "BidderAMX")]
pub const BIDDER_AMX: BidderName = BidderName::from_static("amx");
/// Go constant `BidderApacdex`.
#[doc(alias = "BidderApacdex")]
pub const BIDDER_APACDEX: BidderName = BidderName::from_static("apacdex");
/// Go constant `BidderAppnexus`.
#[doc(alias = "BidderAppnexus")]
pub const BIDDER_APPNEXUS: BidderName = BidderName::from_static("appnexus");
/// Go constant `BidderAppush`.
#[doc(alias = "BidderAppush")]
pub const BIDDER_APPUSH: BidderName = BidderName::from_static("appush");
/// Go constant `BidderAps`.
#[doc(alias = "BidderAps")]
pub const BIDDER_APS: BidderName = BidderName::from_static("aps");
/// Go constant `BidderAso`.
#[doc(alias = "BidderAso")]
pub const BIDDER_ASO: BidderName = BidderName::from_static("aso");
/// Go constant `BidderAudienceNetwork`.
#[doc(alias = "BidderAudienceNetwork")]
pub const BIDDER_AUDIENCE_NETWORK: BidderName = BidderName::from_static("audienceNetwork");
/// Go constant `BidderAutomatad`.
#[doc(alias = "BidderAutomatad")]
pub const BIDDER_AUTOMATAD: BidderName = BidderName::from_static("automatad");
/// Go constant `BidderAvocet`.
#[doc(alias = "BidderAvocet")]
pub const BIDDER_AVOCET: BidderName = BidderName::from_static("avocet");
/// Go constant `BidderAxis`.
#[doc(alias = "BidderAxis")]
pub const BIDDER_AXIS: BidderName = BidderName::from_static("axis");
/// Go constant `BidderAxonix`.
#[doc(alias = "BidderAxonix")]
pub const BIDDER_AXONIX: BidderName = BidderName::from_static("axonix");
/// Go constant `BidderBeachfront`.
#[doc(alias = "BidderBeachfront")]
pub const BIDDER_BEACHFRONT: BidderName = BidderName::from_static("beachfront");
/// Go constant `BidderBeintoo`.
#[doc(alias = "BidderBeintoo")]
pub const BIDDER_BEINTOO: BidderName = BidderName::from_static("beintoo");
/// Go constant `BidderBematterfull`.
#[doc(alias = "BidderBematterfull")]
pub const BIDDER_BEMATTERFULL: BidderName = BidderName::from_static("bematterfull");
/// Go constant `BidderBeop`.
#[doc(alias = "BidderBeop")]
pub const BIDDER_BEOP: BidderName = BidderName::from_static("beop");
/// Go constant `BidderBetween`.
#[doc(alias = "BidderBetween")]
pub const BIDDER_BETWEEN: BidderName = BidderName::from_static("between");
/// Go constant `BidderBeyondMedia`.
#[doc(alias = "BidderBeyondMedia")]
pub const BIDDER_BEYOND_MEDIA: BidderName = BidderName::from_static("beyondmedia");
/// Go constant `BidderBidmachine`.
#[doc(alias = "BidderBidmachine")]
pub const BIDDER_BIDMACHINE: BidderName = BidderName::from_static("bidmachine");
/// Go constant `BidderBidmatic`.
#[doc(alias = "BidderBidmatic")]
pub const BIDDER_BIDMATIC: BidderName = BidderName::from_static("bidmatic");
/// Go constant `BidderBidmyadz`.
#[doc(alias = "BidderBidmyadz")]
pub const BIDDER_BIDMYADZ: BidderName = BidderName::from_static("bidmyadz");
/// Go constant `BidderBidsCube`.
#[doc(alias = "BidderBidsCube")]
pub const BIDDER_BIDS_CUBE: BidderName = BidderName::from_static("bidscube");
/// Go constant `BidderBidstack`.
#[doc(alias = "BidderBidstack")]
pub const BIDDER_BIDSTACK: BidderName = BidderName::from_static("bidstack");
/// Go constant `BidderBidtheatre`.
#[doc(alias = "BidderBidtheatre")]
pub const BIDDER_BIDTHEATRE: BidderName = BidderName::from_static("bidtheatre");
/// Go constant `BidderBigoAd`.
#[doc(alias = "BidderBigoAd")]
pub const BIDDER_BIGO_AD: BidderName = BidderName::from_static("bigoad");
/// Go constant `BidderBlasto`.
#[doc(alias = "BidderBlasto")]
pub const BIDDER_BLASTO: BidderName = BidderName::from_static("blasto");
/// Go constant `BidderBliink`.
#[doc(alias = "BidderBliink")]
pub const BIDDER_BLIINK: BidderName = BidderName::from_static("bliink");
/// Go constant `BidderBlis`.
#[doc(alias = "BidderBlis")]
pub const BIDDER_BLIS: BidderName = BidderName::from_static("blis");
/// Go constant `BidderBlue`.
#[doc(alias = "BidderBlue")]
pub const BIDDER_BLUE: BidderName = BidderName::from_static("blue");
/// Go constant `BidderBluesea`.
#[doc(alias = "BidderBluesea")]
pub const BIDDER_BLUESEA: BidderName = BidderName::from_static("bluesea");
/// Go constant `BidderBmtm`.
#[doc(alias = "BidderBmtm")]
pub const BIDDER_BMTM: BidderName = BidderName::from_static("bmtm");
/// Go constant `BidderBoldwin`.
#[doc(alias = "BidderBoldwin")]
pub const BIDDER_BOLDWIN: BidderName = BidderName::from_static("boldwin");
/// Go constant `BidderBoldwinRapid`.
#[doc(alias = "BidderBoldwinRapid")]
pub const BIDDER_BOLDWIN_RAPID: BidderName = BidderName::from_static("boldwin_rapid");
/// Go constant `BidderBrave`.
#[doc(alias = "BidderBrave")]
pub const BIDDER_BRAVE: BidderName = BidderName::from_static("brave");
/// Go constant `BidderBWX`.
#[doc(alias = "BidderBWX")]
pub const BIDDER_BWX: BidderName = BidderName::from_static("bwx");
/// Go constant `BidderCadentApertureMX`.
#[doc(alias = "BidderCadentApertureMX")]
pub const BIDDER_CADENT_APERTURE_MX: BidderName = BidderName::from_static("cadent_aperture_mx");
/// Go constant `BidderCcx`.
#[doc(alias = "BidderCcx")]
pub const BIDDER_CCX: BidderName = BidderName::from_static("ccx");
/// Go constant `BidderClydo`.
#[doc(alias = "BidderClydo")]
pub const BIDDER_CLYDO: BidderName = BidderName::from_static("clydo");
/// Go constant `BidderCointraffic`.
#[doc(alias = "BidderCointraffic")]
pub const BIDDER_COINTRAFFIC: BidderName = BidderName::from_static("cointraffic");
/// Go constant `BidderCoinzilla`.
#[doc(alias = "BidderCoinzilla")]
pub const BIDDER_COINZILLA: BidderName = BidderName::from_static("coinzilla");
/// Go constant `BidderColossus`.
#[doc(alias = "BidderColossus")]
pub const BIDDER_COLOSSUS: BidderName = BidderName::from_static("colossus");
/// Go constant `BidderCompass`.
#[doc(alias = "BidderCompass")]
pub const BIDDER_COMPASS: BidderName = BidderName::from_static("compass");
/// Go constant `BidderConcert`.
#[doc(alias = "BidderConcert")]
pub const BIDDER_CONCERT: BidderName = BidderName::from_static("concert");
/// Go constant `BidderConnatix`.
#[doc(alias = "BidderConnatix")]
pub const BIDDER_CONNATIX: BidderName = BidderName::from_static("connatix");
/// Go constant `BidderConnectAd`.
#[doc(alias = "BidderConnectAd")]
pub const BIDDER_CONNECT_AD: BidderName = BidderName::from_static("connectad");
/// Go constant `BidderConsumable`.
#[doc(alias = "BidderConsumable")]
pub const BIDDER_CONSUMABLE: BidderName = BidderName::from_static("consumable");
/// Go constant `BidderContxtful`.
#[doc(alias = "BidderContxtful")]
pub const BIDDER_CONTXTFUL: BidderName = BidderName::from_static("contxtful");
/// Go constant `BidderConversant`.
#[doc(alias = "BidderConversant")]
pub const BIDDER_CONVERSANT: BidderName = BidderName::from_static("conversant");
/// Go constant `BidderCopper6ssp`.
#[doc(alias = "BidderCopper6ssp")]
pub const BIDDER_COPPER6SSP: BidderName = BidderName::from_static("copper6ssp");
/// Go constant `BidderCpmstar`.
#[doc(alias = "BidderCpmstar")]
pub const BIDDER_CPMSTAR: BidderName = BidderName::from_static("cpmstar");
/// Go constant `BidderCriteo`.
#[doc(alias = "BidderCriteo")]
pub const BIDDER_CRITEO: BidderName = BidderName::from_static("criteo");
/// Go constant `BidderCWire`.
#[doc(alias = "BidderCWire")]
pub const BIDDER_C_WIRE: BidderName = BidderName::from_static("cwire");
/// Go constant `BidderDatablocks`.
#[doc(alias = "BidderDatablocks")]
pub const BIDDER_DATABLOCKS: BidderName = BidderName::from_static("datablocks");
/// Go constant `BidderDecenterAds`.
#[doc(alias = "BidderDecenterAds")]
pub const BIDDER_DECENTER_ADS: BidderName = BidderName::from_static("decenterads");
/// Go constant `BidderDeepintent`.
#[doc(alias = "BidderDeepintent")]
pub const BIDDER_DEEPINTENT: BidderName = BidderName::from_static("deepintent");
/// Go constant `BidderDefinemedia`.
#[doc(alias = "BidderDefinemedia")]
pub const BIDDER_DEFINEMEDIA: BidderName = BidderName::from_static("definemedia");
/// Go constant `BidderDianomi`.
#[doc(alias = "BidderDianomi")]
pub const BIDDER_DIANOMI: BidderName = BidderName::from_static("dianomi");
/// Go constant `BidderDisplayio`.
#[doc(alias = "BidderDisplayio")]
pub const BIDDER_DISPLAYIO: BidderName = BidderName::from_static("displayio");
/// Go constant `BidderEdge226`.
#[doc(alias = "BidderEdge226")]
pub const BIDDER_EDGE226: BidderName = BidderName::from_static("edge226");
/// Go constant `BidderDmx`.
#[doc(alias = "BidderDmx")]
pub const BIDDER_DMX: BidderName = BidderName::from_static("dmx");
/// Go constant `BidderDriftPixel`.
#[doc(alias = "BidderDriftPixel")]
pub const BIDDER_DRIFT_PIXEL: BidderName = BidderName::from_static("driftpixel");
/// Go constant `BidderElementalTV`.
#[doc(alias = "BidderElementalTV")]
pub const BIDDER_ELEMENTAL_TV: BidderName = BidderName::from_static("elementaltv");
/// Go constant `BidderEmtv`.
#[doc(alias = "BidderEmtv")]
pub const BIDDER_EMTV: BidderName = BidderName::from_static("emtv");
/// Go constant `BidderEmxDigital`.
#[doc(alias = "BidderEmxDigital")]
pub const BIDDER_EMX_DIGITAL: BidderName = BidderName::from_static("emx_digital");
/// Go constant `BidderEPlanning`.
#[doc(alias = "BidderEPlanning")]
pub const BIDDER_E_PLANNING: BidderName = BidderName::from_static("eplanning");
/// Go constant `BidderEpom`.
#[doc(alias = "BidderEpom")]
pub const BIDDER_EPOM: BidderName = BidderName::from_static("epom");
/// Go constant `BidderEscalax`.
#[doc(alias = "BidderEscalax")]
pub const BIDDER_ESCALAX: BidderName = BidderName::from_static("escalax");
/// Go constant `BidderEskimi`.
#[doc(alias = "BidderEskimi")]
pub const BIDDER_ESKIMI: BidderName = BidderName::from_static("eskimi");
/// Go constant `BidderExco`.
#[doc(alias = "BidderExco")]
pub const BIDDER_EXCO: BidderName = BidderName::from_static("exco");
/// Go constant `BidderEVolution`.
#[doc(alias = "BidderEVolution")]
pub const BIDDER_E_VOLUTION: BidderName = BidderName::from_static("e_volution");
/// Go constant `BidderEzoic`.
#[doc(alias = "BidderEzoic")]
pub const BIDDER_EZOIC: BidderName = BidderName::from_static("ezoic");
/// Go constant `BidderFeedAd`.
#[doc(alias = "BidderFeedAd")]
pub const BIDDER_FEED_AD: BidderName = BidderName::from_static("feedad");
/// Go constant `BidderFerio`.
#[doc(alias = "BidderFerio")]
pub const BIDDER_FERIO: BidderName = BidderName::from_static("ferio");
/// Go constant `BidderFlatads`.
#[doc(alias = "BidderFlatads")]
pub const BIDDER_FLATADS: BidderName = BidderName::from_static("flatads");
/// Go constant `BidderFlipp`.
#[doc(alias = "BidderFlipp")]
pub const BIDDER_FLIPP: BidderName = BidderName::from_static("flipp");
/// Go constant `BidderFloxis`.
#[doc(alias = "BidderFloxis")]
pub const BIDDER_FLOXIS: BidderName = BidderName::from_static("floxis");
/// Go constant `BidderFreewheelSSP`.
#[doc(alias = "BidderFreewheelSSP")]
pub const BIDDER_FREEWHEEL_SSP: BidderName = BidderName::from_static("freewheelssp");
/// Go constant `BidderFWSSP`.
#[doc(alias = "BidderFWSSP")]
pub const BIDDER_FWSSP: BidderName = BidderName::from_static("fwssp");
/// Go constant `BidderFRVRAdNetwork`.
#[doc(alias = "BidderFRVRAdNetwork")]
pub const BIDDER_FRVR_AD_NETWORK: BidderName = BidderName::from_static("frvradn");
/// Go constant `BidderGamma`.
#[doc(alias = "BidderGamma")]
pub const BIDDER_GAMMA: BidderName = BidderName::from_static("gamma");
/// Go constant `BidderGamoshi`.
#[doc(alias = "BidderGamoshi")]
pub const BIDDER_GAMOSHI: BidderName = BidderName::from_static("gamoshi");
/// Go constant `BidderGlobalsun`.
#[doc(alias = "BidderGlobalsun")]
pub const BIDDER_GLOBALSUN: BidderName = BidderName::from_static("globalsun");
/// Go constant `BidderGoldbach`.
#[doc(alias = "BidderGoldbach")]
pub const BIDDER_GOLDBACH: BidderName = BidderName::from_static("goldbach");
/// Go constant `BidderGrid`.
#[doc(alias = "BidderGrid")]
pub const BIDDER_GRID: BidderName = BidderName::from_static("grid");
/// Go constant `BidderGumGum`.
#[doc(alias = "BidderGumGum")]
pub const BIDDER_GUM_GUM: BidderName = BidderName::from_static("gumgum");
/// Go constant `BidderHuaweiAds`.
#[doc(alias = "BidderHuaweiAds")]
pub const BIDDER_HUAWEI_ADS: BidderName = BidderName::from_static("huaweiads");
/// Go constant `BidderImds`.
#[doc(alias = "BidderImds")]
pub const BIDDER_IMDS: BidderName = BidderName::from_static("imds");
/// Go constant `BidderImpactify`.
#[doc(alias = "BidderImpactify")]
pub const BIDDER_IMPACTIFY: BidderName = BidderName::from_static("impactify");
/// Go constant `BidderImprovedigital`.
#[doc(alias = "BidderImprovedigital")]
pub const BIDDER_IMPROVEDIGITAL: BidderName = BidderName::from_static("improvedigital");
/// Go constant `BidderInfyTV`.
#[doc(alias = "BidderInfyTV")]
pub const BIDDER_INFY_TV: BidderName = BidderName::from_static("infytv");
/// Go constant `BidderInMobi`.
#[doc(alias = "BidderInMobi")]
pub const BIDDER_IN_MOBI: BidderName = BidderName::from_static("inmobi");
/// Go constant `BidderInsticator`.
#[doc(alias = "BidderInsticator")]
pub const BIDDER_INSTICATOR: BidderName = BidderName::from_static("insticator");
/// Go constant `BidderIntenze`.
#[doc(alias = "BidderIntenze")]
pub const BIDDER_INTENZE: BidderName = BidderName::from_static("intenze");
/// Go constant `BidderInteractiveoffers`.
#[doc(alias = "BidderInteractiveoffers")]
pub const BIDDER_INTERACTIVEOFFERS: BidderName = BidderName::from_static("interactiveoffers");
/// Go constant `BidderInvibes`.
#[doc(alias = "BidderInvibes")]
pub const BIDDER_INVIBES: BidderName = BidderName::from_static("invibes");
/// Go constant `BidderIQX`.
#[doc(alias = "BidderIQX")]
pub const BIDDER_IQX: BidderName = BidderName::from_static("iqx");
/// Go constant `BidderIQZone`.
#[doc(alias = "BidderIQZone")]
pub const BIDDER_IQ_ZONE: BidderName = BidderName::from_static("iqzone");
/// Go constant `BidderIx`.
#[doc(alias = "BidderIx")]
pub const BIDDER_IX: BidderName = BidderName::from_static("ix");
/// Go constant `BidderJixie`.
#[doc(alias = "BidderJixie")]
pub const BIDDER_JIXIE: BidderName = BidderName::from_static("jixie");
/// Go constant `BidderKargo`.
#[doc(alias = "BidderKargo")]
pub const BIDDER_KARGO: BidderName = BidderName::from_static("kargo");
/// Go constant `BidderKayzen`.
#[doc(alias = "BidderKayzen")]
pub const BIDDER_KAYZEN: BidderName = BidderName::from_static("kayzen");
/// Go constant `BidderKidoz`.
#[doc(alias = "BidderKidoz")]
pub const BIDDER_KIDOZ: BidderName = BidderName::from_static("kidoz");
/// Go constant `BidderKiviads`.
#[doc(alias = "BidderKiviads")]
pub const BIDDER_KIVIADS: BidderName = BidderName::from_static("kiviads");
/// Go constant `BidderLmKiviads`.
#[doc(alias = "BidderLmKiviads")]
pub const BIDDER_LM_KIVIADS: BidderName = BidderName::from_static("lm_kiviads");
/// Go constant `BidderKobler`.
#[doc(alias = "BidderKobler")]
pub const BIDDER_KOBLER: BidderName = BidderName::from_static("kobler");
/// Go constant `BidderKrushmedia`.
#[doc(alias = "BidderKrushmedia")]
pub const BIDDER_KRUSHMEDIA: BidderName = BidderName::from_static("krushmedia");
/// Go constant `BidderKueezRTB`.
#[doc(alias = "BidderKueezRTB")]
pub const BIDDER_KUEEZ_RTB: BidderName = BidderName::from_static("kueezrtb");
/// Go constant `BidderLemmadigital`.
#[doc(alias = "BidderLemmadigital")]
pub const BIDDER_LEMMADIGITAL: BidderName = BidderName::from_static("lemmadigital");
/// Go constant `BidderLimelightDigital`.
#[doc(alias = "BidderLimelightDigital")]
pub const BIDDER_LIMELIGHT_DIGITAL: BidderName = BidderName::from_static("limelightDigital");
/// Go constant `BidderLockerDome`.
#[doc(alias = "BidderLockerDome")]
pub const BIDDER_LOCKER_DOME: BidderName = BidderName::from_static("lockerdome");
/// Go constant `BidderLogan`.
#[doc(alias = "BidderLogan")]
pub const BIDDER_LOGAN: BidderName = BidderName::from_static("logan");
/// Go constant `BidderLogicad`.
#[doc(alias = "BidderLogicad")]
pub const BIDDER_LOGICAD: BidderName = BidderName::from_static("logicad");
/// Go constant `BidderLoopme`.
#[doc(alias = "BidderLoopme")]
pub const BIDDER_LOOPME: BidderName = BidderName::from_static("loopme");
/// Go constant `BidderLoyal`.
#[doc(alias = "BidderLoyal")]
pub const BIDDER_LOYAL: BidderName = BidderName::from_static("loyal");
/// Go constant `BidderLunaMedia`.
#[doc(alias = "BidderLunaMedia")]
pub const BIDDER_LUNA_MEDIA: BidderName = BidderName::from_static("lunamedia");
/// Go constant `BidderMabidder`.
#[doc(alias = "BidderMabidder")]
pub const BIDDER_MABIDDER: BidderName = BidderName::from_static("mabidder");
/// Go constant `BidderMadSense`.
#[doc(alias = "BidderMadSense")]
pub const BIDDER_MAD_SENSE: BidderName = BidderName::from_static("madsense");
/// Go constant `BidderMadvertise`.
#[doc(alias = "BidderMadvertise")]
pub const BIDDER_MADVERTISE: BidderName = BidderName::from_static("madvertise");
/// Go constant `BidderMarsmedia`.
#[doc(alias = "BidderMarsmedia")]
pub const BIDDER_MARSMEDIA: BidderName = BidderName::from_static("marsmedia");
/// Go constant `BidderMatterfullRTB`.
#[doc(alias = "BidderMatterfullRTB")]
pub const BIDDER_MATTERFULL_RTB: BidderName = BidderName::from_static("matterfullrtb");
/// Go constant `BidderMediafuse`.
#[doc(alias = "BidderMediafuse")]
pub const BIDDER_MEDIAFUSE: BidderName = BidderName::from_static("mediafuse");
/// Go constant `BidderMediaGo`.
#[doc(alias = "BidderMediaGo")]
pub const BIDDER_MEDIA_GO: BidderName = BidderName::from_static("mediago");
/// Go constant `BidderMedianet`.
#[doc(alias = "BidderMedianet")]
pub const BIDDER_MEDIANET: BidderName = BidderName::from_static("medianet");
/// Go constant `BidderMediasquare`.
#[doc(alias = "BidderMediasquare")]
pub const BIDDER_MEDIASQUARE: BidderName = BidderName::from_static("mediasquare");
/// Go constant `BidderMeloZen`.
#[doc(alias = "BidderMeloZen")]
pub const BIDDER_MELO_ZEN: BidderName = BidderName::from_static("melozen");
/// Go constant `BidderMetaX`.
#[doc(alias = "BidderMetaX")]
pub const BIDDER_META_X: BidderName = BidderName::from_static("metax");
/// Go constant `BidderMgid`.
#[doc(alias = "BidderMgid")]
pub const BIDDER_MGID: BidderName = BidderName::from_static("mgid");
/// Go constant `BidderMgidX`.
#[doc(alias = "BidderMgidX")]
pub const BIDDER_MGID_X: BidderName = BidderName::from_static("mgidX");
/// Go constant `BidderMicrosoft`.
#[doc(alias = "BidderMicrosoft")]
pub const BIDDER_MICROSOFT: BidderName = BidderName::from_static("msft");
/// Go constant `BidderMinuteMedia`.
#[doc(alias = "BidderMinuteMedia")]
pub const BIDDER_MINUTE_MEDIA: BidderName = BidderName::from_static("minutemedia");
/// Go constant `BidderMissena`.
#[doc(alias = "BidderMissena")]
pub const BIDDER_MISSENA: BidderName = BidderName::from_static("missena");
/// Go constant `BidderMobfoxpb`.
#[doc(alias = "BidderMobfoxpb")]
pub const BIDDER_MOBFOXPB: BidderName = BidderName::from_static("mobfoxpb");
/// Go constant `BidderMobileFuse`.
#[doc(alias = "BidderMobileFuse")]
pub const BIDDER_MOBILE_FUSE: BidderName = BidderName::from_static("mobilefuse");
/// Go constant `BidderMobkoi`.
#[doc(alias = "BidderMobkoi")]
pub const BIDDER_MOBKOI: BidderName = BidderName::from_static("mobkoi");
/// Go constant `BidderMotorik`.
#[doc(alias = "BidderMotorik")]
pub const BIDDER_MOTORIK: BidderName = BidderName::from_static("motorik");
/// Go constant `BidderNativery`.
#[doc(alias = "BidderNativery")]
pub const BIDDER_NATIVERY: BidderName = BidderName::from_static("nativery");
/// Go constant `BidderNativo`.
#[doc(alias = "BidderNativo")]
pub const BIDDER_NATIVO: BidderName = BidderName::from_static("nativo");
/// Go constant `BidderNextMillennium`.
#[doc(alias = "BidderNextMillennium")]
pub const BIDDER_NEXT_MILLENNIUM: BidderName = BidderName::from_static("nextmillennium");
/// Go constant `BidderNexx360`.
#[doc(alias = "BidderNexx360")]
pub const BIDDER_NEXX360: BidderName = BidderName::from_static("nexx360");
/// Go constant `BidderNoBid`.
#[doc(alias = "BidderNoBid")]
pub const BIDDER_NO_BID: BidderName = BidderName::from_static("nobid");
/// Go constant `BidderOdeeo`.
#[doc(alias = "BidderOdeeo")]
pub const BIDDER_ODEEO: BidderName = BidderName::from_static("odeeo");
/// Go constant `BidderOgury`.
#[doc(alias = "BidderOgury")]
pub const BIDDER_OGURY: BidderName = BidderName::from_static("ogury");
/// Go constant `BidderOms`.
#[doc(alias = "BidderOms")]
pub const BIDDER_OMS: BidderName = BidderName::from_static("oms");
/// Go constant `BidderOneTag`.
#[doc(alias = "BidderOneTag")]
pub const BIDDER_ONE_TAG: BidderName = BidderName::from_static("onetag");
/// Go constant `BidderOpenWeb`.
#[doc(alias = "BidderOpenWeb")]
pub const BIDDER_OPEN_WEB: BidderName = BidderName::from_static("openweb");
/// Go constant `BidderOpenx`.
#[doc(alias = "BidderOpenx")]
pub const BIDDER_OPENX: BidderName = BidderName::from_static("openx");
/// Go constant `BidderOperaads`.
#[doc(alias = "BidderOperaads")]
pub const BIDDER_OPERAADS: BidderName = BidderName::from_static("operaads");
/// Go constant `BidderOptidigital`.
#[doc(alias = "BidderOptidigital")]
pub const BIDDER_OPTIDIGITAL: BidderName = BidderName::from_static("optidigital");
/// Go constant `BidderOraki`.
#[doc(alias = "BidderOraki")]
pub const BIDDER_ORAKI: BidderName = BidderName::from_static("oraki");
/// Go constant `BidderOrbidder`.
#[doc(alias = "BidderOrbidder")]
pub const BIDDER_ORBIDDER: BidderName = BidderName::from_static("orbidder");
/// Go constant `BidderOutbrain`.
#[doc(alias = "BidderOutbrain")]
pub const BIDDER_OUTBRAIN: BidderName = BidderName::from_static("outbrain");
/// Go constant `BidderOwnAdx`.
#[doc(alias = "BidderOwnAdx")]
pub const BIDDER_OWN_ADX: BidderName = BidderName::from_static("ownadx");
/// Go constant `BidderPangle`.
#[doc(alias = "BidderPangle")]
pub const BIDDER_PANGLE: BidderName = BidderName::from_static("pangle");
/// Go constant `BidderPanxo`.
#[doc(alias = "BidderPanxo")]
pub const BIDDER_PANXO: BidderName = BidderName::from_static("panxo");
/// Go constant `BidderPeak226`.
#[doc(alias = "BidderPeak226")]
pub const BIDDER_PEAK226: BidderName = BidderName::from_static("peak226");
/// Go constant `BidderPGAMSsp`.
#[doc(alias = "BidderPGAMSsp")]
pub const BIDDER_PGAM_SSP: BidderName = BidderName::from_static("pgamssp");
/// Go constant `BidderPixfuture`.
#[doc(alias = "BidderPixfuture")]
pub const BIDDER_PIXFUTURE: BidderName = BidderName::from_static("pixfuture");
/// Go constant `BidderPlaydigo`.
#[doc(alias = "BidderPlaydigo")]
pub const BIDDER_PLAYDIGO: BidderName = BidderName::from_static("playdigo");
/// Go constant `BidderProxistore`.
#[doc(alias = "BidderProxistore")]
pub const BIDDER_PROXISTORE: BidderName = BidderName::from_static("proxistore");
/// Go constant `BidderPubmatic`.
#[doc(alias = "BidderPubmatic")]
pub const BIDDER_PUBMATIC: BidderName = BidderName::from_static("pubmatic");
/// Go constant `BidderPubrise`.
#[doc(alias = "BidderPubrise")]
pub const BIDDER_PUBRISE: BidderName = BidderName::from_static("pubrise");
/// Go constant `BidderPubnative`.
#[doc(alias = "BidderPubnative")]
pub const BIDDER_PUBNATIVE: BidderName = BidderName::from_static("pubnative");
/// Go constant `BidderPulsepoint`.
#[doc(alias = "BidderPulsepoint")]
pub const BIDDER_PULSEPOINT: BidderName = BidderName::from_static("pulsepoint");
/// Go constant `BidderPWBid`.
#[doc(alias = "BidderPWBid")]
pub const BIDDER_PW_BID: BidderName = BidderName::from_static("pwbid");
/// Go constant `BidderQT`.
#[doc(alias = "BidderQT")]
pub const BIDDER_QT: BidderName = BidderName::from_static("qt");
/// Go constant `BidderReadpeak`.
#[doc(alias = "BidderReadpeak")]
pub const BIDDER_READPEAK: BidderName = BidderName::from_static("readpeak");
/// Go constant `BidderRediads`.
#[doc(alias = "BidderRediads")]
pub const BIDDER_REDIADS: BidderName = BidderName::from_static("rediads");
/// Go constant `BidderRelevantDigital`.
#[doc(alias = "BidderRelevantDigital")]
pub const BIDDER_RELEVANT_DIGITAL: BidderName = BidderName::from_static("relevantdigital");
/// Go constant `BidderResetDigital`.
#[doc(alias = "BidderResetDigital")]
pub const BIDDER_RESET_DIGITAL: BidderName = BidderName::from_static("resetdigital");
/// Go constant `BidderRevcontent`.
#[doc(alias = "BidderRevcontent")]
pub const BIDDER_REVCONTENT: BidderName = BidderName::from_static("revcontent");
/// Go constant `BidderRichaudience`.
#[doc(alias = "BidderRichaudience")]
pub const BIDDER_RICHAUDIENCE: BidderName = BidderName::from_static("richaudience");
/// Go constant `BidderRise`.
#[doc(alias = "BidderRise")]
pub const BIDDER_RISE: BidderName = BidderName::from_static("rise");
/// Go constant `BidderRoulax`.
#[doc(alias = "BidderRoulax")]
pub const BIDDER_ROULAX: BidderName = BidderName::from_static("roulax");
/// Go constant `BidderRTBHouse`.
#[doc(alias = "BidderRTBHouse")]
pub const BIDDER_RTB_HOUSE: BidderName = BidderName::from_static("rtbhouse");
/// Go constant `BidderRTBStack`.
#[doc(alias = "BidderRTBStack")]
pub const BIDDER_RTB_STACK: BidderName = BidderName::from_static("rtbstack");
/// Go constant `BidderRubicon`.
#[doc(alias = "BidderRubicon")]
pub const BIDDER_RUBICON: BidderName = BidderName::from_static("rubicon");
/// Go constant `BidderSeedingAlliance`.
#[doc(alias = "BidderSeedingAlliance")]
pub const BIDDER_SEEDING_ALLIANCE: BidderName = BidderName::from_static("seedingAlliance");
/// Go constant `BidderSeedtag`.
#[doc(alias = "BidderSeedtag")]
pub const BIDDER_SEEDTAG: BidderName = BidderName::from_static("seedtag");
/// Go constant `BidderSaLunaMedia`.
#[doc(alias = "BidderSaLunaMedia")]
pub const BIDDER_SA_LUNA_MEDIA: BidderName = BidderName::from_static("sa_lunamedia");
/// Go constant `BidderScalibur`.
#[doc(alias = "BidderScalibur")]
pub const BIDDER_SCALIBUR: BidderName = BidderName::from_static("scalibur");
/// Go constant `BidderSharethrough`.
#[doc(alias = "BidderSharethrough")]
pub const BIDDER_SHARETHROUGH: BidderName = BidderName::from_static("sharethrough");
/// Go constant `BidderShowheroes`.
#[doc(alias = "BidderShowheroes")]
pub const BIDDER_SHOWHEROES: BidderName = BidderName::from_static("showheroes");
/// Go constant `BidderSilverMob`.
#[doc(alias = "BidderSilverMob")]
pub const BIDDER_SILVER_MOB: BidderName = BidderName::from_static("silvermob");
/// Go constant `BidderSilverPush`.
#[doc(alias = "BidderSilverPush")]
pub const BIDDER_SILVER_PUSH: BidderName = BidderName::from_static("silverpush");
/// Go constant `BidderSmaato`.
#[doc(alias = "BidderSmaato")]
pub const BIDDER_SMAATO: BidderName = BidderName::from_static("smaato");
/// Go constant `BidderSmartAdserver`.
#[doc(alias = "BidderSmartAdserver")]
pub const BIDDER_SMART_ADSERVER: BidderName = BidderName::from_static("smartadserver");
/// Go constant `BidderSmartHub`.
#[doc(alias = "BidderSmartHub")]
pub const BIDDER_SMART_HUB: BidderName = BidderName::from_static("smarthub");
/// Go constant `BidderSmartRTB`.
#[doc(alias = "BidderSmartRTB")]
pub const BIDDER_SMART_RTB: BidderName = BidderName::from_static("smartrtb");
/// Go constant `BidderSmartx`.
#[doc(alias = "BidderSmartx")]
pub const BIDDER_SMARTX: BidderName = BidderName::from_static("smartx");
/// Go constant `BidderSmartyAds`.
#[doc(alias = "BidderSmartyAds")]
pub const BIDDER_SMARTY_ADS: BidderName = BidderName::from_static("smartyads");
/// Go constant `BidderSmileWanted`.
#[doc(alias = "BidderSmileWanted")]
pub const BIDDER_SMILE_WANTED: BidderName = BidderName::from_static("smilewanted");
/// Go constant `BidderSmoot`.
#[doc(alias = "BidderSmoot")]
pub const BIDDER_SMOOT: BidderName = BidderName::from_static("smoot");
/// Go constant `BidderSmrtconnect`.
#[doc(alias = "BidderSmrtconnect")]
pub const BIDDER_SMRTCONNECT: BidderName = BidderName::from_static("smrtconnect");
/// Go constant `BidderSonobi`.
#[doc(alias = "BidderSonobi")]
pub const BIDDER_SONOBI: BidderName = BidderName::from_static("sonobi");
/// Go constant `BidderSovrn`.
#[doc(alias = "BidderSovrn")]
pub const BIDDER_SOVRN: BidderName = BidderName::from_static("sovrn");
/// Go constant `BidderSovrnXsp`.
#[doc(alias = "BidderSovrnXsp")]
pub const BIDDER_SOVRN_XSP: BidderName = BidderName::from_static("sovrnXsp");
/// Go constant `BidderSparteo`.
#[doc(alias = "BidderSparteo")]
pub const BIDDER_SPARTEO: BidderName = BidderName::from_static("sparteo");
/// Go constant `BidderSspBC`.
#[doc(alias = "BidderSspBC")]
pub const BIDDER_SSP_BC: BidderName = BidderName::from_static("sspBC");
/// Go constant `BidderStackAdapt`.
#[doc(alias = "BidderStackAdapt")]
pub const BIDDER_STACK_ADAPT: BidderName = BidderName::from_static("stackadapt");
/// Go constant `BidderStartIO`.
#[doc(alias = "BidderStartIO")]
pub const BIDDER_START_IO: BidderName = BidderName::from_static("startio");
/// Go constant `BidderStroeerCore`.
#[doc(alias = "BidderStroeerCore")]
pub const BIDDER_STROEER_CORE: BidderName = BidderName::from_static("stroeerCore");
/// Go constant `BidderSuperEdge`.
#[doc(alias = "BidderSuperEdge")]
pub const BIDDER_SUPER_EDGE: BidderName = BidderName::from_static("superedge");
/// Go constant `BidderSynapseHX`.
#[doc(alias = "BidderSynapseHX")]
pub const BIDDER_SYNAPSE_HX: BidderName = BidderName::from_static("synapseHX");
/// Go constant `BidderTaboola`.
#[doc(alias = "BidderTaboola")]
pub const BIDDER_TABOOLA: BidderName = BidderName::from_static("taboola");
/// Go constant `BidderTAdvertisingBlis`.
#[doc(alias = "BidderTAdvertisingBlis")]
pub const BIDDER_T_ADVERTISING_BLIS: BidderName = BidderName::from_static("tadvertising_blis");
/// Go constant `BidderTappx`.
#[doc(alias = "BidderTappx")]
pub const BIDDER_TAPPX: BidderName = BidderName::from_static("tappx");
/// Go constant `BidderTargetVideo`.
#[doc(alias = "BidderTargetVideo")]
pub const BIDDER_TARGET_VIDEO: BidderName = BidderName::from_static("targetVideo");
/// Go constant `BidderTeads`.
#[doc(alias = "BidderTeads")]
pub const BIDDER_TEADS: BidderName = BidderName::from_static("teads");
/// Go constant `BidderTeal`.
#[doc(alias = "BidderTeal")]
pub const BIDDER_TEAL: BidderName = BidderName::from_static("teal");
/// Go constant `BidderTelaria`.
#[doc(alias = "BidderTelaria")]
pub const BIDDER_TELARIA: BidderName = BidderName::from_static("telaria");
/// Go constant `BidderTeqBlaze`.
#[doc(alias = "BidderTeqBlaze")]
pub const BIDDER_TEQ_BLAZE: BidderName = BidderName::from_static("teqblaze");
/// Go constant `BidderTheadx`.
#[doc(alias = "BidderTheadx")]
pub const BIDDER_THEADX: BidderName = BidderName::from_static("theadx");
/// Go constant `BidderTheTradeDesk`.
#[doc(alias = "BidderTheTradeDesk")]
pub const BIDDER_THE_TRADE_DESK: BidderName = BidderName::from_static("thetradedesk");
/// Go constant `BidderTpmn`.
#[doc(alias = "BidderTpmn")]
pub const BIDDER_TPMN: BidderName = BidderName::from_static("tpmn");
/// Go constant `BidderTradPlus`.
#[doc(alias = "BidderTradPlus")]
pub const BIDDER_TRAD_PLUS: BidderName = BidderName::from_static("tradplus");
/// Go constant `BidderTrafficGate`.
#[doc(alias = "BidderTrafficGate")]
pub const BIDDER_TRAFFIC_GATE: BidderName = BidderName::from_static("trafficgate");
/// Go constant `BidderTriplelift`.
#[doc(alias = "BidderTriplelift")]
pub const BIDDER_TRIPLELIFT: BidderName = BidderName::from_static("triplelift");
/// Go constant `BidderTripleliftNative`.
#[doc(alias = "BidderTripleliftNative")]
pub const BIDDER_TRIPLELIFT_NATIVE: BidderName = BidderName::from_static("triplelift_native");
/// Go constant `BidderTrustedstack`.
#[doc(alias = "BidderTrustedstack")]
pub const BIDDER_TRUSTEDSTACK: BidderName = BidderName::from_static("trustedstack");
/// Go constant `BidderTrustX`.
#[doc(alias = "BidderTrustX")]
pub const BIDDER_TRUST_X: BidderName = BidderName::from_static("trustx");
/// Go constant `BidderTunnl`.
#[doc(alias = "BidderTunnl")]
pub const BIDDER_TUNNL: BidderName = BidderName::from_static("tunnl");
/// Go constant `BidderUcfunnel`.
#[doc(alias = "BidderUcfunnel")]
pub const BIDDER_UCFUNNEL: BidderName = BidderName::from_static("ucfunnel");
/// Go constant `BidderUndertone`.
#[doc(alias = "BidderUndertone")]
pub const BIDDER_UNDERTONE: BidderName = BidderName::from_static("undertone");
/// Go constant `BidderUnicorn`.
#[doc(alias = "BidderUnicorn")]
pub const BIDDER_UNICORN: BidderName = BidderName::from_static("unicorn");
/// Go constant `BidderUnruly`.
#[doc(alias = "BidderUnruly")]
pub const BIDDER_UNRULY: BidderName = BidderName::from_static("unruly");
/// Go constant `BidderViant`.
#[doc(alias = "BidderViant")]
pub const BIDDER_VIANT: BidderName = BidderName::from_static("viant");
/// Go constant `BidderVidazoo`.
#[doc(alias = "BidderVidazoo")]
pub const BIDDER_VIDAZOO: BidderName = BidderName::from_static("vidazoo");
/// Go constant `BidderVideoByte`.
#[doc(alias = "BidderVideoByte")]
pub const BIDDER_VIDEO_BYTE: BidderName = BidderName::from_static("videobyte");
/// Go constant `BidderVideoHeroes`.
#[doc(alias = "BidderVideoHeroes")]
pub const BIDDER_VIDEO_HEROES: BidderName = BidderName::from_static("videoheroes");
/// Go constant `BidderVidoomy`.
#[doc(alias = "BidderVidoomy")]
pub const BIDDER_VIDOOMY: BidderName = BidderName::from_static("vidoomy");
/// Go constant `BidderVisibleMeasures`.
#[doc(alias = "BidderVisibleMeasures")]
pub const BIDDER_VISIBLE_MEASURES: BidderName = BidderName::from_static("visiblemeasures");
/// Go constant `BidderVisx`.
#[doc(alias = "BidderVisx")]
pub const BIDDER_VISX: BidderName = BidderName::from_static("visx");
/// Go constant `BidderVox`.
#[doc(alias = "BidderVox")]
pub const BIDDER_VOX: BidderName = BidderName::from_static("vox");
/// Go constant `BidderVrtcal`.
#[doc(alias = "BidderVrtcal")]
pub const BIDDER_VRTCAL: BidderName = BidderName::from_static("vrtcal");
/// Go constant `BidderVungle`.
#[doc(alias = "BidderVungle")]
pub const BIDDER_VUNGLE: BidderName = BidderName::from_static("vungle");
/// Go constant `BidderXeworks`.
#[doc(alias = "BidderXeworks")]
pub const BIDDER_XEWORKS: BidderName = BidderName::from_static("xeworks");
/// Go constant `BidderYahooAds`.
#[doc(alias = "BidderYahooAds")]
pub const BIDDER_YAHOO_ADS: BidderName = BidderName::from_static("yahooAds");
/// Go constant `BidderYandex`.
#[doc(alias = "BidderYandex")]
pub const BIDDER_YANDEX: BidderName = BidderName::from_static("yandex");
/// Go constant `BidderYeahmobi`.
#[doc(alias = "BidderYeahmobi")]
pub const BIDDER_YEAHMOBI: BidderName = BidderName::from_static("yeahmobi");
/// Go constant `BidderYieldlab`.
#[doc(alias = "BidderYieldlab")]
pub const BIDDER_YIELDLAB: BidderName = BidderName::from_static("yieldlab");
/// Go constant `BidderYieldmo`.
#[doc(alias = "BidderYieldmo")]
pub const BIDDER_YIELDMO: BidderName = BidderName::from_static("yieldmo");
/// Go constant `BidderYieldone`.
#[doc(alias = "BidderYieldone")]
pub const BIDDER_YIELDONE: BidderName = BidderName::from_static("yieldone");
/// Go constant `BidderZentotem`.
#[doc(alias = "BidderZentotem")]
pub const BIDDER_ZENTOTEM: BidderName = BidderName::from_static("zentotem");
/// Go constant `BidderZeroClickFraud`.
#[doc(alias = "BidderZeroClickFraud")]
pub const BIDDER_ZERO_CLICK_FRAUD: BidderName = BidderName::from_static("zeroclickfraud");
/// Go constant `BidderZetaGlobalSsp`.
#[doc(alias = "BidderZetaGlobalSsp")]
pub const BIDDER_ZETA_GLOBAL_SSP: BidderName = BidderName::from_static("zeta_global_ssp");
/// Go constant `BidderZmaticoo`.
#[doc(alias = "BidderZmaticoo")]
pub const BIDDER_ZMATICOO: BidderName = BidderName::from_static("zmaticoo");
/// Go: the initial `coreBidderNames`, in its order.
static CORE_BIDDER_NAMES: [BidderName; 282] = [
    BIDDER33_ACROSS,
    BIDDER_AAX,
    BIDDER_ACEEX,
    BIDDER_ACUITY_ADS,
    BIDDER_ADAGIO,
    BIDDER_ADELEMENT,
    BIDDER_ADF,
    BIDDER_ADGENERATION,
    BIDDER_ADHESE,
    BIDDER_ADKERNEL,
    BIDDER_ADKERNEL_ADN,
    BIDDER_ADMAN,
    BIDDER_ADMATIC,
    BIDDER_ADMIXER,
    BIDDER_ADNUNTIUS,
    BIDDER_ADOT,
    BIDDER_ADPONE,
    BIDDER_ADPRIME,
    BIDDER_ADQUERY,
    BIDDER_ADRINO,
    BIDDER_ADTARGET,
    BIDDER_ADTRGTME,
    BIDDER_ADTELLIGENT,
    BIDDER_AD_TONOS,
    BIDDER_AD_UP_TECH,
    BIDDER_ADVANGELISTS,
    BIDDER_ADVERXO,
    BIDDER_AD_VIEW,
    BIDDER_ADXCG,
    BIDDER_ADYOULIKE,
    BIDDER_AFRONT,
    BIDDER_AIDEM,
    BIDDER_AJA,
    BIDDER_AKCELO,
    BIDDER_ALGORIX,
    BIDDER_ALKIMI,
    BIDDER_ALLIANCE_GRAVITY,
    BIDDER_AMX,
    BIDDER_APACDEX,
    BIDDER_APPNEXUS,
    BIDDER_APPUSH,
    BIDDER_APS,
    BIDDER_ASO,
    BIDDER_AUDIENCE_NETWORK,
    BIDDER_AUTOMATAD,
    BIDDER_AVOCET,
    BIDDER_AXIS,
    BIDDER_AXONIX,
    BIDDER_BEACHFRONT,
    BIDDER_BEINTOO,
    BIDDER_BEMATTERFULL,
    BIDDER_BEOP,
    BIDDER_BETWEEN,
    BIDDER_BEYOND_MEDIA,
    BIDDER_BIDMACHINE,
    BIDDER_BIDMATIC,
    BIDDER_BIDMYADZ,
    BIDDER_BIDS_CUBE,
    BIDDER_BIDSTACK,
    BIDDER_BIDTHEATRE,
    BIDDER_BIGO_AD,
    BIDDER_BLASTO,
    BIDDER_BLIINK,
    BIDDER_BLIS,
    BIDDER_BLUE,
    BIDDER_BLUESEA,
    BIDDER_BMTM,
    BIDDER_BOLDWIN,
    BIDDER_BOLDWIN_RAPID,
    BIDDER_BRAVE,
    BIDDER_BWX,
    BIDDER_CADENT_APERTURE_MX,
    BIDDER_CCX,
    BIDDER_CLYDO,
    BIDDER_COINTRAFFIC,
    BIDDER_COINZILLA,
    BIDDER_COLOSSUS,
    BIDDER_COMPASS,
    BIDDER_CONCERT,
    BIDDER_CONNATIX,
    BIDDER_CONNECT_AD,
    BIDDER_CONSUMABLE,
    BIDDER_CONTXTFUL,
    BIDDER_CONVERSANT,
    BIDDER_COPPER6SSP,
    BIDDER_CPMSTAR,
    BIDDER_CRITEO,
    BIDDER_C_WIRE,
    BIDDER_DATABLOCKS,
    BIDDER_DECENTER_ADS,
    BIDDER_DEEPINTENT,
    BIDDER_DEFINEMEDIA,
    BIDDER_DIANOMI,
    BIDDER_DISPLAYIO,
    BIDDER_EDGE226,
    BIDDER_DMX,
    BIDDER_DRIFT_PIXEL,
    BIDDER_ELEMENTAL_TV,
    BIDDER_EMTV,
    BIDDER_EMX_DIGITAL,
    BIDDER_E_PLANNING,
    BIDDER_EPOM,
    BIDDER_ESCALAX,
    BIDDER_ESKIMI,
    BIDDER_E_VOLUTION,
    BIDDER_EXCO,
    BIDDER_EZOIC,
    BIDDER_FEED_AD,
    BIDDER_FERIO,
    BIDDER_FLATADS,
    BIDDER_FLIPP,
    BIDDER_FLOXIS,
    BIDDER_FREEWHEEL_SSP,
    BIDDER_FWSSP,
    BIDDER_FRVR_AD_NETWORK,
    BIDDER_GAMMA,
    BIDDER_GAMOSHI,
    BIDDER_GLOBALSUN,
    BIDDER_GOLDBACH,
    BIDDER_GRID,
    BIDDER_GUM_GUM,
    BIDDER_HUAWEI_ADS,
    BIDDER_IMDS,
    BIDDER_IMPACTIFY,
    BIDDER_IMPROVEDIGITAL,
    BIDDER_INFY_TV,
    BIDDER_IN_MOBI,
    BIDDER_INSTICATOR,
    BIDDER_INTENZE,
    BIDDER_INTERACTIVEOFFERS,
    BIDDER_INVIBES,
    BIDDER_IQX,
    BIDDER_IQ_ZONE,
    BIDDER_IX,
    BIDDER_JIXIE,
    BIDDER_KARGO,
    BIDDER_KAYZEN,
    BIDDER_KIDOZ,
    BIDDER_KIVIADS,
    BIDDER_LM_KIVIADS,
    BIDDER_KOBLER,
    BIDDER_KRUSHMEDIA,
    BIDDER_KUEEZ_RTB,
    BIDDER_LEMMADIGITAL,
    BIDDER_LIMELIGHT_DIGITAL,
    BIDDER_LOCKER_DOME,
    BIDDER_LOGAN,
    BIDDER_LOGICAD,
    BIDDER_LOOPME,
    BIDDER_LOYAL,
    BIDDER_LUNA_MEDIA,
    BIDDER_MABIDDER,
    BIDDER_MAD_SENSE,
    BIDDER_MADVERTISE,
    BIDDER_MARSMEDIA,
    BIDDER_MATTERFULL_RTB,
    BIDDER_MEDIAFUSE,
    BIDDER_MEDIA_GO,
    BIDDER_MEDIANET,
    BIDDER_MEDIASQUARE,
    BIDDER_MELO_ZEN,
    BIDDER_META_X,
    BIDDER_MGID,
    BIDDER_MGID_X,
    BIDDER_MICROSOFT,
    BIDDER_MINUTE_MEDIA,
    BIDDER_MISSENA,
    BIDDER_MOBFOXPB,
    BIDDER_MOBILE_FUSE,
    BIDDER_MOBKOI,
    BIDDER_MOTORIK,
    BIDDER_NATIVERY,
    BIDDER_NATIVO,
    BIDDER_NEXT_MILLENNIUM,
    BIDDER_NEXX360,
    BIDDER_NO_BID,
    BIDDER_ODEEO,
    BIDDER_OGURY,
    BIDDER_OMS,
    BIDDER_ONE_TAG,
    BIDDER_OPEN_WEB,
    BIDDER_OPENX,
    BIDDER_OPERAADS,
    BIDDER_OPTIDIGITAL,
    BIDDER_ORAKI,
    BIDDER_ORBIDDER,
    BIDDER_OUTBRAIN,
    BIDDER_OWN_ADX,
    BIDDER_PANGLE,
    BIDDER_PANXO,
    BIDDER_PEAK226,
    BIDDER_PGAM_SSP,
    BIDDER_PIXFUTURE,
    BIDDER_PLAYDIGO,
    BIDDER_PROXISTORE,
    BIDDER_PUBMATIC,
    BIDDER_PUBRISE,
    BIDDER_PUBNATIVE,
    BIDDER_PULSEPOINT,
    BIDDER_PW_BID,
    BIDDER_QT,
    BIDDER_READPEAK,
    BIDDER_REDIADS,
    BIDDER_RELEVANT_DIGITAL,
    BIDDER_RESET_DIGITAL,
    BIDDER_REVCONTENT,
    BIDDER_RICHAUDIENCE,
    BIDDER_RISE,
    BIDDER_ROULAX,
    BIDDER_RTB_HOUSE,
    BIDDER_RTB_STACK,
    BIDDER_RUBICON,
    BIDDER_SEEDING_ALLIANCE,
    BIDDER_SEEDTAG,
    BIDDER_SA_LUNA_MEDIA,
    BIDDER_SCALIBUR,
    BIDDER_SHARETHROUGH,
    BIDDER_SHOWHEROES,
    BIDDER_SILVER_MOB,
    BIDDER_SILVER_PUSH,
    BIDDER_SMAATO,
    BIDDER_SMART_ADSERVER,
    BIDDER_SMART_HUB,
    BIDDER_SMART_RTB,
    BIDDER_SMARTX,
    BIDDER_SMARTY_ADS,
    BIDDER_SMILE_WANTED,
    BIDDER_SMOOT,
    BIDDER_SMRTCONNECT,
    BIDDER_SONOBI,
    BIDDER_SOVRN,
    BIDDER_SOVRN_XSP,
    BIDDER_SPARTEO,
    BIDDER_SSP_BC,
    BIDDER_STACK_ADAPT,
    BIDDER_START_IO,
    BIDDER_STROEER_CORE,
    BIDDER_SUPER_EDGE,
    BIDDER_SYNAPSE_HX,
    BIDDER_TABOOLA,
    BIDDER_T_ADVERTISING_BLIS,
    BIDDER_TAPPX,
    BIDDER_TARGET_VIDEO,
    BIDDER_TEADS,
    BIDDER_TEAL,
    BIDDER_TELARIA,
    BIDDER_TEQ_BLAZE,
    BIDDER_THEADX,
    BIDDER_THE_TRADE_DESK,
    BIDDER_TPMN,
    BIDDER_TRAD_PLUS,
    BIDDER_TRAFFIC_GATE,
    BIDDER_TRIPLELIFT,
    BIDDER_TRIPLELIFT_NATIVE,
    BIDDER_TRUSTEDSTACK,
    BIDDER_TRUST_X,
    BIDDER_TUNNL,
    BIDDER_UCFUNNEL,
    BIDDER_UNDERTONE,
    BIDDER_UNICORN,
    BIDDER_UNRULY,
    BIDDER_VIANT,
    BIDDER_VIDAZOO,
    BIDDER_VIDEO_BYTE,
    BIDDER_VIDEO_HEROES,
    BIDDER_VIDOOMY,
    BIDDER_VISIBLE_MEASURES,
    BIDDER_VISX,
    BIDDER_VOX,
    BIDDER_VRTCAL,
    BIDDER_VUNGLE,
    BIDDER_XEWORKS,
    BIDDER_YAHOO_ADS,
    BIDDER_YANDEX,
    BIDDER_YEAHMOBI,
    BIDDER_YIELDLAB,
    BIDDER_YIELDMO,
    BIDDER_YIELDONE,
    BIDDER_ZENTOTEM,
    BIDDER_ZERO_CLICK_FRAUD,
    BIDDER_ZETA_GLOBAL_SSP,
    BIDDER_ZMATICOO,
];
