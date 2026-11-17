// Package rhgate judges a ReplayHarness baseline run: the JSON report of
// `replayharness run MAP --target B --baseline A`, where side A is the Go
// server, the reference, and side B the implementation under test.
//
// ReplayHarness compares each answer of B with A's answer to the same probe
// on the status and on the shape of a JSON body, nothing else
// (internal/check/check.go, func baseline, at rhreplay.PinnedCommit): a
// mismatch is a "divergence" finding, of severity error when the statuses
// differ, warning when only the shape of the body does, and info when A
// did not answer. Judge counts those, and the probes B did not answer while
// A did, which ReplayHarness reports as connection or timeout findings
// instead. Given the report of the same probes run on A alone, Judge also
// compares the findings each side draws, so a difference the probes see in
// headers or bodies of the same shape (a Content-Type, a leaked stack trace)
// fails the gate too. Byte-for-byte equality is the parity runner's job
// (parity run --corpus replayharness).
package rhgate

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sort"
	"strings"
)

// Checks of a ReplayHarness report the gate treats apart.
const (
	// CheckDivergence is the check that compares B with A.
	CheckDivergence = "divergence"
	checkSlow       = "slow"
	checkTimeout    = "timeout"
	checkConnection = "connection"
	reportTool      = "replayharness"
	// sevShape and shapePrefix mark a divergence in the shape of the body
	// alone, when both sides answered with the same status (func baseline).
	// A divergence that does not match both counts, whatever the endpoint.
	sevShape    = "warning"
	shapePrefix = "response shape differs from the baseline"
)

// CookieSync is the endpoint whose shape divergences the gate does not count.
const CookieSync = "POST /cookie_sync"

// cookieSyncReason is why the shape divergences of CookieSync are not
// counted. Only the body depends on the syncer order, so its status
// divergences, and the findings one side draws, still count.
const cookieSyncReason = "Go answers it nondeterministically: without a bidder list, or with coopSync, it takes the " +
	"syncers from ranging over a map (usersync/chooser.go) and shuffles them (usersync/shuffler.go), so two " +
	"Go servers sync different bidders and the debug list comes and goes; that order changes the shape of the " +
	"body, never the status, so a status divergence still counts; the parity runner compares the cookie_sync " +
	"requests that list their bidders"

// ErrBadReport is a file that is not a ReplayHarness JSON report.
var ErrBadReport = errors.New("rhgate: not a ReplayHarness JSON report")

// Report is the part of a ReplayHarness JSON report the gate reads.
type Report struct {
	Tool        string   `json:"tool"`
	Version     string   `json:"version"`
	Target      string   `json:"target"`
	Baseline    string   `json:"baseline,omitempty"`
	Interrupted bool     `json:"interrupted,omitempty"`
	Results     []Result `json:"results"`
}

// Result is one probe and what the sides answered.
type Result struct {
	Scenario Scenario   `json:"scenario"`
	Observed []Observed `json:"observed"`
	Baseline *Observed  `json:"baseline,omitempty"`
	Findings []Finding  `json:"findings,omitempty"`
}

// Scenario names a probe.
type Scenario struct {
	ID       string `json:"id"`
	Endpoint string `json:"endpoint"`
	Name     string `json:"name"`
}

// Observed is one answer.
type Observed struct {
	Status int    `json:"status,omitempty"`
	Error  string `json:"error,omitempty"`
}

// Finding is what a check found in an answer.
type Finding struct {
	Check    string `json:"check"`
	Severity string `json:"severity"`
	Message  string `json:"message"`
}

func (f Finding) String() string { return f.Severity + " " + f.Check + ": " + f.Message }

// Read decodes a ReplayHarness JSON report.
func Read(r io.Reader) (*Report, error) {
	var rep Report
	if err := json.NewDecoder(r).Decode(&rep); err != nil {
		return nil, fmt.Errorf("%w: %w", ErrBadReport, err)
	}

	if rep.Tool != reportTool {
		return nil, fmt.Errorf("%w: tool %q", ErrBadReport, rep.Tool)
	}

	return &rep, nil
}

// ShapeNotComparedReason returns why the gate does not count the shape
// divergences of an endpoint ("METHOD /path"), or "". Every other difference
// on that endpoint counts.
func ShapeNotComparedReason(endpoint string) string {
	if endpoint == CookieSync {
		return cookieSyncReason
	}

	return ""
}

// Item is one difference between the sides on one probe.
type Item struct {
	Endpoint string `json:"endpoint"`
	Scenario string `json:"scenario"`
	Name     string `json:"name"`
	What     string `json:"what"`
}

// Judge compares the sides of the baseline report base. ref, when not nil,
// is the report of the same probes run on side A alone.
func Judge(base, ref *Report) *Verdict {
	v := &Verdict{Target: base.Target, Baseline: base.Baseline, Version: base.Version, Probes: len(base.Results)}

	switch {
	case base.Baseline == "":
		v.Incomplete = append(v.Incomplete, "the report is not of a --baseline run")
	case base.Interrupted:
		v.Incomplete = append(v.Incomplete, "the run was interrupted")
	case len(base.Results) == 0:
		v.Incomplete = append(v.Incomplete, "the report has no probes")
	}

	for _, r := range base.Results {
		for _, d := range divergences(r) {
			v.add(&v.Divergences, r, d.what, d.shape)
		}
	}

	if ref != nil {
		v.compareFindings(base, ref)
	}

	for _, items := range [][]Item{v.Divergences, v.OnlyB, v.OnlyA, v.NotCompared} {
		sortItems(items)
	}

	return v
}

// Verdict is the outcome of Judge.
type Verdict struct {
	Target      string   `json:"target"`
	Baseline    string   `json:"baseline"`
	Version     string   `json:"version"`
	Probes      int      `json:"probes"`
	Incomplete  []string `json:"incomplete,omitempty"`
	Divergences []Item   `json:"divergences,omitempty"`
	OnlyB       []Item   `json:"only_b,omitempty"`
	OnlyA       []Item   `json:"only_a,omitempty"`
	// NotCompared are the shape divergences on the endpoints
	// ShapeNotComparedReason names: printed, not counted.
	NotCompared []Item `json:"not_compared,omitempty"`
	// Shared counts the findings both sides draw: A is the reference, so
	// they are not failures here.
	Shared int `json:"shared"`
	// Compared reports whether the findings were compared with a report of
	// A alone.
	Compared bool `json:"findings_compared"`
}

// Failed reports whether the sides differ on a compared probe.
func (v *Verdict) Failed() bool {
	return len(v.Divergences)+len(v.OnlyB)+len(v.OnlyA) > 0
}

// Write prints the verdict for people.
func (v *Verdict) Write(w io.Writer) error {
	var b strings.Builder

	fmt.Fprintf(&b, "%d probes on side B %s against side A %s (ReplayHarness %s)\n", v.Probes, v.Target, v.Baseline, v.Version)

	if v.Compared {
		fmt.Fprintf(&b, "%d divergences, %d findings only side B draws, %d only side A draws; %d findings both draw (side A is the reference)\n",
			len(v.Divergences), len(v.OnlyB), len(v.OnlyA), v.Shared)
	} else {
		fmt.Fprintf(&b, "%d divergences (findings not compared: no report of side A alone)\n", len(v.Divergences))
	}

	for _, list := range [][]Item{v.Divergences, v.OnlyB, v.OnlyA} {
		for _, it := range list {
			fmt.Fprintf(&b, "  %s  %s (%s): %s\n", it.Endpoint, it.Name, it.Scenario, it.What)
		}
	}

	if len(v.NotCompared) > 0 {
		fmt.Fprintf(&b, "%d shape divergences on endpoints whose shape is not compared, reported and not counted:\n", len(v.NotCompared))

		reasons := map[string]bool{}

		for _, it := range v.NotCompared {
			fmt.Fprintf(&b, "  %s  %s (%s): %s\n", it.Endpoint, it.Name, it.Scenario, it.What)

			if !reasons[it.Endpoint] {
				reasons[it.Endpoint] = true
				fmt.Fprintf(&b, "    shape not compared: %s\n", ShapeNotComparedReason(it.Endpoint))
			}
		}
	}

	for _, why := range v.Incomplete {
		fmt.Fprintf(&b, "incomplete: %s\n", why)
	}

	if _, err := io.WriteString(w, b.String()); err != nil {
		return fmt.Errorf("rhgate: writing the verdict: %w", err)
	}

	return nil
}

// add files a difference under list, or, when it is a shape divergence on
// an endpoint whose shape is not compared, under NotCompared.
func (v *Verdict) add(list *[]Item, r Result, what string, shape bool) {
	item := Item{Endpoint: r.Scenario.Endpoint, Scenario: r.Scenario.ID, Name: r.Scenario.Name, What: what}
	if shape && ShapeNotComparedReason(r.Scenario.Endpoint) != "" {
		v.NotCompared = append(v.NotCompared, item)

		return
	}

	*list = append(*list, item)
}

// compareFindings files the findings one side draws on a probe and the
// other does not. Timing (slow), and answering at all (connection,
// timeout), which divergences already covers, are left out.
func (v *Verdict) compareFindings(base, ref *Report) {
	v.Compared = true

	if ref.Version != base.Version {
		v.Incomplete = append(v.Incomplete, fmt.Sprintf("the report of side A alone is of ReplayHarness %s, the baseline run of %s", ref.Version, base.Version))
	}

	onA := map[string]Result{}
	for _, r := range ref.Results {
		onA[r.Scenario.ID] = r
	}

	for _, r := range base.Results {
		a, ok := onA[r.Scenario.ID]
		if !ok {
			v.Incomplete = append(v.Incomplete, "the report of side A alone has no probe "+r.Scenario.ID)

			continue
		}

		delete(onA, r.Scenario.ID)

		fb, fa := drawn(r.Findings), drawn(a.Findings)

		for key := range fb {
			if fa[key] {
				v.Shared++

				continue
			}

			v.add(&v.OnlyB, r, "only side B: "+key, false)
		}

		for key := range fa {
			if !fb[key] {
				v.add(&v.OnlyA, r, "only side A: "+key, false)
			}
		}
	}

	if len(onA) > 0 {
		v.Incomplete = append(v.Incomplete, fmt.Sprintf("the baseline run lacks %d probes of the report of side A alone", len(onA)))
	}
}

// divergence is one difference between the sides' answers to a probe.
type divergence struct {
	what string
	// shape marks a divergence in the shape of the body alone.
	shape bool
}

// divergences returns what differs between the sides on one probe.
func divergences(r Result) []divergence {
	if len(r.Observed) == 0 {
		return []divergence{{what: "side B has no answer in the report"}}
	}

	if r.Baseline == nil {
		return []divergence{{what: "side A has no answer in the report"}}
	}

	var out []divergence

	for _, f := range r.Findings {
		if f.Check == CheckDivergence {
			shape := f.Severity == sevShape && strings.HasPrefix(f.Message, shapePrefix)
			out = append(out, divergence{what: f.String(), shape: shape})
		}
	}

	if b, a := r.Observed[0], r.Baseline; b.Status == 0 && a.Status != 0 {
		out = append(out, divergence{what: fmt.Sprintf("side B did not answer (%s); side A answered %d", b.Error, a.Status)})
	}

	return out
}

// drawn returns the findings the gate compares between the sides.
func drawn(findings []Finding) map[string]bool {
	out := map[string]bool{}

	for _, f := range findings {
		switch f.Check {
		case CheckDivergence, checkSlow, checkTimeout, checkConnection:
			continue
		}

		out[f.String()] = true
	}

	return out
}

func sortItems(items []Item) {
	sort.Slice(items, func(i, j int) bool {
		a, b := items[i], items[j]
		if a.Endpoint != b.Endpoint {
			return a.Endpoint < b.Endpoint
		}

		if a.Scenario != b.Scenario {
			return a.Scenario < b.Scenario
		}

		return a.What < b.What
	})
}
