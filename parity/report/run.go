package report

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/diff"
)

// Run modes recorded in run.json.
const (
	ModeRun      = "run"
	ModeSelfTest = "selftest"
)

// Counts are case totals by status.
type Counts struct {
	Total              int `json:"total"`
	Passing            int `json:"passing"`
	Mismatch           int `json:"mismatch"`
	NotYetInstrumented int `json:"not_yet_instrumented"`
	Error              int `json:"error"`
	Skipped            int `json:"skipped"`
}

// add counts one case.
func (c *Counts) add(s CaseStatus) {
	c.Total++
	switch s {
	case CasePassing:
		c.Passing++
	case CaseMismatch:
		c.Mismatch++
	case CaseNotYetInstrumented:
		c.NotYetInstrumented++
	case CaseError:
		c.Error++
	case CaseSkipped:
		c.Skipped++
	}
}

// Compared is the number of cases both implementations were compared on.
func (c Counts) Compared() int { return c.Passing + c.Mismatch }

// ServerRecord is one server process the runner started.
type ServerRecord struct {
	Batch        string           `json:"batch"`
	Profile      string           `json:"profile"`
	Shard        int              `json:"shard"`
	Side         string           `json:"side"`
	Impl         string           `json:"impl"`
	Binary       string           `json:"binary"`
	Instrumented bool             `json:"instrumented"`
	Reason       string           `json:"reason,omitempty"`
	Build        parity.BuildInfo `json:"build"`
	Log          string           `json:"log,omitempty"`
}

// FixtureTotals count the adapter fixture self-test results.
type FixtureTotals struct {
	Total   int `json:"total"`
	Passing int `json:"passing"`
	Failing int `json:"failing"`
	Excused int `json:"excused"`
	NotRun  int `json:"not_run"`
}

// RunSummary is run.json.
type RunSummary struct {
	Schema           string            `json:"schema"`
	SchemaVersion    string            `json:"schema_version"`
	RunID            string            `json:"run_id"`
	Mode             string            `json:"mode"`
	ImplA            string            `json:"impl_a"`
	ImplB            string            `json:"impl_b"`
	BinaryA          string            `json:"binary_a"`
	BinaryB          string            `json:"binary_b"`
	PathBase         string            `json:"path_base"`
	StartedAt        string            `json:"started_at"`
	FinishedAt       string            `json:"finished_at"`
	DurationMS       int64             `json:"duration_ms"`
	Counts           Counts            `json:"counts"`
	Mismatches       int               `json:"mismatches"`
	MismatchesByKind map[string]int    `json:"mismatches_by_kind"`
	Clusters         int               `json:"clusters"`
	PrimaryClusters  int               `json:"primary_clusters"`
	Fixtures         *FixtureTotals    `json:"fixtures,omitempty"`
	Servers          []ServerRecord    `json:"servers"`
	Options          map[string]string `json:"options"`
	Notes            []string          `json:"notes"`
}

// ClusterEntry is one cluster of clusters.json with its repro artifacts.
type ClusterEntry struct {
	diff.Cluster
	// Representative is the case the cluster is minimized from: the
	// smallest case ID in which the signature is primary (else the example).
	Representative string `json:"representative"`
	// CaseDir is the representative's case directory.
	CaseDir     string `json:"case_dir"`
	MinEnvelope string `json:"min_envelope,omitempty"`
	MinReport   string `json:"min_report,omitempty"`
}

// ClustersDoc is clusters.json.
type ClustersDoc struct {
	Schema        string         `json:"schema"`
	SchemaVersion string         `json:"schema_version"`
	Clusters      []ClusterEntry `json:"clusters"`
}

// Run is a loaded or finished run.
type Run struct {
	Layout   Layout
	Summary  RunSummary
	Cases    []CaseSummary
	Diffs    []parity.DiffDocument
	Clusters []ClusterEntry
	Index    parity.AuctionIndex
	Fixtures []FixtureResult
}

// Case returns the summary of caseID.
func (r *Run) Case(caseID string) (*CaseSummary, bool) {
	i := sort.Search(len(r.Cases), func(i int) bool { return r.Cases[i].CaseID >= caseID })
	if i < len(r.Cases) && r.Cases[i].CaseID == caseID {
		return &r.Cases[i], true
	}
	return nil, false
}

// Writer collects the cases of a run and writes the run documents. AddCase
// is safe for concurrent use.
type Writer struct {
	layout Layout

	mu       sync.Mutex
	cases    map[string]CaseSummary
	diffs    map[string]parity.DiffDocument
	fixtures []FixtureResult
}

// NewWriter creates the run directory and returns its writer.
func NewWriter(l Layout) (*Writer, error) {
	if err := os.MkdirAll(l.Path(DirCases), 0o755); err != nil {
		return nil, fmt.Errorf("report: creating %s: %w", l.Dir, err)
	}
	return &Writer{layout: l, cases: map[string]CaseSummary{}, diffs: map[string]parity.DiffDocument{}}, nil
}

// Layout returns the writer's layout.
func (w *Writer) Layout() Layout { return w.layout }

// CasePath returns the absolute path of a file in caseID's directory.
func (w *Writer) CasePath(caseID, name string) string {
	return w.layout.Path(filepath.FromSlash(CaseDirName(caseID)), name)
}

// WriteCaseJSON writes one case artifact and returns its recorded path.
func (w *Writer) WriteCaseJSON(caseID, name string, v any) (string, error) {
	p := w.CasePath(caseID, name)
	if err := WriteJSON(p, v); err != nil {
		return "", err
	}
	return w.layout.Ref(p), nil
}

// WriteCaseFile writes raw bytes as one case artifact and returns its recorded path.
func (w *Writer) WriteCaseFile(caseID, name string, raw []byte) (string, error) {
	p := w.CasePath(caseID, name)
	if err := WriteFile(p, raw); err != nil {
		return "", err
	}
	return w.layout.Ref(p), nil
}

// AddCase records a finished case: it writes the diff document (when the
// case was compared) and summary.json, and keeps both for Finish.
func (w *Writer) AddCase(s CaseSummary, doc *parity.DiffDocument) error {
	s.Artifacts.Dir = w.layout.Ref(w.layout.Path(filepath.FromSlash(CaseDirName(s.CaseID))))
	if doc != nil {
		raw, err := diff.EncodeDiff(*doc)
		if err != nil {
			return err
		}
		if s.Artifacts.Diff, err = w.WriteCaseFile(s.CaseID, FileDiff, raw); err != nil {
			return err
		}
		s.ApplyDiff(*doc)
	}
	s.Artifacts.Summary = w.layout.Ref(w.CasePath(s.CaseID, FileCaseSummary))
	if err := WriteJSON(w.CasePath(s.CaseID, FileCaseSummary), s); err != nil {
		return err
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	w.cases[s.CaseID] = s
	if doc != nil {
		w.diffs[s.CaseID] = *doc
	}
	return nil
}

// AddFixtures records the adapter fixture self-test results.
func (w *Writer) AddFixtures(results []FixtureResult) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.fixtures = append(w.fixtures, results...)
}

// Finish computes the clusters and index and writes index.json,
// clusters.json, run.json, summary.md, junit.xml and (when present)
// fixtures.json. summary carries the run identity; Finish fills its totals.
func (w *Writer) Finish(summary RunSummary) (*Run, error) {
	w.mu.Lock()
	run := &Run{Layout: w.layout, Summary: summary, Fixtures: append([]FixtureResult(nil), w.fixtures...)}
	for _, c := range w.cases {
		run.Cases = append(run.Cases, c)
	}
	for _, d := range w.diffs {
		run.Diffs = append(run.Diffs, d)
	}
	w.mu.Unlock()
	return run, run.write()
}

// write derives the run documents from the cases and diffs and writes them.
func (r *Run) write() error {
	r.derive()
	if r.Summary.FinishedAt == "" {
		r.Summary.FinishedAt = time.Now().UTC().Format(time.RFC3339)
	}
	raw, err := EncodeJSON(r.Index)
	if err != nil {
		return fmt.Errorf("report: encoding index: %w", err)
	}
	if err := ValidateIndex(raw); err != nil {
		return err
	}
	if err := WriteFile(r.Layout.Path(FileIndex), raw); err != nil {
		return err
	}
	docs := []struct {
		name string
		v    any
	}{
		{FileClusters, ClustersDoc{Schema: SchemaClusters, SchemaVersion: parity.SchemaVersion, Clusters: r.Clusters}},
		{FileRun, r.Summary},
	}
	if len(r.Fixtures) > 0 {
		docs = append(docs, struct {
			name string
			v    any
		}{FileFixtures, FixturesDoc{Schema: SchemaFixtures, SchemaVersion: parity.SchemaVersion, Results: r.Fixtures}})
	}
	for _, d := range docs {
		if err := WriteJSON(r.Layout.Path(d.name), d.v); err != nil {
			return err
		}
	}
	return r.writeRendered()
}

// writeRendered writes summary.md and junit.xml.
func (r *Run) writeRendered() error {
	if err := WriteFile(r.Layout.Path(FileSummaryMD), RenderSummary(r)); err != nil {
		return err
	}
	junit, err := JUnit(r)
	if err != nil {
		return err
	}
	return WriteFile(r.Layout.Path(FileJUnit), junit)
}

// derive sorts the cases and computes the index, clusters and totals.
func (r *Run) derive() {
	sort.Slice(r.Cases, func(i, j int) bool { return r.Cases[i].CaseID < r.Cases[j].CaseID })
	sort.Slice(r.Diffs, func(i, j int) bool { return r.Diffs[i].CaseID < r.Diffs[j].CaseID })
	sort.Slice(r.Fixtures, func(i, j int) bool { return r.Fixtures[i].CaseID < r.Fixtures[j].CaseID })
	r.Index = BuildIndex(r.Cases)
	r.Clusters = r.clusterEntries(diff.BuildClusters(r.Diffs))
	s := &r.Summary
	s.Schema, s.SchemaVersion, s.PathBase = SchemaRunSummary, parity.SchemaVersion, r.Layout.PathBase()
	s.Counts = Counts{}
	for _, c := range r.Cases {
		s.Counts.add(c.Status)
	}
	s.MismatchesByKind = map[string]int{}
	s.Mismatches = 0
	for kind, n := range diff.CountByKind(r.Diffs) {
		s.MismatchesByKind[string(kind)] = n
		s.Mismatches += n
	}
	s.Clusters, s.PrimaryClusters = len(r.Clusters), len(diff.PrimaryClusters(clusterList(r.Clusters)))
	s.Fixtures = fixtureTotals(r.Fixtures)
	if s.Servers == nil {
		s.Servers = []ServerRecord{}
	}
	if s.Options == nil {
		s.Options = map[string]string{}
	}
	if s.Notes == nil {
		s.Notes = []string{}
	}
}

// clusterEntries attaches representatives and minimized repro paths.
func (r *Run) clusterEntries(clusters []diff.Cluster) []ClusterEntry {
	out := make([]ClusterEntry, 0, len(clusters))
	for _, c := range clusters {
		e := ClusterEntry{Cluster: c, Representative: r.representative(c)}
		if cs, ok := r.Case(e.Representative); ok {
			e.CaseDir = cs.Artifacts.Dir
			e.MinEnvelope, e.MinReport = cs.Artifacts.MinEnvelope, cs.Artifacts.MinReport
		}
		out = append(out, e)
	}
	return out
}

// representative is the smallest case ID in which the cluster's signature is
// primary; a cluster with no primary record falls back to its example.
func (r *Run) representative(c diff.Cluster) string {
	for _, id := range c.Cases {
		if cs, ok := r.Case(id); ok && containsString(cs.PrimarySignatures, c.Signature) {
			return id
		}
	}
	return c.Example.CaseID
}

func clusterList(entries []ClusterEntry) []diff.Cluster {
	out := make([]diff.Cluster, len(entries))
	for i, e := range entries {
		out[i] = e.Cluster
	}
	return out
}

// LoadRun reads a finished run directory: run.json, every case summary and
// diff document, clusters.json and fixtures.json.
func LoadRun(dir string) (*Run, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return nil, fmt.Errorf("report: run directory %q: %w", dir, err)
	}
	r := &Run{Layout: Layout{Dir: abs}}
	if err := ReadJSON(r.Layout.Path(FileRun), &r.Summary); err != nil {
		return nil, err
	}
	if r.Summary.PathBase != "" {
		r.Layout.Base = filepath.Clean(filepath.Join(abs, filepath.FromSlash(r.Summary.PathBase)))
	}
	if err := r.loadCases(); err != nil {
		return nil, err
	}
	sort.Slice(r.Cases, func(i, j int) bool { return r.Cases[i].CaseID < r.Cases[j].CaseID })
	sort.Slice(r.Diffs, func(i, j int) bool { return r.Diffs[i].CaseID < r.Diffs[j].CaseID })
	var clusters ClustersDoc
	if err := ReadJSON(r.Layout.Path(FileClusters), &clusters); err != nil {
		return nil, err
	}
	r.Clusters = clusters.Clusters
	if err := ReadJSON(r.Layout.Path(FileIndex), &r.Index); err != nil {
		return nil, err
	}
	return r, r.loadFixtures()
}

// loadCases reads cases/*/summary.json and the diff documents they reference,
// refreshing the minimized-repro paths from the files present on disk.
func (r *Run) loadCases() error {
	root := r.Layout.Path(DirCases)
	return filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return fmt.Errorf("report: reading %s: %w", root, err)
		}
		if d.IsDir() || d.Name() != FileCaseSummary {
			return nil
		}
		var s CaseSummary
		if err := ReadJSON(p, &s); err != nil {
			return err
		}
		r.refreshMin(&s, filepath.Dir(p))
		r.Cases = append(r.Cases, s)
		if s.Artifacts.Diff == "" {
			return nil
		}
		var doc parity.DiffDocument
		if err := ReadJSON(filepath.Join(filepath.Dir(p), FileDiff), &doc); err != nil {
			return err
		}
		r.Diffs = append(r.Diffs, doc)
		return nil
	})
}

// refreshMin records minimized-repro files written after the run.
func (r *Run) refreshMin(s *CaseSummary, dir string) {
	files := []struct {
		name string
		dst  *string
	}{{FileMinEnvelope, &s.Artifacts.MinEnvelope}, {FileMinCase, &s.Artifacts.MinCase}, {FileMinReport, &s.Artifacts.MinReport}}
	for _, f := range files {
		p := filepath.Join(dir, f.name)
		if _, err := os.Stat(p); err == nil {
			*f.dst = r.Layout.Ref(p)
		}
	}
}

func (r *Run) loadFixtures() error {
	var doc FixturesDoc
	err := ReadJSON(r.Layout.Path(FileFixtures), &doc)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			return nil
		}
		return err
	}
	r.Fixtures = doc.Results
	return nil
}

// Rebuild reloads a run directory from its case files and rewrites the run
// documents (used after minimization adds repro files).
func Rebuild(dir string) (*Run, error) {
	r, err := LoadRun(dir)
	if err != nil {
		return nil, err
	}
	return r, r.write()
}

func containsString(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}
