package main

import (
	"strconv"
	"strings"
	"unicode"
)

var caseInputs = []string{
	"", "abc", "ABC", "MiXeD 123", "\u0130stanbul", "\u0131", "stra\u00dfe", "STRASSE",
	"\u01c5", "\u01c4\u01c6", "\u03a3\u0391\u03a3", "\u03c3\u03c2", "\u1e9e", "\ufb03", "\u212a",
	"\u00b5", "\u0345", "\u2126", "\u10d0", "\u1c90", "\U00010400", "\U0001e922", "\u0587",
}

var equalFoldPairs = [][2]string{
	{"Go", "GO"}, {"go", "gO"}, {"abc", "abd"}, {"abc", "ab"}, {"", ""}, {"a", ""},
	{"\u212a", "k"}, {"\u212a", "K"}, {"\u017f", "s"}, {"\u017f", "S"}, {"\u00df", "SS"},
	{"\u03c3", "\u03c2"}, {"\u03a3", "\u03c2"}, {"\u0130", "i"}, {"\u0131", "I"},
	{"\u01c4", "\u01c5"}, {"\u01c5", "\u01c6"}, {"\u00b5", "\u039c"}, {"\u0345", "\u03b9"},
	{"one\u00e9", "ONE\u00c9"}, {"\u00e9", "e"}, {"abc\u212a", "ABCk"}, {"x\u212a", "XK\u212a"},
}

var spaceInputs = []string{
	"", " ", "  a  ", "\t\n\v\f\r a b \r\n", "\u0085a\u00a0", "\u2000\u3000x\u2028\u2029",
	"\u200bzero-width\u200b", "\u180emongolian\u180e", "a\u1680b", "\ufeffbom\ufeff",
	"1 2  3   4", "\u00a0\u00a0", "x\x1cy\x1d",
}

var splitCases = [][2]string{
	{"a,b,c", ","}, {"a,b,c,", ","}, {",", ","}, {"", ","}, {"", ""}, {"abc", ""},
	{"\u00e9t\u00e9", ""}, {"a::b::", "::"}, {"aaa", "aa"}, {"no-sep", "|"}, {"_x_y", "_"},
	{"http://host:8080/p", ":"},
}

var replacerCases = [][]string{
	{".", "_", "-", "_"},
	{"a", "1", "aa", "2", "aaa", "3"},
	{"aaa", "3", "aa", "2", "a", "1"},
	{"a", "x", "a", "y"},
	{"", "X"},
	{"", "X", "b", "B"},
	{"${AUCTION_PRICE}", "1.23", "${AUCTION_ID}", "id"},
	{"\"", "\\\"", "\\", "\\\\"},
	{"\u00e9", "e", "\u20ac", "EUR"},
}

var replacerInputs = []string{
	"", "a", "aaaa", "a.b-c", "ab", "price=${AUCTION_PRICE}&id=${AUCTION_ID}&x=${AUCTION}",
	"say \"hi\" \\ there", "caf\u00e9 \u20ac5", "\u00e9",
}

func genStrings() {
	var t table
	for _, s := range caseInputs {
		t.row("case", s, strings.ToLower(s), strings.ToUpper(s))
	}
	for _, p := range equalFoldPairs {
		t.row("fold", p[0], p[1], boolText(strings.EqualFold(p[0], p[1])))
	}
	for _, s := range spaceInputs {
		t.row("space", s, strings.TrimSpace(s), strings.Join(strings.Fields(s), "|"), strconv.Itoa(len(strings.Fields(s))))
	}
	for _, c := range splitCases {
		for n := -1; n <= 3; n++ {
			t.row("split", c[0], c[1], strconv.Itoa(n),
				quoteAll(strings.SplitN(c[0], c[1], n)), quoteAll(strings.SplitAfterN(c[0], c[1], n)))
		}
		t.row("split", c[0], c[1], "all", quoteAll(strings.Split(c[0], c[1])), quoteAll(strings.SplitAfter(c[0], c[1])))
	}
	for i, oldnew := range replacerCases {
		r := strings.NewReplacer(oldnew...)
		for _, s := range replacerInputs {
			t.row("replace", strconv.Itoa(i), s, lossy(r.Replace(s)))
		}
	}
	t.write("strings.txt")

	// Every rune once: lower, upper, and EqualFold against its SimpleFold orbit neighbours.
	sw := newSweep()
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if r >= 0xD800 && r <= 0xDFFF {
			continue
		}
		s := string(r)
		f := string(unicode.SimpleFold(r))
		sw.add(strings.ToLower(s) + "\t" + strings.ToUpper(s) + "\t" + boolText(strings.EqualFold(s, f)) +
			boolText(strings.EqualFold(s+"a", "A"+f)) + boolText(strings.EqualFold(s, strings.ToUpper(s))))
	}
	var sums table
	sums.row("strings_runes", sw.sum())
	sums.write("strings_sweeps.txt")
}

// quoteAll renders a []string as its strconv.Quote'd elements joined by commas.
func quoteAll(parts []string) string {
	q := make([]string, len(parts))
	for i, p := range parts {
		q[i] = strconv.Quote(p)
	}
	return "[" + strings.Join(q, ",") + "]"
}
