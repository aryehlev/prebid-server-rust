package report

import (
	"encoding/json"
	"encoding/xml"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/diff"
)

// record builds one mismatch record with its real signature.
func record(caseID string, stage parity.StageID, domain parity.Domain, path string, kind parity.MismatchKind, primary bool) parity.MismatchRecord {
	r := parity.MismatchRecord{
		CaseID: caseID, FixturePath: "fixtures/" + caseID + ".json", Endpoint: "/openrtb2/auction", Profile: "default",
		Stage: stage, Key: "_", Domain: domain, Requirement: "AUCT-01", Kind: kind, JSONPath: path,
		Go: json.RawMessage(`1`), Rust: json.RawMessage(`2`), Primary: primary,
		Signature: diff.Signature(stage, domain, path, kind),
	}
	if !primary {
		of := "upstream"
		r.ConsequentialOf = &of
	}
	return r
}

func diffDoc(caseID string, records ...parity.MismatchRecord) *parity.DiffDocument {
	return &parity.DiffDocument{
		Schema: parity.SchemaDiff, SchemaVersion: parity.SchemaVersion, CaseID: caseID,
		FixturePath: "fixtures/" + caseID + ".json", Endpoint: "/openrtb2/auction", Profile: "default",
		Mismatches: append([]parity.MismatchRecord{}, records...),
	}
}

func summary(caseID string, status CaseStatus, exercised ...parity.Domain) CaseSummary {
	s := NewCaseSummary(caseID)
	s.FixturePath, s.Endpoint, s.Profile, s.Kind, s.Corpus = "fixtures/"+caseID+".json", "/openrtb2/auction", "default", "endpoint", "endpoints"
	s.ImplA, s.ImplB, s.Status = "go", "rust", status
	s.ExercisedDomains = append(s.ExercisedDomains, exercised...)
	if !status.Compared() {
		s.Reason = "rust does not serve /__parity/exec"
	}
	return s
}

// notCompareReason is the reason of the sample run's not-compared request.
const notCompareReason = "truncated_body: the recorder cut the body at 65536 bytes, so it is not the body that was sent"

// notCompared is the summary the runner writes for a request a corpus does
// not compare.
func notCompared(caseID string) CaseSummary {
	s := summary(caseID, CaseNotCompared)
	s.Corpus, s.Endpoint, s.Reason = "replayharness", "/cookie_sync POST", notCompareReason
	return s
}

// sampleRun writes a run with one case of every status under base/out/run.
func sampleRun(t *testing.T) (*Run, string) {
	t.Helper()
	base := t.TempDir()
	layout, err := NewLayout(filepath.Join(base, "out", "run"), base)
	if err != nil {
		t.Fatalf("NewLayout: %v", err)
	}
	w, err := NewWriter(layout)
	if err != nil {
		t.Fatalf("NewWriter: %v", err)
	}
	cases := []struct {
		s   CaseSummary
		doc *parity.DiffDocument
	}{
		{summary("a-pass", "", parity.DomainEndpoints), diffDoc("a-pass")},
		{summary("b-mismatch", "", parity.DomainEndpoints, parity.DomainAuctionLifecycle), diffDoc("b-mismatch",
			record("b-mismatch", parity.StageRequestValidated, parity.DomainAuctionLifecycle, "$.imp[0].id", parity.KindValue, true),
			record("b-mismatch", parity.StageHTTPResponse, parity.DomainEndpoints, "$.body.seatbid", parity.KindMissingInRust, false))},
		{summary("c-mismatch", "", parity.DomainAuctionLifecycle), diffDoc("c-mismatch",
			record("c-mismatch", parity.StageRequestValidated, parity.DomainAuctionLifecycle, "$.imp[3].id", parity.KindValue, true))},
		{summary("d-nyi", CaseNotYetInstrumented), nil},
		{summary("e-error", CaseError), nil},
		{summary("f-skipped", CaseSkipped), nil},
		{notCompared("g-not-compared"), nil},
	}
	for _, c := range cases {
		if err := w.AddCase(c.s, c.doc); err != nil {
			t.Fatalf("AddCase %s: %v", c.s.CaseID, err)
		}
	}
	w.AddFixtures([]FixtureResult{
		{CaseID: "fx-1", Bidder: "appnexus", Status: FixturePassing, Failures: []string{}},
		{CaseID: "fx-2", Bidder: "appnexus", Status: FixtureFailing, Failures: []string{"mockResponse 0: httpCalls[0].expectedRequest.body differs"}},
		{CaseID: "fx-3", Bidder: "adnuntius", Status: FixtureExcused, Excuse: "needs_fake", Failures: []string{"uuid differs"}},
	})
	run, err := w.Finish(RunSummary{RunID: "run-1", Mode: ModeRun, ImplA: "go", ImplB: "rust", StartedAt: "2026-01-01T00:00:00Z"})
	if err != nil {
		t.Fatalf("Finish: %v", err)
	}
	return run, base
}

func TestFinishWritesRunDocuments(t *testing.T) {
	run, base := sampleRun(t)
	want := Counts{Total: 7, Passing: 1, Mismatch: 2, NotYetInstrumented: 1, Error: 1, Skipped: 1, NotCompared: 1}
	if run.Summary.Counts != want {
		t.Errorf("counts = %+v, want %+v", run.Summary.Counts, want)
	}
	if run.Summary.Mismatches != 3 || run.Summary.MismatchesByKind["value"] != 2 || run.Summary.PrimaryClusters != 1 || run.Summary.Clusters != 2 {
		t.Errorf("summary totals = %+v", run.Summary)
	}
	if f := run.Summary.Fixtures; f == nil || f.Total != 3 || f.Passing != 1 || f.Failing != 1 || f.Excused != 1 {
		t.Errorf("fixture totals = %+v", f)
	}
	for _, name := range []string{FileIndex, FileClusters, FileRun, FileSummaryMD, FileJUnit, FileFixtures} {
		if _, err := os.Stat(run.Layout.Path(name)); err != nil {
			t.Errorf("%s not written: %v", name, err)
		}
	}
	raw, err := os.ReadFile(run.Layout.Path(FileIndex))
	if err != nil {
		t.Fatalf("reading index: %v", err)
	}
	if err := ValidateIndex(raw); err != nil {
		t.Errorf("index.json does not validate: %v", err)
	}
	var ids []string
	for _, e := range run.Index.Cases {
		ids = append(ids, e.CaseID)
		if strings.HasPrefix(e.SummaryPath, "/") || !strings.HasPrefix(e.SummaryPath, "out/run/cases/") {
			t.Errorf("%s summary_path %q is not relative to the base", e.CaseID, e.SummaryPath)
		}
		if _, err := os.Stat(filepath.Join(base, filepath.FromSlash(e.SummaryPath))); err != nil {
			t.Errorf("%s summary_path does not resolve: %v", e.CaseID, err)
		}
	}
	if want := []string{"a-pass", "b-mismatch", "c-mismatch", "d-nyi", "e-error"}; !reflect.DeepEqual(ids, want) {
		t.Errorf("index cases = %v, want %v (skipped and not-compared cases have no entry)", ids, want)
	}
	if g, ok := run.Case("g-not-compared"); !ok || g.Status != CaseNotCompared || g.Reason != notCompareReason || g.Artifacts.Summary == "" {
		t.Errorf("g-not-compared summary = %+v", g)
	}
	var onDisk RunSummary
	if err := ReadJSON(run.Layout.Path(FileRun), &onDisk); err != nil || onDisk.Counts.NotCompared != 1 {
		t.Errorf("run.json counts = %+v (%v), want not_compared 1", onDisk.Counts, err)
	}
	b, _ := run.Case("b-mismatch")
	if b.Status != CaseMismatch || b.FirstStage != parity.StageRequestValidated || b.PrimaryDomain != parity.DomainAuctionLifecycle || b.Primary != 1 || b.Mismatches != 2 {
		t.Errorf("b-mismatch summary = %+v", b)
	}
	if got := run.Index.Cases[1]; got.Stage != b.FirstStage || got.Cluster != b.Cluster || !reflect.DeepEqual(got.Domains, []parity.Domain{parity.DomainEndpoints, parity.DomainAuctionLifecycle}) {
		t.Errorf("index entry = %+v", got)
	}
	if a, _ := run.Case("a-pass"); a.Status != CasePassing || a.Artifacts.Diff == "" {
		t.Errorf("a-pass summary = %+v", a)
	}
}

func TestClusterRepresentatives(t *testing.T) {
	run, _ := sampleRun(t)
	var primary *ClusterEntry
	for i := range run.Clusters {
		if run.Clusters[i].Primary() {
			primary = &run.Clusters[i]
		}
	}
	if primary == nil {
		t.Fatal("no primary cluster")
	}
	if primary.Representative != "b-mismatch" || !reflect.DeepEqual(primary.Cases, []string{"b-mismatch", "c-mismatch"}) || primary.CaseDir == "" {
		t.Errorf("primary cluster = %+v", primary)
	}
}

func TestLoadRunRoundTrip(t *testing.T) {
	run, _ := sampleRun(t)
	loaded, err := LoadRun(run.Layout.Dir)
	if err != nil {
		t.Fatalf("LoadRun: %v", err)
	}
	if loaded.Layout != run.Layout {
		t.Errorf("layout = %+v, want %+v", loaded.Layout, run.Layout)
	}
	for name, pair := range map[string][2]any{
		"cases": {loaded.Cases, run.Cases}, "diffs": {loaded.Diffs, run.Diffs}, "clusters": {loaded.Clusters, run.Clusters},
		"index": {loaded.Index, run.Index}, "fixtures": {loaded.Fixtures, run.Fixtures}, "summary": {loaded.Summary, run.Summary},
	} {
		got, _ := json.Marshal(pair[0])
		want, _ := json.Marshal(pair[1])
		if string(got) != string(want) {
			t.Errorf("%s differ after LoadRun:\n%s\n%s", name, got, want)
		}
	}
}

func TestRebuildPicksUpMinimizedRepro(t *testing.T) {
	run, _ := sampleRun(t)
	dir := run.Layout.Path(filepath.FromSlash(CaseDirName("b-mismatch")))
	if err := WriteJSON(filepath.Join(dir, FileMinEnvelope), map[string]string{"case_id": "b-mismatch"}); err != nil {
		t.Fatalf("writing min envelope: %v", err)
	}
	rebuilt, err := Rebuild(run.Layout.Dir)
	if err != nil {
		t.Fatalf("Rebuild: %v", err)
	}
	if rebuilt.Summary.FinishedAt != run.Summary.FinishedAt {
		t.Errorf("finished_at changed: %q -> %q", run.Summary.FinishedAt, rebuilt.Summary.FinishedAt)
	}
	var found string
	for _, c := range rebuilt.Clusters {
		if c.Representative == "b-mismatch" {
			found = c.MinEnvelope
		}
	}
	if !strings.HasSuffix(found, CaseDirName("b-mismatch")+"/"+FileMinEnvelope) {
		t.Errorf("cluster min_envelope = %q", found)
	}
	md, err := os.ReadFile(rebuilt.Layout.Path(FileSummaryMD))
	if err != nil {
		t.Fatalf("reading summary.md: %v", err)
	}
	if !strings.Contains(string(md), found) {
		t.Errorf("summary.md does not link the minimized repro %q", found)
	}
}

func TestRenderSummary(t *testing.T) {
	run, _ := sampleRun(t)
	md := string(RenderSummary(run))
	for _, d := range parity.AllDomains() {
		if !strings.Contains(md, "### "+string(d)+"\n") {
			t.Errorf("summary.md has no section for %s", d)
		}
	}
	for _, want := range []string{"| mismatch | 2 |", "value 2", "## Cases not compared", "rust does not serve", "## Adapter fixture self-test", "`fx-2`",
		"| not-compared | 1 |", "## Requests not compared", "| 1 | " + notCompareReason + " |"} {
		if !strings.Contains(md, want) {
			t.Errorf("summary.md lacks %q", want)
		}
	}
	_, reasons, _ := strings.Cut(md, "## Cases not compared")
	reasons, _, _ = strings.Cut(reasons, "## Requests not compared")
	if strings.Contains(reasons, "truncated_body") {
		t.Errorf("the not-compared request is also among the cases not compared:\n%s", reasons)
	}
}

func TestJUnit(t *testing.T) {
	run, _ := sampleRun(t)
	raw, err := JUnit(run)
	if err != nil {
		t.Fatalf("JUnit: %v", err)
	}
	var doc junitSuites
	if err := xml.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("junit.xml is not well-formed: %v\n%s", err, raw)
	}
	if doc.Tests != 10 || doc.Failures != 3 || doc.Errors != 1 || doc.Skipped != 4 {
		t.Errorf("totals tests=%d failures=%d errors=%d skipped=%d", doc.Tests, doc.Failures, doc.Errors, doc.Skipped)
	}
	if len(doc.Suites) != 3 || doc.Suites[0].Name != "endpoints" || doc.Suites[1].Name != SuiteFixtures || doc.Suites[2].Name != "replayharness" {
		t.Fatalf("suites = %+v", doc.Suites)
	}
	if c := doc.Suites[2].Cases; len(c) != 1 || c[0].Skipped == nil || c[0].Skipped.Message != "not-compared: "+notCompareReason {
		t.Errorf("not-compared test case = %+v", c)
	}
	for _, c := range doc.Suites[0].Cases {
		if c.Name == "b-mismatch" && (c.Failure == nil || !strings.Contains(c.Failure.Message, "first at request.validated") || !strings.Contains(c.Failure.Text, "$.imp[0].id")) {
			t.Errorf("b-mismatch failure = %+v", c.Failure)
		}
	}
}

func TestValidateIndex(t *testing.T) {
	entry := `{"case_id":"c1","fixture_path":"f","endpoint":"/openrtb2/auction","domains":[],"mismatch_kinds":[],"summary_path":"s","artifact_path":"a"}`
	tests := []struct {
		name    string
		doc     string
		wantErr bool
	}{
		{"valid", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[` + entry + `]}`, false},
		{"empty", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[]}`, false},
		{"duplicate case", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[` + entry + `,` + entry + `]}`, true},
		{"missing member", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[{"case_id":"c1","fixture_path":"f","endpoint":"e","domains":[],"mismatch_kinds":[],"summary_path":"s"}]}`, true},
		{"null member", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[` + strings.Replace(entry, `"domains":[]`, `"domains":null`, 1) + `]}`, true},
		{"empty summary path", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[` + strings.Replace(entry, `"summary_path":"s"`, `"summary_path":""`, 1) + `]}`, true},
		{"wrong schema", `{"schema":"pbs-parity-diff/1","schema_version":"1.0.0","cases":[]}`, true},
		{"unknown domain", `{"schema":"pbs-parity-index/1","schema_version":"1.0.0","cases":[` + strings.Replace(entry, `"domains":[]`, `"domains":["imp"]`, 1) + `]}`, true},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			if err := ValidateIndex([]byte(tc.doc)); (err != nil) != tc.wantErr {
				t.Errorf("ValidateIndex err = %v, wantErr %v", err, tc.wantErr)
			}
		})
	}
}

func TestNewLayout(t *testing.T) {
	base := t.TempDir()
	tests := []struct {
		name     string
		dir      string
		wantBase string
		wantRef  string
	}{
		{"inside the base", filepath.Join(base, "parity", "out", "r"), base, "parity/out/r/index.json"},
		{"outside the base", filepath.Join(filepath.Dir(base), "elsewhere"), "", filepath.ToSlash(filepath.Join(filepath.Dir(base), "elsewhere", "index.json"))},
		{"sibling with a common prefix", base + "-other", "", filepath.ToSlash(filepath.Join(base+"-other", "index.json"))},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			l, err := NewLayout(tc.dir, base)
			if err != nil {
				t.Fatalf("NewLayout: %v", err)
			}
			if l.Base != tc.wantBase {
				t.Errorf("base = %q, want %q", l.Base, tc.wantBase)
			}
			ref := l.Ref(l.Path(FileIndex))
			if ref != tc.wantRef {
				t.Errorf("ref = %q, want %q", ref, tc.wantRef)
			}
			if got := l.Resolve(ref); got != l.Path(FileIndex) {
				t.Errorf("resolve = %q, want %q", got, l.Path(FileIndex))
			}
		})
	}
}

func TestCheckRatchet(t *testing.T) {
	base := Baseline{Schema: SchemaBaseline, Compared: 10, Errors: 1, Mismatches: 5, ByKind: map[string]int{"value": 4, "lexical": 1}, Clusters: map[string]int{"s1": 4, "s2": 1}}
	tests := []struct {
		name            string
		mutate          func(b *Baseline)
		wantOK          bool
		wantRegressions int
	}{
		{"unchanged", func(*Baseline) {}, true, 0},
		{"fewer mismatches", func(b *Baseline) {
			b.Mismatches, b.ByKind, b.Clusters = 4, map[string]int{"value": 4}, map[string]int{"s1": 4}
		}, true, 0},
		{"more mismatches", func(b *Baseline) { b.Mismatches, b.ByKind = 6, map[string]int{"value": 5, "lexical": 1} }, false, 2},
		{"new cluster", func(b *Baseline) { b.Clusters = map[string]int{"s1": 4, "s3": 1} }, false, 1},
		{"fewer compared cases", func(b *Baseline) { b.Compared = 2 }, false, 1},
		{"more errors", func(b *Baseline) { b.Errors = 3 }, false, 1},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			cur := base
			cur.ByKind, cur.Clusters = map[string]int{}, map[string]int{}
			for k, v := range base.ByKind {
				cur.ByKind[k] = v
			}
			for k, v := range base.Clusters {
				cur.Clusters[k] = v
			}
			tc.mutate(&cur)
			r := CheckRatchet(base, cur)
			if r.OK != tc.wantOK || len(r.Regressions) != tc.wantRegressions {
				t.Errorf("ratchet = %+v, want ok=%v with %d regressions", r, tc.wantOK, tc.wantRegressions)
			}
		})
	}
}

func TestBaselineOf(t *testing.T) {
	run, _ := sampleRun(t)
	b := BaselineOf(run)
	if b.Mismatches != 3 || b.Compared != 3 || b.Errors != 1 || len(b.Clusters) != 2 || b.ByKind["missing_in_rust"] != 1 {
		t.Errorf("baseline = %+v", b)
	}
	path := filepath.Join(t.TempDir(), "baseline.json")
	if err := WriteJSON(path, b); err != nil {
		t.Fatalf("WriteJSON: %v", err)
	}
	loaded, err := LoadBaseline(path)
	if err != nil {
		t.Fatalf("LoadBaseline: %v", err)
	}
	if !reflect.DeepEqual(loaded, b) {
		t.Errorf("loaded = %+v, want %+v", loaded, b)
	}
	if r := CheckRatchet(b, loaded); !r.OK {
		t.Errorf("baseline regresses against itself: %+v", r)
	}
	if err := WriteJSON(path, map[string]string{"schema": "other"}); err != nil {
		t.Fatalf("WriteJSON: %v", err)
	}
	if _, err := LoadBaseline(path); err == nil {
		t.Error("LoadBaseline accepted another schema")
	}
}
