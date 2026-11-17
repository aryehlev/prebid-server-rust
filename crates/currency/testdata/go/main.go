// Command currencygolden writes the golden files that the pbs-currency Rust tests replay.
//
// It drives the real Go code: golang.org/x/text/currency.ParseISO (at the version pinned in
// the repository go.mod), the prebid-server currency package, and jsonutil.UnmarshalValid /
// jsoniter for decoding the currency file. Run it from anywhere inside the repository:
//
//	go run ./rust/crates/currency/testdata/go
//
// The output files are written next to this directory, under ../golden. random.golden holds
// digests of random corpora that the Rust tests regenerate; see random.go.
package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"hash/fnv"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"golang.org/x/text/currency"

	pbscurrency "github.com/prebid/prebid-server/v4/currency"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// golden writes lines to one golden file.
type golden struct {
	f *os.File
	w *bufio.Writer
}

func newGolden(dir, name string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		panic(err)
	}
	return &golden{f: f, w: bufio.NewWriter(f)}
}

func (g *golden) line(fields ...string) {
	if _, err := g.w.WriteString(strings.Join(fields, "\t") + "\n"); err != nil {
		panic(err)
	}
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		panic(err)
	}
	if err := g.f.Close(); err != nil {
		panic(err)
	}
}

func hx(s string) string {
	return hex.EncodeToString([]byte(s))
}

// parseISOResult renders the outcome of currency.ParseISO.
func parseISOResult(s string) string {
	u, err := currency.ParseISO(s)
	if err != nil {
		return "err:" + err.Error()
	}
	return "ok:" + u.String()
}

func writeISO(dir string) {
	g := newGolden(dir, "iso4217.golden")
	defer g.close()

	upper := "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
	for _, a := range upper {
		for _, b := range upper {
			for _, c := range upper {
				code := string([]rune{a, b, c})
				if u, err := currency.ParseISO(code); err == nil {
					g.line("code", code, u.String())
				}
			}
		}
	}

	explicit := []string{
		"", "U", "US", "USD", "USDX", "USDXY", "usd", "Usd", "uSd", "usD", "UsD",
		"XXX", "xxx", "XxX", "XTS", "xts", "VES", "ves", "MRU", "FOO", "foo", "123",
		"US1", "U D", "US\x00", "\x00US", "ÉUR", "€", "ŒU", " USD", "USD ",
		"\tUSD", "U-D", "usd\n", "@AB", "[US", "`ab", "{ab", "ZZZ", "zzz", "AAA", "ZWR",
		"ADP", "zwr", "\U0001f4b5", "ÿÿ", "éu",
	}
	for _, s := range explicit {
		g.line("case", hx(s), parseISOResult(s))
	}

	ascii := fnv.New64a()
	count := 0
	for a := 0; a < 128; a++ {
		for b := 0; b < 128; b++ {
			for c := 0; c < 128; c++ {
				s := string([]byte{byte(a), byte(b), byte(c)})
				if _, err := ascii.Write([]byte(s + "\t" + parseISOResult(s) + "\n")); err != nil {
					panic(err)
				}
				count++
			}
		}
	}
	g.line("digest", "ascii3", fmt.Sprint(count), fmt.Sprintf("%016x", ascii.Sum64()))

	letters := upper + strings.ToLower(upper)
	lh := fnv.New64a()
	count = 0
	for i := 0; i < len(letters); i++ {
		for j := 0; j < len(letters); j++ {
			for k := 0; k < len(letters); k++ {
				s := string([]byte{letters[i], letters[j], letters[k]})
				if _, err := lh.Write([]byte(s + "\t" + parseISOResult(s) + "\n")); err != nil {
					panic(err)
				}
				count++
			}
		}
	}
	g.line("digest", "letters3", fmt.Sprint(count), fmt.Sprintf("%016x", lh.Sum64()))
}

// validKey mirrors what the Rust port stores for a decoded key: every byte that is not part
// of a valid UTF-8 sequence becomes U+FFFD.
func validKey(k string) string {
	return string([]rune(k))
}

func sortedKeys[V any](m map[string]V) ([]string, map[string]V) {
	conv := make(map[string]V, len(m))
	keys := make([]string, 0, len(m))
	for k, v := range m {
		vk := validKey(k)
		if _, dup := conv[vk]; dup {
			panic(fmt.Sprintf("corpus produced two keys that collapse to %q", vk))
		}
		conv[vk] = v
		keys = append(keys, vk)
	}
	sort.Strings(keys)
	return keys, conv
}

// dumpConversions renders a decoded conversions map, distinguishing nil and empty maps.
func dumpConversions(m map[string]map[string]float64) string {
	if m == nil {
		return "nil"
	}
	var sb strings.Builder
	sb.WriteString("{")
	keys, conv := sortedKeys(m)
	for _, k := range keys {
		inner := conv[k]
		sb.WriteString(hx(k) + ":")
		if inner == nil {
			sb.WriteString("nil;")
			continue
		}
		sb.WriteString("{")
		ikeys, iconv := sortedKeys(inner)
		for _, ik := range ikeys {
			sb.WriteString(fmt.Sprintf("%s=%016x,", hx(ik), math.Float64bits(iconv[ik])))
		}
		sb.WriteString("};")
	}
	sb.WriteString("}")
	return sb.String()
}

// decodeResult decodes data exactly as RateConverter.fetch does and renders the outcome,
// including the raw jsoniter error text so the Rust port's iterator positions are checked too.
func decodeResult(data []byte) string {
	rates := &pbscurrency.Rates{}
	err := jsonutil.UnmarshalValid(data, rates)
	raw := jsoniter.ConfigCompatibleWithStandardLibrary.Unmarshal(data, &pbscurrency.Rates{})
	if (err == nil) != (raw == nil) {
		panic("jsonutil and jsoniter disagree")
	}
	if err != nil {
		return "err:" + raw.Error() + "\x1f" + err.Error()
	}
	return "ok:" + dumpConversions(rates.Conversions)
}

var jsonCases = []string{
	``,
	` `,
	`null`,
	` null `,
	`nul`,
	`nulx`,
	`malformed`,
	`{}`,
	`{ }`,
	`{}x`,
	"{}\x00garbage",
	"{}\x00",
	`[]`,
	`"str"`,
	`5`,
	`{"conversions":null}`,
	`{"conversions":{}}`,
	`{"conversions":{"USD":null}}`,
	`{"conversions":{"USD":{}}}`,
	`{"dataAsOf":"2018-09-12","conversions":{"USD":{"GBP":0.77208},"GBP":{"USD":1.2952}}}`,
	// Bodies from currency/rate_converter_test.go.
	`{"message": "Not Found"}`,
	`{"message": Invalid-JSON-No-Surrounding-Quotes}`,
	`{"Conversions":{"USD":{"GBP":1}}}`,
	`{"CONVERSIONS":{"USD":{"GBP":1}}}`,
	`{"conversionS":{"USD":{"GBP":1}}}`,
	`{"conversions":{"USD":{"GBP":1}}}`,
	`{"conversionS":{"USD":{"GBP":1}}}`,
	`{"conversions ":{"USD":{"GBP":1}}}`,
	`{"conversions":{"USD":{"GBP":0.7662523901,"MXN":20.00},"USD":{"GBP":0.4815162342}}}`,
	`{"conversions":{"USD":{"GBP":1}},"conversions":{"EUR":{"GBP":2}}}`,
	`{"conversions":{"USD":{"GBP":1}},"conversions":null}`,
	`{"conversions":{"USD":{"GBP":1}},"conversions":{"USD":null}}`,
	`{"conversions":{null:{"GBP":null}}}`,
	`{"conversions":{"USD":{null:1}}}`,
	`{"conversions":{"USD":{"GBP":null}}}`,
	`{"conversions":{"USD":{"GBP":+5}}}`,
	`{"conversions":{"USD":{"GBP":-+5}}}`,
	`{"conversions":{"USD":{"GBP":+.5}}}`,
	`{"conversions":{"USD":{"GBP":-0}}}`,
	`{"conversions":{"USD":{"GBP":-0.0}}}`,
	`{"conversions":{"USD":{"GBP":1e5}}}`,
	`{"conversions":{"USD":{"GBP":1E+5}}}`,
	`{"conversions":{"USD":{"GBP":1e-5}}}`,
	`{"conversions":{"USD":{"GBP":1e400}}}`,
	`{"conversions":{"USD":{"GBP":-1e400}}}`,
	`{"conversions":{"USD":{"GBP":1e-400}}}`,
	`{"conversions":{"USD":{"GBP":1e}}}`,
	`{"conversions":{"USD":{"GBP":01}}}`,
	`{"conversions":{"USD":{"GBP":00.5}}}`,
	`{"conversions":{"USD":{"GBP":0.}}}`,
	`{"conversions":{"USD":{"GBP":5.}}}`,
	`{"conversions":{"USD":{"GBP":.5}}}`,
	`{"conversions":{"USD":{"GBP":-.5}}}`,
	`{"conversions":{"USD":{"GBP":5.e3}}}`,
	`{"conversions":{"USD":{"GBP":1.2.3}}}`,
	`{"conversions":{"USD":{"GBP":--5}}}`,
	`{"conversions":{"USD":{"GBP":-}}}`,
	`{"conversions":{"USD":{"GBP":}}}`,
	`{"conversions":{"USD":{"GBP":0x10}}}`,
	`{"conversions":{"USD":{"GBP":Infinity}}}`,
	`{"conversions":{"USD":{"GBP":NaN}}}`,
	`{"conversions":{"USD":{"GBP":"1.5"}}}`,
	`{"conversions":{"USD":{"GBP":true}}}`,
	`{"conversions":{"USD":{"GBP":[1]}}}`,
	`{"conversions":{"USD":{"GBP":1.0000001}}}`,
	`{"conversions":{"USD":{"GBP":123456789012345678901234567890}}}`,
	`{"conversions":{"USD":{"GBP":9007199254740993}}}`,
	`{"conversions":{"USD":{"GBP":9007199254740993.5}}}`,
	`{"conversions":{"USD":{"GBP":1844674407370955161}}}`,
	`{"conversions":{"USD":{"GBP":0.1234567890123456789}}}`,
	`{"conversions":{"USD":{"GBP":5` + "\r" + `}}}`,
	`{"conversions":{"USD":{"GBP":1}}`,
	`{"conversions":{"USD":{"GBP":1}`,
	`{"conversions":{"USD":{"GBP":1`,
	`{"conversions":{"USD":{"GBP":`,
	`{"conversions":{"USD":{"GBP"`,
	`{"conversions":{"USD":{"GBP`,
	`{"conversions":{"USD":{`,
	`{"conversions":{"USD"`,
	`{"conversions":`,
	`{"conversions"`,
	`{"conv`,
	`{`,
	`{"conversions":{"USD":{"GBP":1}},}`,
	`{"conversions":{"USD":{"GBP":1},}}`,
	`{"conversions":{"USD":{"GBP":1,}}}`,
	`{"conversions":{"USD":{"GBP":1}} x`,
	`{"conversions":{"USD":{"GBP":1}}}}`,
	`{"conversions":{"USD" {"GBP":1}}}`,
	`{"conversions":{"USD":{"GBP" 1}}}`,
	`{"conversions" {"USD":{"GBP":1}}}`,
	`{"conversions":["USD"]}`,
	`{"conversions":"USD"}`,
	`{"conversions":{"USD":"GBP"}}`,
	`{"conversions":{"USD":[]}}`,
	`{"conversions":{USD:{"GBP":1}}}`,
	`{conversions:{"USD":{"GBP":1}}}`,
	`{"x":1,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":[1,2,{"a":[]}],"conversions":{"USD":{"GBP":1}}}`,
	`{"x":{"a":{"b":[null,true,false,"s",-1.5e3]}},"conversions":{"USD":{"GBP":1}}}`,
	`{"x":tru,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":nul,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":fals,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":falsy,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-.5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-.5e,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01e5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01+,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01e99999999999999999999,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01e2147483647,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01e2147483646,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-.1e-2147483647,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-00e5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-00e999999999999,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01ee,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01e5e5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01.5.5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-01.5e3,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-1.5e,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-1e400,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":1e400,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":-}`,
	`{"x":-`,
	`{"x":0.5e39,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":0.1e39,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":01,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":0,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":0e,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":1.5.5,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":1.,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":1x,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":"a\"b","conversions":{"USD":{"GBP":1}}}`,
	`{"x":"a\qb","conversions":{"USD":{"GBP":1}}}`,
	"{\"x\":\"a\x01b\",\"conversions\":{\"USD\":{\"GBP\":1}}}",
	"{\"x\":\"a\\nb\x01\",\"conversions\":{\"USD\":{\"GBP\":1}}}",
	`{"x":"\ud800","conversions":{"USD":{"GBP":1}}}`,
	`{"x":"unterminated`,
	`{"x":[1,2`,
	`{"x":[1 2],"conversions":{}}`,
	`{"x":{"a" 1},"conversions":{}}`,
	`{"x":{"a":1 "b":2},"conversions":{}}`,
	`{"x":{1:2},"conversions":{}}`,
	`{"x":@,"conversions":{}}`,
	`{"x":],"conversions":{}}`,
	`{"x":1,"conversions":{"USD":{"GBP":01}},"y":2}`,
	`{"x":01,"conversions":{"USD":{"GBP":1}},"conversions":{"EUR":{"GBP":2}}}`,
	`{"conversions":{"USD":{"GBP":01}},"conversions":{"EUR":{"GBP":2}}}`,
	`{"conversions":{"USD":{"GBP":1}}}`,
	`{"conversions":{"💵":{"\ud800A":1,"\ud800x":2,"\udc00\ud800":3}}}`,
	`{"conversions":{"\ud800\n":{"a\/b":1}}}`,
	`{"conversions":{"\uZZZZ":{"GBP":1}}}`,
	`{"conversions":{"\u12":{"GBP":1}}}`,
	"{\"conversions\":{\"\xff\":{\"GBP\":1}}}",
	"{\"conversions\":{\"US\xe2\x82\":{\"GBP\":1}}}",
	"{\"conversions\":{\"USD\":{\"GBP\":1}}}\n",
	"\r\n\t {\"conversions\":{\"USD\":{\"GBP\":1}}} \r\n",
	"{\"conversions\":{\"USD\":{\"GBP\":1}}}\r",
	`{"conversions":{"USD":{"GBP":1}}}null`,
	`{"x":"é","conversions":{"USD":{"GBP":1}}}`,
	`{"é":1,"conversions":{"USD":{"GBP":1}}}`,
	`{"x\"y":1,"conversions":{"USD":{"GBP":1}}}`,
	`{"x":[[[[[[[[[[]]]]]]]]]],"conversions":{"USD":{"GBP":1}}}`,
	`{"x":[[[[[[[[[[]]]]]]]]],"conversions":{"USD":{"GBP":1}}}`,
}

var mutationBases = []string{
	`{"dataAsOf":"2018-09-12","conversions":{"USD":{"GBP":0.77208,"EUR":-1.5e3},"GBP":null}}`,
	`{"x":[0.5,-01,{"y":"é\n"}],"Conversions":{"usd":{"A":1}}}`,
	`{"conversions":{"USD":{"GBP":12.5,"JPY":0e1}},"t":true,"f":false,"n":null}`,
}

const mutationAlphabet = "{}[]\",:\\ \t\nnul0123456789.eE+-xtrfa\x00\xff"

func writeMutationDigest(g *golden, base int, kind string, pos int, inputs []string) {
	h := fnv.New64a()
	for _, in := range inputs {
		if _, err := h.Write([]byte(decodeResult([]byte(in)) + "\n")); err != nil {
			panic(err)
		}
	}
	g.line("mut", fmt.Sprint(base), kind, fmt.Sprint(pos), fmt.Sprint(len(inputs)), fmt.Sprintf("%016x", h.Sum64()))
}

func writeJSON(dir string) {
	g := newGolden(dir, "rates_json.golden")
	defer g.close()

	for _, c := range jsonCases {
		g.line("json", hx(c), hx(decodeResult([]byte(c))))
	}

	// Nesting around jsoniter's depth limit of 10000, inside a skipped field.
	shapes := []struct{ name, open, close string }{{"array", "[", "]"}, {"object", `{"a":`, "}"}}
	for _, shape := range shapes {
		for _, n := range []int{9998, 9999, 10000, 200000} {
			for _, closed := range []bool{false, true} {
				doc := `{"x":` + strings.Repeat(shape.open, n)
				if closed {
					doc += "1" + strings.Repeat(shape.close, n) + "}"
				}
				g.line("deep", shape.name, fmt.Sprint(n), fmt.Sprint(closed), hx(decodeResult([]byte(doc))))
			}
		}
	}

	g.line("alphabet", hx(mutationAlphabet))
	for bi, base := range mutationBases {
		g.line("base", fmt.Sprint(bi), hx(base))
		for pos := 0; pos <= len(base); pos++ {
			writeMutationDigest(g, bi, "trunc", pos, []string{base[:pos]})
		}
		for pos := 0; pos < len(base); pos++ {
			inputs := make([]string, 0, len(mutationAlphabet))
			for i := 0; i < len(mutationAlphabet); i++ {
				inputs = append(inputs, base[:pos]+mutationAlphabet[i:i+1]+base[pos+1:])
			}
			writeMutationDigest(g, bi, "repl", pos, inputs)
		}
		for pos := 0; pos <= len(base); pos++ {
			inputs := make([]string, 0, len(mutationAlphabet))
			for i := 0; i < len(mutationAlphabet); i++ {
				inputs = append(inputs, base[:pos]+mutationAlphabet[i:i+1]+base[pos:])
			}
			writeMutationDigest(g, bi, "ins", pos, inputs)
		}
		for pos := 0; pos < len(base); pos++ {
			writeMutationDigest(g, bi, "del", pos, []string{base[:pos] + base[pos+1:]})
		}
	}
}

// rateTables are currency files (decoded through UnmarshalValid like the server does).
var rateTables = []struct {
	name string
	json string
}{
	{"simple", `{"conversions":{"USD":{"GBP":0.77208},"GBP":{"USD":1.2952}}}`},
	{"reverse", `{"conversions":{"USD":{"GBP":0.77208},"EUR":{"USD":0.88723}}}`},
	{"intermediate", `{"conversions":{"USD":{"SEK":10.23842,"NOK":10.47089},"EUR":{"THB":35.23842,"ZAR":18.47089}}}`},
	{"ambiguous", `{"conversions":{"USD":{"GBP":0.8,"SEK":10,"NOK":11,"MXN":17},"EUR":{"GBP":0.88,"SEK":11.5,"NOK":11.7,"THB":38},"JPY":{"SEK":0.07,"THB":0.24,"USD":0.0067}}}`},
	{"nilmap", `{}`},
	{"empty", `{"conversions":{}}`},
	{"nullinner", `{"conversions":{"USD":null,"EUR":{"USD":1.1,"SEK":11.2}}}`},
	{"zero", `{"conversions":{"USD":{"GBP":0,"EUR":-0},"GBP":{"EUR":-2}}}`},
	{"lowercase", `{"conversions":{"usd":{"gbp":2},"GBP":{"jpy":3,"MXN":4}}}`},
	{"xxx", `{"conversions":{"XXX":{"USD":3},"EUR":{"XXX":2,"GBP":0.9}}}`},
	{"self", `{"conversions":{"USD":{"USD":5,"GBP":2},"GBP":{"GBP":7}}}`},
}

// rateCodes are used with every table; invalidCodes only with the tables in fullCodeTables,
// because a malformed or unknown code fails before the table is consulted.
var rateCodes = []string{
	"USD", "GBP", "EUR", "MXN", "JPY", "SEK", "NOK", "THB", "ZAR", "XXX", "usd", "gBp",
}

var invalidCodes = []string{"xxx", "VES", "FOO", "", "US", "USDX", "U$D"}

var fullCodeTables = map[string]bool{"simple": true}

func codesFor(table string) []string {
	if fullCodeTables[table] {
		return append(append([]string{}, rateCodes...), invalidCodes...)
	}
	return rateCodes
}

func loadTable(js string) *pbscurrency.Rates {
	r := &pbscurrency.Rates{}
	if err := jsonutil.UnmarshalValid([]byte(js), r); err != nil {
		panic(err)
	}
	return r
}

func bits(f float64) string {
	return fmt.Sprintf("%016x", math.Float64bits(f))
}

func rateResult(rate float64, err error) string {
	if err != nil {
		return "err:" + err.Error()
	}
	return "ok:" + bits(rate)
}

// rateCandidates lists every result Rates.GetRate can return for from/to, given that the
// intermediate search walks r.Conversions in Go's randomized map order.
func rateCandidates(r *pbscurrency.Rates, from, to string) []string {
	fromUnit, err := currency.ParseISO(from)
	if err != nil {
		return []string{rateResult(0, err)}
	}
	toUnit, err := currency.ParseISO(to)
	if err != nil {
		return []string{rateResult(0, err)}
	}
	if fromUnit.String() == toUnit.String() {
		return []string{rateResult(1, nil)}
	}
	if r.Conversions == nil {
		return []string{"err:rates are nil"}
	}
	if c, ok := r.Conversions[fromUnit.String()][toUnit.String()]; ok {
		return []string{rateResult(c, nil)}
	}
	if c, ok := r.Conversions[toUnit.String()][fromUnit.String()]; ok {
		return []string{rateResult(1/c, nil)}
	}
	set := map[string]struct{}{}
	for _, conv := range r.Conversions {
		toRate, hasTo := conv[toUnit.String()]
		fromRate, hasFrom := conv[fromUnit.String()]
		if hasTo && hasFrom {
			set[rateResult(toRate/fromRate, nil)] = struct{}{}
		}
	}
	if len(set) == 0 {
		return []string{rateResult(0, pbscurrency.ConversionNotFoundError{FromCur: fromUnit.String(), ToCur: toUnit.String()})}
	}
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func renderCandidates(actual string, cands []string) string {
	found := false
	for _, c := range cands {
		if c == actual {
			found = true
		}
	}
	if !found {
		panic(fmt.Sprintf("GetRate returned %q, outside the candidates %q", actual, cands))
	}
	if len(cands) == 1 {
		return cands[0]
	}
	return "any:" + strings.Join(cands, "|")
}

func writeRates(dir string) {
	g := newGolden(dir, "get_rate.golden")
	defer g.close()

	tables := map[string]*pbscurrency.Rates{}
	for _, t := range rateTables {
		g.line("table", t.name, t.json)
		tables[t.name] = loadTable(t.json)
	}

	for _, t := range rateTables {
		r := tables[t.name]
		codes := codesFor(t.name)
		for _, from := range codes {
			for _, to := range codes {
				actual := rateResult(r.GetRate(from, to))
				g.line("rate", t.name, from, to, renderCandidates(actual, rateCandidates(r, from, to)))
			}
		}
	}

	constant := pbscurrency.NewConstantRates()
	for _, from := range codesFor("simple") {
		for _, to := range codesFor("simple") {
			g.line("const", from, to, rateResult(constant.GetRate(from, to)))
		}
	}

	aggregates := [][2]string{
		{"simple", "intermediate"},
		{"ambiguous", "reverse"},
		{"empty", "simple"},
		{"nilmap", "simple"},
		{"lowercase", "ambiguous"},
	}
	for _, pair := range aggregates {
		custom, server := tables[pair[0]], tables[pair[1]]
		agg := pbscurrency.NewAggregateConversions(custom, server)
		codes := codesFor(pair[0])
		for _, from := range codes {
			for _, to := range codes {
				actual := rateResult(agg.GetRate(from, to))
				cands := rateCandidates(custom, from, to)
				if len(cands) == 1 && strings.HasPrefix(cands[0], "err:Currency conversion rate not found") {
					cands = rateCandidates(server, from, to)
				}
				g.line("agg", pair[0], pair[1], from, to, renderCandidates(actual, cands))
			}
		}
	}
}

// validationTables are request-level custom rates (ext.prebid.currency.rates).
var validationTables = []string{
	`{"conversions":{}}`,
	`{}`,
	`{"conversions":{"USD":{"GBP":1.2,"MXN":0.05}}}`,
	`{"conversions":{"FOO":{"GBP":1.2,"MXN":0.05,"JPY":0.95}}}`,
	`{"conversions":{"USD":{"FOO":10.0,"MXN":0.05}}}`,
	`{"conversions":{"usd":{"mxn":10.0}}}`,
	`{"conversions":{"USD":{"BAR":1,"FOO":2},"EUR":{"US":1}}}`,
	`{"conversions":{"USD":null,"XXX":{"xxx":1}}}`,
	`{"conversions":{"":{"USD":1}}}`,
	`{"conversions":{"USD":{"VES":1},"AAA":{"MRU":2}}}`,
}

func validationCandidates(m map[string]map[string]float64) []string {
	set := map[string]struct{}{}
	for from, rates := range m {
		if _, err := currency.ParseISO(from); err != nil {
			set["currency code "+from+" is not recognized or malformed"] = struct{}{}
			continue
		}
		for to := range rates {
			if _, err := currency.ParseISO(to); err != nil {
				set["currency code "+to+" is not recognized or malformed"] = struct{}{}
			}
		}
	}
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, "err:"+k)
	}
	sort.Strings(out)
	if len(out) == 0 {
		return []string{"ok"}
	}
	return out
}

func writeValidation(dir string) {
	g := newGolden(dir, "validation.golden")
	defer g.close()
	for _, js := range validationTables {
		r := loadTable(js)
		err := pbscurrency.ValidateCustomRates(&openrtb_ext.ExtRequestCurrency{ConversionRates: r.Conversions})
		actual := "ok"
		if err != nil {
			actual = "err:" + err.Error()
		}
		g.line("validate", hx(js), renderCandidates(actual, validationCandidates(r.Conversions)))
	}
}

func main() {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("cannot locate generator source")
	}
	dir := filepath.Join(filepath.Dir(file), "..", "golden")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		panic(err)
	}
	writeISO(dir)
	writeJSON(dir)
	writeRates(dir)
	writeValidation(dir)
	writeRandom(dir)
}
