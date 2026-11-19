package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"math"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Prebid Server registers its json.RawMessage encoder with jsoniter at startup (main.go:23-25),
// so jsonutil.Marshal compacts raw messages; this program does the same.
func init() {
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// written is what one Go writer made of a value: its bytes, and its error text if it failed.
type written struct {
	Out string `json:"out"`
	Err string `json:"err"`
}

func wrote(b []byte, err error) written {
	if err != nil {
		return written{Out: string(b), Err: err.Error()}
	}
	return written{Out: string(b)}
}

// encodeNoHTML writes v as the auction, amp and cookie_sync endpoints and the exchange write
// their documents (auction.go:383-384, amp_auction.go:407-408, cookie_sync.go:560-561,
// exchange.go:984-986): an encoding/json Encoder with SetEscapeHTML(false), which ends the
// document with a newline.
func encodeNoHTML(v any) written {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	err := enc.Encode(v)
	return wrote(buf.Bytes(), err)
}

// writers holds what each of the three ways Prebid Server writes JSON made of one value:
// jsonutil.Marshal (jsoniter's ConfigCompatibleWithStandardLibrary, util/jsonutil/jsonutil.go:159-167),
// encoding/json's Marshal (the adapters) and the endpoints' Encoder.
type writers struct {
	JSONUtil      written `json:"jsonutil_marshal"`
	JSONMarshal   written `json:"json_marshal"`
	EncoderNoHTML written `json:"encoder_no_html"`
}

func writeAll(v any) writers {
	return writers{
		JSONUtil:      wrote(jsonutil.Marshal(v)),
		JSONMarshal:   wrote(json.Marshal(v)),
		EncoderNoHTML: encodeNoHTML(v),
	}
}

type htmlEscapeCase struct {
	Input   string  `json:"input"`
	Writers writers `json:"writers"`
}

// floatCase is one float64, and the float32 nearest to it, through each writer.
type floatCase struct {
	Bits   string  `json:"bits"`
	F64    writers `json:"f64"`
	Bits32 string  `json:"bits32"`
	F32    writers `json:"f32"`
}

// valueNumber is a JSON number decoded into interface{} and written back, as ext blobs and
// other untyped documents go through Prebid Server.
type valueNumber struct {
	Input    string  `json:"input"`
	JSONUtil written `json:"jsonutil"`
	JSON     written `json:"json"`
}

// pricedBid holds a price, as openrtb2.Bid does.
type pricedBid struct {
	Price float64 `json:"price"`
}

type nonFiniteCase struct {
	Name  string  `json:"name"`
	Bare  writers `json:"bare"`
	Field writers `json:"field"`
}

// bid is the decode target of the key, duplicate and UTF-8 cases.
type bid struct {
	ID    string  `json:"id"`
	ImpID string  `json:"impid"`
	Price float64 `json:"price"`
}

// decodedBid is what one decoder made of a document: its error text, or the fields (the id as
// hex, since it may hold bytes that are not UTF-8).
type decodedBid struct {
	Err   string `json:"err"`
	IDHex string `json:"id_hex"`
	ImpID string `json:"impid"`
	Price string `json:"price"`
}

func decodeBid(decode func([]byte, any) error, input []byte) decodedBid {
	var b bid
	if err := decode(input, &b); err != nil {
		return decodedBid{Err: err.Error()}
	}
	return decodedBid{
		IDHex: hex.EncodeToString([]byte(b.ID)),
		ImpID: b.ImpID,
		Price: strconv.FormatFloat(b.Price, 'g', -1, 64),
	}
}

// bidDecodes holds what jsonutil.Unmarshal (util/jsonutil/jsonutil.go:136-144),
// jsonutil.UnmarshalValid (:148-155) and encoding/json's Unmarshal made of one document.
type bidDecodes struct {
	InputHex      string     `json:"input_hex"`
	JSONUtil      decodedBid `json:"jsonutil_unmarshal"`
	JSONUtilValid decodedBid `json:"jsonutil_unmarshal_valid"`
	JSON          decodedBid `json:"json_unmarshal"`
}

func decodeBids(input string) bidDecodes {
	b := []byte(input)
	return bidDecodes{
		InputHex:      hex.EncodeToString(b),
		JSONUtil:      decodeBid(jsonutil.Unmarshal, b),
		JSONUtilValid: decodeBid(jsonutil.UnmarshalValid, b),
		JSON:          decodeBid(json.Unmarshal, b),
	}
}

// nullTarget has a field of every non-pointer kind a null can be given to.
type nullTarget struct {
	ID    string            `json:"id"`
	Price float64           `json:"price"`
	W     int64             `json:"w"`
	OK    bool              `json:"ok"`
	Imp   []string          `json:"imp"`
	Ext   map[string]string `json:"ext"`
	Sub   struct {
		A string `json:"a"`
	} `json:"sub"`
}

// decodedNulls is what one decoder made of the null document: its error text, or the decoded
// struct written back with encoding/json.
type decodedNulls struct {
	Err   string `json:"err"`
	Value string `json:"value"`
}

func decodeNulls(decode func([]byte, any) error, input []byte) decodedNulls {
	var target nullTarget
	if err := decode(input, &target); err != nil {
		return decodedNulls{Err: err.Error()}
	}
	out, err := json.Marshal(target)
	if err != nil {
		panic(err)
	}
	return decodedNulls{Value: string(out)}
}

type nullDecodes struct {
	Input         string       `json:"input"`
	JSONUtil      decodedNulls `json:"jsonutil_unmarshal"`
	JSONUtilValid decodedNulls `json:"jsonutil_unmarshal_valid"`
	JSON          decodedNulls `json:"json_unmarshal"`
}

// rawBid carries a json.RawMessage, as the ext fields of the OpenRTB types do.
type rawBid struct {
	ID  string          `json:"id"`
	Ext json.RawMessage `json:"ext,omitempty"`
}

type rawCase struct {
	Ext     string  `json:"ext"`
	Valid   bool    `json:"valid"`
	Writers writers `json:"writers"`
}

// controlEscapeCase is a string of control characters (and DEL) through each writer.
type controlEscapeCase struct {
	InputHex string  `json:"input_hex"`
	Writers  writers `json:"writers"`
}

// nestedRaw holds a nested ext as the OpenRTB types' ext fields do, in a json.RawMessage.
type nestedRaw struct {
	ID  string          `json:"id"`
	Ext json.RawMessage `json:"ext"`
}

// nestedAny holds it as Prebid's untyped documents do, in an interface{}.
type nestedAny struct {
	ID  string `json:"id"`
	Ext any    `json:"ext"`
}

// nestedSkip has no ext field, so the decoder skips the nested value as an unknown key.
type nestedSkip struct {
	ID string `json:"id"`
}

// depthOutcome is what one decoder made of the nested document into each holder: its error
// text, or nothing.
type depthOutcome struct {
	RawErr  string `json:"raw_err"`
	AnyErr  string `json:"any_err"`
	SkipErr string `json:"skip_err"`
}

// decodeDepth decodes input into each holder with decode.
func decodeDepth(decode func([]byte, any) error, input []byte) depthOutcome {
	var out depthOutcome
	var raw nestedRaw
	if err := decode(input, &raw); err != nil {
		out.RawErr = err.Error()
	}
	var v nestedAny
	if err := decode(input, &v); err != nil {
		out.AnyErr = err.Error()
	}
	var skip nestedSkip
	if err := decode(input, &skip); err != nil {
		out.SkipErr = err.Error()
	}
	return out
}

// depthCase is what each decoder made of a document whose ext nests Arrays arrays inside the
// outer object.
type depthCase struct {
	Arrays        int          `json:"arrays"`
	JSONUtil      depthOutcome `json:"jsonutil_unmarshal"`
	JSONUtilValid depthOutcome `json:"jsonutil_unmarshal_valid"`
	JSON          depthOutcome `json:"json_unmarshal"`
}

// nestedDocument is {"id":"x","ext":[[...]]} with arrays levels of arrays.
func nestedDocument(arrays int) []byte {
	return []byte(`{"id":"x","ext":` + strings.Repeat("[", arrays) + strings.Repeat("]", arrays) + `}`)
}

type jsonDoc struct {
	Header         header            `json:"header"`
	HTMLEscape     htmlEscapeCase    `json:"html_escape"`
	FloatFormat    []floatCase       `json:"float_format"`
	ValueNumbers   []valueNumber     `json:"value_numbers"`
	NonFinite      []nonFiniteCase   `json:"non_finite"`
	KeyCase        bidDecodes        `json:"key_case"`
	DuplicateKeys  bidDecodes        `json:"duplicate_keys"`
	InvalidUTF8    bidDecodes        `json:"invalid_utf8"`
	NullIntoValue  nullDecodes       `json:"null_into_value"`
	RawMessage     []rawCase         `json:"raw_message"`
	LoneSurrogates []bidDecodes      `json:"lone_surrogates"`
	ControlEscape  controlEscapeCase `json:"control_escape"`
	Depth          []depthCase       `json:"depth"`
}

// loneSurrogateInputs are documents whose id holds \u escapes of UTF-16 surrogates that form no
// pair: a high surrogate before a letter, a low surrogate alone, and two high surrogates, as a
// string cut in the middle of an escaped emoji (a page title, a keyword) reaches the server.
var loneSurrogateInputs = []string{
	`{"id":"a\ud83db","impid":"y","price":1}`,
	`{"id":"a\ude00b","impid":"y","price":1}`,
	`{"id":"\ud83d\ud83d","impid":"y","price":1}`,
}

// depthArrays are nesting depths around serde_json's recursion limit (128 levels) and around
// jsoniter's and encoding/json's limit (10000 levels); the outer object adds one level.
var depthArrays = []int{126, 127, 9999, 10000}

// formatFloats are the floats of the float_format golden: integral values, the bounds of the
// plain form (1e-6 and 1e21) on both sides, -0, the extremes and ordinary prices.
var formatFloats = []float64{
	0, math.Copysign(0, -1), 1, 100, 300, 1.5, 0.1, 0.3, 3.14159, 1234567.125, 123456789,
	0.000001, 1e-7, 2.5e-8, 1e20, 1e21, 1.5e300, math.MaxFloat64, math.SmallestNonzeroFloat64,
}

// valueNumberInputs are JSON numbers read into interface{} and written back.
var valueNumberInputs = []string{
	"300", "300.0", "1E2", "1.5", "0.000001", "1e-7", "-0", "12345678901234567890", "1e21",
}

func jsonGolden() jsonDoc {
	doc := jsonDoc{Header: newHeader("github.com/json-iterator/go")}

	html := "<a href=\"x\">&</a>  "
	doc.HTMLEscape = htmlEscapeCase{Input: html, Writers: writeAll(html)}

	for _, f := range formatFloats {
		f32 := float32(f)
		doc.FloatFormat = append(doc.FloatFormat, floatCase{
			Bits:   strconv.FormatUint(math.Float64bits(f), 16),
			F64:    writeAll(f),
			Bits32: strconv.FormatUint(uint64(math.Float32bits(f32)), 16),
			F32:    writeAll(f32),
		})
	}

	for _, input := range valueNumberInputs {
		var viaUtil, viaJSON any
		vn := valueNumber{Input: input}
		if err := jsonutil.Unmarshal([]byte(input), &viaUtil); err != nil {
			vn.JSONUtil = written{Err: err.Error()}
		} else {
			vn.JSONUtil = wrote(jsonutil.Marshal(viaUtil))
		}
		if err := json.Unmarshal([]byte(input), &viaJSON); err != nil {
			vn.JSON = written{Err: err.Error()}
		} else {
			vn.JSON = wrote(json.Marshal(viaJSON))
		}
		doc.ValueNumbers = append(doc.ValueNumbers, vn)
	}

	for _, nf := range []struct {
		name string
		v    float64
	}{{"NaN", math.NaN()}, {"+Inf", math.Inf(1)}, {"-Inf", math.Inf(-1)}} {
		doc.NonFinite = append(doc.NonFinite, nonFiniteCase{
			Name:  nf.name,
			Bare:  writeAll(nf.v),
			Field: writeAll(pricedBid{Price: nf.v}),
		})
	}

	doc.KeyCase = decodeBids(`{"ID":"x","IMPID":"y","Price":1.5}`)
	doc.DuplicateKeys = decodeBids(`{"id":"a","id":"b","impid":"y","price":1}`)
	doc.InvalidUTF8 = decodeBids("{\"id\":\"a\xffb\",\"impid\":\"y\",\"price\":1}")

	nulls := `{"id":null,"price":null,"w":null,"ok":null,"imp":null,"ext":null,"sub":null}`
	doc.NullIntoValue = nullDecodes{
		Input:         nulls,
		JSONUtil:      decodeNulls(jsonutil.Unmarshal, []byte(nulls)),
		JSONUtilValid: decodeNulls(jsonutil.UnmarshalValid, []byte(nulls)),
		JSON:          decodeNulls(json.Unmarshal, []byte(nulls)),
	}

	for _, ext := range []string{`{ "a" : "<" }`, `{"a":1}`, `{"a":`} {
		doc.RawMessage = append(doc.RawMessage, rawCase{
			Ext:     ext,
			Valid:   json.Valid([]byte(ext)),
			Writers: writeAll(rawBid{ID: "1", Ext: json.RawMessage(ext)}),
		})
	}

	for _, input := range loneSurrogateInputs {
		doc.LoneSurrogates = append(doc.LoneSurrogates, decodeBids(input))
	}

	controls := "\x08\x0c\x00\x01\x1f\x7f"
	doc.ControlEscape = controlEscapeCase{
		InputHex: hex.EncodeToString([]byte(controls)),
		Writers:  writeAll(controls),
	}

	for _, arrays := range depthArrays {
		input := nestedDocument(arrays)
		doc.Depth = append(doc.Depth, depthCase{
			Arrays:        arrays,
			JSONUtil:      decodeDepth(jsonutil.Unmarshal, input),
			JSONUtilValid: decodeDepth(jsonutil.UnmarshalValid, input),
			JSON:          decodeDepth(json.Unmarshal, input),
		})
	}
	return doc
}
