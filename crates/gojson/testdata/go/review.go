package main

import (
	"encoding/json"
	"fmt"
	"math/rand"
	"reflect"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Ten has exactly ten lower-case JSON names, so jsoniter decodes it with its largest hashed
// decoder (tenFieldsStructDecoder), which lower-cases only the ASCII bytes of a key.
type Ten struct {
	A int            `json:"a"`
	B string         `json:"b"`
	C *int           `json:"c"`
	D []int          `json:"d"`
	S bool           `json:"s"`
	F float64        `json:"f"`
	K string         `json:"k"`
	P *Small         `json:"p"`
	M map[string]int `json:"m"`
	G string         `json:"σ"`
}

// Eleven is Ten with an eleventh field, so jsoniter uses its generalStructDecoder, which
// matches a key exactly and then by strings.ToLower.
type Eleven struct {
	A int            `json:"a"`
	B string         `json:"b"`
	C *int           `json:"c"`
	D []int          `json:"d"`
	S bool           `json:"s"`
	F float64        `json:"f"`
	K string         `json:"k"`
	P *Small         `json:"p"`
	M map[string]int `json:"m"`
	G string         `json:"σ"`
	X int8           `json:"x"`
}

// Ints holds every integer kind, to decode numbers inside a long buffer (jsoniter's
// readUint32/readUint64 fast paths) and with ",string".
type Ints struct {
	I8  int8   `json:"i8"`
	I16 int16  `json:"i16"`
	I32 int32  `json:"i32"`
	I64 int64  `json:"i64"`
	I   int    `json:"i"`
	U8  uint8  `json:"u8"`
	U16 uint16 `json:"u16"`
	U32 uint32 `json:"u32"`
	U64 uint64 `json:"u64"`
	U   uint   `json:"u"`
	Pad string `json:"pad"`
}

// Tags has JSON names that encoding/json's isValidTag rejects, so it uses the Go field names
// (jsoniter takes every name as written): a superscript digit (No), a Devanagari vowel sign
// (Mc), a Roman numeral (Nl) and U+A7DC, which Go's Unicode 15.0.0 does not assign. The Greek
// letter and the Arabic-Indic digit are a letter and a digit to both libraries.
type Tags struct {
	Sup   int `json:"x²"`
	Mark  int `json:"नाम"`
	Roman int `json:"Ⅳ"`
	New   int `json:"Ƛ"`
	Greek int `json:"σ"`
	Digit int `json:"x٣"`
}

// Lambda is named by U+A7DC, unassigned in Go's Unicode 15.0.0 (Unicode 16.0.0 made it the
// upper case of U+019B): Go neither lower-cases nor folds it, so jsoniter keeps its hashed
// decoder and neither library matches U+019B to it.
type Lambda struct {
	L int `json:"Ƛ"`
	S int `json:"σ"`
}

const tenSeed = `{"a":1,"b":"x","c":2,"d":[1,2],"s":true,"f":1.5,"k":"k","p":{"id":"p","w":1},` +
	`"m":{"a":1},"σ":"g"}`

const elevenSeed = `{"a":1,"b":"x","c":2,"d":[1,2],"s":true,"f":1.5,"k":"k","p":{"id":"p","w":1},` +
	`"m":{"a":1},"σ":"g","x":-5}`

const bidResponseSeed = `{"id":"r1","seatbid":[{"bid":[{"id":"b1","impid":"i1","price":1.25,` +
	`"adm":"<div>&amp;</div>","crid":"c","w":300,"h":250,"mtype":1,"cat":["IAB1"],` +
	`"ext":{"prebid":{"type":"banner"}}}],"seat":"s"}],"cur":"USD","nbr":2}`

// fuzzTargets are decoded with seeds mutated at random: errors land at every kind of position,
// and jsoniter's error windows cut through multi-byte runes.
var fuzzTargets = []struct {
	target target
	seeds  []string
}{
	{targetOf[Small](), []string{
		`{"id":"a","w":300,"h":250,"price":1.5,"tags":["x","y"],"ext":{"k":[1, 2]}}`,
		`{"ID":"<é>","W":null,"tags":[],"ext":null,"price":-0,"h":-7}`,
	}},
	{targetOf[Wide](), []string{
		`{"a":"x","b":1,"c":255,"d":-128,"e":1.5,"f":true,"g":"s","h":[1,2],"i":{"k":1},` +
			`"j":{"z":[1,"2",null,true]},"k":[{"id":"a"},{"id":"b","w":1}],"l":{"id":"l"},` +
			`"m":[1, 2],"n":"AQID","o":{"10":"a","9":"b"},"p":{"x": 1}}`,
	}},
	{targetOf[Ten](), []string{tenSeed}},
	{targetOf[Eleven](), []string{elevenSeed}},
	{targetOf[Upper](), []string{`{"ID":"a","Name":"n","count":1,"internal":5,"Skipped":"s","-":"d"}`}},
	{targetOf[Embeds](), []string{`{"id":"a","name":"n","dup":1,"level":2,"note":"x","own":3}`}},
	{targetOf[Quoted](), []string{`{"N":"1","s":"\"x\"","p":"2","b":"true","f":"1.5","u":"7"}`}},
	{targetOf[Custom](), []string{`{"si":5,"is":"x","v":5,"pv":7}`, `{"si":"12","is":1.50,"pv":null}`}},
	{targetOf[Maps](), []string{
		`{"smalls":{"a":{"id":"x"},"b":{"w":1}},"ptrs":{"a":1,"b":null},` +
			`"bools":{"1":true,"255":false},"raw":{"a":[1, 2],"b":null}}`,
	}},
	{targetOf[Numbers](), []string{`{"n":1.5e3,"o":"","s":"12","p":-0,"l":[1,"2",null]}`}},
	{targetOf[Anon](), []string{
		`{"prebid":{"type":"banner","meta":{"count":2,"price":1.5}},"list":[{"a":1},{"b":["x"]}]}`,
	}},
	{targetOf[Ints](), []string{
		`{"i8":-128,"i16":32767,"i32":-2147483648,"i64":9223372036854775807,"i":-1,"u8":255,` +
			`"u16":65535,"u32":4294967295,"u64":18446744073709551615,"u":0,"pad":"0123456789"}`,
	}},
	{targetOf[openrtb2.BidResponse](), []string{bidResponseSeed}},
	{targetOf[[]Small](), []string{`[{"id":"a"},{"id":"b","w":2}]`}},
	{targetOf[map[string]Small](), []string{`{"x":{"id":"a"},"y":{"h":2}}`}},
	{targetOf[*Small](), []string{`{"id":"a","w":1}`}},
	{targetOf[[]int](), []string{`[1,-2,30000000000,4]`}},
	{targetOf[[]*int](), []string{`[1,null,3]`}},
	{targetOf[[]string](), []string{`["a","\u00e9","<>"]`}},
	{targetOf[map[string]int](), []string{`{"a":1,"b":-2}`}},
	{targetOf[map[int]string](), []string{`{"1":"a","-2":"b"}`}},
	{targetOf[map[string]interface{}](), []string{`{"a":[1,2.5,"x",null,true,{"b":-0}],"c":"\u2028"}`}},
	{targetOf[interface{}](), []string{`[{"a":1},"x",1.5e3,-0,null,true,[]]`}},
	{targetOf[[]json.RawMessage](), []string{`[{"a" : 1}, null, "<", [ ]]`}},
	{targetOf[json.Number](), []string{`-1.5e+3`}},
}

// fuzzTokens are what mutations insert.
var fuzzTokens = []string{
	"{", "}", "[", "]", ",", ":", `"`, `\`, " ", "\t", "\n", "\r", "0", "1", "7", "-", "+", ".",
	"e", "E", "n", "u", "l", "t", "r", "f", "a", "x", "\x00", "\x01", "\x1f", "\x7f", "\x80",
	"\xff", "\xc3", "\xe2\x80", "\u2028", "é", "😀", "null", "true", "false", "nul", "1e400", "-0",
	"0.5", "1.5", "01", `""`, "[]", "{}", `\u00e9`, `\ud800`, `\u`, `\n`, `\"`, `"\`,
	"18446744073709551616", "9223372036854775808", "-129", "256", "12345678901", "<>&", `"K"`,
	`"\u212a"`, `"ſ"`, `"Σ"`, `"ς"`, `"İD"`, `"ID"`, `"Id"`, `null:`, `"x":`, `"pad":"`,
}

// padRunes fill the long runs that mutations insert.
var padRunes = []string{"é", "😀", "\u2028", "a", " ", "\xff", "€"}

func splice(b []byte, start, end int, insert string) []byte {
	out := make([]byte, 0, len(b)+len(insert))
	out = append(out, b[:start]...)
	out = append(out, insert...)
	return append(out, b[end:]...)
}

func mutateOnce(rng *rand.Rand, b []byte) []byte {
	pos := rng.Intn(len(b) + 1)
	switch rng.Intn(10) {
	case 0, 1, 2:
		return splice(b, pos, pos, fuzzTokens[rng.Intn(len(fuzzTokens))])
	case 3:
		end := pos + 1 + rng.Intn(3)
		if end > len(b) {
			end = len(b)
		}
		return splice(b, pos, end, "")
	case 4:
		if pos == len(b) {
			return b
		}
		return splice(b, pos, pos+1, fuzzTokens[rng.Intn(len(fuzzTokens))])
	case 5:
		return b[:pos]
	case 6:
		start := rng.Intn(len(b) + 1)
		end := start + rng.Intn(len(b)-start+1)
		return splice(b, pos, pos, string(b[start:end]))
	case 7:
		var pad strings.Builder
		for i := 10 + rng.Intn(50); i > 0; i-- {
			pad.WriteString(padRunes[rng.Intn(len(padRunes))])
		}
		// After a quote the run usually lands inside a string.
		if q := strings.IndexByte(string(b[pos:]), '"'); q >= 0 {
			pos += q + 1
		}
		return splice(b, pos, pos, pad.String())
	case 8:
		return splice(b, pos, pos, strings.Repeat(" ", 40+rng.Intn(40)))
	default:
		return []byte(strings.Repeat(" ", rng.Intn(60)) + string(b) + strings.Repeat("\n", rng.Intn(60)))
	}
}

func mutate(rng *rand.Rand, seed string) string {
	b := []byte(seed)
	for n := 1 + rng.Intn(3); n > 0; n-- {
		b = mutateOnce(rng, b)
	}
	return string(b)
}

// lossyTarget reports whether the Rust mirror of the type holds Go strings in serde_json::Value
// or String, which cannot keep invalid UTF-8 (a documented limit of the Rust crate).
func lossyTarget(name string) bool {
	return strings.Contains(name, "interface") || name == "main.Wide" || name == "main.Upper"
}

// numberText is a random integer-like text: lengths around the 32- and 64-bit limits, signs,
// fractions and exponents.
func numberText(rng *rand.Rand) string {
	var b strings.Builder
	switch rng.Intn(6) {
	case 0:
		b.WriteByte('-')
	case 1:
		if rng.Intn(4) == 0 {
			b.WriteByte('+')
		}
	}
	digits := 1 + rng.Intn(22)
	for i := 0; i < digits; i++ {
		d := byte('0' + rng.Intn(10))
		if i == 0 && rng.Intn(5) != 0 && d == '0' {
			d = '1'
		}
		b.WriteByte(d)
	}
	switch rng.Intn(8) {
	case 0:
		b.WriteString("." + strconv.Itoa(rng.Intn(100)))
	case 1:
		b.WriteString([]string{"e", "E", "e+", "e-"}[rng.Intn(4)] + strconv.Itoa(rng.Intn(25)))
	case 2:
		b.WriteString([]string{".", "e", "-", "x", "..1", "]"}[rng.Intn(6)])
	}
	return b.String()
}

// limits are the integer texts at the edges of each kind.
var limits = []string{
	"127", "128", "-128", "-129", "255", "256", "32767", "32768", "-32768", "-32769", "65535",
	"65536", "2147483647", "2147483648", "-2147483648", "-2147483649", "4294967295",
	"4294967296", "9223372036854775807", "9223372036854775808", "-9223372036854775808",
	"-9223372036854775809", "18446744073709551615", "18446744073709551616",
	"18446744073709551619", "18446744073709551620", "30000000000000000000",
	"99999999999999999999", "184467440737095516150", "1844674407370955161", "1844674407370955162",
	"429496729", "4294967290", "4294967299", "42949672950", "-0", "00", "-01", "0.0", "1.0",
	"-1.5", "0.5", "-0.5", "1e2", "1E+2", "1.", "-", "--1", "+1", "0x10", "1_0",
}

// reviewInputs are targeted cases for the areas a port most easily gets wrong.
func reviewInputs(rng *rand.Rand) []struct {
	target target
	inputs []string
} {
	keys := []string{
		`"k"`, `"K"`, `"\u212a"`, "\"\u212a\"", `"s"`, `"S"`, "\"\u017f\"", `"\u017f"`, `"σ"`,
		`"Σ"`, `"ς"`, `"\u03a3"`, `"İ"`, `"ı"`, `"A"`, `"a"`, `"\u0041"`, "\"\xff\"", `"P"`,
		`"\u0070"`, "\"k\x01\"", "\"\\u006b\x01\"", "\"\x01\\u006b\"", `null`, `1`, `"x"`,
	}
	var tenKeys, elevenKeys []string
	for _, k := range keys {
		member := "{" + k + `:"v","a":1}`
		tenKeys = append(tenKeys, member)
		elevenKeys = append(elevenKeys, member)
	}
	dups := []string{
		`{"p":{"id":"a"},"p":{"w":1}}`, `{"p":{"id":"a"},"p":null,"p":{"w":1}}`,
		`{"p":{"id":"a"},"p":{"id":1}}`, `{"p":{"id":"a","tags":["x","y"]},"p":{"tags":["z"]}}`,
		`{"m":{"a":1},"m":{"b":2}}`, `{"m":{"a":1},"m":null}`, `{"d":[1,2,3],"d":[4]}`,
		`{"c":1,"c":null}`, `{"c":null,"c":2}`, `{"p":{"w":1},"P":{"h":2}}`, `{"b":"x","B":null}`,
		`{"a":1,"a":"x"}`, `{"p":{"id":"x"},"p":[]}`,
	}
	tenKeys = append(tenKeys, dups...)
	elevenKeys = append(elevenKeys, dups...)
	// Errors after long multi-byte runs: the ±10 and ±50 byte windows split runes.
	var windows []string
	for k := 0; k < 30; k++ {
		run := strings.Repeat("é", k)
		windows = append(windows, `{"b":"`+run+`",x}`, `{"b":"`+run+`"`, `{"b":"`+run+`","a":x`+run+`}`)
		windows = append(windows, "x"+strings.Repeat("😀", k), strings.Repeat(" ", k)+`{"a":}`)
	}
	ints := []string{}
	for _, l := range limits {
		ints = append(ints,
			`{"i8":`+l+`,"pad":"0123456789abcdef"}`, `{"u8":`+l+`}`, `{"i16":`+l+`,"pad":""}`,
			`{"u16":`+l+`}`, `{"i32":`+l+`,"pad":"0123456789abcdef"}`, `{"u32":`+l+`}`,
			`{"i64":`+l+`,"pad":"0123456789abcdef"}`, `{"u64":`+l+`}`,
			`{"i":`+l+`,"pad":"0123456789abcdef"}`, `{"u":`+l+`}`, `{"u64":`+l+`,"pad":"x"}`)
	}
	var scalars []string
	for i := 0; i < 120; i++ {
		scalars = append(scalars, numberText(rng))
	}
	scalars = append(scalars, limits...)
	var padded []string
	for _, s := range scalars {
		padded = append(padded, "["+s+",0,0,0,0,0,0,0]", "["+s+"]")
	}
	// Numbers that end before the end of the input take jsoniter's fast float paths, which
	// divide by a power of ten instead of calling strconv.ParseFloat.
	var floats, prices []string
	for i := 0; i < 700; i++ {
		f := decimal(rng)
		if i%3 == 0 {
			f = numberText(rng)
		}
		floats = append(floats, "["+f+"]", "["+f+" ,1]")
		if i%5 == 0 {
			prices = append(prices, `{"price":`+f+`}`, `{"price":`+f+`,"id":"0123456789"}`)
		}
	}
	for _, f := range floatTexts {
		floats = append(floats, "["+f+"]", "[-"+f+"]")
	}
	nulls := []string{`null`, ` null`, `nul`, `nulx`, `[null]`, `{"a":null}`, `{"a":nul}`}
	return []struct {
		target target
		inputs []string
	}{
		{targetOf[Ten](), append(append(tenKeys, windows...), nulls...)},
		{targetOf[Eleven](), append(append(elevenKeys, windows...), nulls...)},
		{targetOf[Ints](), ints},
		{targetOf[int8](), scalars}, {targetOf[uint8](), scalars}, {targetOf[int16](), scalars},
		{targetOf[uint16](), scalars}, {targetOf[int32](), scalars}, {targetOf[uint32](), scalars},
		{targetOf[int64](), scalars}, {targetOf[uint64](), scalars}, {targetOf[int](), scalars},
		{targetOf[uint](), scalars}, {targetOf[float32](), scalars}, {targetOf[float64](), scalars},
		{targetOf[[]int8](), padded}, {targetOf[[]uint64](), padded}, {targetOf[[]int64](), padded},
		{targetOf[[]float32](), append(padded, floats...)},
		{targetOf[[]float64](), floats},
		{targetOf[Small](), prices},
		{targetOf[map[string]int](), []string{`{null:1}`, `{null:1,"a":2}`, `{"a":1,null:2}`, `{nul:1}`, `{"a":1,}`}},
		{targetOf[Small](), []string{`{null:1}`, `{null:"x","id":"a"}`, `{"id":"a",null:1}`, "{\"i\x01d\":\"a\"}", "{\"\\u0069\x01\":\"a\"}"}},
		{targetOf[Wide](), []string{`{null:1}`, `{null:"x","a":"a"}`, `{"a":"x",null:1}`, "{\"a\x01\":\"a\"}", `{"a":"x",nul:1}`, `{"x":1e400}`, `{"x":-1e400,"a":"y"}`, `{"x":1e-400}`, `{"x":123456789012345678901234567890}`, `{"x":1.5e3x}`, `{"x":01}`, `{"x":-}`, `{"x":1.}`, `{"x":1.e5}`, `{"x":1..5}`, `{"x":"\ud800\u0041"}`, `{"x":"a\\` + "\x01" + `"}`, "{\"x\":\"\x01\\n\"}", "{\"a\":\"\\n\x01\"}", "{\"a\":\"\x01\\n\"}", `{"l":{"id":"a"},"l":{"w":1}}`, `{"l":{"id":"a"},"l":null,"l":{"w":1}}`}},
		{targetOf[Embeds](), []string{`{"level":1,"note":"x","level":2}`, `{"note":"x","note":null}`, `{"level":1,"level":"x"}`, `{"Level":1,"LEVEL":2}`, `{"dup":1,"DUP":2}`}},
		{targetOf[interface{}](), []string{"\"\\n\x01\"", "\"\x01\\n\"", `"\ud800\ud800"`, `"\ud83d\ude00\ud800"`, `"\ud800\n"`, `"\ud800\\"`, `"\ud800\u12"`, `"\ud800\uzzzz"`, `[1e400]`, `{"a":1e400}`, `[01]`, `[-]`, `[1.]`, `[.5]`, `[1e]`, `{"a" :1, "b": [ 1 , 2 ] }`}},
	}
}

func writeReviewDecode(dir string) {
	g := create(dir, "review_decode.txt", "Adversarial review cases, columns as in decode.txt:\n"+
		"type\tprefill\tinput\tjsoniter error\tjsoniter value (json.Marshal)\t"+
		"jsonutil.Unmarshal error\tencoding/json error\tencoding/json value (json.Marshal)")
	defer g.close()
	rng := rand.New(rand.NewSource(20261118))
	skipped := 0
	emit := func(t target, input string) {
		row, ok := decodeRow(t, nil, []byte(input))
		if !ok {
			skipped++
			return
		}
		g.row(append([][]byte{[]byte(t.name), nil, []byte(input)}, row...)...)
	}
	for _, ri := range reviewInputs(rng) {
		for _, input := range ri.inputs {
			emit(ri.target, input)
		}
	}
	// Nesting at and past both libraries' limit of 10000.
	for _, d := range []struct {
		target            target
		prefix, open, end string
		close, suffix     string
	}{
		{targetOf[interface{}](), "", "[", "", "]", ""},
		{targetOf[interface{}](), "", `{"a":`, "1", "}", ""},
		{targetOf[Small](), `{"x":`, "[", "", "]", "}"},
		{targetOf[Ten](), `{"x":`, `{"a":`, "1", "}", "}"},
		{targetOf[Eleven](), `{"x":`, "[", "", "]", "}"},
		{targetOf[json.RawMessage](), "", "[", "", "]", ""},
		{targetOf[[]interface{}](), "", "[", "", "]", ""},
	} {
		for _, depth := range []int{9999, 10000, 10001} {
			input := d.prefix + strings.Repeat(d.open, depth) + d.end + strings.Repeat(d.close, depth) + d.suffix
			emit(d.target, input)
		}
	}
	// Names whose validity, case mapping or folding depends on Go's Unicode tables.
	for _, input := range []string{
		"{\"x²\":1,\"नाम\":2,\"Ⅳ\":3,\"Ƛ\":4,\"σ\":5,\"x٣\":6}",
		`{"Sup":1,"Mark":2,"Roman":3,"New":4,"Greek":5,"Digit":6}`,
		`{"X²":1,"ⅳ":3,"ƛ":4,"Σ":5,"X٣":6}`, `{"sup":1,"roman":3,"new":4,"ς":5}`,
	} {
		emit(targetOf[Tags](), input)
	}
	for _, input := range []string{
		"{\"Ƛ\":1,\"σ\":2}", `{"ƛ":1}`, `{"Σ":2}`, `{"ς":2}`, "{\"ƛ\":1,\"Σ\":2,\"Ƛ\":3}",
	} {
		emit(targetOf[Lambda](), input)
	}
	for _, ft := range fuzzTargets {
		for n := 0; n < 140; {
			input := mutate(rng, ft.seeds[rng.Intn(len(ft.seeds))])
			if lossyTarget(ft.target.name) && !utf8.ValidString(input) {
				continue
			}
			emit(ft.target, input)
			n++
		}
	}
	fmt.Printf("review decode: %d panicking cases skipped\n", skipped)
}

// jsonString is a random JSON string literal mixing what the three encoders escape
// differently: HTML characters, U+2028/U+2029, control characters, invalid UTF-8 and lone
// surrogates (jsoniter keeps the decoded bytes as they are).
func jsonString(rng *rand.Rand) string {
	pieces := []string{
		"a", "Z", "<", ">", "&", `\u003c`, `\u2028`, "\u2028", "\u2029", `\u0000`, `\u001f`,
		`\u007f`, "\x7f", `\b`, `\f`, `\n`, `\r`, `\t`, `\"`, `\\`, "/", `\/`, "é", "😀", "\xff",
		"\xc3", "\xed\xa0\x80", `\ud800`, `\ud83d\ude00`, `\udc00`, " ", "\xef\xbf\xbd", "\xef\xbf\xbe",
	}
	var b strings.Builder
	b.WriteByte('"')
	for i := rng.Intn(6); i > 0; i-- {
		b.WriteString(pieces[rng.Intn(len(pieces))])
	}
	b.WriteByte('"')
	return b.String()
}

var floatTexts = []string{
	"0", "-0", "1e-7", "1e-6", "0.000001", "9.999999999999999e-7", "0.0000009999999", "1e21",
	"1e20", "9.999999999999999e20", "999999999999999900000", "1000000000000000000000",
	"5e-324", "-5e-324", "1.7976931348623157e308", "-1.7976931348623157e308",
	"2.2250738585072014e-308", "123456789", "0.1", "100", "1e-5", "3.4028234663852886e38",
	"1.401298464324817e-45", "16777217", "0.30000000000000004", "1e-320", "123456789012345680000",
}

func floatText(rng *rand.Rand) string {
	if rng.Intn(2) == 0 {
		return floatTexts[rng.Intn(len(floatTexts))]
	}
	return decimal(rng)
}

// rawJSON is a random valid JSON value with random spacing.
func rawJSON(rng *rand.Rand, depth int) string {
	sp := func() string { return []string{"", " ", "\n ", "\t"}[rng.Intn(4)] }
	switch k := rng.Intn(7); {
	case k == 0 && depth < 3:
		var parts []string
		for i := rng.Intn(3); i > 0; i-- {
			parts = append(parts, sp()+rawJSON(rng, depth+1)+sp())
		}
		return "[" + strings.Join(parts, ",") + "]"
	case k == 1 && depth < 3:
		var parts []string
		for i := rng.Intn(3); i > 0; i-- {
			parts = append(parts, sp()+jsonString(rng)+sp()+":"+sp()+rawJSON(rng, depth+1))
		}
		return "{" + strings.Join(parts, ",") + sp() + "}"
	case k == 2:
		return jsonString(rng)
	case k == 3:
		return floatText(rng)
	case k == 4:
		return []string{"true", "false", "null"}[rng.Intn(3)]
	default:
		return strconv.Itoa(rng.Intn(2000) - 1000)
	}
}

// quoteRaw wraps s in a JSON string, escaping only quotes and backslashes.
func quoteRaw(s string) string {
	return `"` + strings.NewReplacer(`\`, `\\`, `"`, `\"`).Replace(s) + `"`
}

type encodeTarget struct {
	target target
	doc    func(rng *rand.Rand) string
}

func smallDoc(rng *rand.Rand) string {
	return `{"id":` + jsonString(rng) + `,"w":` + []string{"null", "0", "300"}[rng.Intn(3)] +
		`,"h":` + strconv.Itoa(rng.Intn(3)-1) + `,"price":` + floatText(rng) + `,"tags":` +
		[]string{"null", "[]", "[" + jsonString(rng) + "," + jsonString(rng) + "]"}[rng.Intn(3)] +
		`,"ext":` + []string{"null", rawJSON(rng, 0), rawJSON(rng, 0)}[rng.Intn(3)] + `}`
}

func tenDoc(rng *rand.Rand) string {
	return `{"a":` + strconv.Itoa(rng.Intn(5)-2) + `,"b":` + jsonString(rng) + `,"c":` +
		[]string{"null", "0", "7"}[rng.Intn(3)] + `,"d":` + []string{"null", "[]", "[1,-2]"}[rng.Intn(3)] +
		`,"s":` + []string{"true", "false"}[rng.Intn(2)] + `,"f":` + floatText(rng) + `,"k":` +
		jsonString(rng) + `,"p":` + []string{"null", smallDoc(rng)}[rng.Intn(2)] + `,"m":{` +
		jsonString(rng) + `:1,` + jsonString(rng) + `:2},"σ":` + jsonString(rng) + `}`
}

var encodeTargets = []encodeTarget{
	{targetOf[Small](), smallDoc},
	{targetOf[Ten](), tenDoc},
	{targetOf[Eleven](), func(rng *rand.Rand) string {
		return strings.TrimSuffix(tenDoc(rng), "}") + `,"x":` + strconv.Itoa(rng.Intn(256)-128) + `}`
	}},
	{targetOf[map[string]int](), func(rng *rand.Rand) string {
		var parts []string
		for i := 1 + rng.Intn(4); i > 0; i-- {
			parts = append(parts, jsonString(rng)+":"+strconv.Itoa(i))
		}
		return "{" + strings.Join(parts, ",") + "}"
	}},
	{targetOf[map[string]string](), func(rng *rand.Rand) string {
		return "{" + jsonString(rng) + ":" + jsonString(rng) + "," + jsonString(rng) + ":" + jsonString(rng) + "}"
	}},
	{targetOf[[]string](), func(rng *rand.Rand) string {
		return "[" + jsonString(rng) + "," + jsonString(rng) + "]"
	}},
	{targetOf[[]float64](), func(rng *rand.Rand) string {
		return "[" + floatText(rng) + "," + floatText(rng) + "," + floatText(rng) + "]"
	}},
	{targetOf[[]float32](), func(rng *rand.Rand) string {
		return "[" + floatText(rng) + "," + floatText(rng) + "," + floatText(rng) + "]"
	}},
	{targetOf[[]json.RawMessage](), func(rng *rand.Rand) string {
		return "[" + rawJSON(rng, 0) + " , " + rawJSON(rng, 0) + "]"
	}},
	{targetOf[map[string]json.RawMessage](), func(rng *rand.Rand) string {
		return "{" + jsonString(rng) + ":" + rawJSON(rng, 0) + "," + jsonString(rng) + ":" + rawJSON(rng, 0) + "}"
	}},
	{targetOf[Quoted](), func(rng *rand.Rand) string {
		return `{"N":"` + strconv.Itoa(rng.Intn(9)) + `","s":` + quoteRaw(jsonString(rng)) +
			`,"b":"true","f":"` + floatText(rng) + `"}`
	}},
	{targetOf[openrtb2.BidResponse](), func(rng *rand.Rand) string {
		return `{"id":` + jsonString(rng) + `,"seatbid":[{"bid":[{"id":` + jsonString(rng) +
			`,"impid":"i","price":` + floatText(rng) + `,"adm":` + jsonString(rng) + `,"ext":` +
			rawJSON(rng, 0) + `}],"seat":` + jsonString(rng) + `}],"cur":"USD"}`
	}},
	{targetOf[Wide](), template(`{"a":$S,"b":$I,"c":200,"d":$I,"e":$F,"f":$B,"g":$S,"h":[$I,$I],` +
		`"i":{$S:$I},"j":$R,"k":[{"id":$S,"price":$F}],"l":{"id":$S,"ext":$R},"m":$R,"n":"AQID",` +
		`"o":{"$I":$S},"p":$R}`)},
	{targetOf[interface{}](), template(`$R`)},
	{targetOf[map[string]interface{}](), template(`{$S:$R,$S:$R,$S:$F}`)},
	{targetOf[Numbers](), template(`{"n":$F,"o":$F,"s":"$F","p":$F,"l":[$F,"$F",null]}`)},
	{targetOf[Anon](), template(`{"prebid":{"type":$S,"meta":{"count":$I,"price":$F}},` +
		`"list":[{"a":$I,"b":[$S]}]}`)},
	{targetOf[Embeds](), template(`{"id":$S,"name":$S,"dup":$I,"level":$I,"note":$S,"own":$I}`)},
	{targetOf[Maps](), template(`{"smalls":{$S:{"id":$S,"price":$F}},"ptrs":{$S:$I,$S:null},` +
		`"bools":{"1":$B,"200":$B},"raw":{$S:$R}}`)},
	{targetOf[Custom](), template(`{"si":$I,"is":$S,"v":5,"pv":7}`)},
	{targetOf[Upper](), template(`{"ID":$S,"Name":$S,"count":$I,"-":$S}`)},
	{targetOf[Tags](), template("{\"x²\":$I,\"नाम\":$I,\"Ⅳ\":$I,\"Ƛ\":$I,\"σ\":$I,\"x٣\":$I}")},
}

// template returns a document generator that replaces each $S (string), $F (float), $R (raw
// JSON value), $I (small integer) and $B (boolean) of tmpl.
func template(tmpl string) func(rng *rand.Rand) string {
	return func(rng *rand.Rand) string {
		var b strings.Builder
		for i := 0; i < len(tmpl); i++ {
			if tmpl[i] != '$' || i+1 == len(tmpl) {
				b.WriteByte(tmpl[i])
				continue
			}
			i++
			switch tmpl[i] {
			case 'S':
				b.WriteString(jsonString(rng))
			case 'F':
				b.WriteString(floatText(rng))
			case 'R':
				b.WriteString(rawJSON(rng, 0))
			case 'I':
				b.WriteString(strconv.Itoa(rng.Intn(200) - 100))
			case 'B':
				b.WriteString([]string{"true", "false"}[rng.Intn(2)])
			default:
				b.WriteByte('$')
				b.WriteByte(tmpl[i])
			}
		}
		return b.String()
	}
}

func writeReviewEncode(dir string) {
	g := create(dir, "review_encode.txt", "type\tinput (decoded with jsonutil.Unmarshal; a failed decode keeps "+
		"what it decoded)\tjsonutil.Marshal\tjson.Marshal\tjson.Encoder with SetEscapeHTML(false)\n"+
		"An output starting with ! is the error text.")
	defer g.close()
	rng := rand.New(rand.NewSource(20261119))
	for _, et := range encodeTargets {
		for i := 0; i < 60; i++ {
			doc := et.doc(rng)
			for lossyTarget(et.target.name) && !utf8.ValidString(doc) {
				doc = et.doc(rng)
			}
			v := et.target.newValue()
			_ = jsonutil.Unmarshal([]byte(doc), v)
			elem := reflect.ValueOf(v).Elem().Interface()
			g.row(append([][]byte{[]byte(et.target.name), []byte(doc)}, encodeRow(elem)...)...)
		}
	}
	keys := [][]string{
		{`"\u00ff"`, "\"\xff\"", "\"\xef\xbf\xbe\"", `"a"`},
		{"\"\xc3\"", `"\u00c3"`, "\"\xc3\xbf\""},
		{"\"\xed\xa0\x80\"", "\"\xef\xbf\xbe\"", "\"\xee\x80\x80\""},
		{`"<"`, `"&"`, `"\u2028"`, "\"\x7f\"", `"\u0001"`},
	}
	for _, ks := range keys {
		var parts []string
		for i, k := range ks {
			parts = append(parts, k+":"+strconv.Itoa(i))
		}
		doc := "{" + strings.Join(parts, ",") + "}"
		v := targetOf[map[string]int]().newValue()
		if err := jsonutil.Unmarshal([]byte(doc), v); err != nil {
			panic(err)
		}
		elem := reflect.ValueOf(v).Elem().Interface()
		g.row(append([][]byte{[]byte("map[string]int"), []byte(doc)}, encodeRow(elem)...)...)
	}
}
