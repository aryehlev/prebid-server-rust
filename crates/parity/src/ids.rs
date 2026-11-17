//! Stage, decision and outbound-call identifiers (Go: `parity/ids.go`), with the registry
//! metadata of `stages.json` and `decisions.json`.
//!
//! The IDs are the cross-language contract: both servers record the same stage IDs and read the
//! same decision IDs. The tests check every table here against the registry JSON vendored under
//! `contract/`.

use std::{borrow::Cow, str};

use pbs_gocompat::strconv::quote_bytes;
use pbs_gojson::{
    Encoder, GoEncode, GoKind, GoMapKey, GoType, JsonError, JsoniterDecode, StdDecode,
    jsoniter::Iter,
    stdjson::{Class, StdDecoder, Store, Token, literal_store},
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

/// Go JSON for the closed string enums that documents carry where Go has a named string type.
///
/// Encoding writes the ID string. Go's string types accept any value; these enums cannot hold
/// an unregistered one, so decoding it records an error naming the value. Only the Go runner
/// reads the documents that carry them, so Rust never takes that path in practice.
macro_rules! go_string_enum {
    ($ty:ident, $go:literal, $fallback:ident) => {
        impl GoType for $ty {
            const KIND: GoKind = GoKind::String;

            fn go_type_name(out: &mut String) {
                out.push_str($go);
            }
        }

        impl GoEncode for $ty {
            fn go_encode(&self, enc: &mut Encoder) {
                enc.write_str(self.as_str().as_bytes());
            }

            fn go_is_empty(&self) -> bool {
                false
            }
        }

        impl GoMapKey for $ty {
            fn key_text(&self) -> Cow<'_, [u8]> {
                Cow::Borrowed(self.as_str().as_bytes())
            }

            fn std_parse_key(key: &[u8]) -> Option<Self> {
                str::from_utf8(key).ok().and_then(Self::from_name)
            }

            fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
                let text = it.read_string();
                str::from_utf8(&text)
                    .ok()
                    .and_then(Self::from_name)
                    .unwrap_or_else(|| {
                        if !it.has_any_error() {
                            it.set_error(unregistered($go, &text));
                        }
                        Self::$fallback
                    })
            }
        }

        impl JsoniterDecode for $ty {
            fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
                if it.read_nil() {
                    return;
                }
                let mut text = String::new();
                text.jsoniter_decode(it);
                if it.has_any_error() {
                    return;
                }
                match Self::from_name(&text) {
                    Some(value) => *self = value,
                    None => it.set_error(unregistered($go, text.as_bytes())),
                }
            }
        }

        impl StdDecode for $ty {
            fn std_decode_as<N: GoType + ?Sized>(
                &mut self,
                d: &mut StdDecoder<'_>,
            ) -> Result<(), JsonError> {
                match d.peek() {
                    Token::Literal => {
                        let item = d.read_literal();
                        self.std_literal_as::<N>(item, false, d)
                    }
                    Token::Array | Token::Object => String::new().std_decode_as::<N>(d),
                }
            }

            fn std_literal_as<N: GoType + ?Sized>(
                &mut self,
                item: &[u8],
                from_quoted: bool,
                d: &mut StdDecoder<'_>,
            ) -> Result<(), JsonError> {
                if let Store::Str(text) = literal_store::<N>(item, from_quoted, d, Class::String)? {
                    match str::from_utf8(&text).ok().and_then(Self::from_name) {
                        Some(value) => *self = value,
                        None => d.save_error(JsonError::Message(unregistered($go, &text))),
                    }
                }
                Ok(())
            }
        }
    };
}

/// The error text for an ID the registry does not have.
fn unregistered(go_type: &str, text: &[u8]) -> Vec<u8> {
    let mut msg = b"parity: unregistered ".to_vec();
    msg.extend_from_slice(go_type.as_bytes());
    msg.extend_from_slice(b" value ");
    msg.extend_from_slice(quote_bytes(text).as_bytes());
    msg
}

go_string_enum!(StageId, "parity.StageID", ConfigResolved);
go_string_enum!(StageStatus, "parity.StageStatus", NotReached);
go_string_enum!(DecisionId, "parity.DecisionID", FloorsSkip);

#[cfg(test)]
#[path = "ids_test.rs"]
mod tests;
