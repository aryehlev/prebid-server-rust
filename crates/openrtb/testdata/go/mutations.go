package main

import (
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"strconv"
	"strings"
	"unicode"
)

var rawMessageType = reflect.TypeOf(json.RawMessage(nil))

// escapes turns each "%" of s into a backslash. Documents with JSON escapes are written with "%"
// so that no tool that edits this file turns an escape into the character it stands for.
func escapes(s string) string {
	return strings.ReplaceAll(s, "%", string(rune(0x5c)))
}

// mutate returns the generated cases of one ported type: for a struct, documents that exercise
// each JSON field (sample values, null, wrong JSON types, edge values, key spellings and
// duplicates) and the struct as a whole; for a named type, its values as top-level documents.
func mutate(t reflect.Type) []testCase {
	var out []testCase
	add := func(label, doc string) {
		out = append(out, testCase{ref: "'" + doc, data: []byte(doc), typ: t, label: label})
	}
	for _, d := range [][2]string{
		{"null", "null"},
		{"empty document", ""},
		{"truncated", "{"},
		{"trailing data", "{} {}"},
		{"white space", " \n\t{} \r\n"},
		{"byte order mark", "\xef\xbb\xbf{}"},
	} {
		add(d[0], d[1])
	}
	if t.Kind() != reflect.Struct {
		v := valuesFor(t)
		for i, value := range append(append(v.samples, v.edges...), v.wrong...) {
			add(fmt.Sprintf("value %d", i), value)
		}
		add("object", "{}")
		return out
	}
	for _, d := range [][2]string{
		{"empty object", "{}"},
		{"array", "[]"},
		{"string", `"x"`},
		{"number", "1"},
		{"unknown field", `{"zzunknown":1}`},
		{"unknown nested field", escapes(`{"zz":{"a":[1,{"b":null}],"c":"%u00e9"},"yy":[]}`)},
		{"invalid UTF-8 in an unknown field", "{\"zz\":\"\xff\"}"},
		{"bad unknown field", `{"zz":[1,}`},
		{"missing colon", `{"zz" 1}`},
		{"trailing comma", `{"zz":1,}`},
		{"single-quoted key", `{'zz':1}`},
	} {
		add(d[0], d[1])
	}
	var every, zeros []string
	// The first field of each Go type gets every edge and wrong value; the others one of each.
	full := map[reflect.Type]bool{}
	for _, f := range jsonFields(t, nil) {
		v := valuesFor(f.typ)
		key := strconv.Quote(f.key)
		member := func(value string) string { return "{" + key + ":" + value + "}" }
		edges, wrong := v.edges, v.wrong
		if full[f.typ] {
			edges, wrong = nil, wrong[:min(len(wrong), 1)]
		}
		full[f.typ] = true
		for i, value := range v.samples {
			add(fmt.Sprintf("%s sample %d", f.key, i), member(value))
		}
		for i, value := range edges {
			add(fmt.Sprintf("%s edge %d", f.key, i), member(value))
		}
		for i, value := range wrong {
			add(fmt.Sprintf("%s wrong %d", f.key, i), member(value))
		}
		add(f.key+" null", member("null"))
		first, second := v.samples[0], v.samples[len(v.samples)-1]
		if upper := strings.ToUpper(f.key); upper != f.key {
			add(f.key+" upper-case key", "{"+strconv.Quote(upper)+":"+first+"}")
		}
		if title := titleCase(f.key); title != f.key && title != strings.ToUpper(f.key) {
			add(f.key+" title-case key", "{"+strconv.Quote(title)+":"+first+"}")
		}
		escaped := escapes(fmt.Sprintf(`"%%u%04x%s"`, f.key[0], f.key[1:]))
		add(f.key+" escaped key", "{"+escaped+":"+first+"}")
		add(f.key+" duplicate key", "{"+key+":"+first+","+key+":"+second+"}")
		add(f.key+" null then value", "{"+key+":null,"+key+":"+first+"}")
		add(f.key+" value then null", "{"+key+":"+first+","+key+":null}")
		every = append(every, key+":"+first)
		zeros = append(zeros, key+":"+v.zero)
	}
	if len(every) > 0 {
		add("every field", "{"+strings.Join(every, ",")+"}")
		add("every field zero", "{"+strings.Join(zeros, ",")+"}")
		add("every field reversed", "{"+strings.Join(reversed(every), ",")+"}")
	}
	return out
}

func reversed(s []string) []string {
	out := make([]string, len(s))
	for i, v := range s {
		out[len(s)-1-i] = v
	}
	return out
}

func titleCase(s string) string {
	r := []rune(s)
	if len(r) == 0 {
		return s
	}
	r[0] = unicode.ToUpper(r[0])
	return string(r)
}

// jsonField is a field as encoding/json sees it: its key and Go type.
type jsonField struct {
	key string
	typ reflect.Type
}

// jsonFields are the JSON fields of struct t (embedded structs' fields promoted), with the keys
// Go uses for them; fields hidden by another of the same key are left out.
func jsonFields(t reflect.Type, seen map[string]bool) []jsonField {
	if seen == nil {
		seen = map[string]bool{}
	}
	var out, embedded []jsonField
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		if !f.IsExported() && !f.Anonymous {
			continue
		}
		tag := f.Tag.Get("json")
		if tag == "-" {
			continue
		}
		name := strings.Split(tag, ",")[0]
		if f.Anonymous && name == "" {
			ft := f.Type
			if ft.Kind() == reflect.Ptr {
				ft = ft.Elem()
			}
			if ft.Kind() == reflect.Struct {
				embedded = append(embedded, jsonField{typ: ft})
				continue
			}
		}
		if name == "" {
			name = f.Name
		}
		if !seen[name] {
			seen[name] = true
			out = append(out, jsonField{key: name, typ: f.Type})
		}
	}
	for _, e := range embedded {
		out = append(out, jsonFields(e.typ, seen)...)
	}
	return out
}

// values are JSON values for a field of one Go type.
type values struct {
	samples []string // valid values
	edges   []string // values at the edges of the type (ranges, formats, escapes, bad syntax)
	wrong   []string // values of other JSON types
	zero    string   // the value that decodes to Go's zero (or empty) value
}

func valuesFor(t reflect.Type) values {
	if t == rawMessageType {
		return values{
			samples: []string{`{"a":1}`, `[1,"x"]`},
			edges: []string{
				`{ "a" : [ 1 , 2 ] }`, `"s"`, `1.50`, `true`, `[]`, `{}`, `""`, `{"a":}`, `[1,]`,
				escapes(`"%u00e9%u003c"`),
			},
			zero: `{}`,
		}
	}
	switch t.Kind() {
	case reflect.Ptr:
		return valuesFor(t.Elem())
	case reflect.String:
		return values{
			samples: []string{`"x"`, `"<a href=\"x\">&amp;</a>"`},
			edges: []string{
				`""`,
				`"` + string([]rune{0xe9, 0x1f600, 0x2028, 0x2029}) + `"`,
				escapes(`"%u00e9%ud83d%ude00%u2028%u2029"`),
				escapes(`"%ud800"`),
				escapes(`"%udc00x"`),
				escapes(`"%ud83d%u0041"`),
				escapes(`"%u0000%u001f%u007f"`),
				escapes(`"%u003c%u003e%u0026"`),
				escapes(`"%u12"`),
				`"tab\tnl\nq\"bs\\"`,
				`"\/\b\f\r"`,
				`"\x"`,
				"\"\xff\"",
				"\"a\tb\"",
			},
			wrong: []string{`1`, `true`, `{}`, `[]`},
			zero:  `""`,
		}
	case reflect.Bool:
		return values{
			samples: []string{`true`, `false`},
			edges:   []string{`tru`, `TRUE`},
			wrong:   []string{`"true"`, `1`, `{}`},
			zero:    `false`,
		}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		bits := t.Bits()
		max := int64(math.MaxInt64 >> (64 - bits))
		above := strconv.FormatUint(uint64(max)+1, 10)
		return values{
			samples: []string{`1`, `-2`},
			edges: []string{
				strconv.FormatInt(max, 10), "-" + above, above, "-" + plusOne(above),
				`0`, `-0`, `1.0`, `1e2`, `1.5`, `01`, `-`, `99999999999999999999999`,
			},
			wrong: []string{`"1"`, `true`, `{}`, `[]`},
			zero:  `0`,
		}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		bits := t.Bits()
		max := uint64(math.MaxUint64 >> (64 - bits))
		return values{
			samples: []string{`1`, `2`},
			edges: []string{
				strconv.FormatUint(max, 10), plusOne(strconv.FormatUint(max, 10)),
				`0`, `-1`, `1.0`, `1e2`,
			},
			wrong: []string{`"1"`, `true`, `{}`, `[]`},
			zero:  `0`,
		}
	case reflect.Float32, reflect.Float64:
		return values{
			samples: []string{`1.5`, `-0.25`},
			edges: []string{
				`0`, `-0`, `100`, `0.1`, `1e20`, `1e21`, `1e-6`, `1e-7`, `123456789.123456789`,
				`5e-324`, `1.7976931348623157e308`, `1e309`, `3.4028235e38`, `3.5e38`, `1E+2`,
				`1.`, `.5`, `1e`,
			},
			wrong: []string{`"1"`, `true`, `{}`, `[]`},
			zero:  `0`,
		}
	case reflect.Slice:
		e := valuesFor(t.Elem())
		first, last := e.samples[0], e.samples[len(e.samples)-1]
		v := values{
			samples: []string{"[" + first + "]", "[" + first + "," + last + "]"},
			edges:   []string{`[]`, `[null]`, `[ ` + first + ` , ` + first + ` ]`, `[` + first + `,]`},
			wrong:   []string{`{}`, `"x"`, `1`},
			zero:    `[]`,
		}
		if len(e.wrong) > 0 {
			v.wrong = append(v.wrong, "["+first+","+e.wrong[0]+"]")
		}
		return v
	case reflect.Map:
		return values{
			samples: []string{`{"a":"x"}`, `{"b":"1","a":"2"}`},
			edges:   []string{`{}`, `{"a":null}`, `{"a":"x","a":"y"}`, escapes(`{"%u0061":"x"}`)},
			wrong:   []string{`[]`, `"x"`, `{"a":1}`},
			zero:    `{}`,
		}
	case reflect.Struct:
		return values{
			samples: []string{`{}`, `{"zz":1}`},
			wrong:   []string{`[]`, `"x"`, `1`, `true`},
			zero:    `{}`,
		}
	case reflect.Interface:
		return values{
			samples: []string{`{"a":[1,"x",null]}`, `1.5`},
			edges:   []string{`"s"`, `true`, `[]`, `1e400`, `{"b":2,"a":1}`},
			zero:    `null`,
		}
	default:
		panic("no JSON values for " + t.String())
	}
}

// plusOne adds one to a decimal number of digits.
func plusOne(digits string) string {
	b := []byte(digits)
	for i := len(b) - 1; i >= 0; i-- {
		if b[i] < '9' {
			b[i]++
			return string(b)
		}
		b[i] = '0'
	}
	return "1" + string(b)
}
