package matrix

import (
	"bytes"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/report"
)

// Snapshot file names written by WriteSnapshot (Phase-1 parity/matrix layout).
const (
	FileStatus = "status.json"
	FileReadme = "README.md"
)

// Run-directory file names written by WriteRunFiles.
const (
	FileRunMatrixJSON = "matrix.json"
	FileRunMatrixMD   = "matrix.md"
)

// EncodeStatus encodes status.json (deterministic: fixed member order, rows
// in locked order and tables sorted).
func EncodeStatus(m *Matrix) ([]byte, error) {
	if err := m.Validate(); err != nil {
		return nil, err
	}
	raw, err := report.EncodeJSON(m)
	if err != nil {
		return nil, fmt.Errorf("matrix: encoding status: %w", err)
	}
	return raw, nil
}

// WriteSnapshot writes status.json and README.md into dir.
func WriteSnapshot(m *Matrix, dir string) error {
	return write(m, filepath.Join(dir, FileStatus), filepath.Join(dir, FileReadme))
}

// WriteRunFiles writes matrix.json and matrix.md into a run directory.
func WriteRunFiles(m *Matrix, runDir string) error {
	return write(m, filepath.Join(runDir, FileRunMatrixJSON), filepath.Join(runDir, FileRunMatrixMD))
}

func write(m *Matrix, jsonPath, mdPath string) error {
	raw, err := EncodeStatus(m)
	if err != nil {
		return err
	}
	if err := report.WriteFile(jsonPath, raw); err != nil {
		return err
	}
	return report.WriteFile(mdPath, RenderMarkdown(m))
}

// RenderMarkdown renders the matrix as Markdown: the domain rows (every
// locked domain, not-yet-instrumented ones included), then the endpoint,
// bidder and stage tables.
func RenderMarkdown(m *Matrix) []byte {
	var b bytes.Buffer
	b.WriteString("# Parity coverage matrix\n\n")
	fmt.Fprintf(&b, "Run `%s`: implementation A `%s` against implementation B `%s`, finished %s. Source: `%s`.\n\n",
		m.RunID, m.ImplA, m.ImplB, orDash(m.FinishedAt), m.Index)
	fmt.Fprintf(&b, "%d cases: %d passing, %d mismatch, %d not-yet-instrumented, %d error, %d skipped.\n\n",
		m.Counts.Total, m.Counts.Passing, m.Counts.Mismatch, m.Counts.NotYetInstrumented, m.Counts.Error, m.Counts.Skipped)
	renderDomains(&b, m)
	renderEndpoints(&b, m)
	renderBidders(&b, m)
	renderStages(&b, m)
	return b.Bytes()
}

func renderDomains(b *bytes.Buffer, m *Matrix) {
	b.WriteString("## Domains\n\n| Domain | Status | Exercised | Mismatching | Summary | Evidence |\n|---|---|---|---|---|---|\n")
	for _, r := range m.Rows {
		fmt.Fprintf(b, "| %s (`%s`) | %s | %d | %d | %s | %s |\n", r.Title, r.Domain, r.Status, r.Exercised, r.Mismatching, cell(r.Summary), evidenceCell(r))
	}
	b.WriteString("\n")
}

// evidenceCell lists a row's evidence links: case ID, artifact and cluster.
func evidenceCell(r Row) string {
	if len(r.EvidenceLinks) == 0 {
		return "none"
	}
	parts := make([]string, 0, len(r.EvidenceLinks))
	for _, l := range r.EvidenceLinks {
		p := fmt.Sprintf("`%s` (`%s`", cell(l.CaseID), cell(l.ArtifactPath))
		if l.Cluster != "" {
			p += fmt.Sprintf(", cluster `%s`", short(l.Cluster))
		}
		parts = append(parts, p+")")
	}
	return strings.Join(parts, "<br>")
}

func renderEndpoints(b *bytes.Buffer, m *Matrix) {
	b.WriteString("## Endpoints\n\n| Endpoint | Status | Cases | Passing | Mismatch | Not yet instrumented | Error | Skipped |\n|---|---|---|---|---|---|---|---|\n")
	for _, r := range m.Endpoints {
		fmt.Fprintf(b, "| `%s` | %s | %d | %d | %d | %d | %d | %d |\n", cell(r.Endpoint), r.Status, r.Cases, r.Passing, r.Mismatch, r.NotYetInstrumented, r.Error, r.Skipped)
	}
	b.WriteString("\n")
}

func renderBidders(b *bytes.Buffer, m *Matrix) {
	b.WriteString("## Bidders\n\nPass rates are passing over compared cases. Fixture self-test columns count implementation A against the adapter fixtures' own expectations.\n\n")
	b.WriteString("| Bidder | Status | Adapter fixtures | Adapter pass % | Lifted auctions | Lifted pass % | needs_fake | Fixture self-test (pass / fail / excused) |\n|---|---|---|---|---|---|---|---|\n")
	for _, r := range m.Bidders {
		fmt.Fprintf(b, "| `%s` | %s | %d | %s | %d | %s | %d | %d / %d / %d |\n", cell(r.Bidder), r.Status,
			r.AdapterCases, pctCell(r.AdapterPassPct), r.LiftedCases, pctCell(r.LiftedPassPct), r.NeedsFake,
			r.FixturesPassing, r.FixturesFailing, r.FixturesExcused)
	}
	b.WriteString("\n")
}

func renderStages(b *bytes.Buffer, m *Matrix) {
	b.WriteString("## Stages\n\nCounts are over compared cases. Implementation B reports `unimplemented` for stages it has no capture point for yet.\n\n")
	b.WriteString("| # | Stage | Domain | Status (B) | A captured | B captured | B unimplemented | Mismatching cases | First divergence |\n|---|---|---|---|---|---|---|---|---|\n")
	for _, r := range m.Stages {
		fmt.Fprintf(b, "| %d | `%s` | %s | %s | %d | %d | %d | %d | %d |\n", r.Order, r.Stage, r.Domain, r.Status,
			r.CapturedA, r.CapturedB, r.UnimplementedB, r.MismatchCases, r.FirstDivergent)
	}
	b.WriteString("\n")
}

func pctCell(p *float64) string {
	if p == nil {
		return "n/a"
	}
	return fmt.Sprintf("%.1f", *p)
}

func orDash(s string) string {
	if s == "" {
		return "-"
	}
	return s
}

// cell makes s safe inside a Markdown table cell.
func cell(s string) string {
	return strings.ReplaceAll(strings.ReplaceAll(s, "\n", " "), "|", `\|`)
}

func short(sig string) string {
	const n = 12
	if len(sig) > n {
		return sig[:n]
	}
	return sig
}
