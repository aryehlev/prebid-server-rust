//! Stage, decision and outbound-call identifiers (Go: `parity/ids.go`), with the registry
//! metadata of `stages.json` and `decisions.json`.
//!
//! The IDs are the cross-language contract: both servers record the same stage IDs and read the
//! same decision IDs. The tests check every table here against the registry JSON vendored under
//! `contract/`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::contracts::Domain;

/// How the entries of a stage are keyed (`stages.json` `keyed_by`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyedBy {
    /// `_`: one entry under [`UNKEYED_STAGE`].
    Unkeyed,
    /// `bidder`: one entry per bidder name.
    Bidder,
    /// `seat`: one entry per seat.
    Seat,
    /// `bidder#call`: one entry per bidder and zero-based call index in `MakeRequests` order
    /// (`appnexus#0`, or `appnexus#unindexed`).
    BidderCall,
}

impl KeyedBy {
    /// The registry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unkeyed => "_",
            Self::Bidder => "bidder",
            Self::Seat => "seat",
            Self::BidderCall => "bidder#call",
        }
    }
}

/// The trace member that holds a stage (`stages.json` `trace_field`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TraceField {
    /// `stages`: `trace.stages[<stage>][<key>]`.
    Stages,
    /// `introspection`: compared through `GET /__parity/config` and `/__parity/bidders`.
    Introspection,
    /// `http_in`
    HttpIn,
    /// `http_out`
    HttpOut,
    /// `analytics`
    Analytics,
    /// `metrics_delta`
    MetricsDelta,
    /// `http_response`
    HttpResponse,
}

impl TraceField {
    /// The registry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stages => "stages",
            Self::Introspection => "introspection",
            Self::HttpIn => "http_in",
            Self::HttpOut => "http_out",
            Self::Analytics => "analytics",
            Self::MetricsDelta => "metrics_delta",
            Self::HttpResponse => "http_response",
        }
    }
}

macro_rules! stages {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $id:literal, $domain:ident, $keyed:ident, $field:ident;
    )+) => {
        /// One capture point (Go: `parity.StageID`), in `stages.json` order: the order defines the
        /// first diverging stage of a case.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum StageId {
            $(
                $(#[$attr])*
                #[doc = concat!("`", $id, "`")]
                $variant,
            )+
        }

        impl StageId {
            /// Every stage in `stages.json` order (Go: `parity.AllStages`).
            pub const ALL: [Self; 45] = [$(Self::$variant,)+];

            /// The stage ID.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            /// The owning matrix domain.
            #[must_use]
            pub const fn domain(self) -> Domain {
                match self {
                    $(Self::$variant => Domain::$domain,)+
                }
            }

            /// How the stage's entries are keyed.
            #[must_use]
            pub const fn keyed_by(self) -> KeyedBy {
                match self {
                    $(Self::$variant => KeyedBy::$keyed,)+
                }
            }

            /// The trace member holding the stage.
            #[must_use]
            pub const fn trace_field(self) -> TraceField {
                match self {
                    $(Self::$variant => TraceField::$field,)+
                }
            }
        }
    };
}

stages! {
    #[default]
    ConfigResolved = "config.resolved", ConfigBootstrap, Unkeyed, Introspection;
    BiddersResolved = "bidders.resolved", ConfigBootstrap, Unkeyed, Introspection;
    HttpIn = "http.in", Endpoints, Unkeyed, HttpIn;
    HooksEntrypoint = "hooks.entrypoint", HooksModules, Unkeyed, Stages;
    StoredFetch = "stored.fetch", StoredData, Unkeyed, Stages;
    AccountResolved = "account.resolved", StoredData, Unkeyed, Stages;
    HooksRawAuction = "hooks.raw_auction", HooksModules, Unkeyed, Stages;
    RequestMerged = "request.merged", StoredData, Unkeyed, Stages;
    RequestNormalized = "request.normalized", AuctionLifecycle, Unkeyed, Stages;
    StoredResponses = "stored.responses", CacheSupportFlows, Unkeyed, Stages;
    RequestValidated = "request.validated", AuctionLifecycle, Unkeyed, Stages;
    GdprContext = "gdpr.context", PolicyPrivacyIdentity, Unkeyed, Stages;
    HooksProcessedAuction = "hooks.processed_auction", HooksModules, Unkeyed, Stages;
    AuctionSetup = "auction.setup", AuctionLifecycle, Unkeyed, Stages;
    FloorsEnrich = "floors.enrich", PolicyPrivacyIdentity, Unkeyed, Stages;
    FpdResolved = "fpd.resolved", PolicyPrivacyIdentity, Unkeyed, Stages;
    PrivacyLabels = "privacy.labels", PolicyPrivacyIdentity, Unkeyed, Stages;
    PrivacyBidder = "privacy.bidder", PolicyPrivacyIdentity, Bidder, Stages;
    BidderRequest = "bidder.request", BiddersAdapters, Bidder, Stages;
    HooksBidderRequest = "hooks.bidder_request", HooksModules, Bidder, Stages;
    AdapterRequests = "adapter.requests", BiddersAdapters, Bidder, Stages;
    AdapterRequestsFinal = "adapter.requests_final", BiddersAdapters, Bidder, Stages;
    HttpOut = "http.out", BiddersAdapters, Unkeyed, HttpOut;
    AdapterBids = "adapter.bids", BiddersAdapters, BidderCall, Stages;
    HooksRawBidderResponse = "hooks.raw_bidder_response", HooksModules, BidderCall, Stages;
    BidsAdjusted = "bids.adjusted", PolicyPrivacyIdentity, Seat, Stages;
    BidderResult = "bidder.result", AuctionLifecycle, Bidder, Stages;
    FloorsEnforce = "floors.enforce", PolicyPrivacyIdentity, Unkeyed, Stages;
    CategoryMapping = "category.mapping", PolicyPrivacyIdentity, Unkeyed, Stages;
    EventsTracking = "events.tracking", CacheSupportFlows, Unkeyed, Stages;
    HooksAllProcessedBidResponses = "hooks.all_processed_bid_responses",
        HooksModules, Unkeyed, Stages;
    AuctionWinners = "auction.winners", PolicyPrivacyIdentity, Unkeyed, Stages;
    CacheResult = "cache.result", CacheSupportFlows, Unkeyed, Stages;
    Targeting = "targeting", PolicyPrivacyIdentity, Unkeyed, Stages;
    SeatNonBids = "seat_non_bids", AuctionLifecycle, Unkeyed, Stages;
    ExchangeResponse = "exchange.response", AuctionLifecycle, Unkeyed, Stages;
    VideoPods = "video.pods", Endpoints, Unkeyed, Stages;
    VideoCache = "video.cache", CacheSupportFlows, Unkeyed, Stages;
    AmpTargetingResponse = "amp.targeting_response", Endpoints, Unkeyed, Stages;
    HooksAuctionResponse = "hooks.auction_response", HooksModules, Unkeyed, Stages;
    HooksExitpoint = "hooks.exitpoint", HooksModules, Unkeyed, Stages;
    HookOutcomes = "hook_outcomes", HooksModules, Unkeyed, Stages;
    Analytics = "analytics", CutoverProof, Unkeyed, Analytics;
    MetricsDelta = "metrics_delta", CutoverProof, Unkeyed, MetricsDelta;
    HttpResponse = "http.response", Endpoints, Unkeyed, HttpResponse;
}

impl StageId {
    /// The stage's `stages.json` order (1-based).
    #[must_use]
    pub const fn order(self) -> usize {
        self as usize + 1
    }

    /// The stage with ID `name`, or `None` when it is not registered.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }

    /// The stage's bit in a 64-bit stage set.
    #[must_use]
    pub const fn bit(self) -> u64 {
        1 << (self as u32)
    }
}

/// The key of a stage that is not keyed by bidder, seat or call (Go: `parity.UnkeyedStage`). An
/// empty key records under it.
pub const UNKEYED_STAGE: &str = "_";

/// Whether a stage was observed during one case (Go: `parity.StageStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StageStatus {
    /// `captured`: the implementation recorded at least one key.
    Captured,
    /// `not_reached`: the implementation supports the stage but the case did not reach it.
    #[default]
    NotReached,
    /// `unimplemented`: the implementation has no equivalent capture point yet (Rust only; Go is
    /// the source of truth and never reports it).
    Unimplemented,
}

impl StageStatus {
    /// Every status.
    pub const ALL: [Self; 3] = [Self::Captured, Self::NotReached, Self::Unimplemented];

    /// The status as the trace spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::NotReached => "not_reached",
            Self::Unimplemented => "unimplemented",
        }
    }

    /// The status spelled `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }
}

/// The value domain of a decision point (`decisions.json` `domain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecisionDomain {
    /// `binary`: the integer 0 or 1.
    Binary,
    /// `index`: a zero-based integer.
    Index,
    /// `enum`: one of the listed strings.
    Enum,
}

impl DecisionDomain {
    /// The registry spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Index => "index",
            Self::Enum => "enum",
        }
    }
}

/// One point where Go consults a random source on the request path (Go: `parity.DecisionID`),
/// in `decisions.json` order. The envelope can pin each one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecisionId {
    /// `floors.skip`: 1 skips floors (`ext.prebid.floors.skipped`).
    #[default]
    FloorsSkip,
    /// `floors.data_rate`: 1 uses the fetched floors data.
    FloorsDataRate,
    /// `floors.modelgroup`: the index of the winning model group.
    FloorsModelGroup,
    /// `floors.enforce`: 1 enforces floors.
    FloorsEnforce,
    /// `category.dedupe`: 1 drops the earlier (dupe) bid and keeps the current one.
    CategoryDedupe,
    /// `pricegranularity.timesplit`: 1 floors the increments, 0 ceils them.
    PriceGranularityTimesplit,
    /// `usersync.shuffle`: `identity` keeps the bidder order.
    UsersyncShuffle,
}

impl DecisionId {
    /// Every decision point in `decisions.json` order (Go: `parity.AllDecisions`).
    pub const ALL: [Self; 7] = [
        Self::FloorsSkip,
        Self::FloorsDataRate,
        Self::FloorsModelGroup,
        Self::FloorsEnforce,
        Self::CategoryDedupe,
        Self::PriceGranularityTimesplit,
        Self::UsersyncShuffle,
    ];

    /// The decision ID.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FloorsSkip => "floors.skip",
            Self::FloorsDataRate => "floors.data_rate",
            Self::FloorsModelGroup => "floors.modelgroup",
            Self::FloorsEnforce => "floors.enforce",
            Self::CategoryDedupe => "category.dedupe",
            Self::PriceGranularityTimesplit => "pricegranularity.timesplit",
            Self::UsersyncShuffle => "usersync.shuffle",
        }
    }

    /// The decision's value domain.
    #[must_use]
    pub const fn domain(self) -> DecisionDomain {
        match self {
            Self::FloorsSkip
            | Self::FloorsDataRate
            | Self::FloorsEnforce
            | Self::CategoryDedupe
            | Self::PriceGranularityTimesplit => DecisionDomain::Binary,
            Self::FloorsModelGroup => DecisionDomain::Index,
            Self::UsersyncShuffle => DecisionDomain::Enum,
        }
    }

    /// The allowed envelope values of an `enum` point (Go: `trace.enumDecisions`); empty for
    /// the other domains.
    #[must_use]
    pub const fn enum_values(self) -> &'static [&'static str] {
        match self {
            Self::UsersyncShuffle => &[USERSYNC_SHUFFLE_IDENTITY],
            Self::FloorsSkip
            | Self::FloorsDataRate
            | Self::FloorsModelGroup
            | Self::FloorsEnforce
            | Self::CategoryDedupe
            | Self::PriceGranularityTimesplit => &[],
        }
    }

    /// The value a parity-mode server (fakes installed) takes at the site for a request outside
    /// an exec case, on its main port, where no envelope pins the point and nothing is recorded
    /// (`decisions.json` `outside_case`). `None` means the site draws from the random source
    /// there, as in production. Only `usersync.shuffle` has one, `identity`, so two parity-mode
    /// servers answer `/cookie_sync` alike (Go: the `usersync/shuffler.go` seam).
    #[must_use]
    pub const fn outside_case(self) -> Option<&'static str> {
        match self {
            Self::UsersyncShuffle => Some(USERSYNC_SHUFFLE_IDENTITY),
            Self::FloorsSkip
            | Self::FloorsDataRate
            | Self::FloorsModelGroup
            | Self::FloorsEnforce
            | Self::CategoryDedupe
            | Self::PriceGranularityTimesplit => None,
        }
    }

    /// The decision point with ID `name`, or `None` when it is not registered.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

/// `trace.decisions[].source` of a value read from the envelope
/// (Go: `parity.DecisionSourceEnvelope`).
pub const DECISION_SOURCE_ENVELOPE: &str = "envelope";
/// `trace.decisions[].source` of a value drawn from the real random source
/// (Go: `parity.DecisionSourceRNG`).
pub const DECISION_SOURCE_RNG: &str = "rng";
/// The only non-random value of `usersync.shuffle` (Go: `parity.UsersyncShuffleIdentity`).
pub const USERSYNC_SHUFFLE_IDENTITY: &str = "identity";

/// The kind of an outbound call (Go: the `parity.Outbound*` constants).
///
/// A call made with a bidder in its context is always [`OutboundKind::Bidder`]; otherwise it is
/// the label of the client it was sent with, the shared general client refined by URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutboundKind {
    /// `bidder`
    Bidder,
    /// `cache`
    Cache,
    /// `floors`
    Floors,
    /// `currency`
    Currency,
    /// `vendorlist`
    VendorList,
    /// `stored`
    Stored,
    /// `module`
    Module,
    /// `general`
    General,
    /// `default`
    Default,
}

impl OutboundKind {
    /// Every kind in schema order (Go: `parity.AllOutboundKinds`).
    pub const ALL: [Self; 9] = [
        Self::Bidder,
        Self::Cache,
        Self::Floors,
        Self::Currency,
        Self::VendorList,
        Self::Stored,
        Self::Module,
        Self::General,
        Self::Default,
    ];

    /// The kind as documents spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bidder => "bidder",
            Self::Cache => "cache",
            Self::Floors => "floors",
            Self::Currency => "currency",
            Self::VendorList => "vendorlist",
            Self::Stored => "stored",
            Self::Module => "module",
            Self::General => "general",
            Self::Default => "default",
        }
    }

    /// The kind spelled `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

/// `mock.mode` oracle: outbound calls go to the oracle (Go: `parity.MockModeOracle`).
pub const MOCK_MODE_ORACLE: &str = "oracle";
/// `mock.mode` inline: outbound calls are answered in-process (Go: `parity.MockModeInline`).
pub const MOCK_MODE_INLINE: &str = "inline";
/// `mock.level` http (Go: `parity.MockLevelHTTP`).
pub const MOCK_LEVEL_HTTP: &str = "http";
/// `mock.level` `bidder_response` (Go: `parity.MockLevelBidderResp`).
pub const MOCK_LEVEL_BIDDER_RESPONSE: &str = "bidder_response";
/// `mock.source` of an inline response (Go: `parity.MockSourceInline`).
pub const MOCK_SOURCE_INLINE: &str = "inline";
/// `mock.source` of an inline rule (Go: `parity.MockSourceRule`).
pub const MOCK_SOURCE_RULE: &str = "rule";
/// `mock.source` of a replayed fixture (Go: `parity.MockSourceFixture`).
pub const MOCK_SOURCE_FIXTURE: &str = "fixture_replay";
/// `mock.source` of an oracle template (Go: `parity.MockSourceTemplate`).
pub const MOCK_SOURCE_TEMPLATE: &str = "template";
/// `mock.source` of a built-in default (Go: `parity.MockSourceDefault`).
pub const MOCK_SOURCE_DEFAULT: &str = "default";
/// `mock.source` of a static oracle answer (Go: `parity.MockSourceStatic`).
pub const MOCK_SOURCE_STATIC: &str = "static";
/// An outbound call that timed out (Go: `parity.OutboundErrTimeout`).
pub const OUTBOUND_ERR_TIMEOUT: &str = "timeout";
/// An outbound call whose connection was refused (Go: `parity.OutboundErrRefused`).
pub const OUTBOUND_ERR_REFUSED: &str = "refused";
/// An outbound call whose connection was reset (Go: `parity.OutboundErrReset`).
pub const OUTBOUND_ERR_RESET: &str = "reset";
/// An outbound call refused by inline mode (Go: `parity.OutboundErrNetDisabled`).
pub const OUTBOUND_ERR_NETWORK_DISABLED: &str = "network_disabled";
/// Any other outbound failure (Go: `parity.OutboundErrOther`).
pub const OUTBOUND_ERR_OTHER: &str = "error";

/// JSON for the closed string enums that documents carry where Go has a named string type: the
/// ID string, as a value and as a map key.
///
/// Go's string types accept any value; these enums cannot hold an unregistered one, so reading
/// it fails the document, naming the value. Only the Go runner writes the documents that carry
/// them, from the same registry, so Rust never takes that path in practice.
macro_rules! string_enum {
    ($ty:ident, $go:literal) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Name;

                impl Visitor<'_> for Name {
                    type Value = $ty;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str(concat!("a registered ", $go))
                    }

                    fn visit_str<E: de::Error>(self, name: &str) -> Result<$ty, E> {
                        $ty::from_name(name).ok_or_else(|| {
                            E::custom(format_args!(
                                concat!("parity: unregistered ", $go, " value {:?}"),
                                name
                            ))
                        })
                    }
                }

                deserializer.deserialize_str(Name)
            }
        }
    };
}

string_enum!(StageId, "parity.StageID");
string_enum!(StageStatus, "parity.StageStatus");
string_enum!(DecisionId, "parity.DecisionID");

#[cfg(test)]
#[path = "ids_test.rs"]
mod tests;
