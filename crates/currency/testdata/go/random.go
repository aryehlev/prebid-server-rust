package main

import (
	"fmt"
	"hash/fnv"
	"math"
	"strconv"
	"strings"
	"time"

	pbscurrency "github.com/prebid/prebid-server/v4/currency"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// The random corpora below are generated from splitmix64, which the Rust tests
// (src/golden.rs) implement identically, so both sides rebuild the same inputs and
// random.golden only holds digests. Each batch line carries a digest of the inputs, which
// catches a generator that drifted, and a digest of Go's outcomes. Cases where Go's
// randomized map order allows several outcomes are listed one per line instead; the Rust
// result must be one of them.

const randomBatch = 500

// splitmix64 is the generator shared with the Rust tests.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *splitmix64) below(n int) int {
	return int(s.next() % uint64(n))
}

func (s *splitmix64) pick(options []string) string {
	return options[s.below(len(options))]
}

func (s *splitmix64) digits(n int) string {
	var sb strings.Builder
	for i := 0; i < n; i++ {
		sb.WriteByte(byte('0' + s.below(10)))
	}
	return sb.String()
}

// Codes used as table keys: canonical, lower and mixed case (never matched directly by
// GetRate, but scanned as intermediate pivots), XXX and XTS, and historic CLDR 32 codes.
var randomTableCodes = []string{
	"USD", "GBP", "EUR", "MXN", "JPY", "SEK", "NOK", "CHF", "XXX", "XTS", "XAU", "ADP", "DEM",
	"usd", "gbp", "Eur", "xts",
}

// Codes passed to GetRate, including malformed and unrecognized ones.
var randomQueryCodes = []string{
	"USD", "GBP", "EUR", "MXN", "JPY", "SEK", "NOK", "CHF", "XXX", "XTS", "XAU", "ADP", "DEM",
	"usd", "gbp", "eUR", "xxx", "xTs", "dem", "chf", "VES", "MRU", "FOO", "", "US", "USDX",
}

// Few codes, so that several pivots often hold both currencies of a query.
var randomPivotCodes = []string{"USD", "GBP", "EUR", "SEK", "usd"}

var randomPivotQueryCodes = []string{"USD", "GBP", "EUR", "SEK", "usd", "gbp"}

// Codes used as keys of request rates for ValidateCustomRates; an invalid one is picked for
// one key in 16.
var randomValidCodes = []string{"USD", "GBP", "EUR", "usd", "Mxn", "XXX", "xts", "ADP"}

var randomInvalidCodes = []string{"VES", "MRU", "FOO", "", "US", "USDX", "U$D", "123", "€"}

// Rates with exact edge values: zeros of both signs, subnormals, the largest float64, and
// values whose reciprocal rounds.
var randomRateValues = []string{
	"0", "-0", "1", "-1", "0.5", "3", "7", "0.1", "0.3", "1e-5", "5e-324", "1e-310",
	"2.2250738585072014e-308", "1e308", "1.7976931348623157e308", "0.77208", "1.2952",
	"10.23842", "123456789.123456789", "0.333333333333333314829616256247",
}

// randomDecimal returns a JSON number for a rate, from the edge values or digits that take
// both jsoniter float paths (up to 6 decimals is the fast path). Every draw is a statement
// of its own so that the Rust port draws in the same order.
func randomDecimal(s *splitmix64) string {
	switch s.below(4) {
	case 0:
		return s.pick(randomRateValues)
	case 1:
		lead := 1 + s.below(9)
		rest := s.digits(s.below(6))
		return strconv.Itoa(lead) + rest
	case 2:
		whole := s.below(10000)
		fraction := s.digits(1 + s.below(17))
		return strconv.Itoa(whole) + "." + fraction
	default:
		lead := 1 + s.below(9)
		fraction := s.digits(1 + s.below(12))
		sign := s.pick([]string{"", "-"})
		exponent := s.below(300)
		return strconv.Itoa(lead) + "." + fraction + "e" + sign + strconv.Itoa(exponent)
	}
}

// randomTable returns a currency file body whose keys come from code.
func randomTable(s *splitmix64, code func() string) string {
	switch s.below(16) {
	case 0:
		return `{}`
	case 1:
		return `{"conversions":null}`
	case 2:
		return `{"conversions":{}}`
	}
	entries := make([]string, 0, 6)
	for i := 1 + s.below(5); i > 0; i-- {
		from := code()
		if s.below(10) == 0 {
			entries = append(entries, `"`+from+`":null`)
			continue
		}
		inner := make([]string, 0, 5)
		for j := s.below(5); j > 0; j-- {
			to := code()
			rate := randomDecimal(s)
			inner = append(inner, `"`+to+`":`+rate)
		}
		entries = append(entries, `"`+from+`":{`+strings.Join(inner, ",")+`}`)
	}
	return `{"conversions":{` + strings.Join(entries, ",") + `}}`
}

func pickFrom(s *splitmix64, codes []string) func() string {
	return func() string { return s.pick(codes) }
}

func validationCode(s *splitmix64) func() string {
	return func() string {
		if s.below(16) == 0 {
			return s.pick(randomInvalidCodes)
		}
		return s.pick(randomValidCodes)
	}
}

// batchDigests accumulates the input and outcome digests of one batch.
type batchDigests struct {
	g        *golden
	kind     string
	inputs   []byte
	outcomes []byte
	cases    int
}

func (b *batchDigests) add(input, outcome string) {
	b.inputs = append(b.inputs, input+"\n"...)
	b.outcomes = append(b.outcomes, outcome+"\n"...)
	b.cases++
	if b.cases%randomBatch == 0 {
		b.g.line(b.kind+"-batch", fmt.Sprint(b.cases/randomBatch-1), fnv64a(b.inputs), fnv64a(b.outcomes))
		b.inputs, b.outcomes = nil, nil
	}
}

func fnv64a(data []byte) string {
	h := fnv.New64a()
	if _, err := h.Write(data); err != nil {
		panic(err)
	}
	return fmt.Sprintf("%016x", h.Sum64())
}

// outcome folds a candidate list into the digest stream, listing ambiguous cases separately.
func outcome(g *golden, kind string, index int, actual string, cands []string) string {
	if !contains(cands, actual) {
		panic(fmt.Sprintf("%s %d: Go returned %q, outside the candidates %q", kind, index, actual, cands))
	}
	if len(cands) == 1 {
		return cands[0]
	}
	g.line(kind+"-multi", fmt.Sprint(index), strings.Join(cands, "|"))
	return "multi"
}

func contains(options []string, s string) bool {
	for _, option := range options {
		if option == s {
			return true
		}
	}
	return false
}

// writeRandomRates checks Rates.GetRate and AggregateConversions.GetRate on random tables.
func writeRandomRates(g *golden, name string, seed uint64, tableCodes, queryCodes []string, cases int) {
	s := &splitmix64{state: seed}
	rates := &batchDigests{g: g, kind: name + "-rate"}
	aggs := &batchDigests{g: g, kind: name + "-agg"}
	for i := 0; i < cases; i++ {
		customJSON := randomTable(s, pickFrom(s, tableCodes))
		serverJSON := randomTable(s, pickFrom(s, tableCodes))
		from := s.pick(queryCodes)
		to := s.pick(queryCodes)
		input := customJSON + "\t" + serverJSON + "\t" + from + "\t" + to
		custom, server := loadTable(customJSON), loadTable(serverJSON)

		cands := rateCandidates(custom, from, to)
		rates.add(input, outcome(g, rates.kind, i, rateResult(custom.GetRate(from, to)), cands))

		aggCands := cands
		if len(cands) == 1 && strings.HasPrefix(cands[0], "err:Currency conversion rate not found") {
			aggCands = rateCandidates(server, from, to)
		}
		agg := pbscurrency.NewAggregateConversions(custom, server)
		aggs.add(input, outcome(g, aggs.kind, i, rateResult(agg.GetRate(from, to)), aggCands))
	}
}

// writeRandomValidation checks ValidateCustomRates on random request rates.
func writeRandomValidation(g *golden, cases int) {
	s := &splitmix64{state: 20240902}
	vals := &batchDigests{g: g, kind: "val"}
	for i := 0; i < cases; i++ {
		js := randomTable(s, validationCode(s))
		conversions := loadTable(js).Conversions
		err := pbscurrency.ValidateCustomRates(&openrtb_ext.ExtRequestCurrency{ConversionRates: conversions})
		actual := "ok"
		if err != nil {
			actual = "err:" + err.Error()
		}
		vals.add(js, outcome(g, "val", i, actual, validationCandidates(conversions)))
	}
}

// writeRandomStaleness checks RateConverter.checkStaleRates: whether the wall-clock time
// from the last update to now, as Go's Duration.Seconds() float, exceeds the threshold's.
// Time.Sub saturates beyond about 292 years, and the float comparison ignores nanoseconds
// once the durations are long.
func writeRandomStaleness(g *golden, cases int) {
	s := &splitmix64{state: 20240903}
	stale := &batchDigests{g: g, kind: "stale"}
	for i := 0; i < cases; i++ {
		lastSec := int64(s.next() % (1 << 34))
		lastNsec := int64(s.below(1e9))
		var nowSec, nowNsec, threshold int64
		switch s.below(4) {
		case 0:
			// Near the threshold, to the nanosecond.
			threshold = int64(s.next() % (1 << 62))
			delta := threshold + int64(s.below(2001)) - 1000
			nowSec, nowNsec = lastSec+delta/1e9, lastNsec+delta%1e9
		case 1:
			// Whole seconds, as stale_rates_seconds configures it.
			threshold = int64(s.below(1e6)) * int64(time.Second)
			delta := threshold + int64(s.below(3)) - 1
			nowSec, nowNsec = lastSec+delta/1e9, lastNsec+delta%1e9
		case 2:
			// Anywhere, including far enough apart to saturate Time.Sub.
			threshold = int64(s.next() >> 1)
			nowSec = int64(s.next() % (1 << 34))
			nowNsec = int64(s.below(1e9))
		default:
			// Thresholds near Go's longest duration.
			threshold = math.MaxInt64 - int64(s.next()%(1<<40))
			delta := math.MaxInt64 - int64(s.next()%(1<<40))
			nowSec, nowNsec = lastSec+delta/1e9, lastNsec+delta%1e9
		}
		lastUpdated := time.Unix(lastSec, lastNsec)
		now := time.Unix(nowSec, nowNsec)
		// Go: rate_converter.go checkStaleRates, for a positive threshold.
		delta := now.UTC().Sub(lastUpdated.UTC())
		isStale := threshold > 0 && delta.Seconds() > time.Duration(threshold).Seconds()
		input := fmt.Sprintf("%d.%09d\t%d.%09d\t%d", lastUpdated.Unix(), lastUpdated.Nanosecond(),
			now.Unix(), now.Nanosecond(), threshold)
		stale.add(input, fmt.Sprint(isStale))
	}
}

func writeRandom(dir string) {
	g := newGolden(dir, "random.golden")
	defer g.close()
	writeRandomRates(g, "mixed", 20240901, randomTableCodes, randomQueryCodes, 20*randomBatch)
	writeRandomRates(g, "pivot", 20240904, randomPivotCodes, randomPivotQueryCodes, 10*randomBatch)
	writeRandomValidation(g, 10*randomBatch)
	writeRandomStaleness(g, 20*randomBatch)
}
