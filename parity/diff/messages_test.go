package diff

import (
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/xeipuuv/gojsonschema"
)

// schemaErrorItems validates doc against schema with gojsonschema and returns
// each result error as openrtb_ext/bidders.go:865-867 writes it.
func schemaErrorItems(t *testing.T, schema, doc string) []string {
	t.Helper()
	s, err := gojsonschema.NewSchema(gojsonschema.NewStringLoader(schema))
	if err != nil {
		t.Fatalf("NewSchema: %v", err)
	}
	res, err := s.Validate(gojsonschema.NewStringLoader(doc))
	if err != nil {
		t.Fatalf("Validate: %v", err)
	}
	items := make([]string, 0, len(res.Errors()))
	for _, e := range res.Errors() {
		items = append(items, e.String())
	}
	return items
}

const paramsPrefix = "request.imp[0].ext.prebid.bidder.beachfront failed validation.\n"

const paramsSchema = `{"type":"object","properties":{
	"appId":{"type":"string"},"bidfloor":{"type":"number","minimum":0.01},
	"sizes":{"type":"array","minItems":1},"kind":{"type":"string","enum":["banner","video"]},
	"code":{"type":"string","pattern":"^[a-z]+$","minLength":3},"deps":{"type":"integer","maximum":5}},
	"required":["appId"],"additionalProperties":false}`

func TestSortBidderParamsErrorsRealMessages(t *testing.T) {
	tests := []struct {
		name string
		doc  string
	}{
		{"two types", `{"appId":{},"bidfloor":"1"}`},
		{"required and enum", `{"kind":"audio","bidfloor":0}`},
		{"many keywords", `{"appId":1,"bidfloor":-1,"sizes":[],"kind":"x","code":"A","deps":9,"extra":true}`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			items := schemaErrorItems(t, paramsSchema, tt.doc)
			if len(items) < 2 {
				t.Fatalf("want several errors, got %q", items)
			}
			want := sortedParams(t, paramsPrefix+strings.Join(items, ""))
			for i := range items {
				rotated := append(append([]string(nil), items[i:]...), items[:i]...)
				reversed := make([]string, len(rotated))
				for j, it := range rotated {
					reversed[len(rotated)-1-j] = it
				}
				for _, order := range [][]string{rotated, reversed} {
					if got := sortedParams(t, paramsPrefix+strings.Join(order, "")); got != want {
						t.Fatalf("order %q:\n got %q\nwant %q", order, got, want)
					}
				}
			}
		})
	}
}

// sortedParams sorts the bidder-params errors of an exact string value.
func sortedParams(t *testing.T, s string) string {
	t.Helper()
	n, err := parseExact([]byte(jsonQuote(s)))
	if err != nil {
		t.Fatalf("parseExact: %v", err)
	}
	sortBidderParamsErrors(n)
	if n.raw != jsonQuote(n.str) {
		t.Fatalf("raw %s does not spell %q", n.raw, n.str)
	}
	return n.str
}

func TestSortBidderParamsErrors(t *testing.T) {
	const b = "bidfloor: Invalid type. Expected: number, given: string"
	const a = "appId: Invalid type. Expected: string, given: object"
	tests := []struct {
		name string
		in   string
		want string
	}{
		{"sorted by field", paramsPrefix + b + a, paramsPrefix + a + b},
		{"already sorted", paramsPrefix + a + b, paramsPrefix + a + b},
		{"http response text keeps its frame", "Invalid request: " + paramsPrefix + b + a + "\n", "Invalid request: " + paramsPrefix + a + b + "\n"},
		{"one item", paramsPrefix + b, paramsPrefix + b},
		{"unknown description is left as recorded", paramsPrefix + b + "appId: something else", paramsPrefix + b + "appId: something else"},
		{"no prefix", b + a, b + a},
		{"ambiguous split is left as recorded", paramsPrefix + "x: Has a dependency on fooy: Has a dependency on bar",
			paramsPrefix + "x: Has a dependency on fooy: Has a dependency on bar"},
		{"two messages", paramsPrefix + b + a + "\n" + paramsPrefix + b + a, paramsPrefix + a + b + "\n" + paramsPrefix + a + b},
		{"quoted patterns", paramsPrefix + "token: Does not match pattern '[0-9|a-z|A-Z]{16}'pubid: Does not match pattern '^[0-9]+$'",
			paramsPrefix + "pubid: Does not match pattern '^[0-9]+$'token: Does not match pattern '[0-9|a-z|A-Z]{16}'"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := sortedParams(t, tt.in); got != tt.want {
				t.Fatalf("got %q\nwant %q", got, tt.want)
			}
		})
	}
}

func TestRewriteTextKeepsSpelling(t *testing.T) {
	tests := []struct {
		name    string
		raw     string
		pieces  []textPiece
		wantStr string
		wantRaw string
	}{
		{"spans swap with escapes kept", `"a\u00e9b\ncd"`, []textPiece{spanPiece(5, 6), spanPiece(0, 5)}, "ca\u00e9b\n", `"ca\u00e9b\n"`},
		{"literal piece", `"x\/y"`, []textPiece{spanPiece(0, 2), literalPiece("<p>"), spanPiece(2, 3)}, "x/<p>y", `"x\/<p>y"`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			n, err := parseExact([]byte(tt.raw))
			if err != nil {
				t.Fatalf("parseExact: %v", err)
			}
			rewriteText(n, tt.pieces)
			if n.str != tt.wantStr || n.raw != tt.wantRaw {
				t.Fatalf("got %q %s, want %q %s", n.str, n.raw, tt.wantStr, tt.wantRaw)
			}
		})
	}
}

// storedView is a view whose stored.responses stage holds the given stored
// auction responses by imp.
func storedView(t *testing.T, auction string) *view {
	t.Helper()
	tr := newTrace(parity.ImplGo)
	put(tr, parity.StageStoredResponses, parity.UnkeyedStage, `{"auction":`+auction+`,"bid":{},"replace_imp_id":{}}`)
	return buildView(tr, "go")
}

func storedBodyJSON(seatBids string) string { return `{"json":` + seatBids + `}` }

func TestStoredAuctionFailures(t *testing.T) {
	untyped := storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","price":1}]}]`)
	typed := storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","price":1,"ext":{"prebid":{"type":"banner"}}}]}]`)
	tests := []struct {
		name    string
		auction string
		want    []string
	}{
		{"untyped and typed", `{"i1":` + untyped + `,"i2":` + typed + `}`, []string{"i1"}},
		{"two untyped", `{"i1":` + untyped + `,"i2":` + untyped + `}`, []string{"i1", "i2"}},
		{"mtype set", `{"i1":` + storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","mtype":2}]}]`) + `}`, nil},
		{"mtype out of range", `{"i1":` + storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","mtype":7}]}]`) + `}`, []string{"i1"}},
		{"mtype not int8 fails to unmarshal first", `{"i1":` + storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","mtype":300}]}]`) + `}`, nil},
		{"text body", `{"i1":{"text":"[{\"seat\":\"a\",\"bid\":[{\"id\":\"b\"}]}]"}}`, []string{"i1"}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := storedAuctionFailures(storedView(t, tt.auction))
			if len(got) != len(tt.want) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
			for _, imp := range tt.want {
				if !got[imp] {
					t.Fatalf("got %v, want %v", got, tt.want)
				}
			}
		})
	}
}

func TestMaskStoredAuctionImp(t *testing.T) {
	failing := map[string]bool{"i1": true, "i2": true}
	tests := []struct {
		name string
		in   string
		want string
	}{
		{"candidate", `Failed to parse bid mediatype for impression "i2"`, `Failed to parse bid mediatype for impression "<stored-auction-imp>"`},
		{"mType variant", `Failed to parse bid mType for impression "i1"`, `Failed to parse bid mType for impression "<stored-auction-imp>"`},
		{"not a candidate", `Failed to parse bid mediatype for impression "i3"`, `Failed to parse bid mediatype for impression "i3"`},
		{"other text", `impression "i1"`, `impression "i1"`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			n := newString(tt.in)
			maskStoredAuctionImp(n, failing)
			if n.str != tt.want || n.raw != jsonQuote(tt.want) {
				t.Fatalf("got %q %s, want %q", n.str, n.raw, tt.want)
			}
		})
	}
}

// storedAuctionPair reports the error of a different failing imp on each
// side, as two Go processes do.
func storedAuctionPair(goImp, rustImp string, failing int) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		untyped := storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","price":1}]}]`)
		typed := storedBodyJSON(`[{"seat":"a","bid":[{"id":"b","price":1,"mtype":1}]}]`)
		second := typed
		if failing > 1 {
			second = untyped
		}
		stored := `{"auction":{"i1":` + untyped + `,"i2":` + second + `},"bid":{},"replace_imp_id":{}}`
		for _, side := range []struct {
			tr  *parity.Trace
			imp string
		}{{g, goImp}, {r, rustImp}} {
			put(side.tr, parity.StageStoredResponses, parity.UnkeyedStage, stored)
			side.tr.HTTPResponse = textResponse(500, `Critical error while running the auction: Failed to parse bid mediatype for impression "`+side.imp+`"`)
		}
		return Case{CaseID: "rule"}, g, r
	}
}

// responseCurPair holds two seats with bids in different currencies, and a
// response cur that each side took from another seat, as buildBidResponse's
// map iteration gives.
func responseCurPair(goCur, rustCur string) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		for _, side := range []struct {
			tr  *parity.Trace
			cur string
		}{{g, goCur}, {r, rustCur}} {
			put(side.tr, parity.StageBidderResult, "nobid", `{"seat_bids":[{"seat":"nobid","currency":"USD","bids":[{"bid":{"id":"b1","impid":"i1"}}]}]}`)
			put(side.tr, parity.StageBidderResult, "seedingAlliance", `{"seat_bids":[{"seat":"seedingAlliance","currency":"EUR","bids":[{"bid":{"id":"b2","impid":"i1"}}]}]}`)
			put(side.tr, parity.StageExchangeResponse, parity.UnkeyedStage, `{"bid_response":{"id":"r1","cur":"`+side.cur+`"}}`)
		}
		return Case{CaseID: "rule"}, g, r
	}
}

// adjustedCallsPair holds a bidder whose two requests come out of MakeRequests
// in another order on each side (a map-ordered adapter), so the adapter.bids
// keys and the bids.adjusted call that names one of them differ.
func adjustedCallsPair() (Case, *parity.Trace, *parity.Trace) {
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	first := `{"request_data":{"uri":"https://m.example/?p=a","body":{"text":"{}"}},"bidder_response":{"currency":"USD","bids":[{"bid":{"id":"x","impid":"i1"},"type":"banner"}]}}`
	second := `{"request_data":{"uri":"https://m.example/?p=b","body":{"text":"{}"}},"bidder_response":{"currency":"USD","bids":[]}}`
	put(g, parity.StageAdapterBids, "m#0", first)
	put(g, parity.StageAdapterBids, "m#1", second)
	put(r, parity.StageAdapterBids, "m#0", second)
	put(r, parity.StageAdapterBids, "m#1", first)
	put(g, parity.StageBidsAdjusted, "m", `{"bids":[{"call":"m#0","bidder":"m","bid_id":"x","imp_id":"i1"}],"conversion_errors":[]}`)
	put(r, parity.StageBidsAdjusted, "m", `{"bids":[{"call":"m#1","bidder":"m","bid_id":"x","imp_id":"i1"}],"conversion_errors":[]}`)
	return Case{CaseID: "rule"}, g, r
}

// ampBids are two AMP winners from different seats, both cached, and a losing
// bid without a cache ID that the AMP endpoint does not merge.
const ampBids = `{"bid_response":{"id":"r1","seatbid":[` +
	`{"seat":"mabidder","bid":[{"id":"b1","impid":"1","ext":{"prebid":{"targeting":{"hb_bidder":"mabidder","hb_pb":"2.70","hb_cache_id":"c-1","hb_deal":"d1"}}}}]},` +
	`{"seat":"showheroes-bs","bid":[{"id":"b2","impid":"2","ext":{"prebid":{"targeting":{"hb_bidder":"showheroes-bs","hb_pb":"0.50","hb_cache_id":"c-2"}}}}]},` +
	`{"seat":"loser","bid":[{"id":"b3","impid":"2","ext":{"prebid":{"targeting":{"hb_bidder":"loser","hb_pb":"0.10"}}}}]}]}}`

// ampTargetingPair records the AMP targeting each side merged from ampBids.
func ampTargetingPair(goTargeting, rustTargeting string) ruleCase {
	return func() (Case, *parity.Trace, *parity.Trace) {
		g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
		for _, side := range []struct {
			tr        *parity.Trace
			targeting string
		}{{g, goTargeting}, {r, rustTargeting}} {
			put(side.tr, parity.StageExchangeResponse, parity.UnkeyedStage, ampBids)
			put(side.tr, parity.StageAMPTargetingResponse, parity.UnkeyedStage, `{"targeting":`+side.targeting+`}`)
		}
		return Case{CaseID: "rule"}, g, r
	}
}

const (
	ampShowheroesLast = `{"hb_bidder":"showheroes-bs","hb_pb":"0.50","hb_cache_id":"c-2","hb_deal":"d1"}`
	ampMabidderLast   = `{"hb_bidder":"mabidder","hb_pb":"2.70","hb_cache_id":"c-1","hb_deal":"d1"}`
	// ampMixed takes hb_bidder from one bid and hb_pb from the other, which no
	// merge order gives.
	ampMixed = `{"hb_bidder":"mabidder","hb_pb":"0.50","hb_cache_id":"c-1","hb_deal":"d1"}`
	// ampNoDeal lacks hb_deal, which only one bid sets and every order keeps.
	ampNoDeal = `{"hb_bidder":"showheroes-bs","hb_pb":"0.50","hb_cache_id":"c-2"}`
	// ampLoser takes the uncached bid's values, which Go never merges.
	ampLoser = `{"hb_bidder":"loser","hb_pb":"0.10","hb_cache_id":"c-2","hb_deal":"d1"}`
)

// TestCandidateRuleScope: a candidate rule masks a value only when the value
// is one of at least two candidates the side's own trace shows.
func TestCandidateRuleScope(t *testing.T) {
	tests := []struct {
		name     string
		pair     ruleCase
		mismatch bool
	}{
		{"two failing imps", storedAuctionPair("i1", "i2", 2), false},
		{"one failing imp", storedAuctionPair("i1", "i2", 1), true},
		{"reported imp is not a candidate", storedAuctionPair("i1", "i9", 2), true},
		{"cur of another seat", responseCurPair("USD", "EUR"), false},
		{"cur of no seat", responseCurPair("USD", "JPY"), true},
		{"amp targeting merged in another order", ampTargetingPair(ampMabidderLast, ampShowheroesLast), false},
		{"amp targeting no order merges", ampTargetingPair(ampMabidderLast, ampMixed), true},
		{"amp targeting drops a key one bid sets", ampTargetingPair(ampShowheroesLast, ampNoDeal), true},
		{"amp targeting of an uncached bid", ampTargetingPair(ampShowheroesLast, ampLoser), true},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c, g, r := tt.pair()
			doc := compareTraces(t, e, c, g, r)
			if got := len(doc.Mismatches) != 0; got != tt.mismatch {
				t.Fatalf("mismatch = %v, want %v: %v", got, tt.mismatch, ids(doc))
			}
		})
	}
}
