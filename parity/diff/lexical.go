package diff

import (
	"strconv"
	"strings"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
)

// The lexical comparison runs on values that are semantically equal and were
// both parsed from exact implementation bytes. It reports byte-level
// differences (kind lexical) and never normalizes them away:
//
//   - html_escape: <, > or & spelled as \u003c, \u003e, \u0026 on one side
//     (Go's jsonutil.Marshal escapes HTML, util/jsonutil/jsonutil.go:128-129).
//   - unicode_escape: any other escape difference (\u2028, \/, \u00e9 against a literal e-acute).
//   - number_format: the same number spelled differently (1 vs 1.0, 1e2 vs 100).
//   - member_order: the members both sides have appear in a different order.
//   - whitespace: insignificant whitespace differs (reported once per document).
//   - trailing_newline: one document ends with a newline the other lacks
//     (json.Encoder.Encode, endpoints/openrtb2/auction.go:383-394).

// escapeUnit is one decoded rune of a JSON string literal and its spelling.
type escapeUnit struct {
	r        rune
	spelling string
}

// stringUnits splits the body of a JSON string literal (quotes included in
// raw) into decoded runes and their source spellings.
func stringUnits(raw string) []escapeUnit {
	body := strings.TrimSuffix(strings.TrimPrefix(raw, `"`), `"`)
	var out []escapeUnit
	for i := 0; i < len(body); {
		if body[i] != '\\' || i+1 >= len(body) {
			r, size := utf8.DecodeRuneInString(body[i:])
			out = append(out, escapeUnit{r: r, spelling: body[i : i+size]})
			i += size
			continue
		}
		r, size := decodeEscape(body[i:])
		out = append(out, escapeUnit{r: r, spelling: body[i : i+size]})
		i += size
	}
	return out
}

// decodeEscape decodes one backslash escape at the start of s, combining a
// UTF-16 surrogate pair into one rune.
func decodeEscape(s string) (rune, int) {
	switch s[1] {
	case 'u':
		r1, ok := hex4(s, 2)
		if !ok {
			return utf8.RuneError, 2
		}
		if utf16.IsSurrogate(r1) && len(s) >= 12 && s[6] == '\\' && s[7] == 'u' {
			if r2, ok := hex4(s, 8); ok {
				if r := utf16.DecodeRune(r1, r2); r != utf8.RuneError {
					return r, 12
				}
			}
		}
		return r1, 6
	case 'b':
		return '\b', 2
	case 'f':
		return '\f', 2
	case 'n':
		return '\n', 2
	case 'r':
		return '\r', 2
	case 't':
		return '\t', 2
	default:
		return rune(s[1]), 2
	}
}

func hex4(s string, at int) (rune, bool) {
	if len(s) < at+4 {
		return 0, false
	}
	v, err := strconv.ParseUint(s[at:at+4], 16, 32)
	if err != nil {
		return 0, false
	}
	return rune(v), true
}

// stringSpellingClass classifies the spelling difference between two JSON
// string literals that decode to the same string. It returns "" when the
// spellings are identical.
func stringSpellingClass(rawGo, rawRust string) string {
	if rawGo == rawRust {
		return ""
	}
	a, b := stringUnits(rawGo), stringUnits(rawRust)
	if len(a) != len(b) {
		return parity.LexicalOther
	}
	class := ""
	for i := range a {
		if a[i].spelling == b[i].spelling {
			continue
		}
		if a[i].r == '<' || a[i].r == '>' || a[i].r == '&' {
			return parity.LexicalHTMLEscape
		}
		class = parity.LexicalUnicodeEscape
	}
	if class == "" {
		return parity.LexicalOther
	}
	return class
}

// trailingClass compares the whitespace after two document roots.
func trailingClass(trailGo, trailRust string) string {
	if trailGo == trailRust {
		return ""
	}
	if strings.TrimSuffix(trailGo, "\n") == strings.TrimSuffix(trailRust, "\n") {
		return parity.LexicalTrailingNewline
	}
	return parity.LexicalWhitespace
}

// commonMemberOrder returns the order of the member names both objects have,
// as each side spells it.
func commonMemberOrder(g, r *node) ([]string, []string) {
	inRust := make(map[string]bool, len(r.members))
	for _, k := range r.keys() {
		inRust[k] = true
	}
	inGo := make(map[string]bool, len(g.members))
	var goOrder []string
	for _, k := range g.keys() {
		inGo[k] = true
		if inRust[k] {
			goOrder = append(goOrder, k)
		}
	}
	var rustOrder []string
	for _, k := range r.keys() {
		if inGo[k] {
			rustOrder = append(rustOrder, k)
		}
	}
	return goOrder, rustOrder
}

func equalStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// scalarSpelling returns the source spelling shown in a lexical mismatch:
// a string's literal without the surrounding quotes, or a number's token.
func scalarSpelling(n *node) string {
	if n.kind == kindString {
		return strings.TrimSuffix(strings.TrimPrefix(n.raw, `"`), `"`)
	}
	return n.raw
}
