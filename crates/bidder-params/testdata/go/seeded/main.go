// Command seeded writes the seeded and curated goldens of the Rust pbs-bidder-params crate from
// github.com/xeipuuv/gojsonschema itself, over every schema Prebid Server loads with it: the
// files of static/bidder-params and modules/prebid/rulesengine/config/rules-engine-schema.json,
// each loaded as Prebid loads it (gojsonschema.NewReferenceLoader on the absolute path).
//
// Run it from the directory of this module (crates/bidder-params/testdata/go):
//
//	go run ./seeded ../golden
//
// It reads the upstream checkout the module builds against (see enterUpstream) and writes:
//
//   - seeded.jsonl: for every schema, 50,000 params documents built by a SplitMix64 generator
//     (seeded with the FNV-64a hash of the schema's path) from the property names and values the
//     line lists, and the SHA-256 digests of the documents and of their outcomes. An outcome is
//     the verdict and, for an invalid document, the error strings in sorted order: Go walks maps
//     in random order, so only the multiset of errors is fixed.
//   - texts.jsonl: documents whose objects hold at most one member, for which Go's error order is
//     fixed, with Go's joined error text (checked over 8 copies of each schema, 4 runs each).
//   - formats.jsonl: the uuid and hostname format checkers over strings and other values, and
//     the formats no Prebid schema uses, which Go loads.
//   - huge_numbers.jsonl: numbers beyond float64's precision and range, in documents and in
//     schemas.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"log"
	"math/big"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"

	"github.com/xeipuuv/gojsonschema"
)

const (
	// schemaDir is the bidder params schema directory, relative to the upstream checkout.
	schemaDir = "static/bidder-params"
	// rulesSchema is the rules engine module's config schema, relative to the upstream checkout.
	rulesSchema = "modules/prebid/rulesengine/config/rules-engine-schema.json"
	// seededCount is the number of seeded documents per schema.
	seededCount = 50000
	// textCopies and textRepeats are the schema copies and runs per copy behind texts.jsonl.
	textCopies  = 8
	textRepeats = 4
	// maxMembers is the most members a generated object has.
	maxMembers = 4
	// maxDepth is the deepest a generated object or array nests.
	maxDepth = 2
)

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: seeded <output directory>")
	}
	out := enterUpstream(os.Args[1])
	if err := os.MkdirAll(out, 0o755); err != nil {
		log.Fatal(err)
	}
	paths := schemaPaths()
	writeLines(filepath.Join(out, "seeded.jsonl"), seededLines(paths))
	writeLines(filepath.Join(out, "texts.jsonl"), textLines(paths))
	writeLines(filepath.Join(out, "formats.jsonl"), formatLines())
	writeLines(filepath.Join(out, "huge_numbers.jsonl"), hugeNumberLines())
}

// schemaPaths is every schema file Prebid loads with gojsonschema, in name order.
func schemaPaths() []string {
	entries, err := os.ReadDir(schemaDir)
	if err != nil {
		log.Fatal(err)
	}
	var paths []string
	for _, entry := range entries {
		paths = append(paths, schemaDir+"/"+entry.Name())
	}
	return append(paths, rulesSchema)
}

// loadSchema loads path as Prebid does.
func loadSchema(path string) *gojsonschema.Schema {
	abs, err := filepath.Abs(path)
	if err != nil {
		log.Fatal(err)
	}
	schema, err := gojsonschema.NewSchema(gojsonschema.NewReferenceLoader("file:///" + filepath.ToSlash(abs)))
	if err != nil {
		log.Fatalf("%s: %v", path, err)
	}
	return schema
}

// readJSON decodes path with numbers kept as json.Number.
func readJSON(path string) interface{} {
	f, err := os.Open(path)
	if err != nil {
		log.Fatal(err)
	}
	defer f.Close()
	dec := json.NewDecoder(f)
	dec.UseNumber()
	var v interface{}
	if err := dec.Decode(&v); err != nil {
		log.Fatalf("%s: %v", path, err)
	}
	return v
}

// outcome is the result of one validation.
type outcome struct {
	kind string   // ok, invalid, decode or panic
	errs []string // the error strings, in Go's order
	text string   // the decode error or the panic value
}

func validate(schema *gojsonschema.Schema, doc []byte) (o outcome) {
	defer func() {
		if r := recover(); r != nil {
			o = outcome{kind: "panic", text: fmt.Sprint(r)}
		}
	}()
	res, err := schema.Validate(gojsonschema.NewBytesLoader(doc))
	if err != nil {
		return outcome{kind: "decode", text: err.Error()}
	}
	if res.Valid() {
		return outcome{kind: "ok"}
	}
	o.kind = "invalid"
	for _, e := range res.Errors() {
		o.errs = append(o.errs, e.String())
	}
	return o
}

// joined is what Prebid writes for an outcome: the decode error, the panic value, or the error
// strings joined without a separator.
func (o outcome) joined() string {
	return o.text + strings.Join(o.errs, "")
}

// splitmix64 is the generator both sides run.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *splitmix64) below(n int) int {
	return int(s.next() % uint64(n))
}

// generator builds params documents from JSON-encoded names and values.
type generator struct {
	rng    splitmix64
	names  []string
	values []string
}

func (g *generator) pick(list []string) string {
	return list[g.rng.below(len(list))]
}

func (g *generator) document() string {
	if g.rng.below(16) == 0 {
		return g.pick(g.values)
	}
	return g.object(0)
}

func (g *generator) object(depth int) string {
	n := g.rng.below(maxMembers + 1)
	members := make([]string, 0, n)
	for i := 0; i < n; i++ {
		key := g.pick(g.names)
		value := g.value(depth + 1)
		members = append(members, key+":"+value)
	}
	return "{" + strings.Join(members, ",") + "}"
}

func (g *generator) value(depth int) string {
	r := g.rng.below(10)
	if r == 0 && depth < maxDepth {
		return g.object(depth)
	}
	if r == 1 && depth < maxDepth {
		n := g.rng.below(3)
		items := make([]string, 0, n)
		for i := 0; i < n; i++ {
			items = append(items, g.value(depth+1))
		}
		return "[" + strings.Join(items, ",") + "]"
	}
	return g.pick(g.values)
}

// genericValues are the values every schema's documents draw from, beside the schema's own.
var genericValues = []string{
	`null`, `true`, `false`, `0`, `1`, `-1`, `2`, `17`, `100`, `101`, `9999`, `10000`, `10001`,
	`12345`, `0.5`, `1.5`, `-0.5`, `1e2`, `1.0`, `-0`, `2.5e-3`,
	`""`, `"a"`, `"ab"`, `"abc"`, `"1"`, `"123"`, `"12345"`, `"abc123"`, `"a_b"`, `"a-b"`, `" "`,
	`"x y"`, `"US"`, `"USD"`, `"example.com"`, `"a..b"`, `"-x.com"`, `"https://example.com/p?q=1"`,
	`"123e4567-e89b-12d3-a456-426614174000"`, `"123E4567-E89B-12D3-A456-426614174000"`,
	`"1000008-AA77BB88CC"`, `"été"`, `"日本"`, `"\ud800"`, `"<a&b>"`, `" "`, `"\t"`,
	`[]`, `[1]`, `["a"]`, `[""]`, `[1,"a"]`, `[[1]]`, `{}`, `{"a":1}`, `{"x":"y"}`,
}

// compact is v as encoding/json writes it.
func compact(v interface{}) string {
	data, err := json.Marshal(v)
	if err != nil {
		log.Fatal(err)
	}
	return string(data)
}

func intOf(v interface{}) (int64, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return 0, false
	}
	i, err := n.Int64()
	return i, err == nil
}

func ratOf(v interface{}) (*big.Rat, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return nil, false
	}
	return new(big.Rat).SetString(string(n))
}

func addStrings(set map[string]bool, v interface{}) {
	items, _ := v.([]interface{})
	for _, item := range items {
		if s, ok := item.(string); ok {
			set[s] = true
		}
	}
}

// tokens is the sorted property names (JSON-encoded) and values (JSON texts) a schema's seeded
// documents are made of: every name its properties, required lists and dependencies mention,
// its enum values, strings around its length limits and numbers around its numeric limits, and
// the generic values.
func tokens(doc interface{}) (names, values []string) {
	nameSet := map[string]bool{"x": true, "unknown": true}
	valueSet := map[string]bool{}
	for _, v := range genericValues {
		valueSet[v] = true
	}
	var walk func(v interface{})
	walk = func(v interface{}) {
		switch t := v.(type) {
		case map[string]interface{}:
			for k, child := range t {
				switch k {
				case "properties":
					if m, ok := child.(map[string]interface{}); ok {
						for name, sub := range m {
							nameSet[name] = true
							walk(sub)
						}
						continue
					}
				case "required":
					addStrings(nameSet, child)
				case "dependencies":
					if m, ok := child.(map[string]interface{}); ok {
						for name, dep := range m {
							nameSet[name] = true
							addStrings(nameSet, dep)
							walk(dep)
						}
						continue
					}
				case "enum":
					items, _ := child.([]interface{})
					for _, item := range items {
						valueSet[compact(item)] = true
					}
					continue
				case "minLength", "maxLength":
					if n, ok := intOf(child); ok && n <= 64 {
						for _, l := range []int64{n - 1, n, n + 1} {
							if l >= 0 {
								valueSet[compact(strings.Repeat("a", int(l)))] = true
							}
						}
					}
				case "minimum", "maximum":
					if r, ok := ratOf(child); ok && r.IsInt() {
						for _, d := range []int64{-1, 0, 1} {
							valueSet[new(big.Rat).Add(r, big.NewRat(d, 1)).FloatString(0)] = true
						}
						valueSet[new(big.Rat).Add(r, big.NewRat(1, 2)).FloatString(1)] = true
					}
				}
				walk(child)
			}
		case []interface{}:
			for _, item := range t {
				walk(item)
			}
		}
	}
	walk(doc)
	for name := range nameSet {
		names = append(names, compact(name))
	}
	for value := range valueSet {
		values = append(values, value)
	}
	sort.Strings(names)
	sort.Strings(values)
	return names, values
}

// seededLine is one line of seeded.jsonl.
type seededLine struct {
	File     string   `json:"file"`
	Seed     uint64   `json:"seed"`
	Count    int      `json:"count"`
	Inputs   string   `json:"inputs"`
	Outcomes string   `json:"outcomes"`
	Names    []string `json:"names"`
	Values   []string `json:"values"`
}

// outcomeLine is a document and its outcome as both sides hash them: the document, the verdict
// and, for an invalid document, the error strings in sorted order, separated by tabs.
func outcomeLine(doc string, o outcome) string {
	errs := append([]string(nil), o.errs...)
	sort.Strings(errs)
	return strings.Join(append([]string{doc, o.kind}, errs...), "\t") + "\n"
}

func runSeeded(path string) seededLine {
	schema := loadSchema(path)
	names, values := tokens(readJSON(path))
	h := fnv.New64a()
	h.Write([]byte(path))
	line := seededLine{File: path, Seed: h.Sum64(), Count: seededCount, Names: names, Values: values}
	g := generator{rng: splitmix64{state: line.Seed}, names: names, values: values}
	inputs, outcomes := sha256.New(), sha256.New()
	for i := 0; i < seededCount; i++ {
		doc := g.document()
		o := validate(schema, []byte(doc))
		if o.kind != "ok" && o.kind != "invalid" {
			log.Fatalf("%s: %s gives %s %s", path, doc, o.kind, o.text)
		}
		inputs.Write([]byte(doc + "\n"))
		outcomes.Write([]byte(outcomeLine(doc, o)))
	}
	line.Inputs = hex.EncodeToString(inputs.Sum(nil))
	line.Outcomes = hex.EncodeToString(outcomes.Sum(nil))
	return line
}

func seededLines(paths []string) []interface{} {
	lines := make([]interface{}, len(paths))
	work := make(chan int)
	var wg sync.WaitGroup
	for w := 0; w < runtime.NumCPU(); w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range work {
				lines[i] = runSeeded(paths[i])
			}
		}()
	}
	for i := range paths {
		work <- i
	}
	close(work)
	wg.Wait()
	return lines
}

// textValues are the member values of texts.jsonl's documents.
var textValues = []string{
	`null`, `true`, `0`, `-1`, `1.5`, `10001`, `""`, `"a"`, `"123"`, `"x y"`, `[]`, `[1]`, `{}`,
	`{"x":1}`,
}

// textLine is one line of texts.jsonl.
type textLine struct {
	File string `json:"file"`
	Doc  string `json:"doc"`
	Kind string `json:"kind"`
	Text string `json:"text"`
}

// rootNames is the sorted names of the root properties of a schema document, and "x".
func rootNames(doc interface{}) []string {
	names := []string{"x"}
	root, _ := doc.(map[string]interface{})
	props, _ := root["properties"].(map[string]interface{})
	for name := range props {
		if name != "x" {
			names = append(names, name)
		}
	}
	sort.Strings(names)
	return names
}

func textCases(path string) []interface{} {
	copies := make([]*gojsonschema.Schema, textCopies)
	for i := range copies {
		copies[i] = loadSchema(path)
	}
	docs := []string{`{}`, `null`, `[]`, `"x"`, `1`, `1.5`, `true`}
	for _, name := range rootNames(readJSON(path)) {
		for _, v := range textValues {
			docs = append(docs, "{"+compact(name)+":"+v+"}")
		}
	}
	var lines []interface{}
	for _, doc := range docs {
		first := validate(copies[0], []byte(doc))
		for _, schema := range copies {
			for r := 0; r < textRepeats; r++ {
				o := validate(schema, []byte(doc))
				if o.kind != first.kind || o.joined() != first.joined() {
					log.Fatalf("%s: Go's output for %s is not fixed", path, doc)
				}
			}
		}
		lines = append(lines, textLine{File: path, Doc: doc, Kind: first.kind, Text: first.joined()})
	}
	return lines
}

func textLines(paths []string) []interface{} {
	var lines []interface{}
	for _, path := range paths {
		lines = append(lines, textCases(path)...)
	}
	return lines
}

// caseLine is one line of formats.jsonl and huge_numbers.jsonl: a schema that loads ("loaded",
// with no document), fails to load ("load_error") or validates the document.
type caseLine struct {
	Schema string `json:"schema"`
	Doc    string `json:"doc,omitempty"`
	Kind   string `json:"kind"`
	Text   string `json:"text,omitempty"`
}

// cases loads every schema and validates every document against each one that loads.
func cases(schemas, docs []string) []interface{} {
	var lines []interface{}
	for _, s := range schemas {
		schema, err := gojsonschema.NewSchema(gojsonschema.NewStringLoader(s))
		if err != nil {
			lines = append(lines, caseLine{Schema: s, Kind: "load_error", Text: err.Error()})
			continue
		}
		lines = append(lines, caseLine{Schema: s, Kind: "loaded"})
		for _, doc := range docs {
			o := validate(schema, []byte(doc))
			lines = append(lines, caseLine{Schema: s, Doc: doc, Kind: o.kind, Text: o.joined()})
		}
	}
	return lines
}

// hostOfLength is a valid host name of n bytes (n > 204): four 50-byte labels and a last one.
func hostOfLength(n int) string {
	label := strings.Repeat("a", 50) + "."
	return strings.Repeat(label, 4) + strings.Repeat("b", n-204)
}

func formatLines() []interface{} {
	strs := []string{
		"", "123e4567-e89b-12d3-a456-426614174000", "00000000-0000-0000-0000-000000000000",
		"ffffffff-ffff-ffff-ffff-ffffffffffff", "123E4567-E89B-12D3-A456-426614174000",
		"123e4567-e89b-12d3-a456-42661417400", "123e4567-e89b-12d3-a456-4266141740000",
		"123e4567e89b12d3a456426614174000", "{123e4567-e89b-12d3-a456-426614174000}",
		"urn:uuid:123e4567-e89b-12d3-a456-426614174000", "123e4567-e89b-12d3-a456-426614174000\n",
		" 123e4567-e89b-12d3-a456-426614174000", "g23e4567-e89b-12d3-a456-426614174000",
		"123e4567-e89b-12d3-a456_426614174000", "٣" + "23e4567-e89b-12d3-a456-426614174000",
		"example.com", "localhost", "a", "a.b", "a.b.c", "-a.com", "a-.com", "a-b.com", "a--b.com",
		"a_b.com", "a..b", ".a", "a.", "xn--abc.com", "1.2.3.4", "123", "ex ample.com", "é.com",
		"EXAMPLE.COM", "a.-b.com", "a\n", "a.b\n",
		strings.Repeat("a", 63) + ".com", strings.Repeat("a", 64) + ".com",
		hostOfLength(253), hostOfLength(254), hostOfLength(255), hostOfLength(256), hostOfLength(257),
	}
	var docs []string
	for _, s := range strs {
		docs = append(docs, compact(s))
	}
	docs = append(docs, `1`, `1.5`, `true`, `null`, `[]`, `{}`, `["example.com"]`)
	var schemas []string
	for _, format := range []string{"uuid", "hostname"} {
		schemas = append(schemas,
			`{"$schema":"http://json-schema.org/draft-04/schema#","format":"`+format+`"}`,
			`{"format":"`+format+`"}`,
			`{"type":"string","format":"`+format+`"}`,
			`{"type":["string","integer"],"format":"`+format+`"}`,
		)
	}
	lines := cases(schemas, docs)
	var unused []string
	for _, format := range []string{
		"date", "time", "date-time", "email", "idn-email", "hostname ", "idn-hostname", "ipv4",
		"ipv6", "uri", "uri-reference", "iri", "iri-reference", "uri-template", "regex",
		"json-pointer", "relative-json-pointer", "custom", "UUID", "",
	} {
		unused = append(unused, `{"format":`+compact(format)+`}`)
	}
	return append(lines, cases(unused, nil)...)
}

func hugeNumberLines() []interface{} {
	numbers := []string{
		`9007199254740992`, `9007199254740993`, `-9007199254740993`, `18446744073709551615`,
		`18446744073709551616`, `123456789012345678901234567890`, `1.0000000000000001`,
		`0.99999999999999999`, `1e308`, `1.7976931348623157e308`, `1.5e300`, `1e400`, `-1e400`,
		`1e-400`, `0.1e-1000005`, `1e999999999`, `-0`, `-0.0`,
	}
	var docs []string
	for _, n := range numbers {
		docs = append(docs, n, `{"n":`+n+`}`)
	}
	schemas := []string{
		`{"type":"integer"}`,
		`{"type":"number","minimum":1}`,
		`{"type":"number","maximum":9007199254740992}`,
		`{"type":"integer","maximum":100}`,
		`{"enum":["a"]}`,
		`{"type":"string"}`,
		`{"properties":{"n":{"type":"integer","minimum":0}}}`,
	}
	lines := cases(schemas, docs)
	limits := []string{
		`{"minimum":1e400}`,
		`{"maximum":9007199254740993}`,
		`{"minimum":0.30000000000000001}`,
		`{"minLength":-0}`,
		`{"minLength":1.0}`,
		`{"minLength":18446744073709551616}`,
		`{"maxItems":9223372036854775807}`,
	}
	return append(lines, cases(limits, []string{`0`, `1`, `9007199254740993`, `"abc"`, `[]`})...)
}

func writeLines(path string, lines []interface{}) {
	var b strings.Builder
	for _, line := range lines {
		data, err := json.Marshal(line)
		if err != nil {
			log.Fatal(err)
		}
		b.Write(data)
		b.WriteByte('\n')
	}
	if err := os.WriteFile(path, []byte(b.String()), 0o644); err != nil {
		log.Fatal(err)
	}
}
