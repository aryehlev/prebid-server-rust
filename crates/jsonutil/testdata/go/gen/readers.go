package main

import (
	"encoding/hex"
	"errors"
	"fmt"
	"strconv"
	"strings"
	_ "unsafe" // for go:linkname

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// The seeded corpora of the readers and writers outside MergeClone, MergePatch and FindElement:
// strings with escapes and surrogates as jsonutil.Unmarshal decodes them, json.RawMessage values
// as jsonutil.Marshal compacts them, jsoniter-style messages through tryExtractErrorMessage, and
// the values StringInt, IntString and ParseIntoString read. Each corpus keeps to inputs both sides
// read alike or names the ledgered difference a case meets as its outcome ("quirk <id>", the
// entry's id without "json."), so every other outcome must be exact.
const (
	stringsSeed      = 0x4a53_4f4e_5354_5231 // "JSONSTR1"
	compactSeed      = 0x4a53_4f4e_434d_5031 // "JSONCMP1"
	extractSeed      = 0x4a53_4f4e_4558_5431 // "JSONEXT1"
	stringIntSeed    = 0x4a53_4f4e_5349_4e31 // "JSONSIN1"
	intStringSeed    = 0x4a53_4f4e_4953_5431 // "JSONIST1"
	parseIntoSeed    = 0x4a53_4f4e_5049_5331 // "JSONPIS1"
	readersCount     = 50000
	readersBatchSize = 1000
)

// tryExtractErrorMessage is jsonutil's unexported function itself (an empty linkname.s lets the
// declaration go without a body), so the corpus runs Go's code rather than a copy.
//
//go:linkname tryExtractErrorMessage github.com/prebid/prebid-server/v4/util/jsonutil.tryExtractErrorMessage
func tryExtractErrorMessage(err error) string

// backslashU starts a \u escape; the escapes are built from it and four hex digits.
const backslashU = "\\" + "u"

var (
	strPlain   = []string{"a", "b", " ", "\xc3\xa9", "\xf0\x9f\x98\x80", "\x7f", "<"}
	strEscapes = []string{"\\\\", "\\n", "\\\"", "\\/", "\\t"}
	strUnits   = []string{"0041", "00e9", "001f", "0000", "d800", "dbff", "dc00", "dfff", "d83d", "de00", "fffd", "2028", "ffff"}
	strHigh    = []string{"d800", "dbff", "d83d"}
	strLow     = []string{"dc00", "dfff", "de00"}
	strInvalid = []string{"\\x", backslashU + "12", backslashU + "zzzz", "\\"}
)

// strUnit is a \u escape of one of units, its hex digits upper-case half of the time.
func (s *splitmix64) strUnit(units []string) string {
	unit := s.pick(units)
	if s.below(2) == 1 {
		unit = strings.ToUpper(unit)
	}
	return backslashU + unit
}

// strContent is the inside of a JSON string: plain characters, escapes, \u escapes of single
// code units (lone surrogates among them) and of pairs, and now and then an escape that is not
// JSON. surrogates false keeps to code units every Go reader decodes alike (gjson drops the
// escape after a lone surrogate, json.surrogate-pairing) and leaves out the lone backslash of
// strInvalid, which would escape the backslash of a pair after it and leave its low half alone;
// invalid false puts a plain character where an escape that is not JSON would go (gjson reads a
// value out of those, json.parse-into-string-lenient).
func (s *splitmix64) strContent(surrogates, invalid bool) string {
	var b strings.Builder
	n := s.below(6)
	for i := 0; i < n; i++ {
		k := s.below(16)
		switch {
		case k <= 4:
			b.WriteString(s.pick(strPlain))
		case k <= 6:
			b.WriteString(s.pick(strEscapes))
		case k <= 11:
			if surrogates {
				b.WriteString(s.strUnit(strUnits))
			} else {
				b.WriteString(s.strUnit(strUnits[:4]))
			}
		case k <= 14:
			b.WriteString(s.strUnit(strHigh) + s.strUnit(strLow))
		case invalid && surrogates:
			b.WriteString(s.pick(strInvalid))
		case invalid:
			b.WriteString(s.pick(strInvalid[:3]))
		default:
			b.WriteString(s.pick(strPlain))
		}
	}
	return b.String()
}

func hexOf(texts ...string) string {
	parts := make([]string, len(texts))
	for i, t := range texts {
		parts[i] = hex.EncodeToString([]byte(t))
	}
	return strings.Join(parts, ",")
}

func stringsCorpus() seededCorpus {
	return seededCorpus{seed: stringsSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (string, string) {
		switch form := s.below(5); {
		case form <= 2:
			doc := `{"s":"` + s.strContent(true, true) + `"}`
			var v stringTarget
			if err := jsonutil.Unmarshal([]byte(doc), &v); err != nil {
				return doc, "error"
			}
			return doc, hexOf(v.S)
		case form == 3:
			doc := `{"` + s.strContent(true, true) + `":1}`
			var m map[string]int
			if err := jsonutil.Unmarshal([]byte(doc), &m); err != nil {
				return doc, "error"
			}
			keys := make([]string, 0, len(m))
			for k := range m {
				keys = append(keys, k)
			}
			return doc, hexOf(keys...)
		default:
			n := 1 + s.below(3)
			parts := make([]string, n)
			for i := range parts {
				parts[i] = `"` + s.strContent(true, true) + `"`
			}
			doc := "[" + strings.Join(parts, ",") + "]"
			var v []string
			if err := jsonutil.Unmarshal([]byte(doc), &v); err != nil {
				return doc, "error"
			}
			return doc, hexOf(v...)
		}
	}}
}

var (
	cmpNumbers = []string{"0", "-0", "12", "-7", "2.50", "1E+2", "1e-7", "3.0", "123456789012345678901234567890", "1.5e300"}
	cmpStrings = []string{`"a b"`, `"x  y <&>"`, `""`, `"\"q\""`, `"\\"`, "\"\xc3\xa9\"", `"` + backslashU + `00e9 \/"`, `"` + backslashU + `d83d` + backslashU + `de00"`}
	cmpKeys    = []string{`"a"`, `"b"`, `"a b"`, `""`}
)

func (s *splitmix64) cmpSpace() string {
	switch s.below(6) {
	case 3:
		return " "
	case 4:
		return "\n  "
	case 5:
		return "\t\r\n"
	default:
		return ""
	}
}

// cmpValue is a JSON value with whitespace between its tokens: numbers and strings in the
// spellings both writers keep, nested objects (repeated keys among them) and arrays.
func (s *splitmix64) cmpValue(depth int) string {
	n := s.below(10)
	if depth >= 3 && n >= 7 {
		n = s.below(7)
	}
	switch n {
	case 0:
		return "null"
	case 1:
		return "true"
	case 2:
		return "false"
	case 3, 4:
		return s.pick(cmpNumbers)
	case 5, 6:
		return s.pick(cmpStrings)
	case 7, 8:
		k := s.below(4)
		parts := make([]string, k)
		for i := range parts {
			key := s.pick(cmpKeys)
			parts[i] = s.cmpSpace() + key + s.cmpSpace() + ":" + s.cmpSpace() + s.cmpValue(depth+1) + s.cmpSpace()
		}
		return "{" + strings.Join(parts, ",") + s.cmpSpace() + "}"
	default:
		k := s.below(4)
		parts := make([]string, k)
		for i := range parts {
			parts[i] = s.cmpSpace() + s.cmpValue(depth+1) + s.cmpSpace()
		}
		return "[" + strings.Join(parts, ",") + s.cmpSpace() + "]"
	}
}

func compactCorpus() seededCorpus {
	return seededCorpus{seed: compactSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (string, string) {
		doc := `{"ext":` + s.cmpSpace() + s.cmpValue(1) + s.cmpSpace() + `}`
		var v rawOmit
		if err := jsonutil.Unmarshal([]byte(doc), &v); err != nil {
			return doc, "error: " + err.Error()
		}
		return doc, marshalled(v)
	}}
}

var (
	extractOps     = []string{"openrtb2.Imp.Banner", "openrtb_ext.ExtRequestPrebid.Cache", "readObjectStart", "ReadString", "Skip", "unmarshalerDecoder", "main.T.F", "a b", ""}
	extractDetails = []string{"expect { or n, but found m", "request.imp[0].ext is invalid", "do not know how to skip: 109", "x: y", "", "value: with: colons", "request.", "found in #3"}
	extractSafe    = []string{"abc", "", `{"a":1}`, ", error found in #9"}
	extractAll     = []string{"abc", "", `{"a":1}`, ", error found in #9", "a: b", "x : y"}
)

// extractMessage is a message in jsoniter's shape: operations, a detail and the context jsoniter
// appends. When no ": " comes before the context, the context holds none either: Go's function
// slices out of range there and the port returns the message (unreachable with serde_json's
// messages, which carry no context).
func (s *splitmix64) extractMessage() string {
	k := s.below(4)
	ops := make([]string, k)
	for i := range ops {
		ops[i] = s.pick(extractOps)
	}
	detail := s.pick(extractDetails)
	msg := detail
	if k > 0 {
		msg = strings.Join(ops, ": ") + ": " + detail
	}
	if s.below(4) == 0 {
		return msg
	}
	contexts := extractAll
	if k == 0 && !strings.Contains(detail, ": ") {
		contexts = extractSafe
	}
	at := strconv.Itoa(s.below(100))
	return msg + ", error found in #" + at + " byte of ...|" + s.pick(contexts) + "|..., bigger context ...|" + s.pick(contexts) + "|..."
}

func extractCorpus() seededCorpus {
	return seededCorpus{seed: extractSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (input, outcome string) {
		msg := s.extractMessage()
		defer func() {
			if r := recover(); r != nil {
				input, outcome = msg, fmt.Sprint("panic: ", r)
			}
		}()
		return msg, tryExtractErrorMessage(errors.New(msg))
	}}
}

// sinDigits is n digits without a leading zero (or "0").
func (s *splitmix64) sinDigits(n int, leadingZero bool) string {
	if n == 0 {
		return "0"
	}
	var b strings.Builder
	for i := 0; i < n; i++ {
		d := s.below(10)
		if i == 0 && d == 0 && !leadingZero {
			d = 1
		}
		b.WriteByte(byte('0' + d))
	}
	return b.String()
}

// sinNumber is a JSON number: an integer of up to 21 digits, now and then with a fraction or an
// exponent.
func (s *splitmix64) sinNumber() string {
	sign := ""
	if s.below(3) == 1 {
		sign = "-"
	}
	text := sign + s.sinDigits(s.below(22), false)
	if s.below(8) == 0 {
		text += s.pick([]string{".5", "e2", ".0"})
	}
	return text
}

// stringIntQuirk is json.string-int-parse: a lone minus, which Go reads as 0, and a leading plus,
// which Go rejects and i64::from_str reads.
func stringIntQuirk(value string) bool {
	text := value
	if strings.HasPrefix(text, `"`) && len(text) >= 2 {
		text = text[1 : len(text)-1]
	}
	return text == "-" || strings.HasPrefix(text, "+")
}

func stringIntCorpus() seededCorpus {
	return seededCorpus{seed: stringIntSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (string, string) {
		var value string
		switch s.below(4) {
		case 0:
			value = s.sinNumber()
		case 1:
			switch s.below(6) {
			case 0:
				value = `"` + s.sinDigits(s.below(22), true) + `"`
			case 1:
				value = `""`
			case 2:
				value = `"-"`
			case 3:
				value = `"+` + s.sinDigits(1+s.below(5), true) + `"`
			case 4:
				value = `"` + s.pick([]string{" 5", "0x1", "1.5", "abc", "--1", "-12"}) + `"`
			default:
				value = `"` + backslashU + `0033"`
			}
		case 2:
			value = s.pick([]string{"true", "false", "null"})
		default:
			value = s.pick([]string{"[]", "{}", "[1]"})
		}
		doc := `{"item_id":` + value + `}`
		if stringIntQuirk(value) {
			return doc, "quirk string-int-parse"
		}
		var item stringIntItem
		if err := jsonutil.UnmarshalValid([]byte(doc), &item); err != nil {
			return doc, "error"
		}
		return doc, strconv.Itoa(int(item.ItemId))
	}}
}

var istExponents = []string{"", "", "e2", "E+2", "e-7", "e308", "e-320", "e400"}

// outOfRange is json.out-of-range-numbers: a number jsoniter's skipper rejects before IntString
// reads it, and serde_json reads. The skipper parses a number that starts with 0 as a float32 and
// one with an exponent as a float64 (others it only scans), and fails beyond their range.
func outOfRange(value string) bool {
	switch {
	case value == "":
		return false
	case value[0] == '0':
		_, err := strconv.ParseFloat(value, 32)
		return err != nil
	case (value[0] == '-' || value[0] >= '1' && value[0] <= '9') && strings.ContainsAny(value, "eE"):
		_, err := strconv.ParseFloat(value, 64)
		return err != nil
	default:
		return false
	}
}

// istNumber is a JSON number: an integer of up to 30 digits, or a decimal with an exponent, up to
// beyond float64's range.
func (s *splitmix64) istNumber() string {
	sign := ""
	if s.below(3) == 1 {
		sign = "-"
	}
	if s.below(2) == 0 {
		return sign + s.sinDigits(s.below(31), false)
	}
	text := sign + s.sinDigits(s.below(4), false)
	if s.below(2) == 0 {
		text += "." + s.sinDigits(1+s.below(4), true)
	}
	return text + s.pick(istExponents)
}

func intStringCorpus() seededCorpus {
	return seededCorpus{seed: intStringSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (string, string) {
		var value string
		switch s.below(5) {
		case 0, 1:
			value = s.istNumber()
		case 2:
			value = `"` + s.strContent(false, true) + `"`
		case 3:
			value = s.pick([]string{"true", "false", "null"})
		default:
			value = s.pick([]string{"[]", "{}", "[1]"})
		}
		doc := `{"item_id":` + value + `}`
		if outOfRange(value) {
			return doc, "quirk out-of-range-numbers"
		}
		var item intStringItem
		if err := jsonutil.UnmarshalValid([]byte(doc), &item); err != nil {
			return doc, "error"
		}
		return doc, hexOf(string(item.ItemId))
	}}
}

var pisSpaces = []string{"", "", " ", "\n", "\t "}

// pisValue is a JSON value ParseIntoString reads alike on both sides: strings without lone
// surrogate escapes, numbers (beyond float64's range among them, which read "+Inf" and "-Inf"),
// literals, objects and arrays, or nothing.
func (s *splitmix64) pisValue() string {
	switch s.below(6) {
	case 0:
		return `"` + s.strContent(false, false) + `"`
	case 1:
		return s.sinNumber()
	case 2:
		return s.istNumber()
	case 3:
		return s.pick([]string{"true", "false", "null"})
	case 4:
		return s.pick([]string{`{"a": 1}`, `[1, 2]`, `{}`, `[]`, `{"a":[1,{"b":null}]}`})
	default:
		return ""
	}
}

func parseIntoCorpus() seededCorpus {
	return seededCorpus{seed: parseIntoSeed, count: readersCount, batch: readersBatchSize, next: func(s *splitmix64) (string, string) {
		input := s.pick(pisSpaces) + s.pisValue() + s.pick(pisSpaces)
		var got *string
		if err := jsonutil.ParseIntoString([]byte(input), &got); err != nil {
			return input, "error"
		}
		if got == nil {
			return input, "nil"
		}
		return input, hexOf(*got)
	}}
}

// readersCorpora names each corpus of readers.json.
func readersCorpora() map[string]seededCorpus {
	return map[string]seededCorpus{
		"strings":           stringsCorpus(),
		"compact":           compactCorpus(),
		"extract":           extractCorpus(),
		"string_int":        stringIntCorpus(),
		"int_string":        intStringCorpus(),
		"parse_into_string": parseIntoCorpus(),
	}
}

type readersDoc struct {
	Header header                   `json:"header"`
	Seeded map[string]seededDigests `json:"seeded"`
}

func readersGolden() readersDoc {
	doc := readersDoc{
		Header: newHeader("github.com/json-iterator/go", "github.com/buger/jsonparser", "github.com/tidwall/gjson"),
		Seeded: map[string]seededDigests{},
	}
	for name, corpus := range readersCorpora() {
		doc.Seeded[name] = corpus.digests()
	}
	return doc
}
