// Command mergeclone writes the differential goldens for pbs-gojson's jsonutil::merge_clone:
// util/jsonutil.MergeClone into real openrtb2 values, the way Prebid Server calls it
// (exchange/utils.go mergeImpFPD on an openrtb2.Imp, firstpartydata on openrtb2.Site, App,
// User and Device, plus whole openrtb2.BidRequests), and into a test struct with every field
// shape the merge-clone decoders treat specially. tests/mergeclone.rs replays them.
//
// The existing values are openrtb2 objects collected from the repository's JSON corpora
// (endpoints/openrtb2/sample-requests, exchange/exchangetest, firstpartydata/tests), decoded
// with jsonutil.Unmarshal; some get a nil or malformed Ext. The overlays are the first-party
// data objects of those corpora (bidder config FPD and imp.ext.prebid.imp) and seeded random
// overlays built from corpus objects: copied, nulled, nested, mistyped, re-cased and
// malformed members, and ext objects with nulls to merge. Each case records the MergeClone
// error and jsonutil.Marshal of the value afterwards (also after an error).
//
// Output (every field strconv.QuoteToASCII-quoted, tab-separated):
//
//	C  type  index  existing JSON                       a corpus value
//	M  type  index  ext  overlay  result  error         a MergeClone case
//
// where ext is "-" (keep the decoded Ext), "nil", or "=" followed by the bytes Ext is set to.
//
// Run it from the repository root:
//
//	go run ./rust/crates/gojson/testdata/go/mergeclone
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io/fs"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always runs with it (it
	// only adds an encoder; MergeClone's decoders are unaffected).
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// Base is embedded through a pointer in Shapes.
type Base struct {
	B  string          `json:"b,omitempty"`
	BX json.RawMessage `json:"bx,omitempty"`
}

// Inner is a small struct reached through slices, maps and pointers.
type Inner struct {
	ID   string          `json:"id,omitempty"`
	Tags []string        `json:"tags,omitempty"`
	N    *int            `json:"n,omitempty"`
	Ext  json.RawMessage `json:"ext,omitempty"`
}

// Shapes has a field of every shape the merge-clone decoders treat differently.
type Shapes struct {
	*Base
	Strs     []string                   `json:"strs"`
	Imps     []*openrtb2.Imp            `json:"imps"`
	Inners   []Inner                    `json:"inners"`
	Nested   [][]int                    `json:"nested"`
	PtrSlice *[]string                  `json:"ptrslice"`
	Bytes    []byte                     `json:"bytes"`
	MapInt   map[string]int             `json:"mapint"`
	MapNum   map[int]string             `json:"mapnum"`
	MapInner map[string]*Inner          `json:"mapinner"`
	MapRaw   map[string]json.RawMessage `json:"mapraw"`
	MapSlice map[string][]string        `json:"mapslice"`
	Raws     []json.RawMessage          `json:"raws"`
	RawPtr   *json.RawMessage           `json:"rawptr"`
	Raw      json.RawMessage            `json:"raw"`
	Any      interface{}                `json:"any"`
	AnyMap   map[string]interface{}     `json:"anymap"`
	Int8Ptr  *int8                      `json:"int8ptr"`
	StrNum   int                        `json:"strnum,string"`
	Num      jsonutil.StringInt         `json:"num"`
	Inner    *Inner                     `json:"inner"`
	InnerVal Inner                      `json:"innerval"`
}

// target is one MergeClone target type.
type target struct {
	name  string
	typ   reflect.Type
	pool  []map[string]interface{} // corpus objects (decoded as interface{})
	raws  []string                 // corpus objects as JSON, in pool order
	fpd   []string                 // real first-party data overlays
	extOK bool                     // the type has an Ext json.RawMessage field
}

func newValue(t *target) interface{} { return reflect.New(t.typ).Interface() }

func setExt(v interface{}, ext json.RawMessage) {
	reflect.ValueOf(v).Elem().FieldByName("Ext").SetBytes(ext)
}

// ---- golden output -------------------------------------------------------------------------

type golden struct {
	w     *bufio.Writer
	f     *os.File
	cases int
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

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// ---- corpus ------------------------------------------------------------------------------

// collect walks every JSON file under the roots and calls visit for each object member.
func collect(roots []string, visit func(path []string, v interface{})) {
	for _, root := range roots {
		var files []string
		err := filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if !d.IsDir() && strings.HasSuffix(p, ".json") {
				files = append(files, p)
			}
			return nil
		})
		if err != nil {
			log.Fatal(err)
		}
		sort.Strings(files)
		for _, f := range files {
			data, err := os.ReadFile(f)
			if err != nil {
				log.Fatal(err)
			}
			var doc interface{}
			if json.Unmarshal(data, &doc) != nil {
				continue
			}
			walk(nil, doc, visit)
		}
	}
}

func walk(path []string, v interface{}, visit func([]string, interface{})) {
	visit(path, v)
	switch x := v.(type) {
	case map[string]interface{}:
		for _, k := range sortedKeys(x) {
			walk(append(append([]string(nil), path...), k), x[k], visit)
		}
	case []interface{}:
		for _, e := range x {
			walk(append(append([]string(nil), path...), "[]"), e, visit)
		}
	}
}

func sortedKeys(m map[string]interface{}) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func last(path []string, n int) []string {
	if len(path) < n {
		return nil
	}
	return path[len(path)-n:]
}

func marshal(v interface{}) string {
	out, err := json.Marshal(v)
	if err != nil {
		log.Fatal(err)
	}
	return string(out)
}

// usable reports whether a corpus object decodes into the target and round-trips without
// invalid UTF-8 (which Rust's String fields cannot hold).
func usable(t *target, data string) bool {
	v := newValue(t)
	if jsonutil.Unmarshal([]byte(data), v) != nil {
		return false
	}
	out, err := jsonutil.Marshal(v)
	return err == nil && !bytes.Contains(out, []byte(`�`)) && len(data) <= 1500
}

func addUnique(list []string, seen map[string]bool, s string) []string {
	if seen[s] {
		return list
	}
	seen[s] = true
	return append(list, s)
}

// ---- random overlays -----------------------------------------------------------------------

type gen struct {
	r *rand.Rand
}

func (g *gen) chance(p float64) bool { return g.r.Float64() < p }

// scalar is a random JSON value of a random type.
func (g *gen) scalar() interface{} {
	switch g.r.Intn(7) {
	case 0:
		return "x" + strconv.Itoa(g.r.Intn(9))
	case 1:
		return float64(g.r.Intn(2000) - 500)
	case 2:
		return g.r.Intn(2) == 0
	case 3:
		return []interface{}{float64(g.r.Intn(9)), "s"}
	case 4:
		return map[string]interface{}{"k": float64(g.r.Intn(9))}
	case 5:
		return 1.5
	default:
		return nil
	}
}

// extObject is a random object to merge into an ext: nested objects, nulls, arrays.
func (g *gen) extObject(depth int) map[string]interface{} {
	out := map[string]interface{}{}
	for i := g.r.Intn(4); i >= 0; i-- {
		key := []string{"a", "b", "c", "prebid", "data", "gpid", "z"}[g.r.Intn(7)]
		switch {
		case depth < 2 && g.chance(0.35):
			out[key] = g.extObject(depth + 1)
		case g.chance(0.3):
			out[key] = nil
		default:
			out[key] = g.scalar()
		}
	}
	return out
}

// mutate builds an overlay from a corpus object: a random subset of its members, each kept,
// nulled, replaced with another corpus object's member, recursed into, mistyped or re-cased.
func (g *gen) mutate(src map[string]interface{}, pool []map[string]interface{}, depth int) map[string]interface{} {
	keys := sortedKeys(src)
	out := map[string]interface{}{}
	if len(keys) == 0 {
		return out
	}
	for n := 1 + g.r.Intn(4); n > 0; n-- {
		key := keys[g.r.Intn(len(keys))]
		val := src[key]
		outKey := key
		if g.chance(0.06) {
			outKey = strings.ToUpper(key)
		}
		switch roll := g.r.Float64(); {
		case roll < 0.35:
			out[outKey] = val
		case roll < 0.45:
			out[outKey] = nil
		case roll < 0.6:
			out[outKey] = val
			if len(pool) > 0 {
				if ov, ok := pool[g.r.Intn(len(pool))][key]; ok {
					out[outKey] = ov
				}
			}
		case roll < 0.75:
			switch x := val.(type) {
			case map[string]interface{}:
				if depth < 3 {
					out[outKey] = g.mutate(x, nil, depth+1)
				} else {
					out[outKey] = x
				}
			case []interface{}:
				out[outKey] = g.mutateArray(x, depth)
			default:
				out[outKey] = val
			}
		case roll < 0.9:
			if key == "ext" || g.chance(0.3) {
				out[outKey] = g.extObject(0)
			} else {
				out[outKey] = g.scalar()
			}
		default:
			out[outKey] = g.scalar()
		}
	}
	if pool != nil && g.chance(0.25) {
		out["ext"] = g.extObject(0)
	}
	return out
}

// mutateArray keeps a random subsequence of the elements, mutating objects among them.
func (g *gen) mutateArray(src []interface{}, depth int) []interface{} {
	out := []interface{}{}
	for _, e := range src {
		if g.chance(0.3) {
			continue
		}
		if m, ok := e.(map[string]interface{}); ok && depth < 3 && g.chance(0.5) {
			out = append(out, g.mutate(m, nil, depth+1))
			continue
		}
		out = append(out, e)
	}
	if g.chance(0.2) && len(src) > 0 {
		out = append(out, src[g.r.Intn(len(src))])
	}
	return out
}

// malform breaks a valid overlay the ways jsoniter reports differently.
func (g *gen) malform(s string) string {
	switch g.r.Intn(8) {
	case 0:
		return s[:g.r.Intn(len(s))]
	case 1:
		if i := strings.Index(s, "null"); i >= 0 {
			return s[:i] + "nul" + s[i+4:]
		}
		return s + "x"
	case 2:
		i := g.r.Intn(len(s))
		return s[:i] + "x" + s[i+1:]
	case 3:
		return s + " x"
	case 4:
		return "[" + s + "]"
	case 5:
		return ""
	case 6:
		i := g.r.Intn(len(s))
		return s[:i] + "," + s[i:]
	default:
		return strings.Replace(s, ":", " ", 1)
	}
}

// ---- cases -------------------------------------------------------------------------------

func runCase(g *golden, t *target, index int, ext *string, overlay string) {
	v := newValue(t)
	if err := jsonutil.Unmarshal([]byte(t.raws[index]), v); err != nil {
		log.Fatalf("%s corpus %d: %v", t.name, index, err)
	}
	extField := "-"
	if ext != nil {
		if *ext == "\x00nil" {
			setExt(v, nil)
			extField = "nil"
		} else {
			setExt(v, json.RawMessage(*ext))
			extField = "=" + *ext
		}
	}
	err := jsonutil.MergeClone(v, json.RawMessage(overlay))
	result, merr := jsonutil.Marshal(v)
	res := string(result)
	if merr != nil {
		res = "MARSHAL: " + merr.Error()
	}
	g.row("M", t.name, strconv.Itoa(index), extField, overlay, res, errText(err))
}

func main() {
	out := flag.String("out", "rust/crates/gojson/testdata/go/mergeclone", "output directory")
	flag.Parse()

	targets := []*target{
		{name: "openrtb2.Imp", typ: reflect.TypeOf(openrtb2.Imp{}), extOK: true},
		{name: "openrtb2.Site", typ: reflect.TypeOf(openrtb2.Site{}), extOK: true},
		{name: "openrtb2.App", typ: reflect.TypeOf(openrtb2.App{}), extOK: true},
		{name: "openrtb2.User", typ: reflect.TypeOf(openrtb2.User{}), extOK: true},
		{name: "openrtb2.Device", typ: reflect.TypeOf(openrtb2.Device{}), extOK: true},
		{name: "openrtb2.BidRequest", typ: reflect.TypeOf(openrtb2.BidRequest{}), extOK: true},
	}
	byKey := map[string]*target{
		"imp": targets[0], "site": targets[1], "app": targets[2], "user": targets[3],
		"device": targets[4],
	}
	seen := map[*target]map[string]bool{}
	fpdSeen := map[*target]map[string]bool{}
	for _, t := range targets {
		seen[t] = map[string]bool{}
		fpdSeen[t] = map[string]bool{}
	}
	roots := []string{"endpoints/openrtb2/sample-requests", "exchange/exchangetest", "firstpartydata/tests"}
	collect(roots, func(path []string, v interface{}) {
		obj, ok := v.(map[string]interface{})
		if !ok || len(path) == 0 {
			return
		}
		data := marshal(obj)
		key := path[len(path)-1]
		if key == "[]" && len(path) >= 2 && path[len(path)-2] == "imp" {
			key = "imp"
		}
		// First-party data overlays: bidderConfigFPD.<bidder>.<site|app|user|device> and
		// imp.ext.prebid.imp.<bidder>.
		if p := last(path, 3); p != nil && p[0] == "bidderConfigFPD" {
			if t := byKey[p[2]]; t != nil {
				t.fpd = addUnique(t.fpd, fpdSeen[t], data)
			}
			return
		}
		if p := last(path, 3); p != nil && p[0] == "prebid" && p[1] == "imp" {
			targets[0].fpd = addUnique(targets[0].fpd, fpdSeen[targets[0]], data)
			return
		}
		if t := byKey[key]; t != nil && usable(t, data) && !seen[t][data] {
			seen[t][data] = true
			t.pool = append(t.pool, obj)
			t.raws = append(t.raws, data)
		}
		if _, hasImp := obj["imp"]; hasImp && (key == "ortbRequest" || key == "mockBidRequest") {
			t := targets[5]
			if usable(t, data) && !seen[t][data] {
				seen[t][data] = true
				t.pool = append(t.pool, obj)
				t.raws = append(t.raws, data)
			}
		}
	})

	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	f, err := os.Create(filepath.Join(*out, "mergeclone.txt"))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f), f: f}
	g.write("# jsonutil.MergeClone(v, overlay) for openrtb2 targets and main.Shapes, then jsonutil.Marshal(v)\n")
	g.write("# C type index existing | M type index ext(-|nil|=bytes) overlay result error\n")

	rng := &gen{r: rand.New(rand.NewSource(20261117))}
	badExts := []string{"malformed", "null", "{", "[1]", `"x"`, "1", "", `{"a":1}x`}
	nilExt := "\x00nil"
	for _, t := range targets {
		// Keep the corpus to a bounded, deterministic sample.
		if len(t.raws) > 90 {
			idx := rng.r.Perm(len(t.raws))[:90]
			sort.Ints(idx)
			pool, raws := make([]map[string]interface{}, 0, 90), make([]string, 0, 90)
			for _, i := range idx {
				pool = append(pool, t.pool[i])
				raws = append(raws, t.raws[i])
			}
			t.pool, t.raws = pool, raws
		}
		for i, raw := range t.raws {
			g.row("C", t.name, strconv.Itoa(i), raw)
		}
		fmt.Printf("%s: %d corpus objects, %d fpd overlays\n", t.name, len(t.raws), len(t.fpd))
		// Every real first-party data overlay over a few existing values.
		for _, fpd := range t.fpd {
			for n := 0; n < 3; n++ {
				runCase(g, t, rng.r.Intn(len(t.raws)), nil, fpd)
			}
		}
		// Whole corpus objects over each other, and random overlays.
		for n := 0; n < 380; n++ {
			index := rng.r.Intn(len(t.raws))
			var ext *string
			switch roll := rng.r.Float64(); {
			case roll < 0.08:
				bad := badExts[rng.r.Intn(len(badExts))]
				ext = &bad
			case roll < 0.16:
				ext = &nilExt
			}
			var overlay string
			switch roll := rng.r.Float64(); {
			case roll < 0.12:
				overlay = t.raws[rng.r.Intn(len(t.raws))]
			case roll < 0.9:
				overlay = marshal(rng.mutate(t.pool[rng.r.Intn(len(t.pool))], t.pool, 0))
			default:
				overlay = rng.malform(marshal(rng.mutate(t.pool[rng.r.Intn(len(t.pool))], t.pool, 0)))
			}
			runCase(g, t, index, ext, overlay)
		}
	}
	writeShapes(g, rng)

	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("mergeclone.txt: %d rows\n", g.cases)
}

// ---- main.Shapes ---------------------------------------------------------------------------

// shapesCorpus is the existing main.Shapes values, as JSON.
var shapesCorpus = []string{
	`{}`,
	`{"strs":["a","b","c"],"imps":[{"id":"1","tagid":"t"},null],"inners":[{"id":"x","tags":["p"],"ext":{"a":1}}],` +
		`"nested":[[1,2],[3]],"ptrslice":["q"],"bytes":"AQID","mapint":{"a":1,"b":2},"mapnum":{"1":"a","2":"b"},` +
		`"mapinner":{"k":{"id":"m","n":3}},"mapraw":{"r":{"z":1},"s":[1]},"mapslice":{"l":["1","2"]},` +
		`"raws":[{"a":1},[2]],"rawptr":{"p":1},"raw":{"a":1,"b":{"c":2}},"any":{"x":[1,{"y":null}]},` +
		`"anymap":{"a":1,"b":"s"},"int8ptr":7,"strnum":"12","num":5,"inner":{"id":"i","tags":["t1","t2"],"ext":{"e":1}},` +
		`"innerval":{"id":"v","n":1},"b":"base","bx":{"q":1}}`,
	`{"strs":[],"imps":[],"inners":[],"nested":[],"ptrslice":[],"bytes":"","mapint":{},"mapnum":{},"mapinner":{},` +
		`"mapraw":{},"mapslice":{},"raws":[],"raw":[],"any":[],"anymap":{},"inner":{},"innerval":{}}`,
	`{"strs":["only"],"inners":[{"id":"a"},{"id":"b","tags":["1","2","3"]},{"ext":{"k":[1,2]}}],"raw":"str","rawptr":null,` +
		`"bx":{"deep":{"er":{"est":1}}},"b":"x","any":"s","int8ptr":-3,"strnum":"-4"}`,
	`{"imps":[{"id":"i1","banner":{"w":300,"h":250,"format":[{"w":300,"h":250}]},"ext":{"prebid":{"bidder":{"a":{}}}}},` +
		`{"id":"i2","video":{"mimes":["video/mp4"],"w":640}}],"raw":{"prebid":{"a":1}},"mapraw":{"x":null}}`,
}

func writeShapes(g *golden, rng *gen) {
	t := &target{name: "main.Shapes", typ: reflect.TypeOf(Shapes{})}
	for _, raw := range shapesCorpus {
		var obj map[string]interface{}
		if err := json.Unmarshal([]byte(raw), &obj); err != nil {
			log.Fatal(err)
		}
		if !usable(t, raw) {
			log.Fatalf("shapes corpus does not decode: %s", raw)
		}
		t.pool = append(t.pool, obj)
		t.raws = append(t.raws, raw)
	}
	for i, raw := range t.raws {
		g.row("C", t.name, strconv.Itoa(i), raw)
	}
	// Hand-picked cases: every field set to null, emptied, malformed and replaced.
	values := map[string][]string{
		"strs":     {`null`, `[]`, `["z"]`, `["x","y","z","w"]`, `nul`, `"s"`, `[1]`, `{}`},
		"imps":     {`null`, `[]`, `[{"tagid":"2"}]`, `[null,{"id":"9"}]`, `[{"id":1}]`, `nul`, `{}`},
		"inners":   {`null`, `[{"tags":["n"]}]`, `[{},{},{},{}]`, `[{"id":"z","ext":{"b":2}}]`, `[1]`},
		"nested":   {`null`, `[[9]]`, `[[],[1,2,3],null]`, `[["x"]]`},
		"ptrslice": {`null`, `[]`, `["a","b"]`, `nul`, `[1]`},
		"bytes":    {`null`, `"BAUG"`, `[1,2]`, `"!!"`, `nul`, `[300]`},
		"mapint":   {`null`, `{}`, `{"c":3}`, `{"a":null}`, `{"a":"x"}`, `nul`, `[]`},
		"mapnum":   {`null`, `{"3":"c"}`, `{"x":"c"}`, `{"1":null}`},
		"mapinner": {`null`, `{"k":{"tags":["z"]}}`, `{"k":null,"j":{"id":"j"}}`, `{"k":1}`},
		"mapraw":   {`null`, `{"r":{"y":2}}`, `{"r":null,"t":[3]}`, `{"s":{"a":1}}`},
		"mapslice": {`null`, `{"l":["9"]}`, `{"l":null}`, `{"m":[]}`},
		"raws":     {`null`, `[]`, `[{"b":2}]`, `[null,{"c":3}]`, `[1,"x",{"a":null}]`},
		"rawptr":   {`null`, `{"q":2}`, `{"p":null}`, `[1]`, `"s"`, `nul`},
		"raw":      {`null`, `{}`, `{"a":null}`, `{"b":{"d":3}}`, `[1]`, `"s"`, `1`, `{"a":{"x":null}}`, `malformed`},
		"any":      {`null`, `{"x":1}`, `[1]`, `"s"`, `1.5`, `nul`},
		"anymap":   {`null`, `{"c":true}`, `{"a":null}`, `[]`},
		"int8ptr":  {`null`, `1`, `300`, `"1"`, `nul`},
		"strnum":   {`null`, `"5"`, `5`, `"x"`, `""`},
		"num":      {`null`, `"7"`, `7`, `"x"`, `true`},
		"inner":    {`null`, `{}`, `{"tags":["z"]}`, `{"ext":{"e":null,"f":2}}`, `{"ext":null}`, `1`},
		"innerval": {`null`, `{"tags":["z"]}`, `{"n":null}`, `{"id":5}`},
		"b":        {`null`, `"nb"`, `1`},
		"bx":       {`null`, `{"q":null,"r":1}`, `[1]`},
	}
	for _, key := range sortedKeysS(values) {
		for _, val := range values[key] {
			for index := range t.raws {
				runCase(g, t, index, nil, `{"`+key+`":`+val+`}`)
			}
		}
	}
	// Random overlays over the corpus.
	for n := 0; n < 400; n++ {
		index := rng.r.Intn(len(t.raws))
		src := t.pool[rng.r.Intn(len(t.pool))]
		if len(src) == 0 {
			src = t.pool[1]
		}
		overlay := marshal(rng.mutate(src, t.pool, 0))
		if rng.chance(0.08) {
			overlay = rng.malform(overlay)
		}
		runCase(g, t, index, nil, overlay)
	}
}

func sortedKeysS(m map[string][]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
