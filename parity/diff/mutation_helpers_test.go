package diff

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

// mutationDir holds the baseline of the mutation tests: one Go trace recorded
// by the A/A self-test (an auction with two bidders, Prebid Cache, targeting
// and analytics; the Go runtime metric series removed) and its envelope.
const mutationDir = "../testdata/diff/mutation"

// loadMutationCase returns the case and two copies of the baseline trace,
// the second relabelled as the Rust side.
func loadMutationCase(t *testing.T) (Case, *parity.Trace, *parity.Trace) {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(mutationDir, "auction.go.trace.json"))
	if err != nil {
		t.Fatalf("reading the baseline trace: %v", err)
	}
	var g, r parity.Trace
	if err := json.Unmarshal(raw, &g); err != nil {
		t.Fatalf("decoding the baseline trace: %v", err)
	}
	if err := json.Unmarshal(raw, &r); err != nil {
		t.Fatalf("decoding the baseline trace: %v", err)
	}
	r.Impl = parity.ImplRust
	envRaw, err := os.ReadFile(filepath.Join(mutationDir, "auction.envelope.json"))
	if err != nil {
		t.Fatalf("reading the baseline envelope: %v", err)
	}
	var env parity.Envelope
	if err := json.Unmarshal(envRaw, &env); err != nil {
		t.Fatalf("decoding the baseline envelope: %v", err)
	}
	return Case{CaseID: g.CaseID, Endpoint: g.Endpoint, Envelope: &env}, &g, &r
}

// mutant edits the two sides of one mutation.
type mutant struct {
	t    *testing.T
	g, r *parity.Trace
}

// stageIndex is the position of a stage in registry (pipeline) order.
func stageIndex(stage parity.StageID) int {
	for i, s := range parity.AllStages() {
		if s == stage {
			return i
		}
	}
	return -1
}

// editEntry replaces old with new in one Rust stage entry, which must
// contain old.
func (m *mutant) editEntry(stage parity.StageID, key, old, new string) {
	m.t.Helper()
	e, ok := m.r.Stages[stage][key]
	if !ok || !replaceEntry(&e, old, new) {
		m.t.Fatalf("%s[%s] does not contain %s", stage, key, old)
	}
	m.r.Stages[stage][key] = e
}

// cascade models a Rust bug introduced at stage from: old becomes new there
// and in everything recorded after it (later stage entries, outbound calls,
// analytics and the response). With bidder set only that bidder's keyed
// entries and outbound calls change. The from stage must contain old.
func (m *mutant) cascade(from parity.StageID, bidder, old, new string) {
	m.t.Helper()
	at, hit := stageIndex(from), false
	for stage, keyed := range m.r.Stages {
		if stageIndex(stage) < at {
			continue
		}
		for key, e := range keyed {
			if bidder != "" && key != bidder && !strings.HasPrefix(key, bidder+"#") {
				continue
			}
			if replaceEntry(&e, old, new) {
				keyed[key] = e
				hit = hit || stage == from
			}
		}
	}
	if !hit {
		m.t.Fatalf("%s does not contain %s", from, old)
	}
	if stageIndex(parity.StageHTTPOut) >= at {
		for i := range m.r.HTTPOut {
			c := &m.r.HTTPOut[i]
			if bidder != "" && c.Bidder != bidder {
				continue
			}
			replaceBody(m.t, c.Request.Body, old, new)
			if c.Response != nil {
				replaceBody(m.t, c.Response.Body, old, new)
			}
		}
	}
	if bidder != "" {
		return
	}
	for i := range m.r.Analytics {
		m.r.Analytics[i].Object = json.RawMessage(replaceBoth(string(m.r.Analytics[i].Object), old, new))
	}
	if m.r.HTTPResponse != nil {
		replaceBody(m.t, m.r.HTTPResponse.Body, old, new)
	}
}

// replaceBoth replaces old in s both as written and as it is spelled inside
// a JSON string (bodies nested in a stage value as {"text": "..."}).
func replaceBoth(s, old, new string) string {
	s = strings.ReplaceAll(s, old, new)
	if q, nq := jsonQuote(old), jsonQuote(new); q != `"`+old+`"` {
		s = strings.ReplaceAll(s, q[1:len(q)-1], nq[1:len(nq)-1])
	}
	return s
}

// replaceEntry edits an entry's exact bytes and keeps its json in step.
func replaceEntry(e *parity.StageEntry, old, new string) bool {
	if e.Raw == nil {
		return false
	}
	s := replaceBoth(*e.Raw, old, new)
	if s == *e.Raw {
		return false
	}
	e.Raw = &s
	if json.Valid([]byte(s)) {
		e.JSON = json.RawMessage(s)
	}
	return true
}

// replaceBody edits a body's decoded content, re-encoding it the way it was
// recorded. The headers (Content-Length) are left as recorded.
func replaceBody(t *testing.T, b *parity.Body, old, new string) bool {
	t.Helper()
	if b == nil {
		return false
	}
	content, err := b.DecodedBytes()
	if err != nil {
		t.Fatalf("decoding a body: %v", err)
	}
	s := strings.ReplaceAll(string(content), old, new)
	if s == string(content) {
		return false
	}
	if b.B64 == nil {
		*b = *parity.NewBody([]byte(s), b.ContentEncoding)
		return true
	}
	wire, err := (&parity.Body{Text: &s, ContentEncoding: b.ContentEncoding}).WireBytes()
	if err != nil {
		t.Fatalf("re-encoding a body: %v", err)
	}
	*b = *parity.NewBody(wire, b.ContentEncoding)
	return true
}

// call returns the Rust side's outbound call of kind and bidder.
func (m *mutant) call(kind, bidder string) *parity.OutboundCall {
	m.t.Helper()
	for i := range m.r.HTTPOut {
		if c := &m.r.HTTPOut[i]; c.Kind == kind && c.Bidder == bidder {
			return c
		}
	}
	m.t.Fatalf("no %s call for %q", kind, bidder)
	return nil
}

// editHeaders applies fn to the header named name (exact spelling), which
// must exist; fn returns false to drop the header.
func editHeaders(t *testing.T, h parity.HeaderList, name string, fn func(*[2]string) bool) parity.HeaderList {
	t.Helper()
	out, found := parity.HeaderList{}, false
	for _, kv := range h {
		if kv[0] == name {
			found = true
			if !fn(&kv) {
				continue
			}
		}
		out = append(out, kv)
	}
	if !found {
		t.Fatalf("no header %s in %v", name, h)
	}
	return out
}

func dropHeader(t *testing.T, h parity.HeaderList, name string) parity.HeaderList {
	return editHeaders(t, h, name, func(*[2]string) bool { return false })
}

func renameHeader(t *testing.T, h parity.HeaderList, from, to string) parity.HeaderList {
	return editHeaders(t, h, from, func(kv *[2]string) bool { kv[0] = to; return true })
}

func setHeader(t *testing.T, h parity.HeaderList, name, value string) parity.HeaderList {
	return editHeaders(t, h, name, func(kv *[2]string) bool { kv[1] = value; return true })
}

// metric returns the Rust side's delta of name whose labels include labels.
func (m *mutant) metric(name string, labels map[string]string) *parity.MetricDelta {
	m.t.Helper()
	for i := range m.r.MetricsDelta {
		d := &m.r.MetricsDelta[i]
		if d.Name != name {
			continue
		}
		match := true
		for k, v := range labels {
			match = match && d.Labels[k] == v
		}
		if match {
			return d
		}
	}
	m.t.Fatalf("no metric %s%v", name, labels)
	return nil
}

// swapFirstTwo swaps the first two elements of the array member of a compact
// JSON object document, keeping every other byte, including the trailing
// newline.
func swapFirstTwo(t *testing.T, doc, member string) string {
	t.Helper()
	dec := json.NewDecoder(strings.NewReader(doc))
	if tok, err := dec.Token(); err != nil || tok != json.Delim('{') {
		t.Fatalf("not an object: %v", err)
	}
	var parts []string
	for dec.More() {
		key, err := dec.Token()
		if err != nil {
			t.Fatalf("member name: %v", err)
		}
		var v json.RawMessage
		if err := dec.Decode(&v); err != nil {
			t.Fatalf("member %v: %v", key, err)
		}
		if key == member {
			var elems []json.RawMessage
			if err := json.Unmarshal(v, &elems); err != nil || len(elems) < 2 {
				t.Fatalf("member %s is not an array of two or more: %v", member, err)
			}
			elems[0], elems[1] = elems[1], elems[0]
			v = json.RawMessage("[" + string(bytes.Join(asBytes(elems), []byte(","))) + "]")
		}
		parts = append(parts, jsonQuote(key.(string))+":"+string(v))
	}
	if tok, err := dec.Token(); err != nil || tok != json.Delim('}') {
		t.Fatalf("unterminated object: %v", err)
	}
	out := "{" + strings.Join(parts, ",") + "}" + doc[dec.InputOffset():]
	if len(out) != len(doc) || out == doc {
		t.Fatalf("rebuilding the document changed its length or nothing: %d -> %d", len(doc), len(out))
		t.Fatalf("rebuilding the document changed its length: %d -> %d", len(doc), len(out))
	}
	return out
}

func asBytes(raws []json.RawMessage) [][]byte {
	out := make([][]byte, len(raws))
	for i, r := range raws {
		out[i] = r
	}
	return out
}

// expect is the attribution a mutation must get: the stage of every primary
// record, and one primary record of this kind and class whose path ends in
// pathSuffix (when set).
type expect struct {
	stage      parity.StageID
	kind       parity.MismatchKind
	class      string
	pathSuffix string
	// downstream, when set, is a later stage that must carry a record
	// marked consequential of the primary one.
	downstream parity.StageID
}

func checkAttribution(t *testing.T, doc parity.DiffDocument, want expect) {
	t.Helper()
	if len(doc.Mismatches) == 0 {
		t.Fatal("the seeded difference was not detected")
	}
	matched, consequential := false, want.downstream == ""
	for _, m := range doc.Mismatches {
		if m.Primary && m.Stage != want.stage {
			t.Errorf("primary record at %s, want only %s: %s", m.Stage, want.stage, recordID(m))
		}
		if m.Primary && m.Kind == want.kind && m.Class == want.class && strings.HasSuffix(m.JSONPath, want.pathSuffix) {
			matched = true
		}
		if m.Stage == want.downstream && !m.Primary && m.ConsequentialOf != nil {
			consequential = true
		}
	}
	if !matched {
		t.Errorf("no primary %s/%s record at %s ending in %q: %v", want.kind, want.class, want.stage, want.pathSuffix, ids(doc))
	}
	if !consequential {
		t.Errorf("no consequential record at %s: %v", want.downstream, ids(doc))
	}
}
