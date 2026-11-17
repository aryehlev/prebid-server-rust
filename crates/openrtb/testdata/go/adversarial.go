package main

import (
	"fmt"
	"reflect"
	"strconv"
	"strings"

	"github.com/prebid/openrtb/v20/native1/request"
	"github.com/prebid/openrtb/v20/native1/response"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/openrtb/v20/openrtb3"
)

// adversarial returns the cases of adversarial.tsv, aimed at what the mutations leave out:
//
//   - duplicate keys whose values merge in Go: a struct or pointer-to-struct field decoded twice
//     keeps the fields of both objects, a map keeps the keys of both, and a slice of structs
//     decodes into its existing elements;
//   - null and wrong values inside slices, first and later in the array (every wrong and edge
//     value once per element type, null and one wrong value in the other slices);
//   - keys that match a field only through Unicode case folding: encoding/json folds U+017F to
//     s and U+212A to k, while jsoniter lower-cases with strings.ToLower (U+0130 to i, U+212A to
//     k) in structs of more than ten fields and only ASCII letters in the others;
//   - a key in two ASCII cases, in both orders;
//   - malformed objects around a known field of every struct, and around unknown fields for one
//     struct per jsoniter struct decoder (it has one for each field count up to ten, and a
//     general one);
//   - json.RawMessage contents that the three encoders escape or compact differently;
//   - for the request and response roots, a wrong value at every field path two to five levels
//     deep, inside the second element of each slice;
//   - documents at the nesting limit (10000) of jsoniter and encoding/json.
//
// The cases labelled "stale element" are a known difference: Go reuses a slice's backing array
// beyond its length, so a third occurrence of a key can revive elements that the second dropped,
// while a Rust Vec drops them (see the parity tests).
func adversarial() []testCase {
	a := &adv{seen: map[string]bool{}, edged: map[reflect.Type]bool{}}
	decoders := map[int]bool{}
	for _, p := range portedTypes {
		t := p.goType
		if t.Kind() != reflect.Struct {
			continue
		}
		fields := jsonFields(t, nil)
		decoder := min(len(fields), 11)
		representative := !decoders[decoder]
		decoders[decoder] = true
		a.malformed(t, fields, representative)
		a.folds(t, fields)
		a.merges(t, fields)
		a.sliceElements(t, fields)
		a.raws(t, fields, representative, rawRoots[t])
	}
	for _, root := range pathRoots {
		a.paths(root, root, func(inner string) string { return inner }, "", 1)
	}
	a.depths()
	return a.out
}

// pathRoots are the types whose nested field paths are exercised.
var pathRoots = []reflect.Type{
	reflect.TypeOf(openrtb2.BidRequest{}),
	reflect.TypeOf(openrtb2.BidResponse{}),
	reflect.TypeOf(request.Request{}),
	reflect.TypeOf(response.Response{}),
	reflect.TypeOf(openrtb3.Body{}),
}

// rawRoots get every RawMessage value, whatever their field count.
var rawRoots = map[reflect.Type]bool{
	reflect.TypeOf(openrtb2.BidRequest{}):  true,
	reflect.TypeOf(openrtb2.Imp{}):         true,
	reflect.TypeOf(openrtb2.Bid{}):         true,
	reflect.TypeOf(openrtb2.BidResponse{}): true,
	reflect.TypeOf(request.Request{}):      true,
	reflect.TypeOf(response.Response{}):    true,
}

type adv struct {
	out  []testCase
	seen map[string]bool
	// edged are the slice element types that have had their edge values.
	edged map[reflect.Type]bool
}

func (a *adv) add(t reflect.Type, label, doc string) {
	key := t.String() + "\x00" + doc
	if a.seen[key] {
		return
	}
	a.seen[key] = true
	a.out = append(a.out, testCase{ref: "'" + doc, data: []byte(doc), typ: t, label: label})
}

func quote(s string) string { return strconv.Quote(s) }

// structOf is t's struct type, through one pointer, or nil.
func structOf(t reflect.Type) reflect.Type {
	if t.Kind() == reflect.Ptr {
		t = t.Elem()
	}
	if t.Kind() == reflect.Struct {
		return t
	}
	return nil
}

// object is an object of struct type t with one field set: the first field to its first
// sample, or (last) the last field to its last sample.
func object(t reflect.Type, last bool) string {
	fields := jsonFields(t, nil)
	if len(fields) == 0 {
		return `{}`
	}
	f, v := fields[0], valuesFor(fields[0].typ).samples
	value := v[0]
	if last {
		f = fields[len(fields)-1]
		v = valuesFor(f.typ).samples
		value = v[len(v)-1]
	}
	return "{" + quote(f.key) + ":" + value + "}"
}

// malformed: broken syntax around the first known field of t, and (for representatives)
// around unknown fields.
func (a *adv) malformed(t reflect.Type, fields []jsonField, representative bool) {
	if representative {
		for i, doc := range []string{
			`{"zz":1 "yy":2}`, `{"zz":1,,"yy":2}`, `{,}`, `{"zz":1}}`, `{"zz":1]`, `{"zz"}`,
			`{"zz":}`, `{:1}`, `{1:1}`, `{"zz":1,"yy"`, `{"zz":"abc`, `{"zz`, `{"zz":nul}`,
			`{"zz":[1 2]}`, `{"zz":{"a" 1}}`, `{"zz":{"a":1,}}`, `{"zz":1e999}`, `{"zz":-}`,
			`{"zz":01}`, escapes(`{"zz":"a%x"}`), "{\"zz\":\"a\x01b\"}", escapes(`{"zz":"%u12"}`),
			`{"zz":tru}`, `{"zz":1.}`, `{"zz":[}`, `{"zz":{]}`, `{} x`, `{}{}`, `{]`, `[}`,
			`{"zz":[[[]]],"yy":{"a":{"b":{}}}}`, escapes(`{"z%u0000z":1}`),
		} {
			a.add(t, fmt.Sprintf("malformed %d", i), doc)
		}
	}
	if len(fields) == 0 {
		return
	}
	k, s := quote(fields[0].key), valuesFor(fields[0].typ).samples[0]
	for i, doc := range []string{
		"{" + k + ":" + s + ` "zz":1}`,
		"{" + k + ":" + s + `,"zz":1,}`,
		"{" + k + ":" + s,
		"{" + k + ":",
		"{" + k,
		"{" + k + ":" + s + "}x",
		"{" + k + ":" + s + "," + k,
		`{"zz":1,` + k + ":" + s + ` "yy":2}`,
		"{" + k + ":" + s + `,,"zz":1}`,
		"{" + k + " " + s + "}",
		"{" + k + ":" + s + "]",
		"{" + k + ":" + s + "}{" + k + ":" + s + "}",
	} {
		a.add(t, fmt.Sprintf("%s malformed %d", fields[0].key, i), doc)
	}
}

// specialFolds are runes that some decoder matches with an ASCII letter of a key.
var specialFolds = []struct {
	ascii   string
	special rune
}{
	{"s", 0x17f},  // LATIN SMALL LETTER LONG S: encoding/json folds it to s
	{"k", 0x212a}, // KELVIN SIGN: encoding/json folds it to k, strings.ToLower lowers it to k
	{"i", 0x130},  // LATIN CAPITAL LETTER I WITH DOT ABOVE: strings.ToLower lowers it to i
	{"i", 0x131},  // LATIN SMALL LETTER DOTLESS I: neither matches it with i
}

// folds: each key spelled with a special rune for one of its letters (escaped too, for the
// first such key of t), and the first key in two ASCII cases.
func (a *adv) folds(t reflect.Type, fields []jsonField) {
	escapedOne := false
	for n, f := range fields {
		value := valuesFor(f.typ).samples[0]
		for _, sf := range specialFolds {
			if !strings.Contains(f.key, sf.ascii) {
				continue
			}
			raw := strings.ReplaceAll(f.key, sf.ascii, string(sf.special))
			a.add(t, fmt.Sprintf("%s fold %U", f.key, sf.special), "{"+quote(raw)+":"+value+"}")
			if !escapedOne {
				escaped := strings.Replace(f.key, sf.ascii, fmt.Sprintf("%%u%04x", sf.special), 1)
				a.add(t, fmt.Sprintf("%s escaped fold %U", f.key, sf.special),
					"{"+escapes(`"`+escaped+`"`)+":"+value+"}")
			}
		}
		escapedOne = escapedOne || strings.ContainsAny(f.key, "ski")
		if n > 0 {
			continue
		}
		samples := valuesFor(f.typ).samples
		first, last := samples[0], samples[len(samples)-1]
		upper := quote(strings.ToUpper(f.key))
		a.add(t, f.key+" key then upper-case key", "{"+quote(f.key)+":"+first+","+upper+":"+last+"}")
		a.add(t, f.key+" upper-case key then key", "{"+upper+":"+first+","+quote(f.key)+":"+last+"}")
	}
}

// merges: a struct, pointer-to-struct, slice-of-struct or map field given twice or three times.
func (a *adv) merges(t reflect.Type, fields []jsonField) {
	for _, f := range fields {
		k := quote(f.key)
		member := func(values ...string) string {
			parts := make([]string, len(values))
			for i, v := range values {
				parts[i] = k + ":" + v
			}
			return "{" + strings.Join(parts, ",") + "}"
		}
		label := func(what string) string { return f.key + " " + what }
		if st := structOf(f.typ); st != nil {
			x, y := object(st, false), object(st, true)
			a.add(t, label("merge"), member(x, y))
			a.add(t, label("merge after null"), member(x, "null", y))
			a.add(t, label("merge into empty"), member("{}", x, "{}"))
			a.add(t, label("merge twice"), member(x, y, x))
			continue
		}
		switch f.typ.Kind() {
		case reflect.Slice:
			st := structOf(f.typ.Elem())
			if st == nil {
				continue
			}
			x, y := object(st, false), object(st, true)
			a.add(t, label("element merge"), member("["+x+"]", "["+y+"]"))
			a.add(t, label("element merge, shorter"), member("["+x+","+x+"]", "["+y+"]"))
			a.add(t, label("element merge, longer"), member("["+x+"]", "["+y+","+y+"]"))
			a.add(t, label("elements then empty"), member("["+x+"]", "[]"))
			a.add(t, label("elements then null then elements"), member("["+x+"]", "null", "["+y+"]"))
			a.add(t, label("stale element"), member("["+x+","+y+"]", "["+y+"]", "[{},{}]"))
		case reflect.Map:
			a.add(t, label("map merge"), member(`{"a":"x"}`, `{"b":"y"}`))
			a.add(t, label("map merge after null"), member(`{"a":"x"}`, "null", `{"b":"y"}`))
			a.add(t, label("map value null"), member(`{"a":"x"}`, `{"a":null}`))
			a.add(t, label("map keys"), member(escapes(`{"<":"x","é":"1","A":"2","":"e","%u2028":"l","b\"":"q"}`)))
		}
	}
}

// sliceElements: elements of scalar slices, first and second in the array: null and every wrong
// and edge value in the first slice of each element type, null and the first wrong value in the
// others.
func (a *adv) sliceElements(t reflect.Type, fields []jsonField) {
	for _, f := range fields {
		if f.typ.Kind() != reflect.Slice || f.typ == rawMessageType || structOf(f.typ.Elem()) != nil {
			continue
		}
		elem := f.typ.Elem()
		v := valuesFor(elem)
		values := append([]string{"null"}, v.wrong[:min(len(v.wrong), 1)]...)
		if !a.edged[elem] {
			values = append(append([]string{"null"}, v.wrong...), v.edges...)
		}
		a.edged[elem] = true
		for i, value := range values {
			if strings.Contains(value, "\xff") {
				continue // the documented String difference; the mutations pin it
			}
			k := quote(f.key)
			a.add(t, fmt.Sprintf("%s element %d", f.key, i), "{"+k+":["+value+"]}")
			a.add(t, fmt.Sprintf("%s second element %d", f.key, i), "{"+k+":["+v.samples[0]+","+value+"]}")
		}
	}
}

// rawValues are json.RawMessage contents: valid ones the encoders escape or compact
// differently, and invalid ones jsoniter's skipping and json.Compact treat differently.
var rawValues = []string{
	"{\"a\":\"<&>  \"}",
	escapes(`{"a":"%u2028%u003c"}`),
	`"<script>&amp;</script>"`,
	" { \"a\" :\t[ 1 ,\n2 ] ,\r\"b\" : { } } ",
	`{"a":1E5,"b":-0.0,"c":12345678901234567890123,"d":1e-7,"e":0.1e1}`,
	`{"a":1,"a":2}`,
	escapes(`{"a":"%/"}`),
	"{\"a\":\"\x01\"}",
	"{\"a\":\"\xff\"}",
	escapes(`{"a":"%ud800"}`),
	`nul`, `1.5e`, `{"a":[1,2}`, `"unterminated`, `{"a":tru}`, `-`, `[1 2]`, `{"a" 1}`,
	`01`, `1.`, `.5`, `+1`, `[,1]`, `{,}`, `[]]`, `{"a":1}}`, `"\x"`, `[[[[]]]]`,
}

// raws: RawMessage values in the first RawMessage field of t: all of them for the request and
// response roots, eight for representatives, two otherwise.
func (a *adv) raws(t reflect.Type, fields []jsonField, representative, root bool) {
	for _, f := range fields {
		if f.typ != rawMessageType {
			continue
		}
		values := rawValues
		if !root {
			n := 2
			if representative {
				n = 8
			}
			values = values[:n]
		}
		for i, value := range values {
			a.add(t, fmt.Sprintf("%s raw %d", f.key, i), "{"+quote(f.key)+":"+value+"}")
		}
		return
	}
}

// wrongValues are what paths puts at a field of type ft: a value of another JSON type, and an
// out-of-range number for integers.
func wrongValues(ft reflect.Type) []string {
	if ft == rawMessageType {
		return []string{`{"a":}`}
	}
	if ft.Kind() == reflect.Ptr {
		ft = ft.Elem()
	}
	switch ft.Kind() {
	case reflect.String:
		return []string{`1`}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		above := strconv.FormatUint(uint64(1)<<(ft.Bits()-1), 10)
		return []string{`"1"`, above}
	case reflect.Float64:
		return []string{`"1"`}
	case reflect.Slice:
		e := valuesFor(ft.Elem())
		out := []string{`{}`}
		if len(e.wrong) > 0 && structOf(ft.Elem()) == nil {
			out = append(out, "["+e.samples[0]+","+e.wrong[0]+"]")
		}
		return out
	case reflect.Map:
		return []string{`[]`, `{"a":1}`}
	case reflect.Struct:
		return []string{`[]`}
	}
	return nil
}

// paths puts each wrong value at every field path below t, up to five levels from the root,
// and descends into struct fields (into the second element of struct slices).
func (a *adv) paths(root, t reflect.Type, wrap func(string) string, path string, depth int) {
	if depth > 5 {
		return
	}
	for _, f := range jsonFields(t, nil) {
		k := quote(f.key)
		p := path + "." + f.key
		if depth > 1 {
			for i, w := range wrongValues(f.typ) {
				a.add(root, fmt.Sprintf("path %s wrong %d", p, i), wrap("{"+k+":"+w+"}"))
			}
		}
		if st := structOf(f.typ); st != nil {
			a.paths(root, st, func(inner string) string { return wrap("{" + k + ":" + inner + "}") }, p, depth+1)
		} else if f.typ.Kind() == reflect.Slice {
			if st := structOf(f.typ.Elem()); st != nil {
				a.paths(root, st, func(inner string) string { return wrap("{" + k + ":[{}," + inner + "]}") }, p+"[1]", depth+1)
			}
		}
	}
}

// depths: documents at the nesting limit of 10000, and one level over it, in a RawMessage, an
// unknown field and the RawMessage of a slice element.
func (a *adv) depths() {
	t := reflect.TypeOf(openrtb2.BidRequest{})
	arrays := func(n int) string { return strings.Repeat("[", n) + strings.Repeat("]", n) }
	for _, n := range []int{9999, 10000} {
		a.add(t, fmt.Sprintf("depth ext %d", n+1), `{"ext":`+arrays(n)+`}`)
		a.add(t, fmt.Sprintf("depth unknown %d", n+1), `{"zz":`+arrays(n)+`}`)
		a.add(t, fmt.Sprintf("depth imp ext %d", n+1), `{"imp":[{"ext":`+arrays(n-2)+`}]}`)
	}
}
