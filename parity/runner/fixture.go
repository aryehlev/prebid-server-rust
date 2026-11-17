package runner

import (
	"encoding/json"
	"fmt"
	"reflect"
	"regexp"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// CheckFixture compares implementation A's adapter endpoint outputs with an
// adapter fixture's own expectations, the way adapters/adapterstest does
// (test_json.go runSpec): error lists by count then literal, regex or
// startswith message; requests matched without assuming order by uri, JSON
// headers (when expected), sorted imp IDs and JSON body; bidder responses by
// currency, bid count, seat, type, JSON bid, JSON video and fledge configs.
// JSON comparison is structural over top-level objects, like gojsondiff.
// Where adapterstest stops a test (t.Fatalf), the check stops too. It returns
// the failures; none means the fixture passes.
func CheckFixture(ac *corpus.AdapterCase, req *parity.AdapterRequestsOutput, bids []*parity.AdapterBidsOutput) []string {
	exp := ac.Expect
	if !exp.AllowErrors && (len(exp.MakeRequestsErrors) > 0 || len(exp.MakeBidsErrors) > 0) {
		return []string{"exemplary spec must not expect errors"}
	}
	var failures []string
	fatal, errs := errorList("MakeRequests", req.Errors, exp.MakeRequestsErrors)
	failures = append(failures, errs...)
	if fatal {
		return failures
	}
	if len(req.Requests) != len(exp.HTTPCalls) {
		return append(failures, fmt.Sprintf("MakeRequests had wrong request count. Expected %d, got %d", len(exp.HTTPCalls), len(req.Requests)))
	}
	failures = append(failures, matchRequests(req.Requests, exp.HTTPCalls)...)
	return append(failures, checkBids(exp, bids)...)
}

// errorList mirrors assertErrorList; fatal is true on a count mismatch or an
// invalid comparison.
func errorList(what string, actual []parity.ErrorRecord, expected []corpus.ExpectedError) (bool, []string) {
	if len(actual) != len(expected) {
		msgs := make([]string, len(actual))
		for i, e := range actual {
			msgs[i] = e.Message
		}
		return true, []string{fmt.Sprintf("%s had wrong error count. Expected %d, got %d (%q)", what, len(expected), len(actual), msgs)}
	}
	var out []string
	for i, e := range actual {
		ok, err := errorMatches(expected[i], e.Message)
		if err != nil {
			return true, append(out, fmt.Sprintf("%s error[%d]: %v", what, i, err))
		}
		if !ok {
			out = append(out, fmt.Sprintf("%s error[%d] had wrong message. Expected %s %q, got %q", what, i, comparisonName(expected[i]), expected[i].Value, e.Message))
		}
	}
	return false, out
}

func comparisonName(e corpus.ExpectedError) string {
	if e.Comparison == "" {
		return "literal"
	}
	return e.Comparison
}

func errorMatches(exp corpus.ExpectedError, msg string) (bool, error) {
	switch exp.Comparison {
	case "", "literal":
		return exp.Value == msg, nil
	case "regex":
		matched, err := regexp.MatchString(exp.Value, msg)
		return err == nil && matched, nil
	case "startswith":
		return strings.HasPrefix(msg, exp.Value), nil
	}
	return false, fmt.Errorf("invalid comparison type %q", exp.Comparison)
}

// matchRequests mirrors assertMakeRequestsOutput: every expected request
// must match a distinct actual one and every actual one must be matched.
func matchRequests(actual []parity.AdapterRequestData, expected []corpus.AdapterHTTPCall) []string {
	var out []string
	actualMatched := map[int]bool{}
	for i := range expected {
		var lastErr error
		matched := false
		for j := range actual {
			if lastErr = diffRequest(fmt.Sprintf("httpRequest[%d]", i), &actual[j], &expected[i].Request); lastErr == nil {
				actualMatched[j], matched = true, true
				break
			}
		}
		if !matched {
			out = append(out, fmt.Sprintf("expected RequestData was not returned by MakeRequests: httpRequest[%d]: %v", i, lastErr))
		}
	}
	for j := range actual {
		if !actualMatched[j] {
			out = append(out, fmt.Sprintf("actual RequestData[%d] was not matched to a result", j))
		}
	}
	return out
}

// diffRequest mirrors diffHttpRequests.
func diffRequest(desc string, actual *parity.AdapterRequestData, exp *corpus.AdapterHTTPRequest) error {
	if exp.URI != actual.URI {
		return fmt.Errorf("%s.uri %q does not match expected %q", desc, actual.URI, exp.URI)
	}
	if exp.Headers != nil {
		if err := diffJSONValues(desc+".headers", headerMap(actual.Headers), exp.Headers); err != nil {
			return err
		}
	}
	if len(exp.ImpIDs) < 1 {
		return fmt.Errorf("expected.ImpIDs must contain at least one imp ID")
	}
	if !sameSortedStrings(exp.ImpIDs, actual.ImpIDs) {
		return fmt.Errorf("%s actual.ImpIDs %q do not match expected %q", desc, actual.ImpIDs, exp.ImpIDs)
	}
	body, err := actual.Body.WireBytes()
	if err != nil {
		return fmt.Errorf("%s body: %w", desc, err)
	}
	return diffJSON(desc+".body", body, exp.Body)
}

// headerMap is the http.Header form of a header list (names as recorded).
func headerMap(list parity.HeaderList) map[string][]string {
	if len(list) == 0 {
		return nil
	}
	out := map[string][]string{}
	for _, h := range list {
		out[h[0]] = append(out[h[0]], h[1])
	}
	return out
}

func sameSortedStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	as, bs := append([]string(nil), a...), append([]string(nil), b...)
	sort.Strings(as)
	sort.Strings(bs)
	return reflect.DeepEqual(as, bs)
}

// checkBids mirrors testMakeBidsImpl and assertMakeBidsOutput.
func checkBids(exp corpus.AdapterExpect, bids []*parity.AdapterBidsOutput) []string {
	var errs []parity.ErrorRecord
	var responses []*parity.AdapterBidderResponse
	for _, b := range bids {
		errs = append(errs, b.Errors...)
		if b.BidderResponse != nil {
			responses = append(responses, b.BidderResponse)
		}
	}
	fatal, out := errorList("MakeBids", errs, exp.MakeBidsErrors)
	if fatal {
		return out
	}
	if len(responses) != len(exp.BidResponses) {
		return append(out, fmt.Sprintf("MakeBids len(bidResponses) = %d vs len(spec.BidResponses) = %d", len(responses), len(exp.BidResponses)))
	}
	for i := range exp.BidResponses {
		out = append(out, checkBidResponse(fmt.Sprintf("bidResponse[%d]", i), responses[i], exp.BidResponses[i])...)
	}
	return out
}

func checkBidResponse(desc string, actual *parity.AdapterBidderResponse, exp corpus.AdapterBidResponse) []string {
	if exp.Currency != "" && exp.Currency != actual.Currency {
		return []string{fmt.Sprintf("%s: wrong currency. Got %s, expected %s", desc, actual.Currency, exp.Currency)}
	}
	if len(actual.Bids) != len(exp.Bids) {
		return []string{fmt.Sprintf("%s: wrong bid count. Got %d, expected %d", desc, len(actual.Bids), len(exp.Bids))}
	}
	var out []string
	for i := range actual.Bids {
		out = append(out, checkBid(fmt.Sprintf("%s typedBid[%d]", desc, i), actual.Bids[i], exp.Bids[i])...)
	}
	hasFledge := len(actual.FledgeAuctionConfigs) > 0 && string(actual.FledgeAuctionConfigs) != "null"
	switch {
	case exp.FledgeAuctionConfigs != nil && !hasFledge:
		out = append(out, desc+": expected fledgeauctionconfigs in bidderResponse")
	case exp.FledgeAuctionConfigs != nil:
		if !jsonEqual(actual.FledgeAuctionConfigs, exp.FledgeAuctionConfigs) {
			out = append(out, desc+": incorrect fledgeauctionconfig")
		}
	case hasFledge:
		out = append(out, desc+": unexpected fledgeauctionconfigs in bidderResponse")
	}
	return out
}

func checkBid(desc string, actual parity.TypedBid, exp corpus.AdapterExpectedBid) []string {
	var out []string
	if exp.Seat != actual.Seat {
		out = append(out, fmt.Sprintf("%s.seat %q does not match expected %q", desc, actual.Seat, exp.Seat))
	}
	if exp.Type != actual.Type {
		out = append(out, fmt.Sprintf("%s.type %q does not match expected %q", desc, actual.Type, exp.Type))
	}
	if len(actual.Bid) == 0 || string(actual.Bid) == "null" {
		out = append(out, desc+": bidders cannot return nil bids")
	} else if err := diffJSON(desc+".bid", actual.Bid, exp.Bid); err != nil {
		out = append(out, err.Error())
	}
	if exp.Video != nil {
		video := actual.Video
		if len(video) == 0 {
			video = json.RawMessage("null")
		}
		if err := diffJSON(desc+".video", video, exp.Video); err != nil {
			out = append(out, err.Error())
		}
	}
	return out
}

// diffJSON mirrors adapterstest diffJson: both empty is equal, one empty is
// a failure, otherwise both must decode as top-level JSON objects (null is an
// empty object) and be structurally equal with numbers as float64.
func diffJSON(desc string, actual, expected []byte) error {
	if len(actual) == 0 && len(expected) == 0 {
		return nil
	}
	if len(actual) == 0 || len(expected) == 0 {
		return fmt.Errorf("%s json diff failed. Expected %d bytes in body, but got %d", desc, len(expected), len(actual))
	}
	var a, e map[string]any
	if err := json.Unmarshal(actual, &a); err != nil {
		return fmt.Errorf("%s json diff failed. %v", desc, err)
	}
	if err := json.Unmarshal(expected, &e); err != nil {
		return fmt.Errorf("%s json diff failed. %v", desc, err)
	}
	if len(a) == 0 && len(e) == 0 {
		return nil
	}
	if !reflect.DeepEqual(a, e) {
		return fmt.Errorf("%s json did not match expected: got %s, want %s", desc, truncateString(string(actual), 300), truncateString(string(expected), 300))
	}
	return nil
}

// diffJSONValues marshals both values and compares them with diffJSON.
func diffJSONValues(desc string, actual, expected any) error {
	a, err := json.Marshal(actual)
	if err != nil {
		return fmt.Errorf("%s: marshalling actual: %w", desc, err)
	}
	e, err := json.Marshal(expected)
	if err != nil {
		return fmt.Errorf("%s: marshalling expected: %w", desc, err)
	}
	return diffJSON(desc, a, e)
}

// jsonEqual is assert.JSONEq: any JSON values, structurally equal.
func jsonEqual(a, b []byte) bool {
	var av, bv any
	if json.Unmarshal(a, &av) != nil || json.Unmarshal(b, &bv) != nil {
		return false
	}
	return reflect.DeepEqual(av, bv)
}

func truncateString(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return strings.ToValidUTF8(s[:n], "") + "..."
}
