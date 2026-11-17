// Command deepdrop writes the differential goldens for pbs-gojson's decoders replacing or
// discarding a value nested close to the 10000-level limit: a duplicate key, a field decoded
// twice, a slice that shrinks, `null` into a pointer, slice or map, and jsonutil.MergeClone
// resetting a slice or merging a map. tests/deepdrop.rs replays them on a 2 MiB thread
// (Tokio's default worker stack) in debug builds, where dropping such a value recursively
// overflows the stack; Go's garbage collector has no Rust counterpart.
//
// Output (every field strconv.QuoteToASCII-quoted, tab-separated, with D standing for the
// deep value in the inputs):
//
//	api  target  base  input  result  error
//
// Run it from the repository root:
//
//	go run ./rust/crates/gojson/testdata/go/deepdrop
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always runs with it.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// levels is how deep D, the deep value, is nested: the deepest document below is three
// levels deeper and stays within the limit.
const levels = 9990

// Deep holds the deep value in every kind of field that can discard one.
type Deep struct {
	M map[string]interface{} `json:"m,omitempty"`
	N map[string]interface{} `json:"n,omitempty"`
	O map[string]interface{} `json:"o,omitempty"`
	L []interface{}          `json:"l,omitempty"`
	P *interface{}           `json:"p,omitempty"`
	V interface{}            `json:"v,omitempty"`
}

// deep is D: {"a":{"a":...1}} nested levels deep.
func deep() string {
	return strings.Repeat(`{"a":`, levels) + "1" + strings.Repeat("}", levels)
}

// decodes are the documents decoded into a zero target; every result is shallow.
var decodes = []struct{ target, input string }{
	{"any", `{"a":D,"a":1}`},
	{"any", `{"x":{"a":D,"a":[D],"a":2},"x":{"b":D,"b":3}}`},
	{"map", `{"a":D,"a":1}`},
	{"map", `{"a":D,"b":1,"a":{"c":D},"a":null}`},
	{"struct", `{"m":{"a":D},"m":null}`},
	{"struct", `{"m":{"a":D,"b":1},"m":{"a":2}}`},
	{"struct", `{"n":{"a":D},"n":null}`},
	{"struct", `{"o":{"a":D},"o":null}`},
	{"struct", `{"l":[1,D],"l":[2]}`},
	{"struct", `{"l":[D],"l":[]}`},
	{"struct", `{"l":[D],"l":null}`},
	{"struct", `{"p":D,"p":null}`},
	{"struct", `{"p":D,"p":3}`},
	{"struct", `{"v":D,"v":null}`},
	{"struct", `{"v":D,"v":"s"}`},
	{"struct", `{"v":D,"v":true}`},
	{"struct", `{"v":D,"v":4}`},
	{"struct", `{"v":D,"v":[5]}`},
}

// merges are jsonutil.MergeClone overlays onto a base that holds D.
var merges = []struct{ base, input string }{
	{`{"m":{"a":D}}`, `{"m":null}`},
	{`{"m":{"a":D,"b":1}}`, `{"m":{"a":2}}`},
	{`{"n":{"a":D}}`, `{"n":null}`},
	{`{"o":{"a":D}}`, `{"o":null}`},
	{`{"l":[D]}`, `{"l":[1]}`},
	{`{"l":[D,D]}`, `{"l":null}`},
	{`{"l":[D]}`, `{"l":[]}`},
	{`{"p":D}`, `{"p":null}`},
	{`{"p":D}`, `{"p":5}`},
	{`{"v":D}`, `{"v":null}`},
	{`{"v":D}`, `{"v":[1]}`},
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func newTarget(target string) interface{} {
	switch target {
	case "any":
		var v interface{}
		return &v
	case "map":
		var m map[string]interface{}
		return &m
	default:
		return &Deep{}
	}
}

type golden struct {
	w    *bufio.Writer
	rows int
}

func (g *golden) row(fields ...string) {
	for i, f := range fields {
		if i > 0 {
			g.w.WriteString("\t")
		}
		g.w.WriteString(strconv.QuoteToASCII(f))
	}
	g.w.WriteString("\n")
	g.rows++
}

func main() {
	out := flag.String("out", "rust/crates/gojson/testdata/go/deepdrop", "output directory")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	f, err := os.Create(filepath.Join(*out, "deepdrop.txt"))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f)}
	g.w.WriteString("# api target base input result error, D nested " + strconv.Itoa(levels) + " levels deep\n")
	d := deep()
	for _, c := range decodes {
		input := []byte(strings.ReplaceAll(c.input, "D", d))

		v := newTarget(c.target)
		err := jsonutil.Unmarshal(input, v)
		res, _ := jsonutil.Marshal(v)
		g.row("jsonutil.Unmarshal", c.target, "", c.input, string(res), errText(err))

		v = newTarget(c.target)
		err = json.Unmarshal(input, v)
		res, _ = json.Marshal(v)
		g.row("json.Unmarshal", c.target, "", c.input, string(res), errText(err))
	}
	for _, c := range merges {
		base := &Deep{}
		if err := jsonutil.Unmarshal([]byte(strings.ReplaceAll(c.base, "D", d)), base); err != nil {
			log.Fatal(err)
		}
		err := jsonutil.MergeClone(base, []byte(c.input))
		res, _ := jsonutil.Marshal(base)
		g.row("jsonutil.MergeClone", "struct", c.base, c.input, string(res), errText(err))
	}
	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	log.Printf("wrote %d rows", g.rows)
}
