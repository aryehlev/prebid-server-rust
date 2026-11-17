// Command golden writes the differential goldens that pbs-gojson's tests replay byte for byte.
//
// Every decode case runs through jsoniter (ConfigCompatibleWithStandardLibrary, as
// jsonutil.Unmarshal decodes), jsonutil.Unmarshal and encoding/json; every encode case through
// jsonutil.Marshal (jsoniter with Prebid's RawMessageExtension), json.Marshal and a json.Encoder
// with SetEscapeHTML(false). Each output and error is written with strconv.QuoteToASCII, one
// tab-separated case per line.
//
// Run it from the repository root:
//
//	go run ./rust/crates/gojson/testdata/go
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always runs with it.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

var iterAPI = jsoniter.ConfigCompatibleWithStandardLibrary

// golden is one output file.
type golden struct {
	w     *bufio.Writer
	f     *os.File
	cases int
}

func create(dir, name, header string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f), f: f}
	for _, line := range strings.Split(header, "\n") {
		g.w.WriteString("# " + line + "\n")
	}
	return g
}

// row writes one case: each field quoted, tab separated.
func (g *golden) row(fields ...[]byte) {
	for i, field := range fields {
		if i > 0 {
			g.w.WriteByte('\t')
		}
		g.w.WriteString(strconv.QuoteToASCII(string(field)))
	}
	g.w.WriteByte('\n')
	g.cases++
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := g.f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s: %d cases\n", g.f.Name(), g.cases)
}

func errText(err error) []byte {
	if err == nil {
		return nil
	}
	return []byte(err.Error())
}

// value is encoding/json's rendering of a decoded value, or "!" and the marshal error.
func value(v interface{}) []byte {
	b, err := json.Marshal(v)
	if err != nil {
		return []byte("!" + err.Error())
	}
	return b
}

// output is an encoder's output, or "!" and its error.
func output(b []byte, err error) []byte {
	if err != nil {
		return []byte("!" + err.Error())
	}
	return b
}

type target struct {
	name     string
	newValue func() interface{}
}

func targetOf[T any]() target {
	return target{
		name:     reflect.TypeOf((*T)(nil)).Elem().String(),
		newValue: func() interface{} { return new(T) },
	}
}

var scalarTargets = []target{
	targetOf[int](), targetOf[int8](), targetOf[int16](), targetOf[int32](), targetOf[int64](),
	targetOf[uint](), targetOf[uint8](), targetOf[uint16](), targetOf[uint32](), targetOf[uint64](),
	targetOf[float32](), targetOf[float64](), targetOf[bool](), targetOf[string](),
	targetOf[*int](), targetOf[*string](), targetOf[[]int](), targetOf[[]byte](),
	targetOf[[]string](), targetOf[[]*int](), targetOf[map[string]int](),
	targetOf[map[int]string](), targetOf[map[string]interface{}](), targetOf[interface{}](),
	targetOf[json.RawMessage](), targetOf[*json.RawMessage](), targetOf[[]json.RawMessage](),
	targetOf[map[string]json.RawMessage](), targetOf[jsonutil.StringInt](),
	targetOf[jsonutil.IntString](), targetOf[Small](), targetOf[Empty](),
	targetOf[json.Number](), targetOf[*json.Number](), targetOf[map[Key]int](),
	targetOf[map[Slot]string](),
}

var generalInputs = []string{
	``, ` `, `null`, ` null `, `nul`, `nulll`, `n`, `true`, `false`, `tru`, `falsey`, `t`,
	`0`, `-0`, `1`, `-1`, `+1`, `01`, `00`, `-`, `1.`, `.5`, `1.5`, `-1.5`, `1e3`, `1E+3`,
	`1e-3`, `1e`, `1e+`, `2.5e1`, `1.0`, `127`, `128`, `-128`, `-129`, `255`, `256`, `32767`,
	`32768`, `65535`, `65536`, `2147483647`, `2147483648`, `-2147483649`, `4294967295`,
	`4294967296`, `9223372036854775807`, `9223372036854775808`, `-9223372036854775808`,
	`-9223372036854775809`, `18446744073709551615`, `18446744073709551616`,
	`123456789012345678901234567890`, `1e400`, `-1e400`, `1e-400`, `3.4028234663852886e38`,
	`3.5e38`, `0.1`, `100000000000000000000000`, `1.7976931348623157e308`,
	`"x"`, `""`, `"123"`, `"\u00e9\ud83d\ude00"`, `"\ud800"`, `"\ud800\u0041"`, `"\udc00"`,
	`"a\"b\\c\/d\b\f\n\r\t"`, `"\q"`, `"\u12"`, `"\u12zz"`, `"abc`, `"`, "\"a\xffb\"",
	"\"\x01\"", `"<>&"`, `"AQID"`, `"AQI"`, `"AQ=="`, `"!!"`, "\"\xe2\x80\xa8\"", `"\u0000"`,
	`[]`, `[ ]`, `[1,2,3]`, `[1,"a",null]`, `[1,]`, `[,1]`, `[1 2]`, `[`, `[[1],[2,[3]]]`,
	`["a","b"]`, `[null]`, `[{"a":1}]`, `[1]]`, `{}`, `{ }`, `{"a":1}`, `{"a":1,"b":"x"}`,
	`{"1":"a","-2":"b"}`, `{"a":}`, `{"a" 1}`, `{"a":1,}`, `{,}`, `{"a":1`, `{1:2}`,
	`{"a":{"b":[1,{"c":null}]}}`, `{"a":1}x`, `1 2`, `1,`, `{"a":1} {"b":2}`, `}`, `]`, `,`,
	`:`, `x`, `-x`, `{"a":null}`, `{"a":"b","a":"c"}`, " {\"a\" : [ 1 , 2 ] } ",
	`{"id":"a","w":300,"price":1.5}`, `{"ID":"a","PRICE":2}`, `{"id":1}`, `{"ext":{"a" : 1}}`,
	`{"tags":["a",]}`, `{"w":"1"}`, `[[[[[[[[[[1]]]]]]]]]]`, `{"a":{"a":{"a":{"a":[]}}}}`,
}

var structTargets = []struct {
	target target
	inputs []string
}{
	{targetOf[Small](), []string{
		`{"id":"a","w":300,"h":250,"price":1.5,"tags":["x","y"],"ext":{"k":[1, 2]}}`,
		`{"ID":"a","W":1,"PRICE":2}`, `{"Id":"a","iD":"b"}`, `{"id":"a","id":"b"}`,
		`{"w":null,"tags":null,"ext":null}`, `{"w":"300"}`, `{"price":"1.5"}`, `{"tags":"x"}`,
		`{"tags":[1]}`, `{"ext":}`, `{"ext":nul}`, `{"ext": {"a":1} }`,
		`{"unknown":{"deep":[1,2,{"x":"y"}]},"id":"a"}`, `{"id":1}`, `{"id":"a",}`,
		`{"id":"a" "w":1}`, `{"id":"a"`, `{"id"}`, `{"id":"a"]`, `{"h":1.5}`,
		`{"h":-9223372036854775809}`, `{"w":1e2}`, `{"price":1e400}`, `{"price":-}`,
		`{"price":.5}`, `{"tags":["a",]}`, `{"id":"\u0041\ud83d\ude00"}`, `{"\u0069d":"esc"}`,
		`{"i\u0064":"esc2"}`, `{"\u0049D":"esc3"}`, `{"id":"a","w":1,"x":[1,{"y":[2]}],"h":2}`,
		`{"id":null,"price":null,"h":null}`, `{"w":-1}`, `{"tags":["a",1,"b"]}`,
		`{"ext":[1,2}`, `{"ext":"unterminated}`, `{"id":"x","price":1e309,"h":1}`,
		`{"w":9223372036854775807,"h":9223372036854775808}`, `{"":1}`, `{"id":"a",,"w":1}`,
	}},
	{targetOf[Wide](), []string{
		`{"a":"x","b":1,"c":255,"d":-128,"e":1.5,"f":true,"g":"s","h":[1,2],"i":{"k":1},` +
			`"j":{"z":[1,"2",null,true]},"k":[{"id":"a"},{"id":"b","w":1}],"l":{"id":"l"},` +
			`"m":[1, 2],"n":"AQID","o":{"10":"a","9":"b"},"p":{"x": 1}}`,
		`{"A":"x","B":2}`, `{"c":256}`, `{"d":128}`, `{"e":3.5e38}`, `{"f":"true"}`,
		`{"g":null}`, `{"h":null}`, `{"h":[]}`, `{"i":null}`, `{"i":[]}`, `{"j":null}`,
		`{"k":[{"id":1}]}`, `{"l":null}`, `{"m":null}`, `{"n":"!"}`, `{"n":[1,2]}`,
		`{"n":null}`, `{"o":{"x":"a"}}`, `{"o":{"1":2}}`, `{"p":null}`, `{"p":"x"}`,
		`{"z":1,"a":`, `{"a":"x","b":"y","c":"z"}`, `{"b":1.5}`, `{"b":1e2}`, `{"b":-}`,
		`{"k":[{"w":"x"}]}`, `{"a":"x","a":"y"}`, `{"AA":1}`, `{"h":[1,"x",3]}`,
		`{"i":{"a":"x"}}`, `{"j":[1e400]}`, `{"l":{"id":"x","h":"y"}}`, `{"e":"x","f":1}`,
		`{"n":"AQID","n":null}`, `{"o":{"9223372036854775808":"x"}}`, `{"c":-1}`,
	}},
	{targetOf[Upper](), []string{
		`{"ID":"a","Name":"n","count":1,"internal":5,"Skipped":"s","-":"d"}`,
		`{"id":"a","name":"n","COUNT":2}`, `{"Id":"x"}`, `{"Dash":"d"}`, `{"skipped":"s"}`,
		`{"ID":1}`, `{"Name":null}`, `{"count":"1"}`, `{"name":1,"ID":2}`, `{"-":1}`,
	}},
	{targetOf[Embeds](), []string{
		`{"id":"a","name":"n","dup":1,"level":2,"note":"x","own":3}`, `{"level":5}`,
		`{"dup":7}`, `{"note":null}`, `{"name":1}`, `{"level":"x"}`, `{"own":1,"level":"x"}`,
		`{"Note":"case"}`, `{"NAME":"upper"}`, `{"id":"a","level":1,"id":2}`, `{"Level":1}`,
	}},
	{targetOf[Quoted](), []string{
		`{"N":"1","s":"\"x\"","p":"2","b":"true","f":"1.5","u":"7"}`, `{"N":1}`, `{"N":"x"}`,
		`{"N":null}`, `{"N":"null"}`, `{"p":null}`, `{"p":"null"}`, `{"s":"x"}`, `{"s":null}`,
		`{"s":1}`, `{"b":"maybe"}`, `{"b":true}`, `{"f":"1e400"}`, `{"u":"70000"}`,
		`{"u":"-1"}`, `{"N":" 1"}`, `{"N":"1 "}`, `{"N":"01"}`, `{"N":""}`, `{"n":"5"}`,
		`{"s":"\"\\u00e9\""}`, `{"s":"\"a\" "}`, `{"s":""}`, `{"b":"null"}`, `{"p":"x"}`,
		`{"N":"1","N":"2x"}`, `{"f":"1.5","b":"x","u":"1"}`, `{"s":"\"x"}`,
	}},
	{targetOf[Custom](), []string{
		`{"si":5,"is":"x","v":5,"pv":7}`, `{"si":"5"}`, `{"si":"x"}`, `{"si":true}`,
		`{"si":null}`, `{"si":""}`, `{"si":"-"}`, `{"si":1.5}`, `{"si":99999999999999999999}`,
		`{"is":1.5}`, `{"is":12}`, `{"is":true}`, `{"is":null}`, `{"is":[1]}`, `{"v":101}`,
		`{"v":"x"}`, `{"v":null}`, `{"pv":null}`, `{"pv":101}`, `{"v":5,"si":"x"}`,
		`{"si":"x","v":101}`, `{"v":[1, 2]}`, `{"si":-12,"is":-1.50}`, `{"is":"a\"b"}`,
	}},
	{targetOf[Loud](), []string{
		`{"s":"hi","t":"x","p":"y"}`, `{"s":1}`, `{"p":null}`, `{"T":"case"}`,
	}},
	{targetOf[Maps](), []string{
		`{"smalls":{"a":{"id":"x"},"b":{"w":1}},"ptrs":{"a":1,"b":null},` +
			`"bools":{"1":true,"255":false},"raw":{"a":[1, 2],"b":null}}`,
		`{"bools":{"256":true}}`, `{"bools":{"x":true}}`, `{"bools":{"-1":true}}`,
		`{"smalls":{"a":1}}`, `{"smalls":null}`, `{"ptrs":{"a":"x"}}`, `{"raw":{"a":}}`,
		`{"smalls":{"a":{"id":"x"}},"smalls":{"b":{}}}`, `{"bools":{"01":true}}`,
		`{"bools":{" 1":true}}`, `{"bools":{"1":"x"}}`, `{"smalls":{"a":{"id":1}}}`,
		`{"ptrs":{"a":1,"a":2}}`, `{"raw":{"b":{"c" : 1}}}`, `{"smalls":[]}`,
	}},
	{targetOf[Empty](), []string{
		`{}`, `{"a":1}`, `[]`, `null`, `"x"`, `1`, `{"a":}`, `{"a":1`, `{"a" 1}`, `{"a":[1,}`,
	}},
	{targetOf[OnlyPrivate](), []string{
		`{}`, `{"a":1}`, `[]`, `null`, `"x"`, `{"a":}`, `{"b":"x","a":2}`,
	}},
	{targetOf[[]Small](), []string{
		`[{"id":"a"},{"id":"b"}]`, `[{"id":1}]`, `[1]`, `{}`, `null`, `[null]`, `[{},]`,
	}},
	{targetOf[map[string]Small](), []string{
		`{"x":{"id":"a"}}`, `{"x":1}`, `{"x":null}`, `{"x":{"id":"a"},"y":{"w":"z"}}`,
	}},
	{targetOf[*Small](), []string{`null`, `{"id":"a"}`, `1`, `{"id":"a","w":"x"}`}},
	{targetOf[[]Validated](), []string{`[1,2]`, `[1,200,3]`, `[null]`, `[]`, `null`}},
	{targetOf[Numbers](), []string{
		`{"n":1.5e3,"o":"","s":"12","p":-0,"l":[1,"2",null]}`, `{"n":"abc"}`, `{"n":"1e+"}`,
		`{"n":true}`, `{"n":{}}`, `{"n":[1]}`, `{"n":null}`, `{"n":+1}`, `{"n":.5}`, `{"n":1-2}`,
		`{"n":-}`, `{"n":"1"}`, `{"n":"-0.0e-0"}`, `{"n":"01"}`, `{"s":"\"7\""}`,
		`{"s":"12abc"}`, `{"s":12}`, `{"s":""}`, `{"s":null}`, `{"s":"null"}`, `{"s":"true"}`,
		`{"s":"-"}`, `{"s":"\"x\""}`, `{"p":null}`, `{"p":"x"}`, `{"p":7}`, `{"l":["a",1]}`,
		`{"l":{}}`,
	}},
	{targetOf[Anon](), []string{
		`{"prebid":{"type":"banner","meta":{"count":2,"price":1.5}},"list":[{"a":1},{"b":["x"]}]}`,
		`{"prebid":{"type":1}}`, `{"prebid":1}`, `{"prebid":{"meta":{"count":"x"}}}`,
		`{"prebid":{"meta":1}}`, `{"prebid":{"meta":null}}`, `{"list":[{"a":"x"}]}`,
		`{"list":{}}`, `{"list":[{"b":[1]}]}`, `{"prebid":{"type":"a"},"prebid":{"meta":{}}}`,
		`{"prebid":null}`, `{"PREBID":{"TYPE":"v"}}`, `{"prebid":{"type":"x",}}`,
		`{"list":[{"a":1},]}`, `{"prebid":{"meta":{"price":"1"}}}`, `{"prebid":[]}`,
	}},
	{targetOf[AnonTop](), []string{
		`{"a":1,"b":["x"]}`, `{"a":"x"}`, `1`, `{"b":[1]}`, `null`, `{"a":1e400}`, `[]`,
		`{"b":"x"}`, `{"A":2,"B":null}`, `{"a":1,}`,
	}},
	{targetOf[*AnonMeta](), []string{`null`, `{"count":1}`, `{"price":"x"}`, `[1]`}},
}

// redecodes decode a prefill document and then the input into the same value.
var redecodes = []struct {
	target          target
	prefill, inputs string
}{
	{targetOf[Small](), `{"id":"a","w":1,"tags":["x","y","z"],"ext":{}}`, `{"tags":["q"],"w":null}`},
	{targetOf[[]Small](), `[{"id":"a","h":1},{"id":"b","h":2}]`, `[{"w":5}]`},
	{targetOf[Maps](), `{"smalls":{"a":{"id":"x","h":1}},"bools":{"1":true}}`, `{"smalls":{"a":{"w":2},"b":{}},"bools":{"2":false}}`},
	{targetOf[Wide](), `{"j":{"a":1},"g":"s","l":{"id":"x"},"h":[1,2,3],"n":"AQID"}`, `{"j":{"b":2},"g":null,"l":{"h":3},"h":[9],"n":null}`},
	{targetOf[Embeds](), `{"level":1,"id":"x"}`, `{"own":2}`},
	{targetOf[Quoted](), `{"p":"5","N":"3"}`, `{"p":null,"N":null}`},
	{targetOf[Custom](), `{"si":5,"v":5,"pv":6}`, `{"si":"","v":7,"pv":null}`},
	{targetOf[Small](), `{"id":"a","price":2.5,"h":3}`, `{"id":null,"price":null,"h":"x"}`},
}

// decodeRow runs one input through all three decoders. It reports false when Go panics
// (jsonutil.StringInt slices a lone quote out of range); those inputs are skipped.
func decodeRow(t target, prefill, input []byte) (row [][]byte, ok bool) {
	defer func() {
		if recover() != nil {
			ok = false
		}
	}()
	v := t.newValue()
	if prefill != nil {
		_ = iterAPI.Unmarshal(prefill, v)
	}
	err := iterAPI.Unmarshal(input, v)
	row = append(row, errText(err), value(v))
	v = t.newValue()
	if prefill != nil {
		_ = jsonutil.Unmarshal(prefill, v)
	}
	row = append(row, errText(jsonutil.Unmarshal(input, v)))
	v = t.newValue()
	if prefill != nil {
		_ = json.Unmarshal(prefill, v)
	}
	err = json.Unmarshal(input, v)
	row = append(row, errText(err), value(v))
	return row, true
}

func writeDecode(dir string) {
	g := create(dir, "decode.txt", "type\tprefill\tinput\tjsoniter error\tjsoniter value (json.Marshal)\t"+
		"jsonutil.Unmarshal error\tencoding/json error\tencoding/json value (json.Marshal)")
	defer g.close()
	skipped := 0
	emit := func(t target, prefill, input []byte) {
		row, ok := decodeRow(t, prefill, input)
		if !ok {
			skipped++
			return
		}
		g.row(append([][]byte{[]byte(t.name), prefill, input}, row...)...)
	}
	for _, t := range scalarTargets {
		for _, input := range generalInputs {
			emit(t, nil, []byte(input))
		}
	}
	for _, st := range structTargets {
		for _, input := range st.inputs {
			emit(st.target, nil, []byte(input))
		}
	}
	for _, r := range redecodes {
		emit(r.target, []byte(r.prefill), []byte(r.inputs))
	}
	fmt.Printf("decode: %d panicking cases skipped\n", skipped)
}

func encodeRow(v interface{}) [][]byte {
	util, err := jsonutil.Marshal(v)
	jsonutilOut := output(util, err)
	std, err := json.Marshal(v)
	stdOut := output(std, err)
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	err = enc.Encode(v)
	return [][]byte{jsonutilOut, stdOut, output(buf.Bytes(), err)}
}

var encodeInputs = []struct {
	target target
	inputs []string
}{
	{targetOf[string](), []string{
		`"<a&b>\u2028\u2029\u00e9\ud83d\ude00\u0001\u001f\"\\/\u007f"`, `"\ufffd"`, `""`,
		`"\b\f\n\r\t"`,
	}},
	{targetOf[float64](), []string{
		`0`, `-0`, `1e21`, `1e-7`, `123456789.123`, `0.000001`, `1e20`, `5e-324`,
		`1.7976931348623157e308`, `0.1`, `100`, `-1.5e-10`,
	}},
	{targetOf[float32](), []string{`3.4028234663852886e38`, `1e-7`, `16777217`, `0.1`, `1e21`}},
	{targetOf[[]byte](), []string{`"AQID"`, `null`, `""`, `"/+/+"`}},
	{targetOf[map[int]string](), []string{`{"10":"a","9":"b","-1":"c"}`, `{}`, `null`}},
	{targetOf[map[string]interface{}](), []string{
		`{"b":1,"a":[true,null,"x",{"z":1.5,"y":"<"}]}`, `{"<":"&","\u2028":1}`,
	}},
	{targetOf[interface{}](), []string{`[1e21,1e-7,0.1,-0,100,12345678901234567890]`, `null`}},
	{targetOf[[]int](), []string{`[]`, `null`, `[1,-2]`}},
	{targetOf[*int](), []string{`null`, `5`}},
	{targetOf[json.RawMessage](), []string{` { "a" : [ 1 , 2 ] , "b":"<x>" } `, `null`, `"\u2028"`}},
	{targetOf[*json.RawMessage](), []string{`null`, `[1, 2]`}},
	{targetOf[[]json.RawMessage](), []string{`[{"a" : 1}, null, "<"]`}},
	{targetOf[map[string]json.RawMessage](), []string{`{"b":[1, 2],"a":{"x" : "<"}}`}},
	{targetOf[Small](), []string{
		`{"id":"<x>","w":0,"h":0,"price":1e21,"tags":[],"ext":{"a" : [1, 2]}}`, `{"ext":null}`,
		`{}`, `{"tags":null,"w":5,"ext":"\u2028"}`,
	}},
	{targetOf[Wide](), []string{
		`{"a":"x","b":1,"c":255,"d":-128,"e":1.5,"f":true,"g":"s","h":[1,2],"i":{"k":1,"a":2},` +
			`"j":{"z":[1,"2",null,true]},"k":[{"id":"a"},{"id":"b","w":1}],"l":{"id":"l"},` +
			`"m":[1, 2],"n":"AQID","o":{"10":"a","9":"b"},"p":{"x": 1}}`,
		`{}`, `{"h":[],"i":{},"k":[],"n":""}`,
	}},
	{targetOf[Upper](), []string{`{"ID":"a","Name":"n","count":1,"-":"d"}`, `{}`}},
	{targetOf[Embeds](), []string{
		`{"id":"a","name":"n","dup":1,"level":2,"note":"x","own":3}`, `{"id":"a"}`,
		`{"level":0}`,
	}},
	{targetOf[Quoted](), []string{`{"N":"1","s":"\"x\"","p":"2","b":"true","f":"1.5","u":"7"}`, `{}`}},
	{targetOf[Custom](), []string{`{"si":5,"is":"x","v":5,"pv":7}`, `{}`}},
	{targetOf[Loud](), []string{`{"s":"hi","t":"there","p":"x"}`, `{"s":"bad"}`, `{"s":"<b>"}`, `{"t":"x"}`}},
	{targetOf[Maps](), []string{
		`{"smalls":{"b":{"id":"x"},"a":{"w":1}},"ptrs":{"a":1,"b":null},` +
			`"bools":{"10":true,"9":false},"raw":{"a":[1, 2],"b":null}}`,
		`{}`,
	}},
	{targetOf[Empty](), []string{`{}`}},
	{targetOf[OnlyPrivate](), []string{`{}`}},
	{targetOf[json.Number](), []string{`0`, `-1.5e+3`, `"12"`, `null`, `1E400`}},
	{targetOf[map[Key]int](), []string{`{"b":1,"a":2,"<":3}`, `{}`, `null`}},
	{targetOf[map[Slot]string](), []string{`{"10":"a","9":"b","-1":"c"}`}},
	{targetOf[Numbers](), []string{
		`{"n":1,"o":2,"s":"3","p":4,"l":[5,"6"]}`, `{}`, `{"p":null,"l":[]}`, `{"o":0,"s":"-0"}`,
	}},
	{targetOf[Anon](), []string{
		`{"prebid":{"type":"<b>","meta":{"count":2}},"list":[{"a":1,"b":["x"]}]}`, `{}`,
		`{"prebid":{"meta":{}},"list":[]}`,
	}},
	{targetOf[AnonTop](), []string{`{"a":1,"b":["x"]}`, `{}`}},
}

func ptr[T any](v T) *T {
	return &v
}

// specials are values JSON input cannot produce; the Rust tests build the same values by name.
var specials = []struct {
	name  string
	value interface{}
}{
	{"string-invalid-utf8", "a\xffb\xc3"},
	{"string-controls", "\x00\x01\x1f\x7f\"\\/\u2028\u2029<>&"},
	{"float64-nan", math.NaN()},
	{"float64-inf", math.Inf(1)},
	{"float64-neginf", math.Inf(-1)},
	{"float32-nan", float32(math.NaN())},
	{"small-nan", Small{ID: "x", Price: math.NaN()}},
	{"wide-two-errors", Wide{E: float32(math.Inf(1)), L: &Small{Price: math.NaN()}}},
	{"raw-nil", json.RawMessage(nil)},
	{"raw-empty", json.RawMessage{}},
	{"raw-invalid", json.RawMessage(`{bad`)},
	{"small-raw-invalid", Small{ID: "a", Ext: json.RawMessage(`{bad`)}},
	{"small-raw-empty", Small{Ext: json.RawMessage{}}},
	{"ptr-raw-nil", (*json.RawMessage)(nil)},
	{"ptr-raw-spaces", ptr(json.RawMessage(" [1, 2] \n"))},
	{"ptr-raw-invalid", ptr(json.RawMessage(`{bad`))},
	{"ptr-raw-to-nil", ptr(json.RawMessage(nil))},
	{"wide-raw", Wide{M: json.RawMessage(`[1, {"a" : 2}]`), P: ptr(json.RawMessage(`{"b" : 3}`))}},
	{"wide-raw-invalid", Wide{M: json.RawMessage(`nope`), P: ptr(json.RawMessage(`nope`))}},
	{"loud-empty", Loud{}},
	{"loud-ptr-bad", Loud{S: "a", P: ptr(Shout("bad"))}},
	{"bytes-nil", []byte(nil)},
	{"bytes-empty", []byte{}},
	{"maps-raw-invalid", Maps{Raw: map[string]json.RawMessage{"a": json.RawMessage(`{x`), "b": nil}}},
	{"embeds-nil-extra", Embeds{Base: Base{ID: "a"}, Name: "n"}},
	{"slice-raw-invalid", []json.RawMessage{json.RawMessage(`1`), json.RawMessage(`{`), nil}},
	{"wide-slice-inf", Wide{I: map[string]int{"a": 1}, K: []Small{{ID: "k", Price: math.Inf(-1)}}}},
	{"number-empty", json.Number("")},
	{"number-invalid", json.Number("12abc")},
	{"number-plus", json.Number("+1")},
	{"number-html", json.Number("<1>")},
	{"numbers-invalid", Numbers{N: "1e+", S: "x"}},
	{"numbers-quoted-invalid", Numbers{N: "1", S: "x<"}},
	{"numbers-empty-ptr", Numbers{P: ptr(json.Number(""))}},
	{"numbers-slice-invalid", []json.Number{"1", "", "x"}},
	{"anon-nan", Anon{Prebid: AnonPrebid{Type: "x", Meta: &AnonMeta{Price: math.NaN()}}}},
	{"anon-meta-inf", &AnonMeta{Count: 1, Price: math.Inf(1)}},
	{"anon-list-meta-nan", []*AnonMeta{{Count: 1}, {Price: math.NaN()}}},
}

func writeEncode(dir string) {
	g := create(dir, "encode.txt", "type\tinput (decoded with encoding/json) or special value name\t"+
		"jsonutil.Marshal\tjson.Marshal\tjson.Encoder with SetEscapeHTML(false)\n"+
		"An output starting with ! is the error text.")
	defer g.close()
	for _, ei := range encodeInputs {
		for _, input := range ei.inputs {
			v := ei.target.newValue()
			if err := json.Unmarshal([]byte(input), v); err != nil {
				log.Fatalf("encode input %s %s: %v", ei.target.name, input, err)
			}
			elem := reflect.ValueOf(v).Elem().Interface()
			g.row(append([][]byte{[]byte(ei.target.name), []byte(input)}, encodeRow(elem)...)...)
		}
	}
	for _, s := range specials {
		g.row(append([][]byte{[]byte("special"), []byte(s.name)}, encodeRow(s.value)...)...)
	}
}

func f64Specials() []float64 {
	return []float64{
		0, math.Copysign(0, -1), 1, -1, 0.1, 1.0 / 3, 100, 1e20, 1e21, 1e-6, 1e-7,
		9.999999999999999e20, 5e-324, math.MaxFloat64, math.SmallestNonzeroFloat64,
		2.2250738585072014e-308, 1 << 53, 1 << 63, 123456789, 0.000001234,
		math.NaN(), math.Inf(1), math.Inf(-1), 1e-5, 12345678901234567890,
		// Exactly halfway between two shortest candidates (Ryu keeps the even digit).
		1125899906842624.25, 1125899906842624.75, -2251799813685247.75, 562949953421311.875,
	}
}

func f32Specials() []float32 {
	return []float32{
		0, float32(math.Copysign(0, -1)), 1, 0.1, 1e20, 1e21, 1e-6, 1e-7, math.MaxFloat32,
		math.SmallestNonzeroFloat32, 16777216, 16777217, 3.4e38, 9.999999e20,
		float32(math.NaN()), float32(math.Inf(1)), float32(math.Inf(-1)),
		// Exactly halfway between two shortest candidates (Ryu keeps the even digit).
		3429852.25, 262144.125, -262144.375, 1048576.5,
	}
}

func bits64(f float64) []byte {
	return []byte(fmt.Sprintf("%016x", math.Float64bits(f)))
}

func bits32(f float32) []byte {
	return []byte(fmt.Sprintf("%08x", math.Float32bits(f)))
}

// decimal returns a random decimal number in JSON syntax.
func decimal(rng *rand.Rand) string {
	var b strings.Builder
	if rng.Intn(4) == 0 {
		b.WriteByte('-')
	}
	digits := 1 + rng.Intn(20)
	for i := 0; i < digits; i++ {
		d := byte('0' + rng.Intn(10))
		if i == 0 && digits > 1 && d == '0' {
			d = '1'
		}
		b.WriteByte(d)
	}
	if rng.Intn(2) == 0 {
		b.WriteByte('.')
		frac := 1 + rng.Intn(12)
		for i := 0; i < frac; i++ {
			b.WriteByte(byte('0' + rng.Intn(10)))
		}
	}
	if rng.Intn(3) == 0 {
		b.WriteString([]string{"e", "E", "e+", "e-", "E-"}[rng.Intn(5)])
		b.WriteString(strconv.Itoa(rng.Intn(330)))
	}
	return b.String()
}

func parseRow(text string) [][]byte {
	row := [][]byte{[]byte(text)}
	var f64 float64
	err := iterAPI.Unmarshal([]byte(text), &f64)
	row = append(row, output(bits64(f64), err))
	var f32 float32
	err = iterAPI.Unmarshal([]byte(text), &f32)
	row = append(row, output(bits32(f32), err))
	f64 = 0
	err = json.Unmarshal([]byte(text), &f64)
	row = append(row, output(bits64(f64), err))
	f32 = 0
	err = json.Unmarshal([]byte(text), &f32)
	return append(row, output(bits32(f32), err))
}

func writeFloats(dir string) {
	rng := rand.New(rand.NewSource(20261117))
	g := create(dir, "floats.txt", "f64\tbits\tjsonutil.Marshal\tjson.Marshal\n"+
		"f32\tbits\tjsonutil.Marshal\tjson.Marshal\n"+
		"parse\ttext\tjsoniter float64 bits\tjsoniter float32 bits\tencoding/json float64 bits\t"+
		"encoding/json float32 bits\n"+
		"number\ttext\tjson.Number Int64\tjson.Number Float64 bits\n"+
		"An output starting with ! is the error text.")
	defer g.close()
	f64s := f64Specials()
	for i := 0; i < 3000; i++ {
		f64s = append(f64s, math.Float64frombits(rng.Uint64()))
	}
	for i := 0; i < 500; i++ {
		// Values near the 'e' format cutoffs and with few digits.
		f64s = append(f64s, float64(rng.Intn(100000))*math.Pow(10, float64(rng.Intn(40)-25)))
	}
	for _, f := range f64s {
		util, err := jsonutil.Marshal(f)
		std, stdErr := json.Marshal(f)
		g.row([]byte("f64"), bits64(f), output(util, err), output(std, stdErr))
	}
	f32s := f32Specials()
	for i := 0; i < 1500; i++ {
		f32s = append(f32s, math.Float32frombits(rng.Uint32()))
	}
	for i := 0; i < 300; i++ {
		f32s = append(f32s, float32(rng.Intn(100000))*float32(math.Pow(10, float64(rng.Intn(30)-20))))
	}
	for _, f := range f32s {
		util, err := jsonutil.Marshal(f)
		std, stdErr := json.Marshal(f)
		g.row([]byte("f32"), bits32(f), output(util, err), output(std, stdErr))
	}
	texts := []string{
		"0", "-0", "0.0", "1e400", "-1e400", "1e-400", "4.9e-324", "2e-324", "3.4028235e38",
		"3.4028236e38", "1.401298464324817e-45", "7e-46", "0.1", "0.123456", "0.1234567",
		"123456789.123456", "9007199254740993", "9007199254740993.5", "1.000000000000000000001",
		"0.000001", "1e", "1e+", "-", "1.", ".1", "01", "1.5e3", "1E-3", "100000000000000000000000",
		"0.30000000000000004", "2.2250738585072011e-308", "8.589973e9", "0.017997",
		"99999999999999999999999999999999999999999999999999.5", "1.17549435e-38",
	}
	for i := 0; i < 1500; i++ {
		texts = append(texts, decimal(rng))
	}
	for _, text := range texts {
		g.row(append([][]byte{[]byte("parse")}, parseRow(text)...)...)
	}
	numbers := []string{
		"1_000", "1_0.5", "_1", "1__0", "1_", "0x10", "0x1p4", "0x1.8p1", "0X_1p0", "0x_1p0",
		"0x1p", "0x1p+", "0x.8p1", "0x1.fffffffffffff8p1023", "0x1.fffffffffffff7p1023",
		"0x1p-1074", "0x1p-1075", "0x1.8p-1074", "0x3p-1076", "0x1.0000000000000801p0",
		"0x1.00000000000008p0", "0x1.00000000000018p0", "0x10000000000000000p0", "0x1p1024",
		"-0x1p1024", "0x0p0", "-0x0p99999", "0x1p-99999", "0x1P-2", "0xAbC.dEfp-3", "0x1_0p0",
		"0x1p1_0", "0x", "0x1g", "inf", "+Inf", "-infinity", "INFINITY", "infin", "infinityx",
		"NaN", "+nan", "nan", "nAn", "nanx", "1e", "1e+5", ".5", "5.", "+.5e-3", "1_000e1_0",
		"1e1_0", "1_e5", "1._5", "1.5_", "99999999999999999999x", "9223372036854775807",
		"9223372036854775808", "-9223372036854775808", "-9223372036854775809",
		"+9223372036854775807", "18446744073709551615", "18446744073709551616",
		"1844674407370955161x", "+-5", "-", "+", "", " 1", "1 ", "0b101", "0o17", "1e400",
		"-1e400", "1e-400", "-0", "00012", "-00", "1.5", "12abc", "1e99999999999",
		"0e99999999999", "4.9e-324", "2.4703282292062328e-324", "2.4703282292062327e-324",
		"123456789012345678901234567890", "0.1e-99999999999", "1..5", "1.5.5", "1e5e5", "١",
	}
	for i := 0; i < 300; i++ {
		numbers = append(numbers, decimal(rng))
	}
	for _, text := range numbers {
		g.row(numberRow(text)...)
	}
}

// numberRow is json.Number's Int64 and Float64 (strconv.ParseInt and ParseFloat) of text.
func numberRow(text string) [][]byte {
	i, err := json.Number(text).Int64()
	ints := []byte(strconv.FormatInt(i, 10))
	if err != nil {
		ints = []byte("!" + err.Error())
	}
	f, err := json.Number(text).Float64()
	floats := bits64(f)
	if err != nil {
		floats = []byte("!" + err.Error())
	}
	return [][]byte{[]byte("number"), []byte(text), ints, floats}
}

type fixture struct {
	path string
	// source is "imp" (imp[0].ext.bidder of the request) or "response" (the first mock
	// response body).
	source string
	// std decodes with encoding/json instead of jsonutil.Unmarshal.
	std    bool
	target target
}

var fixtures = []fixture{
	{"adapters/pulsepoint/pulsepointtest/supplemental/bad-prebid-params.json", "imp", false,
		targetOf[openrtb_ext.ExtImpPulsePoint]()},
	{"adapters/intenze/intenzetest/supplemental/invalid-bidder-ext-object.json", "imp", false,
		targetOf[openrtb_ext.ExtIntenze]()},
	{"adapters/connatix/connatixtest/supplemental/invalid-bid-response.json", "response", false,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/pulsepoint/pulsepointtest/supplemental/bad-bid-data.json", "response", false,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/apacdex/apacdextest/supplemental/invalid-response-unmarshall-error.json", "response",
		false, targetOf[openrtb2.BidResponse]()},
	{"adapters/zentotem/zentotemtest/supplemental/bad_response.json", "response", true,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/thetradedesk/thetradedesktest/supplemental/invalid-publisher.json", "imp", false,
		targetOf[openrtb_ext.ExtImpTheTradeDesk]()},
	{"adapters/thetradedesk/thetradedesktest/supplemental/invalid-supplySourceId.json", "imp", false,
		targetOf[openrtb_ext.ExtImpTheTradeDesk]()},
	{"adapters/imds/imdstest/supplemental/bad_seat_id.json", "imp", false,
		targetOf[openrtb_ext.ExtImpImds]()},
	{"adapters/ix/ixtest/supplemental/multi-imp-requests-error.json", "imp", false,
		targetOf[openrtb_ext.ExtImpIx]()},
	{"adapters/adkernelAdn/adkerneladntest/supplemental/wrong-imp-ext-1.json", "imp", false,
		targetOf[openrtb_ext.ExtImpAdkernelAdn]()},
	{"adapters/cpmstar/cpmstartest/supplemental/invalid-response-unmarshall-error.json", "response",
		false, targetOf[openrtb2.BidResponse]()},
	{"adapters/nexx360/nexx360test/supplemental/invalid-bid-response.json", "response", false,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/aduptech/aduptechtest/supplemental/broken-response.json", "response", false,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/fwssp/fwssptest/supplemental/invalid_bid_response.json", "response", false,
		targetOf[openrtb2.BidResponse]()},
	{"adapters/trustedstack/trustedstacktest/supplemental/valid-req-200-incorrect-response-format.json",
		"response", false, targetOf[openrtb2.BidResponse]()},
	{"adapters/alliance_gravity/alliance_gravitytest/supplemental/invalid-bid-response.json",
		"response", false, targetOf[openrtb2.BidResponse]()},
}

type fixtureFile struct {
	ExpectedMakeRequestsErrors []fixtureError `json:"expectedMakeRequestsErrors"`
	ExpectedMakeBidsErrors     []fixtureError `json:"expectedMakeBidsErrors"`
	MockBidRequest             struct {
		Imp []struct {
			Ext struct {
				Bidder json.RawMessage `json:"bidder"`
			} `json:"ext"`
		} `json:"imp"`
	} `json:"mockBidRequest"`
	HTTPCalls []struct {
		MockResponse struct {
			Body json.RawMessage `json:"body"`
		} `json:"mockResponse"`
	} `json:"httpCalls"`
}

type fixtureError struct {
	Value      string `json:"value"`
	Comparison string `json:"comparison"`
}

// marshalFixtures decode without error, and the adapter's json.Marshal of the decoded value
// fails; the fixture expects the adapter's prefix followed by that error.
var marshalFixtures = []struct {
	fixture
	prefix string
}{
	{fixture{"adapters/adtarget/adtargettest/supplemental/wrong-impression-ext.json", "imp", false,
		targetOf[openrtb_ext.ExtImpAdtarget]()},
		"ignoring imp id=unsupported-native-imp, error while encoding impExt, err: "},
	{fixture{"adapters/adtelligent/adtelligenttest/supplemental/wrong-impression-ext.json", "imp",
		false, targetOf[openrtb_ext.ExtImpAdtelligent]()},
		"ignoring imp id=unsupported-native-imp, error while marshaling impExt, err: "},
	{fixture{"adapters/bidmatic/bidmatictest/supplemental/wrong-impression-ext.json", "imp", false,
		targetOf[openrtb_ext.ExtImpBidmatic]()},
		"ignoring imp id=unsupported-native-imp, error while marshaling impExt, err: "},
}

func writeFixtures(dir, root string) {
	g := create(dir, "fixtures.txt", "fixture\tdecoder (jsonutil, std or jsonutil+marshal)\ttype\tinput\t"+
		"expected error\tcomparison\tfull error\tadapter prefix\n"+
		"The input is taken from the adapter test fixture; the adapter's prefix and Go's error satisfy\n"+
		"the fixture. jsonutil+marshal decodes without error and then fails in json.Marshal.")
	defer g.close()
	for _, fx := range fixtures {
		writeFixture(g, root, fx, false, "")
	}
	for _, fx := range marshalFixtures {
		writeFixture(g, root, fx.fixture, true, fx.prefix)
	}
}

func writeFixture(g *golden, root string, fx fixture, marshal bool, prefix string) {
	data, err := os.ReadFile(filepath.Join(root, fx.path))
	if err != nil {
		log.Fatal(err)
	}
	var file fixtureFile
	if err := json.Unmarshal(data, &file); err != nil {
		log.Fatalf("%s: %v", fx.path, err)
	}
	var input []byte
	var expected fixtureError
	if fx.source == "imp" {
		input = file.MockBidRequest.Imp[0].Ext.Bidder
		expected = file.ExpectedMakeRequestsErrors[0]
	} else {
		input = file.HTTPCalls[0].MockResponse.Body
		expected = file.ExpectedMakeBidsErrors[0]
	}
	v := fx.target.newValue()
	decoder := "jsonutil"
	if fx.std {
		decoder = "std"
		err = json.Unmarshal(input, v)
	} else {
		err = jsonutil.Unmarshal(input, v)
	}
	if marshal {
		if err != nil {
			log.Fatalf("%s: decoding failed: %v", fx.path, err)
		}
		decoder = "jsonutil+marshal"
		_, err = json.Marshal(v)
	}
	if err == nil {
		log.Fatalf("%s: no error", fx.path)
	}
	got := prefix + err.Error()
	matches := got == expected.Value
	if expected.Comparison == "startswith" {
		matches = strings.HasPrefix(got, expected.Value)
	}
	if !matches {
		log.Fatalf("%s: %q does not satisfy %q (%s)", fx.path, got, expected.Value, expected.Comparison)
	}
	g.row([]byte(fx.path), []byte(decoder), []byte(fx.target.name), input,
		[]byte(expected.Value), []byte(expected.Comparison), []byte(got), []byte(prefix))
}

func main() {
	out := flag.String("out", "rust/crates/gojson/testdata/golden", "output directory")
	root := flag.String("root", ".", "repository root (for adapter fixtures)")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	writeDecode(*out)
	writeEncode(*out)
	writeFloats(*out)
	writeFixtures(*out, *root)
}
