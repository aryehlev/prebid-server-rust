// Command toolkit writes the differential goldens for pbs-gojson's ports of the rest of the Go
// JSON toolkit Prebid Server uses: gopkg.in/evanphx/json-patch.v5 (MergePatch, Equal),
// github.com/buger/jsonparser, encoding/json's and jsoniter's streaming decoders, and
// util/jsonutil's FindElement, DropElement and MergeClone into a json.RawMessage.
// tests/toolkit.rs replays them byte for byte.
//
// The inputs are the test tables of the Go packages themselves (read from their sources with
// go/ast, so every table case is replayed), hand-picked edge cases, and large seeded random
// corpora: nested documents and patches with colliding and escaped keys, nulls, arrays,
// unicode escapes and malformed variants. Every field is written with strconv.QuoteToASCII,
// one tab-separated case per line. A Go panic is recorded as the single result field
// "PANIC: <value>".
//
// Run it from the directory of this module (crates/gojson/testdata/go):
//
//	go run ./toolkit
//
// util/jsonutil's test table is read from the upstream checkout the module builds against.
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"log"
	"math"
	"math/rand"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/buger/jsonparser"
	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	jsonpatch "gopkg.in/evanphx/json-patch.v5"
)

// golden is one output file.
type golden struct {
	f     *os.File
	w     *bufio.Writer
	name  string
	cases int
}

func create(dir, name, header string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{f: f, w: bufio.NewWriter(f), name: name}
	for _, line := range strings.Split(header, "\n") {
		g.write("# " + line + "\n")
	}
	return g
}

func (g *golden) write(s string) {
	if _, err := g.w.WriteString(s); err != nil {
		log.Fatal(err)
	}
}

// row writes one case: every field quoted, tab-separated.
func (g *golden) row(fields ...string) {
	for i, f := range fields {
		if i > 0 {
			g.write("\t")
		}
		g.write(strconv.QuoteToASCII(f))
	}
	g.write("\n")
	g.cases++
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := g.f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s: %d cases\n", g.name, g.cases)
}

// catch runs f, turning a panic into the single field "PANIC: <value>".
func catch(f func() []string) (out []string) {
	defer func() {
		if r := recover(); r != nil {
			out = []string{fmt.Sprintf("PANIC: %v", r)}
		}
	}()
	return f()
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func itoa(i int) string { return strconv.Itoa(i) }

// path encodes a key path as "<n>:" followed by the keys joined with \x1f.
func path(keys []string) string {
	return itoa(len(keys)) + ":" + strings.Join(keys, "\x1f")
}

// paths encodes several key paths joined with \x1e.
func paths(ps [][]string) string {
	parts := make([]string, len(ps))
	for i, p := range ps {
		parts[i] = path(p)
	}
	return strings.Join(parts, "\x1e")
}

// ---- random documents -----------------------------------------------------------------

// node is a generated JSON value; object keys keep their raw (escaped) spelling.
type node struct {
	kind byte // 'o' object, 'a' array, 's' scalar
	raw  string
	keys []string // raw key literals, with quotes
	kids []*node
}

type gen struct {
	r *rand.Rand
}

func (g *gen) pick(options ...string) string { return options[g.r.Intn(len(options))] }

func (g *gen) ws() string {
	if g.r.Intn(3) != 0 {
		return ""
	}
	return g.pick(" ", "  ", "\n", "\t", "\r\n ", " \n\t")
}

var keyChoices = []string{
	`"a"`, `"b"`, `"c"`, `"id"`, `"ext"`, `"imp"`, `"prebid"`, `"bidder"`, `"x"`, `"y"`,
	`"a\"b"`, `"a"`, `"é"`, `"é"`, `"<x>"`, `"a\/b"`, `"a/b"`, `""`, `"a\\"`,
	`"😀"`, `"\n"`, `"[0]"`, `"a b"`, `"tagid"`,
}

func (g *gen) key() string {
	return keyChoices[g.r.Intn(len(keyChoices))]
}

func (g *gen) scalar() string {
	switch g.r.Intn(14) {
	case 0:
		return "null"
	case 1:
		return "true"
	case 2:
		return "false"
	case 3:
		return g.pick("0", "-1", "1.5", "1e3", "1.0", "-0", "12345678901234567890", "1E-7", "2.50", "100")
	case 4:
		return g.pick(`"x"`, `""`, `"a\"b"`, `"<b>&"`, `" "`, `"é\u0000"`, `"😀"`,
			`"\ud800"`, `"tab\t"`, `"é"`, `"a\/b"`, `"\\"`, `"id"`, `"ext"`)
	default:
		return strconv.Itoa(g.r.Intn(100))
	}
}

func (g *gen) value(depth int) *node {
	if depth <= 0 || g.r.Intn(3) == 0 {
		return &node{kind: 's', raw: g.scalar()}
	}
	if g.r.Intn(3) == 0 {
		n := &node{kind: 'a'}
		for i := g.r.Intn(4); i > 0; i-- {
			n.kids = append(n.kids, g.value(depth-1))
		}
		return n
	}
	return g.object(depth)
}

func (g *gen) object(depth int) *node {
	n := &node{kind: 'o'}
	for i := g.r.Intn(5); i > 0; i-- {
		n.keys = append(n.keys, g.key())
		n.kids = append(n.kids, g.value(depth-1))
	}
	return n
}

func (g *gen) write(b *strings.Builder, n *node) {
	b.WriteString(g.ws())
	switch n.kind {
	case 's':
		b.WriteString(n.raw)
	case 'a':
		b.WriteByte('[')
		for i, k := range n.kids {
			if i > 0 {
				b.WriteString(g.ws())
				b.WriteByte(',')
			}
			g.write(b, k)
		}
		b.WriteString(g.ws())
		b.WriteByte(']')
	default:
		b.WriteByte('{')
		for i, k := range n.kids {
			if i > 0 {
				b.WriteString(g.ws())
				b.WriteByte(',')
			}
			b.WriteString(g.ws())
			b.WriteString(n.keys[i])
			b.WriteString(g.ws())
			b.WriteByte(':')
			g.write(b, k)
		}
		b.WriteString(g.ws())
		b.WriteByte('}')
	}
	b.WriteString(g.ws())
}

func (g *gen) text(n *node) string {
	var b strings.Builder
	g.write(&b, n)
	return b.String()
}

// mutate returns a malformed (or differently formed) variant of s.
func (g *gen) mutate(s string) string {
	if s == "" {
		return g.pick("{", "[", `"`, "x", " ")
	}
	i := g.r.Intn(len(s) + 1)
	switch g.r.Intn(5) {
	case 0:
		return s[:i]
	case 1:
		if i < len(s) {
			return s[:i] + s[i+1:]
		}
		return s[:i]
	case 2:
		return s[:i] + g.pick("{", "}", "[", "]", ",", ":", `"`, `\`, "x", "\xff", " ") + s[i:]
	case 3:
		return s + g.pick("x", " ", "}", "]", ",", "{}", "1", `"`, " {", "\n")
	default:
		if i < len(s) {
			return s[:i] + g.pick("{", "}", "[", "]", ",", ":", `"`, "n", "0") + s[i+1:]
		}
		return s
	}
}

// unescapedKey is the key jsonparser compares a path element against.
func unescapedKey(raw string) string {
	inner := raw[1 : len(raw)-1]
	out, err := jsonparser.Unescape([]byte(inner), nil)
	if err != nil {
		return inner
	}
	return string(out)
}

// randomPath walks n, returning a key path that mostly exists.
func (g *gen) randomPath(n *node) []string {
	var keys []string
	for steps := 1 + g.r.Intn(4); steps > 0; steps-- {
		switch {
		case n.kind == 'o' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			k := unescapedKey(n.keys[i])
			if g.r.Intn(6) == 0 {
				k = n.keys[i][1 : len(n.keys[i])-1] // the escaped spelling
			}
			keys = append(keys, k)
			n = n.kids[i]
		case n.kind == 'a' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			keys = append(keys, "["+itoa(i)+"]")
			n = n.kids[i]
		default:
			steps = 0
		}
	}
	switch g.r.Intn(8) {
	case 0:
		keys = append(keys, g.pick("missing", "a", "[0]", "[5]", "id"))
	case 1:
		if len(keys) > 0 {
			keys[len(keys)-1] = g.pick("zz", "[9]", "[-1]", "[x]", "[", "")
		}
	}
	return keys
}

// ---- json-patch -------------------------------------------------------------------------

func mergePatchRow(g *golden, doc, patch string) {
	out := catch(func() []string {
		res, err := jsonpatch.MergePatch([]byte(doc), []byte(patch))
		return []string{string(res), errText(err)}
	})
	g.row(append([]string{"MP", doc, patch}, out...)...)
}

func equalRow(g *golden, a, b string) {
	out := catch(func() []string {
		return []string{strconv.FormatBool(jsonpatch.Equal([]byte(a), []byte(b)))}
	})
	g.row(append([]string{"EQ", a, b}, out...)...)
}

var mergePatchCases = [][2]string{
	// merge_test.go
	{`{ "title": "hello" }`, `{ "title": "goodbye" }`},
	{`{ "title": "hello", "age": 18 }`, `{ "title": "goodbye" }`},
	{`{ "title": null }`, `{ "title": {"foo": "bar"} }`},
	{`{"a": [ {"b":"c"} ] }`, `{"a": [1]}`},
	{`{"a": [ {"b":"c"} ] }`, `{"a": [null, 1]}`},
	{`["a",null]`, `[null]`},
	{`["a"]`, `[null]`},
	{`["a", "b"]`, `["a", null]`},
	{`{"a":["b"]}`, `{"a": ["b", null]}`},
	{`{"a":[]}`, `{"a": ["b", null, null, "a"]}`},
	{`{ "person": { "title": "hello", "age": 18 } }`, `{ "person": { "title": "goodbye" } }`},
	{`[1]`, `[2]`},
	{`{ "a": 1 }`, `[2]`},
	{`[1]`, `{ "a": 1 }`},
	{`{ "a": 1, "b": 1 }`, `{ "a": 2 }`},
	{`[[[[`, `1`},
	{`1`, `[[[[`},
	{`{ "array": ["one", "two"] }`, `{ "array": [] }`},
	{`{"a":"b"}`, `{"a":"c"}`},
	{`{"a":"b"}`, `{"b":"c"}`},
	{`{"a":"b"}`, `{"a":null}`},
	{`{"a":"b","b":"c"}`, `{"a":null}`},
	{`{"a":["b"]}`, `{"a":"c"}`},
	{`{"a":"c"}`, `{"a":["b"]}`},
	{`{"a":{"b": "c"}}`, `{"a": {"b": "d","c": null}}`},
	{`{"a":[{"b":"c"}]}`, `{"a":[1]}`},
	{`["a","b"]`, `["c","d"]`},
	{`{"a":"b"}`, `["c"]`},
	{`{"e":null}`, `{"a":1}`},
	{`[1,2]`, `{"a":"b","c":null}`},
	{`{}`, `{"a":{"bb":{"ccc":null}}}`},
	{`{"a":"foo"}`, `null`},
	{`{"a":"foo"}`, `"bar"`},
	{`{ "obj": { "title/escaped": "hello" } }`, `{ "obj": { "title/escaped": "goodbye" } }`},
	// key order, escaping and malformed input
	{`{"z":1,"a":{"y":2,"b":3},"m":"<&>"}`, `{"q":1}`},
	{`{"z":1,"a":{"y":2,"b":3}}`, `{"a":{"c":null,"d":{"z":1,"a":2}}}`},
	{`{"a": [ {"b" : "c"} ] }`, `{"a": [null, {"x":null,"b":1}]}`},
	{`{"a":1}`, `{"a":[{"b":null}]}`},
	{`{"a":{"x":1}}`, `{"a":[{"b":null}]}`},
	{`[1]`, `{"b":{"d":null,"c":1}}`},
	{`{"a":1}`, `[{"b":null},null]`},
	{`{"a":1}`, `"x"`},
	{`null`, `{}`},
	{`{}`, `null`},
	{`{`, `{}`},
	{`{}`, `{`},
	{`{"é\/":"é \/  "}`, `{}`},
	{`{"a":"é \/  "}`, `{"a":{"é":1}}`},
	{`{"a":1,"a":2}`, `{}`},
	{`{"a":{"x":1,"x":2}}`, `{"b":1}`},
	{`{"a":{"x":1,"x":2}}`, `{"a":{"b":1}}`},
	{"{\"a\":\"\xff\"}", `{"b":1}`},
	{"{\"\xff\":1}", `{"b":1}`},
	{`{"a":1}`, `{"b":1.50e+3}`},
	{`1`, `[ 1 , 2 ]`},
	{`"x"`, `true`},
	{`"x"`, `{"a":null,"b":[null,{"c":null}]}`},
	{`true`, `[]`},
	{`{"a":" "}`, `{"b":" <"}`},
	{`{"a":1,"a":2}`, `{"b":1}`},
	{`{"a":{"b":1}}`, `{"a":{"b":2}}`},
	{` {"a" : 1} `, ` {"b" : [ 1 , {"c" : null} ] } `},
	{`{}`, `{}`},
	{``, `{}`},
	{`{}`, ``},
	{`{"a":1}x`, `{}`},
	{`{}`, `{"a":1}x`},
	{`{"a":{}}`, `{"a":{"b":{}}}`},
	{`{"a":{"b":1}}`, `{"a":{"b":null}}`},
	{`{"a":{"b":null}}`, `{"a":{"c":1}}`},
	{`{"a":null}`, `{"a":{"b":null}}`},
}

var equalCases = [][2]string{
	{`{"a":1,"b":[1,{"c":2}]}`, `{"b":[1,{"c":2}],"a":1}`},
	{`null`, `{}`},
	{`{}`, `null`},
	{`[]`, `null`},
	{`null`, `[]`},
	{`1.0`, `1`},
	{`1`, ` 1 `},
	{`[null]`, `[null]`},
	{`[1,null]`, `[2,null]`},
	{`x`, `x`},
	{`{`, `{`},
	{`{"a":null}`, `{"a":null}`},
	{`{"a":null}`, `{"a":1}`},
	{`{"a":1}`, `{"b":1}`},
	{`{"a":1,"a":2}`, `{"a":2}`},
	{`{"a":1}`, `{"a":1}`},
	{`"a"`, `"a"`},
	{`[1,2]`, `[1,2,3]`},
	{`{"a":[1]}`, `{"a":[1]}`},
	{`{"a":[1]}`, `{"a":{"0":1}}`},
	{`true`, `true`},
	{`"x"`, `{}`},
	{`[]`, `{}`},
	{`{}`, `[]`},
}

func writeMergePatch(dir string, r *rand.Rand) {
	g := create(dir, "mergepatch.txt", "jsonpatch.MergePatch: MP doc patch out err\njsonpatch.Equal: EQ a b result")
	defer g.close()
	for _, c := range mergePatchCases {
		mergePatchRow(g, c[0], c[1])
	}
	for _, c := range equalCases {
		equalRow(g, c[0], c[1])
	}
	gn := &gen{r: r}
	for i := 0; i < 3000; i++ {
		docNode := gn.object(3)
		doc := gn.text(docNode)
		var patch string
		switch r.Intn(6) {
		case 0:
			patch = gn.text(gn.value(3))
		case 1:
			patch = gn.mutate(gn.text(gn.object(3)))
		case 2:
			doc = gn.mutate(doc)
			patch = gn.text(gn.object(2))
		default:
			patch = gn.text(derivePatch(gn, docNode, 3))
		}
		mergePatchRow(g, doc, patch)
		if i%3 == 0 {
			other := doc
			if r.Intn(2) == 0 {
				other = gn.text(docNode)
			} else if r.Intn(2) == 0 {
				other = gn.text(derivePatch(gn, docNode, 2))
			}
			equalRow(g, doc, other)
		}
	}
}

// derivePatch builds a patch over n's keys: nulls, replacements, nested merges, new keys.
func derivePatch(g *gen, n *node, depth int) *node {
	p := &node{kind: 'o'}
	if n.kind != 'o' {
		return g.object(depth)
	}
	for i, k := range n.keys {
		if g.r.Intn(2) == 0 {
			continue
		}
		var v *node
		switch g.r.Intn(4) {
		case 0:
			v = &node{kind: 's', raw: "null"}
		case 1:
			v = g.value(depth - 1)
		default:
			if n.kids[i].kind == 'o' && depth > 1 {
				v = derivePatch(g, n.kids[i], depth-1)
			} else {
				v = g.value(depth - 1)
			}
		}
		p.keys = append(p.keys, k)
		p.kids = append(p.kids, v)
	}
	if g.r.Intn(2) == 0 {
		p.keys = append(p.keys, g.key())
		p.kids = append(p.kids, g.value(depth-1))
	}
	return p
}

// ---- jsonparser -------------------------------------------------------------------------

var setValues = []string{`1`, `"x"`, `{"n":null}`, `[1,2]`, `null`, `"a\"b"`}

// jsonparserRows writes every operation on data at keys.
func jsonparserRows(g *golden, data string, keys []string, setValue string) {
	d := []byte(data)
	p := path(keys)
	g.row(append([]string{"GET", data, p}, catch(func() []string {
		v, t, o, err := jsonparser.Get(d, keys...)
		return []string{string(v), itoa(int(t)), itoa(o), errText(err)}
	})...)...)
	g.row(append([]string{"GETSTRING", data, p}, catch(func() []string {
		v, err := jsonparser.GetString(d, keys...)
		return []string{v, errText(err)}
	})...)...)
	g.row(append([]string{"GETINT", data, p}, catch(func() []string {
		v, err := jsonparser.GetInt(d, keys...)
		return []string{strconv.FormatInt(v, 10), errText(err)}
	})...)...)
	g.row(append([]string{"GETBOOL", data, p}, catch(func() []string {
		v, err := jsonparser.GetBoolean(d, keys...)
		return []string{strconv.FormatBool(v), errText(err)}
	})...)...)
	g.row(append([]string{"ARRAYEACH", data, p}, catch(func() []string {
		var items []string
		o, err := jsonparser.ArrayEach(d, func(value []byte, t jsonparser.ValueType, offset int, _ error) {
			items = append(items, string(value), itoa(int(t)), itoa(offset))
		}, keys...)
		return append([]string{itoa(o), errText(err)}, items...)
	})...)...)
	g.row(append([]string{"OBJECTEACH", data, p}, catch(func() []string {
		var items []string
		err := jsonparser.ObjectEach(d, func(key, value []byte, t jsonparser.ValueType, offset int) error {
			items = append(items, string(key), string(value), itoa(int(t)), itoa(offset))
			return nil
		}, keys...)
		return append([]string{errText(err)}, items...)
	})...)...)
	g.row(append([]string{"DELETE", data, p}, catch(func() []string {
		// Delete may write into its input's spare capacity; give it a private copy.
		return []string{string(jsonparser.Delete(append([]byte(nil), d...), keys...))}
	})...)...)
	g.row(append([]string{"SET", data, p, setValue}, catch(func() []string {
		v, err := jsonparser.Set(append([]byte(nil), d...), []byte(setValue), keys...)
		return []string{string(v), errText(err)}
	})...)...)
}

func eachKeyRow(g *golden, data string, ps [][]string) {
	g.row(append([]string{"EACHKEY", data, paths(ps)}, catch(func() []string {
		var items []string
		o := jsonparser.EachKey([]byte(data), func(idx int, value []byte, t jsonparser.ValueType, err error) {
			items = append(items, itoa(idx), string(value), itoa(int(t)), errText(err))
		}, ps...)
		return append([]string{itoa(o)}, items...)
	})...)...)
}

func parseRows(g *golden, in string) {
	g.row(append([]string{"PARSESTRING", in}, catch(func() []string {
		v, err := jsonparser.ParseString([]byte(in))
		return []string{v, errText(err)}
	})...)...)
	g.row(append([]string{"PARSEINT", in}, catch(func() []string {
		v, err := jsonparser.ParseInt([]byte(in))
		return []string{strconv.FormatInt(v, 10), errText(err)}
	})...)...)
	g.row(append([]string{"UNESCAPE", in}, catch(func() []string {
		v, err := jsonparser.Unescape([]byte(in), nil)
		return []string{string(v), errText(err)}
	})...)...)
}

// tableCase is one entry of a Go test table.
type tableCase struct {
	data    string
	keys    []string
	hasKeys bool
	setData string
}

// moduleDir is the source directory of a module dependency.
func moduleDir(mod string) string {
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", mod).Output()
	if err != nil {
		log.Fatalf("go list %s: %v", mod, err)
	}
	return strings.TrimSpace(string(out))
}

// literal evaluates a string literal, []byte(literal) or []string{literals...}.
func literal(e ast.Expr) (string, []string, bool) {
	switch v := e.(type) {
	case *ast.BasicLit:
		if v.Kind == token.STRING {
			s, err := strconv.Unquote(v.Value)
			return s, nil, err == nil
		}
	case *ast.CallExpr:
		if len(v.Args) == 1 {
			return literal(v.Args[0])
		}
	case *ast.CompositeLit:
		list := []string{}
		for _, elt := range v.Elts {
			s, _, ok := literal(elt)
			if !ok {
				return "", nil, false
			}
			list = append(list, s)
		}
		return "", list, true
	}
	return "", nil, false
}

// tableCases reads the composite literals of a Go test file that have a dataField key.
func tableCases(file, dataField, keysField, setField string) []tableCase {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, file, nil, 0)
	if err != nil {
		log.Fatal(err)
	}
	var cases []tableCase
	ast.Inspect(f, func(n ast.Node) bool {
		lit, ok := n.(*ast.CompositeLit)
		if !ok {
			return true
		}
		var c tableCase
		found := false
		for _, elt := range lit.Elts {
			kv, ok := elt.(*ast.KeyValueExpr)
			if !ok {
				continue
			}
			name, ok := kv.Key.(*ast.Ident)
			if !ok {
				continue
			}
			switch name.Name {
			case dataField:
				if s, _, ok := literal(kv.Value); ok {
					c.data, found = s, true
				}
			case keysField:
				if _, list, ok := literal(kv.Value); ok {
					c.keys, c.hasKeys = list, true
				}
			case setField:
				if s, _, ok := literal(kv.Value); ok {
					c.setData = s
				}
			}
		}
		if found {
			cases = append(cases, c)
		}
		return true
	})
	return cases
}

const eachKeyJSON = `{
	"name": "Name",
	"order": "Order",
	"sum": 100,
	"len": 12,
	"isPaid": true,
	"nested": {"a":"test", "b":2, "nested3":{"a":"test3","b":4}, "c": "unknown"},
	"nested2": {
		"a":"test2",
		"b":3
	},
	"arr": [
		{
			"a":"zxc",
			"b": 1
		},
		{
			"a":"123",
			"b":2
		}
	],
	"arrInt": [1,2,3,4],
	"intPtr": 10,
	"a\n":{
		"b\n":99
	}
}`

func writeJSONParser(dir string, r *rand.Rand) {
	g := create(dir, "jsonparser.txt", strings.Join([]string{
		"buger/jsonparser v1.1.2; a path is \"<n>:\" + keys joined by \\x1f, several paths are joined by \\x1e",
		"GET data path value type offset err",
		"GETSTRING|GETINT|GETBOOL data path value err",
		"ARRAYEACH data path offset err (value type offset)*",
		"OBJECTEACH data path err (key value type offset)*",
		"DELETE data path out; SET data path value out err",
		"EACHKEY data paths offset (idx value type err)*",
		"PARSESTRING|PARSEINT|UNESCAPE in value err",
	}, "\n"))
	defer g.close()
	dir2 := moduleDir("github.com/buger/jsonparser")
	tables := 0
	for _, c := range tableCases(filepath.Join(dir2, "parser_test.go"), "json", "path", "setData") {
		set := c.setData
		if set == "" {
			set = `"set"`
		}
		jsonparserRows(g, c.data, c.keys, set)
		tables++
	}
	for _, file := range []string{"parser_test.go", "escape_test.go", "bytes_test.go"} {
		for _, c := range tableCases(filepath.Join(dir2, file), "in", "", "") {
			parseRows(g, c.data)
			tables++
		}
	}
	fmt.Printf("jsonparser test-table cases: %d\n", tables)
	// TestEachKey, TestArrayEach*, TestPanickingErrors, TestKeyDepth
	eachKeyRow(g, eachKeyJSON, [][]string{
		{"name"}, {"order"}, {"nested", "a"}, {"nested", "b"}, {"nested2", "a"},
		{"nested", "nested3", "b"}, {"arr", "[1]", "b"}, {"arrInt", "[3]"}, {"arrInt", "[5]"},
		{"nested"}, {"arr", "["}, {"a\n", "b\n"}, {"nested", "b"},
	})
	testPaths := [][]string{{"test"}, {"these"}, {"keys"}, {"please"}}
	eachKeyRow(g, `{"test":`, testPaths)
	eachKeyRow(g, `{"test":0}some":[{"these":[{"keys":"some"}]}]}some"}]}],"please":"some"}`, testPaths)
	jsonparserRows(g, `{"test":`, []string{"test"}, "1")
	jsonparserRows(g, `{"some":0}some":[{"some":[{"some":"some"}]}]}some"}]}],"some":"some"}`, []string{"x"}, "1")
	for _, n := range []int{128, 129} {
		var sb strings.Builder
		var keys []string
		sb.WriteString("{")
		for i := 0; i < n; i++ {
			fmt.Fprintf(&sb, `"key%d": %dx,`, i, i)
			keys = append(keys, fmt.Sprintf("key%d", i))
		}
		sb.WriteString("}")
		eachKeyRow(g, sb.String(), [][]string{keys})
	}
	for _, data := range []string{`{"a": { "b":[{"x": 1} ,{"x":2},{ "x":3}, {"x":4} ]}}`, `    ["AAA", "BBB", "CCC"]`, `    1`, `    `, `[]`, `[ ]`, "[\n]", `{"data": []}`, `{"data": [ ]}`, "{\"data\": [\n]}"} {
		jsonparserRows(g, data, nil, "1")
		jsonparserRows(g, data, []string{"a", "b"}, "1")
		jsonparserRows(g, data, []string{"data"}, "1")
	}
	for _, in := range []string{"", "-", "-0", "+1", "01", "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809", "1e5", `é`, `a\"b`, `😀`, `\uD800`, `\uDC00\uDC00`, `\uD83DxxDE00`, `\uD800`, `\q`, "\xff"} {
		parseRows(g, in)
	}
	gn := &gen{r: r}
	for i := 0; i < 800; i++ {
		docNode := gn.value(3)
		if r.Intn(4) != 0 {
			docNode = gn.object(3)
		}
		data := gn.text(docNode)
		if r.Intn(5) == 0 {
			data = gn.mutate(data)
		}
		for j := 0; j < 3; j++ {
			keys := gn.randomPath(docNode)
			if r.Intn(15) == 0 {
				keys = nil
			}
			jsonparserRows(g, data, keys, setValues[r.Intn(len(setValues))])
		}
		var ps [][]string
		for j := 1 + r.Intn(4); j > 0; j-- {
			ps = append(ps, gn.randomPath(docNode))
		}
		eachKeyRow(g, data, ps)
		if i%5 == 0 {
			parseRows(g, gn.scalar())
		}
	}
}

// ---- streaming decoders ---------------------------------------------------------------------

type streamSub struct {
	ID string `json:"id"`
}

type streamDoc struct {
	Status string          `json:"status"`
	N      int             `json:"n"`
	F      float64         `json:"f"`
	List   []string        `json:"list"`
	Ext    json.RawMessage `json:"ext"`
	Sub    *streamSub      `json:"sub"`
}

func marshalText(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		return "marshal error: " + err.Error()
	}
	return string(b)
}

func tokenText(t json.Token) string {
	switch v := t.(type) {
	case json.Delim:
		return "D" + v.String()
	case bool:
		return "B" + strconv.FormatBool(v)
	case float64:
		return "N" + strconv.FormatUint(math.Float64bits(v), 16)
	case string:
		return "S" + v
	case nil:
		return "Z"
	default:
		return fmt.Sprintf("?%T", t)
	}
}

func streamRows(g *golden, data string) {
	d := []byte(data)
	g.row(append([]string{"STD_ANY", data}, catch(func() []string {
		var v any
		dec := json.NewDecoder(bytes.NewReader(d))
		err := dec.Decode(&v)
		return []string{marshalText(v), errText(err), strconv.FormatInt(dec.InputOffset(), 10)}
	})...)...)
	g.row(append([]string{"STD_DOC", data}, catch(func() []string {
		var v streamDoc
		err := json.NewDecoder(bytes.NewReader(d)).Decode(&v)
		return []string{marshalText(v), errText(err)}
	})...)...)
	g.row(append([]string{"STD_MULTI", data}, catch(func() []string {
		dec := json.NewDecoder(bytes.NewReader(d))
		var out []string
		for i := 0; i < 8; i++ {
			var v any
			err := dec.Decode(&v)
			out = append(out, marshalText(v), errText(err), strconv.FormatInt(dec.InputOffset(), 10))
			if err != nil && i > 0 && errText(err) == out[len(out)-5] {
				break
			}
		}
		return out
	})...)...)
	g.row(append([]string{"TOKENS", data}, catch(func() []string {
		dec := json.NewDecoder(bytes.NewReader(d))
		var out []string
		for i := 0; i < 64; i++ {
			t, err := dec.Token()
			if err != nil {
				out = append(out, "E"+err.Error(), strconv.FormatInt(dec.InputOffset(), 10))
				break
			}
			out = append(out, tokenText(t), strconv.FormatInt(dec.InputOffset(), 10))
			if dec.More() {
				out = append(out, "M")
			}
		}
		return out
	})...)...)
	g.row(append([]string{"JI_ANY", data}, catch(func() []string {
		var v any
		err := jsoniter.ConfigCompatibleWithStandardLibrary.NewDecoder(bytes.NewReader(d)).Decode(&v)
		return []string{marshalText(v), errText(err)}
	})...)...)
	g.row(append([]string{"JI_DOC", data}, catch(func() []string {
		var v streamDoc
		err := jsoniter.ConfigCompatibleWithStandardLibrary.NewDecoder(bytes.NewReader(d)).Decode(&v)
		return []string{marshalText(v), errText(err)}
	})...)...)
}

var streamKeys = []string{`"status"`, `"n"`, `"f"`, `"list"`, `"ext"`, `"sub"`, `"id"`, `"x"`, `"N"`, `"Status"`}

func (g *gen) streamValue(depth int) *node {
	if depth <= 0 || g.r.Intn(2) == 0 {
		return &node{kind: 's', raw: g.pick(`"ok"`, `1`, `-2`, `1.5`, `1e400`, `true`, `null`, `"é"`, `"a\"b"`, `[]`, `["x","y"]`, `[1]`, `{}`, `{"id":"i"}`, `"1"`)}
	}
	n := &node{kind: 'o'}
	for i := g.r.Intn(5); i > 0; i-- {
		n.keys = append(n.keys, streamKeys[g.r.Intn(len(streamKeys))])
		n.kids = append(n.kids, g.streamValue(depth-1))
	}
	return n
}

var streamCases = []string{
	``, ` `, "\n\t ", `{}`, `{} `, `{}{}`, `{} {"status":"x"}`, `[1]x`, `[1] 2`, `1 2 3`, `123x`,
	`123`, `-`, `1.`, `tru`, `true`, `truex`, `nul`, `null`, `"abc`, `"abc"`, `"a"x`, `"a" "b"`,
	`{"status":"ok"}`, `{"status":"ok"`, `{"status":`, `{"status"`, `{"status`, `{`, `[`, `]`,
	`}`, `,`, `:`, `x`, `{"n":"1"}`, `{"n":1.5}`, `{"f":1e400}`, `1e400`, `[1e400]`,
	`{"list":"x"}`, `{"sub":{"id":1}}`, `{"ext":{"a" : 1}}`, `{"ext":nul}`, `[1,]`, `[,1]`,
	`{"a":1,}`, `{,}`, `{"a" 1}`, `{"a":1 "b":2}`, `[1 2]`, `[[[]]]`, `[{"a":[{}]}]`,
	`{"a":{"b":{"c":{}}}}`, "\xff", `"\xff"`, "\"\xff\"", `"\ud800"`, `{"a":1}`,
	`[true,false,null,"s",1,{"k":[]}]`, `{"a":[1,{"b":2}],"c":"d"}`, `{"n":1}]`, `[1]]`, `{}}`,
	strings.Repeat("[", 10001), strings.Repeat("[", 100) + strings.Repeat("]", 100),
}

func writeStream(dir string, r *rand.Rand) {
	g := create(dir, "stream.txt", strings.Join([]string{
		"json.NewDecoder(r).Decode: STD_ANY data marshal(any) err offset; STD_DOC data marshal(streamDoc) err",
		"repeated Decode(&any): STD_MULTI data (marshal err offset)*",
		"Token(): TOKENS data (token offset [M])* Eerr offset (D delim, B bool, N float64 bits in hex, S string, Z null; M: More())",
		"jsoniter NewDecoder(r).Decode: JI_ANY|JI_DOC data marshal err",
	}, "\n"))
	defer g.close()
	for _, data := range streamCases {
		streamRows(g, data)
	}
	gn := &gen{r: r}
	for i := 0; i < 1200; i++ {
		var data string
		switch r.Intn(4) {
		case 0:
			data = gn.text(gn.value(4))
		default:
			data = gn.text(gn.streamValue(3))
		}
		switch r.Intn(5) {
		case 0:
			data = gn.mutate(data)
		case 1:
			data += gn.pick("", " ", "x", "{}", "1", "]", ` {"status":"next"}`, "\n2")
		}
		streamRows(g, data)
	}
}

// ---- FindElement, DropElement ----------------------------------------------------------------

func elementRows(g *golden, ext string, names []string) {
	p := path(names)
	g.row(append([]string{"FIND", ext, p}, catch(func() []string {
		found, start, end, err := jsonutil.FindElement([]byte(ext), names...)
		return []string{strconv.FormatBool(found), strconv.FormatInt(start, 10), strconv.FormatInt(end, 10), errText(err)}
	})...)...)
	g.row(append([]string{"DROP", ext, p}, catch(func() []string {
		out, err := jsonutil.DropElement(append([]byte(nil), ext...), names...)
		return []string{string(out), strconv.FormatBool(out == nil), errText(err)}
	})...)...)
}

// randomNames picks names along a path of n, and string values.
func (g *gen) randomNames(n *node) []string {
	var names []string
	for steps := 1 + g.r.Intn(3); steps > 0; steps-- {
		switch {
		case n.kind == 'o' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			var k string
			if err := json.Unmarshal([]byte(n.keys[i]), &k); err != nil {
				k = n.keys[i]
			}
			names = append(names, k)
			n = n.kids[i]
		case n.kind == 'a' && len(n.kids) > 0:
			n = n.kids[g.r.Intn(len(n.kids))]
		case n.kind == 's' && strings.HasPrefix(n.raw, `"`):
			var s string
			if err := json.Unmarshal([]byte(n.raw), &s); err == nil {
				names = append(names, s)
			}
			steps = 0
		default:
			steps = 0
		}
	}
	if len(names) == 0 || g.r.Intn(8) == 0 {
		names = append(names, g.pick("missing", "a", "id", "x"))
	}
	return names
}

func writeElement(dir string, r *rand.Rand) {
	g := create(dir, "element.txt", "jsonutil.FindElement: FIND ext names found start end err\njsonutil.DropElement: DROP ext names out isNil err\n(names are encoded as jsonparser paths)")
	defer g.close()
	repo := moduleDir("github.com/prebid/prebid-server/v4")
	cases := tableCases(filepath.Join(repo, "util", "jsonutil", "jsonutil_test.go"), "input", "elementToRemove", "")
	fmt.Printf("TestDropElement cases: %d\n", len(cases))
	for _, c := range cases {
		elementRows(g, c.data, c.keys)
	}
	for _, c := range [][2]string{
		{`{"a":1,"b":2}`, "a"}, {`{"a":1,"b":2}`, "b"}, {`{"a" : 1 , "b" : 2 }`, "a"}, {`["a",1]`, "a"},
		{`{"k":"a","b":1}`, "a"}, {`"a"`, "a"}, {`{"a":1 "b":2}`, "a"}, {`{"a":`, "a"}, {`{"a`, "a"},
		{`{"a":1}`, "a"}, {`{"a":1e400}`, "a"}, {`{"a":[1,2],"b":{"a":3}}`, "a"}, {``, "a"},
		{`{"x":{"a":1},"a":2}`, "a"}, {`[{"a":1},{"a":2}]`, "a"},
	} {
		elementRows(g, c[0], []string{c[1]})
	}
	elementRows(g, `{"a":1}`, nil)
	elementRows(g, `{"a":{"b":1}}`, []string{"a", "b"})
	elementRows(g, `{"a":{"b":1},"c":2}`, []string{"a", "b"})
	elementRows(g, `{"a":{"c":1,"b":2}}`, []string{"a", "b"})
	elementRows(g, `{"a":1}`, []string{"a", "b"})
	elementRows(g, `["a",{"b":1}]`, []string{"a", "b"})
	elementRows(g, `{"a":"b"}`, []string{"a", "b"})
	gn := &gen{r: r}
	for i := 0; i < 1500; i++ {
		docNode := gn.object(4)
		ext := gn.text(docNode)
		if r.Intn(6) == 0 {
			ext = gn.mutate(ext)
		}
		elementRows(g, ext, gn.randomNames(docNode))
	}
}

// ---- MergeClone into a json.RawMessage --------------------------------------------------------

func mergeCloneRow(g *golden, existing *string, data string) {
	var raw json.RawMessage
	given := "nil"
	if existing != nil {
		raw = json.RawMessage(*existing)
		given = "=" + *existing
	}
	g.row(append([]string{"MC", given, data}, catch(func() []string {
		err := jsonutil.MergeClone(&raw, []byte(data))
		got := "nil"
		if raw != nil {
			got = "=" + string(raw)
		}
		return []string{got, errText(err)}
	})...)...)
}

func strp(s string) *string { return &s }

func writeMergeClone(dir string, r *rand.Rand) {
	g := create(dir, "mergeclone.txt", "jsonutil.MergeClone(&rawMessage, data): MC existing data result err (existing and result: nil or =bytes)")
	defer g.close()
	// TestMergeCloneExt, on the json.RawMessage itself
	for _, c := range []struct {
		existing *string
		incoming string
	}{
		{strp(`{"a":1,"b":2}`), `{"b":200,"c":3}`},
		{nil, `null`},
		{nil, `{}`},
		{strp(`{"b":2}`), `null`},
		{strp(`{"b":2}`), `{}`},
		{strp(`{"b":2}`), `malformed`},
		{nil, `{"a":1}`},
		{strp(`{}`), `{"a":1}`},
		{nil, `{"b":2}`},
		{strp(`malformed`), `{"a":1}`},
		{strp(`null`), `{"a":1}`},
		{strp(``), `{"a":1}`},
		{strp(`{"a":1}`), `1`},
		{strp(`{"a":1}`), `"x"`},
		{strp(`{"a":1}`), `[1]`},
		{strp(`{"a":1}`), `{"a":null}`},
		{strp(`{"a":1}`), `{"a":1}x`},
		{strp(`{"a":1}`), `{"a":`},
		{strp(`{"a":1}`), ``},
		{strp(`{"a":1}`), ` `},
		{strp(`[1]`), `{"a":1}`},
		{strp(`{"a":1}`), ` {"b" : 2} `},
	} {
		mergeCloneRow(g, c.existing, c.incoming)
	}
	gn := &gen{r: r}
	for i := 0; i < 600; i++ {
		docNode := gn.object(3)
		var existing *string
		if r.Intn(5) != 0 {
			s := gn.text(docNode)
			if r.Intn(8) == 0 {
				s = gn.mutate(s)
			}
			existing = &s
		}
		data := gn.text(derivePatch(gn, docNode, 3))
		switch r.Intn(6) {
		case 0:
			data = gn.mutate(data)
		case 1:
			data = gn.text(gn.value(2))
		}
		mergeCloneRow(g, existing, data)
	}
}

func main() {
	out := flag.String("out", "toolkit", "output directory")
	flag.Parse()
	writeMergePatch(*out, rand.New(rand.NewSource(1)))
	writeJSONParser(*out, rand.New(rand.NewSource(2)))
	writeStream(*out, rand.New(rand.NewSource(3)))
	writeElement(*out, rand.New(rand.NewSource(4)))
	writeMergeClone(*out, rand.New(rand.NewSource(5)))
}
