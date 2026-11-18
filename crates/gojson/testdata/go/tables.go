package main

import (
	"bufio"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"unicode"
)

// writeUnicode writes Go's case mapping and case folding where they differ from the identity,
// and the ranges of strconv.IsPrint and of letters and digits, for every rune: what the Rust
// tests check src/unicode against.
func writeUnicode(dir string) {
	g := create(dir, "unicode.txt", "lower\trune\tunicode.ToLower (runes it changes)\n"+
		"upper\trune\tunicode.ToUpper (runes it changes)\n"+
		"fold\trune\tencoding/json foldRune (runes it changes)\n"+
		"print\tfirst\tlast (ranges where strconv.IsPrint is true)\n"+
		"letterdigit\tfirst\tlast (ranges where unicode.IsLetter or unicode.IsDigit is true)\n"+
		"Runes are hexadecimal; Go "+unicode.Version+" tables.")
	defer g.close()
	hex := func(r rune) []byte { return []byte(fmt.Sprintf("%x", r)) }
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if l := unicode.ToLower(r); l != r {
			g.row([]byte("lower"), hex(r), hex(l))
		}
		if u := unicode.ToUpper(r); u != r {
			g.row([]byte("upper"), hex(r), hex(u))
		}
		if f := foldRune(r); f != r {
			g.row([]byte("fold"), hex(r), hex(f))
		}
	}
	for _, r := range runeRanges(strconv.IsPrint) {
		g.row([]byte("print"), hex(r[0]), hex(r[1]))
	}
	for _, r := range runeRanges(isLetterOrDigit) {
		g.row([]byte("letterdigit"), hex(r[0]), hex(r[1]))
	}
}

// foldRune is encoding/json's (fold.go): the smallest rune of r's case-folding orbit.
func foldRune(r rune) rune {
	for {
		r2 := unicode.SimpleFold(r)
		if r2 <= r {
			return r2
		}
		r = r2
	}
}

// writeTables writes src/unicode/tables.rs: this Go version's Unicode tables that pbs-gojson
// needs to map, quote and classify runes exactly as Go does.
func writeTables(path string) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		log.Fatal(err)
	}
	f, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(f)
	fmt.Fprintf(w, "//! Go's Unicode %s tables (%s), written by `testdata/go`; do not edit.\n",
		unicode.Version, runtime.Version())
	w.WriteString("//!\n")
	w.WriteString("//! The tests in `unicode/mod.rs` check every rune against `testdata/golden/unicode.txt`.\n")
	w.WriteString("\n")
	w.WriteString("/// Go's `unicode.UpperLower`: the delta of a range that alternates upper and lower case.\n")
	fmt.Fprintf(w, "pub(super) const UPPER_LOWER: i32 = %s;\n", rustHex(unicode.UpperLower))
	w.WriteString("\n")
	w.WriteString("/// `unicode.CaseRanges`: first rune, last rune, delta to upper case, delta to lower case.\n")
	fmt.Fprintf(w, "pub(super) const CASE_RANGES: [(u32, u32, i32, i32); %d] = [\n", len(unicode.CaseRanges))
	for _, cr := range unicode.CaseRanges {
		fmt.Fprintf(w, "    (%s, %s, %s, %s),\n", rustHex(rune(cr.Lo)), rustHex(rune(cr.Hi)),
			rustDelta(cr.Delta[unicode.UpperCase]), rustDelta(cr.Delta[unicode.LowerCase]))
	}
	w.WriteString("];\n")
	writeRanges(w, "The ranges of runes that `strconv.IsPrint` accepts.", "PRINT", strconv.IsPrint)
	writeRanges(w, "The ranges of runes that are `unicode.IsLetter` or `unicode.IsDigit`.",
		"LETTER_OR_DIGIT", isLetterOrDigit)
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
}

func writeRanges(w *bufio.Writer, doc, name string, in func(rune) bool) {
	ranges := runeRanges(in)
	fmt.Fprintf(w, "\n/// %s\npub(super) const %s: [(u32, u32); %d] = [\n", doc, name, len(ranges))
	for _, r := range ranges {
		fmt.Fprintf(w, "    (%s, %s),\n", rustHex(r[0]), rustHex(r[1]))
	}
	w.WriteString("];\n")
}

// isLetterOrDigit is the rune class encoding/json's isValidTag accepts besides punctuation.
func isLetterOrDigit(r rune) bool {
	return unicode.IsLetter(r) || unicode.IsDigit(r)
}

// runeRanges returns the inclusive ranges of the runes for which in is true.
func runeRanges(in func(rune) bool) [][2]rune {
	var ranges [][2]rune
	start := rune(-1)
	for r := rune(0); r <= unicode.MaxRune+1; r++ {
		if r <= unicode.MaxRune && in(r) {
			if start < 0 {
				start = r
			}
			continue
		}
		if start >= 0 {
			ranges = append(ranges, [2]rune{start, r - 1})
			start = -1
		}
	}
	return ranges
}

// rustHex formats r as a Rust hexadecimal literal in groups of four digits.
func rustHex(r rune) string {
	s := fmt.Sprintf("%X", r)
	for len(s)%4 != 0 {
		s = "0" + s
	}
	groups := make([]string, 0, len(s)/4)
	for i := 0; i < len(s); i += 4 {
		groups = append(groups, s[i:i+4])
	}
	return "0x" + strings.Join(groups, "_")
}

// rustDelta formats a case delta as a Rust literal (UPPER_LOWER for alternating ranges).
func rustDelta(d rune) string {
	if d == unicode.UpperLower {
		return "UPPER_LOWER"
	}
	s := strconv.Itoa(int(d))
	digits := strings.TrimPrefix(s, "-")
	if len(digits) <= 5 {
		return s
	}
	var b strings.Builder
	b.WriteString(s[:len(s)-len(digits)])
	for i, c := range digits {
		if i > 0 && (len(digits)-i)%3 == 0 {
			b.WriteByte('_')
		}
		b.WriteRune(c)
	}
	return b.String()
}
