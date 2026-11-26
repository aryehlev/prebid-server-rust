package main

import (
	"strconv"
	"strings"

	pbscurrency "github.com/prebid/prebid-server/v4/currency"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// decode.golden records how RateConverter.fetch decodes a currency file
// (jsonutil.UnmarshalValid into a currency.Rates) without the library's error text, which
// the Rust port's serde_json words differently (json.error-texts): "ok:" and the decoded map as
// dumpConversions renders it, or "err".
//
// Its fixture lines add the inputs rates_json.golden's jsonCases lack for the differences the
// currency file can show (currency.rates-json). Its decode-batch lines hold a seeded corpus of
// currency files the way a rates server could write them: the conversions member under its
// name or an escape of it, or null, or absent, members Rates skips holding every kind of JSON
// value, rates in every spelling of a number up to float64's range and past it, keys with
// escapes, surrogates and duplicates, null inner maps, spaces between the tokens and
// whitespace around the document, and now and then a rate both decoders reject, a document
// that is no currency file or one cut short. The Rust tests (tests/rates.rs) build the same
// inputs from the same splitmix64 draws, so each batch line holds digests only. The corpus
// leaves out what the ledger lists as differences (which the fixtures hold): another case of
// "conversions", a second conversions member, null where a key or a rate belongs, a null
// document, an array document, a leading plus, malformed or out-of-range numbers in skipped
// members, raw control characters and bytes that are not UTF-8, a NUL after the document, and
// nesting past 128 levels.

const decodeCases = 100 * randomBatch

// decodeFixtures are fixed inputs, each written with its outcome.
var decodeFixtures = []string{
	// Arrays where the struct belongs (json.struct-from-array).
	`[{}]`,
	`[null]`,
	`[{"USD":{"GBP":1}}]`,
	`[{"USD":{"GBP":1}},5]`,
	// The member's name written with escapes, and in other cases (json.key-case).
	`{"\u0063onversions":{"USD":{"GBP":1}}}`,
	`{"conver\u0073ions":{"USD":{"GBP":1}}}`,
	`{"CoNvErSiOnS":{"USD":{"GBP":1}}}`,
	`{"\u0043onversions":{"USD":{"GBP":1}}}`,
	`{"conversions":{"USD":{"GBP":1}},"Conversions":{"EUR":{"GBP":2}}}`,
	`{"Conversions":{"USD":{"GBP":1}},"conversions":null}`,
	// Duplicate keys inside the map: the last one wins in both.
	`{"conversions":{"USD":{"GBP":1,"GBP":2}}}`,
	`{"conversions":{"USD":{"GBP":1},"usd":{"GBP":2},"USD":{"EUR":3}}}`,
	`{"conversions":{"\u0055SD":{"GBP":1},"USD":{"GBP":2}}}`,
	// Null rates beside others (json.null-elements).
	`{"conversions":{"USD":{"GBP":null,"EUR":2}}}`,
	`{"conversions":{"USD":{"GBP":2,"EUR":null}}}`,
	// Numbers at the edges of float64.
	`{"conversions":{"USD":{"GBP":-1e-400}}}`,
	`{"conversions":{"USD":{"GBP":1.7976931348623158e308}}}`,
	`{"conversions":{"USD":{"GBP":1.7976931348623159e308}}}`,
	`{"conversions":{"USD":{"GBP":2.4703282292062328e-324}}}`,
	`{"conversions":{"USD":{"GBP":1e309}}}`,
	// Numbers in skipped members beyond float64's or float32's range
	// (json.out-of-range-numbers), and malformed ones jsoniter's skipper reads.
	`{"x":1e309,"conversions":{}}`,
	`{"x":-1e309,"conversions":{}}`,
	`{"x":1.8e308,"conversions":{}}`,
	`{"x":0.4e39,"conversions":{}}`,
	`{"x":[-01],"conversions":{}}`,
	`{"x":{"y":-.5},"conversions":{}}`,
	`{"x":-0.5,"conversions":{}}`,
	// Control characters: escaped, and raw after an escape or in a member name
	// (json.control-characters).
	`{"x":"a\u0000b","conversions":{"\u0000":{"\t":1}}}`,
	"{\"x\":\"a\\n\tb\",\"conversions\":{}}",
	"{\"conversions\":{\"a\\n\tb\":{\"GBP\":1}}}",
	"{\"conver\tsions\":{\"USD\":{\"GBP\":1}}}",
	"{\"x\\n\t\":1,\"conversions\":{}}",
	// A NUL after the document (json.trailing-nul).
	"{\"conversions\":{\"USD\":{\"GBP\":1}}}\x00",
	"{\"conversions\":{\"USD\":{\"GBP\":1}}} \x00 x",
	// Surrogate escapes in keys: pairs, lone ones and two high ones.
	`{"conversions":{"\ud83d\udcb5":{"\ud800\ud800":1,"\udbff\udfff":2,"\ud800\u0041":3}}}`,
}

// Whitespace around a document.
var decodeOuterSpace = []string{``, ``, ``, ` `, "\n", "\r\n", "\t", " \r\n\t "}

// Documents that are no currency file, which both decoders reject.
var decodeNonObjects = []string{
	``, ` `, `[]`, `"str"`, `5`, `true`, `false`, `nul`, `x`, `{`, `}`, `{}x`, `{}}`, `{,}`,
	`{"a"}`, `{"a":}`, `{:1}`,
}

// Names of the member Rates reads.
var decodeConversionsNames = []string{
	`"conversions"`, `"conversions"`, `"conversions"`, `"\u0063onversions"`, `"conver\u0073ions"`,
}

// Names of members Rates skips: none is "conversions" in another case.
var decodeOtherNames = []string{
	`"dataAsOf"`, `"generatedAt"`, `"x"`, `""`, `"conversion"`, `"conversions "`,
	`" conversions"`, `"rates"`, `"conversions\u0000"`, `"conversions\n"`, `"\u0064ataAsOf"`,
	`"é"`, `"💵"`, `"\ud83d\udcb5"`, `"a\"b"`, `"a\\b"`, `"\ud800"`,
}

// Names inside skipped objects, where any name is skipped.
var decodeNestedNames = []string{
	`"a"`, `"b"`, `""`, `"conversions"`, `"Conversions"`, `"USD"`, `"a\"b"`, `"\u00e9"`,
}

// Keys of the conversions map and its inner maps.
var decodeCodeKeys = []string{
	`"USD"`, `"GBP"`, `"EUR"`, `"MXN"`, `"JPY"`, `"SEK"`, `"usd"`, `"Gbp"`, `"XXX"`, `""`,
	`"\u0055SD"`, `"U\u0053D"`, `"US\u0044"`, `"\u00e9"`, `"é"`, `"\ud83d\udcb5"`, `"💵"`,
	`"a\/b"`, `"a\"b"`, `"a\\b"`, `"\t"`, `"\u0000"`, `"\ud800"`, `"\udc00"`, `"\ud800A"`,
	`"\udc00\ud800"`, `"\ud800\ud800"`,
}

// Skipped strings.
var decodeSkippedStrings = []string{
	`"2018-09-12"`, `"2022-11-24T15:00:46.363Z"`, `""`, `"a\"b"`, `"a\\b"`, `"\u00e9"`, `"é"`,
	`"\ud800"`, `"💵"`, `"\n\t\r"`, `"\/"`, `"{\"conversions\":1}"`,
}

var decodeLiterals = []string{`true`, `false`, `null`}

// Skipped numbers: well-formed, and small enough for jsoniter's skipper, which parses a number
// with an exponent as a float64 and one that starts with 0 as a float32.
var decodeSkippedNumbers = []string{
	`0`, `-0`, `1`, `-1`, `0.5`, `-0.5`, `12.25`, `-1.5e3`, `1E2`, `1e-5`, `0e5`, `-0e0`,
	`123456789012345678901234567890`,
}

// Rates both decoders read the same: zeros of both signs, subnormals and underflow, float64's
// largest value and its rounding boundary, integers past 2^53 and 2^64, and long decimals.
var decodeRateValues = []string{
	"0", "-0", "0.0", "-0.0", "1", "-1", "0.5", "1e5", "1E5", "1e+5", "1E+05", "1e-5",
	"1.5E-3", "5e-324", "4.9e-324", "2.4703282292062327e-324", "1e-310", "1e-400", "-1e-400",
	"2.2250738585072014e-308", "2.2250738585072011e-308", "1.7976931348623157e308",
	"1.7976931348623158e308", "1e308", "9007199254740993", "9007199254740993.5",
	"123456789012345678901234567890", "1844674407370955161", "18446744073709551615",
	"18446744073709551616", "-9223372036854775808", "-9223372036854775809",
	"0.1234567890123456789", "1.0000001", "0.77208", "1.2952", "10.23842",
	"123456789.123456789", "0.333333333333333314829616256247", "1e22", "1e23",
	"100000000000000000000000", "0.1", "0.2", "0.30000000000000004", "8.98846567431158e307",
	"3.4028234663852886e38", "1e-7", "0.000001", "-1.5e3", "12.5", "0e1", "0E-5", "-0e0",
	"0.0e+10",
}

// Rates both decoders reject.
var decodeBadRates = []string{
	`"1.5"`, `true`, `[1]`, `{}`, `01`, `00.5`, `.5`, `-.5`, `1.`, `5.e3`, `1e`, `0x10`, `NaN`,
	`Infinity`, `--5`, `-`, `1.2.3`, `1e400`, `-1e400`,
}

var decodeSigns = []string{"", "", "-"}

var decodeColons = []string{":", ":", ":", " : ", ": ", " :"}

var decodeCommas = []string{",", ",", ", ", " ,"}

var decodeExponents = []string{"e", "E"}

var decodeExponentSigns = []string{"", "+", "-"}

// decodeLists are the lists the corpus draws from, written to decode.golden so that the Rust
// tests draw from the same ones.
var decodeLists = []struct {
	name  string
	items []string
}{
	{"outer_space", decodeOuterSpace},
	{"non_objects", decodeNonObjects},
	{"conversions_names", decodeConversionsNames},
	{"other_names", decodeOtherNames},
	{"nested_names", decodeNestedNames},
	{"code_keys", decodeCodeKeys},
	{"skipped_strings", decodeSkippedStrings},
	{"literals", decodeLiterals},
	{"skipped_numbers", decodeSkippedNumbers},
	{"rate_values", decodeRateValues},
	{"bad_rates", decodeBadRates},
	{"signs", decodeSigns},
	{"colons", decodeColons},
	{"commas", decodeCommas},
	{"exponents", decodeExponents},
	{"exponent_signs", decodeExponentSigns},
}

// decodeOutcome decodes data as RateConverter.fetch does and renders the outcome without the
// error's text.
func decodeOutcome(data []byte) string {
	rates := &pbscurrency.Rates{}
	if err := jsonutil.UnmarshalValid(data, rates); err != nil {
		return "err"
	}
	return "ok:" + dumpConversions(rates.Conversions)
}

// joinItems joins items with commas, drawing each comma's spaces.
func joinItems(s *splitmix64, items []string) string {
	var sb strings.Builder
	for i, item := range items {
		if i > 0 {
			sb.WriteString(s.pick(decodeCommas))
		}
		sb.WriteString(item)
	}
	return sb.String()
}

// randomWhole returns the digits before a number's dot: 0, or digits without a leading zero.
func randomWhole(s *splitmix64, maxRest int) string {
	if s.below(3) == 0 {
		return "0"
	}
	lead := 1 + s.below(9)
	rest := s.digits(s.below(maxRest))
	return strconv.Itoa(lead) + rest
}

// randomRateNumber returns a rate: an edge value, or an integer, a decimal or a number with an
// exponent, whose value may pass float64's range.
func randomRateNumber(s *splitmix64) string {
	switch s.below(5) {
	case 0:
		return s.pick(decodeRateValues)
	case 1:
		sign := s.pick(decodeSigns)
		lead := 1 + s.below(9)
		rest := s.digits(s.below(25))
		return sign + strconv.Itoa(lead) + rest
	case 2:
		sign := s.pick(decodeSigns)
		whole := randomWhole(s, 20)
		fraction := s.digits(1 + s.below(20))
		return sign + whole + "." + fraction
	default:
		sign := s.pick(decodeSigns)
		mantissa := randomWhole(s, 20)
		if s.below(2) == 0 {
			fraction := s.digits(1 + s.below(17))
			mantissa += "." + fraction
		}
		e := s.pick(decodeExponents)
		expSign := s.pick(decodeExponentSigns)
		exponent := s.below(330)
		return sign + mantissa + e + expSign + strconv.Itoa(exponent)
	}
}

// randomRateValue returns a rate, and one in 200 times a value both decoders reject.
func randomRateValue(s *splitmix64) string {
	if s.below(200) == 0 {
		return s.pick(decodeBadRates)
	}
	return randomRateNumber(s)
}

// randomSkippedNumber returns a well-formed number that jsoniter's skipper reads.
func randomSkippedNumber(s *splitmix64) string {
	sign := s.pick(decodeSigns)
	switch s.below(3) {
	case 0:
		lead := 1 + s.below(9)
		rest := s.digits(s.below(15))
		return sign + strconv.Itoa(lead) + rest
	case 1:
		whole := randomWhole(s, 15)
		fraction := s.digits(1 + s.below(15))
		return sign + whole + "." + fraction
	default:
		lead := 1 + s.below(9)
		mantissa := strconv.Itoa(lead)
		fraction := s.digits(s.below(10))
		if fraction != "" {
			mantissa += "." + fraction
		}
		e := s.pick(decodeExponents)
		expSign := s.pick(decodeExponentSigns)
		exponent := s.below(21)
		return sign + mantissa + e + expSign + strconv.Itoa(exponent)
	}
}

// randomSkippedValue returns a value of a member Rates skips, nested at most four deep.
func randomSkippedValue(s *splitmix64, depth int) string {
	kinds := 7
	if depth >= 3 {
		kinds = 4
	}
	switch s.below(kinds) {
	case 0:
		return s.pick(decodeSkippedStrings)
	case 1:
		return s.pick(decodeLiterals)
	case 2:
		return s.pick(decodeSkippedNumbers)
	case 3:
		return randomSkippedNumber(s)
	case 4, 5:
		items := make([]string, 0, 3)
		for i := s.below(4); i > 0; i-- {
			items = append(items, randomSkippedValue(s, depth+1))
		}
		return "[" + joinItems(s, items) + "]"
	default:
		members := make([]string, 0, 3)
		for i := s.below(4); i > 0; i-- {
			name := s.pick(decodeNestedNames)
			colon := s.pick(decodeColons)
			value := randomSkippedValue(s, depth+1)
			members = append(members, name+colon+value)
		}
		return "{" + joinItems(s, members) + "}"
	}
}

// randomOtherMember returns a member Rates skips.
func randomOtherMember(s *splitmix64) string {
	name := s.pick(decodeOtherNames)
	colon := s.pick(decodeColons)
	value := randomSkippedValue(s, 0)
	return name + colon + value
}

// randomConversions returns the value of the conversions member: a map of currencies to maps
// of rates, where a currency may map to null.
func randomConversions(s *splitmix64) string {
	if s.below(12) == 0 {
		return `{}`
	}
	entries := make([]string, 0, 6)
	for i := 1 + s.below(6); i > 0; i-- {
		key := s.pick(decodeCodeKeys)
		colon := s.pick(decodeColons)
		if s.below(8) == 0 {
			entries = append(entries, key+colon+"null")
			continue
		}
		inner := make([]string, 0, 5)
		for j := s.below(6); j > 0; j-- {
			to := s.pick(decodeCodeKeys)
			innerColon := s.pick(decodeColons)
			rate := randomRateValue(s)
			inner = append(inner, to+innerColon+rate)
		}
		entries = append(entries, key+colon+"{"+joinItems(s, inner)+"}")
	}
	return "{" + joinItems(s, entries) + "}"
}

// randomRatesObject returns a currency file: skipped members around the conversions member,
// which may be null or absent.
func randomRatesObject(s *splitmix64) string {
	members := make([]string, 0, 5)
	for i := s.below(3); i > 0; i-- {
		members = append(members, randomOtherMember(s))
	}
	switch s.below(8) {
	case 0:
	case 1:
		name := s.pick(decodeConversionsNames)
		colon := s.pick(decodeColons)
		members = append(members, name+colon+"null")
	default:
		name := s.pick(decodeConversionsNames)
		colon := s.pick(decodeColons)
		value := randomConversions(s)
		members = append(members, name+colon+value)
	}
	for i := s.below(3); i > 0; i-- {
		members = append(members, randomOtherMember(s))
	}
	return "{" + joinItems(s, members) + "}"
}

// randomDocument returns one input of the corpus: in 32, one document that is no currency
// file and one cut short before its end; the others whole, with whitespace around them.
func randomDocument(s *splitmix64) string {
	kind := s.below(32)
	if kind == 0 {
		return s.pick(decodeNonObjects)
	}
	doc := randomRatesObject(s)
	if kind == 1 {
		doc = doc[:s.below(len(doc))]
	}
	lead := s.pick(decodeOuterSpace)
	trail := s.pick(decodeOuterSpace)
	return lead + doc + trail
}

func writeDecode(dir string) {
	g := newGolden(dir, "decode.golden")
	defer g.close()
	for _, list := range decodeLists {
		fields := []string{"list", list.name}
		for _, item := range list.items {
			fields = append(fields, hx(item))
		}
		g.line(fields...)
	}
	for _, fixture := range decodeFixtures {
		g.line("fixture", hx(fixture), hx(decodeOutcome([]byte(fixture))))
	}
	s := &splitmix64{state: 20261126}
	docs := &batchDigests{g: g, kind: "decode"}
	for i := 0; i < decodeCases; i++ {
		doc := randomDocument(s)
		docs.add(doc, decodeOutcome([]byte(doc)))
	}
}
