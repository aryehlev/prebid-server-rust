// Command adversarial writes randomized differential goldens for the Rust pbs-bidder-params
// crate from the real Go code. Where testdata/go/golden enumerates hand-picked cases, this
// program generates many inputs from a fixed seed, aimed at the corners such a corpus misses:
//
//   - adversarial_regexp.jsonl: random Go regexp patterns (valid and invalid) and random
//     Unicode inputs: compile acceptance and MatchString results;
//   - adversarial_engine.jsonl: random schemas (every keyword gojsonschema knows, every draft)
//     and random documents, validated by gojsonschema itself;
//   - adversarial_bidders.jsonl: random documents shaped after every static/bidder-params
//     schema (odd numbers, escapes, duplicate keys, invalid UTF-8, truncation), validated by
//     openrtb_ext.NewBidderParamsValidator.
//
// Run it from the directory of this module (crates/bidder-params/testdata/go), with the Go
// version the server is built with (the Dockerfile's):
//
//	go run ./adversarial ../golden
//
// It reads static/bidder-params and static/bidder-info from the upstream checkout the module
// builds against (see enterUpstream).
//
// Go iterates maps in random order, so every document is validated by several independently
// loaded copies of its schema, several times each, and every distinct output is recorded.
package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"math/rand"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/xeipuuv/gojsonschema"
)

const (
	// seed makes the generated cases reproducible.
	seed = 20261117
	// copies is the number of independently loaded copies of every schema.
	copies = 8
	// repeats is the number of validations per copy.
	repeats = 2
	// schemaDir is the bidder params schema directory, relative to the upstream checkout.
	schemaDir = "static/bidder-params"
)

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: adversarial <output directory>")
	}

	out := enterUpstream(os.Args[1])
	if err := os.MkdirAll(out, 0o755); err != nil {
		log.Fatal(err)
	}
	rng := rand.New(rand.NewSource(seed))
	writeRegexpCases(filepath.Join(out, "adversarial_regexp.jsonl"), rng)
	writeEngineCases(filepath.Join(out, "adversarial_engine.jsonl"), rng)
	writeBidderCases(filepath.Join(out, "adversarial_bidders.jsonl"), rng)
}

// lineWriter writes one JSON value per line, without HTML escaping.
type lineWriter struct {
	file *os.File
	w    *bufio.Writer
}

func newLineWriter(path string) *lineWriter {
	file, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	return &lineWriter{file: file, w: bufio.NewWriter(file)}
}

func (lw *lineWriter) write(v interface{}) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		log.Fatal(err)
	}
	if _, err := lw.w.Write(buf.Bytes()); err != nil {
		log.Fatal(err)
	}
}

func (lw *lineWriter) close() {
	if err := lw.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := lw.file.Close(); err != nil {
		log.Fatal(err)
	}
}

// textFields stores bytes as text when they are UTF-8 and as hex otherwise.
func textFields(b []byte) (*string, string) {
	if utf8.Valid(b) {
		text := string(b)
		return &text, ""
	}
	return nil, hex.EncodeToString(b)
}

func pick(rng *rand.Rand, items []string) string {
	return items[rng.Intn(len(items))]
}

// ---------------------------------------------------------------------------------------------
// Regular expressions.

// regexpAtoms are pattern fragments: Go syntax that is valid, valid only in some contexts, or
// rejected, chosen where Go's regexp and Rust's regex crate are most likely to disagree.
var regexpAtoms = []string{
	"a", "b", "z", "A", "Z", "0", "9", "_", "-", ".", " ", "é", "ß", "K", "k", "K", "ſ",
	"s", "S", "日", "😀", "~", "/", "#", "&", "<", "'", "\"", "\xc2\xa0", "\n",
	`\d`, `\D`, `\w`, `\W`, `\s`, `\S`, `\b`, `\B`, `\A`, `\z`, `^`, `$`, `\.`, `\-`, `\\`,
	`\x41`, `\x{1F600}`, `\x{D800}`, `\x{DFFF}`, `\x{FFFD}`, `\x{110000}`, `\x{}`, `\x4`,
	`\x{10FFFF}`, `\101`, `\0`, `\012`, `\8`, `\1`, `\n`, `\t`, `\f`, `\v`, `\a`, `\r`, `\e`,
	`\_`, `\<`, `\Z`, `\C`, `\G`, `\K`, `\X`, `\R`, `\h`, `\N`, `\o`,
	`\pL`, `\PL`, `\pN`, `\pC`, `\pZ`, `\pM`, `\pP`, `\pS`, `\pl`, `\p{L}`, `\p{Lu}`, `\p{Ll}`,
	`\p{Nd}`, `\p{Greek}`, `\p{greek}`, `\p{GREEK}`, `\p{Old_Italic}`, `\p{OldItalic}`,
	`\p{Han}`, `\p{Latin}`, `\p{^Lu}`, `\P{^Lu}`, `\p{Any}`, `\p{any}`, `\p{Assigned}`,
	`\p{ASCII}`, `\p{ascii}`, `\p{Letter}`, `\p{Uppercase_Letter}`, `\p{uppercase letter}`,
	`\p{Alphabetic}`, `\p{LC}`, `\p{Lc}`, `\p{L&}`, `\p{Cs}`, `\p{Co}`, `\p{Cn}`, `\p{C}`,
	`\p{Common}`, `\p{Inherited}`, `\p{Zzzz}`, `\p{ Lu }`, `\p{l-u}`, `\p{}`, `\p`, `\pX`,
	`\p{Digit}`, `\p{Decimal_Number}`, `\p{Punct}`, `\p{White_Space}`, `\p{Emoji}`,
	`\p{Hangul}`, `\p{Cyrillic}`, `\p{Arabic}`, `\p{Garay}`, `\p{Kawi}`, `\p{sc=Greek}`,
	`\p{Script=Greek}`, `\p{gc=Lu}`, `\p{Lu`, `\p{^}`,
	`\Q.*\E`, `\Qa\E`, `\Q`, `\E`, `\Q\\E`,
	"[a-z]", "[^a-z]", "[0-9|a-z|A-Z]", `[a-fA-F\d\-]`, "[[:alpha:]]", "[[:^digit:]]",
	"[[:word:]]", "[[:foo:]]", "[[:space:]]", "[[:punct:]]", "[[:upper:]]", "[[:^upper:]]",
	"[[:ascii:]]", "[[:xdigit:]]", "[[:graph:]]", "[[:print:]]", "[[:cntrl:]]", "[[:blank:]]",
	"[[:lower:]]", "[[:alnum:]]", "[[:Alpha:]]", "[[:alpha:]", "[[:alpha]]", "[[.a.]]", "[[=a=]]",
	`[\pL\d]`, `[^\s]`, `[\x{D800}-\x{DFFF}]`, `[\x{D000}-\x{D800}]`, `[\x{DFFF}-\x{E001}]`,
	`[\x{D7FF}-\x{E000}]`, `[\x{D800}]`, `[^\x{D800}]`, "[]a]", "[^]a]", "[a-]", "[-a]",
	`[\w-z]`, "[z-a]", `[\b]`, "[.]", "[$^]", "[[]", "[a&&b]", "[~~]", "[--]", "[a-c-e]",
	`[\]]`, `[\[]`, "[^\n]", "[ -~]", `[\x00-\x{10FFFF}]`, `[^\x00-\x{10FFFF}]`, "[é-ü]",
	"[Α-Ω]", `[\d-]`, "[k]", "[^k]", `[\s\S]`, `[\p{Greek}\d]`, `[^\pL]`, `[\P{L}]`, "[]",
	"[^]", `[\Q]\E]`, `[a-\d]`, `[\d-z]`, `[\--\-]`, "[ſ]", "[K]", "[K]", `[\x{1F600}-\x{1F64F}]`,
	"(", ")", "[", "]", "{", "}", "*", "+", "?", "|", "{1}", "{,}", "x{", "x{1", "x{1,", "{a}",
}

// regexpGroups wrap a sub-pattern; "%s" is the sub-pattern.
var regexpGroups = []string{
	"(%s)", "(?:%s)", "(?i)%s", "(?i:%s)", "(?s:%s)", "(?s)%s", "(?m)%s", "(?m:%s)", "(?U)%s",
	"(?P<n>%s)", "(?<n>%s)", "(?P<n1>%s)(?P<n2>x)", "(?i-s:%s)", "(?-i)%s", "(?-i:%s)",
	"(?x)%s", "(?%s)", "(?i%s", "(?P<>%s)", "(?P<1a>%s)", "(?P<n>%s)(?P<n>x)", "(?P=n)%s",
	"(?#c)%s", "(?ims-U:%s)", "(?-)%s", "(?i-)%s", "(?--i)%s", "(?<=a)%s", "(?=a)%s",
	"(?!a)%s", "(?'n'%s)", "(?P<é>%s)", "(?P<n-1>%s)", "(%s", "%s)", "(?i)(?-i:%s)",
	"(?i)[%s]", "[^%s]", "%s(?i)", "(?:)%s", "()%s", "(|%s)",
}

// regexpRepeats follow a sub-pattern.
var regexpRepeats = []string{
	"*", "+", "?", "{2}", "{1,3}", "{2,}", "*?", "+?", "??", "{,3}", "{1001}", "{0}", "**",
	"{2}{3}", "{1000}", "{3,1}", "{01}", "{1,1001}", "{99999999999}", "{0,0}", "{1}?", "*+",
	"?*", "{2}*", "{1000,}", "{0,1000}", "{ 2}", "{2 }", "{1,2,3}", "{-1}", "{+1}",
}

func genPattern(rng *rand.Rand, depth int) string {
	r := rng.Intn(10)
	switch {
	case depth > 2 || r < 4:
		return pick(rng, regexpAtoms)
	case r < 6:
		n := 2 + rng.Intn(2)
		var b strings.Builder
		for i := 0; i < n; i++ {
			b.WriteString(genPattern(rng, depth+1))
		}
		return b.String()
	case r < 7:
		return genPattern(rng, depth+1) + "|" + genPattern(rng, depth+1)
	case r < 8:
		return fmt.Sprintf(pick(rng, regexpGroups), genPattern(rng, depth+1))
	default:
		return genPattern(rng, depth+1) + pick(rng, regexpRepeats)
	}
}

// interestingRunes are input characters where ASCII and Unicode semantics, case folding,
// Unicode versions or line handling differ.
var interestingRunes = []rune{
	'a', 'b', 'z', 'A', 'Z', 'k', 'K', 's', 'S', '0', '1', '9', '_', '-', '.', ' ', '\t', '\n',
	'\r', '\v', '\f', '~', '/', '|', '[', ']', '{', '}', '(', ')', '*', '\\', '^', '$', '#',
	0xe9, 0xc9, 0xdf, 0x1e9e, 0x17f, 0x212a, 0x3a9, 0x3c9, 0x3a3, 0x3c3, 0x3c2, 0x130, 0x131,
	0x1c5, 0xfb00, 0x65e5, 0x1f600, 0xa0, 0x2028, 0x85, 0x663, 0xff11, 0x301, 0x200d, 0xfeff,
	0xfffd, 0x378, 0xe000, 0xd7ff, 0x10ffff, 0x1e030, 0x1c89, 0x0, 0x7f, 0xad, 0x1c4, 0x1c6,
	0xb5, 0x39c, 0x3bc, 0xc5, 0xe5, 0x212b, 0x1f0, 0x1c80, 0x2c2f, 0x1e900, 0x10400, 0x2160,
}

// jsonEscapes turns the ^u placeholders of JSON text into backslash-u escapes.
func jsonEscapes(texts []string) []string {
	out := make([]string, len(texts))
	for i, t := range texts {
		out[i] = strings.ReplaceAll(t, "^u", `\`+"u")
	}
	return out
}

func patternRunes(pattern string) []rune {
	var runes []rune
	for _, r := range pattern {
		runes = append(runes, r)
	}
	return runes
}

func genInput(rng *rand.Rand, extra []rune) string {
	n := rng.Intn(7)
	var b strings.Builder
	for i := 0; i < n; i++ {
		if len(extra) > 0 && rng.Intn(2) == 0 {
			b.WriteRune(extra[rng.Intn(len(extra))])
		} else {
			b.WriteRune(interestingRunes[rng.Intn(len(interestingRunes))])
		}
	}
	return b.String()
}

// regexpLine is one line of adversarial_regexp.jsonl.
type regexpLine struct {
	Pattern string `json:"pattern"`
	// Valid is whether regexp.Compile accepts the pattern.
	Valid bool `json:"valid"`
	// Inputs and Matches ('0' or '1' per input) are the MatchString results.
	Inputs  []string `json:"inputs,omitempty"`
	Matches string   `json:"matches,omitempty"`
}

// bidderPatterns are the patterns static/bidder-params uses plus gojsonschema's own format
// patterns; they get many more inputs.
func bidderPatterns() []string {
	seen := map[string]bool{}
	var patterns []string
	files, err := filepath.Glob(filepath.Join(schemaDir, "*.json"))
	if err != nil {
		log.Fatal(err)
	}
	var walk func(v interface{})
	walk = func(v interface{}) {
		switch t := v.(type) {
		case map[string]interface{}:
			if p, ok := t["pattern"].(string); ok && !seen[p] {
				seen[p] = true
				patterns = append(patterns, p)
			}
			for _, child := range t {
				walk(child)
			}
		case []interface{}:
			for _, child := range t {
				walk(child)
			}
		}
	}
	for _, file := range files {
		data, err := os.ReadFile(file)
		if err != nil {
			log.Fatal(err)
		}
		var doc interface{}
		if err := json.Unmarshal(data, &doc); err != nil {
			log.Fatal(err)
		}
		walk(doc)
	}
	sort.Strings(patterns)
	return append(patterns,
		`^([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])(\.([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]{0,61}[a-zA-Z0-9]))*$`,
		"^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$",
		"^(?:/(?:[^~/]|~0|~1)*)*$",
		"^(?:0|[1-9][0-9]*)(?:#|(?:/(?:[^~/]|~0|~1)*)*)$",
	)
}

func writeRegexpCases(path string, rng *rand.Rand) {
	lw := newLineWriter(path)
	defer lw.close()
	emit := func(pattern string, inputs int) {
		line := regexpLine{Pattern: pattern}
		re, err := regexp.Compile(pattern)
		line.Valid = err == nil
		if re != nil {
			extra := patternRunes(pattern)
			var matches strings.Builder
			for i := 0; i < inputs; i++ {
				input := genInput(rng, extra)
				line.Inputs = append(line.Inputs, input)
				if re.MatchString(input) {
					matches.WriteByte('1')
				} else {
					matches.WriteByte('0')
				}
			}
			line.Matches = matches.String()
		}
		lw.write(line)
	}
	for _, pattern := range bidderPatterns() {
		emit(pattern, 400)
	}
	for _, atom := range regexpAtoms {
		emit(atom, 40)
		emit("^"+atom+"$", 40)
		emit("(?i)^"+atom+"$", 40)
	}
	for i := 0; i < 6000; i++ {
		emit(genPattern(rng, 0), 30)
	}
	// The cases below draw from their own source so that adding them leaves the random
	// streams of the other goldens unchanged.
	writeUnicodeClassCases(lw, rand.New(rand.NewSource(seed+1)))
	writeRepeatCases(lw)
}

// randomRune is a code point from anywhere in Unicode, half of them in the BMP and a quarter
// in the SMP, never a surrogate.
func randomRune(rng *rand.Rand) rune {
	for {
		var r rune
		switch rng.Intn(4) {
		case 0, 1:
			r = rune(rng.Intn(0x10000))
		case 2:
			r = 0x10000 + rune(rng.Intn(0x10000))
		default:
			r = rune(rng.Intn(unicode.MaxRune + 1))
		}
		if r < 0xd800 || r > 0xdfff {
			return r
		}
	}
}

// writeUnicodeClassCases checks every class name Go's unicode package has (categories,
// scripts, aliases), in several spellings, plus names only newer Unicode or other engines
// know, against random code points.
func writeUnicodeClassCases(lw *lineWriter, rng *rand.Rand) {
	var names []string
	for name := range unicode.Categories {
		names = append(names, name)
	}
	for name := range unicode.Scripts {
		names = append(names, name)
	}
	for name := range unicode.CategoryAliases {
		names = append(names, name)
	}
	names = append(names, "Any", "Assigned", "ASCII", "Garay", "Gurung_Khema", "Kirat_Rai",
		"Ol_Onal", "Sunuwar", "Todhri", "Tulu_Tigalari", "Alphabetic", "White_Space", "Emoji",
		"Zyyy", "Grek", "sc=Greek", "Is_Greek", "InGreek", "Cased")
	sort.Strings(names)
	seen := map[string]bool{}
	for _, name := range names {
		spellings := []string{name, strings.ToLower(name), strings.ReplaceAll(name, "_", "")}
		for _, spelling := range spellings {
			for _, form := range []string{`\p{%s}`, `(?i)[^\P{%s}k]`} {
				pattern := fmt.Sprintf(form, spelling)
				if seen[pattern] {
					continue
				}
				seen[pattern] = true
				line := regexpLine{Pattern: pattern}
				re, err := regexp.Compile(pattern)
				line.Valid = err == nil
				if re != nil {
					var matches strings.Builder
					for i := 0; i < 40; i++ {
						input := string(randomRune(rng))
						if i%4 == 0 {
							input = string(interestingRunes[rng.Intn(len(interestingRunes))])
						}
						line.Inputs = append(line.Inputs, input)
						if re.MatchString(input) {
							matches.WriteByte('1')
						} else {
							matches.WriteByte('0')
						}
					}
					line.Matches = matches.String()
				}
				lw.write(line)
			}
		}
	}
}

// writeRepeatCases checks Go's limit on nested counted repetitions (repeatIsValid), stacked
// repetitions around flag groups, and moderately deep nesting.
func writeRepeatCases(lw *lineWriter) {
	emit := func(pattern string, inputs ...string) {
		line := regexpLine{Pattern: pattern}
		re, err := regexp.Compile(pattern)
		line.Valid = err == nil
		if re != nil {
			var matches strings.Builder
			for _, input := range inputs {
				line.Inputs = append(line.Inputs, input)
				if re.MatchString(input) {
					matches.WriteByte('1')
				} else {
					matches.WriteByte('0')
				}
			}
			line.Matches = matches.String()
		}
		lw.write(line)
	}
	repeats := []string{"{2}", "{3}", "{10}", "{100}", "{334}", "{500}", "{1000}", "{0}", "{0,}",
		"{1}", "{2,}", "{0,1}", "{1,2}", "*", "+", "?"}
	for _, inner := range []string{"a", "(a)", "[ab]", "(?:ab)", "(a|b{3})", "(?i)k"} {
		for _, r1 := range repeats {
			for _, r2 := range repeats {
				emit("^(?:"+inner+r1+")"+r2+"$", "", "a", "aa", "aaa", "kK", "abab")
			}
		}
	}
	for _, r1 := range repeats {
		for _, between := range []string{"(?i)", "(?m)", "(?i)(?s)", `\Q\E`, "(?i:)", "()"} {
			for _, r2 := range repeats {
				emit("a"+r1+between+r2, "", "a", "aa", "A")
			}
		}
	}
	for _, depth := range []int{10, 50, 100, 120} {
		for _, shape := range [][2]string{{"(", ")"}, {"(?:", ")"}, {"(?:b|", ")"}, {"(", ")*"},
			{"(?i:", "){1,2}"}, {"[a](", ")"}} {
			emit(strings.Repeat(shape[0], depth)+"a"+strings.Repeat(shape[1], depth), "a", "b", "ab")
		}
	}
}

// ---------------------------------------------------------------------------------------------
// JSON text generation shared by the engine and bidder cases.

var numberPool = []string{
	"0", "1", "-1", "2", "3", "5", "7", "10", "42", "100", "-0", "1.0", "1.00", "-0.0", "0.0",
	"1e0", "1E0", "1e+0", "10e-1", "0.1e1", "100e-2", "1e2", "1E+2", "1e-2", "1.5", "-2.25",
	"0.1", "0.2", "0.3", "0.000001", "1e-7", "3.14159", "2.5e-3", "12345678901234567890",
	"9007199254740993", "9007199254740992", "1e21", "1e20", "1e308", "1e309", "1e400",
	"-1e400", "1e-400", "1e1000001", "0e1000001", "1.5e-1000001", "-1e-1000001",
	"123456789012345678901234567890.5", "0.30000000000000004", "4.35", "1e-6", "9.999999e5",
	"999999", "1000000", "1234567", "0.00001", "0.0001", "-1000000", "2e-5", "1.0000000000000002",
	"99999999999999999999999999", "18446744073709551616", "9223372036854775807",
	"9223372036854775808", "-9223372036854775809", "5e-324", "2.5e-324",
	"1.7976931348623157e308", "1.7976931348623159e308", "0.1e-1000005", "1e9223372036854775807",
	"1e-9223372036854775808", "1e9223372036854775808", "12.5e-1", "1.25e1", "-0e5", "0.0e-5",
}

// slowNumbers are accepted by big.Rat but take it a long time (10^1000000).
var slowNumbers = []string{"1e1000000", "1e-1000000", "-1.5e999999", "0.1e1000001"}

func genNumber(rng *rand.Rand) string {
	if rng.Intn(400) == 0 {
		return pick(rng, slowNumbers)
	}
	if rng.Intn(4) > 0 {
		return pick(rng, numberPool)
	}
	var b strings.Builder
	if rng.Intn(3) == 0 {
		b.WriteByte('-')
	}
	if rng.Intn(4) == 0 {
		b.WriteByte('0')
	} else {
		b.WriteByte(byte('1' + rng.Intn(9)))
		for i := rng.Intn(6); i > 0; i-- {
			b.WriteByte(byte('0' + rng.Intn(10)))
		}
	}
	if rng.Intn(2) == 0 {
		b.WriteByte('.')
		for i := 1 + rng.Intn(5); i > 0; i-- {
			b.WriteByte(byte('0' + rng.Intn(10)))
		}
	}
	if rng.Intn(3) == 0 {
		b.WriteString(pick(rng, []string{"e", "E", "e+", "e-", "E-"}))
		for i := 1 + rng.Intn(3); i > 0; i-- {
			b.WriteByte(byte('0' + rng.Intn(10)))
		}
	}
	return b.String()
}

// stringPool holds JSON string literals (with their quotes), some with raw invalid UTF-8.
var stringPool = jsonEscapes([]string{
	`""`, `"a"`, `" "`, `"abc"`, `"123"`, `"0"`, `"007"`, `"-1"`, `"1.5"`, `"1e5"`, `" 123"`,
	`"123 "`, `"123\n"`, `"\n123"`, `"12a"`, `"١٢٣"`, `"１２３"`, `"gross"`, `"net"`, `"Gross"`,
	`"^u0067ross"`, `"gross^u0000"`, `"banner"`, `"video"`, `"native"`, `"true"`, `"null"`,
	`"123e4567-e89b-12d3-a456-426614174000"`, `"123E4567-E89B-12D3-A456-426614174000"`,
	`"123e4567e89b12d3a456426614174000"`, `"{123e4567-e89b-12d3-a456-426614174000}"`,
	`"123e4567-e89b-12d3-a456-42661417400"`, `"00000000-0000-0000-0000-000000000000"`,
	`"example.com"`, `"a.b"`, `"-bad.com"`, `"exa_mple.com"`, `"xn--bcher-kva.example"`,
	`"example.com."`, `"localhost"`, `"1.2.3.4"`, `"ex ample.com"`, `"例え.jp"`, `"a..b"`,
	`"http://example.com"`, `"https://example.com:8080"`, `"example.com:80"`, `"x.y:z"`,
	`"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.com"`,
	`"^u0000"`, `"^u001f"`, `"^ud800"`, `"^udc00"`, `"^ud800^udc00"`, `"^ud83d^ude00"`,
	`"^ud800^u0041"`, `"^ud800^ud800"`, `"\\"`, `"\""`, `"\/"`, `"^u2028"`, `"<>&"`, `"é"`,
	`"e^u0301"`, `"\b\f\n\r\t"`, `"^uFFFD"`, `"^uffff"`, `"日本語"`, `"😀"`, `"^u00e9"`,
	`"^udbff^udfff"`, `"^uDC00^uD800"`, `"^ud800^u"`, `"^u00E9^u00e9"`, `"^u0130"`, `"^u212a"`,
	`"abcdefghijklmnop"`, `"abcdefghijklmno|"`, `"ABCDEFGHIJKLMNOP"`, `"0123456789abcdef"`,
	`"0123456789ABCDEF0123456789abcdef"`, `"0123456789abcdef0123456789abcdef-123"`,
	`"x-1-2"`, `"1-2"`, `"-1-2"`, `"a-"`, `"a_b"`, `"_a"`, `"-a"`, `"a--b"`, `"ab_-cd"`,
	`".com"`, `"a.co"`, `"a.comm"`, `"a.c0m"`, `"1.5.6"`, `"12.34"`, `"12."`, `"٣.٤"`,
	"\"\xff\"", "\"a\xc0\x80b\"", "\"\xed\xa0\x80\"", "\"\xe2\x82\"", "\"\xf4\x90\x80\x80\"",
	"\"\xc3\xa9\"", "\"\x7f\"", "\"tab\there\"",
})

// genString returns a JSON string literal.
func genString(rng *rand.Rand) string {
	switch rng.Intn(4) {
	case 0:
		// A random run of interesting runes, escaped like encoding/json would or with \u.
		var b strings.Builder
		b.WriteByte('"')
		for i := rng.Intn(20); i > 0; i-- {
			r := interestingRunes[rng.Intn(len(interestingRunes))]
			switch {
			case r == '"' || r == '\\':
				b.WriteByte('\\')
				b.WriteRune(r)
			case r < 0x20 || rng.Intn(5) == 0:
				if r > 0xffff {
					r1, r2 := 0xd800+((r-0x10000)>>10), 0xdc00+((r-0x10000)&0x3ff)
					fmt.Fprintf(&b, `\u%04x\u%04X`, r1, r2)
				} else {
					fmt.Fprintf(&b, `\u%04x`, r)
				}
			default:
				b.WriteRune(r)
			}
		}
		b.WriteByte('"')
		return b.String()
	case 1:
		// A string of a chosen length in ASCII, two-byte or four-byte characters.
		unit := pick(rng, []string{"a", "é", "😀", "1", "\\u00e9", "\\ud83d\\ude00"})
		return `"` + strings.Repeat(unit, rng.Intn(40)) + `"`
	default:
		return pick(rng, stringPool)
	}
}

var keyPool = jsonEscapes([]string{
	`"a"`, `"b"`, `"c"`, `""`, `"a.b"`, `"é"`, `"x y"`, `"A"`, `"aa"`, `"0"`, `"^u0061"`,
	`"a^u0000"`, `"(root)"`, `"$ref"`, `"^ud800"`, "\"\xff\"", `"a\"b"`, `"a\\b"`, `"a/b"`,
})

// genValue returns random JSON text; keys come from keys when it is not empty.
func genValue(rng *rand.Rand, depth int, keys []string) string {
	r := rng.Intn(12)
	if depth > 3 && r >= 9 {
		r = rng.Intn(9)
	}
	switch {
	case r == 0:
		return "null"
	case r == 1:
		return pick(rng, []string{"true", "false"})
	case r < 5:
		return genNumber(rng)
	case r < 9:
		return genString(rng)
	case r < 10:
		n := rng.Intn(4)
		items := make([]string, n)
		for i := range items {
			items[i] = genValue(rng, depth+1, keys)
		}
		if n > 0 && rng.Intn(3) == 0 {
			items = append(items, items[0])
		}
		return "[" + strings.Join(items, ",") + "]"
	default:
		n := rng.Intn(4)
		var members []string
		for i := 0; i < n; i++ {
			key := pick(rng, keyPool)
			if len(keys) > 0 && rng.Intn(3) > 0 {
				key = pick(rng, keys)
			}
			members = append(members, key+":"+genValue(rng, depth+1, keys))
		}
		return "{" + strings.Join(members, ",") + "}"
	}
}

// space returns random insignificant whitespace (sometimes none).
func space(rng *rand.Rand) string {
	if rng.Intn(4) > 0 {
		return ""
	}
	return pick(rng, []string{" ", "\n", "\t", "\r\n", "  "})
}

// mutate damages JSON text: truncation, inserted or replaced bytes, trailing data.
func mutate(rng *rand.Rand, doc string) string {
	b := []byte(doc)
	switch rng.Intn(5) {
	case 0:
		return string(b[:rng.Intn(len(b)+1)])
	case 1:
		i := rng.Intn(len(b) + 1)
		ins := pick(rng, []string{"x", "}", "]", ",", ":", "\"", "\\", "\x00", "\xff", " ", "{", "[", "-", "1", "e", ".", "\x1f", "\xe2\x80\xa8", "nul", "tru"})
		return string(b[:i]) + ins + string(b[i:])
	case 2:
		if len(b) == 0 {
			return doc
		}
		i := rng.Intn(len(b))
		b[i] = pick(rng, []string{"x", "}", "]", ",", ":", "\"", "\\", "\x00", "\xff", " ", "0", "\n"})[0]
		return string(b)
	case 3:
		return doc + pick(rng, []string{"x", " x", "}", "]", " 1", "{}", "\n\n", ",", "\x00", "\"", "[", "  {", "null"})
	default:
		return pick(rng, []string{"\xef\xbb\xbf", " ", "\n", "\x00", "[", "{", "\"", "-", "1"}) + doc
	}
}

// deepDocuments nest close to encoding/json's depth limit of 10000.
func deepDocuments() []string {
	var docs []string
	for _, depth := range []int{9999, 10000, 10001} {
		docs = append(docs,
			strings.Repeat("[", depth)+strings.Repeat("]", depth),
			strings.Repeat("[", depth)+strings.Repeat("]", depth-1),
			strings.Repeat(`{"a":`, depth)+"1"+strings.Repeat("}", depth),
			strings.Repeat(`{"a":`, depth-1)+`[1,{"a":{}}]`+strings.Repeat("}", depth-1)+"x",
		)
	}
	return docs
}

// ---------------------------------------------------------------------------------------------
// Validation sampling.

// outcome is the result of one validation.
type outcome struct {
	kind   string // "ok", "invalid", "decode" or "panic"
	text   string // the joined error strings, or the decode error
	errors []gojsonschema.ResultError
}

func validateOnce(schema *gojsonschema.Schema, doc []byte) (result outcome) {
	defer func() {
		if r := recover(); r != nil {
			result = outcome{kind: "panic", text: fmt.Sprint(r)}
		}
	}()
	res, err := schema.Validate(gojsonschema.NewBytesLoader(doc))
	if err != nil {
		return outcome{kind: "decode", text: err.Error()}
	}
	if res.Valid() {
		return outcome{kind: "ok"}
	}
	var b strings.Builder
	for _, e := range res.Errors() {
		b.WriteString(e.String())
	}
	return outcome{kind: "invalid", text: b.String(), errors: res.Errors()}
}

// sampled is every distinct outcome of a document over all schema copies.
type sampled struct {
	kind    string
	outputs []string
	first   outcome
}

func sample(schemas []*gojsonschema.Schema, doc []byte) sampled {
	seen := map[string]bool{}
	var result sampled
	for _, schema := range schemas {
		for r := 0; r < repeats; r++ {
			o := validateOnce(schema, doc)
			if result.kind == "" {
				result.kind = o.kind
				result.first = o
			} else if result.kind != o.kind {
				log.Fatalf("inconsistent outcome for %q: %s and %s", doc, result.kind, o.kind)
			}
			if o.kind == "invalid" || o.kind == "decode" {
				seen[o.text] = true
			}
		}
	}
	for text := range seen {
		result.outputs = append(result.outputs, text)
	}
	sort.Strings(result.outputs)
	return result
}

// ---------------------------------------------------------------------------------------------
// Engine cases: random schemas and documents.

type draftInfo struct {
	name   string
	prefix string
	level  int // 4, 5 (hybrid), 6 or 7
}

var drafts = []draftInfo{
	{"hybrid", "", 5},
	{"draft4", `"$schema":"http://json-schema.org/draft-04/schema#"`, 4},
	{"draft6", `"$schema":"http://json-schema.org/draft-06/schema#"`, 6},
	{"draft7", `"$schema":"http://json-schema.org/draft-07/schema#"`, 7},
}

var propertyNames = []string{"a", "b", "c", "", "a.b", "é", "x y", "A", "aa", "0", "(root)"}

var validPatterns = []string{
	"^a", "[0-9]", "^[a-c]$", "^$", "b+", "^\\d+$", "^[a-z]{2,3}$", ".", "^(a|b)*$",
	"^[^a]", "(?i)^A", "\\S", "^.{0,2}$", "^\\w+$", "x*", "^[0-9]+(\\.[0-9]+)?$",
}

var formats = []string{
	"uuid", "hostname", "regex", "json-pointer", "relative-json-pointer", "unknown", "UUID",
	"",
}

// schemaGen builds random schemas as JSON text.
type schemaGen struct {
	rng   *rand.Rand
	level int
}

func (g *schemaGen) chance(n int) bool { return g.rng.Intn(n) == 0 }

func (g *schemaGen) types() string {
	all := []string{`"string"`, `"integer"`, `"number"`, `"boolean"`, `"object"`, `"array"`, `"null"`}
	if g.chance(30) {
		return pick(g.rng, []string{`"foo"`, `1`, `[]`, `["string",1]`, `["string","string"]`, `null`, `[null]`})
	}
	if g.chance(3) {
		n := 2 + g.rng.Intn(2)
		var list []string
		for i := 0; i < n; i++ {
			list = append(list, pick(g.rng, all))
		}
		return "[" + strings.Join(list, ",") + "]"
	}
	return pick(g.rng, all)
}

func (g *schemaGen) nonNegative() string {
	if g.chance(15) {
		return pick(g.rng, []string{"-1", "1.0", `"2"`, "1.5", "1e2", "null", "99999999999999999999"})
	}
	return fmt.Sprint(g.rng.Intn(5))
}

func (g *schemaGen) limit() string {
	if g.chance(15) {
		return pick(g.rng, []string{`"1"`, "null", "true"})
	}
	return genNumber(g.rng)
}

func (g *schemaGen) subSchema(depth int) string {
	if g.level >= 6 && g.chance(8) {
		return pick(g.rng, []string{"true", "false"})
	}
	if g.chance(40) {
		return pick(g.rng, []string{"1", `"x"`, "[]", "null", "true"})
	}
	return g.schema(depth+1, "")
}

func (g *schemaGen) schemaList(depth int) string {
	n := 1 + g.rng.Intn(3)
	var list []string
	for i := 0; i < n; i++ {
		list = append(list, g.subSchema(depth))
	}
	return "[" + strings.Join(list, ",") + "]"
}

func (g *schemaGen) schema(depth int, prefix string) string {
	var members []string
	if prefix != "" {
		members = append(members, prefix)
	}
	add := func(key, value string) {
		members = append(members, `"`+key+`":`+value)
	}
	budget := 1 + g.rng.Intn(4)
	if depth > 2 {
		budget = g.rng.Intn(2)
	}
	for i := 0; i < budget; i++ {
		switch g.rng.Intn(24) {
		case 0, 1:
			add("type", g.types())
		case 2, 3:
			n := 1 + g.rng.Intn(3)
			var props []string
			for j := 0; j < n; j++ {
				props = append(props, `"`+pick(g.rng, propertyNames)+`":`+g.subSchema(depth))
			}
			add("properties", "{"+strings.Join(props, ",")+"}")
			if g.chance(2) {
				var req []string
				for j := 0; j < 1+g.rng.Intn(2); j++ {
					req = append(req, `"`+pick(g.rng, propertyNames)+`"`)
				}
				add("required", "["+strings.Join(req, ",")+"]")
			}
		case 4:
			if g.chance(2) {
				add("additionalProperties", pick(g.rng, []string{"false", "true", "1"}))
			} else {
				add("additionalProperties", g.subSchema(depth))
			}
		case 5:
			pp := `"` + strings.ReplaceAll(pick(g.rng, validPatterns), `\`, `\\`) + `":` + g.subSchema(depth)
			if g.chance(10) {
				pp = `"(":{}`
			}
			add("patternProperties", "{"+pp+"}")
		case 6:
			add(pick(g.rng, []string{"minLength", "maxLength"}), g.nonNegative())
		case 7:
			if g.chance(10) {
				add("pattern", pick(g.rng, []string{`"("`, "1", `"\\p{Greek}"`, `"[[:foo:]]"`}))
			} else {
				add("pattern", `"`+strings.ReplaceAll(pick(g.rng, validPatterns), `\`, `\\`)+`"`)
			}
		case 8:
			add("format", `"`+pick(g.rng, formats)+`"`)
		case 9, 10:
			key := pick(g.rng, []string{"minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf"})
			value := g.limit()
			if strings.HasPrefix(key, "exclusive") && g.level <= 5 && g.chance(2) {
				value = pick(g.rng, []string{"true", "false"})
				base := "minimum"
				if key == "exclusiveMaximum" {
					base = "maximum"
				}
				if !g.chance(8) {
					add(base, g.limit())
				}
			}
			add(key, value)
		case 11:
			n := 1 + g.rng.Intn(4)
			var values []string
			for j := 0; j < n; j++ {
				values = append(values, genValue(g.rng, 2, nil))
			}
			add("enum", "["+strings.Join(values, ",")+"]")
		case 12:
			add("const", genValue(g.rng, 2, nil))
		case 13:
			if g.chance(2) {
				add("items", g.subSchema(depth))
			} else {
				add("items", g.schemaList(depth))
				if g.chance(2) {
					if g.chance(2) {
						add("additionalItems", pick(g.rng, []string{"false", "true"}))
					} else {
						add("additionalItems", g.subSchema(depth))
					}
				}
			}
		case 14:
			add(pick(g.rng, []string{"minItems", "maxItems", "minProperties", "maxProperties"}), g.nonNegative())
		case 15:
			add("uniqueItems", pick(g.rng, []string{"true", "false", "1"}))
		case 16:
			add("contains", g.subSchema(depth))
		case 17:
			deps := `"` + pick(g.rng, propertyNames) + `":["` + pick(g.rng, propertyNames) + `"]`
			if g.chance(2) {
				deps = `"` + pick(g.rng, propertyNames) + `":` + g.subSchema(depth)
			}
			if g.chance(10) {
				deps = `"a":[]`
			}
			add("dependencies", "{"+deps+"}")
		case 18:
			add("propertyNames", g.subSchema(depth))
		case 19, 20:
			add(pick(g.rng, []string{"oneOf", "anyOf", "allOf"}), g.schemaList(depth))
		case 21:
			add("not", g.subSchema(depth))
		case 22:
			add(pick(g.rng, []string{"if", "then", "else"}), g.subSchema(depth))
		default:
			add(pick(g.rng, []string{"title", "description", "id", "$id", "definitions", "$comment", "default"}),
				pick(g.rng, []string{`"x"`, "1", "{}", `{"d":{}}`, "true"}))
		}
	}
	return "{" + strings.Join(members, ",") + "}"
}

// documentFor generates a document using the property names the schema mentions.
func documentFor(rng *rand.Rand) string {
	return genValue(rng, 0, []string{`"a"`, `"b"`, `"c"`, `""`, `"a.b"`, `"é"`, `"x y"`, `"A"`, `"aa"`, `"0"`, `"(root)"`})
}

// engineLine is one line of adversarial_engine.jsonl.
type engineLine struct {
	Schema    *string `json:"schema,omitempty"`
	SchemaHex string  `json:"schema_hex,omitempty"`
	// Kind is loaded, load_error or load_panic.
	Kind    string      `json:"kind"`
	Outputs []string    `json:"outputs,omitempty"`
	Docs    []engineDoc `json:"docs,omitempty"`
}

// engineDoc is one validated document of an engine schema.
type engineDoc struct {
	Doc    *string `json:"doc,omitempty"`
	DocHex string  `json:"doc_hex,omitempty"`
	// Kind is ok, invalid, decode or panic.
	Kind    string   `json:"kind"`
	Outputs []string `json:"outputs,omitempty"`
	// Types are the "type context" pairs of the errors, sorted (with Outputs, the field and
	// description are fixed too).
	Types []string `json:"types,omitempty"`
	// Errors are the individual ResultError strings, sorted, when Outputs has several entries.
	Errors []string `json:"errors,omitempty"`
}

func loadOnce(schema string) (s *gojsonschema.Schema, err error, panicked bool) {
	defer func() {
		if r := recover(); r != nil {
			s, err, panicked = nil, fmt.Errorf("%v", r), true
		}
	}()
	s, err = gojsonschema.NewSchema(gojsonschema.NewStringLoader(schema))
	return s, err, false
}

func writeEngineCases(path string, rng *rand.Rand) {
	lw := newLineWriter(path)
	defer lw.close()
	for i := 0; i < 2500; i++ {
		d := drafts[rng.Intn(len(drafts))]
		g := &schemaGen{rng: rng, level: d.level}
		schema := g.schema(0, d.prefix)
		writeEngineSchema(lw, schema, func() string {
			doc := documentFor(rng)
			if rng.Intn(15) == 0 {
				doc = mutate(rng, doc)
			}
			return doc
		}, 8)
	}
	deep := deepDocuments()
	next := 0
	writeEngineSchema(lw, `{"items":{"maxItems":0},"properties":{"a":{"minProperties":2}}}`, func() string {
		next++
		return deep[next-1]
	}, len(deep))
}

// writeEngineSchema loads schema several times and validates count documents from docs.
func writeEngineSchema(lw *lineWriter, schema string, docs func() string, count int) {
	line := engineLine{}
	line.Schema, line.SchemaHex = textFields([]byte(schema))
	var loaded []*gojsonschema.Schema
	loadErrors := map[string]bool{}
	panicked := false
	for c := 0; c < copies; c++ {
		s, err, p := loadOnce(schema)
		panicked = panicked || p
		if err != nil {
			loadErrors[err.Error()] = true
			continue
		}
		loaded = append(loaded, s)
	}
	if len(loadErrors) > 0 {
		if len(loaded) > 0 {
			// Map order decided which keyword failed first: not reproducible.
			return
		}
		line.Kind = "load_error"
		if panicked {
			line.Kind = "load_panic"
		}
		for text := range loadErrors {
			line.Outputs = append(line.Outputs, text)
		}
		sort.Strings(line.Outputs)
		lw.write(line)
		return
	}
	line.Kind = "loaded"
	for j := 0; j < count; j++ {
		doc := docs()
		var docLine engineDoc
		docLine.Doc, docLine.DocHex = textFields([]byte(doc))
		s := sample(loaded, []byte(doc))
		docLine.Kind = s.kind
		docLine.Outputs = s.outputs
		for _, e := range s.first.errors {
			docLine.Types = append(docLine.Types, e.Type()+" "+e.Context().String())
			if len(s.outputs) > 1 {
				docLine.Errors = append(docLine.Errors, e.String())
			}
		}
		sort.Strings(docLine.Types)
		sort.Strings(docLine.Errors)
		line.Docs = append(line.Docs, docLine)
	}
	lw.write(line)
}

// ---------------------------------------------------------------------------------------------
// Bidder cases: random documents against the real bidder schemas.

// bidderLine is one line of adversarial_bidders.jsonl.
type bidderLine struct {
	Bidder string  `json:"bidder"`
	Ext    *string `json:"ext,omitempty"`
	ExtHex string  `json:"ext_hex,omitempty"`
	// Kind is ok, invalid, decode or panic.
	Kind    string   `json:"kind"`
	Outputs []string `json:"outputs,omitempty"`
	// Errors are the individual ResultError strings, sorted, when Outputs has several entries.
	Errors []string `json:"errors,omitempty"`
}

// shape is what the generator knows about one (sub-)schema of a bidder.
type shape struct {
	props    map[string]*shape
	names    []string
	required []string
	types    []string
	enum     []string
	items    *shape
	min      []string
	max      []string
	any      []*shape
}

func shapeOf(v interface{}) *shape {
	m, ok := v.(map[string]interface{})
	if !ok {
		return &shape{}
	}
	s := &shape{props: map[string]*shape{}}
	if props, ok := m["properties"].(map[string]interface{}); ok {
		for name, sub := range props {
			s.props[name] = shapeOf(sub)
			s.names = append(s.names, name)
		}
		sort.Strings(s.names)
	}
	if req, ok := m["required"].([]interface{}); ok {
		for _, r := range req {
			if name, ok := r.(string); ok {
				s.required = append(s.required, name)
			}
		}
	}
	switch t := m["type"].(type) {
	case string:
		s.types = []string{t}
	case []interface{}:
		for _, x := range t {
			if name, ok := x.(string); ok {
				s.types = append(s.types, name)
			}
		}
	}
	if values, ok := m["enum"].([]interface{}); ok {
		for _, v := range values {
			data, err := json.Marshal(v)
			if err == nil {
				s.enum = append(s.enum, string(data))
			}
		}
	}
	if items, ok := m["items"]; ok {
		s.items = shapeOf(items)
	}
	for _, key := range []string{"minimum", "exclusiveMinimum", "minLength", "minItems"} {
		if n, ok := m[key].(json.Number); ok {
			s.min = append(s.min, n.String())
		}
	}
	for _, key := range []string{"maximum", "exclusiveMaximum", "maxLength", "maxItems"} {
		if n, ok := m[key].(json.Number); ok {
			s.max = append(s.max, n.String())
		}
	}
	for _, key := range []string{"oneOf", "anyOf", "allOf"} {
		if list, ok := m[key].([]interface{}); ok {
			for _, sub := range list {
				s.any = append(s.any, shapeOf(sub))
			}
		}
	}
	if not, ok := m["not"]; ok {
		s.any = append(s.any, shapeOf(not))
	}
	for _, alt := range s.any {
		for _, name := range alt.names {
			if _, ok := s.props[name]; !ok {
				s.props[name] = alt.props[name]
				s.names = append(s.names, name)
			}
		}
	}
	sort.Strings(s.names)
	return s
}

// near returns numbers around a limit.
func near(limit string) []string {
	return []string{limit, "-" + limit, limit + ".0", limit + "e0", limit + ".5", "0" + limit}
}

// valueFor generates a value for a property: usually of a declared type, sometimes anything.
func valueFor(rng *rand.Rand, s *shape, depth int) string {
	if s == nil || rng.Intn(5) == 0 {
		return genValue(rng, depth+1, nil)
	}
	if len(s.enum) > 0 && rng.Intn(2) == 0 {
		return pick(rng, s.enum)
	}
	if rng.Intn(4) == 0 && len(s.min)+len(s.max) > 0 {
		limits := append(append([]string{}, s.min...), s.max...)
		candidates := near(strings.TrimPrefix(pick(rng, limits), "-"))
		if rng.Intn(2) == 0 {
			n := 0
			fmt.Sscan(strings.TrimPrefix(pick(rng, limits), "-"), &n)
			if n >= 0 && n < 64 {
				return `"` + strings.Repeat(pick(rng, []string{"a", "é", "😀", "\\u00e9"}), n+rng.Intn(3)-1+boolInt(n == 0)) + `"`
			}
		}
		return pick(rng, candidates)
	}
	t := "any"
	if len(s.types) > 0 {
		t = pick(rng, s.types)
	}
	switch t {
	case "string":
		return genString(rng)
	case "integer", "number":
		return genNumber(rng)
	case "boolean":
		return pick(rng, []string{"true", "false", `"true"`, "1", "0"})
	case "array":
		n := rng.Intn(4)
		var items []string
		for i := 0; i < n; i++ {
			items = append(items, valueFor(rng, s.items, depth+1))
		}
		if n > 0 && rng.Intn(3) == 0 {
			items = append(items, items[0])
		}
		return "[" + space(rng) + strings.Join(items, ","+space(rng)) + "]"
	case "object":
		return objectFor(rng, s, depth+1)
	case "null":
		return "null"
	default:
		if len(s.names) > 0 {
			return objectFor(rng, s, depth+1)
		}
		return genValue(rng, depth+1, nil)
	}
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

// escapeKey writes a property name as a JSON string, sometimes with needless \u escapes.
func escapeKey(rng *rand.Rand, name string) string {
	data, err := json.Marshal(name)
	if err != nil {
		log.Fatal(err)
	}
	if rng.Intn(10) > 0 || name == "" {
		return string(data)
	}
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range name {
		if r < 0x80 && rng.Intn(3) == 0 {
			fmt.Fprintf(&b, `\u%04x`, r)
		} else if r == '"' || r == '\\' {
			b.WriteByte('\\')
			b.WriteRune(r)
		} else {
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}

func objectFor(rng *rand.Rand, s *shape, depth int) string {
	if depth > 4 {
		return "{}"
	}
	var members []string
	used := map[string]bool{}
	addMember := func(name string) {
		used[name] = true
		members = append(members, escapeKey(rng, name)+space(rng)+":"+space(rng)+valueFor(rng, s.props[name], depth))
	}
	for _, name := range s.required {
		if rng.Intn(6) > 0 {
			addMember(name)
		}
	}
	for _, name := range s.names {
		if !used[name] && rng.Intn(3) == 0 {
			addMember(name)
		}
	}
	if rng.Intn(4) == 0 {
		extra := pick(rng, []string{"foo", "", "id", "Id", "placementid", "PlacementId", "placementId ", "é", "a.b", "$ref", "__proto__"})
		if len(s.names) > 0 && rng.Intn(2) == 0 {
			name := pick(rng, s.names)
			extra = pick(rng, []string{strings.ToUpper(name[:1]) + name[1:], name + " ", " " + name, name + "\u0000", strings.ToLower(name)})
		}
		if !used[extra] {
			used[extra] = true
			members = append(members, escapeKey(rng, extra)+":"+genValue(rng, depth+1, nil))
		}
	}
	if len(members) > 0 && rng.Intn(8) == 0 {
		// A repeated key: encoding/json keeps the last value.
		name := "dup"
		if len(s.names) > 0 {
			name = pick(rng, s.names)
		}
		members = append([]string{escapeKey(rng, name) + ":" + genValue(rng, depth+1, nil)}, members...)
		members = append(members, escapeKey(rng, name)+":"+valueFor(rng, s.props[name], depth))
	}
	rng.Shuffle(len(members), func(i, j int) { members[i], members[j] = members[j], members[i] })
	return "{" + space(rng) + strings.Join(members, ","+space(rng)) + space(rng) + "}"
}

func writeBidderCases(path string, rng *rand.Rand) {
	// Registers the aliases declared in static/bidder-info, like the server does at startup.
	if _, err := config.LoadBidderInfoFromDisk("static/bidder-info"); err != nil {
		log.Fatal(err)
	}
	validator, err := openrtb_ext.NewBidderParamsValidator(schemaDir)
	if err != nil {
		log.Fatal(err)
	}
	entries, err := os.ReadDir(schemaDir)
	if err != nil {
		log.Fatal(err)
	}
	aliasesOf := map[string][]string{}
	for alias, parent := range openrtb_ext.GetAliasBidderToParent() {
		aliasesOf[string(parent)] = append(aliasesOf[string(parent)], string(alias))
	}
	lw := newLineWriter(path)
	defer lw.close()
	for _, entry := range entries {
		file := filepath.Join(schemaDir, entry.Name())
		bidder := strings.TrimSuffix(entry.Name(), ".json")
		abs, err := filepath.Abs(file)
		if err != nil {
			log.Fatal(err)
		}
		var loaded []*gojsonschema.Schema
		for c := 0; c < copies; c++ {
			schema, err := gojsonschema.NewSchema(gojsonschema.NewReferenceLoader("file:///" + filepath.ToSlash(abs)))
			if err != nil {
				log.Fatalf("%s: %v", file, err)
			}
			loaded = append(loaded, schema)
		}
		data, err := os.ReadFile(file)
		if err != nil {
			log.Fatal(err)
		}
		dec := json.NewDecoder(bytes.NewReader(data))
		dec.UseNumber()
		var doc interface{}
		if err := dec.Decode(&doc); err != nil {
			log.Fatal(err)
		}
		s := shapeOf(doc)
		names := append([]string{bidder}, aliasesOf[bidder]...)
		sort.Strings(names[1:])
		seen := map[string]bool{}
		for i := 0; i < 32; i++ {
			ext := objectFor(rng, s, 0)
			if rng.Intn(12) == 0 {
				ext = mutate(rng, ext)
			}
			if rng.Intn(25) == 0 {
				ext = valueFor(rng, s, 0)
			}
			if seen[ext] {
				continue
			}
			seen[ext] = true
			name := names[0]
			if len(names) > 1 && rng.Intn(5) == 0 {
				name = pick(rng, names[1:])
			}
			lw.write(runBidderCase(validator, loaded, name, []byte(ext)))
		}
	}
}

func runBidderCase(validator openrtb_ext.BidderParamValidator, loaded []*gojsonschema.Schema, bidder string, ext []byte) bidderLine {
	line := bidderLine{Bidder: bidder}
	line.Ext, line.ExtHex = textFields(ext)
	s := sample(loaded, ext)
	kind, text := validateWithValidator(validator, bidder, ext)
	if kind != s.kind {
		log.Fatalf("%s %q: the validator says %s, the schema copies %s", bidder, ext, kind, s.kind)
	}
	if kind == "invalid" || kind == "decode" {
		found := false
		for _, o := range s.outputs {
			found = found || o == text
		}
		if !found {
			s.outputs = append(s.outputs, text)
			sort.Strings(s.outputs)
		}
	}
	line.Kind = s.kind
	line.Outputs = s.outputs
	if len(s.outputs) > 1 && s.kind == "invalid" {
		for _, e := range s.first.errors {
			line.Errors = append(line.Errors, e.String())
		}
		sort.Strings(line.Errors)
	}
	return line
}

func validateWithValidator(validator openrtb_ext.BidderParamValidator, bidder string, ext []byte) (kind, text string) {
	defer func() {
		if r := recover(); r != nil {
			kind, text = "panic", fmt.Sprint(r)
		}
	}()
	err := validator.Validate(openrtb_ext.BidderName(bidder), json.RawMessage(ext))
	if err == nil {
		return "ok", ""
	}
	if _, loadErr := gojsonschema.NewBytesLoader(ext).LoadJSON(); loadErr != nil {
		return "decode", err.Error()
	}
	return "invalid", err.Error()
}
