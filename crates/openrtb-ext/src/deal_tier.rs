//! Go: `openrtb_ext/deal_tier.go`: `DealTierBidderMap` and reading the bidders' deal tiers of an
//! impression (`DealTier` is generated).

use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_gojson::{GoJson, jsonutil};
use pbs_openrtb::openrtb2;

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

/// The unnamed struct type of `bidder` values in Go's `ReadDealTiersFromImp`.
#[derive(Clone, Debug, Default, GoJson)]
#[gojson(
    go_type = "struct { DealTier *openrtb_ext.DealTier \"json:\\\"dealTier\\\"\" }",
    anonymous
)]
struct BidderDealTier {
    #[gojson(go = "DealTier", json = "dealTier")]
    deal_tier: Option<DealTier>,
}

/// The unnamed struct type of `prebid` in Go's `ReadDealTiersFromImp`.
#[derive(Clone, Debug, Default, GoJson)]
#[gojson(
    go_type = "struct { Bidders map[string]struct { DealTier *openrtb_ext.DealTier \
               \"json:\\\"dealTier\\\"\" } \"json:\\\"bidder\\\"\" }",
    anonymous
)]
struct PrebidDealTiers {
    #[gojson(go = "Bidders", json = "bidder")]
    bidders: Option<BTreeMap<String, BidderDealTier>>,
}

/// The unnamed struct type Go's `ReadDealTiersFromImp` decodes `imp.ext` into.
#[derive(Clone, Debug, Default, GoJson)]
#[gojson(
    go_type = "struct { Prebid struct { Bidders map[string]struct { DealTier \
               *openrtb_ext.DealTier \"json:\\\"dealTier\\\"\" } \"json:\\\"bidder\\\"\" } \
               \"json:\\\"prebid\\\"\" }",
    anonymous
)]
struct ImpPrebidExt {
    #[gojson(go = "Prebid", json = "prebid")]
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
/// # Errors
///
/// `jsonutil.Unmarshal`'s `FailedToUnmarshal` when `imp.ext` does not decode (Go then returns
/// a nil map).
pub fn read_deal_tiers_from_imp(imp: &openrtb2::Imp) -> Result<DealTierBidderMap, GoError> {
    // Go: openrtb_ext/deal_tier.go:22
    let mut deal_tiers = BTreeMap::new();
    if imp.ext.is_empty() {
        return Ok(DealTierBidderMap(Some(deal_tiers)));
    }
    let mut imp_prebid_ext = ImpPrebidExt::default();
    if let Err(err) = jsonutil::unmarshal(imp.ext.as_bytes(), &mut imp_prebid_ext) {
        return Err(GoError::failed_to_unmarshal(err.message.to_string_lossy()));
    }
    for (bidder, param) in imp_prebid_ext.prebid.bidders.into_iter().flatten() {
        if let Some(deal_tier) = param.deal_tier {
            let name = normalize_bidder_name(&bidder).unwrap_or_else(|| BidderName::from(bidder));
            drop(deal_tiers.insert(name, deal_tier));
        }
    }
    Ok(DealTierBidderMap(Some(deal_tiers)))
}
