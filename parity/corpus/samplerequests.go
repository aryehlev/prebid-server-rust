package corpus

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"path"
	"strconv"
	"strings"

	"github.com/buger/jsonparser"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// ampTagParam is the AMP query parameter that names the stored request.
const ampTagParam = "tag_id"

// SampleRequest is one endpoints/openrtb2/sample-requests JSON file, parsed the
// way endpoints/openrtb2/test_utils.go parseTestData reads it (with
// jsonparser, so intentionally malformed mockBidRequest bodies survive byte
// for byte).
type SampleRequest struct {
	Path string `json:"path"`
	// Suite is the directory below sample-requests ("valid-whole/exemplary").
	Suite       string        `json:"suite"`
	Endpoint    string        `json:"endpoint"`
	Description string        `json:"description,omitempty"`
	Config      *SampleConfig `json:"config,omitempty"`
	// BidRequest is mockBidRequest exactly as jsonparser extracts it.
	BidRequest []byte `json:"-"`
	// RequestPayload is the video endpoint body (and the body of the few
	// auction files that use requestPayload).
	RequestPayload []byte `json:"-"`
	Query          string `json:"query,omitempty"`
	// ExpectedReturnCode is 0 when the file has none (video samples).
	ExpectedReturnCode   int                        `json:"expected_return_code,omitempty"`
	ExpectedErrorMessage string                     `json:"expected_error_message,omitempty"`
	ExpectedBidResponse  json.RawMessage            `json:"expected_bid_response,omitempty"`
	ExpectedAmpResponse  json.RawMessage            `json:"expected_amp_response,omitempty"`
	ExpectedValidatedReq json.RawMessage            `json:"expected_validated_bid_request,omitempty"`
	ExpectedBidderReqs   map[string]json.RawMessage `json:"expected_mock_bidder_requests,omitempty"`
	AmpStoredResponses   map[string]json.RawMessage `json:"amp_stored_responses,omitempty"`
	Additional           map[string]json.RawMessage `json:"additional,omitempty"`
}

// SampleConfig mirrors endpoints/openrtb2 testConfigValues.
type SampleConfig struct {
	AccountRequired     bool                          `json:"accountRequired"`
	AliasJSON           string                        `json:"aliases"`
	BlockedApps         []string                      `json:"blockedApps"`
	DisabledAdapters    []string                      `json:"disabledAdapters"`
	CurrencyRates       map[string]map[string]float64 `json:"currencyRates"`
	MockBidders         []SampleMockBidder            `json:"mockBidders"`
	RealParamsValidator bool                          `json:"realParamsValidator"`
	BidderInfoOverrides map[string]json.RawMessage    `json:"bidderInfoOverrides"`
	PreferredMediaType  json.RawMessage               `json:"preferredmediatype"`
}

// SampleMockBidder mirrors endpoints/openrtb2 mockBidderHandler.
type SampleMockBidder struct {
	BidderName string  `json:"bidderName"`
	Currency   string  `json:"currency"`
	Price      float64 `json:"price"`
	DealID     string  `json:"dealid"`
	Seat       string  `json:"seat"`
}

// additionalSampleKeys are optional expectation members kept verbatim.
var additionalSampleKeys = []string{"expectedReqExt", "expectedImpExt", "message"}

// LoadSampleRequests loads every JSON file below
// endpoints/openrtb2/sample-requests.
func LoadSampleRequests(repo *Repo) ([]*SampleRequest, []error) {
	var out []*SampleRequest
	var errs []error
	err := repo.Walk(RootSampleRequests, func(rel string, _ fs.DirEntry) error {
		if path.Ext(rel) != ".json" {
			return nil
		}
		sr, err := LoadSampleRequest(repo, rel)
		if err != nil {
			errs = append(errs, err)
			return nil
		}
		out = append(out, sr)
		return nil
	})
	if err != nil {
		errs = append(errs, err)
	}
	return out, errs
}

// LoadSampleRequest loads one sample-request file (repo-relative path below
// endpoints/openrtb2/sample-requests).
func LoadSampleRequest(repo *Repo, rel string) (*SampleRequest, error) {
	if !strings.HasPrefix(rel, RootSampleRequests+"/") {
		return nil, fmt.Errorf("%w: %q is not below %s", ErrOutsideCorpus, rel, RootSampleRequests)
	}
	data, err := repo.ReadFile(rel)
	if err != nil {
		return nil, err
	}
	sr := &SampleRequest{Path: rel, Suite: path.Dir(strings.TrimPrefix(rel, RootSampleRequests+"/"))}
	if err := sr.parse(data); err != nil {
		return nil, fmt.Errorf("corpus: %s: %w", rel, err)
	}
	sr.Endpoint = sampleEndpoint(sr)
	return sr, nil
}

// parse reads the members parseTestData and the AMP/video tests use.
func (sr *SampleRequest) parse(data []byte) error {
	var err error
	if sr.BidRequest, err = parserValue(data, "mockBidRequest"); err != nil {
		return err
	}
	if sr.RequestPayload, err = parserValue(data, "requestPayload"); err != nil {
		return err
	}
	if sr.BidRequest == nil && sr.RequestPayload == nil {
		return errors.New("neither mockBidRequest nor requestPayload is present")
	}
	if sr.Description, err = optionalString(data, "description"); err != nil {
		return err
	}
	if sr.Query, err = optionalString(data, "query"); err != nil {
		return err
	}
	if sr.ExpectedErrorMessage, err = optionalString(data, "expectedErrorMessage"); err != nil {
		return err
	}
	if err := sr.parseReturnCode(data); err != nil {
		return err
	}
	if err := sr.parseConfig(data); err != nil {
		return err
	}
	return sr.parseExpectations(data)
}

func (sr *SampleRequest) parseReturnCode(data []byte) error {
	code, err := jsonparser.GetInt(data, "expectedReturnCode")
	switch {
	case err == nil:
		sr.ExpectedReturnCode = int(code)
	case !errors.Is(err, jsonparser.KeyPathNotFoundError):
		return fmt.Errorf("expectedReturnCode: %w", err)
	}
	return nil
}

func (sr *SampleRequest) parseConfig(data []byte) error {
	raw, err := optionalValue(data, "config")
	if err != nil || raw == nil {
		return err
	}
	sr.Config = &SampleConfig{}
	if err := jsonutil.UnmarshalValid(raw, sr.Config); err != nil {
		return fmt.Errorf("config: %w", err)
	}
	return nil
}

func (sr *SampleRequest) parseExpectations(data []byte) error {
	raws := []struct {
		key string
		dst *json.RawMessage
	}{
		{"expectedBidResponse", &sr.ExpectedBidResponse},
		{"expectedAmpResponse", &sr.ExpectedAmpResponse},
		{"expectedValidatedBidRequest", &sr.ExpectedValidatedReq},
	}
	for _, r := range raws {
		v, err := optionalValue(data, r.key)
		if err != nil {
			return err
		}
		*r.dst = v
	}
	maps := []struct {
		key string
		dst *map[string]json.RawMessage
	}{
		{"expectedMockBidderRequests", &sr.ExpectedBidderReqs},
		{"mockAmpStoredResponse", &sr.AmpStoredResponses},
	}
	for _, m := range maps {
		v, err := optionalValue(data, m.key)
		if err != nil || v == nil {
			if err != nil {
				return err
			}
			continue
		}
		if err := jsonutil.UnmarshalValid(v, m.dst); err != nil {
			return fmt.Errorf("%s: %w", m.key, err)
		}
	}
	for _, key := range additionalSampleKeys {
		v, err := optionalValue(data, key)
		if err != nil {
			return err
		}
		if v != nil {
			if sr.Additional == nil {
				sr.Additional = map[string]json.RawMessage{}
			}
			sr.Additional[key] = v
		}
	}
	return nil
}

// parserValue returns a member exactly as jsonparser.Get extracts it (what
// parseTestData feeds the endpoint), or nil when absent.
func parserValue(data []byte, key string) ([]byte, error) {
	v, _, _, err := jsonparser.Get(data, key)
	if errors.Is(err, jsonparser.KeyPathNotFoundError) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("%s: %w", key, err)
	}
	return v, nil
}

// optionalValue returns a member's JSON text, or nil when absent.
func optionalValue(data []byte, key string) ([]byte, error) {
	v, typ, _, err := jsonparser.Get(data, key)
	if errors.Is(err, jsonparser.KeyPathNotFoundError) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("%s: %w", key, err)
	}
	if typ == jsonparser.String {
		// jsonparser strips the quotes of a string but leaves its escapes, so
		// re-wrapping it restores the original JSON string literal.
		quoted := make([]byte, 0, len(v)+2)
		quoted = append(quoted, '"')
		quoted = append(quoted, v...)
		return append(quoted, '"'), nil
	}
	return v, nil
}

func optionalString(data []byte, key string) (string, error) {
	v, err := jsonparser.GetString(data, key)
	if errors.Is(err, jsonparser.KeyPathNotFoundError) {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("%s: %w", key, err)
	}
	return v, nil
}

// sampleEndpoint maps a sample's directory to the endpoint its Go test drives.
func sampleEndpoint(sr *SampleRequest) string {
	switch {
	case sr.Suite == "amp" || strings.HasPrefix(sr.Suite, "amp/"):
		return EndpointAMP
	case sr.Suite == "video" || strings.HasPrefix(sr.Suite, "video/"):
		return EndpointVideo
	case sr.Suite == "hooks" && strings.HasPrefix(path.Base(sr.Path), "amp"):
		return EndpointAMP
	}
	return EndpointAuction
}

// SampleCases turns sample requests into exec cases. Stored data the Go tests
// serve from mocks (stored requests, imps, accounts) is attached per endpoint.
func SampleCases(repo *Repo, samples []*SampleRequest) ([]Case, []error) {
	stored, err := loadEndpointStored(repo)
	if err != nil {
		return nil, []error{err}
	}
	var cases []Case
	var errs []error
	for _, sr := range samples {
		cs, err := sampleCase(sr, stored)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		cases = append(cases, cs)
	}
	return cases, errs
}

func sampleCase(sr *SampleRequest, stored endpointStored) (Case, error) {
	profile := sampleProfile(sr)
	cs := Case{
		Kind:        KindEndpoint,
		Corpus:      CorpusSampleRequests,
		FixturePath: sr.Path,
		Description: sr.Description,
		Endpoint:    sr.Endpoint,
		Profile:     profile,
		Expect:      sampleExpect(sr),
	}
	switch sr.Endpoint {
	case EndpointAMP:
		if err := ampCase(&cs, sr, stored.Auction); err != nil {
			return Case{}, err
		}
	case EndpointVideo:
		cs.ID = caseID(sr.Path, "video")
		cs.Envelope = newEnvelope(cs.ID, sr.Path, profile.Name(), jsonPostRequest(EndpointVideo, sr.body()))
		cs.Stored = copyStored(stored.Video)
	default:
		cs.ID = caseID(sr.Path, "auction")
		cs.Envelope = newEnvelope(cs.ID, sr.Path, profile.Name(), jsonPostRequest(EndpointAuction, sr.body()))
		cs.Stored = copyStored(stored.Auction)
	}
	cs.Mocks = sampleMocks(sr)
	cs.Tags, cs.Notes = sampleTags(sr)
	return cs, nil
}

// body returns mockBidRequest, falling back to requestPayload.
func (sr *SampleRequest) body() []byte {
	if sr.BidRequest != nil {
		return sr.BidRequest
	}
	return sr.RequestPayload
}

// ampCase builds GET /openrtb2/amp?<query>. The AMP tests serve
// {tag_id: mockBidRequest} as the stored request; every AMP sample uses
// tag_id=101 with different content, so the tag is rewritten to an ID unique
// to the fixture (both servers see the same rewrite).
func ampCase(cs *Case, sr *SampleRequest, auction Stored) error {
	tag, query, ok := rewriteQueryParam(sr.Query, ampTagParam, "parity-amp-"+shortHash(10, sr.Path))
	if !ok {
		return fmt.Errorf("corpus: %s: AMP sample has no %s in its query", sr.Path, ampTagParam)
	}
	cs.ID = caseID(sr.Path, "amp")
	cs.Envelope = newEnvelope(cs.ID, sr.Path, cs.Profile.Name(), getRequest(EndpointAMP, query))
	cs.Stored = Stored{Accounts: copyStored(auction).Accounts}
	cs.Stored.Requests = map[string]json.RawMessage{tag.newValue: json.RawMessage(sr.body())}
	for id, raw := range sr.AmpStoredResponses {
		unquoted, err := strconv.Unquote(string(raw))
		if err != nil {
			return fmt.Errorf("corpus: %s: mockAmpStoredResponse %q: %w", sr.Path, id, err)
		}
		if cs.Stored.Responses == nil {
			cs.Stored.Responses = map[string]json.RawMessage{}
		}
		cs.Stored.Responses[id] = json.RawMessage(unquoted)
	}
	cs.Notes = append(cs.Notes, fmt.Sprintf("AMP %s %q rewritten to %q so stored request IDs stay unique", ampTagParam, tag.oldValue, tag.newValue))
	return nil
}

type paramRewrite struct {
	oldValue string
	newValue string
}

// rewriteQueryParam replaces the value of the first name=value segment of a
// raw query, leaving every other byte of the query untouched.
func rewriteQueryParam(raw, name, value string) (paramRewrite, string, bool) {
	parts := strings.Split(raw, "&")
	for i, part := range parts {
		key, old, _ := strings.Cut(part, "=")
		if key == name && old != "" {
			parts[i] = name + "=" + value
			return paramRewrite{oldValue: old, newValue: value}, strings.Join(parts, "&"), true
		}
	}
	return paramRewrite{}, raw, false
}

// sampleProfile maps a sample's config block to a profile. AMP tests enable
// GDPR explicitly (amp_auction_test.go TestGoodAmpRequests).
func sampleProfile(sr *SampleRequest) Profile {
	var p Profile
	if sr.Endpoint == EndpointAMP && !strings.HasPrefix(sr.Suite, "hooks") {
		p.GDPREnabled = boolPtr(true)
	}
	cfg := sr.Config
	if cfg == nil {
		return p
	}
	p.AccountRequired = cfg.AccountRequired
	p.BlockedApps = cfg.BlockedApps
	p.DisabledAdapters = cfg.DisabledAdapters
	p.CurrencyRates = cfg.CurrencyRates
	if cfg.AliasJSON != "" {
		p.DefaultRequest = json.RawMessage(cfg.AliasJSON)
	}
	p.BidderInfoOverrides = cfg.BidderInfoOverrides
	if len(cfg.PreferredMediaType) > 0 && string(cfg.PreferredMediaType) != "null" {
		p.PreferredMediaType = cfg.PreferredMediaType
	}
	return p
}

// sampleMocks maps mockBidders to template parameters.
func sampleMocks(sr *SampleRequest) MockPlan {
	plan := MockPlan{ImpIDs: impIDs(sr.body())}
	if sr.Config == nil || len(sr.Config.MockBidders) == 0 {
		return plan
	}
	plan.Bidders = make(map[string]MockBidder, len(sr.Config.MockBidders))
	for _, mb := range sr.Config.MockBidders {
		price := mb.Price
		plan.Bidders[mb.BidderName] = MockBidder{Currency: mb.Currency, Price: &price, DealID: mb.DealID, Seat: mb.Seat}
	}
	return plan
}

func sampleExpect(sr *SampleRequest) Expect {
	return Expect{
		Status:            sr.ExpectedReturnCode,
		ErrorContains:     sr.ExpectedErrorMessage,
		BidResponse:       sr.ExpectedBidResponse,
		AmpResponse:       sr.ExpectedAmpResponse,
		ValidatedRequest:  sr.ExpectedValidatedReq,
		BidderRequests:    sr.ExpectedBidderReqs,
		AdditionalExpects: sr.Additional,
	}
}

func sampleTags(sr *SampleRequest) ([]string, []string) {
	var tags, notes []string
	if sr.Config == nil || !sr.Config.RealParamsValidator {
		tags = addTag(tags, TagMockParamsValidator)
	}
	if strings.HasPrefix(sr.Suite, "hooks") {
		tags = addTag(tags, TagTestHooks)
		notes = append(notes, "the Go test installs mock hook plans; the live servers run without them")
	}
	if !json.Valid(sr.body()) {
		tags = addTag(tags, TagInvalidJSON)
	}
	return tags, notes
}
