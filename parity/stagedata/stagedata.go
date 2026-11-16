// Package stagedata defines the JSON payloads that the exchange, the floors
// package and the adapter call path record for their parity stages
// (parity/schema/stages.json, orders 13-36).
//
// These types are the cross-language contract for the "json" member of those
// stage entries: the Rust port records the same member names with the same
// meaning. Every member is always present (null or an empty collection rather
// than omitted) unless a field comment says otherwise. Where Go builds a list
// from map iteration, the projection sorts it and the field comment names the
// sort key, so two Go runs record identical bytes.
//
// Embedded OpenRTB and Prebid extension objects (openrtb2.*, openrtb_ext.*) are
// recorded exactly as Go marshals them with jsonutil.Marshal.
//
// Every stage entry carries explicit "errors" and "warnings" lists ([] when
// there are none). A type comment that names "the entry's error list" says which
// Go errors go there; stages whose Go site produces no errors record [].
//
// Keys: bidder-keyed stages use the bidder name as the exchange knows it (alias
// names included); bidder#call keys index MakeRequests output from 0 in order,
// followed by the bidder's stored bid responses in ascending imp ID order.
package stagedata

import (
	"encoding/json"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
)

// HookStage is the payload of hooks.processed_auction: whether the hook stage
// rejected the auction. The reject error itself is the entry's error list. The
// request after the stage is evidenced by the next stage that snapshots a
// rebuilt request (floors.enrich, bidder.request), never by rebuilding here.
type HookStage struct {
	Rejected bool `json:"rejected"`
}

// AuctionSetup is the payload of auction.setup (exchange.go HoldAuction, after
// the debug flags are resolved). Targeting prefix warnings are the entry's
// warning list.
type AuctionSetup struct {
	CacheInstructions CacheInstructions `json:"cache_instructions"`
	// TargetingData is null when the request has no ext.prebid.targeting.
	TargetingData *TargetingData `json:"targeting_data"`
	Debug         DebugFlags     `json:"debug"`
	// Server is ext.prebid.server as the exchange set it (null when the host
	// has no external_url, gdpr.host_vendor_id or datacenter).
	Server *openrtb_ext.ExtRequestPrebidServer `json:"server"`
	// CurrencyConversions is ext.prebid.currency as the request sent it.
	CurrencyConversions *openrtb_ext.ExtRequestCurrency `json:"currency_conversions"`
	// ConversionsSource is one of the ConversionsSource* values.
	ConversionsSource string `json:"conversions_source"`
	// ResolvedRequest is the request JSON saved for ext.debug.resolvedrequest
	// (exact bytes), or null when debug and ad server targeting are both off.
	ResolvedRequest json.RawMessage `json:"resolved_request"`
}

// Conversion rate sources selected by currency.GetAuctionCurrencyRates.
const (
	ConversionsSourceNone      = "none"
	ConversionsSourcePBS       = "pbs"
	ConversionsSourceRequest   = "request"
	ConversionsSourceAggregate = "aggregate"
)

// CacheInstructions mirrors the exchange's ext.prebid.cache reading.
type CacheInstructions struct {
	CacheBids      bool `json:"cache_bids"`
	CacheVAST      bool `json:"cache_vast"`
	ReturnCreative bool `json:"return_creative"`
}

// TargetingData mirrors the exchange's resolved targeting settings.
type TargetingData struct {
	PriceGranularity          openrtb_ext.PriceGranularity          `json:"price_granularity"`
	MediaTypePriceGranularity openrtb_ext.MediaTypePriceGranularity `json:"media_type_price_granularity"`
	IncludeWinners            bool                                  `json:"include_winners"`
	IncludeBidderKeys         bool                                  `json:"include_bidder_keys"`
	IncludeCacheBids          bool                                  `json:"include_cache_bids"`
	IncludeCacheVAST          bool                                  `json:"include_cache_vast"`
	IncludeFormat             bool                                  `json:"include_format"`
	PreferDeals               bool                                  `json:"prefer_deals"`
	AlwaysIncludeDeals        bool                                  `json:"always_include_deals"`
	CacheHost                 string                                `json:"cache_host"`
	CachePath                 string                                `json:"cache_path"`
	Prefix                    string                                `json:"prefix"`
}

// DebugFlags are the debug decisions HoldAuction derives from the request, the
// account and the debug override header.
type DebugFlags struct {
	ResponseDebugAllowed     bool `json:"response_debug_allowed"`
	AccountDebugAllowed      bool `json:"account_debug_allowed"`
	DebugOverride            bool `json:"debug_override"`
	DebugEnabledOrOverridden bool `json:"debug_enabled_or_overridden"`
}

// FloorsEnrich is the payload of floors.enrich, recorded by
// floors.EnrichWithPriceFloors right after it rebuilt the request. When
// enrichment stops early (floors disabled for the account or request) only the
// entry's error list is recorded and json is null.
type FloorsEnrich struct {
	Request *openrtb2.BidRequest         `json:"request"`
	Floors  *openrtb_ext.PriceFloorRules `json:"floors"`
}

// FPDResolved is the payload of fpd.resolved: the per-bidder first-party data
// ExtractFPDForBidders resolved (keyed by bidder name). FPD errors are the
// entry's error list.
type FPDResolved struct {
	Bidders map[string]FirstPartyData `json:"bidders"`
}

// FirstPartyData mirrors firstpartydata.ResolvedFirstPartyData.
type FirstPartyData struct {
	Site   *openrtb2.Site   `json:"site"`
	App    *openrtb2.App    `json:"app"`
	User   *openrtb2.User   `json:"user"`
	Device *openrtb2.Device `json:"device"`
}

// PrivacyLabels is the payload of privacy.labels: the request-level privacy
// labels cleanOpenRTBRequests returned. The entry's error list holds every
// error and warning cleanOpenRTBRequests returned (including per-bidder ones).
type PrivacyLabels struct {
	CCPAProvided   bool   `json:"ccpa_provided"`
	CCPAEnforced   bool   `json:"ccpa_enforced"`
	COPPAEnforced  bool   `json:"coppa_enforced"`
	GDPREnforced   bool   `json:"gdpr_enforced"`
	GDPRTCFVersion string `json:"gdpr_tcf_version"`
	LMTEnforced    bool   `json:"lmt_enforced"`
}

// PrivacyBidder is the payload of privacy.bidder (keyed by bidder): the
// privacy decisions cleanOpenRTBRequests made for one bidder. It is recorded
// when the bidder is blocked and when its request is complete; a bidder dropped
// by a request-building error is not recorded (the error is in privacy.labels).
type PrivacyBidder struct {
	Bidder string          `json:"bidder"`
	Core   string          `json:"core"`
	Alias  bool            `json:"alias"`
	GDPR   GDPRPermissions `json:"gdpr"`
	// Activities holds the activity-control results the code evaluated; an
	// activity the code did not reach (because the bidder was blocked) is null.
	Activities   Activities `json:"activities"`
	CCPAEnforced bool       `json:"ccpa_enforced"`
	LMT          bool       `json:"lmt"`
	COPPA        bool       `json:"coppa"`
	Blocked      bool       `json:"blocked"`
	// GPPDowngraded and DownConverted are null when the bidder was blocked.
	GPPDowngraded *bool `json:"gpp_downgraded"`
	DownConverted *bool `json:"down_converted"`
}

// GDPRPermissions mirrors gdpr.AuctionPermissions.
type GDPRPermissions struct {
	AllowBidRequest bool `json:"allow_bid_request"`
	PassGeo         bool `json:"pass_geo"`
	PassID          bool `json:"pass_id"`
}

// Activities are the activity-control results for one bidder.
type Activities struct {
	FetchBids          *bool `json:"fetch_bids"`
	TransmitUFPD       *bool `json:"transmit_ufpd"`
	TransmitPreciseGeo *bool `json:"transmit_precise_geo"`
	TransmitTIDs       *bool `json:"transmit_tids"`
}

// BidderRequest is the payload of bidder.request (keyed by bidder): the
// per-bidder request cleanOpenRTBRequests built, snapshotted after it was
// rebuilt and before any bidder goroutine runs.
type BidderRequest struct {
	Bidder             string                     `json:"bidder"`
	Core               string                     `json:"core"`
	IsAlias            bool                       `json:"is_alias"`
	Request            *openrtb2.BidRequest       `json:"request"`
	StoredBidResponses map[string]json.RawMessage `json:"stored_bid_responses"`
	ImpReplaceImpID    map[string]bool            `json:"imp_replace_impid"`
}

// HookRequest is the payload of hooks.bidder_request (keyed by bidder): the
// bidder request after the bidder-request hook stage and its rebuild. On a
// reject Request is null and the reject is the entry's error list.
type HookRequest struct {
	Rejected bool                 `json:"rejected"`
	Request  *openrtb2.BidRequest `json:"request"`
}

// AdapterRequests is the payload of adapter.requests (MakeRequests output, the
// entry's errors are MakeRequests' errors) and adapter.requests_final (after
// the X-Prebid, Sec-GPC and ads.cert headers; the entry's errors are every
// error collected so far). Both are keyed by bidder.
type AdapterRequests struct {
	Requests []parity.AdapterRequestData `json:"requests"`
}

// AdapterBids is the payload of adapter.bids (keyed by bidder#call): one
// MakeBids call. The entry's errors are MakeBids' errors.
type AdapterBids struct {
	RequestData    parity.AdapterRequestData     `json:"request_data"`
	Response       *parity.AdapterHTTPResponse   `json:"response"`
	BidderResponse *parity.AdapterBidderResponse `json:"bidder_response"`
	// Mocked is true when mock.level=bidder_response replaced MakeBids.
	Mocked bool `json:"mocked"`
}

// RawBidderResponse is the payload of hooks.raw_bidder_response (keyed by
// bidder#call): the bidder response after the raw-bidder-response hook stage.
type RawBidderResponse struct {
	RequestData    parity.AdapterRequestData     `json:"request_data"`
	Rejected       bool                          `json:"rejected"`
	BidderResponse *parity.AdapterBidderResponse `json:"bidder_response"`
}

// BidsAdjusted is the payload of bids.adjusted (keyed by seat): currency
// conversion and bid adjustments applied to the bids of one seat, in the order
// requestBid processed them.
type BidsAdjusted struct {
	Bids []AdjustedBid `json:"bids"`
	// ConversionErrors lists the responses whose bids were dropped because no
	// conversion rate was found (recorded under the requesting bidder's seat).
	ConversionErrors []ConversionError `json:"conversion_errors"`
}

// AdjustedBid is one bid going through currency conversion and adjustments.
type AdjustedBid struct {
	// Call is the adapter.bids key of the response the bid came from.
	Call string `json:"call"`
	// Bidder is the bidder that made the request (the seat may differ through
	// alternate bidder codes).
	Bidder      string  `json:"bidder"`
	BidID       string  `json:"bid_id"`
	ImpID       string  `json:"imp_id"`
	OriginalCPM float64 `json:"original_cpm"`
	OriginalCur string  `json:"original_cur"`
	// AltCodeValid is false when the seat is not an allowed alternate bidder
	// code; the bid is then dropped and the remaining members are null.
	AltCodeValid      bool     `json:"alt_code_valid"`
	Factor            *float64 `json:"factor"`
	ConversionRate    *float64 `json:"conversion_rate"`
	RuleAdjustedPrice *float64 `json:"rule_adjusted_price"`
	CurrencyAfter     *string  `json:"currency_after"`
}

// ConversionError is a bidder response dropped for lack of a conversion rate.
type ConversionError struct {
	Call              string             `json:"call"`
	ResponseCurrency  string             `json:"response_currency"`
	RequestCurrencies []string           `json:"request_currencies"`
	Error             parity.ErrorRecord `json:"error"`
}

// BidderResult is the payload of bidder.result (keyed by bidder): what one
// bidder goroutine returned to getAllBids. The entry's errors and warnings are
// the bidder's errors; response time is deliberately excluded.
type BidderResult struct {
	// SeatBids is sorted by seat.
	SeatBids []SeatBid `json:"seat_bids"`
	// NonBids is the bidder's seat non-bid builder, keyed by seat.
	NonBids map[string][]openrtb_ext.NonBid `json:"nonbids"`
}

// SeatBid mirrors entities.PbsOrtbSeatBid.
type SeatBid struct {
	Seat                 string                             `json:"seat"`
	Currency             string                             `json:"currency"`
	Bids                 []Bid                              `json:"bids"`
	FledgeAuctionConfigs []*openrtb_ext.FledgeAuctionConfig `json:"fledge_auction_configs"`
	HTTPCalls            []*openrtb_ext.ExtHttpCall         `json:"http_calls"`
}

// Bid mirrors entities.PbsOrtbBid.
type Bid struct {
	Bid               *openrtb2.Bid                   `json:"bid"`
	Type              string                          `json:"type"`
	Meta              *openrtb_ext.ExtBidPrebidMeta   `json:"meta"`
	Video             *openrtb_ext.ExtBidPrebidVideo  `json:"video"`
	Events            *openrtb_ext.ExtBidPrebidEvents `json:"events"`
	Floors            *openrtb_ext.ExtBidPrebidFloors `json:"floors"`
	Targets           map[string]string               `json:"targets"`
	DealPriority      int                             `json:"deal_priority"`
	DealTierSatisfied bool                            `json:"deal_tier_satisfied"`
	GeneratedBidID    string                          `json:"generated_bid_id"`
	OriginalBidCPM    float64                         `json:"original_bid_cpm"`
	OriginalBidCur    string                          `json:"original_bid_cur"`
	TargetBidderCode  string                          `json:"target_bidder_code"`
	AdapterCode       string                          `json:"adapter_code"`
}

// SeatBids is the payload of hooks.all_processed_bid_responses: every seat's
// bids after the all-processed-bid-responses hook stage, keyed by seat.
type SeatBids map[string]SeatBid

// FloorsEnforce is the payload of floors.enforce, recorded whenever host floors
// are enabled and bids were returned. Enforcement errors are the entry's error
// list.
type FloorsEnforce struct {
	// Enforced is true when the floors.enforce decision was reached and chose
	// to enforce (it is false when floors are disabled for the account or
	// request, signalling was skipped or no imp has a valid floor).
	Enforced bool `json:"enforced"`
	// Rejected is sorted by (seat, imp_id, bid_id).
	Rejected []FloorRejection `json:"rejected"`
}

// FloorRejection is one bid rejected for being below its floor.
type FloorRejection struct {
	Seat     string  `json:"seat"`
	BidID    string  `json:"bid_id"`
	ImpID    string  `json:"imp_id"`
	Price    float64 `json:"price"`
	Currency string  `json:"currency"`
	Floor    float64 `json:"floor"`
	FloorCur string  `json:"floor_cur"`
	DealID   string  `json:"deal_id"`
	// Reason is the seat non-bid status code the rejection records.
	Reason int `json:"reason"`
}

// CategoryMapping is the payload of category.mapping. A category-mapping
// failure (which fails the auction) is the entry's error list.
type CategoryMapping struct {
	// BidCategory maps bid ID to its hb_pb_cat_dur value.
	BidCategory map[string]string `json:"bid_category"`
	Rejections  []string          `json:"rejections"`
}

// EventsTracking is the payload of events.tracking: the event settings and,
// per seat, the generated bid IDs and event URLs after modifyBidsForEvents.
type EventsTracking struct {
	AccountID          string `json:"account_id"`
	EnabledForAccount  bool   `json:"enabled_for_account"`
	EnabledForRequest  bool   `json:"enabled_for_request"`
	AuctionTimestampMS int64  `json:"auction_timestamp_ms"`
	IntegrationType    string `json:"integration_type"`
	ExternalURL        string `json:"external_url"`
	// Bids is keyed by seat; each list keeps the seat's bid order.
	Bids map[string][]EventBid `json:"bids"`
}

// EventBid is the event view of one bid.
type EventBid struct {
	BidID          string                          `json:"bid_id"`
	GeneratedBidID string                          `json:"generated_bid_id"`
	Events         *openrtb_ext.ExtBidPrebidEvents `json:"events"`
}

// AuctionWinners is the payload of auction.winners: per imp the overall winner
// and every seat's ranked bids after multibid limits and price rounding.
type AuctionWinners struct {
	Imps map[string]ImpWinners `json:"imps"`
}

// ImpWinners is the auction view of one imp.
type ImpWinners struct {
	Winner *RankedBid `json:"winner"`
	// Seats is keyed by seat; each list is in the exchange's ranked order.
	Seats map[string][]RankedBid `json:"seats"`
}

// RankedBid is one bid in the auction.
type RankedBid struct {
	BidID             string  `json:"bid_id"`
	Price             float64 `json:"price"`
	DealID            string  `json:"deal_id"`
	DealPriority      int     `json:"deal_priority"`
	DealTierSatisfied bool    `json:"deal_tier_satisfied"`
	RoundedPrice      string  `json:"rounded_price"`
}

// CacheResult is the payload of cache.result (exchange/auction.go doCache): what
// was sent to Prebid Cache and what came back. Cache errors are the entry's
// error list.
type CacheResult struct {
	// Puts is sorted by (kind, bid_id, imp_id, data) because Go builds the
	// cache request from map iteration (auction.go doCache).
	Puts                 []CachePut `json:"puts"`
	CompetitiveExclusion bool       `json:"competitive_exclusion"`
	HBCacheID            string     `json:"hb_cache_id"`
}

// Cache put kinds.
const (
	CachePutBid      = "bid"
	CachePutVAST     = "vast"
	CachePutDebugLog = "debug_log"
)

// CachePut is one Cacheable handed to the cache client.
type CachePut struct {
	Kind       string          `json:"kind"`
	BidID      string          `json:"bid_id"`
	ImpID      string          `json:"imp_id"`
	Type       string          `json:"type"`
	TTLSeconds int64           `json:"ttlseconds"`
	Key        string          `json:"key"`
	Data       json.RawMessage `json:"data"`
	// ID is the UUID the cache returned for this put ("" on failure).
	ID string `json:"id"`
	// Assigned is the cache ID the auction stored for the bid ("" when none).
	Assigned string `json:"assigned"`
}

// Targeting is the payload of targeting: the targeting key/values of every bid
// after setTargeting, keyed by imp ID then seat.
type Targeting struct {
	Imps map[string]map[string][]TargetedBid `json:"imps"`
}

// TargetedBid is the targeting view of one bid.
type TargetedBid struct {
	BidID   string            `json:"bid_id"`
	Targets map[string]string `json:"targets"`
}

// SeatNonBids is the payload of seat_non_bids: the auction's seat non-bid
// builder (keyed by seat, each list in Go's append order).
type SeatNonBids map[string][]openrtb_ext.NonBid

// ExchangeResponse is the payload of exchange.response: what HoldAuction
// returned. A HoldAuction error is the entry's error list, attached once by the
// endpoint that called HoldAuction.
type ExchangeResponse struct {
	BidResponse    *openrtb2.BidResponse       `json:"bid_response"`
	ExtBidResponse *openrtb_ext.ExtBidResponse `json:"ext_bid_response"`
}
