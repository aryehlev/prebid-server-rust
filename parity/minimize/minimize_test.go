package minimize

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"reflect"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

const (
	sigFloors = "sig-floors-enforcepbs"
	sigMulti  = "sig-multi-imp"
)

// originalBody is a realistic auction request. The fake bug needs only
// ext.prebid.floors.enforcement.enforcepbs == true and the X-Bug header.
const originalBody = `{
  "id": "req-1",
  "imp": [
    {"id": "imp-1", "banner": {"format": [{"w": 300, "h": 250}, {"w": 300, "h": 600}]},
     "ext": {"prebid": {"bidder": {"appnexus": {"placementId": 12883451}, "rubicon": {"accountId": 1001, "siteId": 113932, "zoneId": 535510}}}}},
    {"id": "imp-2", "video": {"mimes": ["video/mp4"], "w": 640, "h": 480},
     "ext": {"appnexus": {"placementId": 1}}}
  ],
  "site": {"page": "https://example.com/page?x=1", "publisher": {"id": "pub-1"}},
  "device": {"ua": "Mozilla/5.0", "ip": "1.2.3.4"},
  "cur": ["USD"],
  "tmax": 500,
  "ext": {"prebid": {"targeting": {"pricegranularity": "low"}, "floors": {"enabled": true, "enforcement": {"enforcepbs": true, "floordeals": false}}}}
}`

func originalSubject() Subject {
	return Subject{
		Envelope: parity.Envelope{
			Schema: parity.SchemaEnvelope, SchemaVersion: parity.SchemaVersion, CaseID: "case-1",
			Request: parity.ExecRequest{
				Method: "POST", Path: "/openrtb2/auction", Query: "debug=1&a=b",
				Headers: parity.HeaderList{{"Content-Type", "application/json"}, {"X-Bug", "on"}, {"User-Agent", "test"}},
				Body:    parity.TextBody(originalBody),
			},
			Decisions: map[string]json.RawMessage{"tid": json.RawMessage(`"abc"`)},
		},
		Mocks: corpus.MockPlan{
			HTTP:    []corpus.HTTPMock{{ID: "m1", Bidder: "appnexus"}, {ID: "m2", Bidder: "rubicon"}},
			Bidders: map[string]corpus.MockBidder{"appnexus": {Currency: "USD"}},
		},
	}
}

// fakeServer reproduces a nested-field bug: sigFloors is primary when the
// body enables PBS floor enforcement and the X-Bug header is sent; sigMulti
// is primary when the request has at least two imps.
type fakeServer struct {
	calls int
	fail  func(Subject) bool
}

func (f *fakeServer) Execute(_ context.Context, s Subject) ([]string, error) {
	f.calls++
	if f.fail != nil && f.fail(s) {
		return nil, errors.New("server crashed")
	}
	raw, err := s.Envelope.Request.Body.DecodedBytes()
	if err != nil {
		return nil, err
	}
	var req struct {
		Imp []json.RawMessage `json:"imp"`
		Ext struct {
			Prebid struct {
				Floors struct {
					Enforcement struct {
						EnforcePBS *bool `json:"enforcepbs"`
					} `json:"enforcement"`
				} `json:"floors"`
			} `json:"prebid"`
		} `json:"ext"`
	}
	var sigs []string
	if json.Unmarshal(raw, &req) != nil {
		return sigs, nil
	}
	hasHeader := false
	for _, h := range s.Envelope.Request.Headers {
		hasHeader = hasHeader || h[0] == "X-Bug"
	}
	if e := req.Ext.Prebid.Floors.Enforcement.EnforcePBS; e != nil && *e && hasHeader {
		sigs = append(sigs, sigFloors)
	}
	if len(req.Imp) >= 2 {
		sigs = append(sigs, sigMulti)
	}
	return sigs, nil
}

func bodyText(t *testing.T, s Subject) string {
	t.Helper()
	raw, err := s.Envelope.Request.Body.DecodedBytes()
	if err != nil {
		t.Fatalf("decoding body: %v", err)
	}
	return string(raw)
}

// assertOneMinimal applies every single candidate edit of every phase to the
// result and checks that none of them still reproduces the target.
func assertOneMinimal(t *testing.T, s Subject, target []string) {
	t.Helper()
	b, err := newBase(s)
	if err != nil {
		t.Fatalf("newBase: %v", err)
	}
	var edits []Edit
	edits = append(edits, b.edits(phaseGroups, 0)...)
	for depth := 1; depth <= b.maxDepth(); depth++ {
		edits = append(edits, b.edits(phaseDelete, depth)...)
	}
	edits = append(edits, b.edits(phaseValues, 0)...)
	exec := &fakeServer{}
	for _, e := range edits {
		cand, err := b.apply([]Edit{e})
		if err != nil {
			t.Fatalf("apply %s: %v", e, err)
		}
		sigs, err := exec.Execute(context.Background(), cand)
		if err != nil {
			t.Fatalf("execute: %v", err)
		}
		if containsAll(sigs, target) {
			t.Errorf("not 1-minimal: edit %s still reproduces %v (body %s)", e, target, bodyText(t, cand))
		}
	}
}

func TestMinimize(t *testing.T) {
	tests := []struct {
		name        string
		target      []string
		wantBody    string
		wantHeaders parity.HeaderList
	}{
		{
			name:        "explicit target keeps only the nested field",
			target:      []string{sigFloors},
			wantBody:    `{"ext":{"prebid":{"floors":{"enforcement":{"enforcepbs":true}}}}}`,
			wantHeaders: parity.HeaderList{{"X-Bug", "on"}},
		},
		{
			name:        "default target keeps both signatures",
			wantBody:    `{"imp":[{},{}],"ext":{"prebid":{"floors":{"enforcement":{"enforcepbs":true}}}}}`,
			wantHeaders: parity.HeaderList{{"X-Bug", "on"}},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			exec := &fakeServer{}
			res, err := Minimize(context.Background(), originalSubject(), exec, Options{Target: tc.target})
			if err != nil {
				t.Fatalf("Minimize: %v", err)
			}
			if got := bodyText(t, res.Subject); got != tc.wantBody {
				t.Errorf("body = %s, want %s", got, tc.wantBody)
			}
			req := res.Subject.Envelope.Request
			if !reflect.DeepEqual(req.Headers, tc.wantHeaders) {
				t.Errorf("headers = %v, want %v", req.Headers, tc.wantHeaders)
			}
			if req.Query != "" || len(res.Subject.Mocks.HTTP) != 0 || res.Subject.Mocks.Bidders != nil || res.Subject.Envelope.Decisions != nil {
				t.Errorf("query, mocks and decisions not dropped: %+v", res.Subject)
			}
			rep := res.Report
			if !rep.OneMinimal || rep.BudgetExhausted {
				t.Errorf("report one_minimal=%v budget_exhausted=%v", rep.OneMinimal, rep.BudgetExhausted)
			}
			if rep.Executions != exec.calls || rep.Executions > DefaultBudget {
				t.Errorf("executions = %d, executor calls = %d", rep.Executions, exec.calls)
			}
			if rep.After.BodyBytes >= rep.Before.BodyBytes || len(rep.Applied) == 0 {
				t.Errorf("no reduction recorded: %+v", rep)
			}
			assertOneMinimal(t, res.Subject, rep.Target)
		})
	}
}

func TestMinimizeDeterministic(t *testing.T) {
	encode := func() []byte {
		res, err := Minimize(context.Background(), originalSubject(), &fakeServer{}, Options{Target: []string{sigFloors}})
		if err != nil {
			t.Fatalf("Minimize: %v", err)
		}
		raw, err := json.Marshal(res)
		if err != nil {
			t.Fatalf("encoding result: %v", err)
		}
		return raw
	}
	first := encode()
	for i := 0; i < 3; i++ {
		if again := encode(); !bytes.Equal(first, again) {
			t.Fatalf("run %d differs:\n%s\n%s", i, first, again)
		}
	}
}

func TestMinimizeBudget(t *testing.T) {
	tests := []struct {
		name   string
		budget int
	}{
		{"one execution", 1},
		{"a few executions", 6},
		{"twenty executions", 20},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			exec := &fakeServer{}
			res, err := Minimize(context.Background(), originalSubject(), exec, Options{Target: []string{sigFloors}, Budget: tc.budget})
			if err != nil {
				t.Fatalf("Minimize: %v", err)
			}
			if exec.calls > tc.budget || res.Report.Executions != exec.calls {
				t.Errorf("calls = %d, executions = %d, budget %d", exec.calls, res.Report.Executions, tc.budget)
			}
			if !res.Report.BudgetExhausted || res.Report.OneMinimal {
				t.Errorf("budget_exhausted=%v one_minimal=%v", res.Report.BudgetExhausted, res.Report.OneMinimal)
			}
			sigs, err := (&fakeServer{}).Execute(context.Background(), res.Subject)
			if err != nil || !containsAll(sigs, []string{sigFloors}) {
				t.Errorf("budget-limited result does not reproduce: %v %v", sigs, err)
			}
		})
	}
}

func TestMinimizeErrors(t *testing.T) {
	tests := []struct {
		name    string
		exec    Executor
		target  []string
		wantErr error
	}{
		{"target not reproduced", &fakeServer{}, []string{"sig-unknown"}, ErrNotReproduced},
		{"original fails", ExecutorFunc(func(context.Context, Subject) ([]string, error) { return nil, errors.New("down") }), nil, nil},
		{"no signatures", ExecutorFunc(func(context.Context, Subject) ([]string, error) { return nil, nil }), nil, ErrNotReproduced},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			_, err := Minimize(context.Background(), originalSubject(), tc.exec, Options{Target: tc.target})
			if err == nil {
				t.Fatal("Minimize succeeded, want an error")
			}
			if tc.wantErr != nil && !errors.Is(err, tc.wantErr) {
				t.Errorf("err = %v, want %v", err, tc.wantErr)
			}
		})
	}
	if _, err := Minimize(context.Background(), originalSubject(), nil, Options{}); err == nil {
		t.Error("nil executor accepted")
	}
}

func TestMinimizeCandidateFailures(t *testing.T) {
	// Candidates without a site member crash the server; they count as
	// not reproducing, so site survives.
	exec := &fakeServer{fail: func(s Subject) bool {
		raw, _ := s.Envelope.Request.Body.DecodedBytes()
		return s.Envelope.CaseID == "case-1" && !strings.Contains(string(raw), `"site"`)
	}}
	res, err := Minimize(context.Background(), originalSubject(), exec, Options{Target: []string{sigFloors}})
	if err != nil {
		t.Fatalf("Minimize: %v", err)
	}
	if res.Report.ExecErrors == 0 {
		t.Error("exec_errors = 0")
	}
	if got := bodyText(t, res.Subject); !strings.Contains(got, `"site":{}`) {
		t.Errorf("body = %s, want site kept as {}", got)
	}
}

func TestMinimizeCancelled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	exec := ExecutorFunc(func(_ context.Context, s Subject) ([]string, error) {
		cancel()
		return []string{sigFloors}, nil
	})
	if _, err := Minimize(ctx, originalSubject(), exec, Options{}); !errors.Is(err, context.Canceled) {
		t.Errorf("err = %v, want context.Canceled", err)
	}
}

func TestTreeRoundTrip(t *testing.T) {
	tests := []struct {
		name string
		in   string
		want string
	}{
		{"member order kept", `{"b":1,"a":2}`, `{"b":1,"a":2}`},
		{"number spelling kept", `{"p":1.50,"e":1e3,"n":-0}`, `{"p":1.50,"e":1e3,"n":-0}`},
		{"duplicate names kept", `{"a":1,"a":2}`, `{"a":1,"a":2}`},
		{"html not escaped", `{"u":"<a>&"}`, `{"u":"<a>&"}`},
		{"nested arrays", ` [ [1, [2]], {}, null, true, "x" ] `, `[[1,[2]],{},null,true,"x"]`},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			tree, err := parseTree([]byte(tc.in))
			if err != nil {
				t.Fatalf("parseTree: %v", err)
			}
			var b bytes.Buffer
			tree.encode(&b, newChanges())
			if b.String() != tc.want {
				t.Errorf("encode = %s, want %s", b.String(), tc.want)
			}
		})
	}
	for _, bad := range []string{`{"a":`, `{"a":1} {}`, `[1,]`} {
		if _, err := parseTree([]byte(bad)); err == nil {
			t.Errorf("parseTree(%s) succeeded", bad)
		}
	}
}

func TestEditsCompose(t *testing.T) {
	// Deleting two elements of one array in one application must remove
	// exactly those two, whatever their order.
	s := originalSubject()
	s.Envelope.Request.Body = parity.TextBody(`{"a":[10,11,12,13]}`)
	b, err := newBase(s)
	if err != nil {
		t.Fatalf("newBase: %v", err)
	}
	edits := b.edits(phaseDelete, 2)
	if len(edits) != 4 {
		t.Fatalf("edits = %v", edits)
	}
	for _, pick := range [][]Edit{{edits[1], edits[2]}, {edits[2], edits[1]}} {
		out, err := b.apply(pick)
		if err != nil {
			t.Fatalf("apply: %v", err)
		}
		if got := bodyText(t, out); got != `{"a":[10,13]}` {
			t.Errorf("body = %s", got)
		}
	}
}

func TestBidderEdit(t *testing.T) {
	b, err := newBase(originalSubject())
	if err != nil {
		t.Fatalf("newBase: %v", err)
	}
	var drop Edit
	for _, e := range b.edits(phaseGroups, 0) {
		if e.Kind == EditDropBidder && e.Target == "appnexus" {
			drop = e
		}
	}
	if drop.Kind == "" {
		t.Fatal("no drop_bidder edit for appnexus")
	}
	out, err := b.apply([]Edit{drop})
	if err != nil {
		t.Fatalf("apply: %v", err)
	}
	body := bodyText(t, out)
	if strings.Contains(body, "appnexus") || !strings.Contains(body, "rubicon") {
		t.Errorf("body = %s", body)
	}
	if len(out.Mocks.HTTP) != 1 || out.Mocks.HTTP[0].ID != "m2" || out.Mocks.Bidders != nil {
		t.Errorf("mocks = %+v", out.Mocks)
	}
	if sz := SizeOf(originalSubject()); sz.Imps != 2 || sz.Bidders != 2 || sz.Headers != 3 || sz.QueryParams != 2 || sz.MockRules != 2 {
		t.Errorf("size = %+v", sz)
	}
}
