package report

import (
	"bytes"
	"fmt"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
)

// Fixture self-test statuses.
const (
	FixturePassing = "passing"
	FixtureFailing = "failing"
	// FixtureExcused marks a failing fixture whose case carries a tag that
	// explains it (needs_fake, unresolved_build); it does not fail the selftest.
	FixtureExcused = "excused"
	// FixtureNotRun marks a fixture whose adapter endpoints did not answer.
	FixtureNotRun = "not-run"
)

// FixtureResult is the adapter fixture self-test result of one adapter case:
// implementation A's adapter endpoints compared with the fixture's own
// expectations, using adapterstest semantics.
type FixtureResult struct {
	CaseID      string   `json:"case_id"`
	FixturePath string   `json:"fixture_path"`
	Bidder      string   `json:"bidder"`
	Status      string   `json:"status"`
	Excuse      string   `json:"excuse,omitempty"`
	Failures    []string `json:"failures"`
}

// FixturesDoc is fixtures.json.
type FixturesDoc struct {
	Schema        string          `json:"schema"`
	SchemaVersion string          `json:"schema_version"`
	Results       []FixtureResult `json:"results"`
}

// fixtureTotals counts fixture results; nil when there are none.
func fixtureTotals(results []FixtureResult) *FixtureTotals {
	if len(results) == 0 {
		return nil
	}
	t := &FixtureTotals{}
	for _, r := range results {
		t.Total++
		switch r.Status {
		case FixturePassing:
			t.Passing++
		case FixtureFailing:
			t.Failing++
		case FixtureExcused:
			t.Excused++
		default:
			t.NotRun++
		}
	}
	return t
}

// Limits of the rendered summary.
const (
	summaryClustersPerDomain = 10
	summaryReasons           = 8
	summaryFixtureFailures   = 25
	shortSignature           = 12
)

// RenderSummary renders summary.md: run identity, totals, not-yet-instrumented
// reasons, the top clusters of every domain (primary clusters first) with
// their minimized repro paths, and the fixture self-test.
func RenderSummary(r *Run) []byte {
	var b bytes.Buffer
	s := r.Summary
	fmt.Fprintf(&b, "# Parity run %s\n\n", s.RunID)
	fmt.Fprintf(&b, "Mode `%s`. Implementation A: `%s`. Implementation B: `%s`. Started %s, %.1fs.\n\n",
		s.Mode, s.ImplA, s.ImplB, s.StartedAt, float64(s.DurationMS)/1000)
	renderCounts(&b, s)
	renderReasons(&b, r.Cases)
	renderDomains(&b, r)
	renderFixtures(&b, r.Fixtures)
	return b.Bytes()
}

func renderCounts(b *bytes.Buffer, s RunSummary) {
	b.WriteString("| Status | Cases |\n|---|---|\n")
	rows := []struct {
		name string
		n    int
	}{
		{string(CasePassing), s.Counts.Passing}, {string(CaseMismatch), s.Counts.Mismatch},
		{string(CaseNotYetInstrumented), s.Counts.NotYetInstrumented}, {string(CaseError), s.Counts.Error},
		{string(CaseSkipped), s.Counts.Skipped}, {"total", s.Counts.Total},
	}
	for _, row := range rows {
		fmt.Fprintf(b, "| %s | %d |\n", row.name, row.n)
	}
	fmt.Fprintf(b, "\n%d mismatches%s. %d clusters, %d of them primary.\n\n", s.Mismatches, kindBreakdown(s.MismatchesByKind), s.Clusters, s.PrimaryClusters)
}

// kindBreakdown renders " (value 3, lexical 1)" in schema order.
func kindBreakdown(byKind map[string]int) string {
	var parts []string
	for _, k := range parity.AllMismatchKinds() {
		if n := byKind[string(k)]; n > 0 {
			parts = append(parts, fmt.Sprintf("%s %d", k, n))
		}
	}
	if len(parts) == 0 {
		return ""
	}
	return " (" + strings.Join(parts, ", ") + ")"
}

// renderReasons lists the most frequent reasons of cases that were not compared.
func renderReasons(b *bytes.Buffer, cases []CaseSummary) {
	counts := map[string]int{}
	for _, c := range cases {
		if !c.Status.Compared() && c.Reason != "" {
			counts[string(c.Status)+": "+c.Reason]++
		}
	}
	if len(counts) == 0 {
		return
	}
	reasons := sortedByCount(counts)
	b.WriteString("## Cases not compared\n\n| Cases | Reason |\n|---|---|\n")
	for i, reason := range reasons {
		if i == summaryReasons {
			fmt.Fprintf(b, "| ... | %d more reasons |\n", len(reasons)-i)
			break
		}
		fmt.Fprintf(b, "| %d | %s |\n", counts[reason], cell(reason))
	}
	b.WriteString("\n")
}

func renderDomains(b *bytes.Buffer, r *Run) {
	b.WriteString("## Domains\n\n")
	exercised, mismatching := domainCaseCounts(r.Cases)
	for _, d := range parity.AllDomains() {
		fmt.Fprintf(b, "### %s\n\n%d cases exercised this domain, %d of them mismatch.\n\n", d, exercised[d], mismatching[d])
		var rows []ClusterEntry
		for _, c := range r.Clusters {
			if c.Domain == d {
				rows = append(rows, c)
			}
		}
		if len(rows) == 0 {
			continue
		}
		b.WriteString("| Cluster | Stage | Kind | Path | Primary / records | Cases | Repro |\n|---|---|---|---|---|---|---|\n")
		for i, c := range rows {
			if i == summaryClustersPerDomain {
				fmt.Fprintf(b, "| ... | %d more clusters | | | | | |\n", len(rows)-i)
				break
			}
			fmt.Fprintf(b, "| `%s` | %s | %s | `%s` | %d / %d | %d | %s |\n", short(c.Signature), c.Stage, kindLabel(c),
				cell(c.PathPattern), c.PrimaryCount, c.Count, len(c.Cases), repro(c))
		}
		b.WriteString("\n")
	}
}

// domainCaseCounts counts the compared cases that exercised and that
// mismatched each domain.
func domainCaseCounts(cases []CaseSummary) (map[parity.Domain]int, map[parity.Domain]int) {
	exercised, mismatching := map[parity.Domain]int{}, map[parity.Domain]int{}
	for _, c := range cases {
		if !c.Status.Compared() {
			continue
		}
		for _, d := range c.ExercisedDomains {
			exercised[d]++
		}
		for _, d := range c.MismatchDomains {
			mismatching[d]++
		}
	}
	return exercised, mismatching
}

func kindLabel(c ClusterEntry) string {
	if len(c.Classes) == 0 {
		return string(c.Kind)
	}
	return string(c.Kind) + " (" + strings.Join(c.Classes, ", ") + ")"
}

// repro links the minimized envelope when there is one, else the case directory.
func repro(c ClusterEntry) string {
	switch {
	case c.MinEnvelope != "":
		return "`" + c.MinEnvelope + "`"
	case c.CaseDir != "":
		return "`" + c.CaseDir + "`"
	}
	return ""
}

func renderFixtures(b *bytes.Buffer, results []FixtureResult) {
	t := fixtureTotals(results)
	if t == nil {
		return
	}
	fmt.Fprintf(b, "## Adapter fixture self-test\n\n%d fixtures: %d passing, %d failing, %d excused, %d not run.\n\n",
		t.Total, t.Passing, t.Failing, t.Excused, t.NotRun)
	shown := 0
	for _, r := range results {
		if r.Status != FixtureFailing {
			continue
		}
		if shown == 0 {
			b.WriteString("| Fixture | First failure |\n|---|---|\n")
		}
		if shown == summaryFixtureFailures {
			fmt.Fprintf(b, "| ... | %d more failing fixtures |\n", t.Failing-shown)
			break
		}
		first := ""
		if len(r.Failures) > 0 {
			first = r.Failures[0]
		}
		fmt.Fprintf(b, "| `%s` | %s |\n", cell(r.CaseID), cell(truncate(first, 200)))
		shown++
	}
	b.WriteString("\n")
}

// cell makes s safe inside a Markdown table cell.
func cell(s string) string {
	s = strings.ReplaceAll(s, "\n", " ")
	return strings.ReplaceAll(s, "|", `\|`)
}

func short(sig string) string {
	if len(sig) > shortSignature {
		return sig[:shortSignature]
	}
	return sig
}

// truncate shortens s to at most n bytes on a rune boundary.
func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n] + "..."
}

// sortedByCount returns the keys by descending count, then by key.
func sortedByCount(counts map[string]int) []string {
	keys := make([]string, 0, len(counts))
	for k := range counts {
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool {
		if counts[keys[i]] != counts[keys[j]] {
			return counts[keys[i]] > counts[keys[j]]
		}
		return keys[i] < keys[j]
	})
	return keys
}
