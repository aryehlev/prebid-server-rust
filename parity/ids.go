package parity

// StageID names one capture point. Stage IDs are the cross-language contract:
// both servers record the same IDs and parity/schema/stages.json gives each one
// its order, owning domain and requirement IDs.
type StageID string

// Stage IDs in stages.json order. The order defines "first diverging stage".
const (
	StageConfigResolved                StageID = "config.resolved"
	StageBiddersResolved               StageID = "bidders.resolved"
	StageHTTPIn                        StageID = "http.in"
	StageHooksEntrypoint               StageID = "hooks.entrypoint"
	StageStoredFetch                   StageID = "stored.fetch"
	StageAccountResolved               StageID = "account.resolved"
	StageHooksRawAuction               StageID = "hooks.raw_auction"
	StageRequestMerged                 StageID = "request.merged"
	StageRequestNormalized             StageID = "request.normalized"
	StageStoredResponses               StageID = "stored.responses"
	StageRequestValidated              StageID = "request.validated"
	StageGDPRContext                   StageID = "gdpr.context"
	StageHooksProcessedAuction         StageID = "hooks.processed_auction"
	StageAuctionSetup                  StageID = "auction.setup"
	StageFloorsEnrich                  StageID = "floors.enrich"
	StageFPDResolved                   StageID = "fpd.resolved"
	StagePrivacyLabels                 StageID = "privacy.labels"
	StagePrivacyBidder                 StageID = "privacy.bidder"
	StageBidderRequest                 StageID = "bidder.request"
	StageHooksBidderRequest            StageID = "hooks.bidder_request"
	StageAdapterRequests               StageID = "adapter.requests"
	StageAdapterRequestsFinal          StageID = "adapter.requests_final"
	StageHTTPOut                       StageID = "http.out"
	StageAdapterBids                   StageID = "adapter.bids"
	StageHooksRawBidderResponse        StageID = "hooks.raw_bidder_response"
	StageBidsAdjusted                  StageID = "bids.adjusted"
	StageBidderResult                  StageID = "bidder.result"
	StageFloorsEnforce                 StageID = "floors.enforce"
	StageCategoryMapping               StageID = "category.mapping"
	StageEventsTracking                StageID = "events.tracking"
	StageHooksAllProcessedBidResponses StageID = "hooks.all_processed_bid_responses"
	StageAuctionWinners                StageID = "auction.winners"
	StageCacheResult                   StageID = "cache.result"
	StageTargeting                     StageID = "targeting"
	StageSeatNonBids                   StageID = "seat_non_bids"
	StageExchangeResponse              StageID = "exchange.response"
	StageVideoPods                     StageID = "video.pods"
	StageVideoCache                    StageID = "video.cache"
	StageAMPTargetingResponse          StageID = "amp.targeting_response"
	StageHooksAuctionResponse          StageID = "hooks.auction_response"
	StageHooksExitpoint                StageID = "hooks.exitpoint"
	StageHookOutcomes                  StageID = "hook_outcomes"
	StageAnalytics                     StageID = "analytics"
	StageMetricsDelta                  StageID = "metrics_delta"
	StageHTTPResponse                  StageID = "http.response"
)

// AllStages returns every stage ID in stages.json order.
func AllStages() []StageID {
	return []StageID{
		StageConfigResolved, StageBiddersResolved, StageHTTPIn,
		StageHooksEntrypoint, StageStoredFetch, StageAccountResolved,
		StageHooksRawAuction, StageRequestMerged, StageRequestNormalized,
		StageStoredResponses, StageRequestValidated, StageGDPRContext,
		StageHooksProcessedAuction, StageAuctionSetup, StageFloorsEnrich,
		StageFPDResolved, StagePrivacyLabels, StagePrivacyBidder,
		StageBidderRequest, StageHooksBidderRequest, StageAdapterRequests,
		StageAdapterRequestsFinal, StageHTTPOut, StageAdapterBids,
		StageHooksRawBidderResponse, StageBidsAdjusted, StageBidderResult,
		StageFloorsEnforce, StageCategoryMapping, StageEventsTracking,
		StageHooksAllProcessedBidResponses, StageAuctionWinners,
		StageCacheResult, StageTargeting, StageSeatNonBids,
		StageExchangeResponse, StageVideoPods, StageVideoCache,
		StageAMPTargetingResponse, StageHooksAuctionResponse,
		StageHooksExitpoint, StageHookOutcomes, StageAnalytics,
		StageMetricsDelta, StageHTTPResponse,
	}
}

// StageStatus says whether a stage was observed during one case.
type StageStatus string

// Stage statuses reported in trace.stage_status.
const (
	// StageCaptured means the implementation recorded at least one key.
	StageCaptured StageStatus = "captured"
	// StageNotReached means the implementation supports the stage but the case
	// did not reach it.
	StageNotReached StageStatus = "not_reached"
	// StageUnimplemented means the implementation has no equivalent capture
	// point yet (Rust only; Go is the source of truth and never reports it).
	StageUnimplemented StageStatus = "unimplemented"
)

// UnkeyedStage is the key used by stages that are not keyed by bidder, seat or call.
const UnkeyedStage = "_"

// DecisionID names one point where Go consults a random source. The envelope
// can pin each one; both servers must read the same IDs (decisions.json).
type DecisionID string

// Decision IDs in decisions.json order.
const (
	DecisionFloorsSkip                DecisionID = "floors.skip"
	DecisionFloorsDataRate            DecisionID = "floors.data_rate"
	DecisionFloorsModelGroup          DecisionID = "floors.modelgroup"
	DecisionFloorsEnforce             DecisionID = "floors.enforce"
	DecisionCategoryDedupe            DecisionID = "category.dedupe"
	DecisionPriceGranularityTimesplit DecisionID = "pricegranularity.timesplit"
	DecisionUsersyncShuffle           DecisionID = "usersync.shuffle"
)

// AllDecisions returns every decision ID in decisions.json order.
func AllDecisions() []DecisionID {
	return []DecisionID{
		DecisionFloorsSkip,
		DecisionFloorsDataRate,
		DecisionFloorsModelGroup,
		DecisionFloorsEnforce,
		DecisionCategoryDedupe,
		DecisionPriceGranularityTimesplit,
		DecisionUsersyncShuffle,
	}
}

// Decision sources reported in trace.decisions[].source.
const (
	DecisionSourceEnvelope = "envelope"
	DecisionSourceRNG      = "rng"
)

// UsersyncShuffleIdentity is the only non-random value of usersync.shuffle.
const UsersyncShuffleIdentity = "identity"

// Outbound call kinds. A call made with a bidder in its context is always
// KindBidder; otherwise the kind is the label of the client it was sent with,
// refined for the shared general client by URL (see transport docs).
const (
	OutboundBidder     = "bidder"
	OutboundCache      = "cache"
	OutboundFloors     = "floors"
	OutboundCurrency   = "currency"
	OutboundVendorList = "vendorlist"
	OutboundStored     = "stored"
	OutboundModule     = "module"
	OutboundGeneral    = "general"
	OutboundDefault    = "default"
)

// AllOutboundKinds returns every outbound kind in schema order.
func AllOutboundKinds() []string {
	return []string{
		OutboundBidder, OutboundCache, OutboundFloors, OutboundCurrency,
		OutboundVendorList, OutboundStored, OutboundModule, OutboundGeneral,
		OutboundDefault,
	}
}

// Mock modes and levels (envelope.mock.mode / envelope.mock.level).
const (
	MockModeOracle         = "oracle"
	MockModeInline         = "inline"
	MockLevelHTTP          = "http"
	MockLevelBidderResp    = "bidder_response"
	MockSourceInline       = "inline"
	MockSourceRule         = "rule"
	MockSourceFixture      = "fixture_replay"
	MockSourceTemplate     = "template"
	MockSourceDefault      = "default"
	MockSourceStatic       = "static"
	OutboundErrTimeout     = "timeout"
	OutboundErrRefused     = "refused"
	OutboundErrReset       = "reset"
	OutboundErrNetDisabled = "network_disabled"
	OutboundErrOther       = "error"
)
