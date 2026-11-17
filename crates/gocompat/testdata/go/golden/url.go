package main

import (
	"go/build"
	"go/scanner"
	"go/token"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"unicode/utf8"
)

// lossy converts a Go string the way the Rust port stores decoded URL parts: every byte that
// does not start a valid UTF-8 sequence becomes U+FFFD ([]rune conversion semantics).
func lossy(s string) string { return string([]rune(s)) }

func boolText(b bool) string { return strconv.FormatBool(b) }

// lossyValues re-keys v with lossy strings, as the Rust Values (String keys and values) holds them.
func lossyValues(v url.Values) url.Values {
	out := url.Values{}
	for k, vs := range v {
		for _, s := range vs {
			out[lossy(k)] = append(out[lossy(k)], lossy(s))
		}
	}
	return out
}

// urlSkipped reports whether a parsed URL holds decoded bytes the Rust String model cannot
// round-trip: invalid UTF-8 in the host or userinfo (Path and Fragment keep their raw form).
func urlSkipped(u *url.URL) bool {
	if !utf8.ValidString(u.Host) {
		return true
	}
	if u.User == nil {
		return false
	}
	p, _ := u.User.Password()
	return !utf8.ValidString(u.User.Username()) || !utf8.ValidString(p)
}

// urlFields renders one url.Parse or url.ParseRequestURI result. It returns nil when the result
// is skipped (see urlSkipped).
func urlFields(fn, in string) []string {
	var u *url.URL
	var err error
	if fn == "Parse" {
		u, err = url.Parse(in)
	} else {
		u, err = url.ParseRequestURI(in)
	}
	if err != nil {
		return []string{fn, in, errText(err)}
	}
	if urlSkipped(u) {
		return nil
	}
	user := "nil"
	if u.User != nil {
		p, set := u.User.Password()
		user = u.User.Username() + ":" + p + ":" + boolText(set) + ":" + u.User.String()
	}
	// When the decoded Path/Fragment is not valid UTF-8, the Rust port holds U+FFFD in it and
	// keeps Go's EscapedPath/EscapedFragment in RawPath/RawFragment, so String still reproduces
	// Go's bytes.
	rawPath := u.RawPath
	if !utf8.ValidString(u.Path) {
		rawPath = u.EscapedPath()
	}
	rawFragment := u.RawFragment
	if !utf8.ValidString(u.Fragment) {
		rawFragment = u.EscapedFragment()
	}
	return []string{
		fn, in, "",
		u.Scheme, u.Opaque, lossy(user), u.Host, lossy(u.Path), rawPath,
		boolText(u.OmitHost), boolText(u.ForceQuery), u.RawQuery, lossy(u.Fragment), rawFragment,
		u.String(), u.EscapedPath(), u.EscapedFragment(), u.Hostname(), u.Port(), u.RequestURI(),
		boolText(u.IsAbs()), lossyValues(u.Query()).Encode(),
	}
}

// queryFields renders url.ParseQuery.
func queryFields(in string) []string {
	v, err := url.ParseQuery(in)
	return []string{"ParseQuery", in, errText(err), lossyValues(v).Encode(), strconv.Itoa(len(v))}
}

// unescapeFields renders url.QueryUnescape and url.PathUnescape (decoded bytes kept exact).
func unescapeFields(in string) []string {
	q, qerr := url.QueryUnescape(in)
	p, perr := url.PathUnescape(in)
	return []string{"Unescape", in, q, errText(qerr), p, errText(perr)}
}

// urlLine is the sweep form of a row: its fields quoted and joined by tabs.
func urlLine(fields []string) string {
	q := make([]string, len(fields))
	for i, f := range fields {
		q[i] = strconv.Quote(f)
	}
	return strings.Join(q, "\t")
}

// goStringLiterals returns the distinct string literals of a Go source file, in order of first
// appearance.
func goStringLiterals(path string) []string {
	src, err := os.ReadFile(path)
	check(err)
	fset := token.NewFileSet()
	file := fset.AddFile(path, fset.Base(), len(src))
	var s scanner.Scanner
	s.Init(file, src, nil, 0)
	var out []string
	for {
		_, tok, lit := s.Scan()
		if tok == token.EOF {
			return out
		}
		if tok != token.STRING {
			continue
		}
		v, err := strconv.Unquote(lit)
		if err == nil {
			out = append(out, v)
		}
	}
}

var bidderInfoURL = regexp.MustCompile(`^\s*(endpoint|url)\s*:\s*"?([^"]*)"?\s*$`)

// bidderInfoURLs returns the endpoint and user sync URLs of static/bidder-info.
func bidderInfoURLs() []string {
	files, err := filepath.Glob(filepath.Join("static", "bidder-info", "*.yaml"))
	check(err)
	var out []string
	for _, f := range files {
		b, err := os.ReadFile(f)
		check(err)
		for _, line := range strings.Split(string(b), "\n") {
			if m := bidderInfoURL.FindStringSubmatch(line); m != nil {
				out = append(out, m[2])
			}
		}
	}
	return out
}

var curatedURLs = []string{
	"",
	"*",
	"http://www.google.com",
	"http://www.google.com/",
	"http://www.google.com/file%20one%26two",
	"http://www.google.com/#file%20one%26two",
	"https://user:pass@example.com:8080/p/a/t/h?query=string&a=1#hash",
	"http://user@host.com",
	"http://us%20er:p%40ss@host.com/",
	"http://j@ne:password@google.com",
	"http://[fe80::1%25en0]:8080/",
	"http://[fe80::1%en0]/",
	"http://[::1]:namedport",
	"http://[::ffff:1.2.3.4]/",
	"http://[1.2.3.4]/",
	"http://[fe80::1%25]/",
	"http://[::1x]/",
	"http://[1::2::3]/",
	"http://[::1.2.3]/",
	"http://[1:2:3:4:5:6:7:8:9]/",
	"http://[12345::]/",
	"http://[::01.2.3.4]/",
	"http://[%41::1]/",
	"http://[::1",
	"http://a[::1]/",
	"http://host:1:2/",
	"mongodb://a:1,b:2/db",
	"http://%41:8080/",
	"http://%ff/",
	"http://ex ample.com/",
	"mailto:webmaster@golang.org",
	"mailto:?subject=hi",
	"//foo",
	"///threeslashes",
	"/foo?",
	"/foo?a=1?",
	"foo:bar/baz",
	"./this:that",
	"a/b:c",
	":foo",
	"1http://bad",
	"http://x/a%2Fb",
	"http://x/a%2fb%zz",
	"http://x/?q=a+b%20c&q=%E2%82%AC",
	"http://x/#frag%zz",
	"http://x/\x7f",
	"HTTP://EXAMPLE.COM/PATH",
	"http://ib.adnxs.com/openrtb2?query1&query2=val2",
	"https://{{.Host}}.example.com/{{.AccountID}}?ep={{.PublisherID}}",
	"https://prebid.example.com/setuid?bidder=x&gdpr={{.GDPR}}&uid=$UID",
	"http://localhost:8000/openrtb2/auction?debug=1",
	"https://pbs.example.com/cache?uuid=4f2c6e1b-30f9-4b33-9cfd-81bd3c5f1a77",
	"https://example.com/p;a;r;a;m/",
	"http://example.com/%7Efoo",
	"http://example.com/\u00e9t\u00e9",
	"http://\u00e9.example/",
	"http://example.com:/",
	"http://example.com:80a/",
}

var curatedQueries = []string{
	"",
	"a=1&b=2",
	"a=1&a=2&a=banana",
	"ascii=%3Ckey%3A+0x90%3E",
	"a=1;b=2",
	"a;b=1&c=2",
	"a=%3B",
	"a%3Bb=1",
	"a=1&a=2;a=banana",
	"=",
	"a=1&=2&&b",
	"x=%zz&y=1",
	"x=%e2%82%ac&y=%ff",
	"q=hello+world&e=%40",
	strings.Repeat("a&", 9999) + "a",
	strings.Repeat("a&", 10000) + "a",
}

// urlSweepTokens are the pieces random URL strings are assembled from.
var urlSweepTokens = []string{
	"a", "Z", "0", "9", "-", ".", "_", "~", ":", "/", "?", "#", "[", "]", "@", "!", "$", "&",
	"'", "(", ")", "*", "+", ",", ";", "=", "%", "%2", "%25", "%41", "%zz", "%ff", "%e2%82%ac",
	" ", "\x7f", "\u00e9", "\u20ac", "http", "https:", "http://", "x://", "mailto:", "//",
	"::1", "1.2.3.4", "localhost", ":8080", "%2F", "%3A", "\"", "<", ">", "\\", "{", "|", "^",
	"`", "{{.Host}}", "a=b", "&c=d",
}

func genURL() {
	inputs := append([]string{}, curatedURLs...)
	inputs = append(inputs, goStringLiterals(filepath.Join(build.Default.GOROOT, "src", "net", "url", "url_test.go"))...)
	inputs = append(inputs, bidderInfoURLs()...)
	seen := map[string]bool{}
	var corpus table
	var corpusList []string
	for _, in := range inputs {
		// Rust takes &str: skip invalid UTF-8, and U+FFFD so that a replacement character in a
		// decoded part always means invalid bytes.
		if seen[in] || !utf8.ValidString(in) || strings.ContainsRune(in, utf8.RuneError) {
			continue
		}
		seen[in] = true
		corpus.row(in)
		corpusList = append(corpusList, in)
	}
	corpus.write("url_corpus.txt")

	var rows table
	for _, in := range curatedURLs {
		for _, fn := range []string{"Parse", "ParseRequestURI"} {
			if f := urlFields(fn, in); f != nil {
				rows.row(f...)
			}
		}
	}
	rows.write("url_parse.txt")

	var q table
	for _, in := range curatedQueries {
		f := queryFields(in)
		if len(in) > 100 {
			// Only the outcome of the parameter-limit inputs is kept.
			f[1] = "<" + strconv.Itoa(strings.Count(in, "&")+1) + " params>"
			f[3] = "<" + strconv.Itoa(len(f[3])) + " bytes>"
		}
		q.row(f...)
	}
	q.write("url_query.txt")

	var esc table
	for c := 0; c < 256; c++ {
		s := string([]byte{byte(c)})
		esc.row(s, url.QueryEscape(s), url.PathEscape(s))
	}
	for _, s := range []string{"", "one two", "10%", " ?&=#+%!<>#\"{}|\\^[]`\u263a\t:/@$'()*,;", "\u00e9\u20ac"} {
		esc.row(s, url.QueryEscape(s), url.PathEscape(s))
	}
	esc.write("url_escape.txt")

	var unesc table
	for _, s := range []string{"", "abc", "1%41", "1%41%42%43", "%4a", "%6F", "%", "%a", "%1", "123%45%6", "%zzzzz", "a+b", "a%20b", "%e2%82%ac", "%ff%fe", "%\u00e9", "%\u20ac"} {
		unesc.row(unescapeFields(s)...)
	}
	unesc.write("url_unescape.txt")

	// Values operations replayed step by step by the Rust test.
	var vals table
	v := url.Values{}
	v.Add("b", "2")
	v.Add("a", "1 2")
	v.Set("c", "x&y=z")
	v.Add("a", "3")
	v["empty"] = []string{}
	vals.row("build", v.Encode(), v.Get("a"), boolText(v.Has("empty")), boolText(v.Has("d")))
	v.Del("a")
	v.Set("b", "\u20ac")
	vals.row("modify", v.Encode(), v.Get("a"), boolText(v.Has("a")), v.Get("b"))
	vals.write("url_values.txt")

	sums := map[string]*sweep{"url_corpus": newSweep(), "url_random": newSweep()}
	fill := func(s *sweep, in string) {
		for _, fn := range []string{"Parse", "ParseRequestURI"} {
			if f := urlFields(fn, in); f != nil {
				s.add(urlLine(f))
			}
		}
		s.add(urlLine(queryFields(in)))
		s.add(urlLine(unescapeFields(in)))
		s.add(urlLine([]string{"Escape", url.QueryEscape(in), url.PathEscape(in)}))
	}
	for _, in := range corpusList {
		fill(sums["url_corpus"], in)
	}
	rng := splitmix64{state: 2}
	for i := 0; i < 50000; i++ {
		n := int(rng.next()%12) + 1
		var b strings.Builder
		for j := 0; j < n; j++ {
			b.WriteString(urlSweepTokens[rng.next()%uint64(len(urlSweepTokens))])
		}
		fill(sums["url_random"], b.String())
	}
	var t table
	for _, name := range []string{"url_corpus", "url_random"} {
		t.row(name, sums[name].sum())
	}
	t.write("url_sweeps.txt")
}
