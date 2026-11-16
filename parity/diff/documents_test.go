package diff

import (
	"encoding/json"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

func requestsOutput(impl, body string, headers parity.HeaderList, errs []parity.ErrorRecord) *parity.AdapterRequestsOutput {
	return &parity.AdapterRequestsOutput{
		Schema: parity.SchemaAdapterRequestsOutput, SchemaVersion: parity.SchemaVersion, Impl: impl, CaseID: "adapter", Bidder: "appnexus",
		Requests: []parity.AdapterRequestData{{
			Method: "POST", URI: "https://ib.adnxs.com/openrtb2", Headers: headers, Body: parity.TextBody(body), ImpIDs: []string{"i1"},
		}},
		Errors: errs,
	}
}

func TestCompareAdapterRequests(t *testing.T) {
	e := newTestEngine(t)
	ct := parity.HeaderList{{"Content-Type", "application/json;charset=utf-8"}, {"X-Prebid", "1"}}
	tests := []struct {
		name       string
		goBody     string
		rustBody   string
		rustHeader parity.HeaderList
		rustErrs   []parity.ErrorRecord
		want       []brief
	}{
		{"equal", `{"id":"r1"}`, `{"id":"r1"}`, ct, []parity.ErrorRecord{}, nil},
		{"header names are case-insensitive", `{"id":"r1"}`, `{"id":"r1"}`,
			parity.HeaderList{{"content-type", "application/json;charset=utf-8"}, {"x-prebid", "1"}}, []parity.ErrorRecord{}, nil},
		{"missing header", `{"id":"r1"}`, `{"id":"r1"}`, ct[:1], []parity.ErrorRecord{},
			[]brief{{parity.KindHeader, "", "$.requests[0].headers['X-Prebid']"}}},
		{"body content", `{"id":"r1","imp":[{"id":"i1"}]}`, `{"id":"r1","imp":[{"id":"i2"}]}`, ct, []parity.ErrorRecord{},
			[]brief{{parity.KindValue, "", "$.requests[0].body.imp[0].id"}}},
		{"body spelling is lexical", `{"w":300}`, `{"w":300.0}`, ct, []parity.ErrorRecord{},
			[]brief{{parity.KindLexical, parity.LexicalNumberFormat, "$.requests[0].body.w"}}},
		{"body trailing newline", "{\"w\":300}\n", `{"w":300}`, ct, []parity.ErrorRecord{},
			[]brief{{parity.KindLexical, parity.LexicalTrailingNewline, "$.requests[0].body"}}},
		{"errors by code", `{}`, `{}`, ct, []parity.ErrorRecord{{Code: 2, Message: "bad"}},
			[]brief{{parity.KindExtraInRust, "", "$.errors[0]"}}},
		{"null errors are not []", `{}`, `{}`, ct, nil,
			[]brief{{parity.KindType, "", "$.errors"}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g := requestsOutput(parity.ImplGo, tt.goBody, ct, []parity.ErrorRecord{})
			r := requestsOutput(parity.ImplRust, tt.rustBody, tt.rustHeader, tt.rustErrs)
			res, err := e.CompareAdapterRequests(Case{CaseID: "adapter"}, g, r)
			if err != nil {
				t.Fatal(err)
			}
			validateDiff(t, res.Doc)
			if got := briefs(res.Doc.Mismatches); !equalBriefs(got, tt.want) {
				t.Fatalf("got %v\nwant %v", got, tt.want)
			}
			for _, m := range res.Doc.Mismatches {
				if m.Stage != parity.StageAdapterRequests || m.Key != "appnexus" || !m.Primary {
					t.Fatalf("record %s/%s primary=%v", m.Stage, m.Key, m.Primary)
				}
			}
		})
	}
}

func TestCompareAdapterBids(t *testing.T) {
	e := newTestEngine(t)
	mk := func(impl string, price string) *parity.AdapterBidsOutput {
		return &parity.AdapterBidsOutput{
			Schema: parity.SchemaAdapterBidsOutput, SchemaVersion: parity.SchemaVersion, Impl: impl, Bidder: "rubicon",
			BidderResponse: &parity.AdapterBidderResponse{Currency: "USD", Bids: []parity.TypedBid{{
				Bid: json.RawMessage(`{"id":"b1","impid":"i1","price":` + price + `}`), Type: "banner",
			}}},
			Errors: []parity.ErrorRecord{},
		}
	}
	res, err := e.CompareAdapterBids(Case{CaseID: "bids"}, mk(parity.ImplGo, "1.5"), mk(parity.ImplRust, "1.25"))
	if err != nil {
		t.Fatal(err)
	}
	validateDiff(t, res.Doc)
	if got := ids(res.Doc); !equalStrings(got, []string{"adapter.bids/rubicon value: $.bidder_response.bids[0].bid.price"}) {
		t.Fatalf("got %v", got)
	}
	if _, err := e.CompareAdapterBids(Case{}, mk(parity.ImplGo, "1"), nil); err == nil {
		t.Fatal("accepted a nil output")
	}
}

func TestCompareIntrospection(t *testing.T) {
	e := newTestEngine(t)
	cfg := func(impl, raw string) *parity.ConfigDump {
		return &parity.ConfigDump{Schema: parity.SchemaConfigDump, SchemaVersion: parity.SchemaVersion, Impl: impl, Config: json.RawMessage(raw)}
	}
	res, err := e.CompareConfig(Case{CaseID: "config"}, cfg(parity.ImplGo, `{"gdpr":{"default_value":"1"},"port":8000}`),
		cfg(parity.ImplRust, `{"port":8000,"gdpr":{"default_value":"0"}}`))
	if err != nil {
		t.Fatal(err)
	}
	validateDiff(t, res.Doc)
	if got := ids(res.Doc); !equalStrings(got, []string{"config.resolved/_ value: $.gdpr.default_value"}) {
		t.Fatalf("config: %v", got)
	}
	if res.Doc.Mismatches[0].Domain != parity.DomainConfigBootstrap {
		t.Fatalf("config domain %s", res.Doc.Mismatches[0].Domain)
	}

	bidders := func(impl string, active []string) *parity.BiddersDump {
		return &parity.BiddersDump{Schema: parity.SchemaBiddersDump, SchemaVersion: parity.SchemaVersion, Impl: impl,
			Bidders: json.RawMessage(`{"appnexus":{"maintainer":"x"}}`), Active: active, Disabled: map[string]string{}}
	}
	res, err = e.CompareBidders(Case{CaseID: "bidders"}, bidders(parity.ImplGo, []string{"appnexus", "rubicon"}),
		bidders(parity.ImplRust, []string{"appnexus"}))
	if err != nil {
		t.Fatal(err)
	}
	if got := ids(res.Doc); !equalStrings(got, []string{"bidders.resolved/_ missing_in_rust: $.active[1]"}) {
		t.Fatalf("bidders: %v", got)
	}

	wrong := cfg(parity.ImplRust, `{}`)
	wrong.Schema = parity.SchemaBiddersDump
	if _, err := e.CompareConfig(Case{}, cfg(parity.ImplGo, `{}`), wrong); err == nil {
		t.Fatal("accepted a document of another schema")
	}
}
