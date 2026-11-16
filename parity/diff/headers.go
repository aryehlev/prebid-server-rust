package diff

import (
	"encoding/json"
	"net/textproto"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// headerOptions controls one header-list comparison.
type headerOptions struct {
	// caseInsensitive applies the header_names rule: names compare
	// case-insensitively and order across names is ignored; the order of the
	// values of one name is kept. Without it the lists compare exactly.
	caseInsensitive bool
	// ignore holds lower-case names ignored entirely (date_header rule).
	ignore map[string]bool
	// wire is true when both lists are exact wire captures (oracle http.out,
	// the response the handler chain wrote). Wire differences are then
	// reported as kind wire with a class, never normalized away.
	wire bool
	// request selects the transport defaults of an outbound request rather
	// than those of a response.
	request bool
}

// Framing headers are reported by the wire comparison with their own class.
const (
	headerContentLength    = "content-length"
	headerTransferEncoding = "transfer-encoding"
)

// requestTransportDefaults are request headers that HTTP clients manage
// themselves (Go's transport adds Accept-Encoding: gzip; reqwest sends
// accept: */*). A difference is a wire default_header difference.
var requestTransportDefaults = map[string]bool{
	"accept-encoding": true, "connection": true, "host": true, "te": true,
	"keep-alive": true, "proxy-connection": true,
}

// requestClientValues are the default values of headers an adapter may also
// set explicitly; only these values make a difference a default_header one.
var requestClientValues = map[string][]string{
	"user-agent": {"Go-http-client/", "reqwest/", "hyper/", "curl/"},
	"accept":     {"*/*"},
}

// responseTransportDefaults are response headers the server stack manages.
var responseTransportDefaults = map[string]bool{
	"connection": true, "keep-alive": true, "server": true,
}

// headerGroup is the values and spellings of one header name, in list order.
type headerGroup struct {
	values    []string
	spellings []string
}

func groupHeaders(list parity.HeaderList, ignore map[string]bool) (map[string]*headerGroup, []string) {
	groups := map[string]*headerGroup{}
	var order []string
	for _, kv := range list {
		lname := strings.ToLower(kv[0])
		if ignore[lname] {
			continue
		}
		g, ok := groups[lname]
		if !ok {
			g = &headerGroup{}
			groups[lname] = g
			order = append(order, lname)
		}
		g.values = append(g.values, kv[1])
		g.spellings = append(g.spellings, kv[0])
	}
	return groups, order
}

// headers compares two header lists under path (for example $['@headers']).
func (c *comparer) headers(path string, g, r parity.HeaderList, opt headerOptions) {
	gm, gOrder := groupHeaders(g, opt.ignore)
	rm, rOrder := groupHeaders(r, opt.ignore)
	if opt.wire {
		c.wireHeaders(path, gm, rm, gOrder, rOrder, opt.request)
	}
	if !opt.caseInsensitive {
		c.exactHeaders(path, g, r, opt.ignore)
		return
	}
	for _, lname := range unionNames(gm, rm) {
		gv, rv := groupValues(gm[lname]), groupValues(rm[lname])
		if opt.wire && wireOwned(lname, gv, rv, opt.request) {
			continue
		}
		if !equalStrings(gv, rv) {
			c.add(parity.KindHeader, "", child(path, canonicalHeader(lname)), headerValuesJSON(gv), headerValuesJSON(rv))
		}
	}
}

// exactHeaders compares the lists name by name and value by value, in order
// and case-sensitively (used when the header_names rule is not active).
func (c *comparer) exactHeaders(path string, g, r parity.HeaderList, ignore map[string]bool) {
	gf, rf := filterHeaders(g, ignore), filterHeaders(r, ignore)
	if len(gf) == len(rf) {
		same := true
		for i := range gf {
			same = same && gf[i] == rf[i]
		}
		if same {
			return
		}
	}
	c.add(parity.KindHeader, "", path, valueJSON(gf), valueJSON(rf))
}

func filterHeaders(list parity.HeaderList, ignore map[string]bool) parity.HeaderList {
	out := parity.HeaderList{}
	for _, kv := range list {
		if !ignore[strings.ToLower(kv[0])] {
			out = append(out, kv)
		}
	}
	return out
}

// wireHeaders reports transport-level differences: framing headers, client
// and server defaults, name spelling and the order of names.
func (c *comparer) wireHeaders(path string, gm, rm map[string]*headerGroup, gOrder, rOrder []string, request bool) {
	for _, lname := range unionNames(gm, rm) {
		gv, rv := groupValues(gm[lname]), groupValues(rm[lname])
		name := child(path, canonicalHeader(lname))
		if class := wireClass(lname, gv, rv, request); class != "" && !equalStrings(gv, rv) {
			c.add(parity.KindWire, class, name, headerValuesJSON(gv), headerValuesJSON(rv))
		}
		if gm[lname] == nil || rm[lname] == nil {
			continue
		}
		if gs, rs := distinct(gm[lname].spellings), distinct(rm[lname].spellings); !equalStrings(gs, rs) {
			c.add(parity.KindWire, parity.WireHeaderCase, name, stringsJSON(gs), stringsJSON(rs))
		}
	}
	gCommon, rCommon := commonNames(gOrder, rm), commonNames(rOrder, gm)
	if !equalStrings(gCommon, rCommon) {
		c.add(parity.KindWire, parity.WireHeaderOrder, path, stringsJSON(gCommon), stringsJSON(rCommon))
	}
}

// wireOwned reports whether a header difference belongs to the wire
// comparison rather than to the semantic header comparison.
func wireOwned(lname string, gv, rv []string, request bool) bool {
	return wireClass(lname, gv, rv, request) != ""
}

// wireClass returns the wire class of a header name, or "" for an ordinary
// header whose differences are semantic (kind header).
func wireClass(lname string, gv, rv []string, request bool) string {
	switch {
	case lname == headerContentLength:
		return parity.WireContentLength
	case lname == headerTransferEncoding:
		return parity.WireTransferEncoding
	case request && requestTransportDefaults[lname]:
		return parity.WireDefaultHeader
	case !request && responseTransportDefaults[lname]:
		return parity.WireDefaultHeader
	case request && (hasClientValue(lname, gv) || hasClientValue(lname, rv)):
		return parity.WireDefaultHeader
	}
	return ""
}

func hasClientValue(lname string, values []string) bool {
	for _, v := range values {
		for _, prefix := range requestClientValues[lname] {
			if strings.HasPrefix(v, prefix) {
				return true
			}
		}
	}
	return false
}

// distinct returns the distinct values of list in first-occurrence order.
func distinct(list []string) []string {
	seen := make(map[string]bool, len(list))
	out := make([]string, 0, len(list))
	for _, v := range list {
		if !seen[v] {
			seen[v] = true
			out = append(out, v)
		}
	}
	return out
}

func commonNames(order []string, other map[string]*headerGroup) []string {
	out := []string{}
	for _, lname := range order {
		if other[lname] != nil {
			out = append(out, lname)
		}
	}
	return out
}

func unionNames(a, b map[string]*headerGroup) []string {
	seen := make(map[string]bool, len(a)+len(b))
	out := make([]string, 0, len(a)+len(b))
	for _, m := range []map[string]*headerGroup{a, b} {
		for lname := range m {
			if !seen[lname] {
				seen[lname] = true
				out = append(out, lname)
			}
		}
	}
	sort.Strings(out)
	return out
}

func groupValues(g *headerGroup) []string {
	if g == nil {
		return nil
	}
	return g.values
}

// headerValuesJSON renders the values of one header, or null when absent.
func headerValuesJSON(values []string) json.RawMessage {
	if values == nil {
		return json.RawMessage("null")
	}
	return stringsJSON(values)
}

// canonicalHeader returns the canonical spelling used in JSON paths, so that
// signatures do not depend on either side's spelling.
func canonicalHeader(lname string) string {
	return textproto.CanonicalMIMEHeaderKey(lname)
}

// headerListNode converts a header list inside a stage value ([[name, value],
// ...]) into its case-insensitive semantic form {Canonical-Name: [values]}
// (header_names rule). Differences inside it are reported as kind header.
// It returns nil when n is not a header list.
func headerListNode(n *node) *node {
	if n == nil || n.kind != kindArray {
		return nil
	}
	out := &node{kind: kindObject, exact: false, members: []member{}, headerMap: true}
	for _, pair := range n.elems {
		if pair.kind != kindArray || len(pair.elems) != 2 ||
			pair.elems[0].kind != kindString || pair.elems[1].kind != kindString {
			return nil
		}
		lname := canonicalHeader(strings.ToLower(pair.elems[0].str))
		values := out.get(lname)
		if values == nil {
			values = &node{kind: kindArray, elems: []*node{}}
			out.set(lname, values)
		}
		values.elems = append(values.elems, pair.elems[1])
	}
	sort.SliceStable(out.members, func(i, j int) bool { return out.members[i].key < out.members[j].key })
	return out
}

// headerMap compares two header maps built by headerListNode: one header
// finding per name whose values differ.
func (c *comparer) headerMap(path string, g, r *node) {
	for _, name := range unionKeys(g, r) {
		gv, rv := g.get(name), r.get(name)
		if gv != nil && rv != nil && semanticEqual(gv, rv) {
			continue
		}
		c.add(parity.KindHeader, "", child(path, name), render(gv), render(rv))
	}
}
