//! Map iteration order (Go: `parity/fakes/order.go`, `fakes.json` `orders`).
//!
//! Some Go code ranges over a map whose iteration order Go randomizes, and the order changes
//! the outcome: adapters that overwrite a shared request's `Imp` with each imp group (so
//! `MakeBids` sees the last group visited), validators that report the first failure, the
//! auction's tie-breaks. In parity mode those sites visit the keys in the ascending order the
//! contract names for each one ([`OrderSite`]); otherwise they keep the map's own order.

use core::cmp::Ordering;

use super::active;

/// Every named order of the contract (`fakes.json` `orders`), in registry order.
///
/// Most are [`map_keys`] sites. Two order concurrent work instead and belong to the exchange:
/// [`OrderSite::BidderRequestPhase`] and [`OrderSite::BidderResponseOrder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrderSite {
    /// `bidder_request_phase`: the bidders of one auction build their requests one at a time in
    /// bidder name order (`MakeRequests` and the bidder-request hooks), while HTTP calls and
    /// `MakeBids` still run concurrently (Go: `exchange/parity_turns.go`).
    BidderRequestPhase,
    /// `fpd_bidders`: first party data is resolved for the bidders in ascending name order.
    FpdBidders,
    /// `bidder_response_order`: a bidder's responses reach `MakeBids` in call order (the
    /// `MakeRequests` output order, then the stored bid responses by imp ID), whatever order
    /// they complete in (Go: `exchange/parity_stages.go` `inCallOrder`).
    BidderResponseOrder,
    /// `bidder_params_validation`: imp ext validation checks the imp's bidders in ascending name
    /// order.
    BidderParamsValidation,
    /// `stored_bid_responses_validation`: the bidders of an imp's stored bid responses are
    /// checked in ascending name order.
    StoredBidResponsesValidation,
    /// `alias_validation`: `ext.prebid.aliases` are checked in ascending alias order.
    AliasValidation,
    /// `alias_gvlid_validation`: `ext.prebid.aliasgvlids` are checked in ascending alias order.
    AliasGvlidValidation,
    /// `bid_adjustment_factors_validation`: `ext.prebid.bidadjustmentfactors` are checked in
    /// ascending bidder order.
    BidAdjustmentFactorsValidation,
    /// `auction_winners`: the auction visits the seats in ascending bidder name order when it
    /// picks each imp's winning bid.
    AuctionWinners,
    /// `category_dedupe`: category mapping visits the seats in ascending bidder name order.
    CategoryDedupe,
    /// `adtarget`: imp groups ascending by source ID.
    Adtarget,
    /// `bidmatic`: imp groups ascending by source ID.
    Bidmatic,
    /// `adtelligent`: imp groups ascending by source ID.
    Adtelligent,
    /// `datablocks`: imp groups ascending by source ID only (Go: `fakes.LessDatablocks`).
    Datablocks,
    /// `trafficgate`: imp groups ascending by (placement ID, host) (Go:
    /// `fakes.LessTrafficGate`; the placement ID is a string).
    Trafficgate,
    /// `zeroclickfraud`: imp groups ascending by (source ID, host) (Go:
    /// `fakes.LessZeroClickFraud`).
    Zeroclickfraud,
}

impl OrderSite {
    /// Every order in registry order.
    pub const ALL: [Self; 16] = [
        Self::BidderRequestPhase,
        Self::FpdBidders,
        Self::BidderResponseOrder,
        Self::BidderParamsValidation,
        Self::StoredBidResponsesValidation,
        Self::AliasValidation,
        Self::AliasGvlidValidation,
        Self::BidAdjustmentFactorsValidation,
        Self::AuctionWinners,
        Self::CategoryDedupe,
        Self::Adtarget,
        Self::Bidmatic,
        Self::Adtelligent,
        Self::Datablocks,
        Self::Trafficgate,
        Self::Zeroclickfraud,
    ];

    /// The registry name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BidderRequestPhase => "bidder_request_phase",
            Self::FpdBidders => "fpd_bidders",
            Self::BidderResponseOrder => "bidder_response_order",
            Self::BidderParamsValidation => "bidder_params_validation",
            Self::StoredBidResponsesValidation => "stored_bid_responses_validation",
            Self::AliasValidation => "alias_validation",
            Self::AliasGvlidValidation => "alias_gvlid_validation",
            Self::BidAdjustmentFactorsValidation => "bid_adjustment_factors_validation",
            Self::AuctionWinners => "auction_winners",
            Self::CategoryDedupe => "category_dedupe",
            Self::Adtarget => "adtarget",
            Self::Bidmatic => "bidmatic",
            Self::Adtelligent => "adtelligent",
            Self::Datablocks => "datablocks",
            Self::Trafficgate => "trafficgate",
            Self::Zeroclickfraud => "zeroclickfraud",
        }
    }

    /// Whether the site orders map keys with [`map_keys`] (the others order concurrent work).
    #[must_use]
    pub const fn is_map_keys(self) -> bool {
        !matches!(self, Self::BidderRequestPhase | Self::BidderResponseOrder)
    }

    /// The order spelled `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.name() == name)
    }
}

/// The keys of a map in the order a site ranges over them (Go: `fakes.MapKeys`).
///
/// `keys` is the map's own iteration (`map.keys().cloned()`). While the fakes are installed
/// (parity mode) the keys are sorted ascending by `less`; otherwise they keep the order `keys`
/// yields, which is what ranging over the map gives. Go sorts with `sort.Slice`, which is not
/// stable: keys that `less` cannot tell apart (two datablocks groups with the same source ID)
/// have no defined order in Go either.
#[must_use]
pub fn map_keys<K, I, F>(keys: I, mut less: F) -> Vec<K>
where
    I: IntoIterator<Item = K>,
    F: FnMut(&K, &K) -> bool,
{
    let mut keys: Vec<K> = keys.into_iter().collect();
    if active().is_some() {
        keys.sort_by(|a, b| {
            if less(a, b) {
                Ordering::Less
            } else if less(b, a) {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
    }
    keys
}
