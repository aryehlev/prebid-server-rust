package main

import (
	"net/url"
	"strings"
)

// urlEscapeCount is the number of seeded strings of the escape sweep.
const urlEscapeCount = 50000

// urlEscapeRunes are the runes the seeded strings are made of: every ASCII byte, then runes of
// two, three and four bytes, white space outside ASCII and a byte-order mark.
var urlEscapeRunes = func() []string {
	var runes []string
	for c := 0; c < 0x80; c++ {
		runes = append(runes, string(rune(c)))
	}
	return append(runes, "\u00e9", "\u00a0", "\u65e5", "\u2028", "\ufeff", "\U0001d11e",
		"\u0130", "\ufffd")
}()

// seededText draws up to maxRunes runes of urlEscapeRunes: the count is intn(maxRunes+1), then
// each rune is urlEscapeRunes[intn(len)].
func seededText(r *splitmix64, maxRunes int) string {
	n := r.intn(maxRunes + 1)
	var b strings.Builder
	for i := 0; i < n; i++ {
		b.WriteString(urlEscapeRunes[r.intn(len(urlEscapeRunes))])
	}
	return b.String()
}

// valuesKeys are the keys a seeded url.Values draws from, so that keys repeat and sort.
var valuesKeys = []string{"", "a", "b", "B", "a b", "a+b", "~", "*", "id", "id[]", "\u00e9", "z"}

// seededValues draws a url.Values of up to 6 Add calls, each key from valuesKeys or seeded,
// and returns it with the calls as key/value pairs.
func seededValues(r *splitmix64) (url.Values, []string) {
	v := url.Values{}
	var calls []string
	n := r.intn(7)
	for i := 0; i < n; i++ {
		var k string
		if r.intn(3) == 0 {
			k = seededText(r, 4)
		} else {
			k = valuesKeys[r.intn(len(valuesKeys))]
		}
		val := seededText(r, 6)
		v.Add(k, val)
		calls = append(calls, k, val)
	}
	return v, calls
}

type escapeEdgeJSON struct {
	Input  string `json:"input"`
	Query  string `json:"query"`
	Path   string `json:"path"`
	Unesc  string `json:"query_unescape"`
	UnescE string `json:"query_unescape_error"`
}

type valuesEdgeJSON struct {
	Pairs  []string `json:"pairs"`
	Encode string   `json:"encode"`
}

type parseQueryJSON struct {
	Query  string              `json:"query"`
	Values map[string][]string `json:"values"`
	Error  string              `json:"error"`
	Get    map[string]string   `json:"get"`
}

type urlEscapeJSON struct {
	Seed           uint64           `json:"seed"`
	Count          int              `json:"count"`
	MaxRunes       int              `json:"max_runes"`
	Runes          []string         `json:"runes"`
	InputsSHA256   string           `json:"inputs_sha256"`
	OutputsSHA256  string           `json:"outputs_sha256"`
	ValuesSeed     uint64           `json:"values_seed"`
	ValuesCount    int              `json:"values_count"`
	ValuesKeys     []string         `json:"values_keys"`
	ValuesInputs   string           `json:"values_inputs_sha256"`
	ValuesOutputs  string           `json:"values_outputs_sha256"`
	Edges          []escapeEdgeJSON `json:"edges"`
	ValuesEdges    []valuesEdgeJSON `json:"values_edges"`
	ParseQueryEdge []parseQueryJSON `json:"parse_query"`
}

// genURLEscape writes urlutil_escape.json: url.QueryEscape and url.PathEscape over 50,000 seeded
// strings and url.Values.Encode over 50,000 seeded Values (SHA-256 manifests: an input is the
// string, or the Add calls' keys and values joined by tabs; an output is QueryEscape, a tab and
// PathEscape, or Encode), the escapes of every single rune and of hand-picked strings, and
// url.ParseQuery over the query strings whose parse the url.query-parse ledger entry names.
func genURLEscape() {
	out := urlEscapeJSON{Seed: 0x5EED_0101, Count: urlEscapeCount, MaxRunes: 12,
		Runes: urlEscapeRunes, ValuesSeed: 0x5EED_0102, ValuesCount: urlEscapeCount,
		ValuesKeys: valuesKeys}
	r := &splitmix64{state: out.Seed}
	m := newManifest()
	for i := 0; i < out.Count; i++ {
		s := seededText(r, out.MaxRunes)
		m.add(s, url.QueryEscape(s)+"\t"+url.PathEscape(s))
	}
	out.InputsSHA256, out.OutputsSHA256 = m.inputs(), m.outputs()

	r = &splitmix64{state: out.ValuesSeed}
	vm := newManifest()
	for i := 0; i < out.ValuesCount; i++ {
		v, calls := seededValues(r)
		vm.add(strings.Join(calls, "\t"), v.Encode())
	}
	out.ValuesInputs, out.ValuesOutputs = vm.inputs(), vm.outputs()

	edges := append([]string{}, urlEscapeRunes...)
	edges = append(edges, "", "a b", "a+b", "a%20b", "100%", "%zz", "a/b?c#d", "k=v&k2=v2",
		"~*'()!", "$&+,/:;=?@", "[]{}|\\^`", "\"<>", "\x00\x7f", "\u00e9\u65e5\U0001d11e",
		"https://example.com/p?q=1&r=a b#f", "{{.RedirectURL}}", "gdpr_consent=${GDPR}")
	for _, s := range edges {
		u, err := url.QueryUnescape(s)
		out.Edges = append(out.Edges, escapeEdgeJSON{s, url.QueryEscape(s), url.PathEscape(s), u,
			errText(err)})
	}

	for _, pairs := range [][]string{
		{},
		{"a", "1"},
		{"b", "2", "a", "1"},
		{"a", "2", "a", "1"},
		{"", "x"},
		{"a b", "c d", "a+b", "~*"},
		{"z", "", "y", "\u00e9"},
		{"id[]", "1", "id[]", "2", "ID", "3"},
	} {
		v := url.Values{}
		for i := 0; i+1 < len(pairs); i += 2 {
			v.Add(pairs[i], pairs[i+1])
		}
		out.ValuesEdges = append(out.ValuesEdges, valuesEdgeJSON{pairs, v.Encode()})
	}

	for _, q := range []string{
		"a=1&b=2", "a=1;b=2", "a=1&a=2", "a=%zz&b=2", "a=%2", "a+b=c+d", "a=1&&b", "=x", "a",
		"a=1&b", "%41=%42", "a=1%3Bb=2",
	} {
		values, err := url.ParseQuery(q)
		get := map[string]string{}
		for _, k := range []string{"a", "b", "a b", "A", ""} {
			get[k] = values.Get(k)
		}
		if values == nil {
			values = url.Values{}
		}
		out.ParseQueryEdge = append(out.ParseQueryEdge, parseQueryJSON{q, values, errText(err), get})
	}
	writeJSON("urlutil_escape.json", out)
}
