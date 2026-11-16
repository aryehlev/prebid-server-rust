package diff

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity"
)

// esc spells JSON unicode escapes in test literals: "^u" becomes a backslash
// and u, so esc(`"^u0026"`) is the six-character escape of an ampersand.
func esc(s string) string { return strings.ReplaceAll(s, "^u", `\`+"u") }

// TestCompareEntryValues covers the semantic and lexical comparison of one
// exact stage value.
func TestCompareEntryValues(t *testing.T) {
	lex := func(class, path string) brief { return brief{parity.KindLexical, class, path} }
	sem := func(kind parity.MismatchKind, path string) brief { return brief{kind, "", path} }
	tests := []struct {
		name string
		goV  string
		rust string
		want []brief
	}{
		{"equal", `{"a":[1,"x",null,true]}`, `{"a":[1,"x",null,true]}`, nil},
		{"number spelling", `{"a":1}`, `{"a":1.0}`, []brief{lex(parity.LexicalNumberFormat, "$.a")}},
		{"exponent spelling", `{"a":1e2}`, `{"a":100}`, []brief{lex(parity.LexicalNumberFormat, "$.a")}},
		{"big integers by value", `{"a":12345678901234567890}`, `{"a":12345678901234567891}`, []brief{sem(parity.KindValue, "$.a")}},
		{"html escape", `{"a":"x&y"}`, esc(`{"a":"x^u0026y"}`), []brief{lex(parity.LexicalHTMLEscape, "$.a")}},
		{"html escape lt", esc(`{"a":"^u003cb"}`), `{"a":"<b"}`, []brief{lex(parity.LexicalHTMLEscape, "$.a")}},
		{"unicode escape", esc(`{"a":"^u00e9"}`), "{\"a\":\"\u00e9\"}", []brief{lex(parity.LexicalUnicodeEscape, "$.a")}},
		{"line separator escape", esc(`{"a":"^u2028"}`), "{\"a\":\"\u2028\"}", []brief{lex(parity.LexicalUnicodeEscape, "$.a")}},
		{"solidus escape", `{"a":"a/b"}`, `{"a":"a\/b"}`, []brief{lex(parity.LexicalUnicodeEscape, "$.a")}},
		{"surrogate pair", esc(`{"a":"^ud83d^ude00"}`), "{\"a\":\"\U0001F600\"}", []brief{lex(parity.LexicalUnicodeEscape, "$.a")}},
		{"escaped member name", esc(`{"^u0061":1}`), `{"a":1}`, nil},
		{"member order", `{"a":1,"b":2,"c":3}`, `{"a":1,"c":3,"b":2}`, []brief{lex(parity.LexicalMemberOrder, "$")}},
		{"member order ignores extra members", `{"a":1,"b":2}`, `{"x":0,"a":1,"b":2}`, []brief{sem(parity.KindExtraInRust, "$.x")}},
		{"whitespace once per document", `{"a":[1,2],"b":{"c":1}}`, `{ "a": [1, 2], "b": {"c": 1} }`, []brief{lex(parity.LexicalWhitespace, "$")}},
		{"trailing newline", "{\"a\":1}\n", `{"a":1}`, []brief{lex(parity.LexicalTrailingNewline, "$")}},
		{"trailing whitespace", "{\"a\":1}\n", "{\"a\":1}\t\n", []brief{lex(parity.LexicalWhitespace, "$")}},
		{"missing member", `{"a":1,"b":2}`, `{"a":1}`, []brief{sem(parity.KindMissingInRust, "$.b")}},
		{"null is not absent", `{"a":null}`, `{}`, []brief{sem(parity.KindMissingInRust, "$.a")}},
		{"type", `{"a":1}`, `{"a":"1"}`, []brief{sem(parity.KindType, "$.a")}},
		{"bool", `{"a":true}`, `{"a":false}`, []brief{sem(parity.KindValue, "$.a")}},
		{"array permutation", `{"a":[1,2,3]}`, `{"a":[3,1,2]}`, []brief{sem(parity.KindOrder, "$.a")}},
		{"permutation keeps lexical checks", `{"a":[1,2.0]}`, `{"a":[2,1]}`, []brief{sem(parity.KindOrder, "$.a"), lex(parity.LexicalNumberFormat, "$.a[1]")}},
		{"array insertion is aligned", `{"a":[{"id":1},{"id":2},{"id":3}]}`, `{"a":[{"id":1},{"id":3}]}`, []brief{sem(parity.KindMissingInRust, "$.a[1]")}},
		{"array extra element", `{"a":[1]}`, `{"a":[1,2]}`, []brief{sem(parity.KindExtraInRust, "$.a[1]")}},
		{"changed element compared in place", `{"a":[{"id":1,"v":1},{"id":2}]}`, `{"a":[{"id":1,"v":2},{"id":2}]}`, []brief{sem(parity.KindValue, "$.a[0].v")}},
		{"member names need brackets", `{"a b":{"it's":1}}`, `{"a b":{"it's":2}}`, []brief{sem(parity.KindValue, `$['a b']['it\'s']`)}},
		{"duplicate member: last wins", `{"a":1,"a":2}`, `{"a":2}`, nil},
		{"error objects by code", `{"e":[{"code":1,"message":"m"}]}`, `{"e":[{"code":2,"message":"m"}]}`, []brief{sem(parity.KindErrorCode, "$.e[0].code")}},
		{"error objects by text", `{"e":{"code":1,"message":"m"}}`, `{"e":{"code":1,"message":"n"}}`, []brief{sem(parity.KindErrorText, "$.e.message")}},
		{"not JSON compares as text", `not json`, `not json!`, []brief{sem(parity.KindValue, "$")}},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := briefs(diffEntries(t, e, parity.StageRequestValidated, tt.goV, tt.rust))
			if !equalBriefs(got, tt.want) {
				t.Fatalf("got %v\nwant %v", got, tt.want)
			}
		})
	}
}

// TestLexicalNeedsExactBytes: values recorded only as semantic json (no raw
// bytes) are compared semantically; their spelling is not an implementation
// output, and the engine notes it.
func TestLexicalNeedsExactBytes(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	g.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{"_": {JSON: json.RawMessage(`{"a":1,"b":"&"}`)}}
	r.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{"_": {JSON: json.RawMessage(`{"b":"&","a":1.0}`)}}
	res, err := e.CompareTraces(Case{CaseID: "semantic"}, g, r)
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Doc.Mismatches) != 0 {
		t.Fatalf("got %v", ids(res.Doc))
	}
	if len(res.Notes) != 2 || !strings.Contains(res.Notes[0], "no raw bytes") {
		t.Fatalf("notes = %v", res.Notes)
	}
}

// TestRawB64Entry: raw_b64 carries exact bytes that are not valid UTF-8 text.
func TestRawB64Entry(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	b64 := func(s string) *string { v := base64.StdEncoding.EncodeToString([]byte(s)); return &v }
	g.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{"_": {RawB64: b64(`{"a":1}`)}}
	r.Stages[parity.StageRequestValidated] = map[string]parity.StageEntry{"_": {RawB64: b64(`{"a":1.0}`)}}
	doc := compareTraces(t, e, Case{CaseID: "b64"}, g, r)
	if got := briefs(doc.Mismatches); !equalBriefs(got, []brief{{parity.KindLexical, parity.LexicalNumberFormat, "$.a"}}) {
		t.Fatalf("got %v", got)
	}
}

func gzipped(t *testing.T, s string) string {
	t.Helper()
	var b bytes.Buffer
	w := gzip.NewWriter(&b)
	if _, err := w.Write([]byte(s)); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	return base64.StdEncoding.EncodeToString(b.Bytes())
}

// TestResponseBodies: bodies are decoded before comparison; a different
// Content-Encoding is reported once, separately from the content.
func TestResponseBodies(t *testing.T) {
	e := newTestEngine(t)
	gz := gzipped(t, `{"id":"r1"}`)
	tests := []struct {
		name string
		goB  *parity.Body
		rust *parity.Body
		want []brief
	}{
		{"gzip against identity", &parity.Body{B64: &gz, ContentEncoding: "gzip"}, parity.TextBody(`{"id":"r1"}`),
			[]brief{{parity.KindValue, "", "$['@content_encoding']"}}},
		{"gzip both sides", &parity.Body{B64: &gz, ContentEncoding: "gzip"}, &parity.Body{B64: &gz, ContentEncoding: "GZIP"}, nil},
		{"body only on one side", parity.TextBody(`{"id":"r1"}`), nil, []brief{{parity.KindMissingInRust, "", "$"}}},
		{"non-JSON body", parity.TextBody("ok"), parity.TextBody("OK"), []brief{{parity.KindValue, "", "$"}}},
		{"json-only body is semantic", &parity.Body{JSON: json.RawMessage(`{"a":1}`)}, &parity.Body{JSON: json.RawMessage(`{"a":1.0}`)}, nil},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
			g.HTTPResponse = &parity.HTTPResponse{Status: 200, Body: tt.goB}
			r.HTTPResponse = &parity.HTTPResponse{Status: 200, Body: tt.rust}
			doc := compareTraces(t, e, Case{CaseID: "body"}, g, r)
			if got := briefs(doc.Mismatches); !equalBriefs(got, tt.want) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
		})
	}
}

// TestHTTPIn compares the request each server received.
func TestHTTPIn(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	g.HTTPIn = &parity.HTTPRequest{Method: "POST", Path: "/openrtb2/auction", Query: "a=1", Body: parity.TextBody(`{"id":"r1"}`)}
	r.HTTPIn = &parity.HTTPRequest{Method: "POST", Path: "/openrtb2/auction", Query: "a=2", Body: parity.TextBody(`{"id":"r1"}`)}
	doc := compareTraces(t, e, Case{CaseID: "in"}, g, r)
	if got := briefs(doc.Mismatches); !equalBriefs(got, []brief{{parity.KindValue, "", "$['@query']"}}) {
		t.Fatalf("got %v", got)
	}
	if doc.Mismatches[0].Domain != parity.DomainEndpoints || doc.Mismatches[0].Requirement != "ENDP-01" {
		t.Fatalf("attributed to %s/%s", doc.Mismatches[0].Domain, doc.Mismatches[0].Requirement)
	}
}

// TestStageStatus covers stage presence on each side.
func TestStageStatus(t *testing.T) {
	tests := []struct {
		name       string
		goStatus   parity.StageStatus // "" = captured with a value
		rustStatus parity.StageStatus
		want       []brief
		primary    bool
	}{
		{"both captured", "", "", nil, false},
		{"neither reached", parity.StageNotReached, parity.StageNotReached, nil, false},
		{"rust not reached", "", parity.StageNotReached, []brief{{parity.KindStageMissing, "", "$"}}, true},
		{"rust unimplemented", "", parity.StageUnimplemented, []brief{{parity.KindStageMissing, "", "$"}}, true},
		{"rust extra", parity.StageNotReached, "", []brief{{parity.KindStageExtra, "", "$"}}, true},
		{"go not reached, rust unimplemented", parity.StageNotReached, parity.StageUnimplemented, nil, false},
	}
	e := newTestEngine(t)
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
			for _, side := range []struct {
				t      *parity.Trace
				status parity.StageStatus
			}{{g, tt.goStatus}, {r, tt.rustStatus}} {
				if side.status == "" {
					put(side.t, parity.StageGDPRContext, parity.UnkeyedStage, `{"enforced":false}`)
				} else {
					side.t.StageStatus[parity.StageGDPRContext] = side.status
				}
			}
			doc := compareTraces(t, e, Case{CaseID: "status"}, g, r)
			got := briefs(doc.Mismatches)
			if !equalBriefs(got, tt.want) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
			if len(got) > 0 && doc.Mismatches[0].Primary != tt.primary {
				t.Fatalf("primary = %v", doc.Mismatches[0].Primary)
			}
		})
	}
}

// TestKeyedEntries: entries are matched by key; an entry on one side only is
// reported at the root of that key.
func TestKeyedEntries(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	put(g, parity.StagePrivacyBidder, "appnexus", `{"gdpr":false}`)
	put(g, parity.StagePrivacyBidder, "rubicon", `{"gdpr":false}`)
	put(r, parity.StagePrivacyBidder, "appnexus", `{"gdpr":false}`)
	put(r, parity.StagePrivacyBidder, "openx", `{"gdpr":false}`)
	doc := compareTraces(t, e, Case{CaseID: "keys"}, g, r)
	want := []string{
		"privacy.bidder/openx extra_in_rust: $",
		"privacy.bidder/rubicon missing_in_rust: $",
	}
	if got := ids(doc); !equalStrings(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

// TestEntryMembers: errors, warnings and writes of an entry are compared;
// seq is not (stage_entry_seq).
func TestEntryMembers(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	put(g, parity.StageRequestValidated, parity.UnkeyedStage, `{}`)
	put(r, parity.StageRequestValidated, parity.UnkeyedStage, `{}`)
	ge, re := g.Stages[parity.StageRequestValidated]["_"], r.Stages[parity.StageRequestValidated]["_"]
	ge.Warnings = []parity.ErrorRecord{{Code: 10002, Message: "w"}}
	re.Writes, re.Seq = 2, 9
	g.Stages[parity.StageRequestValidated]["_"], r.Stages[parity.StageRequestValidated]["_"] = ge, re
	got := briefs(compareTraces(t, e, Case{CaseID: "members"}, g, r).Mismatches)
	want := []brief{{parity.KindMissingInRust, "", "$['@warnings'][0]"}, {parity.KindValue, "", "$['@writes']"}}
	if !equalBriefs(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestCompareTracesRejects(t *testing.T) {
	e := newTestEngine(t)
	other := newTrace(parity.ImplRust)
	other.Schema = parity.SchemaEnvelope
	future := newTrace(parity.ImplRust)
	future.SchemaVersion = "2.0.0"
	tests := []struct {
		name string
		g, r *parity.Trace
	}{
		{"nil trace", newTrace(parity.ImplGo), nil},
		{"other document", newTrace(parity.ImplGo), other},
		{"unknown major version", newTrace(parity.ImplGo), future},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if _, err := e.CompareTraces(Case{}, tt.g, tt.r); err == nil {
				t.Fatal("no error")
			}
		})
	}
}

func TestCompareTraceJSON(t *testing.T) {
	e := newTestEngine(t)
	g, r := newTrace(parity.ImplGo), newTrace(parity.ImplRust)
	put(g, parity.StageRequestValidated, parity.UnkeyedStage, `{"tmax":500}`)
	put(r, parity.StageRequestValidated, parity.UnkeyedStage, `{"tmax":1000}`)
	gj, err := json.Marshal(g)
	if err != nil {
		t.Fatal(err)
	}
	rj, err := json.Marshal(r)
	if err != nil {
		t.Fatal(err)
	}
	res, err := e.CompareTraceJSON(Case{CaseID: "json"}, gj, rj)
	if err != nil {
		t.Fatal(err)
	}
	if got := ids(res.Doc); !equalStrings(got, []string{"request.validated/_ value: $.tmax"}) {
		t.Fatalf("got %v", got)
	}
	if len(res.Primary()) != 1 {
		t.Fatalf("primary = %v", res.Primary())
	}
	if _, err := e.CompareTraceJSON(Case{}, []byte("{"), rj); err == nil {
		t.Fatal("accepted a malformed trace")
	}
}

// TestEncodeDiff: diff.json keeps the recorded spellings readable.
func TestEncodeDiff(t *testing.T) {
	e := newTestEngine(t)
	recs := diffEntries(t, e, parity.StageRequestValidated, `{"a":"x&y"}`, esc(`{"a":"x^u0026y"}`))
	out, err := EncodeDiff(parity.DiffDocument{Schema: parity.SchemaDiff, SchemaVersion: parity.SchemaVersion, Mismatches: recs})
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(out, []byte(`"go": "x&y"`)) || !bytes.Contains(out, []byte(`"rust": "x\\u0026y"`)) {
		t.Fatalf("encoded:\n%s", out)
	}
	if !bytes.HasSuffix(out, []byte("}\n")) {
		t.Fatal("no trailing newline")
	}
}

// TestDeepDocument: a document nested beyond the parser's depth limit is
// compared as text rather than crashing the differ.
func TestDeepDocument(t *testing.T) {
	deep := strings.Repeat("[", maxDepth+1) + strings.Repeat("]", maxDepth+1)
	if _, err := parseExact([]byte(deep)); err == nil {
		t.Fatal("parsed a document deeper than maxDepth")
	}
	e := newTestEngine(t)
	if got := diffEntries(t, e, parity.StageRequestValidated, deep, deep); len(got) != 0 {
		t.Fatalf("got %v", briefs(got))
	}
}
