package corpus

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/prebid/openrtb/v20/openrtb2"
	analyticsBuild "github.com/prebid/prebid-server/v4/analytics/build"
	"github.com/prebid/prebid-server/v4/config"
	endpointsOpenrtb2 "github.com/prebid/prebid-server/v4/endpoints/openrtb2"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/hooks"
	metricsConfig "github.com/prebid/prebid-server/v4/metrics/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/ortb"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/stored_requests/backends/empty_fetcher"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

var (
	testCorpusOnce sync.Once
	testCorpus     *Corpus
	testCorpusErr  error
)

// loadTestCorpus loads the repository corpus once per test binary.
func loadTestCorpus(t *testing.T) *Corpus {
	t.Helper()
	testCorpusOnce.Do(func() {
		repo, err := FindRepo(".")
		if err != nil {
			testCorpusErr = err
			return
		}
		testCorpus, testCorpusErr = LoadAll(repo)
	})
	require.NoError(t, testCorpusErr)
	return testCorpus
}

func testRepo(t *testing.T) *Repo {
	t.Helper()
	repo, err := FindRepo(".")
	require.NoError(t, err)
	return repo
}

func TestLoadAllCountsAndErrors(t *testing.T) {
	c := loadTestCorpus(t)
	for _, err := range c.Errors {
		t.Errorf("corpus error: %v", err)
	}
	stats := c.Stats()
	tests := []struct {
		name string
		got  int
		min  int
	}{
		{"adapter fixtures", stats.AdapterFixtures, 3627},
		{"sample requests", stats.SampleRequests, 251},
		{"exchangetest specs", stats.ExchangeSpecs, 101},
		{"adapter runs", stats.AdapterRuns, 266},
		{"params schemas", stats.ParamsSchemas, 280},
		{"seeds", stats.Seeds, 100},
		{"endpoint cases", stats.CasesByKind[KindEndpoint], 251},
		{"exchange cases", stats.CasesByKind[KindExchange], 101},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.GreaterOrEqual(t, tt.got, tt.min)
		})
	}
	assert.GreaterOrEqual(t, stats.CasesByKind[KindAdapter], stats.AdapterFixtures-1, "every fixture with a run yields an adapter case")
	assert.GreaterOrEqual(t, stats.CasesByKind[KindLifted], stats.AdapterFixtures-1, "every fixture with a bidder yields a lifted case")
}

func TestAdapterRunResolutionRate(t *testing.T) {
	c := loadTestCorpus(t)
	resolved := 0
	for _, run := range c.Adapters.Runs {
		if run.Resolved {
			resolved++
			continue
		}
		t.Logf("unresolved run %s: %v", run.ID(), run.Problems)
	}
	require.NotEmpty(t, c.Adapters.Runs)
	rate := float64(resolved) / float64(len(c.Adapters.Runs))
	assert.GreaterOrEqual(t, rate, 0.95, "statically resolved RunJSONBidderTest builds")
}

// TestAdapterRunsBuiltUnderAnotherBiddersName: a test that builds its own
// package's Builder under another bidder's name cannot be reproduced by a
// build by bidder name, so its run is unresolved; ordinary runs are not.
func TestAdapterRunsBuiltUnderAnotherBiddersName(t *testing.T) {
	c := loadTestCorpus(t)
	byPackage := map[string]*AdapterRun{}
	for _, run := range c.Adapters.Runs {
		byPackage[run.Package] = run
	}
	tests := []struct {
		pkg        string
		bidder     string
		unresolved bool
	}{
		{pkg: "adapters/akcelo", bidder: "aja", unresolved: true},
		{pkg: "adapters/smartyads", bidder: "smartadserver", unresolved: true},
		{pkg: "adapters/aja", bidder: "aja"},
		{pkg: "adapters/33across", bidder: "33across"},
	}
	for _, tt := range tests {
		t.Run(tt.pkg, func(t *testing.T) {
			run := byPackage[tt.pkg]
			require.NotNil(t, run)
			assert.Equal(t, tt.bidder, run.Bidder)
			assert.Equal(t, !tt.unresolved, run.Resolved, "%v", run.Problems)
			if tt.unresolved {
				assert.Contains(t, strings.Join(run.Problems, "\n"), "would run another adapter")
			}
		})
	}
}

func TestLoadBuilderPackages(t *testing.T) {
	repo := testRepo(t)
	consts, err := loadBidderConstants(repo)
	require.NoError(t, err)
	got, err := loadBuilderPackages(repo, consts)
	require.NoError(t, err)
	for bidder, dir := range map[string]string{
		"33across":           "adapters/33across",
		"akcelo":             "adapters/akcelo",
		"cadent_aperture_mx": "adapters/cadent_aperture_mx",
	} {
		assert.Equal(t, dir, got[bidder], bidder)
	}
}

func TestAdapterRunsNeedingFakes(t *testing.T) {
	c := loadTestCorpus(t)
	fakes, err := schema.Fakes()
	require.NoError(t, err)
	byBidder := map[string][]*AdapterRun{}
	for _, run := range c.Adapters.Runs {
		byBidder[run.Bidder] = append(byBidder[run.Bidder], run)
	}
	for _, seam := range fakes.Adapters {
		t.Run(seam.Bidder, func(t *testing.T) {
			runs := byBidder[seam.Bidder]
			require.NotEmpty(t, runs)
			for _, run := range runs {
				assert.True(t, run.NeedsFake, run.ID())
			}
		})
	}
	tests := []struct {
		bidder       string
		reason       string
		construction string
	}{
		{"appnexus", "sets a field of the bidder", ConstructBuilder},
		{"flipp", "passes the bidder to setFakeUUIDGenerator", ConstructBuilder},
		{"smaato", "sets a field of the bidder", ConstructBuilder},
		{"yieldlab", "constructs the adapter struct directly", "helper:newTestYieldlabBidder/" + ConstructLiteral},
	}
	for _, tt := range tests {
		t.Run(tt.bidder+" reason", func(t *testing.T) {
			runs := byBidder[tt.bidder]
			require.Len(t, runs, 1)
			assert.Equal(t, tt.construction, runs[0].Construction)
			assert.Contains(t, strings.Join(runs[0].FakeReasons, "\n"), tt.reason)
		})
	}
	plain := byBidder["33across"]
	require.Len(t, plain, 1)
	assert.False(t, plain[0].NeedsFake, "a plain Builder run needs no fake")
}

func TestAdapterRunBuildConfig(t *testing.T) {
	c := loadTestCorpus(t)
	find := func(file, fn string) *AdapterRun {
		for _, run := range c.Adapters.Runs {
			if run.TestFile == file && run.TestFunc == fn {
				return run
			}
		}
		return nil
	}
	server := parity.ServerConfig{ExternalURL: "http://hosturl.com", GvlID: 1, DataCenter: "2"}
	tests := []struct {
		file, fn string
		root     string
		bidder   string
		adapter  parity.AdapterConfig
		server   parity.ServerConfig
	}{
		{
			"adapters/gamoshi/gamoshi_test.go", "TestJsonSamplesWithConfiguredURI", "adapters/gamoshi/gamoshitest", "gamoshi",
			parity.AdapterConfig{Endpoint: "https://rtb.gamoshi.io"}, server,
		},
		{
			"adapters/gamoshi/gamoshi_test.go", "TestJsonSamplesWithHardcodedURI", "adapters/gamoshi/gamoshitest", "gamoshi",
			parity.AdapterConfig{}, server,
		},
		{
			"adapters/msft/msft_test.go", "TestJsonSamplesWithExtraInfo", "adapters/msft/test-extrainfo", "msft",
			parity.AdapterConfig{Endpoint: "http://any.url", ExtraInfo: `{"hb_source": 50, "hb_source_video": 60}`}, server,
		},
		{
			"adapters/adnuntius/adnuntius_test.go", "TestJsonSamples", "adapters/adnuntius/adnuntiustest", "adnuntius",
			parity.AdapterConfig{Endpoint: "http://whatever.url", ExtraInfo: "http://gdpr.url"}, server,
		},
	}
	for _, tt := range tests {
		t.Run(tt.file+":"+tt.fn, func(t *testing.T) {
			run := find(tt.file, tt.fn)
			require.NotNil(t, run)
			assert.True(t, run.Resolved, run.Problems)
			assert.Equal(t, tt.root, run.Root)
			assert.Equal(t, tt.bidder, run.Bidder)
			assert.Equal(t, tt.adapter, run.AdapterConfig)
			assert.Equal(t, tt.server, run.Server)
			build := run.Build()
			assert.Equal(t, parity.WrapRaw, build.Wrap)
			assert.Equal(t, tt.bidder, build.Bidder)
		})
	}
}

func TestRepoResolveRejectsEscapes(t *testing.T) {
	repo := testRepo(t)
	tests := []struct {
		name string
		rel  string
	}{
		{"parent", "../go.mod"},
		{"nested parent", "adapters/../../go.mod"},
		{"clean to unapproved root", "adapters/../parity/README.md"},
		{"absolute", "/etc/passwd"},
		{"empty", ""},
		{"dot", "."},
		{"unapproved root", "parity/README.md"},
		{"root prefix lookalike", "adapters2/x.json"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := repo.Resolve(tt.rel)
			assert.ErrorIs(t, err, ErrOutsideCorpus)
			_, err = repo.ReadFile(tt.rel)
			assert.ErrorIs(t, err, ErrOutsideCorpus)
		})
	}
	_, err := repo.Resolve("adapters/appnexus/appnexustest/exemplary/simple-banner.json")
	assert.NoError(t, err)
}

func TestRepoResolveRejectsSymlinkEscape(t *testing.T) {
	root := t.TempDir()
	outside := t.TempDir()
	require.NoError(t, os.WriteFile(filepath.Join(root, "go.mod"), []byte("module github.com/prebid/prebid-server/v4\n"), 0o600))
	require.NoError(t, os.WriteFile(filepath.Join(outside, "secret.json"), []byte(`{}`), 0o600))
	require.NoError(t, os.MkdirAll(filepath.Join(root, RootAdapters), 0o700))
	link := filepath.Join(root, RootAdapters, "escape.json")
	if err := os.Symlink(filepath.Join(outside, "secret.json"), link); err != nil {
		t.Skipf("symlinks unsupported: %v", err)
	}
	repo, err := OpenRepo(root)
	require.NoError(t, err)
	_, err = repo.ReadFile(RootAdapters + "/escape.json")
	assert.ErrorIs(t, err, ErrOutsideCorpus)
}

func TestOpenRepoRequiresPrebidModule(t *testing.T) {
	dir := t.TempDir()
	_, err := OpenRepo(dir)
	assert.Error(t, err)
	require.NoError(t, os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/other\n"), 0o600))
	_, err = OpenRepo(dir)
	assert.Error(t, err)
}

func TestLoadersRejectPathsOutsideTheirRoot(t *testing.T) {
	repo := testRepo(t)
	tests := []struct {
		name string
		load func() error
	}{
		{"adapter fixture outside adapters", func() error {
			_, err := LoadAdapterFixture(repo, "exchange/exchangetest/exemplary/x.json")
			return err
		}},
		{"adapter fixture traversal", func() error {
			_, err := LoadAdapterFixture(repo, "adapters/../../etc/exemplary/x.json")
			return err
		}},
		{"sample request outside root", func() error {
			_, err := LoadSampleRequest(repo, "adapters/appnexus/appnexustest/exemplary/simple-banner.json")
			return err
		}},
		{"exchange spec traversal", func() error {
			_, err := LoadExchangeSpec(repo, "exchange/exchangetest/../../go.mod")
			return err
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Error(t, tt.load())
		})
	}
}

func TestCaseDocumentsValidateAgainstSchemas(t *testing.T) {
	c := loadTestCorpus(t)
	checked := 0
	for _, cs := range c.Cases {
		docs := map[string]any{}
		if cs.Envelope != nil {
			docs["envelope"] = cs.Envelope
		}
		if cs.Adapter != nil {
			docs["requests"] = cs.Adapter.Requests
			for i, bids := range cs.Adapter.Bids {
				docs[fmt.Sprintf("bids[%d]", i)] = bids
			}
		}
		_, statusGap := unservableStatus(cs.Mocks.HTTP)
		if statusGap {
			assertMockStatusGap(t, cs)
		}
		for name, doc := range docs {
			raw, err := json.Marshal(doc)
			require.NoError(t, err, cs.ID)
			err = schema.Validate(raw)
			if cs.HasTag(TagContractGap) && !statusGap {
				// The tag must stay honest: the gap is real and is the bid type.
				assert.ErrorContains(t, err, ".type", cs.ID)
				continue
			}
			if err != nil {
				t.Errorf("%s %s: %v", cs.ID, name, err)
			}
			checked++
		}
	}
	assert.Greater(t, checked, 8000)
}

// assertMockStatusGap checks that a case with an unservable mocked status is
// tagged contract_gap and that one of its responses really has no encoding:
// status 0 is omitted (and reads as the default 200), others fail the schema.
func assertMockStatusGap(t *testing.T, cs Case) {
	t.Helper()
	assert.True(t, cs.HasTag(TagContractGap), cs.ID)
	rejected := false
	for _, m := range cs.Mocks.HTTP {
		if m.Response.Fail == nil && m.Response.Status == 0 {
			rejected = true
			continue
		}
		raw, err := json.Marshal(m.Response)
		require.NoError(t, err, cs.ID)
		if err := schema.ValidateDefinition(schema.FileEnvelope, "InlineResponse", raw); err != nil {
			assert.ErrorContains(t, err, "status", cs.ID)
			rejected = true
		}
	}
	assert.True(t, rejected, "%s: no mocked response is outside the schema", cs.ID)
}

func TestCaseProvenance(t *testing.T) {
	c := loadTestCorpus(t)
	for _, cs := range c.Cases {
		assert.NotEmpty(t, cs.FixturePath, cs.ID)
		assert.True(t, strings.HasPrefix(cs.ID, cs.FixturePath), cs.ID)
		if cs.Envelope != nil {
			assert.Equal(t, cs.ID, cs.Envelope.CaseID)
			assert.Equal(t, cs.Profile.Name(), cs.Envelope.Profile, cs.ID)
		}
	}
}

func TestAdapterCaseMirrorsAdaptersTest(t *testing.T) {
	c := loadTestCorpus(t)
	tests := []struct {
		dir         string
		allowErrors bool
		entryPoint  string
	}{
		{DirExemplary, false, ""},
		{DirSupplemental, true, ""},
		{DirAMP, true, entryPointAMP},
		{DirVideo, false, entryPointVideo},
		{DirVideoSupplemental, true, entryPointVideo},
	}
	for _, tt := range tests {
		t.Run(tt.dir, func(t *testing.T) {
			f := &AdapterFixture{Dir: tt.dir}
			assert.Equal(t, tt.allowErrors, f.AllowErrors())
			assert.Equal(t, tt.entryPoint, f.EntryPoint())
		})
	}
	var found bool
	for _, cs := range c.Cases {
		if cs.Kind != KindAdapter || !strings.HasPrefix(cs.ID, "adapters/appnexus/appnexustest/exemplary/simple-banner.json#") {
			continue
		}
		found = true
		require.Len(t, cs.Adapter.Bids, 1)
		bids := cs.Adapter.Bids[0]
		assert.Equal(t, http.MethodPost, bids.RequestData.Method)
		assert.Empty(t, bids.RequestData.Headers, "adapterstest passes no request headers to MakeBids")
		assert.Equal(t, 200, bids.Response.Status)
		assert.True(t, cs.HasTag(TagNeedsFake))
		assert.False(t, cs.HasTag(TagAllowErrors))
	}
	assert.True(t, found, "appnexus simple-banner adapter case")
}

// recordingExchange accepts every auction and records the request it got.
type recordingExchange struct {
	last *openrtb2.BidRequest
}

func (e *recordingExchange) HoldAuction(_ context.Context, r *exchange.AuctionRequest, _ *exchange.DebugLog) (*exchange.AuctionResponse, error) {
	e.last = r.BidRequestWrapper.BidRequest
	return &exchange.AuctionResponse{BidResponse: &openrtb2.BidResponse{ID: e.last.ID}}, nil
}

// newValidatingEndpoint builds /openrtb2/auction with the production request
// validator and params schemas and a recording exchange.
func newValidatingEndpoint(t *testing.T, repo *Repo) (func(*http.Request) *httptest.ResponseRecorder, *recordingExchange) {
	t.Helper()
	dir, err := repo.Resolve(RootBidderParams)
	require.NoError(t, err)
	params, err := openrtb_ext.NewBidderParamsValidator(dir)
	require.NoError(t, err)
	bidderMap := openrtb_ext.BuildBidderMap()
	ex := &recordingExchange{}
	handle, err := endpointsOpenrtb2.NewEndpoint(
		fakeUUID{},
		ex,
		ortb.NewRequestValidator(bidderMap, map[string]string{}, params),
		empty_fetcher.EmptyFetcher{},
		empty_fetcher.EmptyFetcher{},
		&config.Configuration{MaxRequestSize: 1 << 20},
		&metricsConfig.NilMetricsEngine{},
		analyticsBuild.New(&config.Analytics{}),
		map[string]string{},
		nil,
		bidderMap,
		empty_fetcher.EmptyFetcher{},
		hooks.EmptyPlanBuilder{},
		nil,
	)
	require.NoError(t, err)
	return func(req *http.Request) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		handle(rec, req, nil)
		return rec
	}, ex
}

type fakeUUID struct{}

func (fakeUUID) Generate() (string, error) { return "parity-uuid", nil }

// envelopeRequest turns an envelope's inbound request into an *http.Request.
func envelopeRequest(t *testing.T, env *parity.Envelope) *http.Request {
	t.Helper()
	body, err := env.Request.Body.DecodedBytes()
	require.NoError(t, err)
	target := env.Request.Path
	if env.Request.Query != "" {
		target += "?" + env.Request.Query
	}
	req := httptest.NewRequest(env.Request.Method, target, strings.NewReader(string(body)))
	for _, h := range env.Request.Headers {
		req.Header.Add(h[0], h[1])
	}
	req.RemoteAddr = env.Request.RemoteAddr
	return req
}

func TestLiftedAppnexusSimpleBannerPassesGoValidation(t *testing.T) {
	c := loadTestCorpus(t)
	repo := testRepo(t)
	cs, ok := c.Case("adapters/appnexus/appnexustest/exemplary/simple-banner.json#lift:auction")
	require.True(t, ok)
	assert.Equal(t, KindLifted, cs.Kind)
	require.NotEmpty(t, cs.Mocks.HTTP)
	assert.Equal(t, "appnexus", cs.Mocks.HTTP[0].Bidder)
	assert.Equal(t, 200, cs.Mocks.HTTP[0].Response.Status)

	serve, ex := newValidatingEndpoint(t, repo)
	rec := serve(envelopeRequest(t, cs.Envelope))
	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	require.NotNil(t, ex.last)
	require.NotEmpty(t, ex.last.Imp)
	assert.NotNil(t, ex.last.App, "the fixture's app is kept")
	assert.Nil(t, ex.last.Site, "no site is added next to an app")
	var impExt struct {
		Prebid struct {
			Bidder map[string]json.RawMessage `json:"bidder"`
		} `json:"prebid"`
		Bidder json.RawMessage `json:"bidder"`
	}
	require.NoError(t, json.Unmarshal(ex.last.Imp[0].Ext, &impExt))
	assert.Contains(t, impExt.Prebid.Bidder, "appnexus")
}

func TestInvalidParamsCaseFailsGoValidation(t *testing.T) {
	c := loadTestCorpus(t)
	repo := testRepo(t)
	serve, _ := newValidatingEndpoint(t, repo)
	var invalid, valid int
	for _, cs := range c.Cases {
		if cs.Kind != KindBidderParams || cs.Endpoint != EndpointAuction {
			continue
		}
		if cs.Expect.Status == http.StatusBadRequest {
			invalid++
			if invalid > 5 {
				continue
			}
			rec := serve(envelopeRequest(t, cs.Envelope))
			assert.Equal(t, http.StatusBadRequest, rec.Code, cs.ID)
			assert.Contains(t, rec.Body.String(), "failed validation", cs.ID)
		} else {
			valid++
		}
	}
	assert.Positive(t, invalid)
	assert.Positive(t, valid)
	get, ok := c.Case(RootBidderParams + "#get:bidders-params")
	require.True(t, ok)
	assert.Equal(t, http.MethodGet, get.Envelope.Request.Method)
	assert.Equal(t, EndpointBidderParams, get.Envelope.Request.Path)
}

func TestSampleRequestCases(t *testing.T) {
	c := loadTestCorpus(t)
	tests := []struct {
		id       string
		endpoint string
		method   string
		check    func(t *testing.T, cs *Case)
	}{
		{
			id:       "endpoints/openrtb2/sample-requests/amp/valid-supplementary/imp-with-stored-resp.json#amp",
			endpoint: EndpointAMP,
			method:   http.MethodGet,
			check: func(t *testing.T, cs *Case) {
				q := cs.Envelope.Request.Query
				assert.NotContains(t, q, "tag_id=101")
				tag := strings.TrimPrefix(q, "tag_id=")
				assert.Contains(t, cs.Stored.Requests, tag)
				assert.Contains(t, cs.Stored.Responses, "6d718149")
				assert.True(t, json.Valid(cs.Stored.Responses["6d718149"]))
				assert.Equal(t, boolPtr(true), cs.Profile.GDPREnabled)
			},
		},
		{
			id:       "endpoints/openrtb2/sample-requests/amp/consent-through-query/addtl-consent-through-query.json#amp",
			endpoint: EndpointAMP,
			method:   http.MethodGet,
			check: func(t *testing.T, cs *Case) {
				assert.True(t, strings.HasSuffix(cs.Envelope.Request.Query, "&addtl_consent=1~1.35.41.101"))
			},
		},
	}
	for _, tt := range tests {
		t.Run(tt.id, func(t *testing.T) {
			cs, ok := c.Case(tt.id)
			require.True(t, ok)
			assert.Equal(t, tt.endpoint, cs.Endpoint)
			assert.Equal(t, tt.method, cs.Envelope.Request.Method)
			tt.check(t, cs)
		})
	}
	var withMocks, withStored, video int
	for _, cs := range c.Cases {
		if cs.Kind != KindEndpoint {
			continue
		}
		if len(cs.Mocks.Bidders) > 0 {
			withMocks++
		}
		if !cs.Stored.IsEmpty() {
			withStored++
		}
		if cs.Endpoint == EndpointVideo {
			video++
		}
	}
	assert.Positive(t, withMocks, "mockBidders map to template params")
	assert.Positive(t, withStored, "stored data extracted from the Go tests")
	assert.Positive(t, video)
}

func TestInvalidStoredSamplesKeepTheirBytes(t *testing.T) {
	c := loadTestCorpus(t)
	var invalid int
	for _, cs := range c.Cases {
		if cs.HasTag(TagInvalidJSON) {
			invalid++
			body, err := cs.Envelope.Request.Body.DecodedBytes()
			require.NoError(t, err)
			assert.False(t, json.Valid(body), cs.ID)
		}
	}
	assert.Positive(t, invalid)
}

func TestEndpointStoredExtraction(t *testing.T) {
	stored, err := loadEndpointStored(testRepo(t))
	require.NoError(t, err)
	tests := []struct {
		name string
		m    map[string]json.RawMessage
		key  string
	}{
		{"auction accounts", stored.Auction.Accounts, "malformed_acct"},
		{"auction accounts", stored.Auction.Accounts, "disabled_acct"},
		{"video accounts", stored.Video.Accounts, "malformed_acct"},
	}
	for _, tt := range tests {
		t.Run(tt.name+"/"+tt.key, func(t *testing.T) {
			assert.Contains(t, tt.m, tt.key)
		})
	}
	assert.NotEmpty(t, stored.Auction.Requests)
	assert.NotEmpty(t, stored.Auction.Imps)
	assert.NotEmpty(t, stored.Video.Requests)
	assert.NotEmpty(t, stored.Video.Imps)
}

func TestExchangeCases(t *testing.T) {
	c := loadTestCorpus(t)
	var withErrors, withCookie bool
	for _, cs := range c.Cases {
		if cs.Kind != KindExchange {
			continue
		}
		require.Equal(t, parity.MockLevelBidderResp, cs.Envelope.Mock.Level, cs.ID)
		for _, mock := range cs.Envelope.Mock.BidderResponses {
			for _, e := range mock.Errors {
				withErrors = true
				assert.Equal(t, unknownErrorCode, e.Code)
			}
		}
		for _, h := range cs.Envelope.Request.Headers {
			if h[0] == "Cookie" {
				withCookie = true
				assert.True(t, strings.HasPrefix(h[1], "uids="))
			}
		}
		assert.Contains(t, cs.Profile.Tokens(), "gdpr", cs.ID)
	}
	assert.True(t, withErrors, "mock errors become code 999")
	assert.True(t, withCookie, "usersyncs become a uids cookie")
}

func TestUIDsCookie(t *testing.T) {
	tests := []struct {
		name    string
		syncs   map[string]string
		keys    map[string]string
		wantKey string
	}{
		{"syncer key", map[string]string{"appnexus": "123"}, map[string]string{"appnexus": "adnxs"}, "adnxs"},
		{"bidder name fallback", map[string]string{"foo": "abc"}, nil, "foo"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cookie, err := uidsCookie(tt.syncs, tt.keys)
			require.NoError(t, err)
			raw, err := base64.URLEncoding.DecodeString(cookie)
			require.NoError(t, err)
			var decoded struct {
				UIDs map[string]struct {
					UID string `json:"uid"`
				} `json:"tempUIDs"`
			}
			require.NoError(t, json.Unmarshal(raw, &decoded))
			require.Contains(t, decoded.UIDs, tt.wantKey)
		})
	}
	empty, err := uidsCookie(nil, nil)
	require.NoError(t, err)
	assert.Empty(t, empty)
}

func TestExchangeMockConversion(t *testing.T) {
	resp := ExchangeBidderResponse{
		SeatBids: []ExchangeSeatBid{
			{Seat: "appnexus", Currency: "USD", Bids: []ExchangeBid{{Bid: json.RawMessage(`{"id":"b1","impid":"i1","price":1}`), Type: "banner"}}},
			{Seat: "groupm", Currency: "EUR", Bids: []ExchangeBid{{Bid: json.RawMessage(`{"id":"b2","impid":"i1","price":2}`), Type: "video"}}},
		},
		Errors: []string{"bidder failed"},
	}
	mock, notes := resp.toMock("appnexus")
	require.Len(t, mock.Bids, 2)
	assert.Equal(t, "", mock.Bids[0].Seat, "the bidder's own seat is implicit")
	assert.Equal(t, "groupm", mock.Bids[1].Seat)
	assert.Equal(t, "USD", mock.Currency)
	assert.Equal(t, []parity.ErrorSpec{{Code: unknownErrorCode, Message: "bidder failed"}}, mock.Errors)
	assert.Len(t, notes, 1, "a second currency is reported")
}

func TestProfileOverrides(t *testing.T) {
	tests := []struct {
		name    string
		profile Profile
		want    map[string]any
	}{
		{"default", Profile{}, map[string]any{}},
		{
			"sample config",
			Profile{AccountRequired: true, BlockedApps: []string{"spam_app"}, DisabledAdapters: []string{"AppNexus"}},
			map[string]any{"account_required": true, "blocked_apps": []string{"spam_app"}, "adapters.appnexus.disabled": true},
		},
		{
			"exchange flags",
			Profile{GDPREnabled: boolPtr(false), LMTEnforce: boolPtr(true), FloorsEnabled: true, AccountDefaultBidLimit: 3, HostVendorID: 15},
			map[string]any{"gdpr.enabled": false, "lmt.enforce": true, "price_floors.enabled": true, "account_defaults.default_bid_limit": 3, "gdpr.host_vendor_id": 15},
		},
		{
			"json valued",
			Profile{HostSChainNode: exchangeTestHostSChain, BidderInfoOverrides: map[string]json.RawMessage{"appnexus": json.RawMessage(`{"openrtb":{"version":"2.6"}}`)}},
			map[string]any{
				"host_schain_node":          map[string]any{"asi": "pbshostcompany.com", "sid": "00001", "rid": "BidRequest", "hp": json.Number("1")},
				"adapters.appnexus.openrtb": map[string]any{"version": "2.6"},
			},
		},
		{"ortb version", Profile{ORTBVersions: map[string]string{"pubmatic": "2.6"}}, map[string]any{"adapters.pubmatic.openrtb.version": "2.6"}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := tt.profile.Overrides()
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
	_, err := Profile{AccountPrivacy: json.RawMessage(`{bad`)}.Overrides()
	assert.Error(t, err)
}

func TestProfileName(t *testing.T) {
	a := Profile{AccountRequired: true}
	b := Profile{AccountRequired: true, BlockedApps: []string{"x"}}
	assert.Equal(t, DefaultProfile, Profile{}.Name())
	assert.True(t, Profile{}.IsDefault())
	assert.Equal(t, a.Name(), Profile{AccountRequired: true}.Name(), "equal settings share a name")
	assert.NotEqual(t, a.Name(), b.Name())
	assert.True(t, strings.HasPrefix(b.Name(), "account-required+blocked-apps-"))
}

func TestMergeStored(t *testing.T) {
	a := Case{ID: "a", Stored: Stored{Requests: map[string]json.RawMessage{"1": json.RawMessage(`{"a":1}`)}}}
	b := Case{ID: "b", Stored: Stored{Requests: map[string]json.RawMessage{"1": json.RawMessage(`{"a":1}`), "2": json.RawMessage(`{}`)}}}
	conflict := Case{ID: "c", Stored: Stored{Requests: map[string]json.RawMessage{"1": json.RawMessage(`{"a":2}`)}}}
	merged, errs := MergeStored([]Case{a, b})
	assert.Empty(t, errs)
	assert.Len(t, merged.Requests, 2)
	_, errs = MergeStored([]Case{a, conflict})
	assert.Len(t, errs, 1)
}

func TestLiftRequest(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want string
	}{
		{
			"adds site and moves params",
			`{"id":"r","imp":[{"id":"1","banner":{},"ext":{"bidder":{"placementId":12345678901234567890}}}]}`,
			`{"id":"r","imp":[{"banner":{},"ext":{"prebid":{"bidder":{"appnexus":{"placementId":12345678901234567890}}}},"id":"1"}],"site":{"page":"https://parity.prebid.test/lifted"}}`,
		},
		{
			"keeps app and adds id",
			`{"app":{"id":"a"},"imp":[{"id":"1","ext":{"prebid":{"is_rewarded_inventory":1},"bidder":{}}}]}`,
			`{"app":{"id":"a"},"id":"parity-lifted-request","imp":[{"ext":{"prebid":{"bidder":{"appnexus":{}},"is_rewarded_inventory":1}},"id":"1"}]}`,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := liftRequest(json.RawMessage(tt.in), "appnexus")
			require.NoError(t, err)
			assert.JSONEq(t, tt.want, string(got))
			if strings.Contains(tt.in, "12345678901234567890") {
				assert.Contains(t, string(got), "12345678901234567890", "numbers keep their text")
			}
		})
	}
	_, err := liftRequest(json.RawMessage(`[]`), "appnexus")
	assert.Error(t, err)
}

// TestLiftedCasesTagUnservableStatuses: a mockResponse status no HTTP server
// can send makes the lifted case a contract gap; ordinary statuses do not.
func TestLiftedCasesTagUnservableStatuses(t *testing.T) {
	c := loadTestCorpus(t)
	tests := []struct {
		fixture string
		gap     bool
	}{
		{fixture: "adapters/logicad/logicadtest/supplemental/responseCode.json", gap: true},
		{fixture: "adapters/mediasquare/mediasquaretest/supplemental/status-not-200.json", gap: true},
		{fixture: "adapters/mediasquare/mediasquaretest/supplemental/status-400.json"},
	}
	for _, tt := range tests {
		t.Run(tt.fixture, func(t *testing.T) {
			cs, ok := c.Case(caseID(tt.fixture, "lift:auction"))
			require.True(t, ok)
			assert.Equal(t, tt.gap, cs.HasTag(TagContractGap), "%v", cs.Notes)
		})
	}
	fail := parity.OutboundErrTimeout
	_, gap := unservableStatus([]HTTPMock{{Response: parity.InlineResponse{Fail: &fail}}})
	assert.False(t, gap, "an injected failure carries no status")
}

func TestRewriteQueryParam(t *testing.T) {
	tests := []struct {
		raw, want string
		ok        bool
	}{
		{"tag_id=101", "tag_id=new", true},
		{"tag_id=101&consent_string&BOMhx", "tag_id=new&consent_string&BOMhx", true},
		{"a=1&tag_id=101&b=2", "a=1&tag_id=new&b=2", true},
		{"tag_id=", "tag_id=", false},
		{"account=1", "account=1", false},
	}
	for _, tt := range tests {
		t.Run(tt.raw, func(t *testing.T) {
			_, got, ok := rewriteQueryParam(tt.raw, ampTagParam, "new")
			assert.Equal(t, tt.ok, ok)
			assert.Equal(t, tt.want, got)
		})
	}
}

func TestSeeds(t *testing.T) {
	c := loadTestCorpus(t)
	byDomain := map[string]int{}
	for _, s := range c.Seeds {
		byDomain[s.Domain]++
		if s.Domain != SeedHookOutcomes {
			assert.True(t, json.Valid(s.Request), s.Path)
		}
	}
	for _, domain := range []string{SeedCache, SeedCustomCacheKey, SeedEventsCache, SeedImpCustomCacheKey, SeedFPD, SeedHookOutcomes} {
		assert.Positive(t, byDomain[domain], domain)
	}
}

func TestLoadAdapterFixtureWithoutBidRequest(t *testing.T) {
	f, err := LoadAdapterFixture(testRepo(t), "adapters/ucfunnel/ucfunneltest/exemplary/ucfunnel.json")
	require.NoError(t, err)
	assert.JSONEq(t, `{}`, string(f.Spec.BidRequest))
	assert.NotEmpty(t, f.Notes)
}

func TestCorpusErrorsAreWrapped(t *testing.T) {
	_, err := LoadSampleRequest(testRepo(t), "parity/x.json")
	assert.True(t, errors.Is(err, ErrOutsideCorpus))
}
