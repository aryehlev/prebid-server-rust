package main

import (
	"fmt"
	"math"
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
var msConfigNames = []string{"strict", "weak", "viper", "viper-unused", "viper-zero"}

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

	// Primitive targets.
	for i, v := range msSamples() {
		for _, target := range []string{"string", "int", "*int", "bool", "float64", "time.Duration",
			"[]string", "interface {}", "map[string]interface {}", "map[string]string"} {
			add(fmt.Sprintf("primitive/%s/%d", target, i), target, v)
		}
	}

	// Every sample value under every Kitchen key.
	for _, key := range msKitchenKeys {
		for i, v := range msSamples() {
			add(fmt.Sprintf("kitchen/%s/%d", key, i), "Kitchen", m{key: v})
		}
	}
	prefilled("kitchen/prefilled", "Kitchen",
		m{"ints": []int{1, 2, 3}, "strmap": m{"a": "1"}, "nested": m{"a": 1, "b": []string{"x"}},
			"ptrint": 5, "any": m{"k": "v"}, "anys": []interface{}{1, 2}},
		m{"ints": []int{9}, "strmap": m{"b": "2"}, "nested": m{"b": []string{"y"}}, "ptrint": nil,
			"any": m{"k2": "v2"}, "anys": []interface{}{"x"}})
	prefilled("kitchen/prefilled-nils", "Kitchen",
		m{"ints": []int{1}, "strmap": m{"a": "1"}, "nested": m{"a": 1}, "string": "s"},
		m{"ints": nil, "strmap": nil, "nested": nil, "string": nil})
	return cases
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
	}
}
