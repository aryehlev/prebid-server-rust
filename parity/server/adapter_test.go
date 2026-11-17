package server

import (
	"context"
	"encoding/json"
	"net/http"
	"regexp"
	"sort"
	"strings"
	"sync"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// replayFixtures are replayed through the adapter endpoints and checked against
// their own expectations with adapterstest semantics. Two per design bidder
// (appnexus fixtures avoid the ad pod ID, the adapter's random field), plus
// adagio for expected request headers.
var replayFixtures = []string{
	"adapters/appnexus/appnexustest/exemplary/simple-banner.json",
	"adapters/appnexus/appnexustest/supplemental/status-400.json",
	"adapters/rubicon/rubicontest/exemplary/simple-banner.json",
	"adapters/rubicon/rubicontest/exemplary/bidonmultiformat.json",
	"adapters/ix/ixtest/exemplary/multi-imp-requests.json",
	"adapters/ix/ixtest/exemplary/fledge.json",
	"adapters/openx/openxtest/exemplary/simple-banner.json",
	"adapters/openx/openxtest/exemplary/simple-video.json",
	"adapters/pubmatic/pubmatictest/exemplary/banner.json",
	"adapters/pubmatic/pubmatictest/supplemental/invalidparam.json",
	"adapters/adagio/adagiotest/exemplary/banner-web.json",
}

var (
	adapterCorpusOnce sync.Once
	adapterCorpus     *corpus.AdapterCorpus
	adapterCorpusErr  error
)

// loadAdapterCorpus loads the adapter fixtures and their test runs once.
func loadAdapterCorpus(t *testing.T) *corpus.AdapterCorpus {
	t.Helper()
	adapterCorpusOnce.Do(func() {
		repo, err := corpus.FindRepo(".")
		if err != nil {
			adapterCorpusErr = err
			return
		}
		adapterCorpus, adapterCorpusErr = corpus.LoadAdapterCorpus(repo)
	})
	require.NoError(t, adapterCorpusErr)
	return adapterCorpus
}

// fixtureCases returns the adapter cases of the given fixtures, one per Go test
// run covering each fixture.
func fixtureCases(t *testing.T, paths []string) []corpus.Case {
	t.Helper()
	byPath := map[string]*corpus.AdapterFixture{}
	for _, f := range loadAdapterCorpus(t).Fixtures {
		byPath[f.Path] = f
	}
	sub := &corpus.AdapterCorpus{}
	for _, p := range paths {
		f := byPath[p]
		require.NotNil(t, f, p)
		require.NotEmpty(t, f.Runs, "%s has a covering RunJSONBidderTest run", p)
		sub.Fixtures = append(sub.Fixtures, f)
	}
	cases, errs := sub.AdapterCases()
	require.Empty(t, errs)
	require.NotEmpty(t, cases)
	return cases
}

// postAdapter posts in to the adapter route and decodes the validated output.
func postAdapter(t *testing.T, h http.Handler, bidder, op string, in, out any) {
	t.Helper()
	rec := call(t, h, http.MethodPost, AdapterPrefix+bidder+"/"+op, mustJSON(t, in))
	decodeDoc(t, rec, http.StatusOK, out)
}

func adapterServer(t *testing.T) http.Handler {
	t.Helper()
	return newServer(wiring.Deps{BidderInfos: staticBidderInfos(t)}, http.NotFoundHandler())
}

func TestAdapterEndpointsReplayFixtures(t *testing.T) {
	h := adapterServer(t)
	for _, cs := range fixtureCases(t, replayFixtures) {
		t.Run(cs.ID, func(t *testing.T) {
			require.False(t, cs.HasTag(corpus.TagUnresolvedBuild), cs.Notes)
			ac := cs.Adapter
			bidder := ac.Requests.Build.Bidder

			var reqOut parity.AdapterRequestsOutput
			postAdapter(t, h, bidder, adapterOpRequests, ac.Requests, &reqOut)
			assert.Equal(t, ac.Requests.CaseID, reqOut.CaseID)
			assert.Equal(t, bidder, reqOut.Bidder)
			assertErrorRecords(t, "MakeRequests", reqOut.Errors, ac.Expect.MakeRequestsErrors)
			assertRequests(t, reqOut.Requests, ac.Expect.HTTPCalls)

			var responses []*parity.AdapterBidderResponse
			bidErrs := []parity.ErrorRecord{}
			for _, in := range ac.Bids {
				var bidsOut parity.AdapterBidsOutput
				postAdapter(t, h, bidder, adapterOpBids, in, &bidsOut)
				bidErrs = append(bidErrs, bidsOut.Errors...)
				if bidsOut.BidderResponse != nil {
					responses = append(responses, bidsOut.BidderResponse)
				}
			}
			assertErrorRecords(t, "MakeBids", bidErrs, ac.Expect.MakeBidsErrors)
			require.Len(t, responses, len(ac.Expect.BidResponses))
			for i, want := range ac.Expect.BidResponses {
				assertBidderResponse(t, responses[i], want)
			}
		})
	}
}

// assertErrorRecords is adapterstest assertErrorList: same count, then each
// message compared literally, by regex or by prefix.
func assertErrorRecords(t *testing.T, what string, got []parity.ErrorRecord, want []corpus.ExpectedError) {
	t.Helper()
	require.Len(t, got, len(want), "%s errors: %v", what, got)
	for i, w := range want {
		msg := got[i].Message
		switch w.Comparison {
		case "", "literal":
			assert.Equal(t, w.Value, msg, "%s error[%d]", what, i)
		case "regex":
			assert.Regexp(t, regexp.MustCompile(w.Value), msg, "%s error[%d]", what, i)
		case "startswith":
			assert.True(t, strings.HasPrefix(msg, w.Value), "%s error[%d] %q starts with %q", what, i, msg, w.Value)
		default:
			t.Fatalf("invalid comparison type %q", w.Comparison)
		}
	}
}

// assertRequests is adapterstest assertMakeRequestsOutput: every expected
// request matches exactly one actual request, in any order.
func assertRequests(t *testing.T, got []parity.AdapterRequestData, want []corpus.AdapterHTTPCall) {
	t.Helper()
	require.Len(t, got, len(want), "MakeRequests request count")
	used := make([]bool, len(got))
	for i, call := range want {
		matched := false
		for j := range got {
			if !used[j] && requestMatches(t, got[j], call.Request) {
				used[j], matched = true, true
				break
			}
		}
		assert.True(t, matched, "expected httpRequest[%d] (uri %s) was not returned", i, call.Request.URI)
	}
}

// requestMatches is adapterstest diffHttpRequests: uri, headers when the
// fixture has them (JSON of the http.Header, names as the adapter set them),
// imp IDs ignoring order and the body as JSON.
func requestMatches(t *testing.T, got parity.AdapterRequestData, want corpus.AdapterHTTPRequest) bool {
	t.Helper()
	if got.URI != want.URI {
		return false
	}
	if want.Headers != nil && !jsonEqual(t, mustJSON(t, headerOrNil(got.Headers)), mustJSON(t, want.Headers)) {
		return false
	}
	if !sameStrings(got.ImpIDs, want.ImpIDs) {
		return false
	}
	body, err := got.Body.WireBytes()
	require.NoError(t, err)
	if len(body) == 0 || len(want.Body) == 0 {
		return len(body) == len(want.Body)
	}
	return jsonEqual(t, body, want.Body)
}

// assertBidderResponse is adapterstest assertMakeBidsOutput.
func assertBidderResponse(t *testing.T, got *parity.AdapterBidderResponse, want corpus.AdapterBidResponse) {
	t.Helper()
	if want.Currency != "" {
		assert.Equal(t, want.Currency, got.Currency)
	}
	require.Len(t, got.Bids, len(want.Bids))
	for i, w := range want.Bids {
		bid := got.Bids[i]
		assert.Equal(t, w.Seat, bid.Seat, "bids[%d].seat", i)
		assert.Equal(t, w.Type, bid.Type, "bids[%d].type", i)
		assert.JSONEq(t, string(w.Bid), string(bid.Bid), "bids[%d].bid", i)
		if w.Video != nil {
			assert.JSONEq(t, string(w.Video), string(orNull(bid.Video)), "bids[%d].video", i)
		}
	}
	if want.FledgeAuctionConfigs != nil {
		require.NotEmpty(t, got.FledgeAuctionConfigs)
		assert.JSONEq(t, string(want.FledgeAuctionConfigs), string(got.FledgeAuctionConfigs))
	} else {
		assert.Empty(t, got.FledgeAuctionConfigs)
	}
}

func jsonEqual(t *testing.T, a, b []byte) bool {
	t.Helper()
	var x, y any
	if json.Unmarshal(a, &x) != nil || json.Unmarshal(b, &y) != nil {
		return false
	}
	return assert.ObjectsAreEqual(x, y)
}

func sameStrings(a, b []string) bool {
	a, b = append([]string(nil), a...), append([]string(nil), b...)
	sort.Strings(a)
	sort.Strings(b)
	return assert.ObjectsAreEqual(a, b) || (len(a) == 0 && len(b) == 0)
}

func orNull(raw json.RawMessage) json.RawMessage {
	if len(raw) == 0 {
		return json.RawMessage("null")
	}
	return raw
}

// simpleBannerCase is the appnexus simple-banner adapter case, the base of the
// error-path tests.
func simpleBannerCase(t *testing.T) *corpus.AdapterCase {
	t.Helper()
	cases := fixtureCases(t, []string{"adapters/appnexus/appnexustest/exemplary/simple-banner.json"})
	return cases[0].Adapter
}

func TestAdapterRequestsBuildDefaults(t *testing.T) {
	h := adapterServer(t)
	base := simpleBannerCase(t)
	tests := []struct {
		name  string
		build parity.AdapterBuild
	}{
		{name: "adapter config from the bidder info", build: parity.AdapterBuild{Bidder: "appnexus", Wrap: parity.WrapRaw}},
		{name: "info aware wrapper", build: parity.AdapterBuild{Bidder: "appnexus", Wrap: parity.WrapInfoAware}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			in := base.Requests
			in.Build = tt.build
			var out parity.AdapterRequestsOutput
			postAdapter(t, h, "appnexus", adapterOpRequests, in, &out)
			require.Len(t, out.Requests, 1)
			assert.True(t, strings.HasPrefix(out.Requests[0].URI, "http://ib.adnxs.com/openrtb2"), out.Requests[0].URI)
			assert.Equal(t, []parity.ErrorRecord{}, out.Errors)
		})
	}
}

func TestAdapterEndpointsResetFakes(t *testing.T) {
	h := adapterServer(t)
	in := simpleBannerCase(t).Requests
	in.Fakes = &parity.FakeValues{NowUnixMS: 1234}
	var out parity.AdapterRequestsOutput
	postAdapter(t, h, "appnexus", adapterOpRequests, in, &out)
	assert.Equal(t, fakes.Defaults(), fakes.Active().Values())
	assert.Nil(t, trace.Current())
}

func TestAdapterEndpointErrors(t *testing.T) {
	h := adapterServer(t)
	base := simpleBannerCase(t)
	requests := func(edit func(*parity.AdapterRequestsInput)) []byte {
		in := base.Requests
		edit(&in)
		return mustJSON(t, in)
	}
	bids := func(edit func(*parity.AdapterBidsInput)) []byte {
		in := base.Bids[0]
		edit(&in)
		return mustJSON(t, in)
	}
	tests := []struct {
		name   string
		method string
		path   string
		body   []byte
		status int
		want   string
	}{
		{name: "unknown operation", method: http.MethodPost, path: AdapterPrefix + "appnexus/bid", status: http.StatusNotFound, want: "parity: no route"},
		{name: "missing operation", method: http.MethodPost, path: AdapterPrefix + "appnexus", status: http.StatusNotFound, want: "parity: no route"},
		{name: "GET is not allowed", method: http.MethodGet, path: AdapterPrefix + "appnexus/requests", status: http.StatusMethodNotAllowed, want: "does not accept GET"},
		{name: "route bidder mismatch", method: http.MethodPost, path: AdapterPrefix + "rubicon/requests", body: requests(func(*parity.AdapterRequestsInput) {}), status: http.StatusBadRequest, want: "does not match the route bidder"},
		{name: "unknown bidder", method: http.MethodPost, path: AdapterPrefix + "nosuchbidder/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.Build.Bidder = "nosuchbidder" }), status: http.StatusBadRequest, want: `unknown bidder "nosuchbidder"`},
		{name: "bad wrap", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.Build.Wrap = "wrapped" }), status: http.StatusBadRequest, want: "build.wrap"},
		{name: "bad media type", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.ExtraInfo.PreferredMediaType = "print" }), status: http.StatusBadRequest, want: "preferred_media_type"},
		{name: "wrong schema", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.Schema = parity.SchemaAdapterBidsInput }), status: http.StatusBadRequest, want: "schema must be"},
		{name: "bids document on the requests route", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: bids(func(*parity.AdapterBidsInput) {}), status: http.StatusBadRequest, want: `unknown field "request_data"`},
		{name: "missing request", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.Request = nil }), status: http.StatusBadRequest, want: "request is required"},
		{name: "malformed request", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: requests(func(in *parity.AdapterRequestsInput) { in.Request = json.RawMessage(`{"imp":"x"}`) }), status: http.StatusBadRequest, want: "request:"},
		{name: "bad response body", method: http.MethodPost, path: AdapterPrefix + "appnexus/bids", body: bids(func(in *parity.AdapterBidsInput) { in.Response.Body = &parity.Body{B64: strPtr("%%%")} }), status: http.StatusBadRequest, want: "response.body"},
		{name: "unknown member", method: http.MethodPost, path: AdapterPrefix + "appnexus/requests", body: []byte(`{"schema":"x","extra":1}`), status: http.StatusBadRequest, want: "unknown field"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assertError(t, call(t, h, tt.method, tt.path, tt.body), tt.status, tt.want)
		})
	}
}

func strPtr(s string) *string { return &s }

func TestRunAdapterRecoversPanics(t *testing.T) {
	err := runAdapter(context.Background(), &parity.FakeValues{NowUnixMS: 99}, "appnexus MakeBids", func() { panic("boom") })
	require.Error(t, err)
	assert.Equal(t, "parity: bidder appnexus MakeBids panicked: boom", err.Error())
	assert.Equal(t, fakes.Defaults(), fakes.Active().Values(), "the fakes are reset after a panic")

	ran := false
	require.NoError(t, runAdapter(context.Background(), nil, "next", func() { ran = true }), "the exec lock is released after a panic")
	assert.True(t, ran)
}

func TestAdapterPanicIsAServerError(t *testing.T) {
	rec := call(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		writeFailure(w, runAdapter(r.Context(), nil, "x MakeRequests", func() { panic("boom") }))
	}), http.MethodPost, "/", nil)
	assertError(t, rec, http.StatusInternalServerError, "panicked: boom")
}
