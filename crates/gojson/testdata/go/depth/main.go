// Command depth writes the differential goldens for pbs-gojson at and past the decoders'
// nesting limit of 10000: every decode, merge, compare and search entry point Prebid Server
// uses, over arrays, objects and a mix of both nested 9999, 10000 and 10001 levels deep.
// tests/depth.rs replays them on a 2 MiB thread (Tokio's default worker stack) in debug
// builds, where Go's growing goroutine stacks have no Rust counterpart.
//
// Output (every field strconv.QuoteToASCII-quoted, tab-separated):
//
//	api  shape  levels  result  error
//
// where result is "=" when the output equals the input document and the output otherwise
// (re-encoded values, merged documents, booleans and FindElement's indices).
//
// Run it from the repository root:
//
//	go run ./rust/crates/gojson/testdata/go/depth
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	jsonpatch "gopkg.in/evanphx/json-patch.v5"
)

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always runs with it.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// Wrapped is a struct whose fields hold the deep value one level down.
type Wrapped struct {
	Ext json.RawMessage `json:"ext,omitempty"`
	Any interface{}     `json:"any,omitempty"`
}

// nested is levels levels of the shape: "array" ([[...]]), "object" ({"a":{"a":...1}}) or
// "mixed" ({"a":[{"a":[...]}]}, starting with an object).
func nested(shape string, levels int) string {
	var open, close strings.Builder
	for i := 0; i < levels; i++ {
		if shape == "array" || (shape == "mixed" && i%2 == 1) {
			open.WriteString("[")
			close.WriteString("]")
		} else {
			open.WriteString(`{"a":`)
			close.WriteString("}")
		}
	}
	inner := ""
	if shape == "object" || (shape == "mixed" && levels%2 == 1) {
		inner = "1"
	}
	closing := []byte(close.String())
	for i, j := 0, len(closing)-1; i < j; i, j = i+1, j-1 {
		closing[i], closing[j] = closing[j], closing[i]
	}
	return open.String() + inner + string(closing)
}

type golden struct {
	w *bufio.Writer
}

func (g *golden) row(fields ...string) {
	for i, f := range fields {
		if i > 0 {
			g.w.WriteString("\t")
		}
		g.w.WriteString(strconv.QuoteToASCII(f))
	}
	g.w.WriteString("\n")
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// result is "=" when out is the input document, out otherwise.
func result(input string, out []byte) string {
	if string(out) == input {
		return "="
	}
	return string(out)
}

func main() {
	out := flag.String("out", "rust/crates/gojson/testdata/go/depth", "output directory")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	f, err := os.Create(filepath.Join(*out, "depth.txt"))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f)}
	g.w.WriteString("# api shape levels result(=: the input) error, at and past the 10000-level limit\n")
	rows := 0
	iterAPI := jsoniter.ConfigCompatibleWithStandardLibrary
	for _, shape := range []string{"array", "object", "mixed"} {
		for _, levels := range []int{9999, 10000, 10001} {
			doc := nested(shape, levels)
			inner := nested(shape, levels-1)
			emit := func(api, res string, err error) {
				g.row(api, shape, strconv.Itoa(levels), res, errText(err))
				rows++
			}

			var a interface{}
			err := jsonutil.Unmarshal([]byte(doc), &a)
			b, _ := jsonutil.Marshal(a)
			emit("jsonutil.Unmarshal", result(doc, b), err)

			var m map[string]interface{}
			err = jsonutil.Unmarshal([]byte(doc), &m)
			b, _ = jsonutil.Marshal(m)
			emit("jsonutil.Unmarshal map", result(doc, b), err)

			var any Wrapped
			anyDoc := `{"any":` + inner + `}`
			err = jsonutil.Unmarshal([]byte(anyDoc), &any)
			b, _ = jsonutil.Marshal(any)
			emit("jsonutil.Unmarshal any field", result(anyDoc, b), err)

			var ext Wrapped
			extDoc := `{"ext":` + inner + `}`
			err = jsonutil.Unmarshal([]byte(extDoc), &ext)
			b, _ = jsonutil.Marshal(ext)
			emit("jsonutil.Unmarshal ext field", result(extDoc, b), err)

			var it interface{}
			err = iterAPI.Unmarshal([]byte(doc), &it)
			b, _ = jsonutil.Marshal(it)
			emit("jsoniter.Unmarshal", result(doc, b), err)

			var s interface{}
			err = json.Unmarshal([]byte(doc), &s)
			b, _ = json.Marshal(s)
			emit("json.Unmarshal", result(doc, b), err)

			var d interface{}
			err = json.NewDecoder(strings.NewReader(doc)).Decode(&d)
			b, _ = json.Marshal(d)
			emit("json.Decoder.Decode", result(doc, b), err)

			merged := Wrapped{Ext: json.RawMessage(inner)}
			err = jsonutil.MergeClone(&merged, []byte(extDoc))
			b, _ = jsonutil.Marshal(merged)
			emit("jsonutil.MergeClone", result(extDoc, b), err)

			b, err = jsonpatch.MergePatch([]byte(doc), []byte(doc))
			emit("jsonpatch.MergePatch", result(doc, b), err)

			emit("jsonpatch.Equal", strconv.FormatBool(jsonpatch.Equal([]byte(doc), []byte(doc))), nil)

			found, start, end, err := jsonutil.FindElement([]byte(doc), "a")
			emit("jsonutil.FindElement", fmt.Sprintf("%t %d %d", found, start, end), err)

			b, err = jsonutil.DropElement([]byte(doc), "a")
			emit("jsonutil.DropElement", result(doc, b), err)

			emit("json.Valid", strconv.FormatBool(json.Valid([]byte(doc))), nil)

			var compact bytes.Buffer
			spaced := strings.ReplaceAll(doc, ":", ": ")
			err = json.Compact(&compact, []byte(spaced))
			emit("json.Compact", result(doc, compact.Bytes()), err)

			b, err = jsonutil.Marshal(json.RawMessage(spaced))
			emit("jsonutil.Marshal raw", result(doc, b), err)
		}
	}
	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("depth.txt: %d rows\n", rows)
}
