// Package report writes and reads the artifacts of one parity run under
// parity/out/<run>/ (gitignored):
//
//	index.json         Phase-1 AuctionIndex (pbs-parity-index/1), one entry per executed case
//	clusters.json      mismatch clusters by signature, with minimized repro paths
//	run.json           run summary: implementations, counts, servers, options
//	summary.md         human summary: totals and the top clusters of every domain
//	junit.xml          one test case per parity case (and per fixture self-test)
//	fixtures.json      adapter fixture self-test results (selftest only)
//	cases/<hash>/      case.json, envelope.json, <impl>.trace.json, diff.json,
//	                   summary.json and, after minimization, min.envelope.json
//
// Paths recorded in the documents are relative to the repository root when
// the run directory is inside the checkout (the Phase-1 index convention), and
// absolute otherwise; run.json records the base so a run can be read back from
// anywhere. The package is harness-only.
package report

import (
	"bytes"
	"crypto/sha1"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// Document names of the runner-only artifacts. They are not cross-language
// contract documents, so they have no JSON Schema; the name still identifies
// the document and its major version.
const (
	SchemaCaseSummary = "pbs-parity-case-summary/1"
	SchemaRunSummary  = "pbs-parity-run/1"
	SchemaClusters    = "pbs-parity-clusters/1"
	SchemaBaseline    = "pbs-parity-baseline/1"
	SchemaFixtures    = "pbs-parity-fixtures/1"
)

// File names inside a run directory.
const (
	FileIndex     = "index.json"
	FileClusters  = "clusters.json"
	FileRun       = "run.json"
	FileSummaryMD = "summary.md"
	FileJUnit     = "junit.xml"
	FileFixtures  = "fixtures.json"
	DirCases      = "cases"
)

// File names inside a case directory.
const (
	FileCase        = "case.json"
	FileEnvelope    = "envelope.json"
	FileDiff        = "diff.json"
	FileCaseSummary = "summary.json"
	FileMinEnvelope = "min.envelope.json"
	FileMinCase     = "min.case.json"
	FileMinReport   = "min.report.json"
)

// CaseStatus is the outcome of one case. The first three values are the
// coverage-matrix statuses.
type CaseStatus string

// Case statuses.
const (
	// CasePassing means both implementations were compared and nothing differed.
	CasePassing CaseStatus = CaseStatus(parity.StatusPassing)
	// CaseMismatch means both implementations were compared and at least one
	// mismatch was recorded.
	CaseMismatch CaseStatus = CaseStatus(parity.StatusMismatch)
	// CaseNotYetInstrumented means one implementation does not serve the
	// parity contract the case needs (no /__parity/* routes, no adapter
	// endpoints, an unsupported mock level), so nothing was compared.
	CaseNotYetInstrumented CaseStatus = CaseStatus(parity.StatusNotYetInstrumented)
	// CaseError means the harness could not complete the case (a server
	// failed, a document did not decode).
	CaseError CaseStatus = "error"
	// CaseSkipped means the case was selected but not run (budget, filters).
	CaseSkipped CaseStatus = "skipped"
)

// Executed reports whether the case was attempted (it gets an index entry).
func (s CaseStatus) Executed() bool { return s != CaseSkipped }

// Compared reports whether both implementations were actually compared.
func (s CaseStatus) Compared() bool { return s == CasePassing || s == CaseMismatch }

// Artifacts are the document paths of one case (see Layout.Ref).
type Artifacts struct {
	Dir         string   `json:"dir"`
	Case        string   `json:"case,omitempty"`
	Envelope    string   `json:"envelope,omitempty"`
	TraceA      string   `json:"trace_a,omitempty"`
	TraceB      string   `json:"trace_b,omitempty"`
	Diff        string   `json:"diff,omitempty"`
	Summary     string   `json:"summary"`
	MinEnvelope string   `json:"min_envelope,omitempty"`
	MinCase     string   `json:"min_case,omitempty"`
	MinReport   string   `json:"min_report,omitempty"`
	Extra       []string `json:"extra,omitempty"`
}

// CaseSummary is cases/<hash>/summary.json: everything the reports and the
// coverage matrix need about one case, without request or response payloads.
type CaseSummary struct {
	Schema        string     `json:"schema"`
	SchemaVersion string     `json:"schema_version"`
	CaseID        string     `json:"case_id"`
	FixturePath   string     `json:"fixture_path"`
	Endpoint      string     `json:"endpoint"`
	Profile       string     `json:"profile"`
	Kind          string     `json:"kind"`
	Corpus        string     `json:"corpus"`
	Bidder        string     `json:"bidder,omitempty"`
	Tags          []string   `json:"tags"`
	ImplA         string     `json:"impl_a"`
	ImplB         string     `json:"impl_b"`
	Status        CaseStatus `json:"status"`
	// Reason explains a not-yet-instrumented, error or skipped status.
	Reason string `json:"reason,omitempty"`
	// ExercisedDomains are the domains of the stages implementation A
	// captured: what a passing comparison proves.
	ExercisedDomains []parity.Domain `json:"exercised_domains"`
	// MismatchDomains are the domains of the recorded mismatches.
	MismatchDomains []parity.Domain `json:"mismatch_domains"`
	MismatchKinds   []string        `json:"mismatch_kinds"`
	Mismatches      int             `json:"mismatches"`
	Primary         int             `json:"primary"`
	// FirstStage, PrimaryDomain and Cluster describe the first primary record:
	// the case's first diverging stage, its domain and its signature.
	FirstStage        parity.StageID `json:"first_stage,omitempty"`
	PrimaryDomain     parity.Domain  `json:"primary_domain,omitempty"`
	Cluster           string         `json:"cluster,omitempty"`
	PrimarySignatures []string       `json:"primary_signatures"`
	// StagesA and StagesB are the effective stage statuses of each side.
	StagesA    map[parity.StageID]parity.StageStatus `json:"stages_a,omitempty"`
	StagesB    map[parity.StageID]parity.StageStatus `json:"stages_b,omitempty"`
	Artifacts  Artifacts                             `json:"artifacts"`
	Notes      []string                              `json:"notes"`
	DurationMS int64                                 `json:"duration_ms"`
}

// NewCaseSummary returns a summary with the document identity and the list
// members initialised, so encoded summaries never carry null lists.
func NewCaseSummary(caseID string) CaseSummary {
	return CaseSummary{
		Schema: SchemaCaseSummary, SchemaVersion: parity.SchemaVersion, CaseID: caseID,
		Tags: []string{}, ExercisedDomains: []parity.Domain{}, MismatchDomains: []parity.Domain{},
		MismatchKinds: []string{}, PrimarySignatures: []string{}, Notes: []string{},
	}
}

// ApplyDiff fills the mismatch members of s from the case's diff document.
func (s *CaseSummary) ApplyDiff(doc parity.DiffDocument) {
	domains := map[parity.Domain]bool{}
	kinds := map[parity.MismatchKind]bool{}
	signatures := map[string]bool{}
	s.Mismatches, s.Primary = len(doc.Mismatches), 0
	for _, m := range doc.Mismatches {
		domains[m.Domain] = true
		kinds[m.Kind] = true
		if !m.Primary {
			continue
		}
		s.Primary++
		signatures[m.Signature] = true
		if s.FirstStage == "" {
			s.FirstStage, s.PrimaryDomain, s.Cluster = m.Stage, m.Domain, m.Signature
		}
	}
	s.MismatchDomains = OrderedDomains(domains)
	s.MismatchKinds = OrderedKinds(kinds)
	s.PrimarySignatures = sortedKeys(signatures)
	if s.Status == "" || s.Status.Compared() {
		s.Status = CasePassing
		if len(doc.Mismatches) > 0 {
			s.Status = CaseMismatch
		}
	}
}

// OrderedDomains returns the set members in locked matrix order.
func OrderedDomains(set map[parity.Domain]bool) []parity.Domain {
	out := []parity.Domain{}
	for _, d := range parity.AllDomains() {
		if set[d] {
			out = append(out, d)
		}
	}
	return out
}

// OrderedKinds returns the set members in schema order.
func OrderedKinds(set map[parity.MismatchKind]bool) []string {
	out := []string{}
	for _, k := range parity.AllMismatchKinds() {
		if set[k] {
			out = append(out, string(k))
		}
	}
	return out
}

// BuildIndex returns the index.json document of the executed cases, sorted by
// case ID. Skipped cases are not executed and get no entry.
func BuildIndex(cases []CaseSummary) parity.AuctionIndex {
	idx := parity.AuctionIndex{Schema: parity.SchemaIndex, SchemaVersion: parity.SchemaVersion, Cases: []parity.AuctionIndexEntry{}}
	for _, c := range cases {
		if !c.Status.Executed() {
			continue
		}
		e := parity.AuctionIndexEntry{
			CaseID: c.CaseID, FixturePath: c.FixturePath, Endpoint: c.Endpoint,
			Domains: append([]parity.Domain{}, c.MismatchDomains...), MismatchKinds: append([]string{}, c.MismatchKinds...),
			SummaryPath: c.Artifacts.Summary, ArtifactPath: artifactPath(c.Artifacts),
			Stage: c.FirstStage, PrimaryDomain: c.PrimaryDomain, Cluster: c.Cluster,
		}
		idx.Cases = append(idx.Cases, e)
	}
	sort.Slice(idx.Cases, func(i, j int) bool { return idx.Cases[i].CaseID < idx.Cases[j].CaseID })
	return idx
}

// artifactPath is the diff document when the case was compared, else the case
// directory.
func artifactPath(a Artifacts) string {
	if a.Diff != "" {
		return a.Diff
	}
	return a.Dir
}

// indexEntryFields are the exact Phase-1 index entry members.
var indexEntryFields = []string{"case_id", "fixture_path", "endpoint", "domains", "mismatch_kinds", "summary_path", "artifact_path"}

// ValidateIndex validates an encoded index.json: the schema (which fixes the
// Phase-1 entry members), a present value for every Phase-1 member, non-empty
// identifiers and paths, and unique case IDs.
func ValidateIndex(raw []byte) error {
	if err := schema.Validate(raw); err != nil {
		return fmt.Errorf("report: index.json: %w", err)
	}
	var doc struct {
		Schema string                       `json:"schema"`
		Cases  []map[string]json.RawMessage `json:"cases"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		return fmt.Errorf("report: index.json: %w", err)
	}
	if doc.Schema != parity.SchemaIndex {
		return fmt.Errorf("report: index.json: schema %q, want %q", doc.Schema, parity.SchemaIndex)
	}
	seen := map[string]bool{}
	for i, entry := range doc.Cases {
		if err := validateIndexEntry(entry, seen); err != nil {
			return fmt.Errorf("report: index.json: cases[%d]: %w", i, err)
		}
	}
	return nil
}

func validateIndexEntry(entry map[string]json.RawMessage, seen map[string]bool) error {
	for _, f := range indexEntryFields {
		v, ok := entry[f]
		if !ok || string(v) == "null" {
			return fmt.Errorf("member %q is missing", f)
		}
	}
	var e parity.AuctionIndexEntry
	raw, err := json.Marshal(entry)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(raw, &e); err != nil {
		return err
	}
	switch {
	case e.CaseID == "":
		return errors.New("case_id is empty")
	case e.SummaryPath == "" || e.ArtifactPath == "":
		return fmt.Errorf("case %q has an empty summary_path or artifact_path", e.CaseID)
	case seen[e.CaseID]:
		return fmt.Errorf("case %q is listed twice", e.CaseID)
	}
	seen[e.CaseID] = true
	return nil
}

// Layout maps a run directory to the paths recorded in its documents.
type Layout struct {
	// Dir is the absolute run directory.
	Dir string
	// Base is the absolute directory recorded paths are relative to; empty
	// means recorded paths are absolute.
	Base string
}

// NewLayout returns the layout of run directory dir. base is the repository
// root; it is used only when dir is inside it.
func NewLayout(dir, base string) (Layout, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return Layout{}, fmt.Errorf("report: run directory %q: %w", dir, err)
	}
	l := Layout{Dir: abs}
	if base == "" {
		return l, nil
	}
	absBase, err := filepath.Abs(base)
	if err != nil {
		return Layout{}, fmt.Errorf("report: path base %q: %w", base, err)
	}
	if rel, err := filepath.Rel(absBase, abs); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		l.Base = absBase
	}
	return l, nil
}

// CaseDirName returns the case directory of caseID relative to the run
// directory: cases/<first 16 hex digits of sha1(case_id)>.
func CaseDirName(caseID string) string {
	sum := sha1.Sum([]byte(caseID))
	return DirCases + "/" + hex.EncodeToString(sum[:])[:16]
}

// Path returns the absolute path of a file inside the run directory.
func (l Layout) Path(elem ...string) string {
	return filepath.Join(append([]string{l.Dir}, elem...)...)
}

// Ref returns the recorded form of an absolute path: slash-separated and
// relative to Base, or absolute when there is no base.
func (l Layout) Ref(abs string) string {
	if l.Base == "" {
		return filepath.ToSlash(abs)
	}
	rel, err := filepath.Rel(l.Base, abs)
	if err != nil {
		return filepath.ToSlash(abs)
	}
	return filepath.ToSlash(rel)
}

// Resolve returns the absolute path of a recorded path.
func (l Layout) Resolve(ref string) string {
	p := filepath.FromSlash(ref)
	if filepath.IsAbs(p) || l.Base == "" {
		return p
	}
	return filepath.Join(l.Base, p)
}

// PathBase returns Base relative to Dir ("" when recorded paths are absolute),
// the value run.json keeps so the run can be read back after a move.
func (l Layout) PathBase() string {
	if l.Base == "" {
		return ""
	}
	rel, err := filepath.Rel(l.Dir, l.Base)
	if err != nil {
		return ""
	}
	return filepath.ToSlash(rel)
}

// EncodeJSON encodes v indented, without HTML escaping and with a trailing
// newline, so artifacts read exactly as recorded.
func EncodeJSON(v any) ([]byte, error) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return b.Bytes(), nil
}

// WriteJSON writes v to path with EncodeJSON, creating parent directories.
func WriteJSON(path string, v any) error {
	raw, err := EncodeJSON(v)
	if err != nil {
		return fmt.Errorf("report: encoding %s: %w", path, err)
	}
	return WriteFile(path, raw)
}

// WriteFile writes raw to path, creating parent directories.
func WriteFile(path string, raw []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return fmt.Errorf("report: creating %s: %w", filepath.Dir(path), err)
	}
	if err := os.WriteFile(path, raw, 0o644); err != nil {
		return fmt.Errorf("report: writing %s: %w", path, err)
	}
	return nil
}

// ReadJSON decodes the JSON file at path into v.
func ReadJSON(path string, v any) error {
	raw, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("report: reading %s: %w", path, err)
	}
	if err := json.Unmarshal(raw, v); err != nil {
		return fmt.Errorf("report: decoding %s: %w", path, err)
	}
	return nil
}

func sortedKeys(set map[string]bool) []string {
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
