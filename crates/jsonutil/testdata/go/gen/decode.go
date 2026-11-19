package main

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"sort"

	"github.com/buger/jsonparser"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/tidwall/sjson"
)

// The decode and encode pins: what jsonutil.Unmarshal, UnmarshalValid and Marshal (with the
// RawMessage encoder main.go registers) and encoding/json do with the inputs each restored or
// ledgered difference of pbs-jsonutil names, and what the custom decoders of util/jsonutil give.

// row is one recorded call: the input, the decoder or writer, and its result or error.
type row struct {
	Name   string `json:"name"`
	Input  string `json:"input"`
	Result string `json:"result"`
	Err    string `json:"err"`
	// ErrType is the Go type of the error, for the wrappers.
	ErrType string `json:"err_type,omitempty"`
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func errType(err error) string {
	if err == nil {
		return ""
	}
	return reflect.TypeOf(err).String()
}

func marshalled(v any) string {
	out, err := jsonutil.Marshal(v)
	if err != nil {
		return "error: " + err.Error()
	}
	return string(out)
}

// nullTarget has a value field of each kind and a pointer.
type nullTarget struct {
	S  string `json:"s"`
	I  int    `json:"i"`
	F  float64
	B  bool `json:"b"`
	St struct {
		A int `json:"a"`
	} `json:"st"`
	L []int          `json:"l"`
	M map[string]int `json:"m"`
	P *int           `json:"p"`
}

type rawOmit struct {
	Ext json.RawMessage `json:"ext,omitempty"`
}

type rawPlain struct {
	Ext json.RawMessage `json:"ext"`
}

type anyTarget struct {
	V interface{} `json:"v"`
}

type idTarget struct {
	ID string `json:"id"`
	N  int    `json:"n"`
}

type stringTarget struct {
	S string `json:"s"`
}

type stringIntItem struct {
	ItemId jsonutil.StringInt `json:"item_id"`
}

type intStringItem struct {
	ItemId jsonutil.IntString `json:"item_id"`
}

type typedInt8 int8

type typedTarget struct {
	V typedInt8 `json:"v"`
}

func nullIntoValue() []row {
	in := `{"s":null,"i":null,"F":null,"b":null,"st":null,"l":null,"m":null,"p":null}`
	var rows []row
	var a nullTarget
	err := jsonutil.Unmarshal([]byte(in), &a)
	rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: marshalled(a), Err: errText(err)})
	var b nullTarget
	err = jsonutil.UnmarshalValid([]byte(in), &b)
	rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: marshalled(b), Err: errText(err)})
	var c nullTarget
	err = json.Unmarshal([]byte(in), &c)
	rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: marshalled(c), Err: errText(err)})
	return rows
}

func rawState(ext json.RawMessage) string {
	if ext == nil {
		return "nil"
	}
	return fmt.Sprintf("%q", string(ext))
}

func nullRaw() []row {
	in := `{"ext":null}`
	var rows []row
	var a rawOmit
	err := jsonutil.Unmarshal([]byte(in), &a)
	rows = append(rows, row{Name: "jsonutil.Unmarshal omitempty", Input: in, Result: rawState(a.Ext) + " " + marshalled(a), Err: errText(err)})
	var b rawPlain
	err = jsonutil.Unmarshal([]byte(in), &b)
	rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: rawState(b.Ext) + " " + marshalled(b), Err: errText(err)})
	var c rawOmit
	err = jsonutil.UnmarshalValid([]byte(in), &c)
	rows = append(rows, row{Name: "jsonutil.UnmarshalValid omitempty", Input: in, Result: rawState(c.Ext) + " " + marshalled(c), Err: errText(err)})
	var d rawOmit
	err = json.Unmarshal([]byte(in), &d)
	rows = append(rows, row{Name: "json.Unmarshal omitempty", Input: in, Result: rawState(d.Ext) + " " + marshalled(d), Err: errText(err)})
	var e rawPlain
	err = json.Unmarshal([]byte(in), &e)
	rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: rawState(e.Ext) + " " + marshalled(e), Err: errText(err)})
	return rows
}

// encodeBlob is TestEncode's document (util/jsonutil/jsonutil_test.go:213-223).
const encodeBlob = `{
  "properties": {
    "string": "Blanks spaces in between words to not be removed if compacted",
    "integer": 5,
    "string_array": [
      "string array elem one",
      "string array elem two"
    ]
  }
}`

func rawCompact() []row {
	var rows []row
	for _, in := range []string{
		`{"ext": {"a" : [1, 2.50], "b":"x  y <&>"} }`,
		`{"ext":` + encodeBlob + `}`,
		"{\"ext\": \"\\u00e9 \\/\" }",
		`{"ext": 1E+2 }`,
		"{\"ext\": [\"\\ud800\", \"x\\udc00\"] }",
	} {
		var v rawOmit
		err := jsonutil.Unmarshal([]byte(in), &v)
		rows = append(rows, row{Name: "jsonutil.Marshal", Input: in, Result: marshalled(v), Err: errText(err)})
	}
	return rows
}

func nilRaw() []row {
	return []row{
		{Name: "field", Result: marshalled(rawPlain{})},
		{Name: "map value", Result: marshalled(map[string]json.RawMessage{"a": nil})},
		{Name: "slice element", Result: marshalled([]json.RawMessage{nil, json.RawMessage(`{"x":1}`)})},
		{Name: "top level", Result: marshalled(json.RawMessage(nil))},
		{Name: "omitempty", Result: marshalled(rawOmit{})},
	}
}

func interfaceNumbers() []row {
	var rows []row
	for _, in := range []string{`{"v":9007199254740993}`, `{"v":1.0}`, `{"v":1e2}`, `{"v":1.5}`, `{"v":12345678901234567890}`, `{"v":[1,{"a":2.0}]}`} {
		var v anyTarget
		err := jsonutil.Unmarshal([]byte(in), &v)
		rows = append(rows, row{Name: "jsonutil", Input: in, Result: marshalled(v), Err: errText(err)})
	}
	return rows
}

func errorTexts() []row {
	var rows []row
	for _, in := range []string{`{"id": 1}`, `{"id":"x"} x`, `{"id":`, `[]`, `{"n": "x"}`, `{"n": 1.5}`, ``} {
		var a idTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Err: errText(err), ErrType: errType(err)})
		var b idTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Err: errText(err), ErrType: errType(err)})
	}
	_, err := jsonutil.Marshal(make(chan int))
	rows = append(rows, row{Name: "jsonutil.Marshal chan int", Err: errText(err), ErrType: errType(err)})
	_, err = jsonutil.Marshal(math.NaN())
	rows = append(rows, row{Name: "jsonutil.Marshal NaN", Err: errText(err), ErrType: errType(err)})
	return rows
}

func unvalidatedRaw() []row {
	var rows []row
	for _, in := range []string{`{"ext":-01}`, `{"ext":[-01]}`, `{"ext":01}`, `{"ext":{"a":1,}}`} {
		var a rawOmit
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: rawState(a.Ext), Err: errText(err)})
		var b rawOmit
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: rawState(b.Ext), Err: errText(err)})
		var c rawOmit
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: rawState(c.Ext), Err: errText(err)})
	}
	return rows
}

func intMapKeyOrder() []row {
	return []row{{Name: "jsonutil.Marshal", Result: marshalled(map[int]string{10: "a", 9: "b", -1: "c", 100: "d"})}}
}

func memberOrder() []row {
	in := `{"b":1,"a":2}`
	viaSjson, err := sjson.SetBytes([]byte(in), "c", 3)
	rows := []row{{Name: "sjson.SetBytes", Input: in, Result: string(viaSjson), Err: errText(err)}}
	viaJsonparser, err := jsonparser.Set([]byte(in), []byte("3"), "c")
	rows = append(rows, row{Name: "jsonparser.Set", Input: in, Result: string(viaJsonparser), Err: errText(err)})
	edited, err := sjson.SetBytes([]byte(in), "b", 5)
	rows = append(rows, row{Name: "sjson.SetBytes existing", Input: in, Result: string(edited), Err: errText(err)})
	return rows
}

func loneSurrogates() []row {
	var rows []row
	for _, s := range []string{
		"\"a\\ud800b\"", "\"\\udc00\"", "\"\\ud800\\ud800\"", "\"\\ud800\\ud800\\udc00\"",
		"\"\\udc00\\ud800\\udc00\"", "\"\\ud800\\u0041\"", "\"\\ud800\\n\"", "\"\\\\ud800\"",
		"\"\\ud83d\\ude00\"", "\"x\\ud800\"", "\"\\uDBFF\\uDFFF\"", "\"\\ud800\\\"\"",
	} {
		in := `{"s":` + s + `}`
		var a stringTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: hex.EncodeToString([]byte(a.S)), Err: errText(err)})
		var b stringTarget
		err = json.Unmarshal([]byte(in), &b)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: hex.EncodeToString([]byte(b.S)), Err: errText(err)})
	}
	in := "{\"\\ud800\":1}"
	var m map[string]int
	err := jsonutil.Unmarshal([]byte(in), &m)
	for k := range m {
		rows = append(rows, row{Name: "jsonutil.Unmarshal key", Input: in, Result: hex.EncodeToString([]byte(k)), Err: errText(err)})
	}
	return rows
}

func stringInts() []row {
	var rows []row
	for _, v := range []string{`"30"`, `30`, `""`, `true`, `null`, `"-"`, `"+5"`, `" 5"`, `"1.5"`, `9223372036854775807`, `9223372036854775808`, `-9223372036854775808`, `"0x1"`, "\"\\u0033\"", `[]`, `{}`, `"007"`, `-0`} {
		in := `{"item_id":` + v + `}`
		var item stringIntItem
		err := jsonutil.UnmarshalValid([]byte(in), &item)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: fmt.Sprint(int(item.ItemId)), Err: errText(err)})
	}
	return rows
}

func intStrings() []row {
	var rows []row
	for _, in := range []string{`{"item_id": null}`, `{"item_id": "30"}`, `{"item_id": 30}`, `{"item_id": []`, `{"item_id": 1.50}`, `{"item_id": 1e2}`, `{"item_id": -0}`, `{"item_id": true}`, "{\"item_id\": \"\\u0041\"}", `{"item_id": 123456789012345678901234567890}`, "{\"item_id\": \"\\ud800\\u0041\"}", "{\"item_id\": \"\\ud800\\ud800\\udc00\"}"} {
		var item intStringItem
		err := jsonutil.UnmarshalValid([]byte(in), &item)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: string(item.ItemId), Err: errText(err)})
	}
	return rows
}

func parseIntoStrings() []row {
	var rows []row
	for _, in := range []string{``, `"1"`, `1`, `null`, `"null"`, `"hello"`, `true`, `{"a":1}`, `[1]`, ` "x" `, `1.50`, ` {"a":1} `, `"abc`, `1x`, `+5`, `nan`, `tru`, `x`, `1e400`, "\"\\ud800\\u0041\"", "\"\\ud800\\ud800\\udc00\""} {
		var got *string
		err := jsonutil.ParseIntoString([]byte(in), &got)
		result := "nil"
		if got != nil {
			result = fmt.Sprintf("%q", *got)
		}
		rows = append(rows, row{Name: "jsonutil.ParseIntoString", Input: in, Result: result, Err: errText(err)})
	}
	err := jsonutil.ParseIntoString([]byte(`"123"`), nil)
	rows = append(rows, row{Name: "jsonutil.ParseIntoString nil", Input: `"123"`, Result: "nil", Err: errText(err)})
	return rows
}

// outOfRangeNumbers is json.out-of-range-numbers: jsoniter fails on a number beyond float64's
// range in a value it skips (a json.RawMessage, an unknown member, the bytes an UnmarshalJSON
// method reads), and on one that starts with 0 beyond float32's range, which its skipper parses as
// a float32; encoding/json checks only the syntax there. Integers of digits only, numbers that
// underflow and float64's largest value read everywhere.
func outOfRangeNumbers() []row {
	var rows []row
	for _, in := range []string{`{"ext":1e400}`, `{"ext":[-1e309]}`, `{"ext":{"a":1.8e308}}`, `{"ext":1.7976931348623157e308}`, `{"ext":1e-400}`, `{"ext":123456789012345678901234567890}`, `{"ext":0.5e39}`, `{"ext":-0.5e39}`} {
		var a rawPlain
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: rawState(a.Ext), Err: errText(err)})
		var b rawPlain
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: rawState(b.Ext), Err: errText(err)})
		var c rawPlain
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: rawState(c.Ext), Err: errText(err)})
	}
	for _, in := range []string{`{"zz":1e400,"s":"x"}`, `{"s":"x","zz":[1,{"a":-1e400}]}`} {
		var a stringTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal unknown member", Input: in, Result: a.S, Err: errText(err)})
		var c stringTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal unknown member", Input: in, Result: c.S, Err: errText(err)})
	}
	for _, in := range []string{`{"item_id":1e400}`, `{"item_id":-1e400}`, `{"item_id":0.165e308}`} {
		var a intStringItem
		err := jsonutil.UnmarshalValid([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid IntString", Input: in, Result: string(a.ItemId), Err: errText(err)})
		var c intStringItem
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal IntString", Input: in, Result: string(c.ItemId), Err: errText(err)})
	}
	return rows
}

func typedInts() []row {
	var rows []row
	for _, v := range []string{`5`, `200`, `-128`, `1.0`, `"5"`, `null`} {
		in := `{"v":` + v + `}`
		var t typedTarget
		err := jsonutil.Unmarshal([]byte(in), &t)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: fmt.Sprint(int(t.V)), Err: errText(err)})
	}
	return rows
}

// intTarget has an integer field of each kind Prebid declares (int, int8, uint and a typed int8,
// as openrtb2's enums are), a pointer to an int and a float64.
type intTarget struct {
	I  int       `json:"i"`
	I8 int8      `json:"i8"`
	U  uint      `json:"u"`
	T  typedInt8 `json:"t"`
	P  *int      `json:"p,omitempty"`
	F  float64   `json:"f"`
}

// negativeZeros is json.negative-zero-int: -0 read into an integer field. jsoniter's ReadInt64
// reads the minus and then the single zero, and encoding/json parses -0 with strconv, so both read
// 0 into a signed integer (a pointer to one included) and reject it for an unsigned one; MergeClone
// writes the 0 over the value there. A float64 holds -0.
func negativeZeros() []row {
	var rows []row
	for _, field := range []string{"i", "i8", "u", "t", "p", "f"} {
		in := `{"` + field + `":-0}`
		var a intTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: marshalled(a), Err: errText(err)})
		var b intTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: marshalled(b), Err: errText(err)})
		var c intTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: marshalled(c), Err: errText(err)})
		d := intTarget{I: 5, I8: 5, U: 5, T: 5, F: 5}
		err = jsonutil.MergeClone(&d, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: marshalled(d), Err: errText(err)})
	}
	return rows
}

// controlTarget reads a string, a json.RawMessage and a map's keys.
type controlTarget struct {
	S   string          `json:"s"`
	Ext json.RawMessage `json:"ext,omitempty"`
	M   map[string]int  `json:"m,omitempty"`
}

// state is what a decoder read: the string, the raw bytes and the map's keys, quoted.
func (v controlTarget) state() string {
	keys := make([]string, 0, len(v.M))
	for k := range v.M {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return fmt.Sprintf("s=%q ext=%s m=%q", v.S, rawState(v.Ext), keys)
}

// controlCharacters is json.control-characters: a raw control character (U+0000 to U+001F)
// inside a string, which JSON forbids. jsoniter's ReadString rejects one that comes before every
// backslash of its string, and its slow path, which takes over at the first backslash, appends
// every byte after it unchecked, so jsoniter reads a control character after an escape: in a
// field, in an unknown member and a json.RawMessage (its skipper falls back to ReadString at a
// backslash) and in a map key. encoding/json rejects them all.
func controlCharacters() []row {
	var rows []row
	for _, in := range []string{
		"{\"s\":\"a\tb\"}",
		"{\"s\":\"\\na\tb\"}",
		"{\"s\":\"\\/\x01\"}",
		"{\"s\":\"\\u0041\n\"}",
		"{\"zz\":\"\tq\",\"s\":\"x\"}",
		"{\"zz\":\"\\n\tq\",\"s\":\"x\"}",
		"{\"ext\":\"\\n\tq\"}",
		"{\"ext\":{\"a\":[\"\\\"\x1f\"]}}",
		"{\"m\":{\"\\n\tk\":1}}",
	} {
		var a controlTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: a.state(), Err: errText(err)})
		var b controlTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: b.state(), Err: errText(err)})
		var c controlTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: c.state(), Err: errText(err)})
		var d controlTarget
		err = jsonutil.MergeClone(&d, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: d.state(), Err: errText(err)})
	}
	return rows
}

// bigControlTarget has more than ten fields, so jsoniter decodes it with its general struct
// decoder, which reads member names with ReadString.
type bigControlTarget struct {
	A1  int    `json:"a1"`
	A2  int    `json:"a2"`
	A3  int    `json:"a3"`
	A4  int    `json:"a4"`
	A5  int    `json:"a5"`
	A6  int    `json:"a6"`
	A7  int    `json:"a7"`
	A8  int    `json:"a8"`
	A9  int    `json:"a9"`
	A10 int    `json:"a10"`
	S   string `json:"s"`
}

// controlMemberNames is json.control-characters in member names: jsoniter decodes a struct of
// ten fields or fewer by a hash of each member's name (createStructDecoder, readFieldHash), which
// checks none of its bytes, so a raw control character anywhere in a name passes there and the
// member matches no field; a larger struct's decoder reads the name with ReadString, which
// rejects one before the first backslash. encoding/json rejects them all.
func controlMemberNames() []row {
	var rows []row
	for _, in := range []string{
		"{\"s\":\"x\",\"z\tz\":1}",
		"{\"\ts\":\"x\"}",
		"{\"s\x01\":\"x\"}",
	} {
		var a controlTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: a.state(), Err: errText(err)})
		var b bigControlTarget
		err = jsonutil.Unmarshal([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.Unmarshal big", Input: in, Result: fmt.Sprintf("s=%q", b.S), Err: errText(err)})
		var c controlTarget
		err = jsonutil.UnmarshalValid([]byte(in), &c)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: c.state(), Err: errText(err)})
		var d bigControlTarget
		err = jsonutil.UnmarshalValid([]byte(in), &d)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid big", Input: in, Result: fmt.Sprintf("s=%q", d.S), Err: errText(err)})
		var e controlTarget
		err = json.Unmarshal([]byte(in), &e)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: e.state(), Err: errText(err)})
		var f controlTarget
		err = jsonutil.MergeClone(&f, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: f.state(), Err: errText(err)})
		var g bigControlTarget
		err = jsonutil.MergeClone(&g, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone big", Input: in, Result: fmt.Sprintf("s=%q", g.S), Err: errText(err)})
	}
	return rows
}

// nullDocuments is json.null-document: a document that is JSON null, as a bidder's response body
// can be. jsoniter and encoding/json return no error and leave a struct and an int as they were
// and a map and a slice nil; jsoniter makes a string "", encoding/json leaves it. MergeClone leaves
// the struct as it was.
func nullDocuments() []row {
	decoders := []struct {
		name   string
		decode func([]byte, any) error
	}{
		{"jsonutil.Unmarshal", jsonutil.Unmarshal},
		{"jsonutil.UnmarshalValid", jsonutil.UnmarshalValid},
		{"json.Unmarshal", json.Unmarshal},
	}
	var rows []row
	for _, in := range []string{"null", " null\n"} {
		for _, d := range decoders {
			st := idTarget{ID: "keep", N: 7}
			err := d.decode([]byte(in), &st)
			rows = append(rows, row{Name: d.name + " struct", Input: in, Result: marshalled(st), Err: errText(err)})
			m := map[string]int{"a": 1}
			err = d.decode([]byte(in), &m)
			rows = append(rows, row{Name: d.name + " map", Input: in, Result: fmt.Sprintf("nil=%v", m == nil), Err: errText(err)})
			s := []int{1}
			err = d.decode([]byte(in), &s)
			rows = append(rows, row{Name: d.name + " slice", Input: in, Result: fmt.Sprintf("nil=%v", s == nil), Err: errText(err)})
			str := "keep"
			err = d.decode([]byte(in), &str)
			rows = append(rows, row{Name: d.name + " string", Input: in, Result: fmt.Sprintf("%q", str), Err: errText(err)})
			n := 7
			err = d.decode([]byte(in), &n)
			rows = append(rows, row{Name: d.name + " int", Input: in, Result: fmt.Sprint(n), Err: errText(err)})
		}
		st := idTarget{ID: "keep", N: 7}
		err := jsonutil.MergeClone(&st, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone struct", Input: in, Result: marshalled(st), Err: errText(err)})
	}
	return rows
}

type elementFormat struct {
	W int `json:"w"`
}

// elementTarget has slices of ints, strings, structs, a typed int and pointers, and maps of ints
// and strings.
type elementTarget struct {
	L  []int             `json:"l,omitempty"`
	S  []string          `json:"s,omitempty"`
	F  []elementFormat   `json:"f,omitempty"`
	T  []typedInt8       `json:"t,omitempty"`
	P  []*int            `json:"p,omitempty"`
	M  map[string]int    `json:"m,omitempty"`
	MS map[string]string `json:"ms,omitempty"`
}

// nullElements is json.null-elements: null as an element of a slice, or a value of a map, whose
// element type is not a pointer. jsoniter and encoding/json leave the element's zero value, as
// they do for a field (json.null-into-value), and so does MergeClone; a pointer element is nil.
func nullElements() []row {
	var rows []row
	for _, in := range []string{
		`{"l":[1,null,2]}`,
		`{"s":["a",null]}`,
		`{"f":[null,{"w":1}]}`,
		`{"t":[null,5]}`,
		`{"p":[null,1]}`,
		`{"m":{"a":null,"b":1}}`,
		`{"ms":{"a":null}}`,
	} {
		var a elementTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: marshalled(a), Err: errText(err)})
		var b elementTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: marshalled(b), Err: errText(err)})
		var c elementTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: marshalled(c), Err: errText(err)})
		var d elementTarget
		err = jsonutil.MergeClone(&d, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: marshalled(d), Err: errText(err)})
	}
	return rows
}

// trailingNuls is json.trailing-nul: jsoniter's nextToken returns 0 for a NUL byte as at the end
// of the input, and Unmarshal takes a 0 after the value for the end, so jsonutil.Unmarshal,
// UnmarshalValid and MergeClone stop at a NUL after the document and ignore every byte after it.
// encoding/json rejects any byte after the value but whitespace. A NUL before or inside the value
// fails everywhere.
func trailingNuls() []row {
	var rows []row
	for _, in := range []string{
		"{\"id\":\"x\"}\x00",
		"{\"id\":\"x\"} \x00 not JSON",
		"\x00{\"id\":\"x\"}",
		"{\"id\":\"x\"\x00}",
	} {
		var a idTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: marshalled(a), Err: errText(err)})
		var b idTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: marshalled(b), Err: errText(err)})
		var c idTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: marshalled(c), Err: errText(err)})
		var d idTarget
		err = jsonutil.MergeClone(&d, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: marshalled(d), Err: errText(err)})
	}
	return rows
}

// floatTarget has a float64, a float32 and an int.
type floatTarget struct {
	F   float64 `json:"f"`
	F32 float32 `json:"f32"`
	I   int     `json:"i"`
}

// plusFloats is json.plus-float: jsoniter's float decoders pass a number that starts with
// neither a digit, a minus nor a dot to strconv.ParseFloat, which accepts a leading +, so +1.5
// reads into a float64 or float32 field (and -+1 as -1). An integer field, a skipped member and
// encoding/json reject it.
func plusFloats() []row {
	var rows []row
	for _, in := range []string{`{"f":+1.5}`, `{"f32":+2}`, `{"f":+.5}`, `{"f":-+1}`, `{"f":+1e2}`, `{"i":+1}`, `{"zz":+1,"f":1}`, `{"f":+}`} {
		var a floatTarget
		err := jsonutil.Unmarshal([]byte(in), &a)
		rows = append(rows, row{Name: "jsonutil.Unmarshal", Input: in, Result: marshalled(a), Err: errText(err)})
		var b floatTarget
		err = jsonutil.UnmarshalValid([]byte(in), &b)
		rows = append(rows, row{Name: "jsonutil.UnmarshalValid", Input: in, Result: marshalled(b), Err: errText(err)})
		var c floatTarget
		err = json.Unmarshal([]byte(in), &c)
		rows = append(rows, row{Name: "json.Unmarshal", Input: in, Result: marshalled(c), Err: errText(err)})
		var d floatTarget
		err = jsonutil.MergeClone(&d, []byte(in))
		rows = append(rows, row{Name: "jsonutil.MergeClone", Input: in, Result: marshalled(d), Err: errText(err)})
	}
	return rows
}

type decodeDoc struct {
	Header           header `json:"header"`
	NullIntoValue    []row  `json:"null_into_value"`
	NullRaw          []row  `json:"null_raw"`
	RawCompact       []row  `json:"raw_compact"`
	NilRaw           []row  `json:"nil_raw"`
	InterfaceNumbers []row  `json:"interface_numbers"`
	ErrorTexts       []row  `json:"error_texts"`
	UnvalidatedRaw   []row  `json:"unvalidated_raw"`
	IntMapKeyOrder   []row  `json:"int_map_key_order"`
	MemberOrder      []row  `json:"member_order"`
	LoneSurrogates   []row  `json:"lone_surrogates"`
	StringInt        []row  `json:"string_int"`
	IntString        []row  `json:"int_string"`
	ParseIntoString  []row  `json:"parse_into_string"`
	OutOfRange       []row  `json:"out_of_range_numbers"`
	TypedInts        []row  `json:"typed_ints"`
	NegativeZero     []row  `json:"negative_zero"`
	ControlChars     []row  `json:"control_characters"`
	NullDocuments    []row  `json:"null_documents"`
	NullElements     []row  `json:"null_elements"`
	TrailingNuls     []row  `json:"trailing_nuls"`
	PlusFloats       []row  `json:"plus_floats"`
	ControlNames     []row  `json:"control_member_names"`
}

func decodeGolden() decodeDoc {
	return decodeDoc{
		Header:           newHeader("github.com/json-iterator/go", "github.com/tidwall/sjson", "github.com/buger/jsonparser", "github.com/tidwall/gjson"),
		NullIntoValue:    nullIntoValue(),
		NullRaw:          nullRaw(),
		RawCompact:       rawCompact(),
		NilRaw:           nilRaw(),
		InterfaceNumbers: interfaceNumbers(),
		ErrorTexts:       errorTexts(),
		UnvalidatedRaw:   unvalidatedRaw(),
		IntMapKeyOrder:   intMapKeyOrder(),
		MemberOrder:      memberOrder(),
		LoneSurrogates:   loneSurrogates(),
		StringInt:        stringInts(),
		IntString:        intStrings(),
		ParseIntoString:  parseIntoStrings(),
		OutOfRange:       outOfRangeNumbers(),
		TypedInts:        typedInts(),
		NegativeZero:     negativeZeros(),
		ControlChars:     controlCharacters(),
		NullDocuments:    nullDocuments(),
		NullElements:     nullElements(),
		TrailingNuls:     trailingNuls(),
		PlusFloats:       plusFloats(),
		ControlNames:     controlMemberNames(),
	}
}
