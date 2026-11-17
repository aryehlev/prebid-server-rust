// Package matrix builds the domain-level parity coverage matrix (Phase-1
// PARI-02) from a finished parity run: one row per locked domain with status
// passing, mismatch or not-yet-instrumented and links to the evidence, plus
// per-endpoint, per-bidder and per-stage tables. Rows carry IDs, counts and
// artifact paths only, never request or response payloads, so the rendered
// status.json and README.md can be committed. The package is harness-only.
package matrix

import (
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/report"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// SchemaMatrix names the status.json document.
const SchemaMatrix = "pbs-parity-matrix/1"

// maxEvidence caps the evidence links of one row.
const maxEvidence = 5

//go:embed domains.json
var domainsJSON []byte

// DomainInfo is one entry of domains.json.
type DomainInfo struct {
	Domain  parity.Domain `json:"domain"`
	Title   string        `json:"title"`
	Surface string        `json:"surface"`
}

// Domains returns the locked rows of domains.json and checks that their order
// is parity.AllDomains().
func Domains() ([]DomainInfo, error) {
	var doc struct {
		Domains []DomainInfo `json:"domains"`
	}
	if err := json.Unmarshal(domainsJSON, &doc); err != nil {
		return nil, fmt.Errorf("matrix: decoding domains.json: %w", err)
	}
	locked := parity.AllDomains()
	if len(doc.Domains) != len(locked) {
		return nil, fmt.Errorf("matrix: domains.json has %d domains, want %d", len(doc.Domains), len(locked))
	}
	for i, d := range doc.Domains {
		if d.Domain != locked[i] {
			return nil, fmt.Errorf("matrix: domains.json row %d is %q, want %q", i, d.Domain, locked[i])
		}
	}
	return doc.Domains, nil
}

// Row is one domain row: the Phase-1 MatrixRow members plus counts.
type Row struct {
	parity.MatrixRow
	Title string `json:"title"`
	// Exercised is the number of compared cases that captured a stage of
	// the domain on implementation A; Mismatching of them have a mismatch
	// attributed to the domain.
	Exercised       int `json:"exercised"`
	Mismatching     int `json:"mismatching"`
	PrimaryClusters int `json:"primary_clusters"`
}

// EndpointRow counts the cases of one endpoint.
type EndpointRow struct {
	Endpoint           string               `json:"endpoint"`
	Status             parity.SurfaceStatus `json:"status"`
	Cases              int                  `json:"cases"`
	Passing            int                  `json:"passing"`
	Mismatch           int                  `json:"mismatch"`
	NotYetInstrumented int                  `json:"not_yet_instrumented"`
	Error              int                  `json:"error"`
	Skipped            int                  `json:"skipped"`
	NotCompared        int                  `json:"not_compared"`
}

// BidderRow is the per-bidder table: adapter-fixture and lifted-auction pass
// rates (passing over compared cases; null when none was compared).
type BidderRow struct {
	Bidder           string               `json:"bidder"`
	Status           parity.SurfaceStatus `json:"status"`
	AdapterCases     int                  `json:"adapter_cases"`
	AdapterCompared  int                  `json:"adapter_compared"`
	AdapterPassing   int                  `json:"adapter_passing"`
	AdapterPassPct   *float64             `json:"adapter_pass_pct"`
	LiftedCases      int                  `json:"lifted_cases"`
	LiftedCompared   int                  `json:"lifted_compared"`
	LiftedPassing    int                  `json:"lifted_passing"`
	LiftedPassPct    *float64             `json:"lifted_pass_pct"`
	NeedsFake        int                  `json:"needs_fake"`
	FixturesPassing  int                  `json:"fixtures_passing"`
	FixturesFailing  int                  `json:"fixtures_failing"`
	FixturesExcused  int                  `json:"fixtures_excused"`
	FixtureSelfTests int                  `json:"fixture_self_tests"`
}

// Stage statuses of the per-stage table, from implementation B's reports.
const (
	StageCaptured      = "captured"
	StagePartial       = "partial"
	StageUnimplemented = "unimplemented"
	StageNotExercised  = "not-exercised"
)

// StageRow is the per-stage table over compared cases: how often each side
// captured the stage, how often implementation B reported it unimplemented,
// and how many cases mismatch there.
type StageRow struct {
	Stage          parity.StageID `json:"stage"`
	Order          int            `json:"order"`
	Domain         parity.Domain  `json:"domain"`
	Status         string         `json:"status"`
	CapturedA      int            `json:"captured_a"`
	CapturedB      int            `json:"captured_b"`
	UnimplementedB int            `json:"unimplemented_b"`
	MismatchCases  int            `json:"mismatch_cases"`
	FirstDivergent int            `json:"first_divergent"`
}

// Matrix is status.json.
type Matrix struct {
	Schema        string        `json:"schema"`
	SchemaVersion string        `json:"schema_version"`
	RunID         string        `json:"run_id"`
	ImplA         string        `json:"impl_a"`
	ImplB         string        `json:"impl_b"`
	FinishedAt    string        `json:"finished_at"`
	Index         string        `json:"index"`
	Counts        report.Counts `json:"counts"`
	Rows          []Row         `json:"rows"`
	Endpoints     []EndpointRow `json:"endpoints"`
	Bidders       []BidderRow   `json:"bidders"`
	Stages        []StageRow    `json:"stages"`
}

// BuildFromRun loads a finished run directory and builds its matrix.
func BuildFromRun(dir string) (*Matrix, error) {
	run, err := report.LoadRun(dir)
	if err != nil {
		return nil, err
	}
	return BuildMatrix(run)
}

// BuildMatrix builds and validates the matrix of a run.
func BuildMatrix(run *report.Run) (*Matrix, error) {
	if run == nil {
		return nil, errors.New("matrix: no run")
	}
	domains, err := Domains()
	if err != nil {
		return nil, err
	}
	stages, err := schema.Stages()
	if err != nil {
		return nil, fmt.Errorf("matrix: loading stages.json: %w", err)
	}
	m := &Matrix{
		Schema: SchemaMatrix, SchemaVersion: parity.SchemaVersion, RunID: run.Summary.RunID,
		ImplA: run.Summary.ImplA, ImplB: run.Summary.ImplB, FinishedAt: run.Summary.FinishedAt,
		Index: run.Layout.Ref(run.Layout.Path(report.FileIndex)), Counts: run.Summary.Counts,
	}
	ev := newEvidence(run)
	for _, d := range domains {
		m.Rows = append(m.Rows, ev.row(d))
	}
	m.Endpoints = endpointRows(run.Cases)
	m.Bidders = bidderRows(run.Cases, run.Fixtures)
	m.Stages = stageRows(stages, run)
	return m, m.Validate()
}

// evidence indexes a run for the domain rows.
type evidence struct {
	run *report.Run
	// signatures holds, per case and domain, the signatures of its records
	// in document order (primary records first).
	signatures map[string]map[parity.Domain][]string
	primary    map[parity.Domain]int
}

func newEvidence(run *report.Run) *evidence {
	e := &evidence{run: run, signatures: map[string]map[parity.Domain][]string{}, primary: map[parity.Domain]int{}}
	for _, doc := range run.Diffs {
		byDomain := map[parity.Domain][]string{}
		for _, pass := range []bool{true, false} {
			for _, m := range doc.Mismatches {
				if m.Primary == pass && !containsString(byDomain[m.Domain], m.Signature) {
					byDomain[m.Domain] = append(byDomain[m.Domain], m.Signature)
				}
			}
		}
		e.signatures[doc.CaseID] = byDomain
	}
	for _, c := range run.Clusters {
		if c.Primary() {
			e.primary[c.Domain]++
		}
	}
	return e
}

// row computes one domain row. Status: mismatch when a compared case has a
// mismatch attributed to the domain; passing when compared cases exercised
// the domain without one; not-yet-instrumented otherwise.
func (e *evidence) row(info DomainInfo) Row {
	d := info.Domain
	r := Row{Title: info.Title, PrimaryClusters: e.primary[d]}
	r.Domain, r.EvidenceLinks = d, []parity.EvidenceLink{}
	var mismatching, passing []report.CaseSummary
	for _, c := range e.run.Cases {
		if !c.Status.Compared() {
			continue
		}
		hasMismatch := containsDomain(c.MismatchDomains, d)
		if containsDomain(c.ExercisedDomains, d) || hasMismatch {
			r.Exercised++
		}
		switch {
		case hasMismatch:
			mismatching = append(mismatching, c)
		case containsDomain(c.ExercisedDomains, d):
			passing = append(passing, c)
		}
	}
	r.Mismatching = len(mismatching)
	switch {
	case len(mismatching) > 0:
		r.Status = parity.StatusMismatch
		r.EvidenceLinks = e.links(mismatching, d)
		r.Summary = fmt.Sprintf("%d of %d compared cases that exercised this domain mismatch; %d primary clusters.", r.Mismatching, r.Exercised, r.PrimaryClusters)
	case len(passing) > 0:
		r.Status = parity.StatusPassing
		r.EvidenceLinks = e.links(passing, d)
		r.Summary = fmt.Sprintf("%d compared cases exercised this domain without a mismatch.", r.Exercised)
	default:
		r.Status = parity.StatusNotYetInstrumented
		r.Summary = e.notInstrumentedSummary()
	}
	return r
}

// links returns up to maxEvidence evidence links, cases whose first
// divergence is in the domain first.
func (e *evidence) links(cases []report.CaseSummary, d parity.Domain) []parity.EvidenceLink {
	sort.SliceStable(cases, func(i, j int) bool {
		pi, pj := cases[i].PrimaryDomain == d, cases[j].PrimaryDomain == d
		if pi != pj {
			return pi
		}
		return cases[i].CaseID < cases[j].CaseID
	})
	out := []parity.EvidenceLink{}
	for _, c := range cases {
		if len(out) == maxEvidence {
			break
		}
		link := parity.EvidenceLink{CaseID: c.CaseID, FixturePath: c.FixturePath, ArtifactPath: artifactPath(c), SummaryPath: c.Artifacts.Summary}
		if sigs := e.signatures[c.CaseID][d]; len(sigs) > 0 {
			link.Cluster = sigs[0]
		}
		out = append(out, link)
	}
	return out
}

// notInstrumentedSummary explains a domain no compared case exercised.
func (e *evidence) notInstrumentedSummary() string {
	c := e.run.Summary.Counts
	if c.Compared() == 0 && c.NotYetInstrumented > 0 {
		return fmt.Sprintf("No case was compared: %d cases were not run because an implementation does not serve the parity contract yet.", c.NotYetInstrumented)
	}
	return "No compared case captured a stage of this domain yet."
}

func artifactPath(c report.CaseSummary) string {
	if c.Artifacts.Diff != "" {
		return c.Artifacts.Diff
	}
	return c.Artifacts.Dir
}

// endpointRows counts cases per endpoint, sorted by endpoint.
func endpointRows(cases []report.CaseSummary) []EndpointRow {
	by := map[string]*EndpointRow{}
	for _, c := range cases {
		r, ok := by[c.Endpoint]
		if !ok {
			r = &EndpointRow{Endpoint: c.Endpoint}
			by[c.Endpoint] = r
		}
		r.Cases++
		switch c.Status {
		case report.CasePassing:
			r.Passing++
		case report.CaseMismatch:
			r.Mismatch++
		case report.CaseNotYetInstrumented:
			r.NotYetInstrumented++
		case report.CaseError:
			r.Error++
		case report.CaseSkipped:
			r.Skipped++
		case report.CaseNotCompared:
			r.NotCompared++
		}
	}
	out := make([]EndpointRow, 0, len(by))
	for _, r := range by {
		r.Status = surfaceStatus(r.Passing, r.Mismatch)
		out = append(out, *r)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Endpoint < out[j].Endpoint })
	return out
}

// surfaceStatus is mismatch when anything mismatched, passing when something
// was compared, not-yet-instrumented otherwise.
func surfaceStatus(passing, mismatch int) parity.SurfaceStatus {
	switch {
	case mismatch > 0:
		return parity.StatusMismatch
	case passing > 0:
		return parity.StatusPassing
	}
	return parity.StatusNotYetInstrumented
}

// Case kinds the bidder table reads (parity/corpus Kind values).
const (
	kindAdapter = "adapter"
	kindLifted  = "lifted_auction"
	tagFake     = "needs_fake"
)

// bidderRows builds the per-bidder table from adapter and lifted cases.
func bidderRows(cases []report.CaseSummary, fixtures []report.FixtureResult) []BidderRow {
	by := map[string]*BidderRow{}
	get := func(b string) *BidderRow {
		if r, ok := by[b]; ok {
			return r
		}
		r := &BidderRow{Bidder: b}
		by[b] = r
		return r
	}
	for _, c := range cases {
		if c.Bidder == "" || (c.Kind != kindAdapter && c.Kind != kindLifted) {
			continue
		}
		r := get(c.Bidder)
		r.add(c)
	}
	for _, f := range fixtures {
		if f.Bidder != "" {
			get(f.Bidder).addFixture(f)
		}
	}
	out := make([]BidderRow, 0, len(by))
	for _, r := range by {
		r.AdapterPassPct = pct(r.AdapterPassing, r.AdapterCompared)
		r.LiftedPassPct = pct(r.LiftedPassing, r.LiftedCompared)
		r.Status = surfaceStatus(r.AdapterPassing+r.LiftedPassing, r.AdapterCompared+r.LiftedCompared-r.AdapterPassing-r.LiftedPassing)
		out = append(out, *r)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Bidder < out[j].Bidder })
	return out
}

func (r *BidderRow) add(c report.CaseSummary) {
	if containsString(c.Tags, tagFake) {
		r.NeedsFake++
	}
	compared, passing := 0, 0
	if c.Status.Compared() {
		compared = 1
	}
	if c.Status == report.CasePassing {
		passing = 1
	}
	if c.Kind == kindAdapter {
		r.AdapterCases++
		r.AdapterCompared += compared
		r.AdapterPassing += passing
		return
	}
	r.LiftedCases++
	r.LiftedCompared += compared
	r.LiftedPassing += passing
}

func (r *BidderRow) addFixture(f report.FixtureResult) {
	r.FixtureSelfTests++
	switch f.Status {
	case report.FixturePassing:
		r.FixturesPassing++
	case report.FixtureFailing:
		r.FixturesFailing++
	case report.FixtureExcused:
		r.FixturesExcused++
	}
}

// pct is passing/compared in percent with one decimal, nil when nothing was compared.
func pct(passing, compared int) *float64 {
	if compared == 0 {
		return nil
	}
	v := math.Round(float64(passing)*1000/float64(compared)) / 10
	return &v
}

// stageRows builds the per-stage table in registry order.
func stageRows(reg schema.StageRegistry, run *report.Run) []StageRow {
	rows := make([]StageRow, 0, len(reg.Stages))
	index := map[parity.StageID]int{}
	for _, s := range reg.Stages {
		index[s.ID] = len(rows)
		rows = append(rows, StageRow{Stage: s.ID, Order: s.Order, Domain: s.Domain})
	}
	for _, c := range run.Cases {
		if !c.Status.Compared() {
			continue
		}
		for stage, st := range c.StagesA {
			if i, ok := index[stage]; ok && st == parity.StageCaptured {
				rows[i].CapturedA++
			}
		}
		for stage, st := range c.StagesB {
			i, ok := index[stage]
			switch {
			case !ok:
			case st == parity.StageCaptured:
				rows[i].CapturedB++
			case st == parity.StageUnimplemented:
				rows[i].UnimplementedB++
			}
		}
		if i, ok := index[c.FirstStage]; ok {
			rows[i].FirstDivergent++
		}
	}
	countStageMismatches(rows, index, run.Diffs)
	for i := range rows {
		rows[i].Status = stageStatus(rows[i])
	}
	return rows
}

func countStageMismatches(rows []StageRow, index map[parity.StageID]int, docs []parity.DiffDocument) {
	for _, doc := range docs {
		seen := map[parity.StageID]bool{}
		for _, m := range doc.Mismatches {
			if i, ok := index[m.Stage]; ok && !seen[m.Stage] {
				seen[m.Stage] = true
				rows[i].MismatchCases++
			}
		}
	}
}

func stageStatus(r StageRow) string {
	switch {
	case r.CapturedB > 0 && r.UnimplementedB == 0:
		return StageCaptured
	case r.CapturedB > 0:
		return StagePartial
	case r.UnimplementedB > 0:
		return StageUnimplemented
	}
	return StageNotExercised
}

// Validate checks the matrix contract: the nine locked rows in order, allowed
// statuses, at least one complete evidence link on every passing or mismatch
// row, and no payload anywhere in the rows.
func (m *Matrix) Validate() error {
	locked := parity.AllDomains()
	if len(m.Rows) != len(locked) {
		return fmt.Errorf("matrix: %d rows, want the %d locked domains", len(m.Rows), len(locked))
	}
	for i, r := range m.Rows {
		if err := validateRow(r, locked[i]); err != nil {
			return err
		}
	}
	return nil
}

func validateRow(r Row, want parity.Domain) error {
	if r.Domain != want {
		return fmt.Errorf("matrix: row %q is out of the locked order (want %q)", r.Domain, want)
	}
	switch r.Status {
	case parity.StatusPassing, parity.StatusMismatch:
		if len(r.EvidenceLinks) == 0 {
			return fmt.Errorf("matrix: %s row is %s without an evidence link", r.Domain, r.Status)
		}
	case parity.StatusNotYetInstrumented:
	default:
		return fmt.Errorf("matrix: %s row has status %q", r.Domain, r.Status)
	}
	if strings.TrimSpace(r.Summary) == "" {
		return fmt.Errorf("matrix: %s row has no summary", r.Domain)
	}
	if err := checkText(string(r.Domain), "summary", r.Summary); err != nil {
		return err
	}
	for _, l := range r.EvidenceLinks {
		if l.CaseID == "" || l.ArtifactPath == "" || l.SummaryPath == "" {
			return fmt.Errorf("matrix: %s row has an incomplete evidence link %+v", r.Domain, l)
		}
		for _, f := range []struct{ name, v string }{{"case_id", l.CaseID}, {"fixture_path", l.FixturePath}, {"artifact_path", l.ArtifactPath}, {"summary_path", l.SummaryPath}, {"cluster", l.Cluster}} {
			if err := checkText(string(r.Domain), f.name, f.v); err != nil {
				return err
			}
		}
	}
	return nil
}

// maxTextLen bounds any text in a row: IDs, paths and one-line summaries.
const maxTextLen = 512

// checkText rejects anything that looks like an embedded payload: JSON
// object or array text, quoted JSON members, newlines or oversized values.
func checkText(domain, field, v string) error {
	trimmed := strings.TrimSpace(v)
	switch {
	case len(v) > maxTextLen:
		return fmt.Errorf("matrix: %s row %s is %d bytes; rows must not embed payloads", domain, field, len(v))
	case strings.ContainsAny(v, "\n\r"):
		return fmt.Errorf("matrix: %s row %s spans lines; rows must not embed payloads", domain, field)
	case strings.Contains(v, `":`), strings.Contains(v, `{"`), strings.HasPrefix(trimmed, "{"), strings.HasPrefix(trimmed, "["):
		return fmt.Errorf("matrix: %s row %s looks like a JSON payload", domain, field)
	}
	return nil
}

func containsDomain(list []parity.Domain, d parity.Domain) bool {
	for _, v := range list {
		if v == d {
			return true
		}
	}
	return false
}

func containsString(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}
