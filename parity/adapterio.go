package parity

import "encoding/json"

// Adapter wrap modes for the adapter endpoints.
const (
	// WrapRaw calls the adapter exactly as its Builder returns it (what the
	// adapterstest JSON fixtures exercise).
	WrapRaw = "raw"
	// WrapInfoAware wraps the adapter with adapters.BuildInfoAwareBidder, which
	// is what the exchange uses in production.
	WrapInfoAware = "info_aware"
)

// AdapterBuild selects and configures the adapter under test. Omitted
// AdapterConfig/Server fall back to what the server derives from its own
// bidder info and host config.
type AdapterBuild struct {
	Bidder        string         `json:"bidder"`
	Wrap          string         `json:"wrap"`
	AdapterConfig *AdapterConfig `json:"adapter_config,omitempty"`
	Server        *ServerConfig  `json:"server,omitempty"`
}

// AdapterConfig mirrors config.Adapter (the Builder's second argument).
type AdapterConfig struct {
	Endpoint   string      `json:"endpoint"`
	ExtraInfo  string      `json:"extra_info,omitempty"`
	PlatformID string      `json:"platform_id,omitempty"`
	AppSecret  string      `json:"app_secret,omitempty"`
	XAPI       *XAPIConfig `json:"xapi,omitempty"`
}

// XAPIConfig mirrors config.AdapterXAPI.
type XAPIConfig struct {
	Username string `json:"username,omitempty"`
	Password string `json:"password,omitempty"`
	Tracker  string `json:"tracker,omitempty"`
}

// ServerConfig mirrors config.Server (the Builder's third argument).
type ServerConfig struct {
	ExternalURL string `json:"external_url"`
	GvlID       int    `json:"gvl_id"`
	DataCenter  string `json:"datacenter"`
}

// ExtraRequestInfo mirrors adapters.ExtraRequestInfo.
type ExtraRequestInfo struct {
	PbsEntryPoint              string         `json:"pbs_entry_point,omitempty"`
	GlobalPrivacyControlHeader string         `json:"global_privacy_control_header,omitempty"`
	CurrencyRates              *CurrencyRates `json:"currency_rates,omitempty"`
	PreferredMediaType         string         `json:"preferred_media_type,omitempty"`
}

// CurrencyRates are the conversions handed to the adapter (from -> to -> rate).
type CurrencyRates struct {
	Conversions map[string]map[string]float64 `json:"conversions"`
}

// AdapterRequestsInput is the body of POST /__parity/adapter/{bidder}/requests.
type AdapterRequestsInput struct {
	Schema        string           `json:"schema"`
	SchemaVersion string           `json:"schema_version"`
	CaseID        string           `json:"case_id"`
	FixturePath   string           `json:"fixture_path,omitempty"`
	Build         AdapterBuild     `json:"build"`
	Request       json.RawMessage  `json:"request"`
	ExtraInfo     ExtraRequestInfo `json:"extra_info"`
	Fakes         *FakeValues      `json:"fakes,omitempty"`
}

// AdapterRequestData mirrors adapters.RequestData.
type AdapterRequestData struct {
	Method  string     `json:"method"`
	URI     string     `json:"uri"`
	Headers HeaderList `json:"headers"`
	Body    *Body      `json:"body"`
	ImpIDs  []string   `json:"imp_ids"`
}

// AdapterRequestsOutput is the response of the requests endpoint. Requests and
// Errors are always present (empty arrays, never omitted).
type AdapterRequestsOutput struct {
	Schema        string               `json:"schema"`
	SchemaVersion string               `json:"schema_version"`
	Impl          string               `json:"impl"`
	CaseID        string               `json:"case_id"`
	Bidder        string               `json:"bidder"`
	Requests      []AdapterRequestData `json:"requests"`
	Errors        []ErrorRecord        `json:"errors"`
}

// AdapterHTTPResponse mirrors adapters.ResponseData.
type AdapterHTTPResponse struct {
	Status  int        `json:"status"`
	Headers HeaderList `json:"headers"`
	Body    *Body      `json:"body"`
}

// AdapterBidsInput is the body of POST /__parity/adapter/{bidder}/bids.
type AdapterBidsInput struct {
	Schema        string              `json:"schema"`
	SchemaVersion string              `json:"schema_version"`
	CaseID        string              `json:"case_id"`
	FixturePath   string              `json:"fixture_path,omitempty"`
	Build         AdapterBuild        `json:"build"`
	Request       json.RawMessage     `json:"request"`
	RequestData   AdapterRequestData  `json:"request_data"`
	Response      AdapterHTTPResponse `json:"response"`
	ExtraInfo     ExtraRequestInfo    `json:"extra_info"`
	Fakes         *FakeValues         `json:"fakes,omitempty"`
}

// AdapterBidderResponse mirrors adapters.BidderResponse.
type AdapterBidderResponse struct {
	Currency             string          `json:"currency"`
	Bids                 []TypedBid      `json:"bids"`
	FledgeAuctionConfigs json.RawMessage `json:"fledge_auction_configs,omitempty"`
}

// AdapterBidsOutput is the response of the bids endpoint. BidderResponse and
// Errors are always both present: Go's MakeBids returns bids and errors together.
type AdapterBidsOutput struct {
	Schema         string                 `json:"schema"`
	SchemaVersion  string                 `json:"schema_version"`
	Impl           string                 `json:"impl"`
	CaseID         string                 `json:"case_id"`
	Bidder         string                 `json:"bidder"`
	BidderResponse *AdapterBidderResponse `json:"bidder_response"`
	Errors         []ErrorRecord          `json:"errors"`
}

// Health is the response of GET /__parity/health.
type Health struct {
	Schema             string       `json:"schema"`
	SchemaVersion      string       `json:"schema_version"`
	Impl               string       `json:"impl"`
	Build              BuildInfo    `json:"build"`
	StagesSupported    []StageID    `json:"stages_supported"`
	DecisionsSupported []DecisionID `json:"decisions_supported"`
	Endpoints          []string     `json:"endpoints"`
	MockModes          []string     `json:"mock_modes"`
	MockLevels         []string     `json:"mock_levels"`
	Fakes              bool         `json:"fakes"`
}

// ConfigDump is the response of GET /__parity/config. Config is keyed by the Go
// mapstructure config keys, without the adapters and parity subtrees.
type ConfigDump struct {
	Schema        string          `json:"schema"`
	SchemaVersion string          `json:"schema_version"`
	Impl          string          `json:"impl"`
	Config        json.RawMessage `json:"config"`
}

// BiddersDump is the response of GET /__parity/bidders.
type BiddersDump struct {
	Schema        string            `json:"schema"`
	SchemaVersion string            `json:"schema_version"`
	Impl          string            `json:"impl"`
	Bidders       json.RawMessage   `json:"bidders"`
	Active        []string          `json:"active"`
	Disabled      map[string]string `json:"disabled"`
}

// ResetResult is the response of POST /__parity/reset.
type ResetResult struct {
	Schema        string        `json:"schema"`
	SchemaVersion string        `json:"schema_version"`
	Impl          string        `json:"impl"`
	Reset         []string      `json:"reset"`
	NotReset      []NotResetRow `json:"not_reset"`
}

// NotResetRow names a cache the server could not reset and why.
type NotResetRow struct {
	Name   string `json:"name"`
	Reason string `json:"reason"`
}
