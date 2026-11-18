// Command mergeclone_review writes adversarial differential goldens for pbs-gojson's
// jsonutil::merge_clone: util/jsonutil.MergeClone into a struct with the field shapes the first
// generator (../mergeclone) leaves out (named slice and map types, nested maps of raw messages
// as in config.AccountModules, pointers to raw messages and maps, [][]byte, **T, ,string on
// pointers and slices, json.Number, embedded structs by value and by pointer, a recursive
// type, interface{} slices and maps) and into openrtb2.Imp, with programmatically generated
// existing values and overlays: duplicate and re-cased keys, whitespace everywhere, malformed
// and truncated values, numbers jsoniter's skip accepts but encoding/json rejects, and
// existing raw messages of every kind. Some existing values are themselves MergeClone results
// (account/fetcher.go merges the defaults and then the account over a zero value).
//
// Output (every field strconv.QuoteToASCII-quoted, tab-separated):
//
//	R  target  mode  existing  raw  overlay  result  error
//
// where mode is "U" (existing decoded with jsonutil.Unmarshal) or "M" (existing merged with
// jsonutil.MergeClone into a zero value), raw is "-" (keep), "nil" or "=" followed by the
// bytes the target's raw message is set to afterwards, and result is jsonutil.Marshal of the
// value after MergeClone (also after an error).
//
// Run it from the directory of this module (crates/gojson/testdata/go):
//
//	go run ./mergeclone_review
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always runs with it.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// Strs is a named slice type.
type Strs []string

// RawMap is a named map of raw messages.
type RawMap map[string]json.RawMessage

// Modules has the shape of config.AccountModules.
type Modules map[string]map[string]json.RawMessage

// Leaf is a small struct reached through slices, maps and pointers.
type Leaf struct {
	ID  string          `json:"id,omitempty"`
	N   *int            `json:"n,omitempty"`
	Ext json.RawMessage `json:"ext,omitempty"`
}

// Emb is embedded by value.
type Emb struct {
	E  string          `json:"e,omitempty"`
	EX json.RawMessage `json:"ex,omitempty"`
	ES []string        `json:"es,omitempty"`
}

// EmbPtr is embedded through a pointer.
type EmbPtr struct {
	P  *Leaf `json:"p,omitempty"`
	PS []int `json:"ps,omitempty"`
}

// Node is a recursive type.
type Node struct {
	ID   string          `json:"id,omitempty"`
	Kids []*Node         `json:"kids,omitempty"`
	Ext  json.RawMessage `json:"ext,omitempty"`
}

// Review has the field shapes ../mergeclone's main.Shapes leaves out.
type Review struct {
	Emb
	*EmbPtr
	Named      Strs                         `json:"named"`
	NamedPtr   *Strs                        `json:"namedptr"`
	RawMap     RawMap                       `json:"rawmap"`
	Modules    Modules                      `json:"modules"`
	RawPtrs    []*json.RawMessage           `json:"rawptrs"`
	MapRawPtr  map[string]*json.RawMessage  `json:"maprawptr"`
	PtrRaws    *[]json.RawMessage           `json:"ptrraws"`
	RawRaws    [][]json.RawMessage          `json:"rawraws"`
	PtrMap     *map[string]int              `json:"ptrmap"`
	MapMap     map[string]map[string]int    `json:"mapmap"`
	SliceMap   []map[string]int             `json:"slicemap"`
	Bytess     [][]byte                     `json:"bytess"`
	PtrPtr     **Leaf                       `json:"ptrptr"`
	StrNumPtr  *int                         `json:"strnumptr,string"`
	StrSlice   []int                        `json:"strslice,string"`
	Number     json.Number                  `json:"number"`
	NumPtr     *json.Number                 `json:"numptr"`
	Leafs      []Leaf                       `json:"leafs"`
	LeafMap    map[string]Leaf              `json:"leafmap"`
	LeafPtrMap map[string]*Leaf             `json:"leafptrmap"`
	Any        interface{}                  `json:"any"`
	Anys       []interface{}                `json:"anys"`
	AnyMap     map[string]interface{}       `json:"anymap"`
	Flag       bool                         `json:"flag"`
	F          float64                      `json:"f"`
	U8         uint8                        `json:"u8"`
	Kids       []*Node                      `json:"kids"`
	Leaf       *Leaf                        `json:"leaf"`
	LeafVal    Leaf                         `json:"leafval"`
	SIntPtr    *jsonutil.StringInt          `json:"sintptr"`
	Raw        json.RawMessage              `json:"raw"`
	RawOmit    json.RawMessage              `json:"rawomit,omitempty"`
	MapRawRaw  map[string][]json.RawMessage `json:"maprawraw"`
}

// ---- golden output -------------------------------------------------------------------------

type golden struct {
	w     *bufio.Writer
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

// ---- generated JSON --------------------------------------------------------------------------

type gen struct {
	r *rand.Rand
}

func (g *gen) chance(p float64) bool { return g.r.Float64() < p }

func (g *gen) pick(items ...string) string { return items[g.r.Intn(len(items))] }

// ws is random insignificant whitespace.
func (g *gen) ws() string {
	if g.chance(0.75) {
		return ""
	}
	return g.pick(" ", "\n", "\t ", "\r\n  ")
}

func (g *gen) str() string {
	return g.pick(`"x"`, `""`, `"a\"b"`, `"été"`, `"<&>"`, `"tab\t"`, `"😀"`, `"y"`)
}

func (g *gen) num() string {
	return g.pick(`1`, `-2`, `0`, `1.5`, `1e3`, `300`, `-129`, `7`, `12`, `2E-2`)
}

// weird is a value jsoniter's Skip accepts or rejects in ways encoding/json does not.
func (g *gen) weird() string {
	return g.pick(`01`, `-`, `1.`, `.5`, `tru`, `nul`, `[1,]`, `{"a":}`, `{"a" 1}`, `"\x"`, `[`,
		`{`, `1.5.5`, `-0`, `00`, `1e`, `"\u12"`, `{"a":1,}`, `[1 2]`, `NaN`, `}`, `]`, `,`,
		`{"a":1}}`, `"unterminated`, `{"":1}`, `{"a":1,"a":2}`)
}

// value is a random JSON value nested up to depth more levels.
func (g *gen) value(depth int) string {
	roll := g.r.Intn(10)
	if depth <= 0 && roll >= 6 {
		roll = g.r.Intn(6)
	}
	switch roll {
	case 0:
		return g.str()
	case 1, 2:
		return g.num()
	case 3:
		return g.pick("true", "false")
	case 4:
		return "null"
	case 5:
		return g.pick(`{}`, `[]`)
	case 6, 7:
		return g.object(depth-1, g.value)
	default:
		return g.array(depth-1, g.value)
	}
}

// rawValue is a raw message: usually valid, sometimes weird.
func (g *gen) rawValue(depth int) string {
	if g.chance(0.12) {
		return g.weird()
	}
	if g.chance(0.5) {
		return g.object(depth, g.rawValue)
	}
	return g.value(depth)
}

// key is an object key, sometimes re-cased, escaped or repeated.
func (g *gen) key(keys ...string) string {
	k := keys[g.r.Intn(len(keys))]
	switch roll := g.r.Float64(); {
	case roll < 0.06:
		k = strings.ToUpper(k)
	case roll < 0.09 && len(k) > 0:
		k = `\u00` + strconv.FormatInt(int64(k[0]), 16) + k[1:]
	}
	return `"` + k + `"`
}

// object is an object whose members' values come from elem.
func (g *gen) object(depth int, elem func(int) string, keys ...string) string {
	if len(keys) == 0 {
		keys = []string{"a", "b", "c", "prebid", "z"}
	}
	n := g.r.Intn(4)
	parts := make([]string, 0, n)
	for i := 0; i < n; i++ {
		parts = append(parts, g.ws()+g.key(keys...)+g.ws()+":"+g.ws()+elem(depth)+g.ws())
	}
	return "{" + strings.Join(parts, ",") + g.ws() + "}"
}

func (g *gen) array(depth int, elem func(int) string) string {
	n := g.r.Intn(4)
	parts := make([]string, 0, n)
	for i := 0; i < n; i++ {
		parts = append(parts, g.ws()+elem(depth)+g.ws())
	}
	return "[" + strings.Join(parts, ",") + g.ws() + "]"
}

// orNull returns null, a malformed null, a wrong-typed value or v.
func (g *gen) orNull(v func() string) string {
	switch roll := g.r.Float64(); {
	case roll < 0.12:
		return "null"
	case roll < 0.15:
		return g.pick("nul", "nULL", "n")
	case roll < 0.22:
		return g.pick(`"s"`, `1`, `true`, `{}`, `[]`, `[1]`, `{"a":1}`)
	default:
		return v()
	}
}

func (g *gen) leaf(depth int) string {
	return g.object(depth, func(int) string {
		return g.orNull(func() string { return g.pick(g.str(), g.num(), g.rawValue(1)) })
	}, "id", "n", "ext", "ID", "Ext")
}

func (g *gen) node(depth int) string {
	return g.object(depth, func(d int) string {
		return g.orNull(func() string {
			if d > 0 && g.chance(0.5) {
				return g.array(d-1, func(dd int) string { return g.orNull(func() string { return g.node(dd) }) })
			}
			return g.pick(g.str(), g.rawValue(1))
		})
	}, "id", "kids", "ext")
}

// field is a random value for one Review member.
func (g *gen) field(key string) string {
	strs := func() string { return g.array(0, func(int) string { return g.orNull(g.str) }) }
	raws := func(d int) string { return g.array(d, func(dd int) string { return g.rawValue(dd) }) }
	ints := func() string { return g.object(0, func(int) string { return g.orNull(g.num) }) }
	switch key {
	case "named", "namedptr", "es":
		return g.orNull(strs)
	case "rawmap", "maprawptr":
		return g.orNull(func() string { return g.object(1, g.rawValue) })
	case "modules":
		return g.orNull(func() string {
			return g.object(1, func(d int) string {
				return g.orNull(func() string { return g.object(d, g.rawValue, "rulesengine", "x", "y") })
			}, "prebid", "acme")
		})
	case "rawptrs", "ptrraws", "anys":
		return g.orNull(func() string { return raws(1) })
	case "rawraws":
		return g.orNull(func() string {
			return g.array(1, func(d int) string { return g.orNull(func() string { return raws(d) }) })
		})
	case "maprawraw":
		return g.orNull(func() string {
			return g.object(1, func(d int) string { return g.orNull(func() string { return raws(d) }) })
		})
	case "ptrmap":
		return g.orNull(ints)
	case "mapmap":
		return g.orNull(func() string { return g.object(0, func(int) string { return g.orNull(ints) }) })
	case "slicemap":
		return g.orNull(func() string { return g.array(0, func(int) string { return g.orNull(ints) }) })
	case "bytess":
		return g.orNull(func() string {
			return g.array(0, func(int) string {
				return g.orNull(func() string { return g.pick(`"AQID"`, `""`, `"!!"`, `[1,2]`, `[300]`, `"BAUG"`) })
			})
		})
	case "ptrptr", "leaf", "leafval", "p":
		return g.orNull(func() string { return g.leaf(1) })
	case "strnumptr":
		return g.orNull(func() string { return g.pick(`"5"`, `"-3"`, `5`, `"x"`, `""`, `"5 "`) })
	case "strslice":
		return g.orNull(func() string { return g.pick(`"[1,2]"`, `"[]"`, `[1]`, `"null"`, `"[1"`, `"x"`) })
	case "number", "numptr":
		return g.orNull(func() string { return g.pick(`1`, `"2"`, `-1.5e3`, `"x"`, `""`, `true`) })
	case "leafs":
		return g.orNull(func() string { return g.array(1, g.leaf) })
	case "leafmap", "leafptrmap":
		return g.orNull(func() string {
			return g.object(1, func(d int) string { return g.orNull(func() string { return g.leaf(d) }) })
		})
	case "any", "anymap":
		return g.value(2)
	case "flag":
		return g.orNull(func() string { return g.pick("true", "false") })
	case "f", "u8":
		return g.orNull(g.num)
	case "kids":
		return g.orNull(func() string {
			return g.array(2, func(d int) string { return g.orNull(func() string { return g.node(d) }) })
		})
	case "sintptr":
		return g.orNull(func() string { return g.pick(`5`, `"6"`, `"x"`, `1.5`) })
	case "raw", "rawomit", "ex":
		return g.rawValue(2)
	case "e":
		return g.orNull(g.str)
	case "ps":
		return g.orNull(func() string { return g.array(0, func(int) string { return g.orNull(g.num) }) })
	default:
		log.Fatalf("no generator for %s", key)
		return ""
	}
}

var reviewKeys = []string{
	"named", "namedptr", "rawmap", "modules", "rawptrs", "maprawptr", "ptrraws", "rawraws",
	"ptrmap", "mapmap", "slicemap", "bytess", "ptrptr", "strnumptr", "strslice", "number",
	"numptr", "leafs", "leafmap", "leafptrmap", "any", "anys", "anymap", "flag", "f", "u8",
	"kids", "leaf", "leafval", "sintptr", "raw", "rawomit", "maprawraw", "e", "ex", "es", "p",
	"ps",
}

// overlay is an object of random Review members; keys repeat now and then.
func (g *gen) overlay(keys []string, field func(string) string) string {
	n := 1 + g.r.Intn(4)
	parts := make([]string, 0, n+1)
	for i := 0; i < n; i++ {
		key := keys[g.r.Intn(len(keys))]
		quoted := `"` + key + `"`
		if g.chance(0.06) {
			quoted = `"` + strings.ToUpper(key) + `"`
		}
		member := g.ws() + quoted + g.ws() + ":" + g.ws() + field(key) + g.ws()
		parts = append(parts, member)
		if g.chance(0.15) {
			// The same key again: merged (raw messages) or replaced (slices) twice.
			parts = append(parts, g.ws()+quoted+":"+field(key))
		}
	}
	return g.ws() + "{" + strings.Join(parts, ",") + "}" + g.ws()
}

// malform breaks a valid overlay.
func (g *gen) malform(s string) string {
	switch g.r.Intn(7) {
	case 0:
		return s[:g.r.Intn(len(s))]
	case 1:
		return s + "x"
	case 2:
		i := g.r.Intn(len(s))
		return s[:i] + g.pick("x", ",", ":", "}", "]", `"`) + s[i:]
	case 3:
		return strings.Replace(s, ":", " ", 1)
	case 4:
		return "[" + s + "]"
	case 5:
		return g.pick("", " ", "null", "nul", "{}", "[]", `"s"`, "1")
	default:
		return strings.Replace(s, "null", "nul", 1)
	}
}

// ---- cases -------------------------------------------------------------------------------

var badRaws = []string{"\x00nil", "", "malformed", "null", "{", "[1]", `"x"`, "1", `{"a":1}x`, `{"a":{"b":1}}`, ` {"a" : 1} `, `[]`, `{}`}

func runCase(g *golden, target string, zero func() interface{}, setRaw func(interface{}, json.RawMessage), mode, existing, raw, overlay string) {
	v := zero()
	if mode == "U" {
		if err := jsonutil.Unmarshal([]byte(existing), v); err != nil {
			log.Fatalf("existing %s: %v", existing, err)
		}
	} else if err := jsonutil.MergeClone(v, json.RawMessage(existing)); err != nil {
		log.Fatalf("existing %s: %v", existing, err)
	}
	rawField := "-"
	if raw != "-" {
		if raw == "\x00nil" {
			setRaw(v, nil)
			rawField = "nil"
		} else {
			setRaw(v, json.RawMessage(raw))
			rawField = "=" + raw
		}
	}
	err := jsonutil.MergeClone(v, json.RawMessage(overlay))
	result, merr := jsonutil.Marshal(v)
	res := string(result)
	if merr != nil {
		res = "MARSHAL: " + merr.Error()
	}
	g.row("R", target, mode, existing, rawField, overlay, res, errText(err))
}

func setField(name string) func(interface{}, json.RawMessage) {
	return func(v interface{}, raw json.RawMessage) {
		reflect.ValueOf(v).Elem().FieldByName(name).SetBytes(raw)
	}
}

// existings returns n overlays that decode (and merge) without error into a zero value.
func existings(r *gen, n int, zero func() interface{}, overlay func() string) []string {
	out := []string{`{}`}
	for len(out) < n {
		s := overlay()
		a, b := zero(), zero()
		if jsonutil.Unmarshal([]byte(s), a) != nil || jsonutil.MergeClone(b, json.RawMessage(s)) != nil {
			continue
		}
		if _, err := jsonutil.Marshal(a); err != nil {
			continue
		}
		out = append(out, s)
	}
	return out
}

func main() {
	out := flag.String("out", "mergeclone_review", "output directory")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	f, err := os.Create(filepath.Join(*out, "mergeclone_review.txt"))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f)}
	g.write("# jsonutil.MergeClone(v, overlay) then jsonutil.Marshal(v): R target mode(U|M) existing raw(-|nil|=bytes) overlay result error\n")

	r := &gen{r: rand.New(rand.NewSource(20261118))}

	// main.Review.
	zeroReview := func() interface{} { return &Review{} }
	reviewOverlay := func() string { return r.overlay(reviewKeys, r.field) }
	base := existings(r, 40, zeroReview, reviewOverlay)
	for n := 0; n < 3200; n++ {
		mode := r.pick("U", "U", "M")
		existing := base[r.r.Intn(len(base))]
		raw := "-"
		if r.chance(0.15) {
			raw = badRaws[r.r.Intn(len(badRaws))]
		}
		overlay := reviewOverlay()
		if r.chance(0.1) {
			overlay = r.malform(overlay)
		}
		runCase(g, "main.Review", zeroReview, setField("Raw"), mode, existing, raw, overlay)
	}

	// openrtb2.Imp, as exchange/utils.go mergeImpFPD merges imp.ext.prebid.imp.<bidder>.
	zeroImp := func() interface{} { return &openrtb2.Imp{} }
	impKeys := []string{"id", "tagid", "banner", "video", "native", "pmp", "ext", "iframebuster", "bidfloor", "secure", "metric", "instl"}
	impField := func(key string) string {
		switch key {
		case "id", "tagid":
			return r.orNull(r.str)
		case "banner":
			return r.orNull(func() string {
				return r.object(1, func(d int) string {
					return r.orNull(func() string {
						return r.pick(r.num(), `[{"w":300,"h":250}]`, `[]`, `{"a":1}`, r.str())
					})
				}, "w", "h", "format", "id", "ext", "pos")
			})
		case "video":
			return r.orNull(func() string {
				return r.object(1, func(int) string {
					return r.orNull(func() string { return r.pick(r.num(), `["video/mp4"]`, `[2,3]`, r.rawValue(1)) })
				}, "mimes", "w", "h", "protocols", "ext")
			})
		case "native":
			return r.orNull(func() string {
				return r.object(1, func(int) string { return r.orNull(func() string { return r.pick(r.str(), r.rawValue(1)) }) }, "request", "ver", "ext")
			})
		case "pmp":
			return r.orNull(func() string {
				return r.object(1, func(int) string {
					return r.orNull(func() string {
						return r.pick(`1`, `0`, `[{"id":"d1","ext":{"a":1}}]`, `[{"id":"d2"},null]`, r.rawValue(1))
					})
				}, "private_auction", "deals", "ext")
			})
		case "ext":
			return r.rawValue(3)
		case "iframebuster":
			return r.orNull(func() string { return r.array(0, func(int) string { return r.orNull(r.str) }) })
		case "bidfloor":
			return r.orNull(r.num)
		case "secure", "instl":
			return r.orNull(func() string { return r.pick(`1`, `0`, `2`, `-1`) })
		default:
			return r.orNull(func() string {
				return r.array(0, func(int) string { return r.object(0, r.value, "type", "value", "vendor") })
			})
		}
	}
	impOverlay := func() string { return r.overlay(impKeys, impField) }
	impBase := existings(r, 30, zeroImp, impOverlay)
	for n := 0; n < 1200; n++ {
		mode := r.pick("U", "U", "M")
		existing := impBase[r.r.Intn(len(impBase))]
		raw := "-"
		if r.chance(0.15) {
			raw = badRaws[r.r.Intn(len(badRaws))]
		}
		overlay := impOverlay()
		if r.chance(0.1) {
			overlay = r.malform(overlay)
		}
		runCase(g, "openrtb2.Imp", zeroImp, setField("Ext"), mode, existing, raw, overlay)
	}

	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("mergeclone_review.txt: %d rows\n", g.cases)
}
