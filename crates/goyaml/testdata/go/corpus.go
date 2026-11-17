package main

import (
	"bytes"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"log"
	"math/rand"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// corpusCase is one YAML input of corpus.txt.
type corpusCase struct {
	name  string
	input []byte
}

// corpus is every input of corpus.txt, without duplicates.
func corpus(repo string) []corpusCase {
	var cases []corpusCase
	seen := map[string]bool{}
	add := func(name, input string) {
		if seen[input] {
			return
		}
		seen[input] = true
		cases = append(cases, corpusCase{name, []byte(input)})
	}
	for i, input := range handwritten {
		add(fmt.Sprintf("handwritten/%d", i), input)
	}
	for i, input := range yamlTestInputs() {
		add(fmt.Sprintf("yaml.v3/%d", i), input)
	}
	seeds := append([]corpusCase(nil), cases...)
	for _, key := range kitchenKeys {
		for i, scalar := range sampleScalars {
			add(fmt.Sprintf("kitchen/%s/%d", key, i), key+": "+scalar+"\n")
		}
		for i, block := range sampleBlocks {
			add(fmt.Sprintf("kitchen/%s/block%d", key, i), key+":\n"+block)
		}
	}
	bidders := 0
	for _, path := range yamlFiles(repo) {
		if !strings.HasPrefix(path, "static/bidder-info/") || bidders == 12 {
			continue
		}
		data, err := os.ReadFile(filepath.Join(repo, path))
		if err != nil {
			log.Fatal(err)
		}
		seeds = append(seeds, corpusCase{path, data})
		bidders++
	}
	rnd := rand.New(rand.NewSource(20261117))
	for _, seed := range seeds {
		for k := 0; k < 5; k++ {
			add(fmt.Sprintf("mutation/%s/%d", seed.name, k), string(mutate(seed.input, rnd)))
		}
	}
	for i, input := range limits() {
		add(fmt.Sprintf("limits/%d", i), input)
	}
	return cases
}

// limits are inputs at and over yaml.v3's depth limits (10000 flow levels or indents), and
// aliasing just under and over its excessive-aliasing ratio.
func limits() []string {
	laughs := func(levels, width int) string {
		var b strings.Builder
		b.WriteString("l0: &l0 [" + strings.Repeat("x, ", width-1) + "x]\n")
		for i := 1; i < levels; i++ {
			ref := fmt.Sprintf("*l%d", i-1)
			b.WriteString(fmt.Sprintf("l%d: &l%d [%s%s]\n", i, i, strings.Repeat(ref+", ", width-1), ref))
		}
		return b.String()
	}
	return []string{
		strings.Repeat("[", 300) + strings.Repeat("]", 300),
		strings.Repeat("[", 10001),
		"x: " + strings.Repeat("{", 10001),
		strings.Repeat("- ", 10001),
		strings.Repeat("{a: ", 300) + "x" + strings.Repeat("}", 300),
		strings.Repeat("- ", 300) + "x\n",
		// 60300 of 60806 decodes through aliases: over the 0.99 ratio.
		"a: &a [1" + strings.Repeat(", 1", 199) + "]\nb: [*a" + strings.Repeat(", *a", 299) + "]\n",
		// 3030 of 3165: under it.
		"a: &a [1" + strings.Repeat(", 1", 99) + "]\nb: [*a" + strings.Repeat(", *a", 29) + "]\n",
		laughs(4, 9),
		laughs(5, 9),
		laughs(9, 9),
		"[" + strings.Repeat("[1, 2], ", 2000) + "[3]]",
	}
}

// yamlTestInputs extracts the YAML documents of yaml.v3's decode_test.go and encode_test.go:
// every constant string in their test tables.
func yamlTestInputs() []string {
	out, err := exec.Command("go", "env", "GOMODCACHE").Output()
	if err != nil {
		log.Fatal(err)
	}
	dir := filepath.Join(strings.TrimSpace(string(out)), "gopkg.in", "yaml.v3@v3.0.1")
	tables := map[string][]string{
		"decode_test.go": {"unmarshalTests", "decoderTests", "unmarshalErrorTests",
			"unmarshalerTests", "unmarshalNullTests", "unmarshalStrictTests", "mergeTests",
			"mergeTestsNested"},
		"encode_test.go": {"marshalTests", "marshalErrorTests"},
	}
	var inputs []string
	for _, file := range []string{"decode_test.go", "encode_test.go"} {
		want := map[string]bool{}
		for _, name := range tables[file] {
			want[name] = true
		}
		f, err := parser.ParseFile(token.NewFileSet(), filepath.Join(dir, file), nil, 0)
		if err != nil {
			log.Fatal(err)
		}
		for _, decl := range f.Decls {
			gen, ok := decl.(*ast.GenDecl)
			if !ok || gen.Tok != token.VAR {
				continue
			}
			for _, spec := range gen.Specs {
				vs, ok := spec.(*ast.ValueSpec)
				if !ok {
					continue
				}
				for i, name := range vs.Names {
					if !want[name.Name] || i >= len(vs.Values) {
						continue
					}
					ast.Inspect(vs.Values[i], func(n ast.Node) bool {
						if s, ok := constString(n); ok {
							inputs = append(inputs, s)
							return false
						}
						return true
					})
				}
			}
		}
	}
	return inputs
}

// constString folds a string literal or a concatenation of string literals.
func constString(n ast.Node) (string, bool) {
	switch e := n.(type) {
	case *ast.BasicLit:
		if e.Kind != token.STRING {
			return "", false
		}
		s, err := strconv.Unquote(e.Value)
		return s, err == nil
	case *ast.BinaryExpr:
		if e.Op != token.ADD {
			return "", false
		}
		l, ok := constString(e.X)
		if !ok {
			return "", false
		}
		r, ok := constString(e.Y)
		if !ok {
			return "", false
		}
		return l + r, true
	case *ast.ParenExpr:
		return constString(e.X)
	}
	return "", false
}

// specials are the bytes mutations insert: YAML indicators, whitespace and line breaks.
var specials = []byte(":-?[]{},#&*!|>'\"%@` \t\n\r\\.~0")

// mutate applies one random edit to input: a byte deleted, inserted or replaced, a line
// duplicated, deleted, swapped with the next, indented or dedented, or the input truncated.
func mutate(input []byte, rnd *rand.Rand) []byte {
	b := append([]byte(nil), input...)
	special := func() byte { return specials[rnd.Intn(len(specials))] }
	switch op := rnd.Intn(9); op {
	case 0:
		if len(b) > 0 {
			i := rnd.Intn(len(b))
			b = append(b[:i], b[i+1:]...)
		}
	case 1:
		i := rnd.Intn(len(b) + 1)
		b = append(b[:i], append([]byte{special()}, b[i:]...)...)
	case 2:
		if len(b) > 0 {
			b[rnd.Intn(len(b))] = special()
		}
	case 6:
		b = b[:rnd.Intn(len(b)+1)]
	default:
		lines := bytes.SplitAfter(b, []byte("\n"))
		i := rnd.Intn(len(lines))
		line := lines[i]
		switch op {
		case 3:
			lines = append(lines[:i+1], append([][]byte{line}, lines[i+1:]...)...)
		case 4:
			lines = append(lines[:i], lines[i+1:]...)
		case 5:
			if i+1 < len(lines) {
				lines[i], lines[i+1] = lines[i+1], lines[i]
			}
		case 7:
			lines[i] = append([]byte(strings.Repeat(" ", 1+rnd.Intn(2))), line...)
		case 8:
			if len(line) > 0 && (line[0] == ' ' || line[0] == '\t') {
				lines[i] = line[1:]
			}
		}
		b = bytes.Join(lines, nil)
	}
	return b
}

// kitchenKeys are the keys of Kitchen's fields, and keys it does not have.
var kitchenKeys = []string{
	"int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64",
	"float32", "float64", "bool", "string", "duration", "time", "ptrint", "ptrstr", "ptrbool",
	"ptrdur", "ptrptr", "ints", "strings", "bytes", "any", "anys", "strmap", "intmap", "anymap",
	"keymap", "nested", "value", "nesteds", "bidtype", "bidtypes", "plain", "Int", "ina", "inb",
	"extra", "skipped", "-", "hidden",
}

// sampleScalars are the YAML values written under each Kitchen key.
var sampleScalars = []string{
	"~", "null", "''", `""`, "true", "false", "yes", "No", "on", "OFF", "y", "n",
	"0", "1", "-1", "+7", "42", "0x1F", "0o17", "017", "0b101", "1_000", "127", "128", "-129",
	"255", "256", "65535", "65536", "-32769", "2147483648", "4294967296",
	"9223372036854775807", "9223372036854775808", "-9223372036854775808",
	"-9223372036854775809", "18446744073709551615", "18446744073709551616",
	"1.5", "-1.5", ".5", "1e3", "1.0", "-0.0", ".inf", "-.inf", ".nan", "1e400", "3.5e38",
	"1e-50", "2.5e9", "-2.5e9", "4.2e19",
	`"12"`, "'1.5'", `"true"`, "abc", `"a b"`, "1h30m", "10s", "-5ms", `"2h"`, "1.5h",
	"2001-12-14", "2001-12-14T21:59:43.10Z", "2001-12-14 21:59:43.10 -5", `"2001-12-14"`,
	"2001-12-14T21:59:43.10+05:30", "1990-02-30",
	"!!str 1", `!!int "3"`, "!!float 1", "!!bool yes", "!!binary aGVsbG8=",
	"!!timestamp 2001-12-14", "!!null x", "!!str", "!!map {}", "!!seq []",
	"[]", "[1, 2]", `[a, "b"]`, "[[1]]", "[~]", "{}", "{a: 1}", "{a: x, b: [1]}", "{1: a}",
	"{a: {b: c}}", `"a,b,c"`, "[1, x]", "{a: 1, b: x}", "{ptr: {a: 2}}", "{a: 1, b: [x, y]}",
	"&x 5", "*x", "[&y 1, *y]",
}

// sampleBlocks are block collections written under each Kitchen key.
var sampleBlocks = []string{
	"  - 1\n  - two\n",
	"  a: 1\n  b: two\n",
	"  - a: 1\n  - b: [x]\n",
	"  <<: {a: 1}\n  b: 2\n",
	"  - - 1\n",
	"  |\n    text\n",
}

// handwritten are inputs for the scanner, parser, composer and decoder paths, valid and not.
var handwritten = []string{
	"",
	"\n",
	"~",
	"null",
	"a: 1",
	"a: 1\nb: 2\n",
	"a: 1\na: 2\n",
	"{a: 1, a: 2}",
	"a:\n  b: 1\n  b: 2\n",
	"[1, 2, 3]",
	"- a\n- b\n- - c\n  - d\n",
	"a:\n  b:\n    c: d\n",
	"? a\n: b\n",
	"? [a, b]\n: c\n",
	"? {a: b}\n: c\n",
	"{[1]: 2}",
	"1: a\n2: b\n",
	"1: a\nb: c\n",
	"true: a\nnull: b\n~: c\n",
	"1.5: a\n",
	"a: &x 1\nb: *x\n",
	"a: &x [1, 2]\nb: *x\n",
	"base: &base\n  a: 1\n  b: 2\nderived:\n  <<: *base\n  b: 3\n",
	"<<: {a: 1}\nb: 2\n",
	"<<: [{a: 1}, {a: 2, b: 2}]\nc: 3\n",
	"<<: 1\n",
	"<<: [1]\n",
	"<<: [[1]]\n",
	"a: {<<: {b: 1}, <<: {c: 2}}\n",
	"<<: {[1]: 2}\n",
	"a: *unknown\n",
	"a: &a\n  b: *a\n",
	"a: !!str 1\nb: !!int '2'\nc: !!float '3'\nd: !!bool 'true'\ne: !!null ''\n",
	"a: !!binary aGVsbG8=\n",
	"a: !!binary |\n  aGVs\n  bG8=\n",
	"a: !!binary ===\n",
	"a: !!timestamp 2001-12-14\n",
	"a: 2001-12-14t21:59:43.10-05:00\nb: 2001-12-14 21:59:43.10 -5\nc: 2001-12-14\nd: 2002-12-14T21:59:43.10Z\n",
	"a: !foo bar\n",
	"a: !!map {b: c}\n",
	"a: !!seq [1]\n",
	"a: !!str [1]\n",
	"a: !!int abc\n",
	"a: !!float abc\n",
	"a: !!bool 1\n",
	"a: !!null 1\n",
	"a: 0x1F\nb: 0o17\nc: 017\nd: 0b101\ne: +12\nf: -0\ng: 1_000\nh: 0x_1F\n",
	"a: 9223372036854775807\nb: 9223372036854775808\nc: -9223372036854775808\nd: -9223372036854775809\ne: 18446744073709551615\nf: 18446744073709551616\n",
	"a: 1.5\nb: -.5\nc: 1e3\nd: 1E-3\ne: .inf\nf: -.Inf\ng: .NaN\nh: 1e400\ni: +.5\nj: 1.\nk: 6.8523015e+5\nl: 685.230_15e+03\n",
	"a: yes\nb: No\nc: on\nd: OFF\ne: y\nf: True\ng: FALSE\nh: tRUE\n",
	"a: 'single ''quoted'''\nb: \"double \\\"quoted\\\" \\t \\u00e9 \\x41 \\U0001F600\"\nc: \"\\N\\_\\L\\P\"\n",
	"a: \"bad escape \\q\"\n",
	"a: |\n  literal\n  text\n\nb: >\n  folded\n  text\n\n  para\nc: |-\n  strip\nd: |+\n  keep\n\ne: >2\n   indented\n",
	"a: |0\n  x\n",
	"a: |\n\tx\n",
	"a: >-\n  one\n\n  two\n   three\n",
	"a: plain\n  continued\n  lines\n",
	"a: 'multi\n  line\n\n  single'\n",
	"a: \"multi\n  line\\\n  escaped\"\n",
	"--- a\n--- b\n...\n--- c\n",
	"---\na: 1\n---\nb: 2\n",
	"a: 1\n---\n- x\n...\n",
	"%YAML 1.2\n---\na: 1\n",
	"%YAML 1.1\n%YAML 1.1\n---\na\n",
	"%YAML 2.0\n---\na\n",
	"%TAG !e! tag:example.com,2000:\n---\na: !e!foo bar\n",
	"%TAG !e! tag:example.com,2000:\n%TAG !e! tag:x:\n---\na\n",
	"%FOO bar\n---\na\n",
	"a: !e!foo bar\n",
	"# comment only\n",
	"a: 1 # trailing\n# head\nb: 2\n\n# foot\n",
	"- # comment\n  - nested\n",
	"a: [1, 2,\n  3]\n",
	"a: {b: 1,\n  c: 2}\n",
	"a: [1, 2\n",
	"a: {b: 1\n",
	"a: ]\n",
	"a: }\n",
	"[a, b]: c\n",
	"a: b: c\n",
	"a:\n- b\n  c: d\n",
	"a:\n  - b\n - c\n",
	"- a\nb: c\n",
	"a: 'unterminated\n",
	"a: \"unterminated\n",
	"a: @at\n",
	"a: `back\n",
	"\tkey: value\n",
	"key:\tvalue\n",
	"a: b\n\tc: d\n",
	"\xef\xbb\xbfa: bom\n",
	"\xff\xfea\x00:\x00 \x001\x00\n\x00",
	"\xfe\xff\x00a\x00:\x00 \x001\x00\n",
	"a: \xff\n",
	"a: \xc3\n",
	"a: \x01\n",
	"a: \u00e9\u4e2d\U0001F600\n",
	"? \n",
	": a\n",
	"a: !!str\n",
	"a: &anchor\n",
	"&a a: b\n*a : c\n",
	"a: *a\n",
	"[*a]\n",
	"&a [*a]\n",
	"*a\n",
	"&a\n",
	"!!null\n",
	"!!str\n",
	"a: [b, c]: d\n",
	"{a: [}\n",
	"a: {b: [c, {d: e}]}\n",
	"a: 1\n  b: 2\n",
	"- 1\n- 2\n-3\n",
	"a: -\n",
	"a:\n  -\n  - b\n",
	"''\n",
	"\"\"\n",
	"'a'\n",
	"\"\\x80\"\n",
	"a: \"\\ud800\"\n",
	"a: \"\\U00110000\"\n",
	"? a\n? b\n",
	"a: 1\n...\nb: 2\n",
	"a: |\n  x\n b\n",
	"- [a, b, c]\n- {a: b}\n- 'c'\n- \"d\"\n- e\n",
	"a: 1\r\nb: 2\r\n",
	"a: 1\rb: 2\r",
	"a:\u0085b\n",
	"key with spaces: value with spaces\n",
	"a: b #c\n",
	"a: b#c\n",
	"a: 'b'c\n",
	"a: \"b\"c\n",
	"a: [b]c\n",
	"\"a\": 1\n'b': 2\n",
	"!!int 1: a\n",
	"{a, b: c}\n",
	"[a: b, c]\n",
	"[? a: b]\n",
	"{? a}\n",
	"a: ! b\n",
	"a: !<tag:yaml.org,2002:str> 1\n",
	"a: !<!> 1\n",
	"a: !!float 1\nb: !!float -1\nc: !!int 1.5\n",
	"time: 2015-02-24T18:19:39Z\nduration: 3s\nptrdur: 1m\n",
	"int: 1\nInt: 2\n",
	"ina: x\ninb: 3\nextra: y\n",
	"nested: {a: 1, ptr: {a: 2, ptr: {b: [x]}}}\nnesteds: [{a: 1}, {a: x}]\n",
	"bidtypes: [banner, video]\nbidtype: native\n",
	"keymap: {1: a, 2: b}\nintmap: {a: 1, b: x}\nstrmap: {a: 1, b: [x]}\n",
	"anys: [1, a, {b: c}, [d]]\nany: {1: 2}\n",
	"ptrptr: 5\nptrint: ~\nptrstr: x\nptrbool: on\n",
	"bytes: aGk=\nstrings: [a, 1, true]\nints: [1, x, 3]\n",
	"maintainer:\n  email: a@b.c\ncapabilities:\n  app:\n    mediaTypes: [banner, video]\n  site:\n    mediaTypes:\n      - native\nuserSync:\n  redirect:\n    url: http://x\n    userMacro: $UID\n  supports: [iframe]\n",
	"openrtb:\n  version: 2.6\n  gpp-supported: yes\n  multiformat-supported: off\ngvlVendorID: 70000\n",
	"geoscope: [USA, \"!EEA\"]\nendpointCompression: gzip\nxapi: {username: u, password: p, tracker: t}\nexperiment:\n  adsCert:\n    enabled: true\n",
	"userSync:\n  skipwhen:\n    gdpr: true\n    gpp_sid: [2, 6]\n  externalUrl: x\n  formatOverride: b\n  enabled: false\n",
	"disabled: 1\n",
	"endpoint: [x]\n",
	"aliasOf: appnexus\ndisabled: true\nmodifyingVastXmlAllowed: false\n",
	"debug: {allow: true, extra: 1}\nunknownKey: 1\n",
}
