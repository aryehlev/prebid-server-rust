package diff

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// goldenDir holds the golden diff cases: a Go trace, a Rust trace and the
// exact list of mismatches the engine must produce for them.
const goldenDir = "../testdata/diff"

// goldenCase is one file of goldenDir.
type goldenCase struct {
	Description string           `json:"description"`
	Case        goldenCaseID     `json:"case"`
	Envelope    *parity.Envelope `json:"envelope"`
	Go          parity.Trace     `json:"go"`
	Rust        parity.Trace     `json:"rust"`
	Want        []goldenWant     `json:"want"`
}

type goldenCaseID struct {
	CaseID      string `json:"case_id"`
	FixturePath string `json:"fixture_path"`
	Endpoint    string `json:"endpoint"`
	Profile     string `json:"profile"`
}

// goldenWant is one expected mismatch. Domain and requirement are checked
// only when set; consequential_of_stage names the stage whose first primary
// signature the record must point to.
type goldenWant struct {
	Stage                parity.StageID      `json:"stage"`
	Key                  string              `json:"key"`
	Kind                 parity.MismatchKind `json:"kind"`
	Class                string              `json:"class"`
	JSONPath             string              `json:"json_path"`
	Primary              bool                `json:"primary"`
	Domain               parity.Domain       `json:"domain"`
	Requirement          string              `json:"requirement"`
	ConsequentialOfStage parity.StageID      `json:"consequential_of_stage"`
}

func (w goldenWant) id() string {
	return fmt.Sprintf("%s/%s %s:%s %s", w.Stage, w.Key, w.Kind, w.Class, w.JSONPath)
}

func recordID(m parity.MismatchRecord) string {
	return fmt.Sprintf("%s/%s %s:%s %s", m.Stage, m.Key, m.Kind, m.Class, m.JSONPath)
}

func TestGoldenDiffs(t *testing.T) {
	files, err := filepath.Glob(filepath.Join(goldenDir, "*.json"))
	if err != nil || len(files) == 0 {
		t.Fatalf("no golden cases in %s: %v", goldenDir, err)
	}
	engine := newTestEngine(t)
	for _, file := range files {
		t.Run(strings.TrimSuffix(filepath.Base(file), ".json"), func(t *testing.T) {
			g := loadGolden(t, file)
			c := Case{
				CaseID: g.Case.CaseID, FixturePath: g.Case.FixturePath,
				Endpoint: g.Case.Endpoint, Profile: g.Case.Profile, Envelope: g.Envelope,
			}
			res, err := engine.CompareTraces(c, &g.Go, &g.Rust)
			if err != nil {
				t.Fatalf("CompareTraces: %v", err)
			}
			checkGolden(t, res.Doc, g.Want)
			validateDiff(t, res.Doc)
		})
	}
}

func loadGolden(t *testing.T, file string) goldenCase {
	t.Helper()
	raw, err := os.ReadFile(file)
	if err != nil {
		t.Fatalf("reading %s: %v", file, err)
	}
	var g goldenCase
	if err := json.Unmarshal(raw, &g); err != nil {
		t.Fatalf("decoding %s: %v", file, err)
	}
	if g.Description == "" {
		t.Fatalf("%s has no description", file)
	}
	return g
}

// checkGolden requires the document to hold exactly the wanted mismatches.
func checkGolden(t *testing.T, doc parity.DiffDocument, want []goldenWant) {
	t.Helper()
	for _, p := range goldenProblems(doc, want) {
		t.Error(p)
	}
}

// goldenProblems lists every way the document differs from the expectation.
func goldenProblems(doc parity.DiffDocument, want []goldenWant) []string {
	got := map[string]parity.MismatchRecord{}
	for _, m := range doc.Mismatches {
		got[recordID(m)] = m
	}
	var problems []string
	wantIDs := map[string]bool{}
	for _, w := range want {
		wantIDs[w.id()] = true
		m, ok := got[w.id()]
		if !ok {
			problems = append(problems, "missing mismatch "+w.id())
			continue
		}
		problems = append(problems, wantProblems(doc, w, m)...)
	}
	for _, m := range doc.Mismatches {
		if !wantIDs[recordID(m)] {
			problems = append(problems, fmt.Sprintf("unexpected mismatch %s (go=%s rust=%s)", recordID(m), m.Go, m.Rust))
		}
	}
	return problems
}

func wantProblems(doc parity.DiffDocument, w goldenWant, m parity.MismatchRecord) []string {
	var problems []string
	add := func(format string, args ...any) {
		problems = append(problems, w.id()+": "+fmt.Sprintf(format, args...))
	}
	if m.Primary != w.Primary {
		add("primary = %v, want %v", m.Primary, w.Primary)
	}
	if w.Domain != "" && m.Domain != w.Domain {
		add("domain = %s, want %s", m.Domain, w.Domain)
	}
	if w.Requirement != "" && m.Requirement != w.Requirement {
		add("requirement = %s, want %s", m.Requirement, w.Requirement)
	}
	switch {
	case w.ConsequentialOfStage == "" && m.ConsequentialOf != nil:
		add("consequential_of = %s, want null", *m.ConsequentialOf)
	case w.ConsequentialOfStage != "":
		wantSig := firstPrimarySignature(doc, w.ConsequentialOfStage)
		if m.ConsequentialOf == nil || *m.ConsequentialOf != wantSig {
			add("consequential_of = %v, want the first %s signature %s", m.ConsequentialOf, w.ConsequentialOfStage, wantSig)
		}
	}
	if m.Signature != Signature(m.Stage, m.Domain, m.JSONPath, m.Kind) {
		add("signature %s does not match its fields", m.Signature)
	}
	return problems
}

// TestGoldenCheckerDetectsDrift: the golden check is not vacuous; any change
// to an expectation is reported.
func TestGoldenCheckerDetectsDrift(t *testing.T) {
	g := loadGolden(t, filepath.Join(goldenDir, "first_divergence_marks_consequential.json"))
	res, err := newTestEngine(t).CompareTraces(Case{CaseID: g.Case.CaseID, Endpoint: g.Case.Endpoint}, &g.Go, &g.Rust)
	if err != nil {
		t.Fatal(err)
	}
	if p := goldenProblems(res.Doc, g.Want); len(p) != 0 {
		t.Fatalf("baseline: %v", p)
	}
	mutations := map[string]func([]goldenWant) []goldenWant{
		"drop a record": func(w []goldenWant) []goldenWant { return w[1:] },
		"add a record": func(w []goldenWant) []goldenWant {
			return append(w, goldenWant{Stage: "http.response", Key: "_", Kind: "value", JSONPath: "$"})
		},
		"flip primary":        func(w []goldenWant) []goldenWant { w[0].Primary = !w[0].Primary; return w },
		"other domain":        func(w []goldenWant) []goldenWant { w[0].Domain = parity.DomainEndpoints; return w },
		"other requirement":   func(w []goldenWant) []goldenWant { w[0].Requirement = "ENDP-01"; return w },
		"no consequence":      func(w []goldenWant) []goldenWant { w[1].ConsequentialOfStage = ""; return w },
		"other consequence":   func(w []goldenWant) []goldenWant { w[1].ConsequentialOfStage = parity.StageHTTPOut; return w },
		"consequence on root": func(w []goldenWant) []goldenWant { w[0].ConsequentialOfStage = parity.StageHTTPOut; return w },
	}
	for name, mutate := range mutations {
		t.Run(name, func(t *testing.T) {
			want := mutate(append([]goldenWant(nil), g.Want...))
			if p := goldenProblems(res.Doc, want); len(p) == 0 {
				t.Fatal("the change went unnoticed")
			}
		})
	}
}

func firstPrimarySignature(doc parity.DiffDocument, stage parity.StageID) string {
	for _, m := range doc.Mismatches {
		if m.Primary && m.Stage == stage {
			return m.Signature
		}
	}
	return ""
}

// validateDiff checks the document against mismatch.schema.json.
func validateDiff(t *testing.T, doc parity.DiffDocument) {
	t.Helper()
	raw, err := json.Marshal(doc)
	if err != nil {
		t.Fatalf("marshal diff: %v", err)
	}
	if err := schema.Validate(raw); err != nil {
		t.Fatalf("diff document fails its schema: %v\n%s", err, raw)
	}
}

func newTestEngine(t *testing.T) *Engine {
	t.Helper()
	e, err := New()
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	return e
}

// ids lists record IDs, sorted, for failure messages.
func ids(doc parity.DiffDocument) []string {
	out := make([]string, 0, len(doc.Mismatches))
	for _, m := range doc.Mismatches {
		out = append(out, recordID(m))
	}
	sort.Strings(out)
	return out
}
