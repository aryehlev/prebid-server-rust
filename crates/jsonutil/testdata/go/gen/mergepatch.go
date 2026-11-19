package main

import (
	"bytes"
	"encoding/json"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	jsonpatch "gopkg.in/evanphx/json-patch.v5"
)

// The seeded corpus of MergePatch: documents and patches of objects, arrays, nulls, numbers and
// strings with the keys a, b and c, spelled as both writers spell them. A case whose patch is not
// an object, whose document is null, or whose patch holds an object with a null member inside an
// array meets one of evanphx's quirks (ledgered, one entry each): its outcome is the quirk's name
// on both sides, and the curated cases record what each side does there. Every other case must
// give the same bytes.
const (
	mergePatchSeed  = 0x4a53_4f4e_5054_4331 // "JSONPTC1"
	mergePatchCount = 50000
	mergePatchBatch = 1000
)

var mpKeys = []string{"a", "b", "c"}

func (s *splitmix64) mpValue(depth int) string {
	n := s.below(10)
	if depth >= 3 && n >= 6 {
		n = s.below(6)
	}
	switch n {
	case 0, 5:
		return "null"
	case 1:
		return "true"
	case 2:
		return strconv.Itoa(s.below(3))
	case 3:
		return `"` + s.pick(mpKeys) + `"`
	case 4:
		return "1.5"
	case 6, 7:
		return s.mpObject(depth + 1)
	default:
		k := s.below(3)
		parts := make([]string, 0, k)
		for i := 0; i < k; i++ {
			parts = append(parts, s.mpValue(depth+1))
		}
		return "[" + strings.Join(parts, ",") + "]"
	}
}

// mpObject is an object with distinct keys (evanphx and serde_json both keep the last of a
// repeated key; json.duplicate-keys covers the decoders that differ).
func (s *splitmix64) mpObject(depth int) string {
	k := s.below(4)
	used := map[string]bool{}
	parts := []string{}
	for i := 0; i < k; i++ {
		key := s.pick(mpKeys)
		if used[key] {
			continue
		}
		used[key] = true
		parts = append(parts, `"`+key+`":`+s.mpValue(depth))
	}
	return "{" + strings.Join(parts, ",") + "}"
}

func (s *splitmix64) mpTop() string {
	if s.below(10) == 0 {
		return s.mpValue(1)
	}
	return s.mpObject(1)
}

// nullInArrayObject reports whether v holds, inside an array, an object with a null member.
func nullInArrayObject(v any, inArray bool) bool {
	switch t := v.(type) {
	case map[string]any:
		for _, e := range t {
			if (e == nil && inArray) || nullInArrayObject(e, inArray) {
				return true
			}
		}
	case []any:
		for _, e := range t {
			if nullInArrayObject(e, true) {
				return true
			}
		}
	}
	return false
}

// mergePatchQuirk names the evanphx quirk a case meets, or "".
func mergePatchQuirk(doc, patch string) string {
	var d, p any
	if json.Unmarshal([]byte(doc), &d) != nil || json.Unmarshal([]byte(patch), &p) != nil {
		return ""
	}
	if _, ok := p.(map[string]any); !ok {
		return "non-object-patch"
	}
	if d == nil {
		return "null-document"
	}
	if nullInArrayObject(p, false) {
		return "array-nulls"
	}
	return ""
}

// mergePatchResult is MergePatch's output or error as one line.
func mergePatchResult(doc, patch string) string {
	out, err := jsonpatch.MergePatch([]byte(doc), []byte(patch))
	if err != nil {
		return "error " + err.Error()
	}
	return "ok " + string(out)
}

func mergePatchCorpus() seededCorpus {
	return seededCorpus{
		seed:  mergePatchSeed,
		count: mergePatchCount,
		batch: mergePatchBatch,
		next: func(s *splitmix64) (string, string) {
			doc := s.mpTop()
			patch := s.mpTop()
			if q := mergePatchQuirk(doc, patch); q != "" {
				return doc + "\t" + patch, "quirk " + q
			}
			return doc + "\t" + patch, mergePatchResult(doc, patch)
		},
	}
}

// mergePatchCase is one recorded merge: the inputs, the quirk they meet ("" for none) and
// MergePatch's output or error.
type mergePatchCase struct {
	Kind   string `json:"kind"`
	Source string `json:"source"`
	Doc    string `json:"doc"`
	Patch  string `json:"patch"`
	Quirk  string `json:"quirk"`
	Out    string `json:"out"`
	Err    string `json:"err"`
}

func newMergePatchCase(kind, source, doc, patch string) mergePatchCase {
	c := mergePatchCase{Kind: kind, Source: source, Doc: doc, Patch: patch, Quirk: mergePatchQuirk(doc, patch)}
	out, err := jsonpatch.MergePatch([]byte(doc), []byte(patch))
	if err != nil {
		c.Err = err.Error()
	} else {
		c.Out = string(out)
	}
	return c
}

type mergePatchDoc struct {
	Header  header           `json:"header"`
	Curated []mergePatchCase `json:"curated"`
	Corpus  []mergePatchCase `json:"corpus"`
	Seeded  seededDigests    `json:"seeded"`
}

// mergePatchCurated are RFC 7386's appendix A, each quirk's edges and the lexical edges (number
// spellings and escapes Go keeps as the inputs wrote them).
var mergePatchCurated = [][3]string{
	{"rfc7386", `{"a":"b"}`, `{"a":"c"}`},
	{"rfc7386", `{"a":"b"}`, `{"b":"c"}`},
	{"rfc7386", `{"a":"b"}`, `{"a":null}`},
	{"rfc7386", `{"a":"b","b":"c"}`, `{"a":null}`},
	{"rfc7386", `{"a":["b"]}`, `{"a":"c"}`},
	{"rfc7386", `{"a":"c"}`, `{"a":["b"]}`},
	{"rfc7386", `{"a":{"b":"c"}}`, `{"a":{"b":"d","c":null}}`},
	{"rfc7386", `{"a":[{"b":"c"}]}`, `{"a":[1]}`},
	{"rfc7386", `["a","b"]`, `["c","d"]`},
	{"rfc7386", `{"a":"b"}`, `["c"]`},
	{"rfc7386", `{"a":"foo"}`, `null`},
	{"rfc7386", `{"a":"foo"}`, `"bar"`},
	{"rfc7386", `{"e":null}`, `{"a":1}`},
	{"rfc7386", `[1,2]`, `{"a":"b","c":null}`},
	{"rfc7386", `{}`, `{"a":{"bb":{"ccc":null}}}`},
	{"array-nulls", `{}`, `{"a":[{"b":null}]}`},
	{"array-nulls", `{"a":[1]}`, `{"a":[{"b":null}]}`},
	{"array-nulls", `{"a":{"x":1}}`, `{"a":[{"b":null}]}`},
	{"array-nulls", `{"a":1}`, `{"a":{"b":null,"c":[{"d":null}]}}`},
	{"array-nulls", `{"a":1}`, `{"a":[null,{"b":null}]}`},
	{"null-document", `null`, `{"a":1}`},
	{"null-document", `null`, `{}`},
	{"null-document", `{"a":null}`, `{"a":{"b":null,"c":1}}`},
	{"non-object-patch", `{"a":1}`, `1`},
	{"non-object-patch", `{"a":1}`, `[null,{"b":null}]`},
	{"non-object-patch", `"x"`, `{"a":1}`},
	{"non-object-patch", `1`, `[1]`},
	{"non-object-patch", `[1]`, `"x"`},
	{"non-object-patch", `{"a":1}`, `true`},
	{"edge", `{"a":1}`, `{}`},
	{"edge", `{}`, `{}`},
	{"edge", ` {"a":1} `, ` {"b":2} `},
	{"edge", `{"a":1,"a":2}`, `{"b":1}`},
	{"edge", `{"a":1}`, `{"a":1,"a":null}`},
	{"edge", `{"a":1}`, `{"a":`},
	{"edge", `{"a":`, `{"b":1}`},
	{"spelling", `{"a":1.0}`, `{"b":1e2}`},
	{"spelling", `{"a":1.50,"b":-0}`, `{"c":100000000000000000000000}`},
	{"spelling", `{"a":"<"}`, `{"b":"&>"}`},
	{"spelling", "{\"a\":\"\\u00e9\"}", "{\"b\":\"\\/\"}"},
	{"spelling", "{\"a\":\"\\ud800\"}", "{\"b\":\"x\\ud800y\"}"},
	{"limits", `{"a":1e400}`, `{"b":1}`},
	{"limits", `{"a":1}`, `{"b":1e400}`},
	{"limits", `{"a":` + strings.Repeat("[", 200) + strings.Repeat("]", 200) + `}`, `{"b":1}`},
	{"limits", `{"a":1}`, `{"b":` + strings.Repeat("[", 200) + strings.Repeat("]", 200) + `}`},
}

// corpusDirs are the upstream test data the corpus merges are drawn from.
var corpusDirs = []string{
	"endpoints/openrtb2/sample-requests",
	"exchange/exchangetest",
	"exchange/eventscachetest",
	"firstpartydata/tests",
	"stored_requests/data/by_id",
	"stored_requests/backends/file_fetcher/test",
}

// corpusKinds maps the member names whose values the corpus collects to the merge they stand
// for: stored requests and imps (endpoints/openrtb2/auction.go:1711-1755), first-party data and
// ext objects (firstpartydata/first_party_data.go:255-384, util/jsonutil/merge.go:165), accounts
// (stored_requests/backends/*/fetcher.go) and bids, which events patch with a win URL
// (exchange/events.go:98).
var corpusKinds = map[string]string{
	"mockBidRequest": "stored-request",
	"imp":            "stored-imp",
	"site":           "fpd",
	"app":            "fpd",
	"user":           "fpd",
	"device":         "fpd",
	"ext":            "ext",
	"account":        "account",
	"bid":            "event",
}

// collect walks one JSON value, appending each object found under a member of corpusKinds (and
// each element of an array found there) to found, compacted with its spelling kept.
func collect(raw json.RawMessage, found map[string][]string) {
	var obj map[string]json.RawMessage
	if json.Unmarshal(raw, &obj) == nil {
		keys := make([]string, 0, len(obj))
		for k := range obj {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			v := obj[k]
			if kind, ok := corpusKinds[k]; ok {
				addCorpusValue(kind, v, found)
			}
			collect(v, found)
		}
		return
	}
	var arr []json.RawMessage
	if json.Unmarshal(raw, &arr) == nil {
		for _, v := range arr {
			collect(v, found)
		}
	}
}

func addCorpusValue(kind string, v json.RawMessage, found map[string][]string) {
	var arr []json.RawMessage
	if json.Unmarshal(v, &arr) == nil {
		for _, e := range arr {
			addCorpusValue(kind, e, found)
		}
		return
	}
	var obj map[string]json.RawMessage
	if json.Unmarshal(v, &obj) != nil {
		return
	}
	var buf bytes.Buffer
	if json.Compact(&buf, v) != nil {
		return
	}
	found[kind] = append(found[kind], buf.String())
}

// upstreamDir is the upstream checkout the module builds against.
func upstreamDir() string {
	return filepath.Join(moduleDir(), "..", "..", "..", "..", ".upstream")
}

// corpusLimit caps the pairs of each kind.
const corpusLimit = 300

// mergePatchCorpusCases pairs consecutive collected objects of each kind, the later merged into
// the earlier; account objects also merge into the first account (the defaults' stand-in), and
// each bid gets the win URL patch events.go builds.
func mergePatchCorpusCases() []mergePatchCase {
	root := upstreamDir()
	found := map[string][]string{}
	var files []string
	for _, dir := range corpusDirs {
		_ = filepath.WalkDir(filepath.Join(root, dir), func(path string, d fs.DirEntry, err error) error {
			if err == nil && !d.IsDir() && strings.HasSuffix(path, ".json") {
				files = append(files, path)
			}
			return nil
		})
	}
	sort.Strings(files)
	for _, path := range files {
		data, err := os.ReadFile(path)
		if err != nil || !json.Valid(data) {
			continue
		}
		rel, _ := filepath.Rel(root, path)
		if strings.Contains(rel, "accounts") {
			addCorpusValue("account", data, found)
		}
		collect(data, found)
	}
	var cases []mergePatchCase
	kinds := make([]string, 0, len(found))
	for k := range found {
		kinds = append(kinds, k)
	}
	sort.Strings(kinds)
	for _, kind := range kinds {
		values := found[kind]
		pairs := 0
		for i := 0; i+1 < len(values) && pairs < corpusLimit; i++ {
			if kind == "event" {
				patch := "{\"wurl\":\"http://localhost:8000/event?t=win\\u0026b=bid" + strconv.Itoa(i) + "\\u0026a=account\\u0026bidder=appnexus\"}"
				cases = append(cases, newMergePatchCase(kind, "win url", values[i], patch))
			} else {
				cases = append(cases, newMergePatchCase(kind, "consecutive", values[i], values[i+1]))
			}
			pairs++
		}
		if kind == "account" {
			for i := 1; i < len(values) && i <= corpusLimit; i++ {
				cases = append(cases, newMergePatchCase(kind, "defaults", values[0], values[i]))
			}
		}
	}
	return cases
}

func mergePatchGolden() mergePatchDoc {
	doc := mergePatchDoc{
		Header: newHeader("gopkg.in/evanphx/json-patch.v5"),
		Corpus: mergePatchCorpusCases(),
		Seeded: mergePatchCorpus().digests(),
	}
	for _, c := range mergePatchCurated {
		doc.Curated = append(doc.Curated, newMergePatchCase(c[0], "curated", c[1], c[2]))
	}
	return doc
}
