package main

import (
	"encoding/json"
	"math"
	"reflect"
	"strings"

	"github.com/mitchellh/mapstructure"
	"github.com/spf13/cast"
	"gopkg.in/yaml.v3"
)

// result is a value or the error that took its place.
type result struct {
	Value any    `json:"value"`
	Error string `json:"error,omitempty"`
}

func resultOf(v any, err error) result {
	if err != nil {
		return result{Error: err.Error()}
	}
	return result{Value: v}
}

// yamlScalar is how yaml.v3 resolves one scalar under a key, untyped (into interface{}), as
// Viper reads pbs.yaml.
type yamlScalar struct {
	Text string `json:"text"`
	result
}

// yamlDoc is how yaml.v3 reads a whole document into map[string]interface{}, as Viper's YAML
// codec does (viper internal/encoding/yaml), and into interface{}.
type yamlDoc struct {
	Name  string `json:"name"`
	Text  string `json:"text"`
	Map   result `json:"map"`
	Value result `json:"value"`
}

// castRow is one value through the cast functions Viper's Get applies by the type of a key's
// default (viper.go:884-935): ToBool, ToString, ToInt, ToFloat64 and ToStringSlice. Get drops
// the error and keeps the zero value.
type castRow struct {
	Input         any    `json:"input"`
	ToBool        result `json:"to_bool"`
	ToInt         result `json:"to_int"`
	ToFloat64     result `json:"to_float64"`
	ToString      result `json:"to_string"`
	ToStringSlice result `json:"to_string_slice"`
}

// weakRow is one value decoded by mapstructure with WeaklyTypedInput and no hook, as
// v.Unmarshal decodes every value that is not account modules, into one target type.
type weakRow struct {
	Input  any    `json:"input"`
	Target string `json:"target"`
	result
}

// scalarsDoc is scalars.json.
type scalarsDoc struct {
	Header      header       `json:"header"`
	Notes       []string     `json:"notes"`
	YAMLScalars []yamlScalar `json:"yaml_scalars"`
	YAMLDocs    []yamlDoc    `json:"yaml_docs"`
	Cast        []castRow    `json:"cast"`
	Weak        []weakRow    `json:"weak"`
}

// yamlScalarTexts are plain, quoted and tagged scalars.
var yamlScalarTexts = []string{
	"true", "True", "TRUE", "tRuE", "false", "False", "FALSE",
	"yes", "Yes", "YES", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "y", "Y", "n", "N",
	"~", "null", "Null", "NULL", "nUll", "",
	"0", "-0", "+1", "007", "08", "09", "0o17", "0O17", "0o8", "0x1F", "0X1F", "0x", "0b101", "0B101",
	"-0b101", "+0b1", "1_000", "_1", "1_", "1__0", "0x_1F", "-0x1F", "+0x1F", "0777", "-017", "+017",
	"9223372036854775807", "9223372036854775808", "18446744073709551615", "18446744073709551616",
	"-9223372036854775808", "-9223372036854775809",
	"1.5", ".5", "5.", "1e3", "1E3", "1.0e+3", "-1.5e-3", "1e", "e3", ".inf", "-.inf", "+.inf", ".Inf", ".INF",
	".nan", ".NaN", ".NAN", "inf", "nan", "Infinity", "1_000.5", "0.1e-2", "+.5", "-.5", "685.230_15e+03",
	"190:20:30.15", "1e400", "1e-400", "0.0", "-0.0", "1.", "0.1",
	"2001-12-14", "2001-12-14t21:59:43.10-05:00", "2001-12-14 21:59:43.10 -5",
	"abc", "\"quoted\"", "'single'", "\"1\"", "'1'", "\"true\"", "\"~\"", "\"\"", "''", "\"a\\tb\"",
	"a#b", "a #b", "=", "<<", "!!str 1", "!!str true", "!!int \"1\"", "!!int x", "!!float 1", "!!bool \"true\"",
	"!!bool yes", "!!null x", "!!binary aGVsbG8=", "&a 1", "!custom x",
}

// yamlDocCases are whole documents: the forms that parse, and the ones that fail.
var yamlDocCases = []struct{ name, text string }{
	{"empty", ""},
	{"comment only", "# c\n"},
	{"null document", "~\n"},
	{"scalar document", "5\n"},
	{"list document", "- a\n"},
	{"flat mapping", "a: 1\nb: x\n"},
	{"nested mapping", "a:\n  b:\n    c: 1\n"},
	{"indented document", "  a: 1\n  b: 2\n"},
	{"tab indent", "a:\n\tb: 1\n"},
	{"tab after a value", "a: 1\t\nb: 2\n"},
	{"unclosed flow sequence", "a: [1\n"},
	{"unclosed quote", "a: \"x\n"},
	{"bad escape", "a: \"\\q\"\n"},
	{"duplicate key", "a: 1\na: 2\n"},
	{"duplicate nested key", "a:\n  b: 1\n  b: 2\n"},
	{"duplicate key in a list item", "a:\n  - b: 1\n    b: 2\n"},
	{"duplicate key across case", "a: 1\nA: 2\n"},
	{"several documents", "a: 1\n---\na: 2\n"},
	{"document end marker", "a: 1\n...\n"},
	{"directive", "%YAML 1.2\n---\na: 1\n"},
	{"yaml 1.1 directive", "%YAML 1.1\n---\na: yes\n"},
	{"merge key", "base: &b\n  x: 1\n  y: 2\nc:\n  <<: *b\n  y: 3\n"},
	{"merge key list", "b1: &b1\n  x: 1\nb2: &b2\n  y: 2\nc:\n  <<: [*b1, *b2]\n"},
	{"merge key of a scalar", "c:\n  <<: 1\n"},
	{"alias of a scalar", "a: &x 1\nb: *x\n"},
	{"undefined alias", "a: *nope\n"},
	{"byte order mark", "\ufeffa: 1\n"},
	{"crlf", "a: 1\r\nb: 2\r\n"},
	{"cr only", "a: 1\rb: 2\r"},
	{"invalid utf-8", "a: \xff\n"},
	{"nul byte", "a: \x00\n"},
	{"integer key", "1: a\n"},
	{"bool key", "true: a\n"},
	{"null key", "~: a\n"},
	{"float key", "1.50: a\n"},
	{"hex key", "0x1F: a\n"},
	{"nested non-string keys", "m:\n  1: a\n  true: b\n  1.50: c\n  0x1F: d\n"},
	{"nested null key", "m:\n  ~: a\n"},
	{"complex key", "? [a]\n: b\n"},
	{"explicit key", "? a\n: b\n"},
	{"long implicit key", strings.Repeat("k", 1100) + ": v\n"},
	{"flow mapping", "{a: 1, b: [x, y]}\n"},
	{"block scalars", "a: |\n  x\n  y\nb: >\n  x\n  y\nc: |-\n  z\n"},
	{"key with spaces", "a b: 1\n"},
	{"key with colon", "\"a:b\": 1\n"},
	{"deep nesting", strings.Repeat("[", 60) + strings.Repeat("]", 60) + "\n"},
	{"empty value", "a:\nb: 1\n"},
	{"empty flow values", "a: {}\nb: []\n"},
	{"tagged mapping", "a: !!map {b: 1}\n"},
	{"set", "a: !!set {x, y}\n"},
	{"omap", "a: !!omap [b: 1]\n"},
	{"comment after value", "a: 1 # c\nb: x#y\n"},
	{"trailing spaces", "a: x   \n"},
	{"quoted multiline", "a: \"x\n  y\"\n"},
}

// castInputs are Go values as a file or the environment gives them to Viper's Get.
var castInputs = []any{
	"", "true", "TRUE", "True", "tRuE", "t", "T", "1", "0", "f", "F", "false", "yes", "on", "y", "maybe",
	" 1", "1 ", "1.0", "1.00", "1.5", "-1.0", "0x1F", "0X1F", "0o17", "017", "0b11", "1_000", "_1", "+5", "-5", "1e3",
	"9223372036854775807", "9223372036854775808", "-9223372036854775809", "NaN", "Inf", "-inf", "infinity",
	"a b", "a,b", " a  b ", "a\tb\nc", "\u00a0a\u2003b",
	0, 1, -1, 42, int64(1), int64(0), uint64(18446744073709551615),
	0.0, 1.0, 1.5, -1.5, 1e21, math.NaN(), math.Inf(1),
	true, false, nil,
	[]any{"a", 1, nil, true, 1.5}, []any{}, []string{"a", "b"},
	map[string]any{"a": 1}, map[string]any{},
}

// weakTargets are the field types Configuration has, and a few more of mapstructure's kinds.
var weakTargets = []struct {
	name string
	typ  reflect.Type
}{
	{"bool", reflect.TypeOf(false)},
	{"int", reflect.TypeOf(int(0))},
	{"int8", reflect.TypeOf(int8(0))},
	{"int16", reflect.TypeOf(int16(0))},
	{"int32", reflect.TypeOf(int32(0))},
	{"int64", reflect.TypeOf(int64(0))},
	{"uint", reflect.TypeOf(uint(0))},
	{"uint8", reflect.TypeOf(uint8(0))},
	{"uint16", reflect.TypeOf(uint16(0))},
	{"uint32", reflect.TypeOf(uint32(0))},
	{"uint64", reflect.TypeOf(uint64(0))},
	{"float32", reflect.TypeOf(float32(0))},
	{"float64", reflect.TypeOf(float64(0))},
	{"string", reflect.TypeOf("")},
	{"[]string", reflect.TypeOf([]string(nil))},
	{"[][]string", reflect.TypeOf([][]string(nil))},
	{"[]int", reflect.TypeOf([]int(nil))},
	{"[2]string", reflect.TypeOf([2]string{})},
	{"[]uint8", reflect.TypeOf([]byte(nil))},
	{"json.RawMessage", reflect.TypeOf(json.RawMessage(nil))},
	{"map[string]string", reflect.TypeOf(map[string]string(nil))},
	{"map[string]interface {}", reflect.TypeOf(map[string]any(nil))},
	{"map[string]bool", reflect.TypeOf(map[string]bool(nil))},
	{"interface {}", reflect.TypeOf((*any)(nil)).Elem()},
	{"*bool", reflect.TypeOf((*bool)(nil))},
	{"*int", reflect.TypeOf((*int)(nil))},
	{"struct", reflect.TypeOf(weakStruct{})},
	{"*struct", reflect.TypeOf((*weakStruct)(nil))},
	{"[]struct", reflect.TypeOf([]weakStruct(nil))},
}

// weakStruct is a struct target with one tagged and one untagged field.
type weakStruct struct {
	Tagged int    `mapstructure:"tagged"`
	Plain  string // mapstructure looks it up as "Plain", in any case
}

// weakInputs are castInputs and a few maps a struct and a map target take.
func weakInputs() []any {
	return append(append([]any{}, castInputs...),
		map[string]any{"tagged": "5", "plain": 1},
		map[string]any{"TAGGED": 2, "Plain": "x", "other": true},
		map[string]any{"tagged": []any{1}},
		[]any{map[string]any{"tagged": 1}, map[string]any{"plain": "p"}},
		[]any{[]any{"a", "b"}, "c"},
		[]any{"x", "y", "z"},
	)
}

func weakDecode(input any, typ reflect.Type) (any, error) {
	out := reflect.New(typ)
	dec, err := mapstructure.NewDecoder(&mapstructure.DecoderConfig{
		Result:           out.Interface(),
		WeaklyTypedInput: true,
	})
	if err != nil {
		return nil, err
	}
	if err := dec.Decode(input); err != nil {
		return nil, err
	}
	return typed(out.Elem()), nil
}

func scalarsGolden() scalarsDoc {
	doc := scalarsDoc{
		Header: newHeader("gopkg.in/yaml.v3", "github.com/spf13/cast", "github.com/mitchellh/mapstructure"),
		Notes: []string{
			"Written by crates/config/testdata/go (go run .); do not edit.",
			"yaml_scalars: yaml.v3 unmarshals \"v: <text>\" into map[string]interface{}; value is v in the compact form of encode.go.",
			"yaml_docs: yaml.v3 unmarshals the document into map[string]interface{} (Viper's YAML codec) and into interface{}.",
			"cast: the cast v1.5.0 functions Viper's Get applies by the type of a key's default; Get drops the error and keeps the zero value.",
			"weak: mapstructure v1.5.0 with WeaklyTypedInput and no hook into the target type; the value is in the typed form of encode.go (a struct as its non-zero fields).",
		},
	}
	for _, text := range yamlScalarTexts {
		var m map[string]any
		err := yaml.Unmarshal([]byte("v: "+text+"\n"), &m)
		doc.YAMLScalars = append(doc.YAMLScalars, yamlScalar{Text: text, result: resultOf(dyn(m["v"]), err)})
	}
	for _, c := range yamlDocCases {
		var m map[string]any
		mErr := yaml.Unmarshal([]byte(c.text), &m)
		var v any
		vErr := yaml.Unmarshal([]byte(c.text), &v)
		row := yamlDoc{Name: c.name, Text: c.text, Map: result{Error: errText(mErr)}, Value: result{Error: errText(vErr)}}
		if mErr == nil {
			row.Map.Value = dyn(m)
		}
		if vErr == nil {
			row.Value.Value = dyn(v)
		}
		doc.YAMLDocs = append(doc.YAMLDocs, row)
	}
	for _, in := range castInputs {
		b, bErr := cast.ToBoolE(in)
		i, iErr := cast.ToIntE(in)
		f, fErr := cast.ToFloat64E(in)
		s, sErr := cast.ToStringE(in)
		ss, ssErr := cast.ToStringSliceE(in)
		doc.Cast = append(doc.Cast, castRow{
			Input:         dyn(in),
			ToBool:        resultOf(b, bErr),
			ToInt:         resultOf(i, iErr),
			ToFloat64:     resultOf(floatValue(f), fErr),
			ToString:      resultOf(s, sErr),
			ToStringSlice: resultOf(dyn(ss), ssErr),
		})
	}
	for _, in := range weakInputs() {
		for _, target := range weakTargets {
			v, err := weakDecode(in, target.typ)
			doc.Weak = append(doc.Weak, weakRow{Input: dyn(in), Target: target.name, result: resultOf(v, err)})
		}
	}
	return doc
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
