package report

import (
	"fmt"
	"sort"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/diff"
)

// Baseline is parity/baseline.json: the mismatch totals CI ratchets against.
// Mismatch counts may only go down; the goal is zero.
type Baseline struct {
	Schema        string `json:"schema"`
	SchemaVersion string `json:"schema_version"`
	// RunID is the run the baseline was taken from.
	RunID string `json:"run_id"`
	ImplA string `json:"impl_a"`
	ImplB string `json:"impl_b"`
	// Compared is the number of cases both implementations were compared on;
	// it may only go up, so a server that stops answering cannot turn its
	// mismatches into a pass.
	Compared   int            `json:"compared"`
	Errors     int            `json:"errors"`
	Mismatches int            `json:"mismatches"`
	ByKind     map[string]int `json:"by_kind"`
	// Clusters maps every cluster signature to its record count.
	Clusters map[string]int `json:"clusters"`
}

// BaselineOf returns the baseline of a run.
func BaselineOf(r *Run) Baseline {
	b := Baseline{
		Schema: SchemaBaseline, SchemaVersion: parity.SchemaVersion, RunID: r.Summary.RunID,
		ImplA: r.Summary.ImplA, ImplB: r.Summary.ImplB,
		Compared: r.Summary.Counts.Compared(), Errors: r.Summary.Counts.Error,
		ByKind: map[string]int{}, Clusters: map[string]int{},
	}
	for kind, n := range diff.CountByKind(r.Diffs) {
		b.ByKind[string(kind)] = n
		b.Mismatches += n
	}
	for _, c := range r.Clusters {
		b.Clusters[c.Signature] = c.Count
	}
	return b
}

// LoadBaseline reads a baseline file.
func LoadBaseline(path string) (Baseline, error) {
	var b Baseline
	if err := ReadJSON(path, &b); err != nil {
		return Baseline{}, err
	}
	if b.Schema != SchemaBaseline {
		return Baseline{}, fmt.Errorf("report: %s: schema %q, want %q", path, b.Schema, SchemaBaseline)
	}
	return b, nil
}

// Ratchet is the outcome of comparing a run with the baseline.
type Ratchet struct {
	// OK is false when anything regressed.
	OK           bool     `json:"ok"`
	Regressions  []string `json:"regressions"`
	Improvements []string `json:"improvements"`
}

// CheckRatchet compares the current totals with the baseline. Regressions:
// more mismatches in total or of any kind, a cluster signature the baseline
// does not have, more harness errors, or fewer compared cases.
// Improvements are reported so the baseline can be tightened.
func CheckRatchet(base, cur Baseline) Ratchet {
	r := Ratchet{Regressions: []string{}, Improvements: []string{}}
	r.compare("mismatches", base.Mismatches, cur.Mismatches, false)
	r.compare("harness errors", base.Errors, cur.Errors, false)
	r.compare("compared cases", base.Compared, cur.Compared, true)
	for _, kind := range unionKeys(base.ByKind, cur.ByKind) {
		r.compare("mismatches of kind "+kind, base.ByKind[kind], cur.ByKind[kind], false)
	}
	for _, sig := range unionKeys(base.Clusters, cur.Clusters) {
		_, inBase := base.Clusters[sig]
		_, inCur := cur.Clusters[sig]
		switch {
		case inCur && !inBase:
			r.Regressions = append(r.Regressions, fmt.Sprintf("new cluster %s (%d records)", sig, cur.Clusters[sig]))
		case inBase && !inCur:
			r.Improvements = append(r.Improvements, fmt.Sprintf("cluster %s is gone (was %d records)", sig, base.Clusters[sig]))
		}
	}
	r.OK = len(r.Regressions) == 0
	return r
}

// compare records a regression or an improvement of one total. higherIsBetter
// flips the direction.
func (r *Ratchet) compare(what string, base, cur int, higherIsBetter bool) {
	if base == cur {
		return
	}
	worse := cur > base
	if higherIsBetter {
		worse = cur < base
	}
	line := fmt.Sprintf("%s: %d -> %d", what, base, cur)
	if worse {
		r.Regressions = append(r.Regressions, line)
		return
	}
	r.Improvements = append(r.Improvements, line)
}

func unionKeys(a, b map[string]int) []string {
	set := map[string]bool{}
	for k := range a {
		set[k] = true
	}
	for k := range b {
		set[k] = true
	}
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
