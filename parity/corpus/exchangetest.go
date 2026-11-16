package corpus

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"path"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Values exchange_test.go hard-codes for every spec.
var (
	// exchangeTestEEACountries is the privacy config's EEA list in runSpec.
	exchangeTestEEACountries = []string{"FIN", "FRA", "GUF"}
	// exchangeTestHostSChain is the node newExchangeForTests uses for host_schain_flag.
	exchangeTestHostSChain = json.RawMessage(`{"asi":"pbshostcompany.com","sid":"00001","rid":"BidRequest","hp":1}`)
)

// unknownErrorCode is errortypes.UnknownErrorCode: exchangetest mock errors are
// plain errors.New values.
const unknownErrorCode = 999

// uidsCookieExpiry is far enough in the future that the parsed cookie never
// expires during a run.
const uidsCookieExpiry = "2099-01-01T00:00:00Z"

// ExchangeSpec is one exchange/exchangetest file, mirroring exchange_test.go
// exchangeSpec with raw JSON where both servers must see the exact bytes.
type ExchangeSpec struct {
	Path                       string                         `json:"path"`
	GDPREnforced               bool                           `json:"gdpr_enforced"`
	FloorsEnabled              bool                           `json:"floors_enabled"`
	IncomingRequest            ExchangeIncoming               `json:"incomingRequest"`
	OutgoingRequests           map[string]*ExchangeBidderSpec `json:"outgoingRequests"`
	Response                   ExchangeResponse               `json:"response"`
	EnforceCCPA                bool                           `json:"enforceCcpa"`
	EnforceLMT                 bool                           `json:"enforceLmt"`
	AssumeGDPRApplies          bool                           `json:"assume_gdpr_applies"`
	DebugLog                   json.RawMessage                `json:"debuglog"`
	EventsEnabled              bool                           `json:"events_enabled"`
	StartTime                  int64                          `json:"start_time_ms"`
	BidIDGenerator             json.RawMessage                `json:"bidIDGenerator"`
	RequestType                json.RawMessage                `json:"requestType"`
	PassthroughFlag            bool                           `json:"passthrough_flag"`
	HostSChainFlag             bool                           `json:"host_schain_flag"`
	HostConfigBidValidation    json.RawMessage                `json:"host_bid_validations"`
	AccountConfigBidValidation json.RawMessage                `json:"account_bid_validations"`
	AccountFloorsEnabled       bool                           `json:"account_floors_enabled"`
	AccountEnforceDealFloors   bool                           `json:"account_enforce_deal_floors"`
	FledgeEnabled              bool                           `json:"fledge_enabled"`
	MultiBid                   *ExchangeMultiBid              `json:"multiBid"`
	Server                     ExchangeServer                 `json:"server"`
	AccountPrivacy             json.RawMessage                `json:"accountPrivacy"`
	ORTBVersion                map[string]string              `json:"ortbversion"`
	AccountEEACountries        []string                       `json:"account_eea_countries"`
}

// ExchangeIncoming mirrors exchangeRequest.
type ExchangeIncoming struct {
	OrtbRequest json.RawMessage   `json:"ortbRequest"`
	Usersyncs   map[string]string `json:"usersyncs"`
}

// ExchangeResponse mirrors exchangeResponse.
type ExchangeResponse struct {
	Bids  json.RawMessage `json:"bids"`
	Error string          `json:"error"`
	Ext   json.RawMessage `json:"ext"`
}

// ExchangeServer mirrors exchangeServer.
type ExchangeServer struct {
	ExternalURL string `json:"externalURL"`
	GvlID       int    `json:"gvlID"`
	DataCenter  string `json:"dataCenter"`
}

// ExchangeMultiBid mirrors multiBidSpec.
type ExchangeMultiBid struct {
	AccountMaxBid          int  `json:"default_bid_limit"`
	AssertMultiBidWarnings bool `json:"assert_multi_bid_warnings"`
}

// ExchangeBidderSpec mirrors bidderSpec.
type ExchangeBidderSpec struct {
	ExpectedRequest         json.RawMessage        `json:"expectRequest"`
	MockResponse            ExchangeBidderResponse `json:"mockResponse"`
	ModifyingVastXMLAllowed bool                   `json:"modifyingVastXmlAllowed"`
}

// ExchangeBidderResponse mirrors bidderResponse.
type ExchangeBidderResponse struct {
	SeatBids  []ExchangeSeatBid `json:"pbsSeatBids"`
	Errors    []string          `json:"errors"`
	HTTPCalls json.RawMessage   `json:"httpCalls"`
}

// ExchangeSeatBid mirrors bidderSeatBid.
type ExchangeSeatBid struct {
	Bids                 []ExchangeBid   `json:"pbsBids"`
	Seat                 string          `json:"seat"`
	Currency             string          `json:"currency"`
	FledgeAuctionConfigs json.RawMessage `json:"fledgeAuctionConfigs"`
}

// ExchangeBid mirrors bidderBid.
type ExchangeBid struct {
	Bid      json.RawMessage `json:"ortbBid"`
	Type     string          `json:"bidType"`
	BidVideo json.RawMessage `json:"bidVideo"`
	Meta     json.RawMessage `json:"bidMeta"`
}

// typedExchangeSpec reproduces the Go loader's parse errors: exchange_test.go
// decodes these members into these types with jsonutil.UnmarshalValid.
type typedExchangeSpec struct {
	IncomingRequest struct {
		OrtbRequest openrtb2.BidRequest `json:"ortbRequest"`
	} `json:"incomingRequest"`
	OutgoingRequests map[string]*struct {
		ExpectedRequest *struct {
			OrtbRequest    openrtb2.BidRequest `json:"ortbRequest"`
			BidAdjustments map[string]float64  `json:"bidAdjustments"`
		} `json:"expectRequest"`
		MockResponse struct {
			SeatBids []*struct {
				Bids []struct {
					Bid *openrtb2.Bid `json:"ortbBid"`
				} `json:"pbsBids"`
			} `json:"pbsSeatBids"`
		} `json:"mockResponse"`
	} `json:"outgoingRequests"`
	Response struct {
		Bids *openrtb2.BidResponse `json:"bids"`
	} `json:"response"`
	HostConfigBidValidation    config.Validations    `json:"host_bid_validations"`
	AccountConfigBidValidation config.Validations    `json:"account_bid_validations"`
	AccountPrivacy             config.AccountPrivacy `json:"accountPrivacy"`
}

// LoadExchangeSpecs loads every *.json file in exchange/exchangetest.
func LoadExchangeSpecs(repo *Repo) ([]*ExchangeSpec, []error) {
	entries, err := repo.ReadDir(RootExchangeTest)
	if err != nil {
		return nil, []error{err}
	}
	var out []*ExchangeSpec
	var errs []error
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		spec, err := LoadExchangeSpec(repo, path.Join(RootExchangeTest, e.Name()))
		if err != nil {
			errs = append(errs, err)
			continue
		}
		out = append(out, spec)
	}
	return out, errs
}

// LoadExchangeSpec loads one exchangetest file (repo-relative path).
func LoadExchangeSpec(repo *Repo, rel string) (*ExchangeSpec, error) {
	data, err := repo.ReadFile(rel)
	if err != nil {
		return nil, err
	}
	var typed typedExchangeSpec
	if err := jsonutil.UnmarshalValid(data, &typed); err != nil {
		return nil, fmt.Errorf("corpus: %s: %w", rel, err)
	}
	spec := &ExchangeSpec{Path: rel}
	if err := json.Unmarshal(data, spec); err != nil {
		return nil, fmt.Errorf("corpus: %s: %w", rel, err)
	}
	if len(spec.IncomingRequest.OrtbRequest) == 0 {
		return nil, fmt.Errorf("corpus: %s: missing incomingRequest.ortbRequest", rel)
	}
	return spec, nil
}

// ExchangeCases turns exchangetest specs into auction cases at
// mock.level=bidder_response. syncerKeys maps bidder names to user-sync keys
// for the uids cookie (missing bidders use their own name).
func ExchangeCases(specs []*ExchangeSpec, syncerKeys map[string]string) ([]Case, []error) {
	var cases []Case
	var errs []error
	for _, spec := range specs {
		cs, err := exchangeCase(spec, syncerKeys)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		cases = append(cases, cs)
	}
	return cases, errs
}

func exchangeCase(spec *ExchangeSpec, syncerKeys map[string]string) (Case, error) {
	id := caseID(spec.Path, "exchange")
	profile := spec.profile()
	req := jsonPostRequest(EndpointAuction, spec.IncomingRequest.OrtbRequest)
	if cookie, err := uidsCookie(spec.IncomingRequest.Usersyncs, syncerKeys); err != nil {
		return Case{}, fmt.Errorf("corpus: %s: %w", spec.Path, err)
	} else if cookie != "" {
		req.Headers = append(req.Headers, [2]string{"Cookie", "uids=" + cookie})
	}
	env := newEnvelope(id, spec.Path, profile.Name(), req)
	env.Mock.Level = parity.MockLevelBidderResp
	env.Mock.BidderResponses = map[string]parity.BidderResponseMock{}
	var notes []string
	for _, bidder := range sortedKeys(spec.OutgoingRequests) {
		mock, mockNotes := spec.OutgoingRequests[bidder].MockResponse.toMock(bidder)
		env.Mock.BidderResponses[bidder] = mock
		notes = append(notes, mockNotes...)
	}
	internal := spec.internalSettings()
	cs := Case{
		ID:          id,
		Kind:        KindExchange,
		Corpus:      CorpusExchangeTest,
		FixturePath: spec.Path,
		Endpoint:    EndpointAuction,
		Profile:     profile,
		Envelope:    env,
		Mocks:       MockPlan{ImpIDs: impIDs(spec.IncomingRequest.OrtbRequest)},
		Expect:      Expect{BidResponse: spec.Response.Bids, ExchangeError: spec.Response.Error, ResponseExt: spec.Response.Ext},
		Notes:       append(notes, internal...),
	}
	if len(internal) > 0 {
		cs.Tags = addTag(cs.Tags, TagExchangeInternal)
	}
	if hasUntypedBid(env.Mock.BidderResponses) {
		cs.Tags = addTag(cs.Tags, TagContractGap)
		cs.Notes = append(cs.Notes, `contract gap: a pbsBid has no bidType (Go uses BidType("")) but common.schema.json TypedBid.type only allows banner, video, audio and native`)
	}
	return cs, nil
}

// hasUntypedBid reports whether any mocked bid has an empty type.
func hasUntypedBid(mocks map[string]parity.BidderResponseMock) bool {
	for _, mock := range mocks {
		for _, bid := range mock.Bids {
			if bid.Type == "" {
				return true
			}
		}
	}
	return false
}

// toMock converts pbsSeatBids into the MakeBids replacement. Seats other than
// the bidder become alternate-bidder-code seats; the first currency wins.
func (r ExchangeBidderResponse) toMock(bidder string) (parity.BidderResponseMock, []string) {
	mock := parity.BidderResponseMock{Bids: []parity.TypedBid{}}
	var notes []string
	var fledge []json.RawMessage
	for _, sb := range r.SeatBids {
		if mock.Currency == "" {
			mock.Currency = sb.Currency
		} else if sb.Currency != "" && sb.Currency != mock.Currency {
			notes = append(notes, fmt.Sprintf("%s: seat %q currency %q differs from %q; one currency per bidder response is kept", bidder, sb.Seat, sb.Currency, mock.Currency))
		}
		seat := sb.Seat
		if seat == bidder {
			seat = ""
		}
		for i, b := range sb.Bids {
			if len(b.Bid) == 0 || string(b.Bid) == "null" {
				notes = append(notes, fmt.Sprintf("%s: seat %q bid %d has no ortbBid and is skipped", bidder, sb.Seat, i))
				continue
			}
			mock.Bids = append(mock.Bids, parity.TypedBid{Bid: b.Bid, Type: b.Type, Seat: seat, Video: nonNull(b.BidVideo), Meta: nonNull(b.Meta)})
		}
		if len(sb.FledgeAuctionConfigs) > 0 && string(sb.FledgeAuctionConfigs) != "null" {
			fledge = append(fledge, sb.FledgeAuctionConfigs)
		}
	}
	if merged, err := concatArrays(fledge); err == nil && merged != nil {
		mock.FledgeAuctionConfigs = merged
	} else if err != nil {
		notes = append(notes, fmt.Sprintf("%s: fledgeAuctionConfigs: %v", bidder, err))
	}
	for _, msg := range r.Errors {
		mock.Errors = append(mock.Errors, parity.ErrorSpec{Code: unknownErrorCode, Message: msg})
	}
	if len(r.HTTPCalls) > 0 && string(r.HTTPCalls) != "null" {
		notes = append(notes, fmt.Sprintf("%s: mock httpCalls (debug output injected by the Go test) have no live equivalent", bidder))
	}
	return mock, notes
}

// profile maps the spec's flags to host settings (exchange_test.go runSpec
// and newExchangeForTests).
func (s *ExchangeSpec) profile() Profile {
	p := Profile{
		GDPREnabled:              boolPtr(s.GDPREnforced),
		GDPRDefaultValue:         "0",
		GDPREEACountries:         exchangeTestEEACountries,
		CCPAEnforce:              s.EnforceCCPA,
		LMTEnforce:               boolPtr(s.EnforceLMT),
		FloorsEnabled:            s.FloorsEnabled,
		AccountFloorsEnabled:     s.AccountFloorsEnabled,
		AccountEnforceDealFloors: s.AccountEnforceDealFloors,
		EventsEnabled:            s.EventsEnabled,
		HostValidations:          nonNull(s.HostConfigBidValidation),
		AccountValidations:       nonNull(s.AccountConfigBidValidation),
		AccountPrivacy:           nonNull(s.AccountPrivacy),
		AccountEEACountries:      s.AccountEEACountries,
		ExternalURL:              s.Server.ExternalURL,
		HostVendorID:             s.Server.GvlID,
		DataCenter:               s.Server.DataCenter,
	}
	if s.AssumeGDPRApplies {
		p.GDPRDefaultValue = "1"
	}
	if s.HostSChainFlag {
		p.HostSChainNode = exchangeTestHostSChain
	}
	if s.MultiBid != nil {
		p.AccountDefaultBidLimit = s.MultiBid.AccountMaxBid
	}
	for bidder, version := range s.ORTBVersion {
		if version == "" {
			continue
		}
		if p.ORTBVersions == nil {
			p.ORTBVersions = map[string]string{}
		}
		p.ORTBVersions[bidder] = version
	}
	for _, bidder := range sortedKeys(s.OutgoingRequests) {
		if s.OutgoingRequests[bidder].ModifyingVastXMLAllowed {
			if p.BidderInfoOverrides == nil {
				p.BidderInfoOverrides = map[string]json.RawMessage{}
			}
			p.BidderInfoOverrides[bidder] = json.RawMessage(`{"modifyingVastXmlAllowed":true}`)
		}
	}
	return p
}

// internalSettings lists spec settings that only exist inside the Go test.
func (s *ExchangeSpec) internalSettings() []string {
	var out []string
	add := func(ok bool, what string) {
		if ok {
			out = append(out, "not reproducible live: "+what)
		}
	}
	add(s.StartTime != 0, "start_time_ms (auction start time)")
	add(len(nonNull(s.BidIDGenerator)) > 0, "bidIDGenerator (deterministic bid.ext.prebid.bidid)")
	add(len(nonNull(s.RequestType)) > 0, "requestType (metrics request type)")
	add(len(nonNull(s.DebugLog)) > 0, "debuglog (video debug log)")
	return out
}

// uidsCookie encodes usersyncs as the uids cookie value (usersync
// Base64Encoder: URL-safe base64 of {"tempUIDs": {key: {uid, expires}}}).
func uidsCookie(syncs map[string]string, syncerKeys map[string]string) (string, error) {
	if len(syncs) == 0 {
		return "", nil
	}
	type entry struct {
		UID     string `json:"uid"`
		Expires string `json:"expires"`
	}
	uids := map[string]entry{}
	for bidder, uid := range syncs {
		key := bidder
		if k, ok := syncerKeys[bidder]; ok && k != "" {
			key = k
		}
		uids[key] = entry{UID: uid, Expires: uidsCookieExpiry}
	}
	raw, err := json.Marshal(struct {
		UIDs map[string]entry `json:"tempUIDs"`
	}{uids})
	if err != nil {
		return "", fmt.Errorf("encoding uids cookie: %w", err)
	}
	return base64.URLEncoding.EncodeToString(raw), nil
}

// SyncerKeys reads static/bidder-info and maps each bidder to its user-sync key.
func SyncerKeys(repo *Repo) (map[string]string, error) {
	dir, err := repo.Resolve(RootBidderInfo)
	if err != nil {
		return nil, err
	}
	infos, err := config.LoadBidderInfoFromDisk(dir)
	if err != nil {
		return nil, fmt.Errorf("corpus: loading bidder info: %w", err)
	}
	out := make(map[string]string, len(infos))
	for name, info := range infos {
		if info.Syncer != nil && info.Syncer.Key != "" {
			out[name] = info.Syncer.Key
		}
	}
	return out, nil
}

func nonNull(raw json.RawMessage) json.RawMessage {
	if len(raw) == 0 || string(raw) == "null" {
		return nil
	}
	return raw
}

// concatArrays joins JSON arrays into one.
func concatArrays(arrays []json.RawMessage) (json.RawMessage, error) {
	if len(arrays) == 0 {
		return nil, nil
	}
	var all []json.RawMessage
	for _, a := range arrays {
		var items []json.RawMessage
		if err := json.Unmarshal(a, &items); err != nil {
			return nil, err
		}
		all = append(all, items...)
	}
	return json.Marshal(all)
}
