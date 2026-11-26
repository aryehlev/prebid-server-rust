package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"log"
	"math"
	"os"
	"reflect"
	"runtime"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// shapeCase is one case of shapes.jsonl: a document decoded as a Go type ("D"), or a patch
// merged into a value of the type with jsonutil.MergeClone ("M").
type shapeCase struct {
	kind  string
	typ   reflect.Type
	label string
	doc   string // the document, or the document the merge target is decoded from
	patch string // the merged document ("M" only)
}

// methodEdges are the edge values of a type with an UnmarshalJSON method (methodValues and
// stringOrStringArray), also as a pointer's target; nothing for other types.
func methodEdges(t reflect.Type) []string {
	for t.Kind() == reflect.Ptr {
		t = t.Elem()
	}
	if v, ok := valuesOf(t); ok {
		return v.edges
	}
	return nil
}

// shapeCases are the shape cases of one type. A struct gets, for each JSON field (embedded
// structs' fields promoted): every sample value, the zero value (an empty slice or map for
// those), null, for integers the extremes of the type and one beyond each, and for a type with
// an UnmarshalJSON method its edge values; and the merges of a sample into the zero value, of
// the last sample over the first, of null over the first, of a value of the wrong JSON type and
// of each edge value over the first sample. The struct as a whole gets the empty object (every
// field absent) and every field at once, sampled, zero and null. A named type, and a type with
// an UnmarshalJSON method, gets the same values as top-level documents.
func shapeCases(t reflect.Type) []shapeCase {
	var out []shapeCase
	decode := func(label, doc string) {
		out = append(out, shapeCase{kind: "D", typ: t, label: label, doc: doc})
	}
	merge := func(label, doc, patch string) {
		out = append(out, shapeCase{kind: "M", typ: t, label: label, doc: doc, patch: patch})
	}
	if _, method := valuesOf(t); method || t.Kind() != reflect.Struct {
		v := valuesFor(t)
		for i, s := range v.samples {
			decode(fmt.Sprintf("sample %d", i), s)
		}
		decode("zero", v.zero)
		decode("null", "null")
		for i, e := range intExtremes(t) {
			decode(fmt.Sprintf("extreme %d", i), e)
		}
		for i, e := range methodEdges(t) {
			decode(fmt.Sprintf("edge %d", i), e)
		}
		first, last := v.samples[0], v.samples[len(v.samples)-1]
		merge("set", v.zero, first)
		merge("replace", first, last)
		merge("null", first, "null")
		for i, w := range v.wrong {
			merge(fmt.Sprintf("wrong %d", i), first, w)
		}
		for i, e := range methodEdges(t) {
			merge(fmt.Sprintf("edge %d", i), first, e)
		}
		return out
	}
	decode("absent", "{}")
	fields := jsonFields(t, nil)
	var every, zeros, nulls []string
	for _, f := range fields {
		v := valuesFor(f.typ)
		key := strconv.Quote(f.key)
		member := func(value string) string { return "{" + key + ":" + value + "}" }
		for i, s := range v.samples {
			decode(fmt.Sprintf("%s sample %d", f.key, i), member(s))
		}
		decode(f.key+" zero", member(v.zero))
		decode(f.key+" null", member("null"))
		for i, e := range intExtremes(f.typ) {
			decode(fmt.Sprintf("%s extreme %d", f.key, i), member(e))
		}
		for i, e := range methodEdges(f.typ) {
			decode(fmt.Sprintf("%s edge %d", f.key, i), member(e))
		}
		first, last := v.samples[0], v.samples[len(v.samples)-1]
		merge(f.key+" set", "{}", member(first))
		merge(f.key+" replace", member(first), member(last))
		merge(f.key+" null", member(first), member("null"))
		if len(v.wrong) > 0 {
			merge(f.key+" wrong", "{}", member(v.wrong[0]))
		}
		for i, e := range methodEdges(f.typ) {
			merge(fmt.Sprintf("%s edge %d", f.key, i), member(first), member(e))
		}
		every = append(every, key+":"+first)
		zeros = append(zeros, key+":"+v.zero)
		nulls = append(nulls, key+":null")
	}
	if len(fields) > 0 {
		all := "{" + strings.Join(every, ",") + "}"
		decode("every field", all)
		decode("every field zero", "{"+strings.Join(zeros, ",")+"}")
		decode("every field null", "{"+strings.Join(nulls, ",")+"}")
		merge("every field", "{}", all)
		merge("every field null", all, "{"+strings.Join(nulls, ",")+"}")
	}
	return out
}

// intExtremes are the largest and smallest values of an integer type and one beyond each (as
// a pointer's target or a slice's only element too); nothing for other types.
func intExtremes(t reflect.Type) []string {
	switch t.Kind() {
	case reflect.Ptr:
		return intExtremes(t.Elem())
	case reflect.Slice:
		if t == rawMessageType {
			return nil
		}
		var out []string
		for _, e := range intExtremes(t.Elem()) {
			out = append(out, "["+e+"]")
		}
		return out
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		max := int64(math.MaxInt64 >> (64 - t.Bits()))
		above := strconv.FormatUint(uint64(max)+1, 10)
		return []string{strconv.FormatInt(max, 10), strconv.FormatInt(-max-1, 10), above, "-" + plusOne(above)}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		max := strconv.FormatUint(math.MaxUint64>>(64-t.Bits()), 10)
		return []string{max, plusOne(max), "-1"}
	default:
		return nil
	}
}

// shapeResult is a decode and re-encode: "!" and the decoder's error text, "?" and the
// encoder's, or the encoded bytes.
func shapeResult(doc []byte, t reflect.Type, decode func([]byte, interface{}) error,
	encode func(interface{}) ([]byte, error)) string {
	v := reflect.New(t)
	if err := decode(doc, v.Interface()); err != nil {
		return "!" + err.Error()
	}
	out, err := encode(v.Elem().Interface())
	if err != nil {
		return "?" + err.Error()
	}
	return string(out)
}

// writeShapes writes shapes.jsonl, the shape cases of types (the decodable types): a first line
// ["#go", version], then one JSON array of strings per case,
//
//	["D", type, label, doc, jsoniter, std]
//	["M", type, label, doc, patch, err, merged]
//
// where jsoniter is doc decoded with jsonutil.Unmarshal and encoded with jsonutil.Marshal, std
// the same with json.Unmarshal and json.Marshal (each a shapeResult), err the text of
// jsonutil.MergeClone's error (empty for none) after doc was decoded with jsonutil.Unmarshal,
// and merged the merged value encoded with jsonutil.Marshal ("?" and the error when that fails).
func writeShapes(path string, types []reflect.Type) int {
	var cases []shapeCase
	for _, t := range types {
		cases = append(cases, shapeCases(t)...)
	}
	f, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(f)
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	row := func(fields ...string) {
		if err := enc.Encode(fields); err != nil {
			log.Fatal(err)
		}
	}
	row("#go", runtime.Version())
	for _, c := range cases {
		name := c.typ.String()
		if c.kind == "D" {
			doc := []byte(c.doc)
			row("D", name, c.label, c.doc,
				shapeResult(doc, c.typ, jsonutil.Unmarshal, jsonutil.Marshal),
				shapeResult(doc, c.typ, json.Unmarshal, json.Marshal))
			continue
		}
		v := reflect.New(c.typ)
		if err := jsonutil.Unmarshal([]byte(c.doc), v.Interface()); err != nil {
			log.Fatalf("%s %s: the merge target does not decode: %v", name, c.label, err)
		}
		errText := ""
		if err := jsonutil.MergeClone(v.Interface(), json.RawMessage(c.patch)); err != nil {
			errText = err.Error()
		}
		merged := "?"
		if out, err := jsonutil.Marshal(v.Elem().Interface()); err != nil {
			merged += err.Error()
		} else {
			merged = string(out)
		}
		row("M", name, c.label, c.doc, c.patch, errText, merged)
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	return len(cases)
}
