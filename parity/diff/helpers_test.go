package diff

import (
	"encoding/json"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

// newTrace returns an empty trace document of one implementation.
func newTrace(impl string) *parity.Trace {
	return &parity.Trace{
		Schema: parity.SchemaTrace, SchemaVersion: parity.SchemaVersion, Impl: impl,
		CaseID: "test", Endpoint: "/openrtb2/auction",
		Stages:      map[parity.StageID]map[string]parity.StageEntry{},
		StageStatus: map[parity.StageID]parity.StageStatus{},
	}
}

// rawEntry is a stage entry recorded from exact bytes.
func rawEntry(raw string) parity.StageEntry {
	e := parity.StageEntry{Seq: 1, Writes: 1, Raw: &raw, Errors: []parity.ErrorRecord{}, Warnings: []parity.ErrorRecord{}}
	if json.Valid([]byte(raw)) {
		e.JSON = json.RawMessage(raw)
	}
	return e
}

// put records one stage entry.
func put(t *parity.Trace, stage parity.StageID, key, raw string) {
	if t.Stages[stage] == nil {
		t.Stages[stage] = map[string]parity.StageEntry{}
	}
	t.Stages[stage][key] = rawEntry(raw)
}

// textResponse is an HTTP response with an identity text body.
func textResponse(status int, body string, headers ...[2]string) *parity.HTTPResponse {
	return &parity.HTTPResponse{Status: status, Headers: parity.HeaderList(headers), Body: parity.TextBody(body)}
}

// compareTraces runs the engine on two traces and fails the test on error.
func compareTraces(t *testing.T, e *Engine, c Case, g, r *parity.Trace) parity.DiffDocument {
	t.Helper()
	res, err := e.CompareTraces(c, g, r)
	if err != nil {
		t.Fatalf("CompareTraces: %v", err)
	}
	validateDiff(t, res.Doc)
	return res.Doc
}

// diffEntries compares one stage entry of each side and returns the records.
func diffEntries(t *testing.T, e *Engine, stage parity.StageID, goRaw, rustRaw string) []parity.MismatchRecord {
	t.Helper()
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	put(g, stage, parity.UnkeyedStage, goRaw)
	put(r, stage, parity.UnkeyedStage, rustRaw)
	return compareTraces(t, e, Case{CaseID: "entry"}, g, r).Mismatches
}

// brief is the identity of a record used in table expectations.
type brief struct {
	kind  parity.MismatchKind
	class string
	path  string
}

func briefs(records []parity.MismatchRecord) []brief {
	out := make([]brief, 0, len(records))
	for _, m := range records {
		out = append(out, brief{kind: m.Kind, class: m.Class, path: m.JSONPath})
	}
	return out
}

func equalBriefs(a, b []brief) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// mustRules loads the embedded allowlist.
func mustRules(t *testing.T) *Rules {
	t.Helper()
	rules, err := LoadRules()
	if err != nil {
		t.Fatalf("LoadRules: %v", err)
	}
	return rules
}
