package main

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// The seeded corpus of FindElement and DropElement: documents of objects, arrays, strings equal
// to the element names, numbers and literals, with whitespace around the tokens, and paths of one
// to three names. Valid JSON throughout, so every outcome (the span, the bytes left, the token
// errors Go's state machine gives and the index panics of FindElement's scans) must be exact.
const (
	elementSeed  = 0x4a53_4f4e_454c_4d31 // "JSONELM1"
	elementCount = 50000
	elementBatch = 1000
)

// elementRawKeys are the member names and string values as the documents spell them: "x:y" holds
// the colon FindElement's scan for the start of a nested element stops at, and the escaped
// spelling of "a" decodes to a name the paths use.
var elementRawKeys = []string{"a", "b", "ext", "test", "x:y", "\\u0061"}

// elementNames are the names the paths are made of.
var elementNames = []string{"a", "b", "ext", "test", "x:y"}

func (s *splitmix64) space() string {
	switch s.below(6) {
	case 0:
		return " "
	case 1:
		return "\n\t"
	default:
		return ""
	}
}

func (s *splitmix64) elementValue(depth int) string {
	n := s.below(10)
	if depth >= 3 && n >= 6 {
		n = s.below(6)
	}
	switch n {
	case 0:
		return "null"
	case 1:
		return "true"
	case 2:
		return strconv.Itoa(s.below(1000))
	case 3:
		return "-1.5e3"
	case 4, 5:
		return `"` + s.pick(elementRawKeys) + `"`
	case 6, 7:
		return s.elementObject(depth + 1)
	default:
		return s.elementArray(depth + 1)
	}
}

func (s *splitmix64) elementObject(depth int) string {
	var b strings.Builder
	b.WriteString("{")
	n := s.below(4)
	for i := 0; i < n; i++ {
		if i > 0 {
			b.WriteString(s.space())
			b.WriteString(",")
		}
		b.WriteString(s.space())
		b.WriteString(`"` + s.pick(elementRawKeys) + `"`)
		b.WriteString(s.space())
		b.WriteString(":")
		b.WriteString(s.space())
		b.WriteString(s.elementValue(depth))
	}
	b.WriteString(s.space())
	b.WriteString("}")
	return b.String()
}

func (s *splitmix64) elementArray(depth int) string {
	var b strings.Builder
	b.WriteString("[")
	n := s.below(4)
	for i := 0; i < n; i++ {
		if i > 0 {
			b.WriteString(s.space())
			b.WriteString(",")
		}
		b.WriteString(s.space())
		b.WriteString(s.elementValue(depth))
	}
	b.WriteString(s.space())
	b.WriteString("]")
	return b.String()
}

// elementCase draws one document and one path.
func (s *splitmix64) elementCase() (string, []string) {
	var doc string
	switch s.below(20) {
	case 0:
		doc = s.elementArray(1)
	case 1:
		doc = s.elementValue(1)
	default:
		doc = s.elementObject(1)
	}
	k := 1 + s.below(3)
	names := make([]string, 0, k)
	for i := 0; i < k; i++ {
		names = append(names, s.pick(elementNames))
	}
	return doc, names
}

// findOutcome is FindElement's result as one line: the span, the error or the panic.
func findOutcome(doc []byte, names []string) (out string) {
	defer func() {
		if r := recover(); r != nil {
			out = "panic: " + fmt.Sprint(r)
		}
	}()
	found, start, end, err := jsonutil.FindElement(doc, names...)
	if err != nil {
		return "error: " + err.Error()
	}
	return fmt.Sprintf("%t %d %d", found, start, end)
}

// dropOutcome is DropElement's result: the bytes left, the error or the panic. DropElement
// writes into its argument, so it gets a copy.
func dropOutcome(doc []byte, names []string) (out string) {
	defer func() {
		if r := recover(); r != nil {
			out = "panic: " + fmt.Sprint(r)
		}
	}()
	res, err := jsonutil.DropElement(append([]byte(nil), doc...), names...)
	if err != nil {
		return "error: " + err.Error()
	}
	return "ok " + string(res)
}

func elementCorpus() seededCorpus {
	return seededCorpus{
		seed:  elementSeed,
		count: elementCount,
		batch: elementBatch,
		next: func(s *splitmix64) (string, string) {
			doc, names := s.elementCase()
			input := doc + "\t" + strings.Join(names, ",")
			outcome := findOutcome([]byte(doc), names) + "\t" + dropOutcome([]byte(doc), names)
			return input, outcome
		},
	}
}

// elementCurated is one recorded case. Texts is "go" when every outcome text is the token state
// machine's or a panic's (the Rust port gives the same words), and "library" when an error text
// is encoding/json's reading a value (ledgered: the Rust side has serde_json's words, and only
// the verdict is compared).
type elementCurated struct {
	Description string   `json:"description"`
	Input       string   `json:"input"`
	Names       []string `json:"names"`
	Find        string   `json:"find"`
	Drop        string   `json:"drop"`
	Texts       string   `json:"texts"`
}

type elementDoc struct {
	Header  header           `json:"header"`
	Curated []elementCurated `json:"curated"`
	Seeded  seededDigests    `json:"seeded"`
}

// elementCases are TestDropElement's cases (util/jsonutil/jsonutil_test.go:16-178) followed by
// the edges of FindElement's scans.
var elementCases = []struct {
	description string
	input       string
	names       []string
	texts       string
}{
	{"Drop Single Element After Another Element", `{"consent": "TESTCONSENT","consented_providers_settings": {"test": 1,"consented_providers": [1608,765,492]}}`, []string{"consented_providers"}, "go"},
	{"Drop Single Element Before Another Element", `{"consent": "TESTCONSENT","consented_providers_settings": {"consented_providers": [1608,765,492],"test": 1}}`, []string{"consented_providers"}, "go"},
	{"Drop Single Element", `{"consent": "TESTCONSENT","consented_providers_settings": {"consented_providers": [1545,2563,1411]}}`, []string{"consented_providers"}, "go"},
	{"Drop Single Element string", `{"consent": "TESTCONSENT","consented_providers_settings": {"consented_providers": "test"}}`, []string{"consented_providers"}, "go"},
	{"Drop Parent Element Between Two Elements", `{"consent": "TESTCONSENT","consented_providers_settings": {"consented_providers": [1608,765,492], "test": 1},"test": 123}`, []string{"consented_providers_settings"}, "go"},
	{"Drop Parent Element Before Element", `{"consented_providers_settings": {"consented_providers": [1608,765,492], "test": 1},"test": 123}`, []string{"consented_providers_settings"}, "go"},
	{"Drop Parent Element After Element", `{"consent": "TESTCONSENT","consented_providers_settings": {"consented_providers": [1608,765,492], "test": 1}}`, []string{"consented_providers_settings"}, "go"},
	{"Drop Parent Element Only", `{"consented_providers_settings": {"consented_providers": [1608,765,492], "test": 1}}`, []string{"consented_providers_settings"}, "go"},
	{"Drop Parent Element List", `{"consented_providers_settings":{"consented_providers":[1608,765,492],"test":1},"data": [{"test1":5},{"test2": [1,2,3]}]}`, []string{"data"}, "go"},
	{"Drop Element That Doesn't Exist", `{"consented_providers_settings": {"consented_providers": [1608,765,492], "test": 1}}`, []string{"test2"}, "go"},
	{"Drop Nested Element Single Occurrence", `{"consented_providers_settings":{"consented_providers":[1608,765,492],"test":1},"data": [{"test1":5},{"test2": [1,2,3]}]}`, []string{"consented_providers_settings", "test"}, "go"},
	{"Drop Nested Element Multiple Occurrence", `{"consented_providers_settings":{"consented_providers":[1608,765,492],"test":1},"data": [{"test":5},{"test": [1,2,3]}]}`, []string{"consented_providers_settings", "test"}, "go"},
	{"Drop Nested Element Multiple Occurrence Skip Path", `{"consented_providers_settings":{"consented_providers":[1608,765,492],"data": {"amp":1, "test": 25}}}`, []string{"consented_providers_settings", "test"}, "go"},
	{"Drop Nested Structure Single Occurrence", `{"consented_providers":{"providers":[1608,765,492],"test":{"nested":true}},"data": [{"test":5},{"test": [1,2,3]}]}`, []string{"consented_providers", "test"}, "go"},
	{"Drop Nested Structure Single Occurrence Deep Nested", `{"consented_providers":{"providers":[1608,765,492],"test":{"nested":true, "nested2": {"test6": 123}}},"data": [{"test":5},{"test": [1,2,3]}]}`, []string{"consented_providers", "test6"}, "go"},
	{"Drop Nested Structure Single Occurrence Deep Nested Full Path", `{"consented_providers":{"providers":[1608,765,492],"test":{"nested":true,"nested2": {"test6": 123}}},"data": [{"test":5},{"test": [1,2,3]}]}`, []string{"consented_providers", "test", "nested"}, "go"},
	{"Drop Nested Structure Doesn't Exist", `{"consented_providers":{"providers":[1608,765,492]},"test":{"nested":true}},"data": [{"test":5},{"test": [1,2,3]}]}`, []string{"consented_providers", "test2"}, "go"},
	{"Error Decode", `{"consented_providers_settings": {"consented_providers": ["123",1,,1365,5678,1545,2563,1411], "test": 1}}`, []string{"consented_providers"}, "library"},
	{"Error Malformed", `{consented_providers_settings: {"consented_providers": [1365,5678,1545,2563,1411], "test": 1}}`, []string{"consented_providers"}, "go"},
	{"string value matched in an array consumes the next element", `["x",1]`, []string{"x"}, "go"},
	{"string value matched in an array, nested path: the colon scan runs off the end", `["x",1]`, []string{"x", "y"}, "go"},
	{"string value matched as the last array element", `["a","x"]`, []string{"x"}, "go"},
	{"string value matched in object value position", `{"a":"x","x":1}`, []string{"x"}, "go"},
	{"top-level string matched at the end", `"x"`, []string{"x"}, "go"},
	{"top-level string matched before another top-level value", `"x" 1`, []string{"x"}, "go"},
	{"top-level string matched before two more values: the comma scan runs off the end", `"x" 1 2`, []string{"x"}, "go"},
	{"whitespace before the comma is kept and the member after it loses its comma", `{"a":1 ,"b":2,"c":3}`, []string{"b"}, "go"},
	{"nested element in an object that has more members after it", `{"a":{"b":1,"c":2},"d":3}`, []string{"a", "b"}, "go"},
	{"nested element whose parent is the last member", `{"a":{"b":1}}`, []string{"a", "b"}, "go"},
	{"nested path through a scalar", `{"a":5,"b":{"a":1}}`, []string{"a", "b"}, "go"},
	{"nested path through an array", `{"a":["b",{"b":1}],"c":1}`, []string{"a", "b"}, "go"},
	{"a key holding a colon moves the nested start into the key", `{"k:ey":{"x":1},"y":2}`, []string{"k:ey", "x"}, "go"},
	{"an escaped key matches its decoded name", "{\"\\u0061\":1,\"b\":2}", []string{"a"}, "go"},
	{"a key with a lone surrogate decodes to U+FFFD and does not match", "{\"\\ud800\":1,\"a\":2}", []string{"a"}, "go"},
	{"a key that is not UTF-8", "{\"\xff\":1,\"a\":2}", []string{"a"}, "library"},
	{"a number out of float64's range before the element", `{"a":1e400,"b":1}`, []string{"b"}, "library"},
	{"a truncated document ends the scan without an error", `{"a":{"b":`, []string{"c"}, "go"},
	{"a truncated value of the element", `{"a":[1,2`, []string{"a"}, "go"},
	{"a matched string value with nothing after it in a truncated array", `["x"`, []string{"x"}, "go"},
	{"an empty document", ``, []string{"a"}, "go"},
	{"an empty path", `{"a":1}`, []string{}, "go"},
	{"a stray closing bracket", `{"a":1}}`, []string{"b"}, "go"},
	{"a missing colon", `{"a" 1}`, []string{"a"}, "go"},
	{"a missing comma between members", `{"a":1 "b":2}`, []string{"b"}, "go"},
	{"a number followed by garbage", `{"a":1x,"b":2}`, []string{"b"}, "library"},
}

func elementGolden() elementDoc {
	doc := elementDoc{Header: newHeader(), Seeded: elementCorpus().digests()}
	for _, c := range elementCases {
		doc.Curated = append(doc.Curated, elementCurated{
			Description: c.description,
			Input:       c.input,
			Names:       append([]string{}, c.names...),
			Find:        findOutcome([]byte(c.input), c.names),
			Drop:        dropOutcome([]byte(c.input), c.names),
			Texts:       c.texts,
		})
	}
	return doc
}
