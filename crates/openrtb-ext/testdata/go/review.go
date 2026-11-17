package main

// The adversarial review's goldens, aimed at what the corpus and the per-field mutations do not
// reach:
//
//   - review.tsv (the format of mutations.tsv): the values that the UnmarshalJSON methods tell
//     apart, nested two to four levels deep in the types that hold them (jsoniter writes the
//     whole path into its error text, and the outer decoders leave partial values behind),
//     grids of price granularity, interstitial and AMP documents, and every edge value at every
//     jsonutil.StringInt, jsonutil.IntString and json.Number field of every ported type.
//   - review_functions.tsv (the format of functions.tsv): bidder names under Go's Unicode case
//     mapping and folding (the Kelvin sign, the long s, the dotted and dotless i), sampled
//     multibid, deal tier, alternate bidder code, ATTS, floors and consented-provider inputs.
//   - review_aliases.tsv (the format of functions.tsv, replayed in order): SetAliasBidderName
//     with reserved, folded, colliding, repeated and non-ASCII names, each followed by the
//     lookups it changes. Registering an alias changes the package's state for good, so these
//     calls run after every other case.

import (
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"math"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// splitMix is SplitMix64: a small generator for the sampled inputs, the same on every run.
type splitMix struct{ state uint64 }

func (s *splitMix) next() uint64 {
	s.state += 0x9e3779b97f4a7c15
	z := s.state
	z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9
	z = (z ^ (z >> 27)) * 0x94d049bb133111eb
	return z ^ (z >> 31)
}

func (s *splitMix) intn(n int) int { return int(s.next() % uint64(n)) }

func (s *splitMix) pick(options []string) string { return options[s.intn(len(options))] }

// object is a JSON object of the members that are not empty.
func object(members ...string) string {
	var kept []string
	for _, m := range members {
		if m != "" {
			kept = append(kept, m)
		}
	}
	return "{" + strings.Join(kept, ",") + "}"
}

// nest puts value at each "@" of wrapper.
func nest(wrapper, value string) string { return strings.ReplaceAll(wrapper, "@", value) }

// allValues are a method's samples, edges and wrong values.
func allValues(v values) []string {
	return append(append(append([]string(nil), v.samples...), v.edges...), v.wrong...)
}

// priceGranularityValues are PriceGranularity documents: the legacy IDs in other spellings, and
// every combination of a precision and ranges member.
func priceGranularityValues() []string {
	out := allValues(methodValues[reflect.TypeOf(openrtb_ext.PriceGranularity{})])
	out = append(out,
		`"Low"`, `"HIGH"`, `" low"`, `"low "`, escapes(`"m%u0065d"`), escapes(`"dense%u0000"`),
		` "med" `, "\n\"auto\"", `"medium"`, `"med"`+"\t", `"de"`, `"densee"`,
	)
	precisions := []string{
		``, `"precision":null`, `"precision":0`, `"precision":2`, `"precision":-1`,
		`"precision":"2"`, `"precision":2.5`, `"precision":1e1`, `"precision":true`,
		`"precision":99999999999999999999`, `"PRECISION":3`,
	}
	ranges := []string{
		``, `"ranges":null`, `"ranges":[]`, `"ranges":[{}]`, `"ranges":[null]`,
		`"ranges":[{"min":0,"max":5,"increment":0.1}]`,
		`"ranges":[{"max":20,"increment":0.1},{"min":20,"max":50,"increment":1}]`,
		`"ranges":[{"min":"0","max":5}]`, `"ranges":[{"max":1e400}]`, `"ranges":"x"`,
		`"ranges":{}`, `"ranges":[{"MIN":1,"Max":2,"INCREMENT":0.5}]`,
		`"ranges":[{"min":1,"max":2,"increment":0.5,"zz":[1]}]`,
	}
	for _, p := range precisions {
		for _, r := range ranges {
			out = append(out, object(p, r))
		}
	}
	return append(out,
		`{"precision":2,"precision":3}`, `{"ranges":[{"max":1}],"ranges":null}`,
		`{ "precision" : 2 , "ranges" : [ ] }`, `{"precision":2,}`, `{"precision":2`,
	)
}

// percentValues are the minwidthperc and minheightperc values of the interstitial documents.
var percentValues = []string{
	``, `0`, `1`, `50`, `100`, `101`, `-1`, `-0`, `05`, `1.0`, `1e2`, `"5"`, `null`, `true`,
	`[]`, `{}`, `+5`, `99999999999999999999`, `9223372036854775807`, `-9223372036854775808`,
	`5 `,
}

// interstitialValues are ExtDeviceInt documents: every pair of percentValues, the method values,
// and documents where jsonparser has to look past other members.
func interstitialValues() []string {
	out := allValues(methodValues[reflect.TypeOf(openrtb_ext.ExtDeviceInt{})])
	member := func(key, value string) string {
		if value == "" {
			return ""
		}
		return strconv.Quote(key) + ":" + value
	}
	for _, w := range percentValues {
		for _, h := range percentValues {
			out = append(out, object(member("minwidthperc", w), member("minheightperc", h)))
		}
	}
	return append(out,
		escapes(`{"min%u0077idthperc":5,"minheightperc":6}`),
		`{"a":[{"minwidthperc":1}],"minwidthperc":2,"minheightperc":3}`,
		`{"a":"\"minwidthperc\":7","minwidthperc":2,"minheightperc":3}`,
		`{"minwidthperc" : 2 , "minheightperc" : 3 }`,
		`{"minwidthperc":2,"minheightperc":3`,
		`{"minwidthperc":2,"minheightperc":3,}`,
		`{"minwidthperc" 2,"minheightperc":3}`,
		`{"minheightperc":3,"minwidthperc":2}`,
		`{"minwidthperc":[2],"minheightperc":3}`,
		`{"minwidthperc":{"minwidthperc":2},"minheightperc":3}`,
	)
}

// ampValues and ampForms make the ExtSite documents.
var (
	ampValues = []string{
		`0`, `1`, `2`, `-1`, `10`, `1.0`, `01`, `00`, `1e0`, `"1"`, `null`, `true`, `[1]`, `{}`,
		`-0`, `9`,
	}
	ampForms = []string{
		`{"amp":@}`, `{"amp": @ }`, `{"x":1,"amp":@}`, `{"amp":@,"x":{"amp":5}}`,
		`{"a":{"amp":2},"amp":@}`, escapes(`{"am%u0070":@}`), `{"amp":@,"amp":0}`, `{"AMP":@}`,
		`{"amp":@`, `[{"amp":@}]`, `{"a":[{"amp":3}],"amp":@}`, `{"amp":@}x`,
	}
)

// keywordValues are ExtImpAppnexusKeywords documents: the method values, one-key objects (an
// object of more keys is joined in Go's random map order), and sampled arrays of entries.
func keywordValues(rng *splitMix) []string {
	out := allValues(methodValues[reflect.TypeOf(openrtb_ext.ExtImpAppnexusKeywords(""))])
	out = append(out,
		`{"k":["a","b"]}`, `{"k":[]}`, `{"k":null}`, `{"":["x"]}`, `{"k":[""]}`,
		`{"k":["a","a"]}`, `{"k":["a"],"k":["b"]}`, `{"k=v":[","]}`, escapes(`{"%u00e9":["%u00e8"]}`),
		`{"k":["a",null]}`, `{"k":[1]}`, `{"k":"a"}`, `{"k":[["a"]]}`,
	)
	entries := []string{
		`{"key":"a"}`, `{"key":"a","value":[]}`, `{"key":"a","value":["b"]}`,
		`{"key":"a","value":["b","c"]}`, `{"value":["x"]}`, `{}`, `{"key":""}`,
		escapes(`{"key":"%u00e9","value":["%u00e8"]}`), `{"key":"k=v","value":[","]}`, `null`,
		`{"key":"a","value":null}`, `{"key":"a","value":[null]}`, `{"KEY":"a","Value":["v"]}`,
		`{"key":"a","key":"b"}`, `{"key":"a","value":[""]}`, `{"key":"<&>","value":["<&>"]}`,
	}
	for i := 0; i < 120; i++ {
		var parts []string
		for n := rng.intn(5); n > 0; n-- {
			parts = append(parts, rng.pick(entries))
		}
		out = append(out, "["+strings.Join(parts, ",")+"]")
	}
	return append(out, `[{"key":1}]`, `[{"key":"a","value":"b"}]`, `[{"key":"a","value":[1]}]`, `["a"]`)
}

// stringOrArrayValues are stringOrStringArray documents.
func stringOrArrayValues(t reflect.Type) []string {
	v, ok := valuesOf(t)
	if !ok {
		log.Fatalf("review: no method values for %v", t)
	}
	return append(allValues(v),
		`["a","b","c"]`, `[""]`, `["a",null]`, `[["a"]]`, `[{}]`, `"a\"b"`,
		escapes(`"%ud83d%ude00"`), `[1,2]`, `[true]`, `"x" `, ` ["a"]`, `["a" , "b"]`, `[,]`,
		`["a",]`, escapes(`"%u0000"`), `"<&>"`, `[null,null]`, `{"a":["b"]}`,
	)
}

// numberEdges are the values of the fields of the jsonutil.StringInt, jsonutil.IntString and
// json.Number types, whose decoding is the types' own.
var numberEdges = []string{
	`0`, `-0`, `1`, `-1`, `"1"`, `"-1"`, `"+1"`, `+1`, `" 1"`, `"1 "`, `""`, `"0"`, `"007"`,
	`007`, `"1.5"`, `1.5`, `1.0`, `-1.50`, `1e2`, `1E+2`, `1e-2`, `"1e2"`, `"0x10"`, `"1_0"`,
	`9223372036854775807`, `9223372036854775808`, `-9223372036854775808`, `-9223372036854775809`,
	`"9223372036854775807"`, `"9223372036854775808"`, `18446744073709551616`,
	`123456789012345678901234567890`, `1e400`, `null`, `true`, `false`, `[]`, `{}`, `[1]`,
	escapes(`"%u0031"`), `"a"`, `"-"`, `-`, `"--1"`, `"12a"`, `"a\"b"`, `" "`, `.5`, `1.`,
	`"<&>"`, escapes(`"%u00e9"`), `"1`, `1 `,
}

// numberField is a field of one of the types with their own number decoding, as the JSON value
// that holds it: the field's key, and "@" for the value inside a pointer or a slice.
type numberField struct {
	typ     reflect.Type
	key     string
	element string
}

// numberFields are the fields of every ported struct type whose type (also through a pointer or
// slice) is jsonutil.StringInt, jsonutil.IntString or json.Number.
func numberFields(decodable []reflect.Type) []numberField {
	special := map[reflect.Type]bool{
		reflect.TypeOf(jsonutil.StringInt(0)):  true,
		reflect.TypeOf(jsonutil.IntString("")): true,
		numberType:                             true,
	}
	var out []numberField
	for _, t := range decodable {
		if t.Kind() != reflect.Struct {
			continue
		}
		for _, f := range jsonFields(t, nil) {
			ft, element := f.typ, "@"
			for ft.Kind() == reflect.Ptr {
				ft = ft.Elem()
			}
			if ft.Kind() == reflect.Slice && special[ft.Elem()] {
				ft, element = ft.Elem(), "[@]"
			}
			if special[ft] {
				out = append(out, numberField{typ: t, key: f.key, element: element})
			}
		}
	}
	return out
}

// reviewDecodeCases are review.tsv's documents.
func reviewDecodeCases(types map[string]reflect.Type, decodable []reflect.Type) []testCase {
	var out []testCase
	typ := func(name string) reflect.Type {
		t, ok := types[name]
		if !ok {
			log.Fatalf("review: no ported type %s", name)
		}
		return t
	}
	add := func(t reflect.Type, label, doc string) {
		out = append(out, testCase{ref: "'" + doc, data: []byte(doc), typ: t, label: label})
	}
	type wrapper struct {
		typ  string
		form string
	}
	nested := func(group string, vals []string, wrappers []wrapper) {
		for wi, w := range wrappers {
			t := typ(w.typ)
			for vi, v := range vals {
				add(t, fmt.Sprintf("%s %d.%d", group, wi, vi), nest(w.form, v))
			}
		}
	}
	rng := &splitMix{state: 0x0b5e55ed}

	nested("price granularity", priceGranularityValues(), []wrapper{
		{"openrtb_ext.PriceGranularity", `@`},
		{"openrtb_ext.ExtRequestTargeting", `{"pricegranularity":@}`},
		{"openrtb_ext.ExtRequestTargeting", `{"includewinners":true,"pricegranularity":@,"includebidderkeys":true}`},
		{"openrtb_ext.ExtRequestTargeting", `{"mediatypepricegranularity":{"banner":@,"video":"low"}}`},
		{"openrtb_ext.MediaTypePriceGranularity", `{"video":@}`},
		{"openrtb_ext.ExtRequestPrebid", `{"debug":true,"targeting":{"pricegranularity":@},"supportdeals":true}`},
		{"openrtb_ext.ExtRequestPrebid", `{"targeting":{"mediatypepricegranularity":{"native":@}}}`},
		{"openrtb_ext.ExtRequest", `{"prebid":{"targeting":{"pricegranularity":@}},"schain":{"ver":"1.0"}}`},
		{"openrtb_ext.ExtRequest", `{"prebid":{"targeting":{"mediatypepricegranularity":{"banner":@}}}}`},
	})

	interstitials := interstitialValues()
	nested("interstitial", interstitials, []wrapper{{"openrtb_ext.ExtDeviceInt", `@`}})
	var sampled []string
	for i, v := range interstitials {
		if i < 45 || i%9 == 0 {
			sampled = append(sampled, v)
		}
	}
	nested("nested interstitial", sampled, []wrapper{
		{"openrtb_ext.ExtDevicePrebid", `{"interstitial":@}`},
		{"openrtb_ext.ExtDevice", `{"atts":3,"prebid":{"interstitial":@}}`},
		{"openrtb_ext.ExtDevice", `{"prebid":{"interstitial":@},"atts":2}`},
	})

	for fi, form := range ampForms {
		for vi, v := range ampValues {
			add(typ("openrtb_ext.ExtSite"), fmt.Sprintf("amp %d.%d", fi, vi), nest(form, v))
		}
	}

	nested("keywords", keywordValues(rng), []wrapper{
		{"openrtb_ext.ExtImpAppnexusKeywords", `@`},
		{"openrtb_ext.ExtImpAppnexus", `{"placementId":1,"keywords":@,"member":"x"}`},
	})

	goldbach := typ("openrtb_ext.ImpExtGoldbach")
	targeting, ok := goldbach.FieldByName("CustomTargeting")
	if !ok {
		log.Fatal("review: ImpExtGoldbach has no CustomTargeting")
	}
	nested("custom targeting", stringOrArrayValues(targeting.Type.Elem()), []wrapper{
		{"openrtb_ext.ImpExtGoldbach", `{"publisherId":"p","customTargeting":{"k":@}}`},
		{"openrtb_ext.ImpExtGoldbach", `{"customTargeting":{"a":"x","k":@,"z":["y"]},"slotId":"s"}`},
	})

	for _, f := range numberFields(decodable) {
		for vi, v := range numberEdges {
			doc := "{" + strconv.Quote(f.key) + ":" + nest(f.element, v) + "}"
			add(f.typ, fmt.Sprintf("number %s %d", f.key, vi), doc)
		}
		add(f.typ, "number "+f.key+" twice", "{"+strconv.Quote(f.key)+":"+nest(f.element, `"1"`)+
			","+strconv.Quote(f.key)+":"+nest(f.element, `2`)+"}")
	}

	impPrebid := typ("openrtb_ext.ExtImpPrebid")
	members := []string{
		`"storedrequest":{"id":"x"}`, `"storedrequest":{"id":1}`, `"storedrequest":{"ID":"x"}`,
		`"storedrequest":null`, `"storedrequest":"x"`, `"storedrequest":{}`,
		`"storedrequest":{"id":null}`, `"storedrequest":{"id":"x","id":"y"}`, `"storedrequest":[]`,
		`"bidder":{"appnexus":{"placementId":1}}`, `"bidder":null`, `"bidder":{"a":null}`,
		`"bidder":[]`, `"bidder":{"a":1,"b":"x","c":[1,{"d":null}]}`, `"bidder":{"a":{"x":1},"a":{"y":2}}`,
		`"bidder":{"":{}}`, `"bidder":{"a":}`, `"bidder":{"a":{ "x" : 1 }}`, `"bidder":{"a":"x"}`,
		`"is_rewarded_inventory":1`, `"is_rewarded_inventory":128`, `"is_rewarded_inventory":"1"`,
		`"options":{"echovideoattrs":true}`, `"options":null`, `"adunitcode":"au"`,
		`"passthrough":{"a":[1]}`, `"passthrough":null`, `"floors":{"floorrule":"r","floorrulevalue":1.5}`,
		`"floors":{"floorvalue":"x"}`, `"imp":{"appnexus":{"a":1}}`, `"imp":null`,
		`"storedauctionresponse":{"id":"r"}`, `"storedbidresponse":[{"bidder":"b","id":"i"}]`,
		`"storedbidresponse":[null]`, `"STOREDREQUEST":{"id":"u"}`, `"Bidder":{"x":{}}`,
	}
	for i, m := range members {
		add(impPrebid, fmt.Sprintf("imp prebid %d", i), "{"+m+"}")
	}
	for i := 0; i < 150; i++ {
		var parts []string
		for n := 1 + rng.intn(4); n > 0; n-- {
			parts = append(parts, rng.pick(members))
		}
		add(impPrebid, fmt.Sprintf("imp prebid sample %d", i), "{"+strings.Join(parts, ",")+"}")
	}

	device := typ("openrtb_ext.ExtDevice")
	for vi, v := range attsValues {
		add(device, fmt.Sprintf("atts %d", vi), `{"atts":`+v+`}`)
		add(device, fmt.Sprintf("atts with prebid %d", vi),
			`{"prebid":{"interstitial":{"minwidthperc":1,"minheightperc":2}},"atts":`+v+`}`)
	}

	dealTier := typ("openrtb_ext.DealTier")
	prefixes := []string{``, `"prefix":""`, `"prefix":"p"`, `"prefix":null`, `"prefix":1`, `"PREFIX":"p"`}
	minimums := []string{
		``, `"minDealTier":0`, `"minDealTier":5`, `"minDealTier":-1`, `"minDealTier":"5"`,
		`"minDealTier":5.5`, `"minDealTier":null`, `"mindealtier":5`, `"minDealTier":99999999999999999999`,
	}
	for pi, p := range prefixes {
		for mi, m := range minimums {
			add(dealTier, fmt.Sprintf("deal tier %d.%d", pi, mi), object(p, m))
		}
	}

	prebid := typ("openrtb_ext.ExtRequestPrebid")
	for i, doc := range multiBidDocs(rng, 60) {
		add(prebid, fmt.Sprintf("multibid %d", i), doc)
	}

	// A method decodes into the value an earlier member left: the second of two members gets
	// what the first one decoded (and keeps it where the method fails or assigns nothing).
	for _, d := range []struct {
		typ, key string
		vals     []string
	}{
		{"openrtb_ext.ExtRequestTargeting", "pricegranularity", priceGranularityValues()},
		{"openrtb_ext.MediaTypePriceGranularity", "banner", priceGranularityValues()},
		{"openrtb_ext.ExtDevicePrebid", "interstitial", interstitials[:60]},
		{"openrtb_ext.ExtImpAppnexus", "keywords", keywordValues(rng)},
	} {
		t := typ(d.typ)
		key := strconv.Quote(d.key)
		for fi, first := range d.vals[:2] {
			for vi, v := range d.vals {
				add(t, fmt.Sprintf("twice %s %d.%d", d.key, fi, vi), "{"+key+":"+first+","+key+":"+v+"}")
			}
		}
	}
	for vi, v := range stringOrArrayValues(targeting.Type.Elem()) {
		add(goldbach, fmt.Sprintf("twice customTargeting %d", vi), `{"customTargeting":{"k":["a"],"k":`+v+`}}`)
	}

	// Random documents of every ported type, and many of the types prebid-server decodes
	// whole requests and responses into; one in six with its syntax broken somewhere.
	fuzz := &splitMix{state: *reviewSeed}
	random := func(t reflect.Type, label string, depth int) {
		doc := fuzzValue(t, fuzz, depth)
		if fuzz.intn(6) == 0 {
			doc = corrupt(doc, fuzz)
		}
		add(t, label, doc)
	}
	for _, t := range decodable {
		for i := 0; i < 3**reviewScale; i++ {
			random(t, fmt.Sprintf("random %d", i), 4)
		}
	}
	for _, name := range []string{
		"openrtb_ext.ExtRequest", "openrtb_ext.ExtRequestPrebid", "openrtb_ext.ExtImpPrebid",
		"openrtb_ext.ExtImp", "openrtb_ext.ExtDevice", "openrtb_ext.ExtUser", "openrtb_ext.ExtRegs",
		"openrtb_ext.ExtSource", "openrtb_ext.ExtApp", "openrtb_ext.ExtBidResponse",
		"openrtb_ext.ExtBidPrebid", "openrtb_ext.ExtBid", "openrtb_ext.PriceFloorRules",
		"openrtb_ext.ExtRequestTargeting", "openrtb_ext.ExtImpAppnexus", "openrtb_ext.ExtImpRubicon",
		"openrtb_ext.ImpExtGoldbach", "openrtb_ext.ExtImpPubmatic", "openrtb_ext.ExtAlternateBidderCodes",
	} {
		t := typ(name)
		for i := 0; i < 40**reviewScale; i++ {
			random(t, fmt.Sprintf("random deep %d", i), 5)
		}
	}
	return out
}

// The size and seed of review.tsv's random documents; a local campaign runs the generator with
// more and other ones (and the parity test on what it writes).
var (
	reviewScale = flag.Int("review-scale", 1, "how many times as many random documents review.tsv gets")
	reviewSeed  = flag.Uint64("review-seed", 0x5eed, "the seed of review.tsv's random documents")
)

// corrupt breaks a document's syntax at a random place: cut short, a comma added, the last
// closing bracket dropped, or a stray character added.
func corrupt(doc string, rng *splitMix) string {
	at := rng.intn(len(doc) + 1)
	switch rng.intn(4) {
	case 0:
		return doc[:at]
	case 1:
		return doc[:at] + "," + doc[at:]
	case 2:
		if i := strings.LastIndexAny(doc, "]}"); i >= 0 {
			return doc[:i] + doc[i+1:]
		}
		return doc + "}"
	default:
		return doc[:at] + rng.pick([]string{"x", ":", `"`, "{", "]", " ", "\\", "0", "-"}) + doc[at:]
	}
}

// Values the random documents draw from.
var (
	fuzzWrong   = []string{`null`, `1`, `-1.5`, `"x"`, `[]`, `{}`, `true`, `[1,"a"]`, `{"a":1}`}
	fuzzAny     = []string{`null`, `1`, `"s"`, `[]`, `{}`, `true`, `{"a":[1,{"b":null}]}`, `[1,"x",null]`, `1e400`, `-0`, escapes(`"%u00e9"`)}
	fuzzStrings = []string{`""`, `"x"`, `"pubmatic"`, `"<a>&"`, escapes(`"%u00e9%ud83d%ude00"`), `"tab\tnl\n"`, escapes(`"%u2028"`), `"1"`, `"true"`, `"USD"`}
	fuzzKeys    = []string{`"a"`, `"appnexus"`, `""`, `"APPNEXUS"`, `"b"`, escapes(`"%u0061"`), `"k k"`, `"<&>"`}
	fuzzFloats  = []string{`0`, `1`, `-1`, `1.5`, `0.1`, `1e20`, `1e21`, `1e-7`, `5e-324`, `1.7976931348623157e308`, `1e309`, `100`, `-0`, `3.4028235e38`, `3.5e38`, `1E+2`}
	fuzzBools   = []string{`true`, `false`}
)

// fuzzInts are integer values for a Go integer type of the bits, signed or not: small values,
// the type's bounds and the values just past them, and values that are not integers.
func fuzzInts(bits int, signed bool) []string {
	out := []string{`0`, `1`, `2`, `-1`, `100`, `1.0`, `1e2`, `1.5`, `-0`}
	if signed {
		max := int64(math.MaxInt64 >> (64 - bits))
		above := strconv.FormatUint(uint64(max)+1, 10)
		return append(out, strconv.FormatInt(max, 10), above, "-"+above, "-"+plusOne(above))
	}
	max := uint64(math.MaxUint64 >> (64 - bits))
	return append(out, strconv.FormatUint(max, 10), plusOne(strconv.FormatUint(max, 10)))
}

// fuzzValue is a random JSON value for Go type t, mostly of the type's own shape: a type with
// an UnmarshalJSON method gets one of the values the method tells apart, and now and then a
// value is null or of another JSON type, or an object has a re-cased, repeated or unknown key.
// Objects and arrays below depth are empty.
func fuzzValue(t reflect.Type, rng *splitMix, depth int) string {
	if v, ok := valuesOf(t); ok {
		return rng.pick(allValues(v))
	}
	switch t {
	case reflect.TypeOf(jsonutil.StringInt(0)), reflect.TypeOf(jsonutil.IntString("")), numberType:
		return rng.pick(numberEdges)
	case rawMessageType:
		return rng.pick(fuzzAny)
	}
	if rng.intn(30) == 0 {
		return rng.pick(fuzzWrong)
	}
	switch t.Kind() {
	case reflect.Ptr:
		if rng.intn(6) == 0 {
			return `null`
		}
		return fuzzValue(t.Elem(), rng, depth)
	case reflect.Interface:
		return rng.pick(fuzzAny)
	case reflect.Struct:
		if depth <= 0 {
			return `{}`
		}
		fields := jsonFields(t, nil)
		var members []string
		for _, f := range fields {
			if rng.intn(len(fields)) >= 4 {
				continue
			}
			key := f.key
			switch rng.intn(25) {
			case 0:
				key = strings.ToUpper(key)
			case 1:
				key = titleCase(key)
			}
			members = append(members, strconv.Quote(key)+":"+fuzzValue(f.typ, rng, depth-1))
			if rng.intn(25) == 0 {
				members = append(members, strconv.Quote(f.key)+":"+fuzzValue(f.typ, rng, depth-1))
			}
		}
		if rng.intn(8) == 0 {
			members = append(members, `"zz":`+rng.pick(fuzzAny))
		}
		for i := len(members) - 1; i > 0; i-- {
			j := rng.intn(i + 1)
			members[i], members[j] = members[j], members[i]
		}
		return "{" + strings.Join(members, ",") + "}"
	case reflect.Slice, reflect.Array:
		if rng.intn(8) == 0 {
			return `null`
		}
		var elements []string
		if depth > 0 {
			for n := rng.intn(4); n > 0; n-- {
				elements = append(elements, fuzzValue(t.Elem(), rng, depth-1))
			}
		}
		return "[" + strings.Join(elements, ",") + "]"
	case reflect.Map:
		if rng.intn(8) == 0 {
			return `null`
		}
		var members []string
		if depth > 0 {
			for n := rng.intn(4); n > 0; n-- {
				members = append(members, rng.pick(fuzzKeys)+":"+fuzzValue(t.Elem(), rng, depth-1))
			}
		}
		return "{" + strings.Join(members, ",") + "}"
	case reflect.String:
		return rng.pick(fuzzStrings)
	case reflect.Bool:
		return rng.pick(fuzzBools)
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return rng.pick(fuzzInts(t.Bits(), true))
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return rng.pick(fuzzInts(t.Bits(), false))
	case reflect.Float32, reflect.Float64:
		return rng.pick(fuzzFloats)
	default:
		log.Fatalf("review: no random values for %v", t)
		return ""
	}
}

// attsValues are device.ext.atts values.
var attsValues = []string{
	`-1`, `0`, `1`, `2`, `3`, `4`, `5`, `-0`, `00`, `01`, `3.0`, `3e0`, `3E0`, `+3`, `"3"`, `null`,
	`true`, `false`, `[]`, `{}`, `[3]`, `9223372036854775807`, `9223372036854775808`,
	`-9223372036854775808`, `-9223372036854775809`, `18446744073709551615`, `18446744073709551616`,
	`1e400`, `-`, `3a`, `0x3`, `.3`, `3.`, `3.5`, `""`,
}

// attsForms are device.ext documents for ParseDeviceExtATTS, with "@" for the atts value.
var attsForms = []string{
	`{"atts":@}`, `{"atts": @ }`, `{ "atts" : @ }`, `{"x":1,"atts":@}`, `{"atts":@,"x":1}`,
	`{"a":{"atts":9},"atts":@}`, `{"a":[{"atts":9}],"atts":@}`, escapes(`{"%u0061tts":@}`),
	`{"atts":@,"atts":4}`, `{"atts":@`, `{"atts":@,}`, `{"b":"\"atts\":9","atts":@}`,
	`{"atts":@}x`, `[{"atts":@}]`,
}

// multiBidDocs are request.ext.prebid documents of sampled multibid entries (never a null one:
// Go panics on it).
func multiBidDocs(rng *splitMix, n int) []string {
	bidder := []string{``, `"bidder":""`, `"bidder":"a"`, `"bidder":"b"`, `"bidder":"A"`, `"bidder":null`}
	bidders := []string{
		``, `"bidders":null`, `"bidders":[]`, `"bidders":["a"]`, `"bidders":["b","a"]`,
		`"bidders":["a","a"]`, `"bidders":["c",""]`, `"bidders":["A"]`,
	}
	maxBids := []string{
		``, `"maxbids":null`, `"maxbids":-1`, `"maxbids":0`, `"maxbids":1`, `"maxbids":5`,
		`"maxbids":9`, `"maxbids":10`, `"maxbids":100`, `"maxbids":-9223372036854775808`,
	}
	prefix := []string{``, `"targetbiddercodeprefix":""`, `"targetbiddercodeprefix":"p"`}
	var out []string
	for i := 0; i < n; i++ {
		var entries []string
		for k := 1 + rng.intn(4); k > 0; k-- {
			entries = append(entries, object(rng.pick(bidder), rng.pick(bidders), rng.pick(maxBids), rng.pick(prefix)))
		}
		out = append(out, `{"multibid":[`+strings.Join(entries, ",")+`]}`)
	}
	return out
}

// foldVariants are spellings of name that Go's case mapping and folding treat specially: upper
// and title case, and each "k", "s" and "i" as the Kelvin sign, the long s, and the dotted
// capital and dotless small i (strings.ToLower maps the Kelvin sign to "k" and the dotted
// capital I to "i"; strings.EqualFold folds "k" with the Kelvin sign and "s" with the long s).
func foldVariants(name string) []string {
	upper := strings.ToUpper(name)
	out := []string{name, upper, titleCase(name)}
	for _, sub := range [][3]string{
		{"k", "K", "\u212a"}, {"s", "S", "\u017f"}, {"i", "I", "\u0130"}, {"i", "I", "\u0131"},
	} {
		if strings.Contains(name, sub[0]) {
			out = append(out, strings.ReplaceAll(name, sub[0], sub[2]))
			out = append(out, strings.Replace(name, sub[0], sub[2], 1))
		}
		if strings.Contains(upper, sub[1]) {
			out = append(out, strings.ReplaceAll(upper, sub[1], sub[2]))
		}
	}
	seen := map[string]bool{}
	var unique []string
	for _, s := range out {
		if !seen[s] {
			seen[s] = true
			unique = append(unique, s)
		}
	}
	return unique
}

// unicodeProbes are names of characters whose Go case mapping differs from a full Unicode one,
// or that only Go's tables (Unicode 15) or a newer Unicode map.
var unicodeProbes = []string{
	"\u212a", "\u212aargo", "\u212aARGO", "\u017fovrn", "\u017f", "\u0130x", "\u0131x",
	"\u0130X", "\u0131X", "\u1e9e", "\u00df", "\u03a3", "\u03c2", "\u01c5", "\u01c4", "\u01c6",
	"\u2126", "\u212b", "\u13a0", "\uab70", "\ua7cb", "\u0264", "\U00010400", "\U00010428",
	"\u1f88", "\u03f4", "\uff21PPNEXUS", "A\u0301", "app\u200dnexus", "appnexus\u0000",
	"APPNEXUS\u0307", "\u0130\u0307x", "i\u0307x", "\u212a\u212a", "\u017f\u212aadn",
	"\u017fKADN", "S\u212aADN", "ig\u017f", "IG\u017f", "\u0130gs", "t\u0130d", "T\u0131D",
	"gp\u0130d", "prebi\u0111", "\u0391E", "\u0430e",
}

// reviewFunctionCases are review_functions.tsv's calls.
func reviewFunctionCases() []call {
	var calls []call
	add := func(name, label string, args []string, f func() []string) {
		calls = append(calls, call{name: name, label: label, args: args, results: protect(f)})
	}
	rng := &splitMix{state: 0xfeedface}

	normalize := func(name string) {
		add("NormalizeBidderName", "", []string{name}, func() []string {
			n, ok := openrtb_ext.NormalizeBidderName(name)
			return []string{string(n), strconv.FormatBool(ok)}
		})
	}
	every := func(name string) {
		add("IsBidderNameReserved", "", []string{name}, func() []string {
			return []string{strconv.FormatBool(openrtb_ext.IsBidderNameReserved(name))}
		})
		add("IsPotentialBidder", "", []string{name}, func() []string {
			return []string{strconv.FormatBool(openrtb_ext.IsPotentialBidder(name))}
		})
		normalize(name)
		add("NormalizeBidderNameOrUnchanged", "", []string{name}, func() []string {
			return []string{string(openrtb_ext.NormalizeBidderNameOrUnchanged(name))}
		})
	}
	reserved := []string{"all", "context", "data", "general", "gpid", "skadn", "prebid", "tid", "ae", "igs"}
	for _, r := range reserved {
		for _, v := range append(foldVariants(r), r+" ", " "+r, r+"\x00", r+"s", r[1:]) {
			every(v)
		}
	}
	for _, p := range unicodeProbes {
		every(p)
	}
	for _, name := range openrtb_ext.CoreBidderNames() {
		for _, v := range foldVariants(string(name)) {
			normalize(v)
		}
	}

	for fi, form := range attsForms {
		for _, v := range attsValues {
			ext := nest(form, v)
			add("ParseDeviceExtATTS", strconv.Itoa(fi), []string{ext}, func() []string {
				status, err := openrtb_ext.ParseDeviceExtATTS(json.RawMessage(ext))
				s := nilArg
				if status != nil {
					s = strconv.Itoa(int(*status))
				}
				return append([]string{s}, errResults(err)...)
			})
		}
	}
	for _, ext := range []string{` `, `null`, `true`, `"atts"`, `[]`, `{}`, `{"atts"}`, `{"atts":}`, `{atts:3}`, "{\"atts\":3}\x00"} {
		add("ParseDeviceExtATTS", "", []string{ext}, func() []string {
			status, err := openrtb_ext.ParseDeviceExtATTS(json.RawMessage(ext))
			s := nilArg
			if status != nil {
				s = strconv.Itoa(int(*status))
			}
			return append([]string{s}, errResults(err)...)
		})
	}

	for _, doc := range multiBidDocs(rng, 250) {
		add("ValidateAndBuildExtMultiBid", "", []string{doc}, func() []string {
			prebid := &openrtb_ext.ExtRequestPrebid{}
			decode(doc, prebid)
			validated, errs := openrtb_ext.ValidateAndBuildExtMultiBid(prebid)
			out := []string{marshal(validated), marshal(prebid.MultiBid)}
			for _, err := range errs {
				out = append(out, errResults(err)...)
			}
			return out
		})
	}
	for _, doc := range multiBidDocs(rng, 60) {
		entries := strings.TrimSuffix(strings.TrimPrefix(doc, `{"multibid":[`), `]}`)
		first := entries
		if i := strings.Index(entries, "},{"); i >= 0 {
			first = entries[:i+1]
		}
		add("ExtMultiBid.String", "", []string{first}, func() []string {
			var mb openrtb_ext.ExtMultiBid
			decode(first, &mb)
			return []string{mb.String()}
		})
	}

	for _, ext := range dealTierDocs(rng) {
		add("ReadDealTiersFromImp", "", []string{ext}, func() []string {
			tiers, err := openrtb_ext.ReadDealTiersFromImp(openrtb2.Imp{Ext: json.RawMessage(ext)})
			return append([]string{marshal(tiers)}, errResults(err)...)
		})
	}

	for _, doc := range alternateCodeDocs() {
		for _, p := range [][2]string{
			{"pubmatic", "pubmatic"}, {"pubmatic", "PUBMATIC"}, {"kargo", "\u212aargo"},
			{"\u212aargo", "KARGO"}, {"sovrn", "\u017fovrn"}, {"ix", "\u0130x"}, {"ix", "\u0131x"},
			{"pubmatic", "groupm"}, {"PUBMATIC", "groupm"}, {"kargo", "groupm"},
			{"\u212aargo", "groupm"}, {"\u017fovrn", "groupm"}, {"sovrn", "groupm"},
			{"SOVRN", "groupm"}, {"ix", "groupm"}, {"\u0130x", "groupm"}, {"IX", "groupm"},
			{"pubmatic", "GROUPM"}, {"pubmatic", "\u017f"}, {"pubmatic", "s"}, {"pubmatic", "*"},
		} {
			add("ExtAlternateBidderCodes.IsValidBidderCode", "", []string{doc, p[0], p[1]}, func() []string {
				var c openrtb_ext.ExtAlternateBidderCodes
				decode(doc, &c)
				valid, err := c.IsValidBidderCode(p[0], p[1])
				return append([]string{strconv.FormatBool(valid)}, errResults(err)...)
			})
		}
		for _, bidder := range []string{
			"pubmatic", "PUBMATIC", "PubMatic", "kargo", "KARGO", "\u212aargo", "sovrn",
			"\u017fovrn", "SOVRN", "ix", "\u0130x", "IX", "\u0131x", "groupm", "",
		} {
			add("ExtAlternateBidderCodes.IsBidderInAlternateBidderCodes", "", []string{doc, bidder}, func() []string {
				c := &openrtb_ext.ExtAlternateBidderCodes{}
				decode(doc, c)
				cfg, found := c.IsBidderInAlternateBidderCodes(bidder)
				return []string{marshal(cfg), strconv.FormatBool(found)}
			})
		}
	}

	for _, doc := range floorsGrid() {
		add("PriceFloorRules.Get", "", []string{doc}, func() []string {
			rules := &openrtb_ext.PriceFloorRules{}
			decode(doc, rules)
			return []string{
				strconv.FormatBool(rules.GetEnforcePBS()),
				strconv.FormatBool(rules.GetFloorsSkippedFlag()),
				strconv.Itoa(rules.GetEnforceRate()),
				strconv.FormatBool(rules.GetEnforceDealsFlag()),
				strconv.FormatBool(rules.GetEnabled()),
			}
		})
		add("PriceFloorRules.DeepCopy", "", []string{doc}, func() []string {
			rules := &openrtb_ext.PriceFloorRules{}
			decode(doc, rules)
			return []string{marshal(rules.DeepCopy())}
		})
	}

	alphabet := []string{"0", "1", "2", "9", "~", "~", ".", ".", "+", "-", " ", "a", "_", "\u0661"}
	providers := []string{"1~", "~", "~~", "1~.", "1~..", "1~1.", "1~.1", "2~1.2~3", "1~-0", "1~+0", "1~00"}
	for i := 0; i < 250; i++ {
		var b strings.Builder
		for n := rng.intn(12); n > 0; n-- {
			b.WriteString(rng.pick(alphabet))
		}
		providers = append(providers, b.String())
	}
	providers = append(providers, "1~9223372036854775807.-9223372036854775808.9223372036854775808")
	for _, s := range providers {
		add("ParseConsentedProvidersString", "", []string{s}, func() []string {
			return []string{marshal(openrtb_ext.ParseConsentedProvidersString(s))}
		})
	}

	for i := 0; i < 120; i++ {
		a, b := cloneDocs[rng.intn(len(cloneDocs))], cloneDocs[rng.intn(len(cloneDocs))]
		doc := object(strings.TrimSuffix(strings.TrimPrefix(a, "{"), "}"), strings.TrimSuffix(strings.TrimPrefix(b, "{"), "}"))
		if strings.Contains(doc, "null]") {
			continue
		}
		add("ExtRequestPrebid.Clone", "", []string{doc}, func() []string {
			prebid := &openrtb_ext.ExtRequestPrebid{}
			decode(doc, prebid)
			return []string{marshal(prebid.Clone())}
		})
	}
	return calls
}

// dealTierDocs are imp.ext documents for ReadDealTiersFromImp: sampled bidder entries under
// distinct bidders (two spellings of one bidder in one document would be read in Go's random
// map order), in spellings that NormalizeBidderName may or may not find.
func dealTierDocs(rng *splitMix) []string {
	spellings := [][]string{
		{"appnexus", "APPNEXUS", "AppNexus", "appnexus "},
		{"rubicon", "RUBICON", "rub\u0130con", "rub\u0131con"},
		{"kargo", "\u212aargo", "\u212aARGO", "KARGO"},
		{"sovrn", "\u017fovrn", "SOVRN"},
		{"ix", "IX", "\u0130x", "\u0131x"},
		{"unknownbidder"}, {"all"}, {"context"}, {"prebid"},
	}
	tiers := []string{
		``, `"dealTier":null`, `"dealTier":{}`, `"dealTier":{"prefix":"p"}`, `"dealTier":{"minDealTier":5}`,
		`"dealTier":{"prefix":"p","minDealTier":5}`, `"dealTier":{"prefix":"","minDealTier":0}`,
		`"dealTier":{"prefix":null,"minDealTier":null}`, `"dealTier":{"minDealTier":-3,"prefix":"x"}`,
		`"dealTier":{"PREFIX":"p","MINDEALTIER":2}`, `"dealTier":{"prefix":"p","minDealTier":5,"zz":[1]}`,
		`"DEALTIER":{"prefix":"u","minDealTier":4}`, `"dealtier":{"prefix":"l","minDealTier":3}`,
	}
	wrong := []string{
		`"dealTier":{"minDealTier":5.0}`, `"dealTier":{"minDealTier":"5"}`, `"dealTier":{"prefix":1}`,
		`"dealTier":"x"`, `"dealTier":[]`, `"dealTier":true`,
	}
	others := []string{``, `"placementId":12345`, `"zz":{"dealTier":{"prefix":"n","minDealTier":9}}`}
	var out []string
	for i := 0; i < 220; i++ {
		var entries []string
		for _, b := range rng.intnPerm(len(spellings), 1+rng.intn(4)) {
			tier := rng.pick(tiers)
			if i%10 == 9 {
				tier = rng.pick(wrong)
			}
			entries = append(entries, strconv.Quote(rng.pick(spellings[b]))+":"+object(rng.pick(others), tier))
		}
		bidder := `"bidder":{` + strings.Join(entries, ",") + `}`
		out = append(out, object(`"prebid":`+object(rng.pick([]string{``, `"supportdeals":true`}), bidder), rng.pick([]string{``, `"tid":"1"`})))
	}
	return append(out,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"prefix":"p","minDealTier":1}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"prefix":"p","minDealTier":1}}}}}x`,
		`{"Prebid":{"Bidder":{"appnexus":{"DealTier":{"Prefix":"p","MinDealTier":1}}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"prefix":"p"}},"bidder":1}}}`,
		`{"prebid":[],"prebid":{"bidder":{"ix":{"dealTier":{"minDealTier":2}}}}}`,
	)
}

// intnPerm is k distinct numbers below n, in a random order.
func (s *splitMix) intnPerm(n, k int) []int {
	perm := make([]int, n)
	for i := range perm {
		perm[i] = i
	}
	for i := n - 1; i > 0; i-- {
		j := s.intn(i + 1)
		perm[i], perm[j] = perm[j], perm[i]
	}
	return perm[:min(k, n)]
}

// alternateCodeDocs are alternate bidder code documents, each with at most one entry that a
// bidder can match case-insensitively (Go tries several in its random map order).
func alternateCodeDocs() []string {
	var out []string
	for _, key := range []string{"pubmatic", "PubMatic", "\u212aargo", "kargo", "\u017fovrn", "\u0130x", "ix", "groupm"} {
		for _, entry := range []string{
			`{"enabled":true}`, `{"enabled":false}`, `{}`, `{"enabled":true,"allowedbiddercodes":null}`,
			`{"enabled":true,"allowedbiddercodes":[]}`, `{"enabled":true,"allowedbiddercodes":["*"]}`,
			`{"enabled":true,"allowedbiddercodes":["groupm"]}`, `{"enabled":true,"allowedbiddercodes":["GROUPM"]}`,
			`{"enabled":true,"allowedbiddercodes":["*","groupm"]}`, escapes(`{"enabled":true,"allowedbiddercodes":["%u017f"]}`),
		} {
			out = append(out, `{"enabled":true,"bidders":{`+strconv.Quote(key)+`:`+entry+`}}`)
		}
		out = append(out, `{"enabled":false,"bidders":{`+strconv.Quote(key)+`:{"enabled":true}}}`)
	}
	return append(out, `{"enabled":true,"bidders":{}}`, `{"enabled":true,"bidders":null}`, `{"enabled":true}`)
}

// floorsGrid are floors documents of every combination of the pointer flags and enforcement.
func floorsGrid() []string {
	flags := []string{``, `null`, `true`, `false`}
	member := func(key, value string) string {
		if value == "" {
			return ""
		}
		return strconv.Quote(key) + ":" + value
	}
	enforcements := []string{``, `"enforcement":null`, `"enforcement":{}`}
	for _, pbs := range flags {
		for _, deals := range flags {
			for _, rate := range []string{``, `0`, `100`} {
				enforcements = append(enforcements, `"enforcement":`+object(
					member("enforcepbs", pbs), member("floordeals", deals), member("enforcerate", rate)))
			}
		}
	}
	var out []string
	for _, enabled := range flags {
		for _, skipped := range flags {
			for _, e := range enforcements {
				out = append(out, object(member("enabled", enabled), member("skipped", skipped), e))
			}
		}
	}
	return out
}

// reviewAliasCases are review_aliases.tsv's calls, in order: aliases registered with
// SetAliasBidderName, each followed by the lookups it changes.
func reviewAliasCases() []call {
	var calls []call
	add := func(name, label string, args []string, f func() []string) {
		calls = append(calls, call{name: name, label: label, args: args, results: protect(f)})
	}
	register := func(alias, parent string) {
		add("SetAliasBidderName", "", []string{alias, parent}, func() []string {
			return errResults(openrtb_ext.SetAliasBidderName(alias, openrtb_ext.BidderName(parent)))
		})
	}
	probe := func(names ...string) {
		for _, name := range names {
			add("NormalizeBidderName", "", []string{name}, func() []string {
				n, ok := openrtb_ext.NormalizeBidderName(name)
				return []string{string(n), strconv.FormatBool(ok)}
			})
			add("NormalizeBidderNameOrUnchanged", "", []string{name}, func() []string {
				return []string{string(openrtb_ext.NormalizeBidderNameOrUnchanged(name))}
			})
			add("IsBidderNameReserved", "", []string{name}, func() []string {
				return []string{strconv.FormatBool(openrtb_ext.IsBidderNameReserved(name))}
			})
		}
	}
	state := func() {
		add("CoreBidderNames", "", nil, func() []string {
			var names []string
			for _, n := range openrtb_ext.CoreBidderNames() {
				names = append(names, string(n))
			}
			return []string{strings.Join(names, ",")}
		})
		add("BuildBidderStringSlice", "", nil, func() []string {
			return []string{strings.Join(openrtb_ext.BuildBidderStringSlice(), ",")}
		})
		add("BuildBidderMap", "", nil, func() []string {
			var entries []string
			for k, v := range openrtb_ext.BuildBidderMap() {
				entries = append(entries, k+"="+string(v))
			}
			sort.Strings(entries)
			return []string{strings.Join(entries, ",")}
		})
		add("BuildBidderNameHashSet", "", nil, func() []string {
			var names []string
			for k := range openrtb_ext.BuildBidderNameHashSet() {
				names = append(names, k)
			}
			sort.Strings(names)
			return []string{strings.Join(names, ",")}
		})
		add("GetAliasBidderToParent", "", nil, func() []string {
			return []string{marshal(openrtb_ext.GetAliasBidderToParent())}
		})
	}

	for _, alias := range []string{
		"\u017fkadn", "s\u212aadn", "SKADN", "ig\u017f", "IG\u017f", "\u212aontext", "All", "GENERAL",
	} {
		register(alias, "appnexus")
	}
	state()
	register("gp\u0130d", "appnexus")
	probe("gpid", "GPID", "gp\u0130d", "gp\u0131d", "GP\u0130D")
	register("\u212aargo2", "kargo")
	probe("kargo2", "KARGO2", "\u212aargo2", "\u212aARGO2", "kargo")
	register("AppNexus", "rubicon")
	probe("appnexus", "APPNEXUS", "AppNexus")
	register("appnexus", "appnexus")
	probe("appnexus")
	register("", "pubmatic")
	probe("", " ")
	register("ALL ", "pubmatic")
	probe("all ", "ALL ", "all")
	register("\u0130X", "ix")
	probe("ix", "IX", "\u0130x", "\u0131x")
	register("\u01c5ab", "appnexus")
	probe("\u01c4AB", "\u01c6ab", "\u01c5ab", "dzab", "\u01c4ab")
	register("\u03a3igma", "appnexus")
	probe("\u03c3igma", "\u03a3IGMA", "\u03c2igma")
	register("\u1e9eeta", "appnexus")
	probe("\u00dfeta", "\u1e9eETA", "sseta")
	register("\u2126mega", "appnexus")
	probe("\u03c9mega", "\u03a9mega", "\u2126MEGA")
	register("\u212bngstrom", "appnexus")
	probe("\u00e5ngstrom", "\u00c5ngstrom", "\u212bNGSTROM")
	register("\u13a0cherokee", "appnexus")
	probe("\uab70cherokee", "\u13a0CHEROKEE")
	register("\ua7cbnew", "appnexus")
	probe("\u0264new", "\ua7cbNEW", "\ua7cbnew")
	register("\U00010400deseret", "appnexus")
	probe("\U00010428deseret", "\U00010400DESERET")
	register("dup", "appnexus")
	register("dup", "rubicon")
	probe("dup", "DUP")
	register("Dup", "pubmatic")
	probe("dup", "Dup")
	state()

	for _, ext := range []string{
		`{"prebid":{"bidder":{"KARGO2":{"dealTier":{"prefix":"k","minDealTier":2}},"gpid":{"dealTier":{"prefix":"g","minDealTier":3}}}}}`,
		`{"prebid":{"bidder":{"":{"dealTier":{"prefix":"e","minDealTier":1}},"ALL ":{"dealTier":{"prefix":"a","minDealTier":1}}}}}`,
		escapes(`{"prebid":{"bidder":{"%u01c4AB":{"dealTier":{"prefix":"d","minDealTier":4}},"%u03a3IGMA":{"dealTier":{"prefix":"s","minDealTier":5}}}}}`),
		`{"prebid":{"bidder":{"APPNEXUS":{"dealTier":{"prefix":"x","minDealTier":6}}}}}`,
	} {
		add("ReadDealTiersFromImp", "", []string{ext}, func() []string {
			tiers, err := openrtb_ext.ReadDealTiersFromImp(openrtb2.Imp{Ext: json.RawMessage(ext)})
			return append([]string{marshal(tiers)}, errResults(err)...)
		})
	}
	return calls
}
