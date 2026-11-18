// Command golden writes the differential goldens of the Rust pbs-bidder-params crate from the
// real Go code: openrtb_ext.NewBidderParamsValidator over static/bidder-params, and
// github.com/xeipuuv/gojsonschema itself for the engine cases.
//
// Run it from the directory of this module (crates/bidder-params/testdata/go):
//
//	go run ./golden ../golden
//
// It reads static/bidder-params and static/bidder-info from the upstream checkout the module
// builds against (see enterUpstream).
//
// Go iterates maps in random order, and gojsonschema walks maps (the properties of a schema,
// fixed when the schema is loaded, and the keys of every validated document), so a value with
// several errors can produce them in different orders. Every case is therefore validated by
// several independently loaded copies of the schema, several times each, and every distinct
// output is recorded.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"log"
	"math/big"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/xeipuuv/gojsonschema"
)

const (
	// copies is the number of independently loaded copies of every schema.
	copies = 12
	// repeats is the number of validations per copy.
	repeats = 3
	// schemaDir is the bidder params schema directory, relative to the upstream checkout.
	schemaDir = "static/bidder-params"
)

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: golden <output directory>")
	}

	out := enterUpstream(os.Args[1])
	if err := os.MkdirAll(out, 0o755); err != nil {
		log.Fatal(err)
	}
	// Registers the aliases declared in static/bidder-info, like the server does at startup.
	if _, err := config.LoadBidderInfoFromDisk("static/bidder-info"); err != nil {
		log.Fatal(err)
	}
	validator, err := openrtb_ext.NewBidderParamsValidator(schemaDir)
	if err != nil {
		log.Fatal(err)
	}
	schemas := loadSchemas()
	writeValidator(filepath.Join(out, "validator.json"), validator, schemas)
	writeBidderCases(filepath.Join(out, "bidder_cases.jsonl"), validator, schemas)
	writeEngineCases(filepath.Join(out, "engine_cases.jsonl"))
}

// loadedSchema is one static/bidder-params file.
type loadedSchema struct {
	file     string
	contents string
	copies   []*gojsonschema.Schema
	doc      *node
}

func loadSchemas() map[string]*loadedSchema {
	entries, err := os.ReadDir(schemaDir)
	if err != nil {
		log.Fatal(err)
	}
	schemas := map[string]*loadedSchema{}
	for _, entry := range entries {
		path := filepath.Join(schemaDir, entry.Name())
		abs, err := filepath.Abs(path)
		if err != nil {
			log.Fatal(err)
		}
		contents, err := os.ReadFile(path)
		if err != nil {
			log.Fatal(err)
		}
		doc, err := parseNode(contents)
		if err != nil {
			log.Fatalf("%s: %v", path, err)
		}
		loaded := &loadedSchema{file: entry.Name(), contents: string(contents), doc: doc}
		for i := 0; i < copies; i++ {
			schema, err := gojsonschema.NewSchema(gojsonschema.NewReferenceLoader("file:///" + filepath.ToSlash(abs)))
			if err != nil {
				log.Fatalf("%s: %v", path, err)
			}
			loaded.copies = append(loaded.copies, schema)
		}
		schemas[strings.TrimSuffix(entry.Name(), ".json")] = loaded
	}
	return schemas
}

// validatorGolden is what the Rust validator is built from and checked against.
type validatorGolden struct {
	// Bidders is BuildBidderMap(): core bidders plus the aliases from static/bidder-info.
	Bidders []string `json:"bidders"`
	// Aliases is GetAliasBidderToParent().
	Aliases map[string]string `json:"aliases"`
	// Schemas maps every bidder to the file whose contents Schema(bidder) returns ("" when it
	// returns "").
	Schemas map[string]string `json:"schemas"`
}

func writeValidator(path string, validator openrtb_ext.BidderParamValidator, schemas map[string]*loadedSchema) {
	golden := validatorGolden{Aliases: map[string]string{}, Schemas: map[string]string{}}
	for name := range openrtb_ext.BuildBidderMap() {
		golden.Bidders = append(golden.Bidders, name)
	}
	sort.Strings(golden.Bidders)
	for alias, parent := range openrtb_ext.GetAliasBidderToParent() {
		golden.Aliases[string(alias)] = string(parent)
	}
	names := append([]string{"notabidder"}, golden.Bidders...)
	for _, name := range names {
		contents := validator.Schema(openrtb_ext.BidderName(name))
		file := ""
		if contents != "" {
			for _, loaded := range schemas {
				if loaded.contents == contents && (file == "" || loaded.file < file) {
					file = loaded.file
				}
			}
			if file == "" {
				log.Fatalf("Schema(%s) is not the contents of any schema file", name)
			}
		}
		golden.Schemas[name] = file
	}
	data, err := json.MarshalIndent(golden, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}

// outcome is the result of one validation.
type outcome struct {
	kind   string // "ok", "invalid", "decode" or "panic"
	text   string // the joined error strings, or the decode error
	errors []gojsonschema.ResultError
}

func validateOnce(schema *gojsonschema.Schema, doc []byte) (result outcome) {
	defer func() {
		if r := recover(); r != nil {
			result = outcome{kind: "panic", text: fmt.Sprint(r)}
		}
	}()
	res, err := schema.Validate(gojsonschema.NewBytesLoader(doc))
	if err != nil {
		return outcome{kind: "decode", text: err.Error()}
	}
	if res.Valid() {
		return outcome{kind: "ok"}
	}
	var b strings.Builder
	for _, e := range res.Errors() {
		b.WriteString(e.String())
	}
	return outcome{kind: "invalid", text: b.String(), errors: res.Errors()}
}

// sampled is every distinct outcome of a case over all schema copies.
type sampled struct {
	kind    string
	outputs []string
	first   outcome
}

func sample(schemas []*gojsonschema.Schema, doc []byte) sampled {
	seen := map[string]bool{}
	var result sampled
	for _, schema := range schemas {
		for r := 0; r < repeats; r++ {
			o := validateOnce(schema, doc)
			if result.kind == "" {
				result.kind = o.kind
				result.first = o
			} else if result.kind != o.kind {
				log.Fatalf("inconsistent outcome for %q: %s and %s", doc, result.kind, o.kind)
			}
			if o.kind == "invalid" || o.kind == "decode" {
				seen[o.text] = true
			}
		}
	}
	for text := range seen {
		result.outputs = append(result.outputs, text)
	}
	sort.Strings(result.outputs)
	return result
}

// bidderCase is one line of bidder_cases.jsonl.
type bidderCase struct {
	Bidder string `json:"bidder"`
	// Source says where the params come from: gen (generated from the schema), params_test
	// (adapters/*/params_test.go), fixture (adapter JSON tests), sample (endpoint sample
	// requests), alias, or edge.
	Source string  `json:"src"`
	Ext    *string `json:"ext,omitempty"`
	ExtHex string  `json:"ext_hex,omitempty"`
	// Kind is ok, invalid, decode or panic.
	Kind string `json:"kind"`
	// Outputs are the distinct Validate error texts (invalid and decode).
	Outputs []string `json:"outputs,omitempty"`
	// Errors are the individual ResultError strings, sorted, when Outputs has several entries.
	Errors []string `json:"errors,omitempty"`
}

type rawCase struct {
	bidder string
	source string
	ext    []byte
}

func writeBidderCases(path string, validator openrtb_ext.BidderParamValidator, schemas map[string]*loadedSchema) {
	aliases := openrtb_ext.GetAliasBidderToParent()
	schemaFor := func(bidder string) *loadedSchema {
		if parent, ok := aliases[openrtb_ext.BidderName(bidder)]; ok {
			return schemas[string(parent)]
		}
		return schemas[bidder]
	}
	var cases []rawCase
	var bidders []string
	for bidder := range schemas {
		bidders = append(bidders, bidder)
	}
	sort.Strings(bidders)
	for _, bidder := range bidders {
		for _, c := range generateCases(schemas[bidder].doc) {
			cases = append(cases, rawCase{bidder: bidder, source: "gen " + c.label, ext: c.doc})
		}
	}
	var aliasNames []string
	for alias := range aliases {
		aliasNames = append(aliasNames, string(alias))
	}
	sort.Strings(aliasNames)
	for _, alias := range aliasNames {
		parent := schemaFor(alias)
		if parent == nil {
			cases = append(cases, rawCase{bidder: alias, source: "alias", ext: []byte("{}")})
			continue
		}
		generated := generateCases(parent.doc)
		for i, c := range generated {
			if i < 4 {
				cases = append(cases, rawCase{bidder: alias, source: "alias " + c.label, ext: c.doc})
			}
		}
	}
	known := map[string]bool{}
	for name := range openrtb_ext.BuildBidderMap() {
		known[name] = schemaFor(name) != nil
	}
	cases = append(cases, paramsTestCases()...)
	cases = append(cases, fixtureCases(known)...)
	cases = append(cases, sampleRequestCases(known)...)
	for _, doc := range []string{"{}", "{", "", "null", "malformedJSON"} {
		cases = append(cases, rawCase{bidder: "notabidder", source: "edge unknown bidder", ext: []byte(doc)})
	}

	file, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(file)
	seen := map[string]bool{}
	for _, c := range cases {
		key := c.bidder + "\x00" + string(c.ext)
		if seen[key] {
			continue
		}
		seen[key] = true
		line := runBidderCase(validator, schemaFor(c.bidder), c)
		data, err := marshalLine(line)
		if err != nil {
			log.Fatal(err)
		}
		if _, err := w.Write(data); err != nil {
			log.Fatal(err)
		}
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := file.Close(); err != nil {
		log.Fatal(err)
	}
}

func runBidderCase(validator openrtb_ext.BidderParamValidator, loaded *loadedSchema, c rawCase) bidderCase {
	line := bidderCase{Bidder: c.bidder, Source: c.source}
	if utf8.Valid(c.ext) {
		text := string(c.ext)
		line.Ext = &text
	} else {
		line.ExtHex = hex.EncodeToString(c.ext)
	}
	var s sampled
	if loaded != nil {
		s = sample(loaded.copies, c.ext)
	}
	// The real validator must agree, and its text is one more sample.
	kind, text := validateWithValidator(validator, c.bidder, c.ext)
	if s.kind == "" {
		s.kind = kind
		if kind == "invalid" || kind == "decode" {
			s.outputs = []string{text}
		}
	}
	if kind != s.kind {
		log.Fatalf("%s %q: the validator says %s, the schema copies %s", c.bidder, c.ext, kind, s.kind)
	}
	if kind == "invalid" || kind == "decode" {
		found := false
		for _, o := range s.outputs {
			found = found || o == text
		}
		if !found {
			s.outputs = append(s.outputs, text)
			sort.Strings(s.outputs)
		}
	}
	line.Kind = s.kind
	line.Outputs = s.outputs
	if len(s.outputs) > 1 && s.kind == "invalid" {
		for _, e := range s.first.errors {
			line.Errors = append(line.Errors, e.String())
		}
		sort.Strings(line.Errors)
	}
	return line
}

func validateWithValidator(validator openrtb_ext.BidderParamValidator, bidder string, ext []byte) (kind, text string) {
	defer func() {
		if r := recover(); r != nil {
			kind, text = "panic", fmt.Sprint(r)
		}
	}()
	err := validator.Validate(openrtb_ext.BidderName(bidder), json.RawMessage(ext))
	if err == nil {
		return "ok", ""
	}
	if _, loadErr := gojsonschema.NewBytesLoader(ext).LoadJSON(); loadErr != nil {
		return "decode", err.Error()
	}
	return "invalid", err.Error()
}

func marshalLine(v interface{}) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// paramsTestCases extracts every string literal of the []string literals in
// adapters/*/params_test.go (validParams, invalidParams, ...), validated against the bidder the
// file's Validate calls name.
func paramsTestCases() []rawCase {
	consts := bidderConstants()
	files, err := filepath.Glob("adapters/*/params_test.go")
	if err != nil {
		log.Fatal(err)
	}
	sort.Strings(files)
	var cases []rawCase
	for _, path := range files {
		fset := token.NewFileSet()
		file, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			log.Fatal(err)
		}
		bidder := ""
		var literals []string
		ast.Inspect(file, func(n ast.Node) bool {
			switch n := n.(type) {
			case *ast.CallExpr:
				if name := validatedBidder(n, consts); name != "" && bidder == "" {
					bidder = name
				}
			case *ast.CompositeLit:
				if isStringSlice(n.Type) {
					for _, elt := range n.Elts {
						if lit, ok := elt.(*ast.BasicLit); ok && lit.Kind == token.STRING {
							if value, err := strconv.Unquote(lit.Value); err == nil {
								literals = append(literals, value)
							}
						}
					}
				}
			}
			return true
		})
		if bidder == "" {
			continue
		}
		for _, literal := range literals {
			cases = append(cases, rawCase{bidder: bidder, source: "params_test " + path, ext: []byte(literal)})
		}
	}
	return cases
}

func validatedBidder(call *ast.CallExpr, consts map[string]string) string {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || sel.Sel.Name != "Validate" || len(call.Args) != 2 {
		return ""
	}
	arg, ok := call.Args[0].(*ast.SelectorExpr)
	if !ok {
		return ""
	}
	if pkg, ok := arg.X.(*ast.Ident); !ok || pkg.Name != "openrtb_ext" {
		return ""
	}
	return consts[arg.Sel.Name]
}

func isStringSlice(expr ast.Expr) bool {
	arr, ok := expr.(*ast.ArrayType)
	if !ok || arr.Len != nil {
		return false
	}
	ident, ok := arr.Elt.(*ast.Ident)
	return ok && ident.Name == "string"
}

// bidderConstants maps the openrtb_ext Bidder* constants to their values.
func bidderConstants() map[string]string {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, "openrtb_ext/bidders.go", nil, 0)
	if err != nil {
		log.Fatal(err)
	}
	consts := map[string]string{}
	for _, decl := range file.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.CONST {
			continue
		}
		for _, spec := range gen.Specs {
			value, ok := spec.(*ast.ValueSpec)
			if !ok || len(value.Names) != 1 || len(value.Values) != 1 {
				continue
			}
			if lit, ok := value.Values[0].(*ast.BasicLit); ok && lit.Kind == token.STRING {
				if text, err := strconv.Unquote(lit.Value); err == nil {
					consts[value.Names[0].Name] = text
				}
			}
		}
	}
	return consts
}

// fixtureCases takes imp[].ext.bidder of every adapter JSON test's mockBidRequest, validated
// against the adapter's own bidder.
func fixtureCases(known map[string]bool) []rawCase {
	var cases []rawCase
	err := filepath.WalkDir("adapters", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}
		parts := strings.Split(filepath.ToSlash(path), "/")
		if len(parts) < 4 || !known[parts[1]] {
			return nil
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		var fixture struct {
			MockBidRequest struct {
				Imp []struct {
					Ext json.RawMessage `json:"ext"`
				} `json:"imp"`
			} `json:"mockBidRequest"`
		}
		if json.Unmarshal(data, &fixture) != nil {
			return nil
		}
		for _, imp := range fixture.MockBidRequest.Imp {
			var ext map[string]json.RawMessage
			if json.Unmarshal(imp.Ext, &ext) != nil {
				continue
			}
			if params, ok := ext["bidder"]; ok {
				cases = append(cases, rawCase{bidder: parts[1], source: "fixture " + filepath.ToSlash(path), ext: params})
			}
		}
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	return cases
}

// sampleRequestCases takes imp[].ext.prebid.bidder.<bidder> and imp[].ext.<bidder> of every
// request in endpoints/openrtb2/sample-requests.
func sampleRequestCases(known map[string]bool) []rawCase {
	var cases []rawCase
	err := filepath.WalkDir("endpoints/openrtb2/sample-requests", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		source := "sample " + filepath.ToSlash(path)
		walkImps(data, func(bidder string, params []byte) {
			if known[bidder] {
				cases = append(cases, rawCase{bidder: bidder, source: source, ext: params})
			}
		})
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	sort.SliceStable(cases, func(i, j int) bool {
		if cases[i].source != cases[j].source {
			return cases[i].source < cases[j].source
		}
		if cases[i].bidder != cases[j].bidder {
			return cases[i].bidder < cases[j].bidder
		}
		return string(cases[i].ext) < string(cases[j].ext)
	})
	return cases
}

func walkImps(raw []byte, emit func(bidder string, params []byte)) {
	var obj map[string]json.RawMessage
	if json.Unmarshal(raw, &obj) == nil {
		if imps, ok := obj["imp"]; ok {
			var list []json.RawMessage
			if json.Unmarshal(imps, &list) == nil {
				for _, imp := range list {
					impParams(imp, emit)
				}
			}
		}
		for _, value := range obj {
			walkImps(value, emit)
		}
		return
	}
	var arr []json.RawMessage
	if json.Unmarshal(raw, &arr) == nil {
		for _, value := range arr {
			walkImps(value, emit)
		}
	}
}

func impParams(imp []byte, emit func(bidder string, params []byte)) {
	var fields struct {
		Ext map[string]json.RawMessage `json:"ext"`
	}
	if json.Unmarshal(imp, &fields) != nil {
		return
	}
	for name, params := range fields.Ext {
		emit(name, params)
	}
	var prebid struct {
		Bidder map[string]json.RawMessage `json:"bidder"`
	}
	if json.Unmarshal(fields.Ext["prebid"], &prebid) != nil {
		return
	}
	for name, params := range prebid.Bidder {
		emit(name, params)
	}
}

// node is a JSON value that keeps object key order, used to read schemas and build cases.
type node struct {
	kind byte // 'o' object, 'a' array, 's' string, 'n' number, 'b' boolean, 'z' null
	keys []string
	vals []*node
	text string
	flag bool
}

func parseNode(data []byte) (*node, error) {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	return readNode(dec)
}

func mustNode(text string) *node {
	n, err := parseNode([]byte(text))
	if err != nil {
		log.Fatalf("%q: %v", text, err)
	}
	return n
}

func readNode(dec *json.Decoder) (*node, error) {
	tok, err := dec.Token()
	if err != nil {
		return nil, err
	}
	switch t := tok.(type) {
	case json.Delim:
		n := &node{kind: 'a'}
		if t == '{' {
			n.kind = 'o'
		}
		for dec.More() {
			if n.kind == 'o' {
				keyToken, err := dec.Token()
				if err != nil {
					return nil, err
				}
				key, _ := keyToken.(string)
				n.keys = append(n.keys, key)
			}
			value, err := readNode(dec)
			if err != nil {
				return nil, err
			}
			n.vals = append(n.vals, value)
		}
		_, err := dec.Token()
		return n, err
	case string:
		return &node{kind: 's', text: t}, nil
	case json.Number:
		return &node{kind: 'n', text: string(t)}, nil
	case bool:
		return &node{kind: 'b', flag: t}, nil
	}
	return &node{kind: 'z'}, nil
}

func (n *node) get(key string) *node {
	if n == nil || n.kind != 'o' {
		return nil
	}
	for i, k := range n.keys {
		if k == key {
			return n.vals[i]
		}
	}
	return nil
}

func (n *node) clone() *node {
	c := *n
	c.keys = append([]string(nil), n.keys...)
	c.vals = nil
	for _, v := range n.vals {
		c.vals = append(c.vals, v.clone())
	}
	return &c
}

func (n *node) set(key string, value *node) {
	for i, k := range n.keys {
		if k == key {
			n.vals[i] = value
			return
		}
	}
	n.keys = append(n.keys, key)
	n.vals = append(n.vals, value)
}

func (n *node) remove(key string) {
	for i, k := range n.keys {
		if k == key {
			n.keys = append(n.keys[:i], n.keys[i+1:]...)
			n.vals = append(n.vals[:i], n.vals[i+1:]...)
			return
		}
	}
}

func (n *node) encode() []byte {
	var b bytes.Buffer
	n.write(&b)
	return b.Bytes()
}

func (n *node) write(b *bytes.Buffer) {
	switch n.kind {
	case 'o':
		b.WriteByte('{')
		for i, k := range n.keys {
			if i > 0 {
				b.WriteByte(',')
			}
			writeString(b, k)
			b.WriteByte(':')
			n.vals[i].write(b)
		}
		b.WriteByte('}')
	case 'a':
		b.WriteByte('[')
		for i, v := range n.vals {
			if i > 0 {
				b.WriteByte(',')
			}
			v.write(b)
		}
		b.WriteByte(']')
	case 's':
		writeString(b, n.text)
	case 'n':
		b.WriteString(n.text)
	case 'b':
		b.WriteString(strconv.FormatBool(n.flag))
	default:
		b.WriteString("null")
	}
}

func writeString(b *bytes.Buffer, s string) {
	data, err := json.Marshal(s)
	if err != nil {
		log.Fatal(err)
	}
	b.Write(data)
}

func schemaTypes(s *node) []string {
	t := s.get("type")
	if t == nil {
		return nil
	}
	if t.kind == 's' {
		return []string{t.text}
	}
	var types []string
	for _, v := range t.vals {
		if v.kind == 's' {
			types = append(types, v.text)
		}
	}
	return types
}

func hasType(s *node, name string) bool {
	for _, t := range schemaTypes(s) {
		if t == name {
			return true
		}
	}
	return false
}

func requiredOf(s *node) []string {
	var names []string
	if r := s.get("required"); r != nil {
		for _, v := range r.vals {
			if v.kind == 's' {
				names = append(names, v.text)
			}
		}
	}
	return names
}

func isObjectSchema(s *node) bool {
	return hasType(s, "object") || (len(schemaTypes(s)) == 0 && (s.get("properties") != nil || s.get("required") != nil))
}

func intKeyword(s *node, key string) (int, bool) {
	v := s.get(key)
	if v == nil || v.kind != 'n' {
		return 0, false
	}
	n, err := strconv.Atoi(v.text)
	return n, err == nil
}

func ratKeyword(s *node, key string) (*big.Rat, bool) {
	v := s.get(key)
	if v == nil || v.kind != 'n' {
		return nil, false
	}
	return new(big.Rat).SetString(v.text)
}

func ratText(r *big.Rat) string {
	if r.IsInt() {
		return r.Num().String()
	}
	text := strings.TrimRight(r.FloatString(30), "0")
	return strings.TrimSuffix(text, ".")
}

// synth builds a value the schema probably accepts.
func synth(s *node, depth int) *node {
	if s == nil || s.kind != 'o' {
		return mustNode("1")
	}
	if e := s.get("enum"); e != nil && e.kind == 'a' && len(e.vals) > 0 {
		return e.vals[0].clone()
	}
	switch {
	case hasType(s, "string"):
		return &node{kind: 's', text: synthString(s)}
	case hasType(s, "integer"):
		return &node{kind: 'n', text: synthNumber(s, true)}
	case hasType(s, "number"):
		return &node{kind: 'n', text: synthNumber(s, false)}
	case hasType(s, "boolean"):
		return mustNode("true")
	case hasType(s, "array"):
		arr := &node{kind: 'a'}
		count := 1
		if min, ok := intKeyword(s, "minItems"); ok && min > count {
			count = min
		}
		for i := 0; i < count && depth < 4; i++ {
			item := synth(s.get("items"), depth+1)
			if item.kind == 'n' && i > 0 {
				item.text = strconv.Itoa(i + 1)
			}
			if item.kind == 's' && i > 0 {
				item.text += strconv.Itoa(i)
			}
			arr.vals = append(arr.vals, item)
		}
		return arr
	case isObjectSchema(s):
		obj := &node{kind: 'o'}
		if depth > 3 {
			return obj
		}
		props := s.get("properties")
		required := requiredOf(s)
		for _, combinator := range []string{"oneOf", "anyOf", "allOf"} {
			if list := s.get(combinator); list != nil && len(list.vals) > 0 {
				required = append(required, requiredOf(list.vals[0])...)
			}
		}
		for _, name := range required {
			if obj.get(name) == nil {
				obj.set(name, synth(props.get(name), depth+1))
			}
		}
		return obj
	}
	return mustNode(`"x"`)
}

func synthString(s *node) string {
	if f := s.get("format"); f != nil {
		switch f.text {
		case "uuid":
			return "123e4567-e89b-12d3-a456-426614174000"
		case "hostname":
			return "example.com"
		}
	}
	text := "abc"
	if min, ok := intKeyword(s, "minLength"); ok && min > len(text) {
		text = strings.Repeat("a", min)
	}
	if max, ok := intKeyword(s, "maxLength"); ok && max < len(text) {
		text = strings.Repeat("a", max)
	}
	return text
}

func synthNumber(s *node, integer bool) string {
	value := big.NewRat(1, 1)
	if !integer {
		value = big.NewRat(3, 2)
	}
	if min, ok := ratKeyword(s, "minimum"); ok {
		value = min
		if e := s.get("exclusiveMinimum"); e != nil && e.kind == 'b' && e.flag {
			value = new(big.Rat).Add(min, big.NewRat(1, 1))
		}
	}
	if min, ok := ratKeyword(s, "exclusiveMinimum"); ok {
		value = new(big.Rat).Add(min, big.NewRat(1, 1))
	}
	if max, ok := ratKeyword(s, "maximum"); ok && value.Cmp(max) > 0 {
		value = max
	}
	if integer && !value.IsInt() {
		value = new(big.Rat).SetInt(new(big.Int).Div(value.Num(), value.Denom()))
	}
	return ratText(value)
}

// generated is one case built from a schema.
type generated struct {
	label string
	doc   []byte
}

func generateCases(schema *node) []generated {
	var cases []generated
	add := func(label string, doc []byte) {
		cases = append(cases, generated{label: label, doc: doc})
	}
	base := synth(schema, 0)
	add("base", base.encode())
	add("empty object", []byte("{}"))
	for _, doc := range []string{"null", "[]", `"x"`, "1", "1.5", "true"} {
		add("root "+doc, []byte(doc))
	}
	if base.kind != 'o' {
		return cases
	}
	props := schema.get("properties")
	if props != nil && props.kind == 'o' {
		full := base.clone()
		wrong := &node{kind: 'o'}
		for i, key := range props.keys {
			if full.get(key) == nil {
				full.set(key, synth(props.vals[i], 1))
			}
			wrong.set(key, mustNode("[]"))
		}
		add("all properties", full.encode())
		add("all properties wrong", wrong.encode())
	}
	extra := base.clone()
	extra.set("zzUnknown", mustNode("1"))
	add("extra property", extra.encode())
	extra.set("zzOther", mustNode(`"x"`))
	add("two extra properties", extra.encode())
	if deps := schema.get("dependencies"); deps != nil && deps.kind == 'o' {
		for _, key := range deps.keys {
			only := &node{kind: 'o'}
			only.set(key, synth(props.get(key), 1))
			add("dependency "+key+" alone", only.encode())
			with := base.clone()
			with.set(key, synth(props.get(key), 1))
			add("dependency "+key, with.encode())
		}
	}
	mutate(base, nil, schema, 0, add)
	return cases
}

// mutate adds cases that change one property (at path) of base.
func mutate(base *node, path []string, schema *node, depth int, add func(string, []byte)) {
	props := schema.get("properties")
	if props == nil || props.kind != 'o' {
		return
	}
	for i, key := range props.keys {
		ps := props.vals[i]
		p := append(append([]string(nil), path...), key)
		label := strings.Join(p, ".")
		if lookup(base, p) != nil {
			add(label+" removed", withValue(base, p, nil).encode())
		}
		for _, candidate := range candidates(ps) {
			add(label+" = "+candidate, withValue(base, p, mustNode(candidate)).encode())
		}
		if depth < 2 && isObjectSchema(ps) {
			mutate(withValue(base, p, synth(ps, depth+1)), p, ps, depth+1, add)
		}
	}
}

func lookup(n *node, path []string) *node {
	for _, key := range path {
		n = n.get(key)
		if n == nil {
			return nil
		}
	}
	return n
}

// withValue returns a copy of root with path set to value (removed when value is nil).
func withValue(root *node, path []string, value *node) *node {
	out := root.clone()
	cur := out
	for i, key := range path {
		if i == len(path)-1 {
			if value == nil {
				cur.remove(key)
			} else {
				cur.set(key, value)
			}
			break
		}
		next := cur.get(key)
		if next == nil || next.kind != 'o' {
			next = &node{kind: 'o'}
			cur.set(key, next)
		}
		cur = next
	}
	return out
}

// candidates are values (JSON text) to try for a property with schema ps.
func candidates(ps *node) []string {
	out := []string{`"str"`, "1", "1.5", "true", "null", "{}", "[]"}
	numeric := hasType(ps, "integer") || hasType(ps, "number")
	if numeric {
		out = append(out, "-1", "0", "1.0", "1e2", "-0", "1e400", "1e999999999")
		for _, key := range []string{"minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"} {
			if limit, ok := ratKeyword(ps, key); ok {
				for _, delta := range []*big.Rat{big.NewRat(0, 1), big.NewRat(-1, 1), big.NewRat(1, 1), big.NewRat(-1, 2), big.NewRat(1, 2)} {
					out = append(out, ratText(new(big.Rat).Add(limit, delta)))
				}
			}
		}
		if multiple, ok := ratKeyword(ps, "multipleOf"); ok {
			for _, factor := range []*big.Rat{big.NewRat(2, 1), big.NewRat(3, 2), big.NewRat(-1, 1)} {
				out = append(out, ratText(new(big.Rat).Mul(multiple, factor)))
			}
		}
	}
	if hasType(ps, "string") {
		out = append(out, `""`, `"日本語"`, `" x"`, `"a<b>&\"\\"`)
		if min, ok := intKeyword(ps, "minLength"); ok && min > 0 && min < 100 {
			out = append(out, strconv.Quote(strings.Repeat("é", min-1)), strconv.Quote(strings.Repeat("a", min)))
		}
		if max, ok := intKeyword(ps, "maxLength"); ok && max < 300 {
			out = append(out, strconv.Quote(strings.Repeat("a", max+1)), strconv.Quote(strings.Repeat("é", max)))
		}
		if f := ps.get("format"); f != nil {
			switch f.text {
			case "uuid":
				out = append(out, `"123E4567-E89B-12D3-A456-426614174000"`, `"123e4567e89b12d3a456426614174000"`, `"not-a-uuid"`)
			case "hostname":
				out = append(out, `"example.com"`, `"-bad-.com"`, `"a..b"`, strconv.Quote(strings.Repeat("a", 64)+".com"))
			}
		}
	}
	if e := ps.get("enum"); e != nil && e.kind == 'a' {
		for i, v := range e.vals {
			if i < 4 {
				out = append(out, string(v.encode()))
			}
			if i == 0 && v.kind == 'n' && !strings.ContainsAny(v.text, ".eE") {
				out = append(out, v.text+".0")
			}
		}
		out = append(out, `"zz-not-a-member"`)
	}
	if hasType(ps, "array") {
		item := string(synth(ps.get("items"), 1).encode())
		out = append(out, "[null]", "[{}]", `["a","a"]`, "[1,1.0]", "["+item+"]", "["+item+","+item+"]")
		if max, ok := intKeyword(ps, "maxItems"); ok && max < 50 {
			out = append(out, "["+strings.TrimSuffix(strings.Repeat(item+",", max+1), ",")+"]")
		}
	}
	if isObjectSchema(ps) {
		out = append(out, `{"zz":1}`)
	}
	seen := map[string]bool{}
	var unique []string
	for _, c := range out {
		if !seen[c] {
			seen[c] = true
			unique = append(unique, c)
		}
	}
	return unique
}
