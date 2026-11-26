//! Go: `openrtb_ext/deal_tier.go`: `DealTierBidderMap` and reading the bidders' deal tiers of an
//! impression (`DealTier` is generated).

use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_jsonutil::{null_default, unmarshal};
use pbs_openrtb::openrtb2;
use serde::Deserialize;

use crate::{
    bidders::{BidderName, normalize_bidder_name},
    generated::DealTier,
    named_map::named_map,
};

/// `DealTierBidderMap` defines a correlation between bidders and deal tiers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DealTierBidderMap(pub Option<BTreeMap<BidderName, DealTier>>);

named_map!(
    DealTierBidderMap,
    "openrtb_ext.DealTierBidderMap",
    BidderName,
    DealTier
);

/// The unnamed struct type of `bidder` values in Go's `ReadDealTiersFromImp`
/// (`struct { DealTier *DealTier "json:\"dealTier\"" }`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BidderDealTier {
    /// Go field `DealTier`.
    #[serde(rename = "dealTier")]
    deal_tier: Option<DealTier>,
}

/// The unnamed struct type of `prebid` in Go's `ReadDealTiersFromImp`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PrebidDealTiers {
    /// Go field `Bidders`. A bidder whose value is null holds Go's zero struct, which has no
    /// tier: `None` here.
    #[serde(rename = "bidder")]
    bidders: Option<BTreeMap<String, Option<BidderDealTier>>>,
}

/// The unnamed struct type Go's `ReadDealTiersFromImp` decodes `imp.ext` into.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ImpPrebidExt {
    /// Go field `Prebid`, a struct value: null reads as its zero value.
    #[serde(deserialize_with = "null_default")]
    prebid: PrebidDealTiers,
}

/// Go: `ReadDealTiersFromImp(imp)` returns a map of bidder deal tiers read from the impression
/// of an original request (not split / cleaned).
///
/// The tiers are `imp.ext.prebid.bidder.<bidder>.dealTier`, keyed by the normalized bidder name
/// when the bidder is known and by the name as written otherwise. An empty `imp.ext` gives an
/// empty map.
///
/// When two names normalize to the same bidder, Go keeps whichever its random map order visits
/// last; here the names are visited in byte order.
///
/// `imp.ext` is decoded with `pbs_jsonutil::unmarshal` (Go: `jsonutil.Unmarshal`), whose keys
/// match the fields exactly where jsoniter's match whatever their case (`json.key-case`).
///
/// # Errors
///
/// `pbs_jsonutil::unmarshal`'s `FailedToUnmarshal` when `imp.ext` does not decode (Go then
/// returns a nil map).
pub fn read_deal_tiers_from_imp(imp: &openrtb2::Imp) -> Result<DealTierBidderMap, GoError> {
    // Go: openrtb_ext/deal_tier.go:22
    let mut deal_tiers = BTreeMap::new();
    if imp.ext.is_empty() {
        return Ok(DealTierBidderMap(Some(deal_tiers)));
    }
    let imp_prebid_ext: ImpPrebidExt = unmarshal(imp.ext.as_bytes())?;
    for (bidder, param) in imp_prebid_ext.prebid.bidders.into_iter().flatten() {
        if let Some(deal_tier) = param.and_then(|param| param.deal_tier) {
            let name = normalize_bidder_name(&bidder).unwrap_or_else(|| BidderName::from(bidder));
            drop(deal_tiers.insert(name, deal_tier));
        }
    }
    Ok(DealTierBidderMap(Some(deal_tiers)))
}
