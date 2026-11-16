package parity

import (
	"encoding/json"
	"fmt"
	"strings"
)

// Envelope is one parity case (envelope.schema.json). The runner posts the same
// envelope to both servers' POST /__parity/exec.
type Envelope struct {
	Schema        string                     `json:"schema"`
	SchemaVersion string                     `json:"schema_version"`
	CaseID        string                     `json:"case_id"`
	FixturePath   string                     `json:"fixture_path,omitempty"`
	Profile       string                     `json:"profile,omitempty"`
	Request       ExecRequest                `json:"request"`
	Mock          MockSpec                   `json:"mock"`
	Decisions     map[string]json.RawMessage `json:"decisions,omitempty"`
	Fakes         *FakeValues                `json:"fakes,omitempty"`
	Capture       CaptureSpec                `json:"capture"`
}

// ExecRequest is the inbound HTTP request the server must dispatch through its
// full outer handler chain.
type ExecRequest struct {
	Method     string     `json:"method"`
	Path       string     `json:"path"`
	Query      string     `json:"query"`
	Host       string     `json:"host,omitempty"`
	Headers    HeaderList `json:"headers"`
	RemoteAddr string     `json:"remote_addr,omitempty"`
	Body       *Body      `json:"body,omitempty"`
}

// HeaderList is an ordered list of [name, value] pairs. Names keep their
// original case and repeated names keep their relative order, so multi-value
// headers survive the round trip.
type HeaderList [][2]string

// Values returns every value of name (case-insensitive) in list order.
func (h HeaderList) Values(name string) []string {
	var out []string
	for _, kv := range h {
		if strings.EqualFold(kv[0], name) {
			out = append(out, kv[1])
		}
	}
	return out
}

// MockSpec selects how outbound calls are answered during the case.
type MockSpec struct {
	Mode            string                        `json:"mode,omitempty"`
	Level           string                        `json:"level,omitempty"`
	Inline          *InlineMocks                  `json:"inline,omitempty"`
	BidderResponses map[string]BidderResponseMock `json:"bidder_responses,omitempty"`
}

// EffectiveMode returns the mock mode, defaulting to oracle.
func (m MockSpec) EffectiveMode() string {
	if m.Mode == "" {
		return MockModeOracle
	}
	return m.Mode
}

// EffectiveLevel returns the mock level, defaulting to http.
func (m MockSpec) EffectiveLevel() string {
	if m.Level == "" {
		return MockLevelHTTP
	}
	return m.Level
}

// InlineMocks answers outbound calls in-process (mode=inline). Resolution order:
// Rules (first match), then Bidders[bidder] / DefaultBidder for bidder calls,
// Cache for cache calls, Kinds[kind] for any other kind. Unresolved calls get
// the built-in defaults: bidder 204, cache 503, everything else a transport
// error "parity: network disabled".
type InlineMocks struct {
	Rules         []InlineRule              `json:"rules,omitempty"`
	Bidders       map[string]InlineResponse `json:"bidders,omitempty"`
	DefaultBidder *InlineResponse           `json:"default_bidder,omitempty"`
	Cache         *InlineResponse           `json:"cache,omitempty"`
	Kinds         map[string]InlineResponse `json:"kinds,omitempty"`
}

// InlineRule matches an outbound call; every non-empty matcher must match.
type InlineRule struct {
	ID       string         `json:"id,omitempty"`
	Kind     string         `json:"kind,omitempty"`
	Bidder   string         `json:"bidder,omitempty"`
	URIRegex string         `json:"uri_regex,omitempty"`
	Response InlineResponse `json:"response"`
}

// InlineResponse is a canned outbound response or failure.
type InlineResponse struct {
	Status  int        `json:"status,omitempty"`
	Headers HeaderList `json:"headers,omitempty"`
	Body    *Body      `json:"body,omitempty"`
	DelayMS int        `json:"delay_ms,omitempty"`
	// Fail, when set, makes the call fail instead of answering: one of
	// timeout, refused, reset or error.
	Fail *string `json:"fail,omitempty"`
}

// BidderResponseMock replaces one bidder's MakeBids output (mock.level =
// bidder_response). MakeRequests still runs and is recorded, no HTTP request is
// sent, and everything after MakeBids (currency, adjustments, alternate bidder
// codes, validation) still runs on the mocked output.
type BidderResponseMock struct {
	// NilResponse makes MakeBids return a nil *BidderResponse (with Errors).
	NilResponse          bool            `json:"nil_response,omitempty"`
	Currency             string          `json:"currency"`
	Bids                 []TypedBid      `json:"bids"`
	FledgeAuctionConfigs json.RawMessage `json:"fledge_auction_configs,omitempty"`
	Errors               []ErrorSpec     `json:"errors,omitempty"`
}

// TypedBid mirrors adapters.TypedBid. It is used both for injected MakeBids
// output and for the bidder_response reported by the adapter bids endpoint.
type TypedBid struct {
	Bid          json.RawMessage `json:"bid"`
	Type         string          `json:"type"`
	Seat         string          `json:"seat,omitempty"`
	Video        json.RawMessage `json:"video,omitempty"`
	Meta         json.RawMessage `json:"meta,omitempty"`
	DealPriority int             `json:"deal_priority,omitempty"`
}

// ErrorSpec describes an error to inject. Code uses the Go errortypes numeric
// codes (error_codes.json).
type ErrorSpec struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// CaptureSpec limits what the server records. Stages is "*" or a list of stage IDs.
type CaptureSpec struct {
	Stages    json.RawMessage `json:"stages,omitempty"`
	Metrics   *bool           `json:"metrics,omitempty"`
	Analytics *bool           `json:"analytics,omitempty"`
}

// StageFilter returns the set of stages to capture, or nil for "all stages".
func (c CaptureSpec) StageFilter() (map[StageID]bool, error) {
	if len(c.Stages) == 0 || string(c.Stages) == "null" {
		return nil, nil
	}
	var all string
	if err := json.Unmarshal(c.Stages, &all); err == nil {
		if all == "*" {
			return nil, nil
		}
		return nil, fmt.Errorf("capture.stages: string value must be \"*\", got %q", all)
	}
	var list []StageID
	if err := json.Unmarshal(c.Stages, &list); err != nil {
		return nil, fmt.Errorf("capture.stages: %w", err)
	}
	set := make(map[StageID]bool, len(list))
	for _, id := range list {
		set[id] = true
	}
	return set, nil
}

// WantsMetrics reports whether metrics_delta should be captured (default true).
func (c CaptureSpec) WantsMetrics() bool { return c.Metrics == nil || *c.Metrics }

// WantsAnalytics reports whether analytics should be captured (default true).
func (c CaptureSpec) WantsAnalytics() bool { return c.Analytics == nil || *c.Analytics }

// FakeValues are the deterministic replacements for wall-clock and random values
// used by the adapters with internal nondeterminism (fakes.json). Zero fields
// fall back to the fakes.json defaults.
type FakeValues struct {
	NowUnixMS int64  `json:"now_unix_ms,omitempty"`
	Int63     int64  `json:"int63,omitempty"`
	UUID      string `json:"uuid,omitempty"`
}

// DecisionInt returns the integer override for point, if the envelope has one.
func (e *Envelope) DecisionInt(point DecisionID) (int64, bool) {
	if e == nil {
		return 0, false
	}
	raw, ok := e.Decisions[string(point)]
	if !ok {
		return 0, false
	}
	var n int64
	if err := json.Unmarshal(raw, &n); err != nil {
		return 0, false
	}
	return n, true
}

// DecisionString returns the string override for point, if the envelope has one.
func (e *Envelope) DecisionString(point DecisionID) (string, bool) {
	if e == nil {
		return "", false
	}
	raw, ok := e.Decisions[string(point)]
	if !ok {
		return "", false
	}
	var s string
	if err := json.Unmarshal(raw, &s); err != nil {
		return "", false
	}
	return s, true
}
