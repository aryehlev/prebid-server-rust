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

// shapeValues are the values shapeCases gives a field of a Go type: its JSON values and its
// integer extremes.
type shapeValues struct {
	values   func(reflect.Type) values
	extremes func(reflect.Type) []string
}

// portedValues are the values of the ported types' fields.
var portedValues = shapeValues{values: valuesFor, extremes: intExtremes}

// shapeCases are the shape cases of one type, with the values of sv. A struct gets, for each JSON
// field (embedded structs' fields promoted): every sample value, the zero value (an empty slice
// or map for those), null and, for integers, the extremes of the type and one beyond each; and
// the merges of a sample into the zero value, of the last sample over the first, of null over
// the first and of a value of the wrong JSON type. The struct as a whole gets the empty object
// (every field absent) and every field at once, sampled, zero and null. A named type gets the
// same values as top-level documents.
func shapeCases(t reflect.Type, sv shapeValues) []shapeCase {
	var out []shapeCase
	decode := func(label, doc string) {
		out = append(out, shapeCase{kind: "D", typ: t, label: label, doc: doc})
	}
	merge := func(label, doc, patch string) {
		out = append(out, shapeCase{kind: "M", typ: t, label: label, doc: doc, patch: patch})
	}
	if t.Kind() != reflect.Struct {
		v := sv.values(t)
		for i, s := range v.samples {
			decode(fmt.Sprintf("sample %d", i), s)
		}
		decode("zero", v.zero)
		decode("null", "null")
		for i, e := range sv.extremes(t) {
			decode(fmt.Sprintf("extreme %d", i), e)
		}
		first, last := v.samples[0], v.samples[len(v.samples)-1]
		merge("set", v.zero, first)
		merge("replace", first, last)
		merge("null", first, "null")
		return out
	}
	decode("absent", "{}")
	fields := jsonFields(t, nil)
	var every, zeros, nulls []string
	for _, f := range fields {
		v := sv.values(f.typ)
		key := strconv.Quote(f.key)
		member := func(value string) string { return "{" + key + ":" + value + "}" }
		for i, s := range v.samples {
			decode(fmt.Sprintf("%s sample %d", f.key, i), member(s))
		}
		decode(f.key+" zero", member(v.zero))
		decode(f.key+" null", member("null"))
		for i, e := range sv.extremes(f.typ) {
			decode(fmt.Sprintf("%s extreme %d", f.key, i), member(e))
		}
		first, last := v.samples[0], v.samples[len(v.samples)-1]
		merge(f.key+" set", "{}", member(first))
		merge(f.key+" replace", member(first), member(last))
		merge(f.key+" null", member(first), member("null"))
		if len(v.wrong) > 0 {
			merge(f.key+" wrong", "{}", member(v.wrong[0]))
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

// writeShapes writes shapes.jsonl, the shape cases of every ported type (see writeShapeCases).
func writeShapes(path string) int {
	var cases []shapeCase
	for _, p := range portedTypes {
		cases = append(cases, shapeCases(p.goType, portedValues)...)
	}
	return writeShapeCases(path, cases)
}

// writeShapeCases writes shape cases: a first line ["#go", version], then one JSON array of
// strings per case,
//
//	["D", type, label, doc, jsoniter, std]
//	["M", type, label, doc, patch, err, merged]
//
// where jsoniter is doc decoded with jsonutil.Unmarshal and encoded with jsonutil.Marshal, std
// the same with json.Unmarshal and json.Marshal (each a shapeResult), err the text of
// jsonutil.MergeClone's error (empty for none) after doc was decoded with jsonutil.Unmarshal,
// and merged the merged value encoded with jsonutil.Marshal ("?" and the error when that fails).
func writeShapeCases(path string, cases []shapeCase) int {
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
