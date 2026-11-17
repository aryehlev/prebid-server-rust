package corpus

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io/fs"
	"net/http"
	"path"
	"sort"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/parity"
)

// Fixture directories adapterstest runs (adapters/adapterstest/test_json.go
// supportedDirs). Fixtures are recognised by the name of their parent directory.
const (
	DirExemplary         = "exemplary"
	DirSupplemental      = "supplemental"
	DirAMP               = "amp"
	DirVideo             = "video"
	DirVideoSupplemental = "videosupplemental"
)

// Entry points adapterstest passes in ExtraRequestInfo.PbsEntryPoint.
const (
	entryPointAMP   = "amp"
	entryPointVideo = "video"
)

var supportedFixtureDirs = map[string]bool{
	DirExemplary: true, DirSupplemental: true, DirAMP: true, DirVideo: true, DirVideoSupplemental: true,
}

// AdapterFixture is one adapterstest JSON file.
type AdapterFixture struct {
	// Path is the repo-relative slash path of the file.
	Path string `json:"path"`
	// Package is the adapters/<pkg> directory the file belongs to.
	Package string `json:"package"`
	// Dir is the fixture directory kind (exemplary, supplemental, amp, video,
	// videosupplemental).
	Dir  string      `json:"dir"`
	Spec AdapterSpec `json:"spec"`
	// Runs are the RunJSONBidderTest runs whose root covers this file, in
	// extraction order. Empty when no run could be matched.
	Runs  []*AdapterRun `json:"-"`
	Notes []string      `json:"notes,omitempty"`
}

// AllowErrors mirrors adapterstest: only exemplary and video fixtures must not
// expect errors.
func (f *AdapterFixture) AllowErrors() bool {
	return f.Dir != DirExemplary && f.Dir != DirVideo
}

// EntryPoint mirrors adapterstest getTestExtraRequestInfo.
func (f *AdapterFixture) EntryPoint() string {
	switch f.Dir {
	case DirAMP:
		return entryPointAMP
	case DirVideo, DirVideoSupplemental:
		return entryPointVideo
	}
	return ""
}

// Bidder returns the bidder of the fixture's first run, or "" when unmatched.
func (f *AdapterFixture) Bidder() string {
	if len(f.Runs) == 0 {
		return ""
	}
	return f.Runs[0].Bidder
}

// AdapterSpec mirrors adapterstest testSpec, keeping raw JSON so both
// implementations receive the exact fixture bytes.
type AdapterSpec struct {
	BidRequest         json.RawMessage      `json:"mockBidRequest"`
	HTTPCalls          []AdapterHTTPCall    `json:"httpCalls"`
	BidResponses       []AdapterBidResponse `json:"expectedBidResponses"`
	MakeRequestsErrors []ExpectedError      `json:"expectedMakeRequestsErrors"`
	MakeBidsErrors     []ExpectedError      `json:"expectedMakeBidsErrors"`
}

// AdapterHTTPCall mirrors adapterstest httpCall.
type AdapterHTTPCall struct {
	Request  AdapterHTTPRequest  `json:"expectedRequest"`
	Response AdapterHTTPResponse `json:"mockResponse"`
}

// AdapterHTTPRequest mirrors adapterstest httpRequest.
type AdapterHTTPRequest struct {
	Body    json.RawMessage `json:"body"`
	URI     string          `json:"uri"`
	Headers http.Header     `json:"headers"`
	ImpIDs  []string        `json:"impIDs"`
}

// AdapterHTTPResponse mirrors adapterstest httpResponse.
type AdapterHTTPResponse struct {
	Status  int             `json:"status"`
	Body    json.RawMessage `json:"body"`
	Headers http.Header     `json:"headers"`
}

// AdapterBidResponse mirrors adapterstest expectedBidResponse.
type AdapterBidResponse struct {
	Bids                 []AdapterExpectedBid `json:"bids"`
	Currency             string               `json:"currency"`
	FledgeAuctionConfigs json.RawMessage      `json:"fledgeauctionconfigs,omitempty"`
}

// AdapterExpectedBid mirrors adapterstest expectedBid.
type AdapterExpectedBid struct {
	Bid   json.RawMessage `json:"bid"`
	Type  string          `json:"type"`
	Seat  string          `json:"seat"`
	Video json.RawMessage `json:"video,omitempty"`
}

// ExpectedError mirrors adapterstest testSpecExpectedError. Comparison is
// literal (default), regex or startswith.
type ExpectedError struct {
	Value      string `json:"value"`
	Comparison string `json:"comparison"`
}

// typedAdapterSpec is decoded only to reproduce the Go loader's parse errors:
// adapterstest unmarshals mockBidRequest into openrtb2.BidRequest.
type typedAdapterSpec struct {
	BidRequest openrtb2.BidRequest `json:"mockBidRequest"`
}

// AdapterCorpus is every adapter fixture plus the runs that execute them.
type AdapterCorpus struct {
	Runs     []*AdapterRun
	Fixtures []*AdapterFixture
	// Errors are per-file load errors; the corpus stays usable without them.
	Errors []error
}

// LoadAdapterCorpus extracts the adapter test runs and loads every fixture in a
// supported directory under adapters/, attaching the runs whose root covers it.
func LoadAdapterCorpus(repo *Repo) (*AdapterCorpus, error) {
	runs, err := ExtractAdapterRuns(repo)
	if err != nil {
		return nil, err
	}
	paths, err := adapterFixturePaths(repo)
	if err != nil {
		return nil, err
	}
	out := &AdapterCorpus{Runs: runs}
	for _, p := range paths {
		fixture, err := LoadAdapterFixture(repo, p)
		if err != nil {
			out.Errors = append(out.Errors, err)
			continue
		}
		fixture.Runs = runsCovering(runs, p)
		out.Fixtures = append(out.Fixtures, fixture)
	}
	return out, nil
}

// adapterFixturePaths lists every *.json file whose parent directory is a
// supported fixture directory, in lexical order.
func adapterFixturePaths(repo *Repo) ([]string, error) {
	var paths []string
	err := repo.Walk(RootAdapters, func(rel string, _ fs.DirEntry) error {
		if path.Ext(rel) == ".json" && supportedFixtureDirs[path.Base(path.Dir(rel))] {
			paths = append(paths, rel)
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	sort.Strings(paths)
	return paths, nil
}

// runsCovering returns the runs whose root directory contains rel.
func runsCovering(runs []*AdapterRun, rel string) []*AdapterRun {
	var out []*AdapterRun
	for _, run := range runs {
		if run.Root != "" && strings.HasPrefix(rel, run.Root+"/") {
			out = append(out, run)
		}
	}
	return out
}

// LoadAdapterFixture loads one adapterstest JSON file (repo-relative path).
func LoadAdapterFixture(repo *Repo, rel string) (*AdapterFixture, error) {
	dir := path.Base(path.Dir(rel))
	if !supportedFixtureDirs[dir] || !strings.HasPrefix(rel, RootAdapters+"/") {
		return nil, fmt.Errorf("corpus: %s is not in a supported adapter fixture directory", rel)
	}
	data, err := repo.ReadFile(rel)
	if err != nil {
		return nil, err
	}
	var spec AdapterSpec
	if err := json.Unmarshal(data, &spec); err != nil {
		return nil, fmt.Errorf("corpus: %s: %w", rel, err)
	}
	var typed typedAdapterSpec
	if err := json.Unmarshal(data, &typed); err != nil {
		return nil, fmt.Errorf("corpus: %s: mockBidRequest: %w", rel, err)
	}
	parts := strings.SplitN(rel, "/", 3)
	fixture := &AdapterFixture{Path: rel, Package: parts[0] + "/" + parts[1], Dir: dir, Spec: spec}
	if len(spec.BidRequest) == 0 || string(spec.BidRequest) == "null" {
		// adapterstest still runs such a file, with a zero openrtb2.BidRequest.
		fixture.Spec.BidRequest = json.RawMessage(`{}`)
		fixture.Notes = append(fixture.Notes, "fixture has no mockBidRequest; adapterstest runs it with an empty request")
	}
	return fixture, nil
}

// AdapterCases returns one KindAdapter case per (fixture, covering run).
func (c *AdapterCorpus) AdapterCases() ([]Case, []error) {
	var cases []Case
	var errs []error
	for _, f := range c.Fixtures {
		for _, run := range f.Runs {
			cs, err := adapterCase(f, run)
			if err != nil {
				errs = append(errs, err)
				continue
			}
			cases = append(cases, cs)
		}
	}
	return cases, errs
}

// adapterCase builds the adapter-endpoint inputs for one fixture run, exactly
// as adapterstest runSpec feeds the bidder.
func adapterCase(f *AdapterFixture, run *AdapterRun) (Case, error) {
	id := caseID(f.Path, "adapter:"+run.TestFunc)
	extra, err := fixtureExtraInfo(f)
	if err != nil {
		return Case{}, fmt.Errorf("corpus: %s: %w", f.Path, err)
	}
	build := run.Build()
	ac := &AdapterCase{
		Run: AdapterRunRef{TestFile: run.TestFile, TestFunc: run.TestFunc, Root: run.Root},
		Requests: parity.AdapterRequestsInput{
			Schema:        parity.SchemaAdapterRequestsInput,
			SchemaVersion: parity.SchemaVersion,
			CaseID:        id,
			FixturePath:   f.Path,
			Build:         build,
			Request:       f.Spec.BidRequest,
			ExtraInfo:     extra,
		},
		Bids: make([]parity.AdapterBidsInput, 0, len(f.Spec.HTTPCalls)),
		Expect: AdapterExpect{
			AllowErrors:        f.AllowErrors(),
			HTTPCalls:          f.Spec.HTTPCalls,
			BidResponses:       f.Spec.BidResponses,
			MakeRequestsErrors: f.Spec.MakeRequestsErrors,
			MakeBidsErrors:     f.Spec.MakeBidsErrors,
		},
	}
	for i, call := range f.Spec.HTTPCalls {
		ac.Bids = append(ac.Bids, parity.AdapterBidsInput{
			Schema:        parity.SchemaAdapterBidsInput,
			SchemaVersion: parity.SchemaVersion,
			CaseID:        fmt.Sprintf("%s:bids[%d]", id, i),
			FixturePath:   f.Path,
			Build:         build,
			Request:       f.Spec.BidRequest,
			RequestData:   fixtureRequestData(call.Request),
			Response:      fixtureResponseData(call.Response),
			ExtraInfo:     extra,
		})
	}
	cs := Case{
		ID:          id,
		Kind:        KindAdapter,
		Corpus:      CorpusAdapters,
		FixturePath: f.Path,
		Endpoint:    EndpointAdapterRequest,
		Profile:     Profile{},
		Adapter:     ac,
		Mocks:       MockPlan{ImpIDs: impIDs(f.Spec.BidRequest)},
		Notes:       append([]string(nil), f.Notes...),
	}
	if run.NeedsFake {
		cs.Tags = addTag(cs.Tags, TagNeedsFake)
		cs.Notes = append(cs.Notes, run.FakeReasons...)
	}
	if !run.Resolved {
		cs.Tags = addTag(cs.Tags, TagUnresolvedBuild)
		cs.Notes = append(cs.Notes, run.Problems...)
	}
	if f.AllowErrors() {
		cs.Tags = addTag(cs.Tags, TagAllowErrors)
	}
	return cs, nil
}

// fixtureRequestData is adapterstest httpRequest.ToRequestData: method POST,
// the expected uri and body, no headers and no imp IDs.
func fixtureRequestData(req AdapterHTTPRequest) parity.AdapterRequestData {
	return parity.AdapterRequestData{
		Method:  http.MethodPost,
		URI:     req.URI,
		Headers: parity.HeaderList{},
		Body:    rawBody(req.Body),
		ImpIDs:  nil,
	}
}

// fixtureResponseData is adapterstest httpResponse.ToResponseData. The body is
// the raw JSON text of the fixture value (a JSON string keeps its quotes).
func fixtureResponseData(resp AdapterHTTPResponse) parity.AdapterHTTPResponse {
	return parity.AdapterHTTPResponse{
		Status:  resp.Status,
		Headers: headerListOf(resp.Headers),
		Body:    rawBody(resp.Body),
	}
}

// fixtureExtraInfo mirrors adapterstest getTestExtraRequestInfo: custom
// currency rates from request.ext.prebid.currency.rates and the entry point.
func fixtureExtraInfo(f *AdapterFixture) (parity.ExtraRequestInfo, error) {
	info := parity.ExtraRequestInfo{PbsEntryPoint: f.EntryPoint()}
	var req struct {
		Ext json.RawMessage `json:"ext"`
	}
	if err := json.Unmarshal(f.Spec.BidRequest, &req); err != nil || len(req.Ext) == 0 {
		return info, nil
	}
	var ext struct {
		Prebid struct {
			Currency *struct {
				Rates map[string]map[string]float64 `json:"rates"`
			} `json:"currency"`
		} `json:"prebid"`
	}
	if err := json.Unmarshal(req.Ext, &ext); err != nil {
		return info, fmt.Errorf("request.ext: %w", err)
	}
	if ext.Prebid.Currency != nil && len(ext.Prebid.Currency.Rates) > 0 {
		info.CurrencyRates = &parity.CurrencyRates{Conversions: ext.Prebid.Currency.Rates}
	}
	return info, nil
}

// LiftedCases lifts every fixture with a resolved bidder into a full
// /openrtb2/auction case whose bidder responses are the fixture's mockResponses.
func (c *AdapterCorpus) LiftedCases() ([]Case, []error) {
	var cases []Case
	var errs []error
	for _, f := range c.Fixtures {
		bidder := f.Bidder()
		if bidder == "" {
			continue
		}
		cs, err := LiftFixture(f, bidder)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		cases = append(cases, cs)
	}
	return cases, errs
}

// LiftFixture turns an adapter fixture into an auction case: imp[].ext.bidder
// moves to imp[].ext.prebid.bidder.<bidder>, a site is added when the request
// has no site, app or dooh, and each httpCall's mockResponse becomes an
// explicit oracle response matched by bidder and imp-ID set.
func LiftFixture(f *AdapterFixture, bidder string) (Case, error) {
	body, err := liftRequest(f.Spec.BidRequest, bidder)
	if err != nil {
		return Case{}, fmt.Errorf("corpus: lifting %s: %w", f.Path, err)
	}
	id := caseID(f.Path, "lift:auction")
	plan := MockPlan{ImpIDs: impIDs(body)}
	for i, call := range f.Spec.HTTPCalls {
		plan.HTTP = append(plan.HTTP, HTTPMock{
			ID:     fmt.Sprintf("%s#httpCalls[%d]", f.Path, i),
			Bidder: bidder,
			ImpIDs: sortedStrings(call.Request.ImpIDs),
			Response: parity.InlineResponse{
				Status:  call.Response.Status,
				Headers: headerListOf(call.Response.Headers),
				Body:    rawBody(call.Response.Body),
			},
		})
	}
	env := newEnvelope(id, f.Path, DefaultProfile, jsonPostRequest(EndpointAuction, body))
	cs := Case{
		ID:          id,
		Kind:        KindLifted,
		Corpus:      CorpusAdapters,
		FixturePath: f.Path,
		Endpoint:    EndpointAuction,
		Profile:     Profile{},
		Envelope:    env,
		Mocks:       plan,
	}
	if status, ok := unservableStatus(plan.HTTP); ok {
		cs.Tags = addTag(cs.Tags, TagContractGap)
		cs.Notes = append(cs.Notes, fmt.Sprintf("contract gap: mockResponse status %d has no encoding in envelope.schema.json InlineResponse.status (%d-%d; 0 is omitted and reads as 200) and cannot be sent over HTTP; the adapter case still covers the fixture", status, minMockStatus, maxMockStatus))
	}
	return cs, nil
}

// The statuses an oracle response may carry (envelope.schema.json
// InlineResponse.status).
const (
	minMockStatus = 100
	maxMockStatus = 599
)

// unservableStatus returns the first mocked status outside the range the
// oracle can serve. Adapter tests hand such statuses (0, 42) to MakeBids
// in-process; a lifted auction can only receive them over the wire.
func unservableStatus(mocks []HTTPMock) (int, bool) {
	for _, m := range mocks {
		if m.Response.Fail == nil && (m.Response.Status < minMockStatus || m.Response.Status > maxMockStatus) {
			return m.Response.Status, true
		}
	}
	return 0, false
}

// liftRequest rewrites an adapter-level request into an auction request.
func liftRequest(raw json.RawMessage, bidder string) ([]byte, error) {
	req, err := decodeObject(raw)
	if err != nil {
		return nil, err
	}
	imps, _ := req["imp"].([]any)
	for i, imp := range imps {
		obj, ok := imp.(map[string]any)
		if !ok {
			return nil, fmt.Errorf("imp[%d] is not an object", i)
		}
		liftImpExt(obj, bidder)
	}
	_, hasSite := req["site"]
	_, hasApp := req["app"]
	_, hasDOOH := req["dooh"]
	if !hasSite && !hasApp && !hasDOOH {
		req["site"] = map[string]any{"page": "https://parity.prebid.test/lifted"}
	}
	if _, ok := req["id"]; !ok {
		req["id"] = "parity-lifted-request"
	}
	return json.Marshal(req)
}

// liftImpExt moves imp.ext.bidder to imp.ext.prebid.bidder.<bidder>.
func liftImpExt(imp map[string]any, bidder string) {
	ext, _ := imp["ext"].(map[string]any)
	if ext == nil {
		ext = map[string]any{}
	}
	params, ok := ext["bidder"]
	if !ok {
		params = map[string]any{}
	}
	delete(ext, "bidder")
	prebid, _ := ext["prebid"].(map[string]any)
	if prebid == nil {
		prebid = map[string]any{}
	}
	bidders, _ := prebid["bidder"].(map[string]any)
	if bidders == nil {
		bidders = map[string]any{}
	}
	bidders[bidder] = params
	prebid["bidder"] = bidders
	ext["prebid"] = prebid
	imp["ext"] = ext
}

// decodeObject decodes a JSON object keeping numbers as json.Number so that
// re-encoding preserves their text.
func decodeObject(raw []byte) (map[string]any, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var obj map[string]any
	if err := dec.Decode(&obj); err != nil {
		return nil, err
	}
	if obj == nil {
		return nil, fmt.Errorf("not a JSON object")
	}
	return obj, nil
}

// impIDs returns imp[].id of an OpenRTB request body, in order. Bodies that do
// not parse yield nil.
func impIDs(body []byte) []string {
	var req struct {
		Imp []struct {
			ID string `json:"id"`
		} `json:"imp"`
	}
	if err := json.Unmarshal(body, &req); err != nil {
		return nil
	}
	out := make([]string, 0, len(req.Imp))
	for _, imp := range req.Imp {
		out = append(out, imp.ID)
	}
	return out
}

// rawBody returns the raw JSON text as a text body, or nil when absent.
func rawBody(raw json.RawMessage) *parity.Body {
	if len(raw) == 0 {
		return nil
	}
	return parity.TextBody(string(raw))
}

// headerListOf flattens an http.Header sorted by name (fixture headers have no order).
func headerListOf(h http.Header) parity.HeaderList {
	out := parity.HeaderList{}
	for _, name := range sortedKeys(h) {
		for _, v := range h[name] {
			out = append(out, [2]string{name, v})
		}
	}
	return out
}
