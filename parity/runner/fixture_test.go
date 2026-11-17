package runner

import (
	"encoding/json"
	"net/http"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// fixtureCase is an adapter case with one expected request and bid.
func fixtureCase() *corpus.AdapterCase {
	return &corpus.AdapterCase{
		Requests: parity.AdapterRequestsInput{Build: parity.AdapterBuild{Bidder: "appnexus"}},
		Bids:     []parity.AdapterBidsInput{{}},
		Expect: corpus.AdapterExpect{
			AllowErrors: true,
			HTTPCalls: []corpus.AdapterHTTPCall{{Request: corpus.AdapterHTTPRequest{
				URI: "https://ib.adnxs.com/openrtb2", Body: json.RawMessage(`{"id":"r1","imp":[{"id":"i1"}],"tmax":100}`),
				Headers: http.Header{"Content-Type": {"application/json;charset=utf-8"}}, ImpIDs: []string{"i1"},
			}}},
			BidResponses: []corpus.AdapterBidResponse{{Currency: "USD", Bids: []corpus.AdapterExpectedBid{{Bid: json.RawMessage(`{"id":"b1","impid":"i1","price":1}`), Type: "banner"}}}},
		},
	}
}

func goodOutputs() (*parity.AdapterRequestsOutput, []*parity.AdapterBidsOutput) {
	body := `{"tmax":100.0,"imp":[{"id":"i1"}],"id":"r1"}`
	req := &parity.AdapterRequestsOutput{
		Requests: []parity.AdapterRequestData{{Method: "POST", URI: "https://ib.adnxs.com/openrtb2",
			Headers: parity.HeaderList{{"Content-Type", "application/json;charset=utf-8"}}, Body: parity.TextBody(body), ImpIDs: []string{"i1"}}},
		Errors: []parity.ErrorRecord{},
	}
	bids := []*parity.AdapterBidsOutput{{
		BidderResponse: &parity.AdapterBidderResponse{Currency: "USD", Bids: []parity.TypedBid{{Bid: json.RawMessage(`{"price":1.0,"impid":"i1","id":"b1"}`), Type: "banner"}}},
		Errors:         []parity.ErrorRecord{},
	}}
	return req, bids
}

func TestCheckFixture(t *testing.T) {
	tests := []struct {
		name    string
		mutate  func(ac *corpus.AdapterCase, req *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput)
		wantSub string
	}{
		{"passes with reordered members and 1 vs 1.0", func(*corpus.AdapterCase, *parity.AdapterRequestsOutput, []*parity.AdapterBidsOutput) {}, ""},
		{"wrong uri", func(_ *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Requests[0].URI = "https://other"
		}, "uri"},
		{"wrong body", func(_ *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Requests[0].Body = parity.TextBody(`{"id":"r2"}`)
		}, "json did not match"},
		{"wrong header", func(_ *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Requests[0].Headers = parity.HeaderList{{"Content-Type", "text/plain"}}
		}, "headers"},
		{"headers ignored when not expected", func(ac *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			ac.Expect.HTTPCalls[0].Request.Headers = nil
			req.Requests[0].Headers = parity.HeaderList{{"X-Other", "1"}}
		}, ""},
		{"no expected imp ids", func(ac *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			ac.Expect.HTTPCalls[0].Request.ImpIDs = nil
		}, "at least one imp ID"},
		{"request count", func(_ *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Requests = append(req.Requests, req.Requests[0])
		}, "wrong request count"},
		{"error count is fatal", func(_ *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Errors = []parity.ErrorRecord{{Message: "boom"}}
			req.Requests = nil
		}, "wrong error count"},
		{"regex error comparison", func(ac *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Errors = []parity.ErrorRecord{{Message: "bad input 42"}}
			ac.Expect.MakeRequestsErrors = []corpus.ExpectedError{{Value: `^bad input \d+$`, Comparison: "regex"}}
		}, ""},
		{"startswith error mismatch", func(ac *corpus.AdapterCase, req *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			req.Errors = []parity.ErrorRecord{{Message: "other"}}
			ac.Expect.MakeRequestsErrors = []corpus.ExpectedError{{Value: "bad", Comparison: "startswith"}}
		}, "wrong message"},
		{"exemplary must not expect errors", func(ac *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, _ []*parity.AdapterBidsOutput) {
			ac.Expect.AllowErrors = false
			ac.Expect.MakeBidsErrors = []corpus.ExpectedError{{Value: "x"}}
		}, "must not expect errors"},
		{"wrong currency", func(_ *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput) {
			bids[0].BidderResponse.Currency = "EUR"
		}, "currency"},
		{"wrong bid type", func(_ *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput) {
			bids[0].BidderResponse.Bids[0].Type = "video"
		}, ".type"},
		{"missing bidder response", func(_ *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput) {
			bids[0].BidderResponse = nil
		}, "len(bidResponses)"},
		{"unexpected fledge", func(_ *corpus.AdapterCase, _ *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput) {
			bids[0].BidderResponse.FledgeAuctionConfigs = json.RawMessage(`[{"impid":"i1"}]`)
		}, "unexpected fledgeauctionconfigs"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			ac := fixtureCase()
			req, bids := goodOutputs()
			tc.mutate(ac, req, bids)
			failures := CheckFixture(ac, req, bids)
			if tc.wantSub == "" {
				if len(failures) != 0 {
					t.Errorf("failures = %v", failures)
				}
				return
			}
			if !strings.Contains(strings.Join(failures, "\n"), tc.wantSub) {
				t.Errorf("failures = %v, want one containing %q", failures, tc.wantSub)
			}
		})
	}
}

func TestDiffJSON(t *testing.T) {
	tests := []struct {
		name     string
		actual   string
		expected string
		wantErr  bool
	}{
		{"both empty", "", "", false},
		{"one empty", "", `{}`, true},
		{"null equals empty object", `null`, `{}`, false},
		{"number spelling", `{"a":1}`, `{"a":1.0}`, false},
		{"array order matters", `{"a":[1,2]}`, `{"a":[2,1]}`, true},
		{"top-level array is an error", `[1]`, `[1]`, true},
		{"nested null is not an object", `{"a":null}`, `{"a":{}}`, true},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if err := diffJSON("x", []byte(tc.actual), []byte(tc.expected)); (err != nil) != tc.wantErr {
				t.Errorf("diffJSON err = %v, wantErr %v", err, tc.wantErr)
			}
		})
	}
}
