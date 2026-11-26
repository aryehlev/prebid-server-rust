package main

import (
	"hash/fnv"
	"regexp"
	"regexp/syntax"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// goregexCount is the number of seeded subjects of every site pattern.
const goregexCount = 1000

// goregexMaxTokens is the most tokens a subject is made of.
const goregexMaxTokens = 12

// genericRegexTokens are mixed into every pattern's tokens: ASCII classes and their
// boundaries, multibyte text, and the runes on which Go's ASCII classes and the regex crate's
// Unicode classes disagree (a vertical tab, Arabic-Indic and fullwidth digits, Unicode spaces,
// the Kelvin sign and the long s that fold to k and s). Every token is valid UTF-8: the Rust
// side matches &str.
var genericRegexTokens = []string{
	"a", "b", "z", "A", "Z", "0", "1", "9", "_", "-", ".", " ", "\n", "\r", "\t", "/", ":", "@",
	"$", "{", "}", "(", ")", "\u00e9", "\u65e5", "\u00a0", "\v", "\f", "\u0663", "\uff11", "\u2003",
	"\u0085", "\u3000", "\u212a", "\u017f", "\u0416", "\u00df", "\u00e9a",
}

// regexInput concatenates up to max tokens, each chosen uniformly (GOC-REGEXP's input).
func regexInput(s *splitmix64, tokens []string, max int) string {
	n := s.intn(max + 1)
	var b strings.Builder
	for i := 0; i < n; i++ {
		b.WriteString(tokens[s.intn(len(tokens))])
	}
	return b.String()
}

// regexSeed is the FNV-1a 64 hash of the pattern, the seed of its subjects.
func regexSeed(p string) uint64 {
	h := fnv.New64a()
	h.Write([]byte(p))
	return h.Sum64()
}

// regexTokens are a pattern's tokens: the literals of its syntax tree (in both cases when it
// folds case), the bounds of its character classes and the runes beside them, strings sampled
// from the tree and their halves, then genericRegexTokens (GOC-REGEXP's sweepTokens with valid
// UTF-8 only).
func regexTokens(pattern string, seed uint64) []string {
	var tokens []string
	seen := map[string]bool{}
	add := func(t string) {
		if t != "" && utf8.ValidString(t) && !seen[t] {
			seen[t] = true
			tokens = append(tokens, t)
		}
	}
	re, err := syntax.Parse(pattern, syntax.Perl)
	if err == nil {
		literalRegexTokens(re, add)
		rng := splitmix64{seed ^ 0x5DEECE66D}
		for i := 0; i < 8; i++ {
			var b strings.Builder
			sampleRegex(&rng, re, &b, 64)
			s := b.String()
			add(s)
			half := len(s) / 2
			for half > 0 && !utf8.RuneStart(s[half]) {
				half--
			}
			add(s[:half])
			add(s[half:])
		}
	}
	for _, t := range genericRegexTokens {
		add(t)
	}
	return tokens
}

func literalRegexTokens(re *syntax.Regexp, add func(string)) {
	switch re.Op {
	case syntax.OpLiteral:
		s := string(re.Rune)
		add(s)
		if re.Flags&syntax.FoldCase != 0 {
			add(strings.ToUpper(s))
			add(strings.ToLower(s))
		}
	case syntax.OpCharClass:
		n := len(re.Rune) / 2
		for i := 0; i < n; i++ {
			if i >= 4 && i < n-4 {
				continue
			}
			lo, hi := re.Rune[2*i], re.Rune[2*i+1]
			for _, r := range []rune{lo - 1, lo, hi, hi + 1} {
				if r >= 0 && r <= unicode.MaxRune {
					add(string(r))
				}
			}
		}
	}
	for _, sub := range re.Sub {
		literalRegexTokens(sub, add)
	}
}

// sampleRegexAny are the runes a sampled `.` produces.
var sampleRegexAny = []string{"a", "\u00e9", "\u65e5", " ", "\n"}

// sampleRegex appends a string the syntax tree would mostly match, stopping once b holds
// budget bytes.
func sampleRegex(rng *splitmix64, re *syntax.Regexp, b *strings.Builder, budget int) {
	if b.Len() >= budget {
		return
	}
	switch re.Op {
	case syntax.OpLiteral:
		for _, r := range re.Rune {
			if re.Flags&syntax.FoldCase != 0 && rng.intn(2) == 0 {
				r = unicode.SimpleFold(r)
			}
			b.WriteRune(r)
		}
	case syntax.OpCharClass:
		if len(re.Rune) == 0 {
			return
		}
		i := rng.intn(len(re.Rune)/2) * 2
		lo, hi := re.Rune[i], re.Rune[i+1]
		b.WriteRune(lo + rune(rng.intn(int(hi-lo)+1)))
	case syntax.OpAnyChar, syntax.OpAnyCharNotNL:
		b.WriteString(sampleRegexAny[rng.intn(len(sampleRegexAny))])
	case syntax.OpCapture:
		sampleRegex(rng, re.Sub[0], b, budget)
	case syntax.OpStar, syntax.OpPlus, syntax.OpQuest, syntax.OpRepeat:
		lo, hi := 0, 3
		switch re.Op {
		case syntax.OpPlus:
			lo = 1
		case syntax.OpQuest:
			hi = 1
		case syntax.OpRepeat:
			lo, hi = re.Min, re.Max
			if hi < 0 || hi > lo+3 {
				hi = lo + 3
			}
		}
		n := lo + rng.intn(hi-lo+1)
		for i := 0; i < n; i++ {
			sampleRegex(rng, re.Sub[0], b, budget)
		}
	case syntax.OpConcat:
		for _, sub := range re.Sub {
			sampleRegex(rng, sub, b, budget)
		}
	case syntax.OpAlternate:
		sampleRegex(rng, re.Sub[rng.intn(len(re.Sub))], b, budget)
	}
}

// regexMatches renders FindAllStringSubmatchIndex(s, -1): matches separated by ";", each the
// start and end of every group (-1 for a group that did not take part) separated by ",".
func regexMatches(re *regexp.Regexp, s string) string {
	var out []string
	for _, m := range re.FindAllStringSubmatchIndex(s, -1) {
		ints := make([]string, len(m))
		for i, v := range m {
			ints[i] = strconv.Itoa(v)
		}
		out = append(out, strings.Join(ints, ","))
	}
	return strings.Join(out, ";")
}

type regexSweepJSON struct {
	Pattern       string   `json:"pattern"`
	Seed          uint64   `json:"seed"`
	NumSubexp     int      `json:"num_subexp"`
	Names         []string `json:"names"`
	Tokens        []string `json:"tokens"`
	Matching      int      `json:"matching"`
	InputsSHA256  string   `json:"inputs_sha256"`
	OutputsSHA256 string   `json:"outputs_sha256"`
}

// regexEdgeJSON is an edge pattern: Go's compile error, or its matches of every edge subject in
// order.
type regexEdgeJSON struct {
	Pattern string   `json:"pattern"`
	Error   string   `json:"error"`
	Matches []string `json:"matches"`
}

type goregexJSON struct {
	Count        int              `json:"count"`
	MaxTokens    int              `json:"max_tokens"`
	Patterns     []regexSweepJSON `json:"patterns"`
	EdgeSubjects []string         `json:"edge_subjects"`
	Edges        []regexEdgeJSON  `json:"edges"`
}

// regexEdgeSubjects are matched against every edge pattern: ASCII text, the runes the ASCII
// classes leave out, case variants and the characters the translation escapes.
var regexEdgeSubjects = []string{
	"", "a", "abc", "ABC", "a1_", "x9", "\u0663", "\uff11", "\u0663\u06645", "\u00e9", "\u00e9a", "a\u00e9", "\u65e5\u672c",
	"a b", "a\u00a0b", "a\u2003b", "a\vb", "a\fb", "a\u0085b", "a\u3000b", " \t\n\r",
	"k", "K", "\u212a", "s", "S", "\u017f", "\u03c3", "\u03a3", "\u03c2", "\u00df", "\u1e9e",
	"a.b", "a*b", "a+b", "x*+?y", "{", "}", "{}", "a{", "a{1", "a{1,", "a{,2}", "a{01}", "aa",
	"aaa", "{{.X}}", "{{ .ExternalURL }}", "&&", "a&&b", "~~", "--", "a-z", "[", "]", "[]",
	"a[b", "\\", "\\E", "<", ">", "<a>", "'", "\"", "#", "/", "%", "=", "!", "@", ",", ";",
	"\x00", "\x01", "\n", "\x7f", "foo bar", "foo-bar", "\u00e9foo", "foo\u00e9", "dtv", "smart-tv",
	"tv box", "1.2.3", "baaa", "ab\nab", "a\n",
}

// regexEdgePatterns exercise each construct the translation handles, and the forms Go rejects.
var regexEdgePatterns = []string{
	// ASCII classes and boundaries.
	`\d`, `\d+`, `\D`, `\D+`, `\w+`, `\W`, `\s`, `\s+`, `\S+`, `\b`, `\B`, `\bfoo\b`, `\Bo`,
	`a\b`, `\btv\b`, `[\d]`, `[^\d]`, `[\D]`, `[^\D]`, `[\w-]+`, `[\s\d]`, `[\S]`, `[\W]`,
	`[\s-]+`, `[^\s]+`, `[a\d]`, `[[:space:]]`, `[[:^alpha:]]`, `[[:word:]]+`, `[^[:alnum:]-.]`,
	`[[:digit:][:space:]]`, `\pL+`, `\PL`, `\pN`, `\p{Greek}`, `[\p{Lu}]`, `\pZ`, `[^\x00-\x7f]`,
	// Case folding.
	`(?i)k`, `(?i)s`, `(?i)\w`, `(?i)[a-z]`, `(?i)[k]`, "(?i)\u03c3", "(?i)\u00df", `(?i)K`, `(?i)[\w]`,
	`(?i)\d`, `(?i:a)b`, `(?i)a(?-i)b`,
	// \Q...\E.
	`\Qa.b\E`, `\Qa.b`, `x\Q*+?\Ey`, `\Q\E`, `\Q\\E`, `\Q\E*`, `a\Q\E*`, `(?i)\Qab\E`,
	`\Q{{\E`, `[\Q]\E]`, `\Q[\E`, `\Qa\E\Qb\E`, `\Q\Q\E`, `\E`,
	// Braces.
	`{`, `a{`, `a{1`, `a{1,`, `a{,2}`, `a{1,2}`, `a{2}`, `a{2,}`, `{}`, `a{}`, `a{0}`,
	`a{01}`, `a{1001}`, `a{1000}`, `a{2,1}`, `{2}`, `{{.X}}`, `\{`, `}`, `a}`, `{,}`,
	`{{\s*\.ExternalURL\s*}}`, `^{h(?::(\d+))?}$`, `^([^{]*({[^}]*})?)*$`, `x{1,2}?`,
	// Characters the regex crate reads as set operators or nested classes.
	`[&&]`, `[a&&b]`, `[~~]`, `[a~~b]`, `[a--b]`, `[--]`, `[a-]`, `[-a]`, `[a-z-9]`, `[[]`,
	`[a[b]`, `[]a]`, `[^]a]`, `[\[]`, `[a\]]`, `[a-\d]`, `[\d-z]`, `[-]`, `[^-]`, `[a^]`,
	`[\^]`, `[\\]`, `[a&]`, `[&a]`, `[~]`,
	// Escaped punctuation.
	`\<`, `\>`, `<\/a>`, `\'`, `\"`, `\#`, `\/`, `\-`, `\%`, `\&`, `\~`, `\=`, `\!`, `\@`,
	`\,`, `\:`, `\;`, `\ `, `\_`, "\\\u00e9", `[\<]`, `[\>]`, `[\-]`, `[\&]`, `[\~]`, `[\#]`,
	// Other escapes.
	`\0`, `\01`, `\012`, `\1`, `\8`, `\x41`, `\x{41}`, `\x{10FFFF}`, `\x{110000}`, `\x4`,
	`\C`, `\z`, `\Z`, `\A`, `\G`, `\a\f\t\n\r\v`, `\e`, `\h`, `\K`, `\R`, `\X`, `[\b]`,
	`[\a]`, `[\0]`, `[\x41-\x43]`,
	// Flags, groups and anchors.
	`(?s).`, `.`, `(?m)^a$`, `^a$`, `a$`, `(?U)a+`, `(?x)a b`, `(?u)a`, `(?R)a`,
	`(?P<n>a)`, `(?<n>a)`, `(?P=n)`, `(?=a)`, `(?:a)`, `(?i-s:a.)`, `(?)`, `(?i)`,
	// Repetitions and empty matches.
	`a*`, `a**`, `a+*`, `a*?`, `a??`, `a+?`, `(a*)*`, `(a|)+`, `(a|b)*c`, `x*`, `|`, `()`,
	`a|b|`, `(?:)`,
}

// genGoRegex writes goregex.json: Go's FindAllStringSubmatchIndex over the seeded subjects of
// the site patterns, and over the edge subjects for the edge patterns.
func genGoRegex() {
	out := goregexJSON{Count: goregexCount, MaxTokens: goregexMaxTokens}
	for _, p := range sitePatterns {
		re := regexp.MustCompile(p)
		seed := regexSeed(p)
		tokens := regexTokens(p, seed)
		rng := splitmix64{state: seed}
		m := newManifest()
		matching := 0
		for i := 0; i < goregexCount; i++ {
			s := regexInput(&rng, tokens, goregexMaxTokens)
			got := regexMatches(re, s)
			if got != "" {
				matching++
			}
			m.add(s, got)
		}
		out.Patterns = append(out.Patterns, regexSweepJSON{
			Pattern: p, Seed: seed, NumSubexp: re.NumSubexp(), Names: re.SubexpNames(),
			Tokens: tokens, Matching: matching, InputsSHA256: m.inputs(), OutputsSHA256: m.outputs(),
		})
	}
	out.EdgeSubjects = regexEdgeSubjects
	for _, p := range regexEdgePatterns {
		edge := regexEdgeJSON{Pattern: p, Matches: []string{}}
		re, err := regexp.Compile(p)
		if err != nil {
			edge.Error = err.Error()
		} else {
			for _, s := range regexEdgeSubjects {
				edge.Matches = append(edge.Matches, regexMatches(re, s))
			}
		}
		out.Edges = append(out.Edges, edge)
	}
	writeJSON("goregex.json", out)
}
