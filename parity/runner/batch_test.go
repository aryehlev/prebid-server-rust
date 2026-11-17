package runner

import (
	"encoding/json"
	"reflect"
	"regexp"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

func envCase(id, corpusName string, prof corpus.Profile, stored corpus.Stored, tags ...string) corpus.Case {
	return corpus.Case{
		ID: id, Kind: corpus.KindEndpoint, Corpus: corpusName, FixturePath: "f/" + id, Endpoint: corpus.EndpointAuction,
		Profile: prof, Stored: stored, Tags: tags,
		Envelope: &parity.Envelope{CaseID: id, Request: parity.ExecRequest{Method: "POST", Path: corpus.EndpointAuction}},
	}
}

func adapterCase(id string) corpus.Case {
	return corpus.Case{
		ID: id, Kind: corpus.KindAdapter, Corpus: corpus.CorpusAdapters, FixturePath: "adapters/appnexus/x.json",
		Adapter: &corpus.AdapterCase{
			Requests: parity.AdapterRequestsInput{CaseID: id, Build: parity.AdapterBuild{Bidder: "appnexus"}},
			Bids:     []parity.AdapterBidsInput{{CaseID: id + ":bids[0]"}},
		},
	}
}

func stored(id, content string) corpus.Stored {
	return corpus.Stored{Requests: map[string]json.RawMessage{id: json.RawMessage(content)}}
}

func TestSelectCases(t *testing.T) {
	c := &corpus.Corpus{Cases: []corpus.Case{
		adapterCase("a#adapter"),
		envCase("s1", corpus.CorpusSampleRequests, corpus.Profile{}, corpus.Stored{}),
		envCase("s2-gap", corpus.CorpusSampleRequests, corpus.Profile{}, corpus.Stored{}, corpus.TagContractGap),
		envCase("x1", corpus.CorpusExchangeTest, corpus.Profile{}, corpus.Stored{}),
	}}
	tests := []struct {
		name        string
		bases       []string
		sel         Selection
		wantRun     []string
		wantSkipped []string
	}{
		{"all under default", []string{"default"}, Selection{}, []string{"a#adapter", "s1", "x1"}, []string{"s2-gap"}},
		{"second profile suffixes ids and skips adapter cases", []string{"default", "floors"}, Selection{Corpora: []string{corpus.CorpusSampleRequests, corpus.CorpusAdapters}},
			[]string{"a#adapter", "s1", "s1@floors"}, []string{"s2-gap", "s2-gap@floors"}},
		{"match and limit", []string{"default"}, Selection{Match: regexp.MustCompile(`^[sx]`), Limit: 2}, []string{"s1"}, []string{"s2-gap"}},
		{"kinds", []string{"default"}, Selection{Kinds: []string{string(corpus.KindAdapter)}}, []string{"a#adapter"}, nil},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			run, skipped := selectCases(c, tc.bases, tc.sel)
			var gotRun, gotSkipped []string
			for _, rc := range run {
				gotRun = append(gotRun, rc.ID)
				if rc.Case.Envelope != nil && rc.Case.Envelope.CaseID != rc.ID {
					t.Errorf("%s: envelope case_id %q", rc.ID, rc.Case.Envelope.CaseID)
				}
			}
			for _, s := range skipped {
				gotSkipped = append(gotSkipped, s.rc.ID)
			}
			if !reflect.DeepEqual(gotRun, tc.wantRun) || !reflect.DeepEqual(gotSkipped, tc.wantSkipped) {
				t.Errorf("run = %v skipped = %v, want %v and %v", gotRun, gotSkipped, tc.wantRun, tc.wantSkipped)
			}
		})
	}
	if c.Cases[1].Envelope.CaseID != "s1" {
		t.Error("selectCases mutated the corpus")
	}
}

func TestRewriteCaseID(t *testing.T) {
	cs := adapterCase("a#adapter")
	rc := newRunCase(&cs, "floors")
	if cs.ID != "a#adapter" || cs.Adapter.Bids[0].CaseID != "a#adapter:bids[0]" {
		t.Errorf("newRunCase mutated the corpus case: %+v", cs.Adapter)
	}
	if rc.ID != "a#adapter@floors" || rc.Case.Adapter.Requests.CaseID != rc.ID || rc.Case.Adapter.Bids[0].CaseID != rc.ID+":bids[0]" || rc.Bidder != "appnexus" {
		t.Errorf("run case = %+v / %+v", rc, rc.Case.Adapter)
	}
}

func TestPlanBatches(t *testing.T) {
	floors := corpus.Profile{FloorsEnabled: true}
	cases := []corpus.Case{
		envCase("c1", corpus.CorpusSampleRequests, corpus.Profile{}, stored("r", `{"a":1}`)),
		envCase("c2", corpus.CorpusSampleRequests, floors, corpus.Stored{}),
		envCase("c3", corpus.CorpusSampleRequests, corpus.Profile{}, stored("r", `{"a":2}`)),
		envCase("c4", corpus.CorpusSampleRequests, corpus.Profile{}, stored("r", `{"a":1}`)),
		envCase("c5", corpus.CorpusSampleRequests, floors, corpus.Stored{}),
	}
	var rcs []*runCase
	for i := range cases {
		rcs = append(rcs, newRunCase(&cases[i], DefaultProfile))
	}
	batches := planBatches(rcs)
	got := map[string][]string{}
	var order []string
	for _, b := range batches {
		order = append(order, b.Name)
		for _, rc := range b.Cases {
			got[b.Name] = append(got[b.Name], rc.ID)
		}
	}
	floorsName := "default/" + floors.Name()
	want := map[string][]string{"default/default": {"c1", "c4"}, floorsName: {"c2", "c5"}, "default/default#2": {"c3"}}
	if !reflect.DeepEqual(got, want) || !reflect.DeepEqual(order, []string{"default/default", floorsName, "default/default#2"}) {
		t.Errorf("batches = %v (order %v), want %v", got, order, want)
	}
	if batches[1].Label() != "default+"+floors.Name() || batches[0].Label() != "default" {
		t.Errorf("labels = %q, %q", batches[0].Label(), batches[1].Label())
	}
	addIntrospection(batches, Profiles{"default": {Name: "default", Path: "parity/fixtures/profiles/default.yaml"}})
	last := batches[0].Cases[len(batches[0].Cases)-1]
	if !last.Intro || last.ID != "introspection:default/default" || last.Case.FixturePath != "parity/fixtures/profiles/default.yaml" {
		t.Errorf("introspection case = %+v", last)
	}
}

func TestMergeDocs(t *testing.T) {
	rec := func(sig string, primary bool, kind parity.MismatchKind) parity.MismatchRecord {
		return parity.MismatchRecord{Signature: sig, Primary: primary, Kind: kind}
	}
	head := parity.DiffDocument{CaseID: "c"}
	got := mergeDocs(head, []parity.DiffDocument{
		{Mismatches: []parity.MismatchRecord{}},
		{Mismatches: []parity.MismatchRecord{rec("req", true, parity.KindValue)}},
		{Mismatches: []parity.MismatchRecord{rec("bids", true, parity.KindValue), rec("wire", true, parity.KindWire)}},
	})
	if len(got.Mismatches) != 3 || !got.Mismatches[0].Primary || got.Mismatches[1].Primary || *got.Mismatches[1].ConsequentialOf != "req" || !got.Mismatches[2].Primary {
		t.Errorf("merged = %+v", got.Mismatches)
	}
}

func TestMaskPorts(t *testing.T) {
	ports := Ports{Main: 1001, Admin: 1002, Prometheus: 1003}
	dump := &parity.ConfigDump{Config: json.RawMessage(`{"port":1001,"admin_port":9999,"metrics":{"prometheus":{"port":1003,"namespace":""}},"host":"127.0.0.1"}`)}
	maskPorts(dump, ports)
	var got map[string]any
	if err := json.Unmarshal(dump.Config, &got); err != nil {
		t.Fatalf("decoding: %v", err)
	}
	prom := got["metrics"].(map[string]any)["prometheus"].(map[string]any)
	if got["port"] != portPlaceholder || got["admin_port"] != float64(9999) || prom["port"] != portPlaceholder || got["host"] != "127.0.0.1" {
		t.Errorf("masked = %v", got)
	}
}
