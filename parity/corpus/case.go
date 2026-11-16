// Package corpus loads the Go test corpora that seed the parity harness:
// adapterstest JSON fixtures (with the Builder configuration each test run
// uses, extracted with go/ast), endpoints/openrtb2 sample requests,
// exchange/exchangetest specs, bidder-params schemas and the smaller
// domain-targeted seed corpora.
//
// Every loader keeps provenance: each Case carries the repo-relative
// fixture_path it came from, and its case_id is "<fixture_path>#<variant>". The
// loaders read the Go fixture formats directly (mirroring the parsers in
// adapters/adapterstest/test_json.go, endpoints/openrtb2/test_utils.go and
// exchange/exchange_test.go) instead of keeping a forked copy of the corpus, and
// every path goes through Repo.Resolve, which only admits the approved corpus
// roots.
//
// The package is harness-only: product code must not import it.
package corpus

import (
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// Kind classifies a Case by how the runner executes it.
type Kind string

// Case kinds.
const (
	// KindAdapter runs an adapter fixture against the pure adapter endpoints
	// (POST /__parity/adapter/{bidder}/requests and .../bids).
	KindAdapter Kind = "adapter"
	// KindLifted runs an adapter fixture's request as a full /openrtb2/auction
	// call, with the fixture's mock responses served by the oracle.
	KindLifted Kind = "lifted_auction"
	// KindEndpoint runs an endpoints/openrtb2 sample request (auction, amp, video).
	KindEndpoint Kind = "endpoint"
	// KindExchange runs an exchange/exchangetest spec as an auction whose bidder
	// responses are injected at mock.level=bidder_response.
	KindExchange Kind = "exchange"
	// KindBidderParams exercises the bidder-params surface (GET /bidders/params).
	KindBidderParams Kind = "bidder_params"
)

// Corpus names used in Case.Corpus.
const (
	CorpusAdapters       = "adapters"
	CorpusSampleRequests = "sample-requests"
	CorpusExchangeTest   = "exchangetest"
	CorpusBidderParams   = "bidder-params"
)

// Endpoint paths used by the cases.
const (
	EndpointAuction        = "/openrtb2/auction"
	EndpointAMP            = "/openrtb2/amp"
	EndpointVideo          = "/openrtb2/video"
	EndpointBidderParams   = "/bidders/params"
	EndpointAdapterRequest = "/__parity/adapter/{bidder}/requests"
)

// Case tags.
const (
	// TagNeedsFake marks adapter runs whose Go test injects deterministic
	// sources (or that use one of the 8 adapters with internal randomness);
	// their fixture expectations hold only with the parity fakes installed.
	TagNeedsFake = "needs_fake"
	// TagUnresolvedBuild marks adapter runs whose Builder configuration the AST
	// extractor could not resolve completely.
	TagUnresolvedBuild = "unresolved_build"
	// TagAllowErrors marks adapter fixtures from directories where errors are allowed.
	TagAllowErrors = "allow_errors"
	// TagMockParamsValidator marks sample requests whose Go test used an
	// accept-all params validator; the live servers always validate params.
	TagMockParamsValidator = "go_test_mock_params_validator"
	// TagTestHooks marks sample requests that only make sense with the mock
	// hook plans of the Go test (endpoints/openrtb2 TestAuctionHooks/TestAmpHooks).
	TagTestHooks = "requires_test_hooks"
	// TagInvalidJSON marks cases whose inbound body is intentionally malformed.
	TagInvalidJSON = "invalid_json_body"
	// TagExchangeInternal marks exchangetest specs that set exchange internals
	// with no live equivalent (see Case.Notes for which).
	TagExchangeInternal = "exchange_internal_state"
	// TagContractGap marks cases whose faithful encoding is not yet accepted by
	// the shared parity JSON schemas (see Case.Notes); runners must skip them
	// until the contract is widened.
	TagContractGap = "contract_gap"
)

// Default inbound request attributes. They match what net/http/httptest
// NewRequest sets in the Go endpoint tests, so values derived from them (for
// example device.ip) agree with the fixtures.
const (
	DefaultRemoteAddr = "192.0.2.1:1234"
	DefaultHost       = "example.com"
)

// Case is one parity case with its provenance.
type Case struct {
	ID          string   `json:"case_id"`
	Kind        Kind     `json:"kind"`
	Corpus      string   `json:"corpus"`
	FixturePath string   `json:"fixture_path"`
	Description string   `json:"description,omitempty"`
	Endpoint    string   `json:"endpoint"`
	Profile     Profile  `json:"profile"`
	Tags        []string `json:"tags,omitempty"`
	Notes       []string `json:"notes,omitempty"`

	// Envelope is the exec envelope for KindLifted, KindEndpoint, KindExchange
	// and KindBidderParams cases.
	Envelope *parity.Envelope `json:"envelope,omitempty"`
	// Adapter holds the adapter-endpoint inputs for KindAdapter cases.
	Adapter *AdapterCase `json:"adapter,omitempty"`
	// Mocks is what the oracle needs to answer this case's outbound calls.
	Mocks MockPlan `json:"mocks"`
	// Stored is the stored data the servers must serve for this case.
	Stored Stored `json:"stored"`
	// Expect holds the Go fixture's own expectations (used by the fixture
	// self-test; Go-vs-Rust parity never compares against them).
	Expect Expect `json:"expect"`
}

// HasTag reports whether the case carries tag.
func (c *Case) HasTag(tag string) bool {
	for _, t := range c.Tags {
		if t == tag {
			return true
		}
	}
	return false
}

// AdapterCase are the adapter-endpoint inputs of one adapter fixture run.
type AdapterCase struct {
	Run      AdapterRunRef               `json:"run"`
	Requests parity.AdapterRequestsInput `json:"requests"`
	// Bids holds one MakeBids input per fixture httpCall, in fixture order,
	// built exactly like adapterstest (method POST, the expected uri and body,
	// no headers) so results match the fixture expectations.
	Bids   []parity.AdapterBidsInput `json:"bids"`
	Expect AdapterExpect             `json:"expect"`
}

// AdapterRunRef identifies the Go test run a KindAdapter case reproduces.
type AdapterRunRef struct {
	TestFile string `json:"test_file"`
	TestFunc string `json:"test_func"`
	Root     string `json:"root"`
}

// AdapterExpect are the adapterstest expectations of one fixture.
type AdapterExpect struct {
	AllowErrors        bool                 `json:"allow_errors"`
	HTTPCalls          []AdapterHTTPCall    `json:"http_calls"`
	BidResponses       []AdapterBidResponse `json:"bid_responses"`
	MakeRequestsErrors []ExpectedError      `json:"make_requests_errors"`
	MakeBidsErrors     []ExpectedError      `json:"make_bids_errors"`
}

// MockPlan is what the oracle needs to answer a case's outbound calls. It is
// expressed in corpus terms; parity/oracle turns it into an oracle plan.
type MockPlan struct {
	// ImpIDs are the imp IDs of the inbound request, in request order. The
	// oracle uses them to key template responses when an outgoing bidder
	// request does not carry OpenRTB imp IDs.
	ImpIDs []string `json:"imp_ids,omitempty"`
	// HTTP are explicit bidder responses (for example an adapter fixture's
	// mockResponses), matched by bidder and imp-ID set.
	HTTP []HTTPMock `json:"http,omitempty"`
	// Bidders are template parameters per bidder (sample-request mockBidders).
	Bidders map[string]MockBidder `json:"bidders,omitempty"`
}

// HTTPMock is one explicit outbound response.
type HTTPMock struct {
	ID       string                `json:"id"`
	Bidder   string                `json:"bidder,omitempty"`
	ImpIDs   []string              `json:"imp_ids,omitempty"`
	URIRegex string                `json:"uri_regex,omitempty"`
	Response parity.InlineResponse `json:"response"`
}

// MockBidder are the template generator parameters of one bidder (the
// endpoints/openrtb2 mockBidderHandler fields). Zero values keep the template's.
type MockBidder struct {
	Currency string   `json:"currency,omitempty"`
	Price    *float64 `json:"price,omitempty"`
	DealID   string   `json:"dealid,omitempty"`
	Seat     string   `json:"seat,omitempty"`
}

// Stored is stored data a case depends on, keyed by ID. The runner writes it
// to the servers' filesystem stored-data directories.
type Stored struct {
	Requests  map[string]json.RawMessage `json:"requests,omitempty"`
	Imps      map[string]json.RawMessage `json:"imps,omitempty"`
	Responses map[string]json.RawMessage `json:"responses,omitempty"`
	Accounts  map[string]json.RawMessage `json:"accounts,omitempty"`
}

// IsEmpty reports whether s holds no stored data.
func (s Stored) IsEmpty() bool {
	return len(s.Requests) == 0 && len(s.Imps) == 0 && len(s.Responses) == 0 && len(s.Accounts) == 0
}

// MergeStored merges the stored data of many cases. Two cases that use the
// same ID with different content cannot share one server process; they are
// reported as conflicts (and the first value is kept).
func MergeStored(cases []Case) (Stored, []error) {
	var out Stored
	var errs []error
	for i := range cases {
		c := &cases[i]
		errs = append(errs, mergeInto(&out.Requests, c.Stored.Requests, "request", c.ID)...)
		errs = append(errs, mergeInto(&out.Imps, c.Stored.Imps, "imp", c.ID)...)
		errs = append(errs, mergeInto(&out.Responses, c.Stored.Responses, "response", c.ID)...)
		errs = append(errs, mergeInto(&out.Accounts, c.Stored.Accounts, "account", c.ID)...)
	}
	return out, errs
}

func mergeInto(dst *map[string]json.RawMessage, src map[string]json.RawMessage, kind, caseID string) []error {
	var errs []error
	for _, id := range sortedKeys(src) {
		if *dst == nil {
			*dst = make(map[string]json.RawMessage)
		}
		if have, ok := (*dst)[id]; ok {
			if string(have) != string(src[id]) {
				errs = append(errs, fmt.Errorf("corpus: case %s: stored %s %q conflicts with another case", caseID, kind, id))
			}
			continue
		}
		(*dst)[id] = src[id]
	}
	return errs
}

// Expect holds a Go fixture's expectations for one exec case.
type Expect struct {
	Status            int                        `json:"status,omitempty"`
	ErrorContains     string                     `json:"error_contains,omitempty"`
	BidResponse       json.RawMessage            `json:"bid_response,omitempty"`
	AmpResponse       json.RawMessage            `json:"amp_response,omitempty"`
	ValidatedRequest  json.RawMessage            `json:"validated_request,omitempty"`
	BidderRequests    map[string]json.RawMessage `json:"bidder_requests,omitempty"`
	ExchangeError     string                     `json:"exchange_error,omitempty"`
	ResponseExt       json.RawMessage            `json:"response_ext,omitempty"`
	AdditionalExpects map[string]json.RawMessage `json:"additional,omitempty"`
}

// newEnvelope returns an envelope with the contract version and defaults set.
func newEnvelope(caseID, fixturePath, profile string, req parity.ExecRequest) *parity.Envelope {
	if req.Headers == nil {
		req.Headers = parity.HeaderList{}
	}
	return &parity.Envelope{
		Schema:        parity.SchemaEnvelope,
		SchemaVersion: parity.SchemaVersion,
		CaseID:        caseID,
		FixturePath:   fixturePath,
		Profile:       profile,
		Request:       req,
		Mock:          parity.MockSpec{Mode: parity.MockModeOracle, Level: parity.MockLevelHTTP},
		Capture:       parity.CaptureSpec{Stages: json.RawMessage(`"*"`)},
	}
}

// jsonPostRequest is a POST of a JSON body with the Go-test inbound defaults.
func jsonPostRequest(path string, body []byte) parity.ExecRequest {
	return parity.ExecRequest{
		Method:     "POST",
		Path:       path,
		Headers:    parity.HeaderList{{"Content-Type", "application/json"}},
		Host:       DefaultHost,
		RemoteAddr: DefaultRemoteAddr,
		Body:       parity.TextBody(string(body)),
	}
}

// getRequest is a GET with the Go-test inbound defaults.
func getRequest(path, query string) parity.ExecRequest {
	return parity.ExecRequest{
		Method:     "GET",
		Path:       path,
		Query:      query,
		Headers:    parity.HeaderList{},
		Host:       DefaultHost,
		RemoteAddr: DefaultRemoteAddr,
	}
}

// caseID builds "<fixture_path>#<variant>".
func caseID(fixturePath, variant string) string {
	return fixturePath + "#" + variant
}

// shortHash returns the first n hex characters of sha1(parts joined by NUL).
func shortHash(n int, parts ...string) string {
	sum := sha1.Sum([]byte(strings.Join(parts, "\x00")))
	return hex.EncodeToString(sum[:])[:n]
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func addTag(tags []string, tag string) []string {
	for _, t := range tags {
		if t == tag {
			return tags
		}
	}
	return append(tags, tag)
}
