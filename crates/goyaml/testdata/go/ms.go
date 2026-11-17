package main

import (
	"fmt"
	"math"
	"math/rand"
	"time"
)

// msCase is one mapstructure.txt input, decoded under every configuration of msConfigNames.
type msCase struct {
	name    string
	target  string
	prefill *interface{}
	input   interface{}
}

// msConfigNames are the configurations of a mapstructure.txt row, in column order. A result
// equal to the "viper" one is written as "=" (with an empty error).
var msConfigNames = []string{"strict", "weak", "viper", "viper-unused", "viper-zero", "prebid"}

type m = map[string]interface{}

func msCases() []msCase {
	var cases []msCase
	add := func(name, target string, input interface{}) {
		cases = append(cases, msCase{name: name, target: target, input: input})
	}
	prefilled := func(name, target string, prefill, input interface{}) {
		cases = append(cases, msCase{name: name, target: target, prefill: &prefill, input: input})
	}

	// The input shapes of mapstructure_test.go.
	basic := m{"vstring": "foo", "vint": 42, "Vuint": 42, "vbool": true, "Vfloat": 42.42,
		"vsilent": true, "vdata": 42}
	add("TestBasicTypes", "Basic", basic)
	add("TestBasic_IntWithFloat", "Basic", m{"vint": float64(42)})
	prefilled("TestBasic_Merge", "Basic", m{"vstring": "foo", "vint": 1}, m{"vint": 42})
	add("TestBasic_Struct", "Basic", m{"vdata": m{"vstring": "foo"}})
	add("TestDecode_BasicSquash", "BasicSquash", m{"vstring": "foo"})
	embedded := m{"vstring": "foo", "Basic": m{"vstring": "innerfoo"}, "vunique": "bar"}
	add("TestDecode_Embedded", "Embedded", embedded)
	add("TestDecode_EmbeddedPointer", "EmbeddedPointer", embedded)
	add("TestDecode_EmbeddedSlice", "EmbeddedSlice", m{"slice_alias": []string{"foo", "bar"}, "vunique": "bar"})
	add("TestDecode_EmbeddedNoSquash", "Embedded", m{"vstring": "foo", "vunique": "bar"})
	add("TestDecode_EmbeddedPointerNoSquash", "EmbeddedPointer", m{"vstring": "foo", "vunique": "bar"})
	add("TestDecode_EmbeddedSquash", "EmbeddedSquash", m{"vstring": "foo", "vunique": "bar"})
	add("TestDecode_EmbeddedAndNamed", "EmbeddedAndNamed", m{"vstring": "foo", "vunique": "bar",
		"Named": m{"vstring": "baz"}})
	add("TestDecode_DecodeHook", "Basic", m{"vint": "WHAT"})
	add("TestDecode_Nil", "Basic", nil)
	add("TestDecode_NilPointerHook", "BasicPointer", m{"vstring": nil, "vint": nil})
	add("TestDecode_NonStruct", "map[string]string", m{"foo": "bar", "bar": "baz"})
	add("TestDecode_TypeConversion", "TypeConversionResult", m{
		"IntToFloat": 42, "IntToUint": 42, "IntToBool": 1, "IntToString": 42,
		"UintToInt": uint(42), "UintToFloat": uint(42), "UintToBool": uint(42),
		"UintToString": uint(42), "BoolToInt": true, "BoolToUint": true, "BoolToFloat": true,
		"BoolToString": true, "FloatToInt": 42.42, "FloatToUint": 42.42, "FloatToBool": 42.42,
		"FloatToString": 42.42, "SliceUint8ToString": []uint8("foo"),
		"StringToSliceUint8": "foo", "StringToInt": "42", "StringToUint": "42",
		"StringToBool": "1", "StringToFloat": "42.42", "StringToStrSlice": "A",
		"StringToIntSlice": "42", "SliceToMap": []interface{}{}, "MapToSlice": m{},
	})
	add("TestDecoder_ErrorUnused", "Basic", m{"vstring": "hello", "foo": "bar"})
	add("TestDecoder_ErrorUnused_NotSetable", "Basic", m{"vsilent": true})
	add("TestMap", "Map", m{"vfoo": "foo", "vother": map[interface{}]interface{}{"foo": "foo", "bar": "bar"}})
	prefilled("TestMapMerge", "Map", m{"vother": m{"existing": "x"}},
		m{"vfoo": "foo", "vother": m{"foo": "foo", "bar": "bar"}})
	add("TestMapOfStruct", "MapOfStruct", m{"value": m{"foo": map[string]string{"vstring": "one"},
		"bar": map[string]string{"vstring": "two"}}})
	nested := m{"vfoo": "foo", "vbar": m{"vstring": "foo", "vint": 42, "vbool": true}}
	add("TestNestedType", "NestedBasic", nested)
	add("TestNestedTypePointer", "NestedPointer", nested)
	add("TestSlice", "Slice", m{"vfoo": "foo", "vbar": []string{"foo", "bar", "baz"}})
	prefilled("TestSliceMerge", "Slice", m{"vbar": []string{"a", "b", "c"}}, m{"vbar": []string{"x"}})
	add("TestInvalidSlice", "Slice", m{"vfoo": "foo", "vbar": 42})
	add("TestSliceOfStruct", "SliceOfStruct", m{"value": []m{{"vstring": "one"}, {"vstring": "two"}}})
	add("TestSliceCornerCases", "[]interface {}", []interface{}(nil))
	add("TestSliceCornerCases2", "[]interface {}", []interface{}{})
	add("TestSliceToMap", "map[string]interface {}", []m{{"foo": "bar"}, {"bar": "baz"}})
	add("TestSliceToMapError", "map[string]interface {}", []interface{}{m{"foo": "bar"}, 42})
	add("TestInvalidType", "Basic", m{"vstring": 42})
	add("TestInvalidType2", "Basic", m{"vuint": -42})
	add("TestInvalidType3", "Basic", m{"vuint": -42.0})
	add("TestTagged", "Tagged", m{"foo": "bar", "bar": "value"})
	add("TestWeakDecode", "Kitchen", m{"int": "4", "string": 5})
	add("TestRemainder", "Remainder", m{"A": "hello", "B": "goodbye", "C": 3})
	add("TestRemainderNone", "Remainder", m{"A": "hello"})
	add("TestDecode_weakEmptyStringToInt", "Basic", m{"vint": "", "vuint": "", "vfloat": "", "vbool": ""})
	add("TestSlicePointer", "SlicePointer", m{"vbar": []string{"foo", "bar"}})
	add("TestSliceOfAlias", "SliceOfAlias", m{"vfoo": "foo", "vbar": []string{"a", "b"}})
	add("TestBasicPointer", "BasicPointer", m{"vstring": "foo", "vint": 42, "vuint": 42,
		"vbool": true, "vfloat": 42.42, "vextra": nil, "vdata": []interface{}{1, 2}})
	add("top-level string", "Basic", "foo")
	add("top-level slice", "Basic", []interface{}{1})
	add("interface keys", "Basic", map[interface{}]interface{}{"vint": 1, "vstring": "a"})
	add("int keys", "Basic", map[int]interface{}{1: "a"})
	add("bool keys", "Basic", map[bool]interface{}{true: "a"})
	add("unused interface key", "Basic", map[interface{}]interface{}{"vint": 1, 2: "b"})
	add("case-insensitive", "Basic", m{"VSTRING": "a", "VINT": "3"})
	add("exact match first", "Basic", m{"Vstring": "exact", "vstring": "lower"})
	add("nested errors", "NestedBasic", m{"vbar": m{"vint": "x", "vuint": -1, "vbool": "maybe"}})
	add("slice errors", "SliceOfStruct", m{"value": []interface{}{m{"vint": "x"}, 5, m{"vuint": "y"}}})
	add("map errors", "MapOfStruct", m{"value": m{"a": m{"vint": "x"}, "b": 5}})

	// Primitive targets, empty and holding a value.
	for i, v := range msSamples() {
		for _, target := range msPrimitiveTargets {
			add(fmt.Sprintf("primitive/%s/%d", target, i), target, v)
		}
		for _, p := range msPrimitivePrefills {
			prefilled(fmt.Sprintf("primitive-prefilled/%s/%d", p.name, i), p.target, p.value, v)
		}
	}

	// Every sample value under every Kitchen key, and over a value already in the field.
	for _, key := range msKitchenKeys {
		for i, v := range msSamples() {
			add(fmt.Sprintf("kitchen/%s/%d", key, i), "Kitchen", m{key: v})
		}
	}
	for _, p := range msKitchenPrefills {
		for i, v := range msSamples() {
			prefilled(fmt.Sprintf("kitchen-prefilled/%s/%d", p.key, i), "Kitchen", m{p.key: p.value},
				m{p.key: v})
		}
	}

	// Random trees of the sample values under Kitchen's and BidderInfo's keys.
	rnd := rand.New(rand.NewSource(20261118))
	treeKeys := append(append([]string(nil), msKitchenKeys...), "a", "b", "ptr", "zz")
	for i := 0; i < msRandomTrees; i++ {
		add(fmt.Sprintf("random/Kitchen/%d", i), "Kitchen", msTree(rnd, treeKeys, 0))
	}
	for i := 0; i < msRandomTrees; i++ {
		add(fmt.Sprintf("random/BidderInfo/%d", i), "BidderInfo", msTree(rnd, msBidderKeys, 0))
	}
	prefilled("kitchen/prefilled", "Kitchen",
		m{"ints": []int{1, 2, 3}, "strmap": m{"a": "1"}, "nested": m{"a": 1, "b": []string{"x"}},
			"ptrint": 5, "any": m{"k": "v"}, "anys": []interface{}{1, 2}},
		m{"ints": []int{9}, "strmap": m{"b": "2"}, "nested": m{"b": []string{"y"}}, "ptrint": nil,
			"any": m{"k2": "v2"}, "anys": []interface{}{"x"}})
	// config.AccountModules, which AccountModulesHookFunc converts to JSON under the prebid
	// configuration (each input has at most one entry it fails on, since the hook ranges over
	// the map in random order); the others decode the values as []byte.
	for i, input := range msModules() {
		add(fmt.Sprintf("modules/%d", i), "modulesHolder", input)
	}
	prefilled("kitchen/prefilled-nils", "Kitchen",
		m{"ints": []int{1}, "strmap": m{"a": "1"}, "nested": m{"a": 1}, "string": "s"},
		m{"ints": nil, "strmap": nil, "nested": nil, "string": nil})
	return cases
}

// msModules are the inputs of the config.AccountModules cases.
func msModules() []interface{} {
	vendor := func(v interface{}) interface{} { return m{"modules": m{"vendor": v}} }
	return []interface{}{
		m{"modules": nil}, m{"modules": m(nil)}, m{"modules": m{}},
		vendor(nil), vendor(m(nil)), vendor(m{}), vendor(m{"enabled": true}),
		m{"modules": m{"vendor": m{"a": 1, "b": 2.5, "c": "x<y>&", "d": nil},
			"other": m{"e": false}}},
		vendor(m{"list": []interface{}{1, "two", m{"k": "v"}, nil, []interface{}{}}}),
		vendor(m{"nested": m{"z": 1, "a": m{"b": []interface{}{1.5, 1e21, 1e-7, -0.0}}}}),
		vendor(m{"ints": []interface{}{map[interface{}]interface{}{2: "v", 10: "w"}},
			"bools":  map[interface{}]interface{}{true: 1, false: 0},
			"floats": map[interface{}]interface{}{1.5: "a", 1e21: "b"}}),
		vendor(m{"typed": []interface{}{uint8(7), int8(-3), float32(0.1), time.Duration(5)}}),
		vendor(m{"strs": map[string]string{"a": "b"}, "bytes": []uint8("hi"),
			"empty": []uint8{}, "nilbytes": []uint8(nil)}),
		vendor("oops"), vendor(map[string]string{"a": "b"}), vendor([]interface{}{m{"a": 1}}),
		vendor(m{"bad": math.NaN()}), vendor(m{"bad": []interface{}{1, math.Inf(1)}}),
		vendor(m{"bad": m{"x": math.Inf(-1)}}),
		vendor(m{"bad": []interface{}{[]interface{}{math.NaN()}, 2}}),
		m{"modules": "abc"}, m{"modules": 5}, m{"modules": []interface{}{}},
		m{"modules": []interface{}{m{"vendor": m{"a": 1}}, m{"other": m{"b": "2"}}}},
		m{"modules": []interface{}{m{"vendor": m{"a": 1}}, nil, "x"}},
		m{"modules": m{"vendor": m{"a": 1}}, "extra": 1},
		m{"Modules": m{"vendor": m{"a": "1,2"}}},
	}
}

// msPrimitiveTargets are the target types every sample value is decoded into.
var msPrimitiveTargets = []string{
	"string", "int", "*int", "bool", "float64", "time.Duration", "[]string", "interface {}",
	"map[string]interface {}", "map[string]string", "int8", "uint8", "uint", "float32", "[]int",
	"[]uint8", "[]interface {}", "map[string]int", "map[int]string", "map[string][]string",
	"[]map[string]string", "time.Time", "*time.Time", "Nested", "*Nested", "**int",
}

// msPrimitivePrefills are targets holding a value (decoded from value) before a sample is
// decoded into them. An interface{} holds scalars only: decoding into one holding a slice or a
// map is a known gap of the Rust port.
var msPrimitivePrefills = []struct {
	name, target string
	value        interface{}
}{
	{"map[string]string", "map[string]string", m{"p": "q"}},
	{"map[string]interface {}", "map[string]interface {}", m{"p": 1}},
	{"map[string]int", "map[string]int", m{"p": 1}},
	{"map[int]string", "map[int]string", map[int]string{1: "a"}},
	{"[]string", "[]string", []string{"p", "q"}},
	{"[]interface {}", "[]interface {}", []interface{}{1, "s"}},
	{"*int", "*int", 5},
	{"**int", "**int", 5},
	{"interface {}=int", "interface {}", 5},
	{"interface {}=string", "interface {}", "s"},
	{"interface {}=bool", "interface {}", true},
	{"interface {}=float64", "interface {}", 2.5},
	{"interface {}=time.Time", "interface {}", time.Date(2001, 12, 14, 0, 0, 0, 0, time.UTC)},
	{"Nested", "Nested", m{"a": 1, "b": []string{"x"}, "ptr": m{"a": 2}}},
	{"*Nested", "*Nested", m{"a": 1, "b": []string{"x"}}},
	{"time.Time", "time.Time", time.Date(2001, 12, 14, 0, 0, 0, 0, time.UTC)},
}

// msKitchenPrefills are Kitchen fields holding a value (decoded from {key: value}) before a
// sample is decoded under the same key.
var msKitchenPrefills = []struct {
	key   string
	value interface{}
}{
	{"ptrint", 5}, {"ptrstr", "s"}, {"ptrbool", true}, {"ptrdur", 7}, {"ptrptr", 9},
	{"ints", []int{1, 2, 3}}, {"strings", []string{"p", "q", "r"}}, {"bytes", []byte("xyz")},
	{"any", 5}, {"anys", []interface{}{1, "s", 2.5}}, {"strmap", m{"p": "q"}},
	{"intmap", m{"p": 1}}, {"anymap", m{"p": 1}}, {"keymap", map[int]string{1: "a"}},
	{"nested", m{"a": 1, "b": []string{"x"}, "ptr": m{"a": 2}}},
	{"value", m{"a": 1, "b": []string{"x"}}},
	{"nesteds", []interface{}{m{"a": 1}, m{"a": 2, "b": []string{"y"}}}},
	{"bidtypes", []string{"banner", "video"}},
	{"time", time.Date(2001, 12, 14, 0, 0, 0, 0, time.UTC)}, {"duration", 5}, {"inb", 3},
	{"zz", "remain"},
}

// msRandomTrees is the number of random trees per target.
const msRandomTrees = 1000

// msBidderKeys are config.BidderInfo's mapstructure keys at every level (some only differing
// from the yaml ones), a few in other cases (each folds to one field name only, since
// mapstructure picks a random match among several), and a key it does not have.
var msBidderKeys = []string{
	"aliasOf", "whiteLabelOnly", "disabled", "endpoint", "extra_info", "openrtb", "maintainer",
	"capabilities", "modifyingVastXmlAllowed", "debug", "geoscope", "gvlVendorID", "userSync",
	"experiment", "xapi", "platform_id", "app_secret", "endpointCompression", "email", "app",
	"site", "dooh", "mediaTypes", "allow", "username", "password", "tracker", "version",
	"gpp-supported", "multiformat-supported", "key", "supports", "iframe", "redirect",
	"external_url", "externalUrl", "format_override", "enabled", "skipwhen", "gdpr", "gpp_sid",
	"url", "redirect_url", "user_macro", "adsCert", "Disabled", "ENDPOINT", "usersync", "other",
}

// msTree is a random map of keys (at the top) or slice, nested up to three levels, with sample
// values for leaves.
func msTree(rnd *rand.Rand, keys []string, depth int) interface{} {
	samples := msSamples()
	if depth > 0 && (depth >= 3 || rnd.Intn(3) == 0) {
		return samples[rnd.Intn(len(samples))]
	}
	if depth > 0 && rnd.Intn(4) == 0 {
		items := make([]interface{}, rnd.Intn(4))
		for i := range items {
			items[i] = msTree(rnd, keys, depth+1)
		}
		return items
	}
	out := m{}
	for n := rnd.Intn(5); n > 0; n-- {
		out[keys[rnd.Intn(len(keys))]] = msTree(rnd, keys, depth+1)
	}
	return out
}

// msKitchenKeys are the keys of Kitchen's mapstructure fields, and keys it does not have.
var msKitchenKeys = []string{
	"int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64",
	"float32", "float64", "bool", "string", "duration", "time", "ptrint", "ptrstr", "ptrbool",
	"ptrdur", "ptrptr", "ints", "strings", "bytes", "any", "anys", "strmap", "intmap", "anymap",
	"keymap", "nested", "value", "nesteds", "bidtype", "bidtypes", "Plain", "PLAIN", "ina",
	"inb", "Extra", "-", "hidden",
}

// msSamples are the input values of the generated mapstructure cases.
func msSamples() []interface{} {
	return []interface{}{
		nil, true, false,
		0, 1, -1, 42, 300, -129, 70000, int8(-8), int16(1000), int32(-5), int64(1 << 40),
		int64(math.MinInt64), uint(7), uint8(200), uint16(60000), uint32(1 << 31),
		uint64(1 << 63), uint64(math.MaxUint64),
		0.0, 1.5, -2.25, 42.42, 1e21, 1e-7, 3.5e38, math.Inf(1), math.Inf(-1), math.NaN(),
		-1.5, 1.8446744073709552e19,
		"", "0", "1", "42", "-7", "+5", "0x1F", "0b11", "017", "1e3", "1.5", " 5", "1_000",
		"true", "false", "T", "f", "yes", "abc", "1h30m", "10s", "-1.5ms", "a,b,c", "x,", ",",
		"18446744073709551616", "9223372036854775808", "-9223372036854775809", "300", "-1",
		time.Duration(1500),
		[]interface{}{}, []interface{}{1, "2", 3.5}, []interface{}{nil}, []interface{}{[]interface{}{1}},
		[]string{"x", "y"}, []string{}, []string(nil), []int{1, 2}, []byte("hi"),
		[]interface{}{m{"a": 1}}, m{}, m{"a": 1, "b": "x"}, m{"a": m{"b": 2}},
		// Keys of one dynamic type only: fmt orders keys of different types by their runtime
		// type pointers, which the Rust port cannot reproduce.
		map[string]string{"k": "v"}, map[interface{}]interface{}{"a": 1, "b": "x"},
		map[interface{}]interface{}{2: "b"}, map[interface{}]interface{}{true: 1, false: 0},
		map[interface{}]interface{}{}, m(nil), m{"ptr": m{"a": 3}, "b": "z,w"},
		m{"A": "4", "B": []interface{}{"x"}}, map[int]string{7: "seven"},
		// time.Time, the struct a yaml.v3 timestamp decodes to: decodeMapFromStruct turns it
		// into an empty map for struct and map targets.
		time.Time{}, time.Date(2001, 12, 14, 21, 59, 43, 100000000, time.UTC),
		time.Date(2001, 12, 14, 21, 59, 43, 0, time.FixedZone("", -5*3600)),
		time.Date(2015, 2, 24, 18, 19, 39, 0, time.FixedZone("EST", -5*3600)),
		[]interface{}{time.Date(2001, 12, 14, 0, 0, 0, 0, time.UTC)},
		m{"t": time.Date(2001, 12, 14, 0, 0, 0, 0, time.UTC), "a": 1},
		// Nil maps of other types than the target's, which reflect.Set rejects.
		map[interface{}]interface{}(nil), map[string]string(nil), map[string]int(nil),
		[]interface{}{m(nil)}, []interface{}{m{"a": "b"}, m(nil)},
		[]interface{}{m{"a": "b"}, map[string]string(nil)}, []interface{}{nil, m{"a": 1}},
		[]interface{}{m{"a": 1}, []interface{}{m{"b": 2}}}, []interface{}{m{}, "x"},
		[]m{{"a": 1}}, []map[string]string{{"a": "b"}, nil}, []interface{}{[]interface{}{}},
		map[string][]string{"a": {"b"}, "c": nil}, m{"a": []interface{}{"x", 1}, "b": m{}},
	}
}
