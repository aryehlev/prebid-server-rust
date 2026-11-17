package matrix

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/diff"
	"github.com/prebid/prebid-server/v4/parity/report"
)

type testCase struct {
	id, kind, bidder string
	status           report.CaseStatus
	exercised        []parity.Domain
	stagesA, stagesB map[parity.StageID]parity.StageStatus
	mismatches       []parity.MismatchRecord
	tags             []string
}

func mismatch(caseID string, stage parity.StageID, domain parity.Domain, path string, primary bool) parity.MismatchRecord {
	r := parity.MismatchRecord{
		CaseID: caseID, FixturePath: "f/" + caseID, Endpoint: "/openrtb2/auction", Profile: "default",
		Stage: stage, Key: "_", Domain: domain, Requirement: "AUCT-01", Kind: parity.KindValue, JSONPath: path,
		Go: json.RawMessage(`{"imp":[{"id":"1"}]}`), Rust: json.RawMessage(`{"imp":[]}`), Primary: primary,
		Signature: diff.Signature(stage, domain, path, parity.KindValue),
	}
	if !primary {
		of := "root"
		r.ConsequentialOf = &of
	}
	return r
}

// writeRun writes a run of cases under a temporary repository root.
func writeRun(t *testing.T, cases []testCase, fixtures []report.FixtureResult) (*report.Run, string) {
	t.Helper()
	base := t.TempDir()
	layout, err := report.NewLayout(filepath.Join(base, "parity", "out", "run"), base)
	if err != nil {
		t.Fatalf("NewLayout: %v", err)
	}
	w, err := report.NewWriter(layout)
	if err != nil {
		t.Fatalf("NewWriter: %v", err)
	}
	for _, c := range cases {
		s := report.NewCaseSummary(c.id)
		s.FixturePath, s.Endpoint, s.Profile, s.Kind, s.Corpus, s.Bidder = "f/"+c.id, "/openrtb2/auction", "default", c.kind, c.kind, c.bidder
		s.ImplA, s.ImplB, s.Status, s.StagesA, s.StagesB = "go", "rust", c.status, c.stagesA, c.stagesB
		s.ExercisedDomains = append(s.ExercisedDomains, c.exercised...)
		s.Tags = append(s.Tags, c.tags...)
		var doc *parity.DiffDocument
		if c.status == "" {
			doc = &parity.DiffDocument{Schema: parity.SchemaDiff, SchemaVersion: parity.SchemaVersion, CaseID: c.id, Mismatches: append([]parity.MismatchRecord{}, c.mismatches...)}
		} else {
			s.Reason = "health answered 501"
		}
		if err := w.AddCase(s, doc); err != nil {
			t.Fatalf("AddCase: %v", err)
		}
	}
	w.AddFixtures(fixtures)
	run, err := w.Finish(report.RunSummary{RunID: "run-1", Mode: report.ModeRun, ImplA: "go", ImplB: "rust"})
	if err != nil {
		t.Fatalf("Finish: %v", err)
	}
	return run, base
}

func mixedRun(t *testing.T) (*report.Run, string) {
	captured := map[parity.StageID]parity.StageStatus{parity.StageHTTPIn: parity.StageCaptured, parity.StageRequestValidated: parity.StageCaptured, parity.StageAdapterRequests: parity.StageCaptured}
	partialB := map[parity.StageID]parity.StageStatus{parity.StageHTTPIn: parity.StageCaptured, parity.StageRequestValidated: parity.StageCaptured, parity.StageAdapterRequests: parity.StageUnimplemented}
	return writeRun(t, []testCase{
		{id: "auction-pass", kind: "endpoint", exercised: []parity.Domain{parity.DomainEndpoints, parity.DomainAuctionLifecycle}, stagesA: captured, stagesB: captured},
		{id: "auction-mismatch", kind: "endpoint", exercised: []parity.Domain{parity.DomainEndpoints, parity.DomainAuctionLifecycle}, stagesA: captured, stagesB: partialB, mismatches: []parity.MismatchRecord{
			mismatch("auction-mismatch", parity.StageRequestValidated, parity.DomainAuctionLifecycle, "$.tmax", true),
			mismatch("auction-mismatch", parity.StageAdapterRequests, parity.DomainBiddersAdapters, "$.requests", false),
		}},
		{id: "adapter-appnexus-1", kind: "adapter", bidder: "appnexus", exercised: []parity.Domain{parity.DomainBiddersAdapters}},
		{id: "adapter-appnexus-2", kind: "adapter", bidder: "appnexus", status: report.CaseNotYetInstrumented, tags: []string{"needs_fake"}},
		{id: "lifted-rubicon-1", kind: "lifted_auction", bidder: "rubicon", status: report.CaseNotYetInstrumented},
	}, []report.FixtureResult{{CaseID: "adapter-appnexus-1", Bidder: "appnexus", Status: report.FixturePassing, Failures: []string{}}})
}

func TestDomainsLockedOrder(t *testing.T) {
	domains, err := Domains()
	if err != nil {
		t.Fatalf("Domains: %v", err)
	}
	locked := parity.AllDomains()
	if len(domains) != 9 || len(locked) != 9 {
		t.Fatalf("got %d domains, %d locked; want 9", len(domains), len(locked))
	}
	for i, d := range domains {
		if d.Domain != locked[i] || d.Title == "" || d.Surface == "" {
			t.Errorf("row %d = %+v, want domain %s with title and surface", i, d, locked[i])
		}
	}
}

func TestBuildMatrixStatuses(t *testing.T) {
	run, _ := mixedRun(t)
	m, err := BuildMatrix(run)
	if err != nil {
		t.Fatalf("BuildMatrix: %v", err)
	}
	want := map[parity.Domain]parity.SurfaceStatus{
		parity.DomainEndpoints:             parity.StatusPassing,
		parity.DomainAuctionLifecycle:      parity.StatusMismatch,
		parity.DomainBiddersAdapters:       parity.StatusMismatch,
		parity.DomainConfigBootstrap:       parity.StatusNotYetInstrumented,
		parity.DomainStoredData:            parity.StatusNotYetInstrumented,
		parity.DomainPolicyPrivacyIdentity: parity.StatusNotYetInstrumented,
		parity.DomainHooksModules:          parity.StatusNotYetInstrumented,
		parity.DomainCacheSupportFlows:     parity.StatusNotYetInstrumented,
		parity.DomainCutoverProof:          parity.StatusNotYetInstrumented,
	}
	for i, r := range m.Rows {
		if r.Domain != parity.AllDomains()[i] {
			t.Errorf("row %d is %s, out of locked order", i, r.Domain)
		}
		if r.Status != want[r.Domain] {
			t.Errorf("%s status = %s, want %s", r.Domain, r.Status, want[r.Domain])
		}
		if r.Status == parity.StatusNotYetInstrumented && (r.Exercised != 0 || len(r.EvidenceLinks) != 0) {
			t.Errorf("%s is not-yet-instrumented with exercised=%d links=%v", r.Domain, r.Exercised, r.EvidenceLinks)
		}
	}
	lifecycle := m.Rows[3]
	if lifecycle.Exercised != 2 || lifecycle.Mismatching != 1 || lifecycle.PrimaryClusters != 1 {
		t.Errorf("auction_lifecycle row = %+v", lifecycle)
	}
	link := lifecycle.EvidenceLinks[0]
	if link.CaseID != "auction-mismatch" || link.Cluster != diff.Signature(parity.StageRequestValidated, parity.DomainAuctionLifecycle, "$.tmax", parity.KindValue) || !strings.HasSuffix(link.ArtifactPath, "/diff.json") {
		t.Errorf("auction_lifecycle evidence = %+v", link)
	}
}

func TestBuildMatrixTables(t *testing.T) {
	run, _ := mixedRun(t)
	m, err := BuildMatrix(run)
	if err != nil {
		t.Fatalf("BuildMatrix: %v", err)
	}
	if len(m.Endpoints) != 1 || m.Endpoints[0].Cases != 5 || m.Endpoints[0].Status != parity.StatusMismatch || m.Endpoints[0].NotYetInstrumented != 2 {
		t.Errorf("endpoints = %+v", m.Endpoints)
	}
	if len(m.Bidders) != 2 {
		t.Fatalf("bidders = %+v", m.Bidders)
	}
	an, rb := m.Bidders[0], m.Bidders[1]
	if an.Bidder != "appnexus" || an.AdapterCases != 2 || an.AdapterCompared != 1 || an.AdapterPassPct == nil || *an.AdapterPassPct != 100 || an.NeedsFake != 1 || an.FixturesPassing != 1 || an.Status != parity.StatusPassing {
		t.Errorf("appnexus = %+v", an)
	}
	if rb.Bidder != "rubicon" || rb.LiftedCases != 1 || rb.LiftedPassPct != nil || rb.Status != parity.StatusNotYetInstrumented {
		t.Errorf("rubicon = %+v", rb)
	}
	if len(m.Stages) != len(parity.AllStages()) {
		t.Fatalf("%d stage rows, want %d", len(m.Stages), len(parity.AllStages()))
	}
	byStage := map[parity.StageID]StageRow{}
	for _, s := range m.Stages {
		byStage[s.Stage] = s
	}
	if s := byStage[parity.StageRequestValidated]; s.CapturedA != 2 || s.CapturedB != 2 || s.Status != StageCaptured || s.FirstDivergent != 1 || s.MismatchCases != 1 {
		t.Errorf("request.validated = %+v", s)
	}
	if s := byStage[parity.StageAdapterRequests]; s.Status != StagePartial || s.UnimplementedB != 1 {
		t.Errorf("adapter.requests = %+v", s)
	}
	if s := byStage[parity.StageTargeting]; s.Status != StageNotExercised {
		t.Errorf("targeting = %+v", s)
	}
}

func TestAllNotYetInstrumented(t *testing.T) {
	run, _ := writeRun(t, []testCase{
		{id: "c1", kind: "endpoint", status: report.CaseNotYetInstrumented},
		{id: "c2", kind: "adapter", bidder: "appnexus", status: report.CaseNotYetInstrumented},
	}, nil)
	m, err := BuildMatrix(run)
	if err != nil {
		t.Fatalf("BuildMatrix: %v", err)
	}
	for _, r := range m.Rows {
		if r.Status != parity.StatusNotYetInstrumented || !strings.Contains(r.Summary, "2 cases were not run") {
			t.Errorf("%s = %s %q", r.Domain, r.Status, r.Summary)
		}
	}
}

func TestValidate(t *testing.T) {
	run, _ := mixedRun(t)
	tests := []struct {
		name   string
		mutate func(m *Matrix)
	}{
		{"passing row without evidence", func(m *Matrix) { m.Rows[0].EvidenceLinks = nil }},
		{"mismatch row without evidence", func(m *Matrix) { m.Rows[3].EvidenceLinks = []parity.EvidenceLink{} }},
		{"incomplete evidence link", func(m *Matrix) { m.Rows[0].EvidenceLinks[0].SummaryPath = "" }},
		{"rows out of order", func(m *Matrix) { m.Rows[0], m.Rows[1] = m.Rows[1], m.Rows[0] }},
		{"missing row", func(m *Matrix) { m.Rows = m.Rows[:8] }},
		{"unknown status", func(m *Matrix) { m.Rows[1].Status = "partial" }},
		{"empty summary", func(m *Matrix) { m.Rows[1].Summary = " " }},
		{"payload in summary", func(m *Matrix) { m.Rows[1].Summary = `{"imp":[{"id":"1"}]}` }},
		{"payload in evidence", func(m *Matrix) { m.Rows[0].EvidenceLinks[0].Cluster = `"imp": []` }},
		{"multi-line summary", func(m *Matrix) { m.Rows[1].Summary = "a\nb" }},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			m, err := BuildMatrix(run)
			if err != nil {
				t.Fatalf("BuildMatrix: %v", err)
			}
			tc.mutate(m)
			if err := m.Validate(); err == nil {
				t.Error("Validate accepted the matrix")
			}
			if _, err := EncodeStatus(m); err == nil {
				t.Error("EncodeStatus accepted the matrix")
			}
		})
	}
}

func TestSnapshotHasNoPayloads(t *testing.T) {
	run, base := mixedRun(t)
	m, err := BuildFromRun(run.Layout.Dir)
	if err != nil {
		t.Fatalf("BuildFromRun: %v", err)
	}
	direct, err := BuildMatrix(run)
	if err != nil {
		t.Fatalf("BuildMatrix: %v", err)
	}
	a, _ := EncodeStatus(m)
	b, _ := EncodeStatus(direct)
	if string(a) != string(b) {
		t.Errorf("BuildFromRun differs from BuildMatrix:\n%s\n%s", a, b)
	}
	dir := t.TempDir()
	if err := WriteSnapshot(m, dir); err != nil {
		t.Fatalf("WriteSnapshot: %v", err)
	}
	status, err := os.ReadFile(filepath.Join(dir, FileStatus))
	if err != nil {
		t.Fatalf("reading status.json: %v", err)
	}
	if strings.Contains(string(status), `"imp"`) {
		t.Errorf("status.json embeds a payload:\n%s", status)
	}
	readme, err := os.ReadFile(filepath.Join(dir, FileReadme))
	if err != nil {
		t.Fatalf("reading README.md: %v", err)
	}
	for _, d := range parity.AllDomains() {
		if !strings.Contains(string(readme), "(`"+string(d)+"`)") {
			t.Errorf("README.md has no row for %s", d)
		}
	}
	if strings.Contains(string(readme), `"imp"`) {
		t.Error("README.md embeds a payload")
	}
	// The index the matrix cites validates against the Phase-1 index fields.
	if m.Index != "parity/out/run/index.json" {
		t.Errorf("index = %q", m.Index)
	}
	raw, err := os.ReadFile(filepath.Join(base, filepath.FromSlash(m.Index)))
	if err != nil {
		t.Fatalf("reading index: %v", err)
	}
	if err := report.ValidateIndex(raw); err != nil {
		t.Errorf("index.json: %v", err)
	}
	if err := WriteRunFiles(m, run.Layout.Dir); err != nil {
		t.Fatalf("WriteRunFiles: %v", err)
	}
	if _, err := os.Stat(run.Layout.Path(FileRunMatrixMD)); err != nil {
		t.Errorf("matrix.md not written: %v", err)
	}
}

func TestBuildMatrixNilRun(t *testing.T) {
	if _, err := BuildMatrix(nil); err == nil {
		t.Error("BuildMatrix(nil) succeeded")
	}
	if _, err := BuildFromRun(filepath.Join(t.TempDir(), "missing")); err == nil {
		t.Error("BuildFromRun of a missing directory succeeded")
	}
}

// TestCheckedInSnapshot keeps status.json and README.md valid and in sync:
// README.md must be the rendering of status.json.
func TestCheckedInSnapshot(t *testing.T) {
	raw, err := os.ReadFile(FileStatus)
	if os.IsNotExist(err) {
		t.Skip("no checked-in snapshot")
	}
	if err != nil {
		t.Fatalf("reading %s: %v", FileStatus, err)
	}
	var m Matrix
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatalf("decoding %s: %v", FileStatus, err)
	}
	enc, err := EncodeStatus(&m)
	if err != nil {
		t.Fatalf("%s: %v", FileStatus, err)
	}
	if string(enc) != string(raw) {
		t.Errorf("%s is not in canonical form; regenerate it with parity matrix --run <dir>", FileStatus)
	}
	md, err := os.ReadFile(FileReadme)
	if err != nil {
		t.Fatalf("reading %s: %v", FileReadme, err)
	}
	if string(md) != string(RenderMarkdown(&m)) {
		t.Errorf("%s does not match %s; regenerate both with parity matrix --run <dir>", FileReadme, FileStatus)
	}
}
