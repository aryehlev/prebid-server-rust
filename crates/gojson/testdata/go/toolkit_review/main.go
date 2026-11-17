// Command toolkit_review writes a second, adversarial set of differential goldens for
// pbs-gojson's toolkit ports (json-patch MergePatch, buger/jsonparser, encoding/json's Decoder,
// jsonutil.FindElement/DropElement and MergeClone into a json.RawMessage). tests/toolkit_review.rs
// replays them byte for byte.
//
// It complements testdata/go/toolkit with what that corpus does not reach: JSON \u escapes in
// keys and values (UTF-16 surrogate pairs, escapes that spell the same key as a literal, U+2028,
// HTML characters), invalid UTF-8, exhaustive \uXXXX\uYYYY pairs for jsonparser.Unescape, every
// spelling of an array index path step, Set into every shape of partial path, Delete of every
// element of arrays and objects under every comma and whitespace layout, findKeyStart with an
// escaped key, duplicate keys and nulls in merge patches, and MergeClone over every shape of
// existing message. Fields are written with strconv.QuoteToASCII, one tab-separated case per
// line; a Go panic is recorded as the single result field "PANIC: <value>".
//
// Run it from the directory of this module (crates/gojson/testdata/go):
//
//	go run ./toolkit_review
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/buger/jsonparser"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	jsonpatch "gopkg.in/evanphx/json-patch.v5"
)

// bs is a single backslash, so that JSON escapes can be spelled out below.
const bs = "\\"

// esc turns every "~" of s into a backslash: esc(`"~u0061"`) is the JSON text "a".
func esc(s string) string { return strings.ReplaceAll(s, "~", bs) }

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

func paths(ps [][]string) string {
	parts := make([]string, len(ps))
	for i, p := range ps {
		parts[i] = path(p)
	}
	return strings.Join(parts, "\x1e")
}

// ---- random documents with escapes -------------------------------------------------------

// keyChoices are raw key literals. Several spell the same Go string ("a", "a"), some
// decode with replacement characters (lone surrogates, invalid UTF-8) and some need escaping
// again when encoding/json writes them (U+2028, "<").
var keyChoices = []string{
	`"a"`, `"b"`, `"ab"`, `"id"`, `"ext"`, `"imp"`, `""`,
	esc(`"~u0061"`), esc(`"~u0061b"`), esc(`"a~u0062"`), esc(`"~u0041"`),
	`"é"`, esc(`"~u00e9"`), esc(`"~u00E9"`),
	"\"\xf0\x9f\x98\x80\"", esc(`"~ud83d~ude00"`), esc(`"~uD83D~uDE00"`),
	"\"\xe2\x80\xa8\"", esc(`"~u2028"`), esc(`"~u2029"`),
	`"<"`, esc(`"~u003c"`), `"&"`, esc(`"~u0026"`),
	esc(`"~ud800"`), esc(`"~udc00"`), esc(`"~ud800~ue000"`), esc(`"~ufffd"`), "\"\xef\xbf\xbd\"",
	"\"\xff\"", "\"\xed\xa0\x80\"", "\"a\xc3\"",
	esc(`"a~u0000"`), esc(`"~/"`), `"/"`, esc(`"a~"b"`), esc(`"a~~"`), esc(`"~b"`), esc(`"~t"`),
	`"[0]"`, `"a b"`,
}

var stringChoices = []string{
	`"x"`, `""`, `"<b>&amp;"`, esc(`"~u003cb~u003e"`), "\"\xe2\x80\xa8\xe2\x80\xa9\"",
	esc(`"~u2028"`), esc(`"~ud83d~ude00"`), esc(`"~ud800"`), esc(`"~ud800~ue000"`), esc(`"~udbff~udfff"`),
	"\"\xff\xfe\"", "\"\xed\xa0\x80\"", esc(`"~/~b~f~n~r~t~"~~"`), esc(`"~u0000"`), `"é"`, esc(`"~u00e9"`),
	esc(`"a~"b"`), `"  spaced  "`, esc(`"~u0061"`), `"a"`, `"id"`,
}

var numberChoices = []string{
	"0", "-0", "1", "-1", "1.5", "1.0", "1e3", "1E+2", "1e-7", "-0.0", "0.1e-2", "1e400",
	"123456789012345678901234567890", "9007199254740993", "100",
}

type node struct {
	kind byte // 'o' object, 'a' array, 's' scalar
	raw  string
	keys []string
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

func (g *gen) key() string { return keyChoices[g.r.Intn(len(keyChoices))] }

func (g *gen) scalar() string {
	switch g.r.Intn(8) {
	case 0:
		return "null"
	case 1:
		return g.pick("true", "false")
	case 2, 3:
		return numberChoices[g.r.Intn(len(numberChoices))]
	default:
		return stringChoices[g.r.Intn(len(stringChoices))]
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

// derivePatch builds a patch over n's keys (in their own or another spelling): nulls,
// replacements, nested merges, duplicates and new keys.
func (g *gen) derivePatch(n *node, depth int) *node {
	if n.kind != 'o' {
		return g.object(depth)
	}
	p := &node{kind: 'o'}
	for i, k := range n.keys {
		if g.r.Intn(2) == 0 {
			continue
		}
		var v *node
		switch g.r.Intn(5) {
		case 0:
			v = &node{kind: 's', raw: "null"}
		case 1:
			v = g.value(depth - 1)
		case 2:
			// an object holding nulls, which are pruned unless it replaces a non-object
			v = &node{kind: 'o', keys: []string{g.key(), g.key()}, kids: []*node{{kind: 's', raw: "null"}, g.value(depth - 2)}}
		default:
			if n.kids[i].kind == 'o' && depth > 1 {
				v = g.derivePatch(n.kids[i], depth-1)
			} else {
				v = g.value(depth - 1)
			}
		}
		p.keys = append(p.keys, k)
		p.kids = append(p.kids, v)
		if g.r.Intn(6) == 0 {
			// the same key again: the last one wins
			p.keys = append(p.keys, k)
			p.kids = append(p.kids, g.pick1Null(depth))
		}
	}
	for j := g.r.Intn(3); j > 0; j-- {
		p.keys = append(p.keys, g.key())
		p.kids = append(p.kids, g.value(depth-1))
	}
	return p
}

func (g *gen) pick1Null(depth int) *node {
	if g.r.Intn(2) == 0 {
		return &node{kind: 's', raw: "null"}
	}
	return g.value(depth - 1)
}

// mutate returns a malformed (or differently formed) variant of s.
func (g *gen) mutate(s string) string {
	if s == "" {
		return g.pick("{", "[", `"`, "x", " ")
	}
	i := g.r.Intn(len(s) + 1)
	switch g.r.Intn(4) {
	case 0:
		return s[:i]
	case 1:
		if i < len(s) {
			return s[:i] + s[i+1:]
		}
		return s[:i]
	case 2:
		return s[:i] + g.pick("{", "}", "[", "]", ",", ":", `"`, bs, "x", "\xff", " ", bs+"u12") + s[i:]
	default:
		return s + g.pick("x", " ", "}", "]", ",", "{}", "1", `"`)
	}
}

// goKey is the Go string encoding/json decodes a raw key literal to.
func goKey(raw string) string {
	var k string
	if err := json.Unmarshal([]byte(raw), &k); err != nil {
		return raw[1 : len(raw)-1]
	}
	return k
}

// parserKey is the key jsonparser compares a path element against: the key unescaped with
// jsonparser's own Unescape.
func parserKey(raw string) string {
	inner := raw[1 : len(raw)-1]
	out, err := jsonparser.Unescape([]byte(inner), nil)
	if err != nil {
		return inner
	}
	return string(out)
}

// randomPath walks n with keys spelled as jsonparser compares them (or escaped, which then
// never matches), and with every spelling of an array index.
func (g *gen) randomPath(n *node) []string {
	var keys []string
	for steps := 1 + g.r.Intn(4); steps > 0; steps-- {
		switch {
		case n.kind == 'o' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			k := parserKey(n.keys[i])
			switch g.r.Intn(8) {
			case 0:
				k = n.keys[i][1 : len(n.keys[i])-1]
			case 1:
				k = goKey(n.keys[i])
			}
			keys = append(keys, k)
			n = n.kids[i]
		case n.kind == 'a' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			keys = append(keys, g.index(i))
			n = n.kids[i]
		default:
			steps = 0
		}
	}
	if g.r.Intn(6) == 0 {
		keys = append(keys, g.pick("missing", "a", "[0]", "[1]", "id", esc("~u0061")))
	}
	return keys
}

// index spells the array index i as a path step, mostly canonically.
func (g *gen) index(i int) string {
	if g.r.Intn(4) != 0 {
		return "[" + itoa(i) + "]"
	}
	return g.pick("[+"+itoa(i)+"]", "[0"+itoa(i)+"]", "[-"+itoa(i)+"]", "[ "+itoa(i)+"]", "["+itoa(i)+" ]",
		"["+itoa(i), itoa(i)+"]", "[]", "[", "[x]", "[99999999999999999999]", "[-9223372036854775808]",
		"[9223372036854775807]", "[1e1]", "["+itoa(i+1)+"]", "["+itoa(i+5)+"]")
}

// ---- json-patch -------------------------------------------------------------------------

func mergePatchRow(g *golden, doc, patch string) {
	out := catch(func() []string {
		res, err := jsonpatch.MergePatch([]byte(doc), []byte(patch))
		return []string{string(res), strconv.FormatBool(res == nil), errText(err)}
	})
	g.row(append([]string{"MP", doc, patch}, out...)...)
}

func writeMergePatch(dir string, r *rand.Rand) {
	g := create(dir, "mergepatch.txt", "jsonpatch.MergePatch: MP doc patch out outIsNil err")
	defer g.close()
	fixed := [][2]string{
		// a key spelled two ways is one map key: the later spelling's value wins
		{esc(`{"a":1,"~u0061":2}`), `{}`},
		{esc(`{"~u0061":1,"a":2}`), `{"b":3}`},
		{`{"a":1}`, esc(`{"~u0061":null}`)},
		{`{"a":{"x":1}}`, esc(`{"~u0061":{"y":2}}`)},
		{`{"a":{"x":1}}`, esc(`{"~u0061":{"y":2},"a":{"z":3}}`)},
		{`{"a":{"x":1}}`, `{"a":{"y":2},"a":null}`},
		{`{"a":{"x":1}}`, `{"a":null,"a":{"y":2}}`},
		{`{"a":{"x":1},"a":{"y":2}}`, `{"a":{"z":3}}`},
		{`{"a":null,"a":{"y":2}}`, `{"a":{"z":null}}`},
		// re-escaped keys and values
		{"{\"\xe2\x80\xa8\":\"\xe2\x80\xa8<>&\"}", `{}`},
		{"{\"\xe2\x80\xa8\":\"\xe2\x80\xa8<>&\"}", `{"b":1}`},
		{esc(`{"~u2028":"~u2028"}`), `{"b":1}`},
		{esc(`{"~ud800":1,"~udc00":2,"~ufffd":3}`), `{"b":1}`},
		{"{\"\xff\":1,\"\xfe\":2}", `{"b":1}`},
		{"{\"a\":\"\xff\"}", "{\"b\":\"\xff\"}"},
		{esc(`{"a":"~ud83d~ude00 ~u00e9"}`), esc(`{"b":"~ud83d~ude00"}`)},
		{esc(`{"~/":1}`), esc(`{"~/":2}`)},
		{`{"<":1}`, esc(`{"~u003c":2}`)},
		// numbers keep their spelling
		{`{"a":1E+2,"b":-0.0,"c":1e400}`, `{"d":0.1e-2}`},
		{`{"a":{"b":1E+2}}`, `{"a":{"c":123456789012345678901234567890}}`},
		// added and replaced values: nested nulls pruned only in added objects and arrays
		{`{"a":1}`, `{"a":{"b":null,"c":{"d":null}}}`},
		{`{"a":{}}`, `{"a":{"b":null,"c":{"d":null}}}`},
		{`{"a":[]}`, `{"a":{"b":null}}`},
		{`{"a":"s"}`, `{"a":[{"b":null},null]}`},
		{`{"a":{"x":1}}`, `{"a":[{"b":null},null]}`},
		{`{}`, `{"a":[{"b":null,"c":[{"d":null}]},null]}`},
		{`{"a":{"z":1,"b":{"y":1,"a":2}}}`, `{"c":1}`},
		{`{"a":{"z":1,"b":{"y":1,"a":2}}}`, `{"a":{"c":1}}`},
		{`{"a":{"z":1,"b":{"y":1,"a":2}}}`, `{"a":{"b":{"c":1}}}`},
		{`{"a" : [ 1 , { "z" : 1 , "a" : 2 } ] }`, `{"b":1}`},
		// non-object documents and patches
		{`[1,2]`, `{"a":null,"b":{"c":null,"d":[null,{"e":null}]}}`},
		{`"x"`, `[null,{"a":null}]`},
		{`1`, `{"a":1}`},
		{`true`, `{}`},
		{`[]`, `[]`},
		{`{}`, `[ ]`},
		{` { } `, ` { } `},
		{"\t{\"a\" :\n1 }\n", "\n{ \"b\" : [ 1 ,\t2 ] }\r\n"},
		{`{"a":1}`, `1`},
		{`{"a":1}`, `true`},
		{`{"a":1}`, `"str"`},
		{`null`, `null`},
		{`null`, `1`},
		{`[`, `null`},
		{`null`, `[`},
		{`{"a":1}`, `{"a":1}{}`},
		{`{"a":1} {}`, `{}`},
		{`{"a":01}`, `{}`},
		{`{}`, `{"a":01}`},
		{`{"a":1,}`, `{}`},
		{"{\"a\":\"\x01\"}", `{}`},
		{`{}`, "{\"a\":\"\x01\"}"},
		{`{"a":1}`, ``},
		{``, ``},
	}
	for _, c := range fixed {
		mergePatchRow(g, c[0], c[1])
	}
	// nesting at and past encoding/json's depth limit of 10000
	for _, depth := range []int{10000, 10001} {
		deep := strings.Repeat(`{"a":`, depth) + "1" + strings.Repeat("}", depth)
		mergePatchRow(g, `{"b":1}`, deep)
		deepArr := strings.Repeat("[", depth) + strings.Repeat("]", depth)
		mergePatchRow(g, deepArr, `{"b":1}`)
		mergePatchRow(g, `{}`, deepArr)
	}
	gn := &gen{r: r}
	for i := 0; i < 3000; i++ {
		docNode := gn.object(4)
		doc := gn.text(docNode)
		var patch string
		switch r.Intn(8) {
		case 0:
			patch = gn.text(gn.value(3))
		case 1:
			patch = gn.mutate(gn.text(gn.derivePatch(docNode, 3)))
		case 2:
			doc = gn.mutate(doc)
			patch = gn.text(gn.derivePatch(docNode, 3))
		case 3:
			doc = gn.text(gn.value(3))
			patch = gn.text(gn.derivePatch(docNode, 3))
		default:
			patch = gn.text(gn.derivePatch(docNode, 4))
		}
		mergePatchRow(g, doc, patch)
	}
}

// ---- jsonparser -------------------------------------------------------------------------

var setValues = []string{`1`, `"v"`, `{}`, `{"n":null}`, `[1]`, esc(`"~u00e9"`), ``}

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
	deleteRow(g, data, keys)
	setRow(g, data, keys, setValue)
}

func deleteRow(g *golden, data string, keys []string) {
	g.row(append([]string{"DELETE", data, path(keys)}, catch(func() []string {
		// Delete may write into its input's spare capacity; give it a private copy.
		return []string{string(jsonparser.Delete(append([]byte(nil), data...), keys...))}
	})...)...)
}

func setRow(g *golden, data string, keys []string, setValue string) {
	g.row(append([]string{"SET", data, path(keys), setValue}, catch(func() []string {
		v, err := jsonparser.Set(append([]byte(nil), data...), []byte(setValue), keys...)
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
	g.row(append([]string{"UNESCAPE", in}, catch(func() []string {
		v, err := jsonparser.Unescape([]byte(in), nil)
		return []string{string(v), errText(err)}
	})...)...)
	// GetString of the same contents as a string value
	data := `"` + in + `"`
	g.row(append([]string{"GETSTRING", data, path(nil)}, catch(func() []string {
		v, err := jsonparser.GetString([]byte(data))
		return []string{v, errText(err)}
	})...)...)
}

// unicodeHex are the code units whose pairs exercise Unescape's surrogate handling.
var unicodeHex = []string{"0000", "0041", "00e9", "07FF", "0800", "2028", "d7ff", "D800", "dbff", "DC00", "dfff", "e000", "fffd", "FFFF"}

// layouts are documents with every comma and whitespace layout around their members; "%s"
// is replaced by the member list.
var layouts = []struct{ open, sep, close string }{
	{"{", ",", "}"}, {"{ ", ", ", " }"}, {"{", " , ", "}"}, {"{\n  ", ",\n  ", "\n}"}, {"{", " ,", " }"},
	{"{ ", "\t,\t", "\t}"}, {"{", ",  ", "  }"},
}

var arrayLayouts = []struct{ open, sep, close string }{
	{"[", ",", "]"}, {"[ ", ", ", " ]"}, {"[", " , ", "]"}, {"[\n  ", ",\n  ", "\n]"}, {"[", " ,", " ]"},
	{"[", ",  ", "  ]"},
}

var memberValues = []string{`1`, `"s"`, `{}`, `{"x":1}`, `[]`, `[1,2]`, `true`, `null`, esc(`"~u0061"`), `"a,b}"`}

func writeJSONParser(dir string, r *rand.Rand) {
	g := create(dir, "jsonparser.txt", strings.Join([]string{
		"buger/jsonparser v1.1.2; a path is \"<n>:\" + keys joined by \\x1f, several paths are joined by \\x1e",
		"GET data path value type offset err",
		"GETSTRING|GETINT|GETBOOL data path value err",
		"ARRAYEACH data path offset err (value type offset)*",
		"OBJECTEACH data path err (key value type offset)*",
		"DELETE data path out; SET data path value out err",
		"EACHKEY data paths offset (idx value type err)*",
		"PARSESTRING|UNESCAPE in value err",
	}, "\n"))
	defer g.close()
	// Unescape over every pair of interesting UTF-16 code units, checked or unchecked \u
	for _, h1 := range unicodeHex {
		parseRows(g, bs+"u"+h1)
		parseRows(g, "x"+bs+"u"+h1+"y")
		for _, h2 := range unicodeHex {
			parseRows(g, bs+"u"+h1+bs+"u"+h2)
			parseRows(g, "a"+bs+"u"+h1+"xx"+h2+"b")
			parseRows(g, bs+"u"+h1+bs+"u"+h2[:3])
		}
	}
	for _, in := range []string{bs, bs + "u", bs + "u12", bs + "u12345", bs + "x", bs + "U0041", bs + bs + bs, bs + `"`,
		bs + "/" + bs + "b" + bs + "f" + bs + "n" + bs + "r" + bs + "t", "\xff" + bs + "n\xfe", strings.Repeat("a", 70) + bs + "n",
		strings.Repeat(bs+"ud83d"+bs+"ude00", 20)} {
		parseRows(g, in)
	}
	// Escaped keys in data and in the path
	escData := []string{
		esc(`{"~u0061":1,"b":{"~u0063":"~u00e9"}}`),
		esc(`{"a~"b":1,"a~~":2,"~/":3,"/":4}`),
		esc(`{"~ud83d~ude00":1,"~ud800~ue000":2,"~ud800":3}`),
		"{\"\xf0\x9f\x98\x80\":1,\"\xff\":2,\"\xe2\x80\xa8\":3}",
		esc(`{"x~"y": 1, "a~"b": 2}`),
		esc(`{"q~"r":1,"a~"b":2,"a~"b":3}`),
		esc(`{"a~"b~"c":1, "a~"b": 2}`),
		esc(`{"~u0061b":1, "a~"b": 2, "ab":3}`),
		esc(`{"x~"y":{"a~"b":1},"a~"b":2}`),
		esc(`{"~q":1,"a":2}`),
		esc(`{"a":{"~q":1,"b":2}}`),
		esc(`{"`) + strings.Repeat("x", 70) + esc(`~"":1,"a~"b":2}`),
		// findKeyStart unescapes an escaped search key into the stack buffer that later keys
		// are unescaped into: an escaped key of the same unescaped length then matches
		esc(`{"x~ty":1,"a~~~"b":2}`),
		esc(`{"x~ty":1,"pad":0,"a~~~"b":2}`),
		esc(`{"x~tq":1,"x~ty":5,"a~~~"b":2}`),
		esc(`{"~~":1,"x~ty":5,"a~~~"b":2}`),
		esc(`{"x~qy":1,"a~~~"b":2}`),
		esc(`{"x~qy":1,"x~"b":3,"a~~~"b":2}`),
		esc(`{"~t~t":1,"~~u00e9":2}`),
		esc(`{"~u00e9x":1,"~t~t":2,"~~u00e9":3}`),
		esc(`{"`) + strings.Repeat("x", 70) + esc(`~t":1,"a~~~"b":2}`),
		esc(`{"w":{"x~ty":1,"a~~~"b":2},"x~ty":3}`),
	}
	escKeys := [][]string{
		{"a"}, {esc("~u0061")}, {"b", "c"}, {"b", esc("~u0063")}, {`a"b`}, {esc(`a~"b`)}, {esc(`a~~`)}, {`a\`},
		{"/"}, {esc("~/")}, {"\xf0\x9f\x98\x80"}, {esc("~ud83d~ude00")}, {"\xef\xbf\xbd"}, {"\xff"}, {"\xe2\x80\xa8"},
		{esc("~u2028")}, {`x"y`}, {esc(`x~"y`)}, {esc(`q~"r`)}, {"ab"}, {esc("~u0061b")}, {esc(`x~"y`), esc(`a~"b`)},
		{esc(`a~"b~"c`)}, {esc(`~q`)}, {"a", esc(`~q`)}, {"a", "b"}, {strings.Repeat("x", 70) + `"`},
		{esc(strings.Repeat("x", 70) + `~"`)}, {esc(`a~"bc`)}, {esc(`a~"b`), "x"},
		{esc(`~u00e9`)}, {esc(`~t~t`)}, {esc(`~~u00e9`)}, {"w", esc(`a~"b`)}, {esc(`x~ty`)},
		{esc(strings.Repeat("y", 70) + `~"`)},
	}
	for _, d := range escData {
		for _, k := range escKeys {
			jsonparserRows(g, d, k, `"v"`)
		}
		var ps [][]string
		for j := 0; j < 5; j++ {
			ps = append(ps, escKeys[r.Intn(len(escKeys))])
		}
		eachKeyRow(g, d, ps)
	}
	// every spelling of an array index step
	indexData := []string{
		`{"a":[1,"x",{"b":2},[3,4]]}`, `[1,"x",{"b":2},[3,4]]`, `{"a":[]}`, `{"a":[ ]}`, `[]`, `{"a":[{"b":[{"c":1}]}]}`,
		` [ "s" , 2 ] `, `{"a":["x","y"],"b":1}`, `{"a":[1,2`, `{"a":[1,]}`, `{"a":[,1]}`, `{"a":["x"]}`, `["\"q\""]`,
	}
	steps := []string{"[0]", "[1]", "[2]", "[3]", "[4]", "[-1]", "[+0]", "[+1]", "[00]", "[01]", "[ 0]", "[0 ]", "[]", "[",
		"]", "[0", "0]", "[a]", "[9223372036854775807]", "[9223372036854775808]", "[99999999999999999999]",
		"[-9223372036854775808]", "[1e1]", "[0x1]", "[1_0]", "a"}
	for _, d := range indexData {
		for _, s := range steps {
			jsonparserRows(g, d, []string{s}, "9")
			jsonparserRows(g, d, []string{"a", s}, "9")
			jsonparserRows(g, d, []string{"a", s, "b"}, "9")
			jsonparserRows(g, d, []string{s, "b"}, "9")
			eachKeyRow(g, d, [][]string{{"a", s}, {s}})
			eachKeyRow(g, d, [][]string{{"a", s, "b"}, {"a", "[0]"}, {"b"}})
		}
	}
	// Set into every shape of partial path
	setData := []string{
		`{}`, `{ }`, "{\n}", `[]`, `[ ]`, `{"a":1}`, `{"a":{}}`, `{"a":{ }}`, `{"a":{"b":1}}`, `{"a":[]}`, `{"a":[1]}`,
		`{"a":[{"b":1}]}`, `{"a":[{}]}`, `{"a":[ {"b":1} ]}`, `{"a":"x"}`, `{"a":null}`, `  {"a":1}  `, "{\"a\":1}\n",
		`[1,2]`, `[{"a":1}]`, `1`, `"x"`, ``, `   `, `{"a":{"b":{"c":1}}}`, `{"a" : { "b" : 1 } }`, `{"x":1,"a":{}}`,
		`{"a":[[1]]}`, `{"a":[[{"b":1}]]}`, esc(`{"~u0061":1}`), `{"a":1`, `{"a":{"b":1}`,
	}
	setPaths := [][]string{
		{"a"}, {"b"}, {"a", "b"}, {"a", "c"}, {"a", "[0]"}, {"a", "[1]"}, {"a", "[0]", "b"}, {"a", "[1]", "b"},
		{"a", "[0]", "c"}, {"[0]"}, {"[1]"}, {"[0]", "a"}, {"[1]", "a"}, {"b", "c", "d"}, {"b", "[0]"},
		{"b", "[0]", "c"}, {"a", "b", "c"}, {"a", "b", "c", "d"}, {"a", "[0]", "[0]"}, {"a", "[0]", "[1]"},
		{"a", "[0]", "[0]", "b"}, {esc("~u0061")}, {"b", "[0]", "[1]"}, {"x", "a"},
	}
	for _, d := range setData {
		for _, p := range setPaths {
			for _, v := range []string{"1", `{"n":2}`} {
				setRow(g, d, p, v)
			}
			deleteRow(g, d, p)
		}
	}
	// Delete every member of objects and arrays under every layout, at several depths
	for n := 1; n <= 4; n++ {
		for _, l := range layouts {
			members := make([]string, n)
			keys := make([]string, n)
			for i := range members {
				keys[i] = string(rune('a' + i))
				members[i] = `"` + keys[i] + `":` + memberValues[r.Intn(len(memberValues))]
			}
			obj := l.open + strings.Join(members, l.sep) + l.close
			for _, wrap := range []string{"%s", `{"w":%s}`, `{"w":%s,"z":1}`, `{"z":1, "w": %s }`, `[%s]`} {
				data := strings.Replace(wrap, "%s", obj, 1)
				for _, k := range keys {
					p := []string{k}
					switch {
					case strings.HasPrefix(wrap, "{"):
						p = []string{"w", k}
					case strings.HasPrefix(wrap, "["):
						p = []string{"[0]", k}
					}
					deleteRow(g, data, p)
					setRow(g, data, p, "7")
				}
				deleteRow(g, data, []string{"missing"})
			}
		}
		for _, l := range arrayLayouts {
			elems := make([]string, n)
			for i := range elems {
				elems[i] = memberValues[r.Intn(len(memberValues))]
			}
			arr := l.open + strings.Join(elems, l.sep) + l.close
			for _, wrap := range []string{"%s", `{"w":%s}`, `{"w":%s,"z":1}`, `{"z":1, "w": %s }`, `[%s]`, `[1,%s]`} {
				data := strings.Replace(wrap, "%s", arr, 1)
				for i := 0; i <= n; i++ {
					var p []string
					switch {
					case strings.HasPrefix(wrap, "{"):
						p = []string{"w", "[" + itoa(i) + "]"}
					case wrap == `[%s]`:
						p = []string{"[0]", "[" + itoa(i) + "]"}
					case wrap == `[1,%s]`:
						p = []string{"[1]", "[" + itoa(i) + "]"}
					default:
						p = []string{"[" + itoa(i) + "]"}
					}
					deleteRow(g, data, p)
					setRow(g, data, p, "7")
					jsonparserRows(g, data, p, "7")
				}
			}
		}
	}
	// random documents with escapes, random paths
	gn := &gen{r: r}
	for i := 0; i < 500; i++ {
		docNode := gn.value(3)
		if r.Intn(4) != 0 {
			docNode = gn.object(3)
		}
		data := gn.text(docNode)
		if r.Intn(6) == 0 {
			data = gn.mutate(data)
		}
		for j := 0; j < 2; j++ {
			jsonparserRows(g, data, gn.randomPath(docNode), setValues[r.Intn(len(setValues))])
		}
		var ps [][]string
		for j := 1 + r.Intn(5); j > 0; j-- {
			ps = append(ps, gn.randomPath(docNode))
		}
		eachKeyRow(g, data, ps)
	}
}

// ---- encoding/json Decoder ----------------------------------------------------------------

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
	g.row(append([]string{"STD_RAW", data}, catch(func() []string {
		var v json.RawMessage
		dec := json.NewDecoder(bytes.NewReader(d))
		err := dec.Decode(&v)
		return []string{string(v), strconv.FormatBool(v == nil), errText(err), strconv.FormatInt(dec.InputOffset(), 10)}
	})...)...)
	g.row(append([]string{"TOKENS", data}, catch(func() []string {
		dec := json.NewDecoder(bytes.NewReader(d))
		var out []string
		for i := 0; i < 200; i++ {
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
	// a Token walk that decodes every value after a key, as FindElement does
	g.row(append([]string{"MIXED", data}, catch(func() []string {
		dec := json.NewDecoder(bytes.NewReader(d))
		var out []string
		for i := 0; i < 200; i++ {
			t, err := dec.Token()
			if err != nil {
				out = append(out, "E"+err.Error(), strconv.FormatInt(dec.InputOffset(), 10))
				break
			}
			out = append(out, tokenText(t), strconv.FormatInt(dec.InputOffset(), 10))
			if _, ok := t.(string); ok && i%2 == 1 {
				var v any
				err := dec.Decode(&v)
				out = append(out, "V"+marshalText(v), errText(err), strconv.FormatInt(dec.InputOffset(), 10))
			}
		}
		return out
	})...)...)
}

func writeStream(dir string, r *rand.Rand) {
	g := create(dir, "stream.txt", strings.Join([]string{
		"json.NewDecoder(r).Decode: STD_ANY data marshal(any) err offset; STD_RAW data raw isNil err offset",
		"Token(): TOKENS data (token offset [M])* Eerr offset (D delim, B bool, N float64 bits in hex, S string, Z null; M: More())",
		"Token() with Decode(&any) after every other string token: MIXED data (token offset [Vvalue err offset])* Eerr offset",
	}, "\n"))
	defer g.close()
	for _, data := range []string{
		esc(`{"~u0061":"~ud83d~ude00","b":["~u2028",1E+2,-0.0,"~ud800"]}`), "\"\xff\xfe\"", esc(`"~ud800~ue000"`),
		esc(`["~u00e9", "~/", "~q"]`), `[1e400]`, `{"a":1e400}`, `[01]`, `[1.]`, `[-]`, `["a" "b"]`, `{"a" "b"}`,
		`{"a":}`, `{:1}`, `[,]`, `{"a":1,}`, `[1,]`, `{} []`, `[] {}`, " \t\r\n[ \t\r\n] \t\r\n", `{"a":{"a":{"a":[]}}}`,
		strings.Repeat(`{"a":`, 10000) + "1" + strings.Repeat("}", 10000),
		strings.Repeat(`[`, 10000) + strings.Repeat("]", 10000) + " 1",
	} {
		streamRows(g, data)
	}
	gn := &gen{r: r}
	for i := 0; i < 700; i++ {
		data := gn.text(gn.value(4))
		switch r.Intn(5) {
		case 0:
			data = gn.mutate(data)
		case 1:
			data += gn.pick("", " ", "x", "{}", "1", "]", ` {"a":"next"}`, "\n2", `"`)
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
		return []string{string(out), errText(err)}
	})...)...)
}

func (g *gen) randomNames(n *node) []string {
	var names []string
	for steps := 1 + g.r.Intn(3); steps > 0; steps-- {
		switch {
		case n.kind == 'o' && len(n.kids) > 0:
			i := g.r.Intn(len(n.kids))
			names = append(names, goKey(n.keys[i]))
			n = n.kids[i]
		case n.kind == 'a' && len(n.kids) > 0:
			n = n.kids[g.r.Intn(len(n.kids))]
		case n.kind == 's' && strings.HasPrefix(n.raw, `"`):
			names = append(names, goKey(n.raw))
			steps = 0
		default:
			steps = 0
		}
	}
	if len(names) == 0 || g.r.Intn(8) == 0 {
		names = append(names, g.pick("missing", "a", "\xef\xbf\xbd", "\xe2\x80\xa8"))
	}
	return names
}

func writeElement(dir string, r *rand.Rand) {
	g := create(dir, "element.txt", "jsonutil.FindElement: FIND ext names found start end err\njsonutil.DropElement: DROP ext names out err\n(names are encoded as jsonparser paths)")
	defer g.close()
	for _, c := range []struct {
		ext   string
		names []string
	}{
		{esc(`{"~u0061":1,"b":2}`), []string{"a"}},
		{esc(`{"b":2,"~u0061":1}`), []string{"a"}},
		{esc(`{"x":"~u0061","a":1}`), []string{"a"}},
		{esc(`{"~ud83d~ude00":{"b":1,"c":2}}`), []string{"\xf0\x9f\x98\x80", "c"}},
		{esc(`{"~ud800":1}`), []string{"\xef\xbf\xbd"}},
		{"{\"\xff\":1}", []string{"\xef\xbf\xbd"}},
		{`{"a":1 , "b":2}`, []string{"b"}},
		{`{"a":1 ,"b":2 , "c":3}`, []string{"b"}},
		{`{"a":{"b":1 , "c":2} , "d":3}`, []string{"a", "c"}},
		{`{"a":{"b":1 , "c":2} , "d":3}`, []string{"a", "b"}},
		{`{"a":[{"b":1},{"c":2}]}`, []string{"a", "c"}},
		{`{"a":[{"b":1},{"c":2}],"d":1}`, []string{"a", "b"}},
		{`{"a":"b","b":1}`, []string{"a", "b"}},
		{`{"a":{}}`, []string{"a", "b"}},
		{`{"a":{"b":{"c":1}},"x":1}`, []string{"a", "b", "c"}},
		{`{"a":{"b":{"c":1,"d":2}},"x":1}`, []string{"a", "b", "c"}},
		{`{"a":1}`, []string{"a", "b", "c"}},
	} {
		elementRows(g, c.ext, c.names)
	}
	gn := &gen{r: r}
	for i := 0; i < 1200; i++ {
		docNode := gn.object(3)
		ext := gn.text(docNode)
		if r.Intn(8) == 0 {
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

func writeMergeClone(dir string, r *rand.Rand) {
	g := create(dir, "mergeclone.txt", "jsonutil.MergeClone(&rawMessage, data): MC existing data result err (existing and result: nil or =bytes)")
	defer g.close()
	existing := []string{"", " ", "null", " null ", "1", `"x"`, "[]", "[1]", "{}", " {} ", `{"a":1}`, `{"a":{"b":1}}`,
		esc(`{"~u0061":1}`), "malformed", "{", `{"a":1}x`, "\xff", "nul"}
	incoming := []string{"", " ", "null", " null", "null ", "nul", "nulx", "n", "1", "1 ", "-", `"x"`, `"x`, "[]", "[1]", "[null]",
		"{}", " {} ", `{"a":null}`, `{"a":2}`, `{"b":{"c":null}}`, esc(`{"~u0061":3}`), `{"a":{"b":2}}`, `{"a":1}x`,
		`{"a":1} `, `{"a":`, "{", "malformed", "\xff", `{"a":1}{"b":2}`, "true", "tru"}
	for _, inc := range incoming {
		mergeCloneRow(g, nil, inc)
		for _, ex := range existing {
			ex := ex
			mergeCloneRow(g, &ex, inc)
		}
	}
	gn := &gen{r: r}
	for i := 0; i < 1000; i++ {
		docNode := gn.object(3)
		s := gn.text(docNode)
		if r.Intn(8) == 0 {
			s = gn.mutate(s)
		}
		data := gn.text(gn.derivePatch(docNode, 3))
		switch r.Intn(6) {
		case 0:
			data = gn.mutate(data)
		case 1:
			data = gn.text(gn.value(2))
		}
		mergeCloneRow(g, &s, data)
	}
}

func main() {
	out := flag.String("out", "toolkit_review", "output directory")
	flag.Parse()
	writeMergePatch(*out, rand.New(rand.NewSource(11)))
	writeJSONParser(*out, rand.New(rand.NewSource(12)))
	writeStream(*out, rand.New(rand.NewSource(13)))
	writeElement(*out, rand.New(rand.NewSource(14)))
	writeMergeClone(*out, rand.New(rand.NewSource(15)))
}
