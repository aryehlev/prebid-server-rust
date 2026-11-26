// Command review writes the break-it goldens of the review of the Rust pbs-bidder-params crate
// from github.com/xeipuuv/gojsonschema itself: two seeded sweeps over schemas an operator's
// bidder-params file could hold, compared by SHA-256 digests.
//
// Run it from the directory of this module (crates/bidder-params/testdata/go):
//
//	go run ./review ../golden
//
// It writes review_sweeps.jsonl, one line per sweep of 50,000 cases:
//
//   - limits: JSON numbers of at most 15 significant digits, and integers int64 or uint64
//     holds, as the minimum and maximum of a schema and as documents against type, minimum and
//     maximum: the texts that print a limit and the verdicts that compare two numbers;
//   - strings: enums and property names made of HTML characters, controls, line separators,
//     astral characters and lone surrogate escapes, each spelled raw or escaped at random: the
//     texts that print an enum's values, a property name or a context.
//
// A case's line holds its inputs and the outcome of each validation (the verdict and, for an
// invalid document, Go's error strings in Go's order); both sides hash the cases and the lines.
// Neither sweep reaches a ledgered difference: no number needs more precision than float64
// keeps, no document fails to decode, no lone surrogate escape precedes another escape, and every
// document has one member, so Go's map order does not change the errors.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"hash"
	"log"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf16"

	"github.com/xeipuuv/gojsonschema"
)

const (
	// sweepCount is the number of cases per sweep.
	sweepCount = 50000
	// sampleCount is the number of outcome lines the golden keeps in full, for diagnostics.
	sampleCount = 8
	// limitsSeed and stringsSeed start the two generators.
	limitsSeed  = 0x6c696d697473
	stringsSeed = 0x737472696e6773
	// outside is a document no generated enum holds: the pool has no x.
	outside = `"xxxxxxxxxxxxxxxxxxxx"`
)

// splitmix64 is the generator both sides run.
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

// digits is n random decimal digits, the first one not 0 when nonZeroFirst.
func (s *splitmix64) digits(n int, nonZeroFirst bool) string {
	var b strings.Builder
	for i := 0; i < n; i++ {
		if i == 0 && nonZeroFirst {
			b.WriteByte(byte('1' + s.below(9)))
		} else {
			b.WriteByte(byte('0' + s.below(10)))
		}
	}
	return b.String()
}

// wideNumbers are the numbers of more than 15 significant digits that serde_json holds exactly
// (int64 and uint64 integers), and spellings of zero and of round numbers.
var wideNumbers = []string{
	`0`, `-0`, `0.0`, `-0.0`, `0e5`, `1`, `-1`, `1e0`, `1E+2`, `1e-2`, `100`, `1e5`, `1e6`,
	`999999`, `1000000`, `0.0001`, `0.00001`, `1.5e-5`, `123456789012345`, `-123456789012345`,
	`9007199254740992`, `-9007199254740992`, `9223372036854775807`, `-9223372036854775808`,
	`18446744073709551615`, `1e20`, `1e21`, `1e-7`, `1.0`, `10.50`, `2.5e+3`,
}

var exponents = []string{"e", "E", "e+", "e-", "E+", "E-"}

// number is a JSON number of at most 15 significant digits or, when wide, one of wideNumbers.
func (s *splitmix64) number(wide bool) string {
	kinds := 3
	if wide {
		kinds = 4
	}
	switch s.below(kinds) {
	case 0:
		sign := ""
		if s.below(2) == 0 {
			sign = "-"
		}
		return sign + s.digits(1+s.below(15), true)
	case 1:
		return strconv.Itoa(s.below(2001) - 1000)
	case 2:
		sign := ""
		if s.below(2) == 0 {
			sign = "-"
		}
		total := 1 + s.below(15)
		point := s.below(total + 1)
		var text string
		if point == 0 {
			text = "0." + s.digits(total, false)
		} else {
			text = s.digits(point, true)
			if point < total {
				text += "." + s.digits(total-point, false)
			}
		}
		if s.below(2) == 0 {
			text += exponents[s.below(len(exponents))] + strconv.Itoa(s.below(21))
		}
		return sign + text
	default:
		return wideNumbers[s.below(len(wideNumbers))]
	}
}

// outcome is one validation, as both sides spell it: ok, invalid and the error strings in
// Go's order, load_error and the text, decode or panic.
func outcome(schema *gojsonschema.Schema, doc string) (o string) {
	defer func() {
		if r := recover(); r != nil {
			o = "panic " + fmt.Sprint(r)
		}
	}()
	res, err := schema.Validate(gojsonschema.NewStringLoader(doc))
	if err != nil {
		return "decode " + err.Error()
	}
	if res.Valid() {
		return "ok"
	}
	fields := []string{"invalid"}
	for _, e := range res.Errors() {
		fields = append(fields, e.String())
	}
	return strings.Join(fields, "\t")
}

// outcomes loads schema and validates each document, or gives the load error.
func outcomes(schema string, docs ...string) []string {
	loaded, err := gojsonschema.NewSchema(gojsonschema.NewStringLoader(schema))
	if err != nil {
		return []string{"load_error " + err.Error()}
	}
	var out []string
	for _, doc := range docs {
		out = append(out, outcome(loaded, doc))
	}
	return out
}

// sweepLine is one line of review_sweeps.jsonl.
type sweepLine struct {
	Sweep    string   `json:"sweep"`
	Seed     uint64   `json:"seed"`
	Count    int      `json:"count"`
	Inputs   string   `json:"inputs"`
	Outcomes string   `json:"outcomes"`
	Sample   []string `json:"sample"`
}

// sweep runs count cases of gen, which gives a case's inputs and its outcomes.
func sweep(name string, seed uint64, gen func(*splitmix64) ([]string, []string)) sweepLine {
	rng := splitmix64{state: seed}
	line := sweepLine{Sweep: name, Seed: seed, Count: sweepCount}
	var inputs, results hash.Hash = sha256.New(), sha256.New()
	for i := 0; i < sweepCount; i++ {
		in, out := gen(&rng)
		inputs.Write([]byte(strings.Join(in, "\t") + "\n"))
		text := strings.Join(append(in, out...), "\t") + "\n"
		results.Write([]byte(text))
		if len(line.Sample) < sampleCount {
			line.Sample = append(line.Sample, text)
		}
	}
	line.Inputs = hex.EncodeToString(inputs.Sum(nil))
	line.Outcomes = hex.EncodeToString(results.Sum(nil))
	return line
}

// limitsCase is a limit, a second limit and a document, and their outcomes.
func limitsCase(rng *splitmix64) ([]string, []string) {
	limit := rng.number(true)
	other := rng.number(false)
	doc := rng.number(false)
	var out []string
	out = append(out, outcomes(`{"maximum":`+limit+`}`, `1e300`, limit, doc)...)
	out = append(out, outcomes(`{"minimum":`+limit+`}`, `-1e300`, limit, doc)...)
	out = append(out, outcomes(`{"type":"string"}`, limit)...)
	out = append(out, outcomes(`{"type":"integer"}`, limit)...)
	out = append(out, outcomes(`{"maximum":`+limit+`,"minimum":`+other+`}`, doc)...)
	out = append(out, outcomes(`{"properties":{"n":{"minimum":`+other+`}}}`, `{"n":`+doc+`}`)...)
	return []string{limit, other, doc}, out
}

// lone marks the two lone surrogates of the pool, which are written as escapes.
const (
	loneHigh = 0xd800
	loneLow  = 0xdc00
)

// pool is the characters of the strings sweep.
var pool = []rune{
	'a', 'Z', '0', ' ', '.', '<', '>', '&', '"', '\\', '/', '\'', '{', '}',
	0x00, 0x01, 0x08, 0x09, 0x0a, 0x0c, 0x0d, 0x1f, 0x7f, 0x80, 0x9f, 0xe9,
	0x0301, 0x200b, 0x2028, 0x2029, 0xfeff, 0xfffd, 0x1f600, loneHigh, loneLow,
}

// escape is \u and the four hex digits of r, each in a random case.
func (s *splitmix64) escape(r rune) string {
	const lower, upper = "0123456789abcdef", "0123456789ABCDEF"
	out := []byte(`\u`)
	for shift := 12; shift >= 0; shift -= 4 {
		d := (r >> shift) & 0xf
		if s.below(2) == 0 {
			out = append(out, lower[d])
		} else {
			out = append(out, upper[d])
		}
	}
	return string(out)
}

// short is the two-character escape JSON has for r, if any.
func short(r rune) string {
	switch r {
	case '"':
		return `\"`
	case '\\':
		return `\\`
	case '\b':
		return `\b`
	case '\f':
		return `\f`
	case '\n':
		return `\n`
	case '\r':
		return `\r`
	case '\t':
		return `\t`
	}
	return ""
}

// char writes one character of the pool, raw or escaped.
func (s *splitmix64) char(b *strings.Builder) {
	r := pool[s.below(len(pool))]
	switch {
	case r == loneHigh || r == loneLow:
		// A q follows, so that the escape never precedes another (json.surrogate-pairing).
		b.WriteString(s.escape(r))
		b.WriteByte('q')
	case r > 0xffff:
		if s.below(2) == 0 {
			b.WriteRune(r)
		} else {
			hi, lo := utf16.EncodeRune(r)
			b.WriteString(s.escape(hi))
			b.WriteString(s.escape(lo))
		}
	case r == '"' || r == '\\' || r < 0x20:
		if sh := short(r); sh != "" && s.below(2) == 0 {
			b.WriteString(sh)
		} else {
			b.WriteString(s.escape(r))
		}
	case r == '/':
		switch s.below(3) {
		case 0:
			b.WriteByte('/')
		case 1:
			b.WriteString(`\/`)
		default:
			b.WriteString(s.escape(r))
		}
	default:
		if s.below(2) == 0 {
			b.WriteRune(r)
		} else {
			b.WriteString(s.escape(r))
		}
	}
}

// str is a JSON string of 0 to 6 characters of the pool.
func (s *splitmix64) str() string {
	var b strings.Builder
	b.WriteByte('"')
	for n := s.below(7); n > 0; n-- {
		s.char(&b)
	}
	b.WriteByte('"')
	return b.String()
}

// stringsCase is an enum of 1 to 4 strings and two property names, and their outcomes.
func stringsCase(rng *splitmix64) ([]string, []string) {
	values := make([]string, 1+rng.below(4))
	for i := range values {
		values[i] = rng.str()
	}
	key := rng.str()
	other := rng.str()
	enum := `{"enum":[` + strings.Join(values, ",") + `]}`
	var out []string
	out = append(out, outcomes(enum, outside, values[0], values[len(values)-1])...)
	object := `{"properties":{` + key + `:` + enum + `},"additionalProperties":false}`
	out = append(out, outcomes(object,
		`{`+key+`:`+outside+`}`,
		`{`+other+`:`+values[0]+`}`,
		`{`+key+`:`+values[len(values)-1]+`}`,
	)...)
	required := `{"required":[` + key + `],"properties":{` + other + `:{"type":"integer"}}}`
	out = append(out, outcomes(required, `{}`, `{`+other+`:`+values[0]+`}`)...)
	return []string{strings.Join(values, ","), key, other}, out
}

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: review <output directory>")
	}
	out := os.Args[1]
	if err := os.MkdirAll(out, 0o755); err != nil {
		log.Fatal(err)
	}
	var b strings.Builder
	for _, line := range []sweepLine{
		sweep("limits", limitsSeed, limitsCase),
		sweep("strings", stringsSeed, stringsCase),
	} {
		data, err := json.Marshal(line)
		if err != nil {
			log.Fatal(err)
		}
		b.Write(data)
		b.WriteByte('\n')
	}
	if err := os.WriteFile(filepath.Join(out, "review_sweeps.jsonl"), []byte(b.String()), 0o644); err != nil {
		log.Fatal(err)
	}
}
