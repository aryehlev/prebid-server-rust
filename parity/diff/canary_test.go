package diff

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
	"sync"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

// TestRaceEngineConcurrentUse: one engine serves many comparisons at once (run
// with -race), and every run of the same inputs gives the same document.
func TestRaceEngineConcurrentUse(t *testing.T) {
	e := newTestEngine(t)
	buggy := rustSide
	buggy.dropXPrebid = true
	compare := func() (parity.DiffDocument, error) {
		res, err := e.CompareTraces(Case{CaseID: "canary", Envelope: canaryEnvelope()}, canaryTrace(goSide), canaryTrace(buggy))
		return res.Doc, err
	}
	want, err := compare()
	if err != nil {
		t.Fatal(err)
	}
	const workers = 8
	docs := make([]parity.DiffDocument, workers)
	errs := make([]error, workers)
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			docs[i], errs[i] = compare()
		}(i)
	}
	wg.Wait()
	for i := range docs {
		if errs[i] != nil {
			t.Fatal(errs[i])
		}
		if !reflect.DeepEqual(docs[i], want) {
			t.Fatalf("worker %d produced a different document", i)
		}
	}
}

// The seeded-bug canary: a Rust side that forgets the X-Prebid header on
// appnexus requests, buried in every kind of run-to-run noise the allowlist
// covers, must produce exactly one primary cluster, at adapter.requests_final.
// Noise alone must produce nothing (A/A).

const canaryInput = `{"id":"r1","imp":[{"id":"i1","banner":{"w":300,"h":250},"ext":{"prebid":{"bidder":{"appnexus":{"placementId":1},"rubicon":{"accountId":2}}}}}],"site":{"page":"https://pub.example"}}`

// canarySide describes how one side of the canary behaves.
type canarySide struct {
	impl string
	// tid and bidid are the UUIDs this run generated (source.tid and a
	// generated bid id, ext.prebid.bidid).
	tid, bidid string
	// noisy reorders everything the allowlist sorts, and changes timings,
	// timestamps and capture sequence numbers.
	noisy bool
	// dropXPrebid is the seeded bug.
	dropXPrebid bool
}

func canaryEnvelope() *parity.Envelope {
	return &parity.Envelope{
		Schema: parity.SchemaEnvelope, SchemaVersion: parity.SchemaVersion, CaseID: "canary",
		Request: parity.ExecRequest{
			Method: "POST", Path: "/openrtb2/auction",
			Headers: parity.HeaderList{{"Content-Type", "application/json"}},
			Body:    parity.TextBody(canaryInput),
		},
	}
}

func canaryTrace(s canarySide) *parity.Trace {
	t := newTrace(s.impl)
	request := fmt.Sprintf(`{"id":"r1","imp":[{"id":"i1","banner":{"w":300,"h":250}}],"site":{"page":"https://pub.example"},"source":{"tid":%q}}`, s.tid)
	put(t, parity.StageRequestValidated, parity.UnkeyedStage, request)
	for _, bidder := range []string{"appnexus", "rubicon"} {
		put(t, parity.StageBidderRequest, bidder, request)
		put(t, parity.StageAdapterRequestsFinal, bidder, adapterRequests(bidder, request, s.headers(bidder)))
	}
	t.HTTPOut = canaryCalls(s, request)
	t.HTTPResponse = textResponse(200, canaryResponse(s), [2]string{"Content-Type", "application/json"})
	if s.noisy {
		for stage, entries := range t.Stages {
			for key, e := range entries {
				e.Seq += 40
				t.Stages[stage][key] = e
			}
		}
	}
	return t
}

func (s canarySide) headers(bidder string) parity.HeaderList {
	h := parity.HeaderList{{"Content-Type", "application/json;charset=utf-8"}, {"Accept", "application/json"}}
	if bidder != "appnexus" || !s.dropXPrebid {
		h = append(h, [2]string{"X-Prebid", "pbs-go/4.0.0"})
	}
	return h
}

func bidderURL(bidder string) string {
	return "https://" + bidder + ".example/openrtb2"
}

func adapterRequests(bidder, body string, headers parity.HeaderList) string {
	data := []parity.AdapterRequestData{{
		Method: "POST", URI: bidderURL(bidder), Headers: headers, Body: parity.TextBody(body), ImpIDs: []string{"i1"},
	}}
	raw, err := json.Marshal(map[string]any{"requests": data})
	if err != nil {
		return "null"
	}
	return string(raw)
}

func canaryCalls(s canarySide, body string) []parity.OutboundCall {
	var calls []parity.OutboundCall
	for i, bidder := range []string{"appnexus", "rubicon"} {
		seq := 10 + i
		if s.noisy {
			seq = 20 - i
		}
		calls = append(calls, parity.OutboundCall{
			Seq: seq, Kind: parity.OutboundBidder, Bidder: bidder, OrigURL: bidderURL(bidder),
			Request:  parity.OutboundRequest{Method: "POST", URL: bidderURL(bidder), Header: s.headers(bidder), Body: parity.TextBody(body)},
			Mock:     &parity.MockRef{Source: "inline", ID: "bidders." + bidder},
			Response: &parity.OutboundResponse{Status: 200, Headers: parity.HeaderList{{"Content-Type", "application/json"}}, Body: parity.TextBody(`{"id":"r1"}`)},
		})
	}
	if s.noisy {
		calls[0], calls[1] = calls[1], calls[0]
	}
	return calls
}

func canaryResponse(s canarySide) string {
	seatbids := []string{
		`{"bid":[{"id":"a2","impid":"i1","price":0.5},{"id":"a1","impid":"i1","price":1.25}],"seat":"appnexus"}`,
		fmt.Sprintf(`{"bid":[{"id":"r1b","impid":"i1","price":2,"ext":{"prebid":{"bidid":%q}}}],"seat":"rubicon"}`, s.bidid),
	}
	warnings := []string{`{"code":10002,"message":"w2"}`, `{"code":10001,"message":"w1"}`}
	rtm, ts := `{"appnexus":12,"rubicon":31}`, "1629473330823"
	if s.noisy {
		seatbids[0] = `{"bid":[{"id":"a1","impid":"i1","price":1.25},{"id":"a2","impid":"i1","price":0.5}],"seat":"appnexus"}`
		seatbids[0], seatbids[1] = seatbids[1], seatbids[0]
		warnings[0], warnings[1] = warnings[1], warnings[0]
		rtm, ts = `{"appnexus":48,"rubicon":7}`, "1700000000123"
	}
	return fmt.Sprintf(`{"id":"r1","seatbid":[%s],"cur":"USD","ext":{"responsetimemillis":%s,"warnings":{"general":[%s]},"prebid":{"auctiontimestamp":%s}}}`+"\n",
		strings.Join(seatbids, ","), rtm, strings.Join(warnings, ","), ts)
}

var (
	goSide   = canarySide{impl: parity.ImplGo, tid: "2e8c1f6a-4b3d-4c2e-9f10-0a1b2c3d4e5f", bidid: "8f1e2d3c-4b5a-4968-a7b6-c5d4e3f2a1b0"}
	rustSide = canarySide{impl: parity.ImplRust, tid: "7d9e0a1b-2c3d-4e5f-8a6b-7c8d9e0f1a2b", bidid: "c3d4e5f6-a7b8-4c9d-ae0f-1a2b3c4d5e6f", noisy: true}
)

func TestCanary(t *testing.T) {
	e := newTestEngine(t)
	buggy := rustSide
	buggy.dropXPrebid = true
	tests := []struct {
		name         string
		goSide       canarySide
		rustSide     canarySide
		wantRecords  int
		wantClusters []string // stage of each primary cluster
	}{
		{name: "identical", goSide: goSide, rustSide: canarySide{impl: parity.ImplRust, tid: goSide.tid, bidid: goSide.bidid}},
		{name: "A/A noise only", goSide: goSide, rustSide: rustSide},
		{name: "seeded X-Prebid bug under noise", goSide: goSide, rustSide: buggy, wantRecords: 2,
			wantClusters: []string{string(parity.StageAdapterRequestsFinal)}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			c := Case{CaseID: "canary", Envelope: canaryEnvelope()}
			doc := compareTraces(t, e, c, canaryTrace(tt.goSide), canaryTrace(tt.rustSide))
			if len(doc.Mismatches) != tt.wantRecords {
				t.Fatalf("got %d mismatches, want %d: %v", len(doc.Mismatches), tt.wantRecords, ids(doc))
			}
			primary := PrimaryClusters(BuildClusters([]parity.DiffDocument{doc}))
			if len(primary) != len(tt.wantClusters) {
				t.Fatalf("got %d primary clusters, want %d: %v", len(primary), len(tt.wantClusters), ids(doc))
			}
			for i, cl := range primary {
				if string(cl.Stage) != tt.wantClusters[i] {
					t.Errorf("primary cluster %d at %s, want %s", i, cl.Stage, tt.wantClusters[i])
				}
			}
		})
	}
}

// TestCanaryAttribution pins what the canary's records say, so a regression
// in path building or attribution is visible.
func TestCanaryAttribution(t *testing.T) {
	e := newTestEngine(t)
	buggy := rustSide
	buggy.dropXPrebid = true
	doc := compareTraces(t, e, Case{CaseID: "canary", Envelope: canaryEnvelope()}, canaryTrace(goSide), canaryTrace(buggy))
	want := []struct {
		stage   parity.StageID
		key     string
		path    string
		primary bool
	}{
		{parity.StageAdapterRequestsFinal, "appnexus", "$.requests[0].headers['X-Prebid']", true},
		{parity.StageHTTPOut, parity.UnkeyedStage, "$[0].request.headers['X-Prebid']", false},
	}
	if len(doc.Mismatches) != len(want) {
		t.Fatalf("got %v", ids(doc))
	}
	for i, w := range want {
		m := doc.Mismatches[i]
		if m.Stage != w.stage || m.Key != w.key || m.JSONPath != w.path || m.Primary != w.primary || m.Kind != parity.KindHeader {
			t.Errorf("record %d = %s/%s %s %s primary=%v, want %s/%s header %s primary=%v",
				i, m.Stage, m.Key, m.Kind, m.JSONPath, m.Primary, w.stage, w.key, w.path, w.primary)
		}
		if string(m.Go) != `["pbs-go/4.0.0"]` || string(m.Rust) != "null" {
			t.Errorf("record %d values go=%s rust=%s", i, m.Go, m.Rust)
		}
	}
	if c := doc.Mismatches[1].ConsequentialOf; c == nil || *c != doc.Mismatches[0].Signature {
		t.Errorf("http.out record is not consequential of the adapter.requests_final record")
	}
	if doc.Mismatches[0].Domain != parity.DomainBiddersAdapters || doc.Mismatches[0].Requirement != "BIDD-01" {
		t.Errorf("attributed to %s/%s, want bidders_adapters/BIDD-01", doc.Mismatches[0].Domain, doc.Mismatches[0].Requirement)
	}
}
