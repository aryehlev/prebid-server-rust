package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// The seeded corpus of same_as_go_float_parse: 50,000 JSON numbers, prices, long mantissas,
// large and small exponents and subnormals among them. Both sides rebuild the inputs from
// splitmix64 (tests/json.rs implements it identically), so float_parse.json holds digests only:
// per batch, one of the inputs (which catches a generator that drifted) and one of the float64
// bits Go reads them as. The edges are listed with their bits.

const (
	floatSeed  = 0x5354_4143_4b46_5031 // "STACKFP1"
	floatCount = 50000
	floatBatch = 1000
)

// splitmix64 is the generator shared with the Rust test.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// below returns a number in [0, n).
func (s *splitmix64) below(n int) int {
	return int(s.next() % uint64(n))
}

// between returns a number in [lo, hi].
func (s *splitmix64) between(lo, hi int) int {
	return lo + s.below(hi-lo+1)
}

// digits returns n decimal digits; the first is not zero when lead is set.
func (s *splitmix64) digits(n int, lead bool) string {
	var sb strings.Builder
	for i := 0; i < n; i++ {
		if i == 0 && lead {
			sb.WriteByte(byte('1' + s.below(9)))
		} else {
			sb.WriteByte(byte('0' + s.below(10)))
		}
	}
	return sb.String()
}

// integer returns "0" or up to maxDigits digits without a leading zero.
func (s *splitmix64) integer(maxDigits int) string {
	n := s.below(maxDigits + 1)
	if n == 0 {
		return "0"
	}
	return s.digits(n, true)
}

// exponent returns an exponent part for a power of ten in [lo, hi], in every spelling JSON
// allows.
func (s *splitmix64) exponent(lo, hi int) string {
	e := s.between(lo, hi)
	marker := "e"
	if s.below(2) == 1 {
		marker = "E"
	}
	switch {
	case e < 0:
		return marker + "-" + strconv.Itoa(-e)
	case s.below(2) == 1:
		return marker + "+" + strconv.Itoa(e)
	default:
		return marker + strconv.Itoa(e)
	}
}

// number returns the next JSON number of the corpus. Every exponent keeps the value finite.
func (s *splitmix64) number() string {
	var body string
	switch s.below(7) {
	case 0: // a price: up to 4 integer digits and 1 to 6 decimals
		body = s.integer(4) + "." + s.digits(s.between(1, 6), false)
	case 1: // an integer of up to 25 digits
		body = s.digits(s.between(1, 25), true)
	case 2: // a long fraction
		body = s.integer(3) + "." + s.digits(s.between(10, 30), false)
	case 3: // scientific, up to 20 significant digits
		body = s.digits(1, true) + "." + s.digits(s.between(1, 19), false) + s.exponent(-330, 307)
	case 4: // 16 to 19 digits and a large exponent, where a fast path rounds twice
		body = s.digits(s.between(16, 19), true) + s.exponent(-340, 288)
	case 5: // subnormals and the normal boundary
		body = s.digits(1, true) + "." + s.digits(s.between(1, 20), false) + s.exponent(-324, -307)
	default: // small decimals with leading zeros
		body = "0." + strings.Repeat("0", s.below(11)) + s.digits(s.between(1, 20), true)
	}
	if s.below(4) == 0 {
		return "-" + body
	}
	return body
}

// goBits returns the bits of the float64 Go reads s as, after checking that
// strconv.ParseFloat, jsonutil.Unmarshal (jsoniter) and encoding/json agree on it.
func goBits(s string) uint64 {
	f, err := strconv.ParseFloat(s, 64)
	if err != nil {
		panic(fmt.Sprintf("strconv.ParseFloat(%q): %v", s, err))
	}
	var viaUtil, viaJSON float64
	if err := jsonutil.Unmarshal([]byte(s), &viaUtil); err != nil {
		panic(fmt.Sprintf("jsonutil.Unmarshal(%q): %v", s, err))
	}
	if err := json.Unmarshal([]byte(s), &viaJSON); err != nil {
		panic(fmt.Sprintf("json.Unmarshal(%q): %v", s, err))
	}
	bits := math.Float64bits(f)
	if math.Float64bits(viaUtil) != bits || math.Float64bits(viaJSON) != bits {
		panic(fmt.Sprintf("%q: strconv %x, jsoniter %x, encoding/json %x", s, bits,
			math.Float64bits(viaUtil), math.Float64bits(viaJSON)))
	}
	return bits
}

// floatEdges are hand-picked inputs: halfway and near-halfway cases, the subnormal and overflow
// boundaries, integers past 2^53 and 2^64, and prices that are not exact in binary.
var floatEdges = []string{
	"0", "-0", "0.0", "-0.0", "0.1", "0.2", "0.3", "0.30000000000000004", "1.15", "2.675",
	"1.005", "0.015", "2.5", "0.000001", "1e-7", "1e23", "8.98846567431158e307",
	"1.7976931348623157e308", "1.7976931348623158e308", "2.2250738585072011e-308",
	"2.2250738585072012e-308", "2.2250738585072014e-308", "4.9e-324", "5e-324",
	"2.4703282292062327e-324", "2.4703282292062328e-324", "1e-400", "9007199254740993",
	"9007199254740995", "18446744073709551615", "18446744073709551616",
	"123456789012345678901234567890",
	"1.00000000000000011102230246251565404236316680908203125",
	"1.00000000000000011102230246251565404236316680908203126",
	"7.038531e-26", "3.0e-1", "1E2", "1e+2", "100e-2", "0.1e1",
}

type floatBatchDigest struct {
	Inputs string `json:"inputs_sha256"`
	Bits   string `json:"bits_sha256"`
}

type floatEdge struct {
	Input string `json:"input"`
	Bits  string `json:"bits"`
}

type floatParseDoc struct {
	Header  header             `json:"header"`
	Seed    string             `json:"seed"`
	Count   int                `json:"count"`
	Batch   int                `json:"batch"`
	Batches []floatBatchDigest `json:"batches"`
	Edges   []floatEdge        `json:"edges"`
}

func floatParseGolden() floatParseDoc {
	doc := floatParseDoc{
		Header: newHeader("github.com/json-iterator/go"),
		Seed:   fmt.Sprintf("%#x", uint64(floatSeed)),
		Count:  floatCount,
		Batch:  floatBatch,
	}
	gen := splitmix64{state: floatSeed}
	for start := 0; start < floatCount; start += floatBatch {
		inputs := sha256.New()
		bits := sha256.New()
		for i := 0; i < floatBatch; i++ {
			s := gen.number()
			fmt.Fprintf(inputs, "%s\n", s)
			fmt.Fprintf(bits, "%016x\n", goBits(s))
		}
		doc.Batches = append(doc.Batches, floatBatchDigest{
			Inputs: hex.EncodeToString(inputs.Sum(nil)),
			Bits:   hex.EncodeToString(bits.Sum(nil)),
		})
	}
	for _, s := range floatEdges {
		doc.Edges = append(doc.Edges, floatEdge{Input: s, Bits: fmt.Sprintf("%016x", goBits(s))})
	}
	return doc
}
