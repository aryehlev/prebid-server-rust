package diff

import (
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

func TestHeaders(t *testing.T) {
	const base = "$['@headers']"
	semantic := headerOptions{caseInsensitive: true}
	exact := headerOptions{}
	wireRequest := headerOptions{caseInsensitive: true, wire: true, request: true}
	wireResponse := headerOptions{caseInsensitive: true, wire: true, ignore: map[string]bool{"date": true}}
	h := func(kv ...string) parity.HeaderList {
		var out parity.HeaderList
		for i := 0; i+1 < len(kv); i += 2 {
			out = append(out, [2]string{kv[i], kv[i+1]})
		}
		return out
	}
	tests := []struct {
		name string
		opt  headerOptions
		g, r parity.HeaderList
		want []brief
	}{
		{"equal", semantic, h("A", "1"), h("A", "1"), nil},
		{"names are case-insensitive", semantic, h("X-Prebid", "1"), h("x-prebid", "1"), nil},
		{"order across names is ignored", semantic, h("A", "1", "B", "2"), h("B", "2", "A", "1"), nil},
		{"value order of one name is kept", semantic, h("A", "1", "A", "2"), h("A", "2", "A", "1"),
			[]brief{{parity.KindHeader, "", base + ".A"}}},
		{"missing header", semantic, h("X-Prebid", "1", "A", "1"), h("A", "1"),
			[]brief{{parity.KindHeader, "", base + "['X-Prebid']"}}},
		{"exact lists without the rule", exact, h("A", "1"), h("a", "1"),
			[]brief{{parity.KindHeader, "", base}}},
		{"wire header case", wireRequest, h("X-Prebid", "1"), h("x-prebid", "1"),
			[]brief{{parity.KindWire, parity.WireHeaderCase, base + "['X-Prebid']"}}},
		{"wire header case of a repeated header", wireRequest, h("A", "1", "A", "2"), h("A", "1", "A", "2"), nil},
		{"wire header order", wireRequest, h("A", "1", "B", "2"), h("B", "2", "A", "1"),
			[]brief{{parity.KindWire, parity.WireHeaderOrder, base}}},
		{"wire order ignores one-sided names", wireRequest, h("A", "1", "C", "3", "B", "2"), h("A", "1", "B", "2"),
			[]brief{{parity.KindHeader, "", base + ".C"}}},
		{"client user agent", wireRequest, h("User-Agent", "Go-http-client/1.1"), h("User-Agent", "reqwest/0.12"),
			[]brief{{parity.KindWire, parity.WireDefaultHeader, base + "['User-Agent']"}}},
		{"adapter user agent is semantic", wireRequest, h("User-Agent", "prebid-server/1.0"), h("User-Agent", "pbs-rust"),
			[]brief{{parity.KindHeader, "", base + "['User-Agent']"}}},
		{"transport accept-encoding", wireRequest, h("Accept-Encoding", "gzip"), nil,
			[]brief{{parity.KindWire, parity.WireDefaultHeader, base + "['Accept-Encoding']"}}},
		{"default accept", wireRequest, h("Accept", "*/*"), nil,
			[]brief{{parity.KindWire, parity.WireDefaultHeader, base + ".Accept"}}},
		{"content length", wireRequest, h("Content-Length", "10"), h("Content-Length", "11"),
			[]brief{{parity.KindWire, parity.WireContentLength, base + "['Content-Length']"}}},
		{"server default", wireResponse, nil, h("Server", "pbs"),
			[]brief{{parity.KindWire, parity.WireDefaultHeader, base + ".Server"}}},
		{"date ignored", wireResponse, h("Date", "a"), h("date", "b"), nil},
		{"transfer encoding", wireResponse, h("Content-Length", "2"), h("Transfer-Encoding", "chunked"),
			[]brief{{parity.KindWire, parity.WireContentLength, base + "['Content-Length']"},
				{parity.KindWire, parity.WireTransferEncoding, base + "['Transfer-Encoding']"}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c := &comparer{stage: parity.StageHTTPOut, key: parity.UnkeyedStage}
			c.headers(base, tt.g, tt.r, tt.opt)
			var got []brief
			for _, f := range c.findings {
				got = append(got, brief{kind: f.kind, class: f.class, path: f.path})
			}
			if !equalBriefs(got, tt.want) {
				t.Fatalf("got %v\nwant %v", got, tt.want)
			}
		})
	}
}

// TestWireOutboundCalls: a call the oracle captured on the wire is compared
// at the transport level; a call recorded by a client hook is not.
func TestWireOutboundCalls(t *testing.T) {
	e := newTestEngine(t)
	mk := func(wire bool, proto string, headers parity.HeaderList) []parity.OutboundCall {
		return []parity.OutboundCall{{
			Seq: 1, Kind: parity.OutboundBidder, Bidder: "appnexus", OrigURL: "https://a.example/",
			Request: parity.OutboundRequest{Method: "POST", URL: "https://a.example/", Proto: proto, Header: headers},
			Wire:    wire, Mock: &parity.MockRef{Source: "inline", ID: "x"}, Response: &parity.OutboundResponse{Status: 204},
		}}
	}
	goHeaders := parity.HeaderList{{"Content-Type", "application/json"}, {"Accept-Encoding", "gzip"}, {"User-Agent", "Go-http-client/1.1"}}
	rustHeaders := parity.HeaderList{{"content-type", "application/json"}}
	tests := []struct {
		name       string
		goWire     bool
		rustWire   bool
		rustProto  string
		wantBriefs []brief
	}{
		{"wire on both sides", true, true, "HTTP/2.0", []brief{
			{parity.KindWire, parity.WireDefaultHeader, "$[0].request.headers['Accept-Encoding']"},
			{parity.KindWire, parity.WireHeaderCase, "$[0].request.headers['Content-Type']"},
			{parity.KindWire, parity.WireDefaultHeader, "$[0].request.headers['User-Agent']"},
			{parity.KindWire, parity.WireHTTPVersion, "$[0].request.proto"},
		}},
		{"client hook captures", false, false, "HTTP/2.0", []brief{
			{parity.KindHeader, "", "$[0].request.headers['Accept-Encoding']"},
			{parity.KindHeader, "", "$[0].request.headers['User-Agent']"},
		}},
		{"capture modes differ", true, false, "HTTP/1.1", []brief{
			{parity.KindHeader, "", "$[0].request.headers['Accept-Encoding']"},
			{parity.KindHeader, "", "$[0].request.headers['User-Agent']"},
			{parity.KindWire, parity.WireOther, "$[0].wire"},
		}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
			g.HTTPOut = mk(tt.goWire, "HTTP/1.1", goHeaders)
			r.HTTPOut = mk(tt.rustWire, tt.rustProto, rustHeaders)
			doc := compareTraces(t, e, Case{CaseID: "wire"}, g, r)
			if got := briefs(doc.Mismatches); !equalBriefs(got, tt.wantBriefs) {
				t.Fatalf("got %v\nwant %v", got, tt.wantBriefs)
			}
		})
	}
}
