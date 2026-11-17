package runner

import (
	"context"
	"html"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/report"
)

// Case IDs of the test corpus.
const (
	caseOracle  = "sample-requests/oracle"
	caseInline  = "sample-requests/inline"
	caseGap     = "sample-requests/gap"
	caseAdapter = "adapters/appnexus/simple#adapter"
)

func fakeImpl(t *testing.T, name, mode string) Impl {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatalf("os.Executable: %v", err)
	}
	return Impl{Name: name, Binary: exe, Env: []string{fakeModeEnv + "=" + mode, fakeImplEnv + "=" + name}}
}

// testCorpus is two auction envelopes (oracle and inline mocks), a contract
// gap and an adapter case.
func testCorpus() *corpus.Corpus {
	env := func(id, mode, body string, tags ...string) corpus.Case {
		cs := envCase(id, corpus.CorpusSampleRequests, corpus.Profile{}, corpus.Stored{}, tags...)
		cs.Envelope.Schema, cs.Envelope.SchemaVersion = parity.SchemaEnvelope, parity.SchemaVersion
		cs.Envelope.Request.Headers = parity.HeaderList{{"Content-Type", "application/json"}}
		cs.Envelope.Request.Body = parity.TextBody(body)
		cs.Envelope.Mock.Mode = mode
		return cs
	}
	return &corpus.Corpus{Cases: []corpus.Case{
		adapterCase(caseAdapter),
		env(caseGap, parity.MockModeOracle, `{"id":"g"}`, corpus.TagContractGap),
		env(caseInline, parity.MockModeInline, `{"id":"i","imp":[{"id":"1"}]}`),
		env(caseOracle, parity.MockModeOracle, `{"id":"o","imp":[{"id":"1","banner":{"w":300,"h":250}}],"site":{"page":"https://example.test/"},"trigger":{"on":true}}`),
	}}
}

func testOptions(t *testing.T, a, b Impl, corpora ...string) Options {
	t.Helper()
	return Options{
		RepoRoot: testRepo(t), OutDir: filepath.Join(t.TempDir(), "run"), RunID: "test", A: a, B: b,
		Corpus: testCorpus(), Selection: Selection{Corpora: corpora}, Shards: 2,
		CaseTimeout: 15 * time.Second, StartTimeout: 30 * time.Second, Logf: t.Logf,
	}
}

func mustCase(t *testing.T, run *report.Run, id string) *report.CaseSummary {
	t.Helper()
	cs, ok := run.Case(id)
	if !ok {
		t.Fatalf("case %s missing from the run", id)
	}
	return cs
}

func wantFiles(t *testing.T, dir string, names ...string) {
	t.Helper()
	for _, n := range names {
		if _, err := os.Stat(filepath.Join(dir, filepath.FromSlash(n))); err != nil {
			t.Errorf("%s: %v", n, err)
		}
	}
}

func TestSelfTestGoVsGo(t *testing.T) {
	opts := testOptions(t, fakeImpl(t, parity.ImplGo, fakeGo), Impl{}, "all")
	res, verdict, err := SelfTest(context.Background(), opts)
	if err != nil {
		t.Fatalf("SelfTest: %v", err)
	}
	if !verdict.OK || verdict.Incomplete {
		t.Errorf("verdict = %+v", verdict)
	}
	c := res.Run.Summary.Counts
	if c.Passing != 3 || c.Mismatch != 0 || c.Error != 0 || c.NotYetInstrumented != 1 || c.Skipped != 1 {
		t.Errorf("counts = %+v", c)
	}
	if res.Run.Summary.Mode != report.ModeSelfTest || res.Run.Summary.ImplB != parity.ImplGo {
		t.Errorf("summary = %+v", res.Run.Summary)
	}
	oracleCase := mustCase(t, res.Run, caseOracle)
	if oracleCase.StagesA[parity.StageHTTPOut] != parity.StageCaptured || oracleCase.StagesB[parity.StageHTTPOut] != parity.StageCaptured {
		t.Errorf("oracle captures were not merged: %v / %v", oracleCase.StagesA, oracleCase.StagesB)
	}
	for _, n := range oracleCase.Notes {
		if strings.Contains(n, "does not validate") {
			t.Errorf("note: %s", n)
		}
	}
	if adapter := mustCase(t, res.Run, caseAdapter); !strings.Contains(adapter.Reason, "501") {
		t.Errorf("adapter case reason = %q", adapter.Reason)
	}
	if intro := mustCase(t, res.Run, "introspection:default/default"); intro.Status != report.CasePassing {
		t.Errorf("introspection = %s (%s): ports were not masked?", intro.Status, intro.Reason)
	}
	if gap := mustCase(t, res.Run, caseGap); gap.Status != report.CaseSkipped || gap.Reason == "" {
		t.Errorf("contract gap = %s (%q)", gap.Status, gap.Reason)
	}
	if len(res.Run.Summary.Servers) != 4 {
		t.Errorf("servers = %+v", res.Run.Summary.Servers)
	}
	for _, s := range res.Run.Summary.Servers {
		if !s.Instrumented || s.Log == "" {
			t.Errorf("server = %+v", s)
		}
	}
	wantFiles(t, res.Run.Layout.Dir, report.FileIndex, report.FileRun, report.FileClusters, report.FileSummaryMD, report.FileJUnit, "matrix.json", "matrix.md")
	if res.Matrix == nil || len(res.Matrix.Rows) == 0 {
		t.Error("no coverage matrix")
	}
}

func TestRunMismatchAndMinimize(t *testing.T) {
	a, b := fakeImpl(t, parity.ImplGo, fakeGo), fakeImpl(t, parity.ImplRust, fakeBug)
	opts := testOptions(t, a, b, corpus.CorpusSampleRequests)
	res, err := Run(context.Background(), opts)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	c := res.Run.Summary.Counts
	if c.Mismatch != 1 || c.Passing != 1 || c.Error != 0 {
		t.Fatalf("counts = %+v", c)
	}
	bad := mustCase(t, res.Run, caseOracle)
	if bad.Status != report.CaseMismatch || bad.Artifacts.Case == "" || bad.Artifacts.Envelope == "" || bad.Artifacts.TraceA == "" || bad.Artifacts.TraceB == "" || bad.Artifacts.Diff == "" {
		t.Fatalf("mismatch case = %+v", bad)
	}
	if len(res.Run.Clusters) == 0 {
		t.Fatal("no clusters")
	}
	outcomes, rebuilt, err := MinimizeRun(context.Background(), MinimizeOptions{RunDir: res.Run.Layout.Dir, RepoRoot: opts.RepoRoot, A: a, B: b, Budget: 80, Logf: t.Logf})
	if err != nil {
		t.Fatalf("MinimizeRun: %v", err)
	}
	if len(outcomes) != 1 || outcomes[0].Error != "" || outcomes[0].Report == nil || outcomes[0].CaseID != caseOracle {
		t.Fatalf("outcomes = %+v", outcomes)
	}
	rep := outcomes[0].Report
	if rep.After.BodyBytes >= rep.Before.BodyBytes || rep.Executions == 0 {
		t.Errorf("report = %+v", rep)
	}
	min := mustCase(t, rebuilt, caseOracle)
	if min.Artifacts.MinEnvelope == "" || min.Artifacts.MinReport == "" || min.Artifacts.MinCase == "" {
		t.Fatalf("minimized artifacts = %+v", min.Artifacts)
	}
	var env parity.Envelope
	if err := report.ReadJSON(rebuilt.Layout.Resolve(min.Artifacts.MinEnvelope), &env); err != nil {
		t.Fatalf("reading the minimized envelope: %v", err)
	}
	body, err := env.Request.Body.WireBytes()
	if err != nil {
		t.Fatalf("minimized body: %v", err)
	}
	if !strings.Contains(string(body), fakeTrigger) || strings.Contains(string(body), "example.test") {
		t.Errorf("minimized body = %s", body)
	}
}

func TestRunNotInstrumented(t *testing.T) {
	tests := []struct {
		name       string
		b          func(t *testing.T) Impl
		wantReason string
	}{
		{"server without parity routes", func(t *testing.T) Impl { return fakeImpl(t, parity.ImplRust, fakeNYI) }, "501"},
		{"no binary", func(*testing.T) Impl { return Impl{Name: parity.ImplRust} }, "no rust server binary"},
		{"wrong implementation", func(t *testing.T) Impl {
			impl := fakeImpl(t, parity.ImplRust, fakeGo)
			impl.Env = append(impl.Env, fakeImplEnv+"="+parity.ImplGo)
			return impl
		}, `reports impl "go"`},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			opts := testOptions(t, fakeImpl(t, parity.ImplGo, fakeGo), tc.b(t), corpus.CorpusSampleRequests, corpus.CorpusAdapters)
			opts.Shards = 1
			res, err := Run(context.Background(), opts)
			if err != nil {
				t.Fatalf("Run: %v", err)
			}
			c := res.Run.Summary.Counts
			if c.NotYetInstrumented != 3 || c.Skipped != 1 || c.Compared() != 0 || c.Error != 0 {
				t.Errorf("counts = %+v", c)
			}
			for _, id := range []string{caseOracle, caseInline, caseAdapter} {
				if cs := mustCase(t, res.Run, id); !strings.Contains(cs.Reason, tc.wantReason) {
					t.Errorf("%s reason = %q, want %q", id, cs.Reason, tc.wantReason)
				}
			}
			if v := Judge(res.Run); v.OK || !v.Incomplete {
				t.Errorf("verdict = %+v", v)
			}
			wantFiles(t, res.Run.Layout.Dir, report.FileIndex, report.FileJUnit, "matrix.json")
		})
	}
}

// TestRunOffContractTrace: a side whose trace does not validate against the
// trace schema is a harness error, never a passing case, even though the
// decoded trace (which drops the unknown member) equals the other side's.
func TestRunOffContractTrace(t *testing.T) {
	opts := testOptions(t, fakeImpl(t, parity.ImplGo, fakeGo), fakeImpl(t, parity.ImplRust, fakeOffContract), corpus.CorpusSampleRequests)
	res, err := Run(context.Background(), opts)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	if c := res.Run.Summary.Counts; c.Passing != 0 || c.Mismatch != 0 || c.Error != 2 {
		t.Fatalf("counts = %+v", c)
	}
	for _, id := range []string{caseOracle, caseInline} {
		cs := mustCase(t, res.Run, id)
		if cs.Status != report.CaseError || !strings.Contains(cs.Reason, "does not validate") || !strings.Contains(cs.Reason, "http_outs") {
			t.Errorf("%s = %s (%q)", id, cs.Status, cs.Reason)
		}
		if cs.Artifacts.TraceB == "" {
			t.Errorf("%s: the off-contract trace was not kept: %+v", id, cs.Artifacts)
		}
	}
	if v := Judge(res.Run); v.OK {
		t.Errorf("verdict = %+v", v)
	}
}

func TestRunBudget(t *testing.T) {
	opts := testOptions(t, fakeImpl(t, parity.ImplGo, fakeGo), fakeImpl(t, parity.ImplRust, fakeGo), "all")
	opts.Budget = time.Nanosecond
	res, err := Run(context.Background(), opts)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	c := res.Run.Summary.Counts
	if c.Skipped != c.Total || c.Total != 5 || len(res.Run.Summary.Servers) != 0 {
		t.Errorf("counts = %+v, servers = %d", c, len(res.Run.Summary.Servers))
	}
	if cs := mustCase(t, res.Run, caseOracle); cs.Reason != reasonBudget {
		t.Errorf("reason = %q", cs.Reason)
	}
}

// TestRunReportsNotCompared: the requests a corpus does not compare are
// reported under every base profile with status not-compared and the
// reason "<tag>: <reason>": counted in run.json, listed in summary.md,
// skipped in junit.xml, with a case summary and no index entry. They count
// as neither compared nor skipped, so a budget that skips every case leaves
// them as they are.
func TestRunReportsNotCompared(t *testing.T) {
	opts := testOptions(t, fakeImpl(t, parity.ImplGo, fakeGo), fakeImpl(t, parity.ImplRust, fakeGo), "all")
	opts.Profiles = []string{DefaultProfile, "floors"}
	opts.Budget = time.Nanosecond
	opts.Corpus.NotCompared = []corpus.NotCompared{{
		ID: "parity/replayharness#post:/cookie_sync:0123456789abcdef", Corpus: corpus.CorpusReplayHarness, Kind: corpus.KindEndpoint,
		FixturePath: corpus.RootReplayHarness, Endpoint: "/cookie_sync POST", Request: "POST /cookie_sync",
		Sources: []string{"traffic.jsonl:7"}, Tag: corpus.RHMapOrder, Reason: "Go's answer lists syncers in map order",
	}}
	res, err := Run(context.Background(), opts)
	if err != nil {
		t.Fatalf("Run: %v", err)
	}
	c := res.Run.Summary.Counts
	if c.NotCompared != 2 || c.Skipped != c.Total-2 || c.Compared() != 0 {
		t.Errorf("counts = %+v", c)
	}
	reason := corpus.RHMapOrder + ": Go's answer lists syncers in map order"
	for _, id := range []string{opts.Corpus.NotCompared[0].ID, opts.Corpus.NotCompared[0].ID + "@floors"} {
		cs := mustCase(t, res.Run, id)
		if cs.Status != report.CaseNotCompared || cs.Reason != reason || cs.Corpus != corpus.CorpusReplayHarness || cs.Endpoint != "/cookie_sync POST" ||
			!slices.Equal(cs.Notes, []string{"request POST /cookie_sync", "from traffic.jsonl:7"}) {
			t.Errorf("%s = %+v", id, cs)
		}
	}
	for _, e := range res.Run.Index.Cases {
		if strings.HasPrefix(e.CaseID, corpus.RootReplayHarness) {
			t.Errorf("index.json has an entry for the not-compared request %s", e.CaseID)
		}
	}
	files := map[string]string{report.FileRun: `"not_compared": 2`, report.FileSummaryMD: "| 2 | " + reason + " |", report.FileJUnit: `message="not-compared: ` + reason}
	for name, want := range files {
		raw, err := os.ReadFile(res.Run.Layout.Path(name))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(html.UnescapeString(string(raw)), want) {
			t.Errorf("%s lacks %q:\n%s", name, want, raw)
		}
	}
	if v := Judge(res.Run); v.OK || !v.Incomplete || !strings.Contains(strings.Join(v.Reasons, "; "), "2 not compared") {
		t.Errorf("verdict = %+v", v)
	}
}

func TestIgnoreOutputRoot(t *testing.T) {
	tests := []struct {
		name string
		path string
		want bool
	}{
		{"run dir", "parity/out/run-1", true},
		{"go build", "parity/out/bin/pbs-go", true},
		{"outside", "elsewhere/run-1", false},
		{"sibling with the same prefix", "parity/outside/run-1", false},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			repo := t.TempDir()
			ignoreOutputRoot(repo, filepath.Join(repo, filepath.FromSlash(tc.path)))
			raw, err := os.ReadFile(filepath.Join(repo, "parity", "out", ".gitignore"))
			if got := err == nil && string(raw) == "*\n"; got != tc.want {
				t.Errorf("ignored = %v (err %v), want %v", got, err, tc.want)
			}
		})
	}
}

func TestJudge(t *testing.T) {
	tests := []struct {
		name           string
		counts         report.Counts
		fixtures       *report.FixtureTotals
		ok, incomplete bool
	}{
		{"clean", report.Counts{Total: 2, Passing: 2}, nil, true, false},
		{"mismatch", report.Counts{Total: 2, Passing: 1, Mismatch: 1}, nil, false, false},
		{"harness error", report.Counts{Total: 2, Passing: 1, Error: 1}, nil, false, false},
		{"failing fixture", report.Counts{Total: 1, Passing: 1}, &report.FixtureTotals{Failing: 1}, false, false},
		{"nothing compared", report.Counts{Total: 2, NotYetInstrumented: 2}, nil, false, true},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			run := &report.Run{Summary: report.RunSummary{Counts: tc.counts, Fixtures: tc.fixtures}}
			v := Judge(run)
			if v.OK != tc.ok || v.Incomplete != tc.incomplete || (tc.ok && len(v.Reasons) != 0) {
				t.Errorf("verdict = %+v", v)
			}
		})
	}
}
