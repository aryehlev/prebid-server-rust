package main

import (
	"encoding/json"
	"strconv"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// The seeded corpus of MergeClone: an existing Shapes value, decoded with jsonutil.Unmarshal from
// a seeded document, and a seeded patch merged into it. The outcome is "error" (the texts are
// jsoniter's, ledgered) or jsonutil.Marshal of the merged value, which the Rust test type writes
// byte for byte: the documents keep to spellings both writers share (no HTML characters, floats
// that are not integers, integer map keys -1 to 9, no null json.RawMessage values Go would write
// as nothing), and json.RawMessage patches stay clear of the merge-patch quirks (objects only,
// arrays of scalars), which merge_patch.json covers.
const (
	mergeCloneSeed  = 0x4a53_4f4e_4d52_4731 // "JSONMRG1"
	mergeCloneCount = 50000
	mergeCloneBatch = 1000
)

// Inner is the struct reached through pointers, slices and maps.
type Inner struct {
	ID   string          `json:"id,omitempty"`
	N    *int64          `json:"n,omitempty"`
	Tags []string        `json:"tags,omitempty"`
	Ext  json.RawMessage `json:"ext,omitempty"`
}

// Shapes has a field of every kind MergeClone's decoders treat differently.
type Shapes struct {
	Str       string                     `json:"str"`
	Int       int64                      `json:"int"`
	Int8      int8                       `json:"int8,omitempty"`
	Uint      uint32                     `json:"uint,omitempty"`
	Float     float64                    `json:"float,omitempty"`
	Bool      bool                       `json:"bool"`
	IntPtr    *int64                     `json:"intptr,omitempty"`
	StrPtr    *string                    `json:"strptr"`
	Inner     *Inner                     `json:"inner,omitempty"`
	InnerVal  Inner                      `json:"innerval"`
	Strs      []string                   `json:"strs"`
	Inners    []Inner                    `json:"inners,omitempty"`
	InnerPtrs []*Inner                   `json:"innerptrs,omitempty"`
	Nested    [][]int64                  `json:"nested,omitempty"`
	MapInt    map[string]int64           `json:"mapint,omitempty"`
	MapNum    map[int64]string           `json:"mapnum,omitempty"`
	MapInner  map[string]*Inner          `json:"mapinner,omitempty"`
	MapRaw    map[string]json.RawMessage `json:"mapraw,omitempty"`
	Raw       json.RawMessage            `json:"raw,omitempty"`
	Raws      []json.RawMessage          `json:"raws,omitempty"`
	Any       interface{}                `json:"any,omitempty"`
	Num       jsonutil.StringInt         `json:"num,omitempty"`
	IntStr    jsonutil.IntString         `json:"intstr,omitempty"`
}

var (
	mcStrings  = []string{"a", "b", "x y", ""}
	mcKeys     = []string{"a", "b", "c"}
	mcFloats   = []string{"0.5", "1.25", "-2.75", "0.125", "3.5"}
	mcNumKeys  = []string{"-1", "0", "1", "2", "7", "9"}
	mcFields   = []string{"str", "int", "int8", "uint", "float", "bool", "intptr", "strptr", "inner", "innerval", "strs", "inners", "innerptrs", "nested", "mapint", "mapnum", "mapinner", "mapraw", "raw", "raws", "any", "num", "intstr"}
	mcInnerFld = []string{"id", "n", "tags", "ext"}
)

func (s *splitmix64) mcString() string {
	return `"` + s.pick(mcStrings) + `"`
}

func (s *splitmix64) mcInt() string {
	return strconv.Itoa(s.below(2000) - 1000)
}

// mcScalar is a JSON scalar both writers spell alike.
func (s *splitmix64) mcScalar() string {
	switch s.below(5) {
	case 0:
		return s.mcString()
	case 1:
		return s.mcInt()
	case 2:
		return s.pick(mcFloats)
	case 3:
		return "true"
	default:
		return "false"
	}
}

// mcArray is an array of n draws of item.
func (s *splitmix64) mcArray(item func() string) string {
	n := s.below(4)
	parts := make([]string, 0, n)
	for i := 0; i < n; i++ {
		parts = append(parts, item())
	}
	return "[" + strings.Join(parts, ",") + "]"
}

// mcObject is an object with up to three members, its names drawn from keys (repeats allowed:
// both sides take members in order).
func (s *splitmix64) mcObject(keys []string, value func() string) string {
	n := s.below(4)
	parts := make([]string, 0, n)
	for i := 0; i < n; i++ {
		key := s.pick(keys)
		parts = append(parts, `"`+key+`":`+value())
	}
	return "{" + strings.Join(parts, ",") + "}"
}

// mcRaw is a json.RawMessage value: an object whose members are scalars, nulls (a merge patch
// deletes them), arrays of scalars and objects of the same kind.
func (s *splitmix64) mcRaw(depth int) string {
	return s.mcObject(mcKeys, func() string {
		n := s.below(6)
		if depth >= 2 && n >= 4 {
			n = s.below(4)
		}
		switch n {
		case 0, 1:
			return s.mcScalar()
		case 2:
			return "null"
		case 3:
			return s.mcArray(s.mcScalar)
		default:
			return s.mcRaw(depth + 1)
		}
	})
}

// mcAny is any JSON value (interface{} replaces, so nulls anywhere are fine).
func (s *splitmix64) mcAny(depth int) string {
	n := s.below(8)
	if depth >= 2 && n >= 6 {
		n = s.below(6)
	}
	switch n {
	case 0:
		return "null"
	case 1, 2, 3, 4, 5:
		return s.mcScalar()
	case 6:
		return s.mcArray(func() string { return s.mcAny(depth + 1) })
	default:
		return s.mcObject(mcKeys, func() string { return s.mcAny(depth + 1) })
	}
}

// mcNonNullRaw is a json.RawMessage value that is not null: a fresh map value or slice element
// holds it as written, and Go writes a nil one as nothing.
func (s *splitmix64) mcNonNullRaw() string {
	switch s.below(3) {
	case 0:
		return s.mcScalar()
	case 1:
		return s.mcArray(s.mcScalar)
	default:
		return s.mcRaw(1)
	}
}

// mcMaybe returns null with probability 1/n when patch is set, else value().
func (s *splitmix64) mcMaybe(patch bool, n int, value func() string) string {
	if patch && s.below(n) == 0 {
		return "null"
	}
	return value()
}

// mcWrong is a value of the wrong kind for most fields, drawn only for patches.
func (s *splitmix64) mcWrong() string {
	return s.pick([]string{`"x"`, `[]`, `{}`, `1.5`, `true`, `300`, `-1`, `"12"`})
}

func (s *splitmix64) mcInnerValue(field string, patch bool) string {
	switch field {
	case "id":
		return s.mcMaybe(patch, 6, s.mcString)
	case "n":
		return s.mcMaybe(patch, 4, s.mcInt)
	case "tags":
		return s.mcMaybe(patch, 4, func() string { return s.mcArray(s.mcString) })
	default:
		if patch && s.below(4) == 0 {
			return "null"
		}
		return s.mcRaw(1)
	}
}

// mcInnerObject is an Inner object with up to three members. A patch may name a field twice (each
// member merges into the result of the one before); an existing document names each field once,
// since jsonutil.Unmarshal decodes a repeated field into the same value and serde rejects it.
func (s *splitmix64) mcInnerObject(patch bool) string {
	n := s.below(4)
	parts := make([]string, 0, n)
	used := map[string]bool{}
	for i := 0; i < n; i++ {
		field := s.pick(mcInnerFld)
		if !patch && used[field] {
			continue
		}
		used[field] = true
		parts = append(parts, `"`+field+`":`+s.mcInnerValue(field, patch))
	}
	return "{" + strings.Join(parts, ",") + "}"
}

// mcValue is a value for one Shapes field; a patch may hold null or a value of the wrong kind.
func (s *splitmix64) mcValue(field string, patch bool) string {
	// A json.RawMessage given a value that is not an object meets the merge-patch quirks, and a
	// float given an integer is written "300" by Go and "300.0" by serde_json, so neither field
	// draws a wrong value.
	if patch && field != "raw" && field != "float" && s.below(25) == 0 {
		return s.mcWrong()
	}
	switch field {
	case "str":
		return s.mcMaybe(patch, 6, s.mcString)
	case "int":
		return s.mcMaybe(patch, 6, s.mcInt)
	case "int8":
		return s.mcMaybe(patch, 6, func() string { return strconv.Itoa(s.below(256) - 128) })
	case "uint":
		return s.mcMaybe(patch, 6, func() string { return strconv.Itoa(s.below(100000)) })
	case "float":
		return s.mcMaybe(patch, 6, func() string { return s.pick(mcFloats) })
	case "bool":
		return s.mcMaybe(patch, 6, func() string { return s.pick([]string{"true", "false"}) })
	case "intptr":
		return s.mcMaybe(true, 4, s.mcInt)
	case "strptr":
		return s.mcMaybe(true, 4, s.mcString)
	case "inner", "innerval":
		return s.mcMaybe(true, 5, func() string { return s.mcInnerObject(patch) })
	case "strs":
		return s.mcMaybe(true, 5, func() string { return s.mcArray(s.mcString) })
	case "inners":
		// A null element of a slice of structs decodes to the zero value in Go only (serde
		// rejects it outside MergeClone), so existing documents hold none.
		return s.mcMaybe(true, 5, func() string {
			return s.mcArray(func() string { return s.mcMaybe(patch, 5, func() string { return s.mcInnerObject(patch) }) })
		})
	case "innerptrs":
		return s.mcMaybe(true, 5, func() string {
			return s.mcArray(func() string { return s.mcMaybe(true, 4, func() string { return s.mcInnerObject(patch) }) })
		})
	case "nested":
		return s.mcMaybe(true, 5, func() string {
			return s.mcArray(func() string { return s.mcMaybe(true, 4, func() string { return s.mcArray(s.mcInt) }) })
		})
	case "mapint":
		return s.mcMaybe(true, 5, func() string { return s.mcObject(mcKeys, s.mcInt) })
	case "mapnum":
		return s.mcMaybe(true, 5, func() string { return s.mcObject(mcNumKeys, s.mcString) })
	case "mapinner":
		return s.mcMaybe(true, 5, func() string {
			return s.mcObject(mcKeys, func() string { return s.mcMaybe(true, 4, func() string { return s.mcInnerObject(patch) }) })
		})
	case "mapraw":
		return s.mcMaybe(true, 5, func() string { return s.mcObject(mcKeys, s.mcNonNullRaw) })
	case "raw":
		return s.mcMaybe(patch, 4, func() string { return s.mcRaw(1) })
	case "raws":
		return s.mcMaybe(true, 5, func() string { return s.mcArray(s.mcNonNullRaw) })
	case "any":
		return s.mcAny(1)
	case "num":
		return s.pick([]string{`"12"`, `7`, `""`, `"-30"`, `0`})
	default:
		return s.pick([]string{`"ab"`, `30`, `"7"`, `1.5`, `-2`})
	}
}

// mcDocument is a Shapes object: up to five members, unknown members among them in a patch. As in
// mcInnerObject, only a patch names a field twice.
func (s *splitmix64) mcDocument(patch bool) string {
	n := s.below(6)
	parts := make([]string, 0, n)
	used := map[string]bool{}
	for i := 0; i < n; i++ {
		if patch && s.below(15) == 0 {
			parts = append(parts, `"zz":`+s.mcAny(1))
			continue
		}
		field := s.pick(mcFields)
		if !patch && used[field] {
			continue
		}
		used[field] = true
		parts = append(parts, `"`+field+`":`+s.mcValue(field, patch))
	}
	return "{" + strings.Join(parts, ",") + "}"
}

// mcPatch is the patch of one case: mostly an object, at times another top-level value.
func (s *splitmix64) mcPatch() string {
	switch s.below(40) {
	case 0:
		return "null"
	case 1:
		return "[]"
	case 2:
		return `"x"`
	case 3:
		return "{}"
	default:
		return s.mcDocument(true)
	}
}

// mergeCloneOutcome merges patch into the Shapes value existing decodes to.
func mergeCloneOutcome(existing, patch string) string {
	var v Shapes
	if err := jsonutil.Unmarshal([]byte(existing), &v); err != nil {
		panic("a seeded existing document does not decode: " + err.Error() + ": " + existing)
	}
	if err := jsonutil.MergeClone(&v, []byte(patch)); err != nil {
		return "error"
	}
	out, err := jsonutil.Marshal(v)
	if err != nil {
		panic(err)
	}
	return string(out)
}

func mergeCloneCorpus() seededCorpus {
	return seededCorpus{
		seed:  mergeCloneSeed,
		count: mergeCloneCount,
		batch: mergeCloneBatch,
		next: func(s *splitmix64) (string, string) {
			existing := s.mcDocument(false)
			patch := s.mcPatch()
			return existing + "\t" + patch, mergeCloneOutcome(existing, patch)
		},
	}
}

// mergeCloneCase is one of merge_test.go's cases (or an adjusted twin): the existing value and the
// result as jsonutil.Marshal writes them, and MergeClone's error. Existing is empty when the
// value cannot be written as JSON (a malformed json.RawMessage).
type mergeCloneCase struct {
	Test     string `json:"test"`
	Name     string `json:"name"`
	Existing string `json:"existing"`
	Patch    string `json:"patch"`
	Result   string `json:"result"`
	Err      string `json:"err"`
}

type mergeCloneDoc struct {
	Header  header           `json:"header"`
	Curated []mergeCloneCase `json:"curated"`
	Seeded  seededDigests    `json:"seeded"`
}

// mergeCase merges patch into v (a pointer) and records both writes and the error.
func mergeCase(test, name string, v any, patch string) mergeCloneCase {
	c := mergeCloneCase{Test: test, Name: name, Patch: patch}
	if before, err := jsonutil.Marshal(v); err == nil && json.Valid(before) {
		c.Existing = string(before)
	}
	if err := jsonutil.MergeClone(v, []byte(patch)); err != nil {
		c.Err = err.Error()
	}
	after, err := jsonutil.Marshal(v)
	if err != nil {
		panic(err)
	}
	c.Result = string(after)
	return c
}

func int8Ptr(v int8) *int8 { return &v }

// mergeCloneCases runs merge_test.go's cases (util/jsonutil/merge_test.go) with the values they
// build, plus twins of the slice cases spelled with the json key itself ("iframebuster"): Go
// matches "iframeBuster" case-insensitively, the port does not (json.key-case).
func mergeCloneCases() []mergeCloneCase {
	var cases []mergeCloneCase
	imp := func() *openrtb2.Imp {
		return &openrtb2.Imp{Banner: &openrtb2.Banner{ID: "1"}, Video: &openrtb2.Video{PodID: "a"}}
	}
	cases = append(cases,
		mergeCase("TestMergeClonePtr", "root", &openrtb2.Imp{Banner: &openrtb2.Banner{ID: "1"}}, `{"banner":{"id":"4"}}`),
		mergeCase("TestMergeClonePtr", "embedded-nil", imp(), `{"banner":null}`),
		mergeCase("TestMergeClonePtr", "embedded-struct", imp(), `{"banner":{"id":"2"}}`),
		mergeCase("TestMergeClonePtr", "embedded-int", &openrtb2.Imp{ClickBrowser: int8Ptr(1)}, `{"clickbrowser":2}`),
		mergeCase("TestMergeClonePtr", "invalid-null", &openrtb2.Imp{Banner: &openrtb2.Banner{ID: "1"}}, `{"banner":nul}`),
		mergeCase("TestMergeClonePtr", "invalid-malformed", &openrtb2.Imp{Banner: &openrtb2.Banner{ID: "1"}}, `{"banner":malformed}`),
	)
	for _, key := range []string{"iframeBuster", "iframebuster"} {
		cases = append(cases,
			mergeCase("TestMergeCloneSlice", "null/"+key, &openrtb2.Imp{IframeBuster: []string{"a", "b"}}, `{"`+key+`":null}`),
			mergeCase("TestMergeCloneSlice", "one/"+key, &openrtb2.Imp{IframeBuster: []string{"a"}}, `{"`+key+`":["b"]}`),
			mergeCase("TestMergeCloneSlice", "many/"+key, &openrtb2.Imp{IframeBuster: []string{"a"}}, `{"`+key+`":["b", "c"]}`),
			mergeCase("TestMergeCloneSlice", "invalid-null/"+key, &openrtb2.Imp{IframeBuster: []string{"a"}}, `{"`+key+`":nul}`),
			mergeCase("TestMergeCloneSlice", "invalid-malformed/"+key, &openrtb2.Imp{IframeBuster: []string{"a"}}, `{"`+key+`":malformed}`),
		)
	}
	cases = append(cases, mergeCase("TestMergeCloneSlice", "replace-not-overlay", &struct {
		Imps []*openrtb2.Imp `json:"imps"`
	}{Imps: []*openrtb2.Imp{{ID: "1"}}}, `{"imps":[{"tagid":"2"}]}`))
	type fooMap struct {
		Foo map[string]int `json:"foo"`
	}
	type fooNum struct {
		Foo map[int]string `json:"foo"`
	}
	cases = append(cases,
		mergeCase("TestMergeCloneMap", "null", &fooMap{Foo: map[string]int{"a": 1, "b": 2}}, `{"foo":null}`),
		mergeCase("TestMergeCloneMap", "key-string", &fooMap{Foo: map[string]int{"a": 1, "b": 2}}, `{"foo":{"c":3}}`),
		mergeCase("TestMergeCloneMap", "key-numeric", &fooNum{Foo: map[int]string{1: "a", 2: "b"}}, `{"foo":{"3":"c"}}`),
		mergeCase("TestMergeCloneMap", "invalid-null", &fooNum{Foo: map[int]string{1: "a", 2: "b"}}, `{"foo":nul}`),
		mergeCase("TestMergeCloneMap", "invalid-malformed", &fooNum{Foo: map[int]string{1: "a", 2: "b"}}, `{"foo":malformed}`),
	)
	ext := []struct{ name, existing, incoming string }{
		{"both-populated", `{"a":1,"b":2}`, `{"b":200,"c":3}`},
		{"both-omitted", "", ""},
		{"both-nil", "", `null`},
		{"both-empty", "", `{}`},
		{"ext-omitted", `{"b":2}`, ""},
		{"ext-nil", `{"b":2}`, `null`},
		{"ext-empty", `{"b":2}`, `{}`},
		{"ext-malformed", `{"b":2}`, `malformed`},
		{"existing-nil", "", `{"a":1}`},
		{"existing-empty", `{}`, `{"a":1}`},
		{"existing-omitted", "", `{"b":2}`},
		{"existing-malformed", `malformed`, `{"a":1}`},
	}
	for _, e := range ext {
		var existing json.RawMessage
		if e.existing != "" {
			existing = json.RawMessage(e.existing)
		}
		data := `{}`
		if e.incoming != "" {
			data = `{"ext":` + e.incoming + `}`
		}
		cases = append(cases, mergeCase("TestMergeCloneExt", e.name, &openrtb2.BidRequest{Ext: existing}, data))
	}
	rawJSON := json.RawMessage(`{"a":1}`)
	cases = append(cases,
		mergeCase("TestMergeCloneCombinations", "slice-of-ptr", &struct {
			Imps []*openrtb2.Imp `json:"imps"`
		}{Imps: []*openrtb2.Imp{{ID: "1"}}}, `{"imps":[{"id":"2"}]}`),
		mergeCase("TestMergeCloneCombinations", "jsonrawmessage-ptr", &struct {
			Foo *json.RawMessage `json:"foo"`
		}{Foo: &rawJSON}, `{"foo":{"b":2}}`),
		mergeCase("TestMergeCloneCombinations", "struct-ptr", &struct {
			Imp *openrtb2.Imp `json:"imp"`
		}{Imp: &openrtb2.Imp{ID: "1"}}, `{"imp":{"id":"2"}}`),
		mergeCase("TestMergeCloneCombinations", "map-of-ptrs", &struct {
			Imps map[string]*openrtb2.Imp `json:"imps"`
		}{Imps: map[string]*openrtb2.Imp{"a": {ID: "1"}}}, `{"imps":{"a":{"id":"2"}}}`),
	)
	// The Shapes edges: null into each kind of field, the custom decoders, a raw merge with a
	// nested delete, duplicate members applied in order, and a mistyped member.
	shapes := func() *Shapes {
		n := int64(5)
		str := "p"
		return &Shapes{
			Str: "s", Int: 3, Int8: 4, Uint: 5, Float: 0.5, Bool: true, IntPtr: &n, StrPtr: &str,
			Inner: &Inner{ID: "i", Ext: json.RawMessage(`{"a":1}`)}, InnerVal: Inner{ID: "v"},
			Strs: []string{"a"}, MapInt: map[string]int64{"a": 1}, MapNum: map[int64]string{1: "a"},
			Raw: json.RawMessage(`{"a":{"b":1,"c":2}}`), Any: "x", Num: 9, IntStr: "q",
		}
	}
	for _, patch := range []string{
		`{"str":null,"int":null,"int8":null,"uint":null,"float":null,"bool":null}`,
		`{"intptr":null,"strptr":null,"inner":null,"innerval":null}`,
		`{"strs":null,"mapint":null,"mapnum":null,"raw":null,"any":null}`,
		`{"num":""}`, `{"num":"12"}`, `{"num":12}`, `{"num":null}`, `{"num":"1.5"}`, `{"num":"-"}`, `{"num":"+5"}`,
		`{"intstr":30}`, `{"intstr":1.50}`, `{"intstr":"x"}`, `{"intstr":null}`, `{"intstr":true}`,
		`{"raw":{"a":{"b":null,"d":3}}}`, `{"inner":{"ext":{"b":2}}}`, `{"inner":{"id":"j"},"inner":{"ext":null}}`,
		`{"str":"t","int":"x","bool":false}`, `{"mapnum":{"3":"c","x":"d"}}`, `{"zz":{"unknown":[1,2]},"str":"u"}`,
		`null`, `[]`, `"x"`, `{}`, `{"str":"a"} `, `{"str":"a"} x`,
	} {
		cases = append(cases, mergeCase("Shapes", patch, shapes(), patch))
	}
	return cases
}

func mergeCloneGolden() mergeCloneDoc {
	return mergeCloneDoc{
		Header:  newHeader("github.com/json-iterator/go", "gopkg.in/evanphx/json-patch.v5", "github.com/prebid/openrtb/v20"),
		Curated: mergeCloneCases(),
		Seeded:  mergeCloneCorpus().digests(),
	}
}
