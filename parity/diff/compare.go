package diff

import (
	"encoding/json"
	"sort"

	"github.com/prebid/prebid-server/v4/parity"
)

// finding is one difference before categorization (domain, requirement,
// signature and primary marking are added by the categorizer).
type finding struct {
	stage   parity.StageID
	key     string
	kind    parity.MismatchKind
	class   string
	path    string
	goVal   json.RawMessage
	rustVal json.RawMessage
	// outbound is the outbound kind of an http.out finding (categorization hint).
	outbound string
	// instrumentation marks a stage_missing finding caused by a Rust stage that
	// reports "unimplemented": a gap in capture, not a behavioural divergence.
	instrumentation bool
}

// comparer walks two normalized values and records findings. Semantic rules:
// objects compare by member name, numbers by exact value, strings by decoded
// content, arrays by position after alignment (array order is significant
// unless a normalization rule sorted the array). Lexical rules run on
// semantically equal values that both sides produced as exact bytes.
type comparer struct {
	stage    parity.StageID
	key      string
	outbound string
	// memberOrderSemantic is true when the json_member_order rule is inactive
	// for the stage, which makes object member order a semantic difference.
	memberOrderSemantic bool
	findings            []finding
	wsReported          bool
}

// maxAlignCells bounds the LCS table used to align arrays; larger arrays are
// compared by position.
const maxAlignCells = 1 << 18

func (c *comparer) add(kind parity.MismatchKind, class, path string, g, r json.RawMessage) {
	c.findings = append(c.findings, finding{
		stage: c.stage, key: c.key, kind: kind, class: class, path: path,
		goVal: g, rustVal: r, outbound: c.outbound,
	})
}

// document compares two document roots (a stage entry value): the whitespace
// around them, then their content. Lexical whitespace is reported once per
// document.
func (c *comparer) document(path string, g, r *node) {
	c.wsReported = false
	c.roots(path, g, r)
	c.value(path, g, r, fieldPlain)
}

// roots compares the whitespace before and after two exact document roots. A
// root one side recorded as exact bytes and the other only as semantic json
// is reported as lexical "unrecorded": its byte-level parity, which gates like
// every lexical difference, cannot be checked.
func (c *comparer) roots(path string, g, r *node) {
	if g == nil || r == nil {
		return
	}
	if (g.exact && r.unrecorded) || (r.exact && g.unrecorded) {
		c.add(parity.KindLexical, parity.LexicalUnrecorded, path, recordedForm(g), recordedForm(r))
		return
	}
	if !g.exact || !r.exact {
		return
	}
	if class := trailingClass(g.trail, r.trail); class != "" {
		c.add(parity.KindLexical, class, path, json.RawMessage(jsonQuote(g.trail)), json.RawMessage(jsonQuote(r.trail)))
	}
	if g.lead != r.lead && !c.wsReported {
		c.wsReported = true
		c.add(parity.KindLexical, parity.LexicalWhitespace, path, json.RawMessage(jsonQuote(g.lead)), json.RawMessage(jsonQuote(r.lead)))
	}
}

// recordedForm says how a document root was recorded, for unrecorded records.
func recordedForm(n *node) json.RawMessage {
	if n.exact {
		return json.RawMessage(`"exact bytes"`)
	}
	return json.RawMessage(`"json only"`)
}

// bodyDocument compares two decoded HTTP bodies, which may be nested inside a
// stage value: content encoding, then the content as a document of its own.
func (c *comparer) bodyDocument(path string, g, r *node) {
	if g != nil && r != nil && g.encoding != r.encoding {
		c.add(parity.KindValue, "", meta(path, "@content_encoding"),
			json.RawMessage(jsonQuote(g.encoding)), json.RawMessage(jsonQuote(r.encoding)))
	}
	saved := c.wsReported
	c.wsReported = false
	c.roots(path, g, r)
	c.content(path, g, r, fieldPlain)
	c.wsReported = saved
}

// fieldKind says how a scalar difference is reported for one member.
type fieldKind uint8

const (
	fieldPlain fieldKind = iota
	fieldErrorCode
	fieldErrorText
)

func (c *comparer) value(path string, g, r *node, field fieldKind) {
	if g != nil && r != nil && (g.body || r.body) {
		c.bodyDocument(path, g, r)
		return
	}
	c.content(path, g, r, field)
}

// content compares two values without the body-level checks.
func (c *comparer) content(path string, g, r *node, field fieldKind) {
	switch {
	case g == nil && r == nil:
		return
	case g == nil:
		c.add(parity.KindExtraInRust, "", path, json.RawMessage("null"), render(r))
		return
	case r == nil:
		c.add(parity.KindMissingInRust, "", path, render(g), json.RawMessage("null"))
		return
	case g.kind != r.kind:
		c.add(typeOrValue(g, r), "", path, render(g), render(r))
		return
	}
	switch g.kind {
	case kindObject:
		c.object(path, g, r)
	case kindArray:
		c.array(path, g, r)
	default:
		c.scalar(path, g, r, field)
	}
}

func (c *comparer) scalar(path string, g, r *node, field fieldKind) {
	equal := false
	switch g.kind {
	case kindNull:
		equal = true
	case kindBool:
		equal = g.b == r.b
	case kindNumber:
		equal = g.num.Cmp(r.num) == 0
	case kindString:
		equal = g.str == r.str
	case kindMasked:
		equal = g.mask == r.mask
	}
	if !equal {
		c.add(scalarKind(field), "", path, render(g), render(r))
		return
	}
	if !g.exact || !r.exact || g.raw == r.raw {
		return
	}
	class := parity.LexicalNumberFormat
	if g.kind == kindString {
		class = stringSpellingClass(g.raw, r.raw)
	}
	if class != "" {
		c.add(parity.KindLexical, class, path,
			json.RawMessage(jsonQuote(scalarSpelling(g))), json.RawMessage(jsonQuote(scalarSpelling(r))))
	}
}

// typeOrValue is the kind of a difference between nodes of different kinds:
// type when their JSON types differ, value when a rule masked one side and
// the other has the same JSON type but falls outside the rule's shape (a
// timestamp in seconds, a fractional duration).
func typeOrValue(g, r *node) parity.MismatchKind {
	if jsonType(g) == jsonType(r) {
		return parity.KindValue
	}
	return parity.KindType
}

func jsonType(n *node) nodeKind {
	if n.kind == kindMasked {
		return n.maskedKind
	}
	return n.kind
}

func scalarKind(field fieldKind) parity.MismatchKind {
	switch field {
	case fieldErrorCode:
		return parity.KindErrorCode
	case fieldErrorText:
		return parity.KindErrorText
	default:
		return parity.KindValue
	}
}

// isErrorShaped reports whether n looks like an error record ({code, message}),
// so that code and message differences get the error_code / error_text kinds.
func isErrorShaped(n *node) bool {
	code, msg := n.get("code"), n.get("message")
	return code != nil && code.kind == kindNumber && msg != nil && msg.kind == kindString
}

func (c *comparer) object(path string, g, r *node) {
	if g.headerMap && r.headerMap {
		c.headerMap(path, g, r)
		return
	}
	errorShaped := isErrorShaped(g) && isErrorShaped(r)
	c.containerWhitespace(path, g, r, len(g.members) == len(r.members))
	keys := unionKeys(g, r)
	for _, k := range keys {
		field := fieldPlain
		if errorShaped && k == "code" {
			field = fieldErrorCode
		} else if errorShaped && k == "message" {
			field = fieldErrorText
		}
		c.value(child(path, k), g.get(k), r.get(k), field)
	}
	goOrder, rustOrder := commonMemberOrder(g, r)
	if equalStrings(goOrder, rustOrder) {
		return
	}
	switch {
	case c.memberOrderSemantic:
		c.add(parity.KindOrder, "", path, stringsJSON(goOrder), stringsJSON(rustOrder))
	case g.exact && r.exact:
		c.add(parity.KindLexical, parity.LexicalMemberOrder, path, stringsJSON(goOrder), stringsJSON(rustOrder))
	}
}

// containerWhitespace reports the first whitespace difference of a document.
func (c *comparer) containerWhitespace(path string, g, r *node, sameShape bool) {
	if c.wsReported || !sameShape || !g.exact || !r.exact || g.ws == r.ws {
		return
	}
	c.wsReported = true
	c.add(parity.KindLexical, parity.LexicalWhitespace, path, json.RawMessage(jsonQuote(g.ws)), json.RawMessage(jsonQuote(r.ws)))
}

func unionKeys(g, r *node) []string {
	seen := map[string]bool{}
	var out []string
	for _, n := range []*node{g, r} {
		for _, k := range n.keys() {
			if !seen[k] {
				seen[k] = true
				out = append(out, k)
			}
		}
	}
	sort.Strings(out)
	return out
}

func (c *comparer) array(path string, g, r *node) {
	c.containerWhitespace(path, g, r, len(g.elems) == len(r.elems))
	gk, rk := canonicalKeys(g.elems), canonicalKeys(r.elems)
	if len(gk) == len(rk) && !equalStrings(gk, rk) && sameMultiset(gk, rk) {
		c.add(parity.KindOrder, "", path, render(g), render(r))
		for i, j := range permutationPairs(gk, rk) {
			c.value(index(path, i), g.elems[i], r.elems[j], fieldPlain)
		}
		return
	}
	for _, op := range align(gk, rk) {
		switch {
		case op.goIndex >= 0 && op.rustIndex >= 0:
			c.value(index(path, op.goIndex), g.elems[op.goIndex], r.elems[op.rustIndex], fieldPlain)
		case op.goIndex >= 0:
			c.add(parity.KindMissingInRust, "", index(path, op.goIndex), render(g.elems[op.goIndex]), json.RawMessage("null"))
		default:
			c.add(parity.KindExtraInRust, "", index(path, op.rustIndex), json.RawMessage("null"), render(r.elems[op.rustIndex]))
		}
	}
}

func canonicalKeys(elems []*node) []string {
	out := make([]string, len(elems))
	for i, e := range elems {
		out[i] = canonical(e, nil)
	}
	return out
}

func sameMultiset(a, b []string) bool {
	counts := make(map[string]int, len(a))
	for _, k := range a {
		counts[k]++
	}
	for _, k := range b {
		counts[k]--
		if counts[k] < 0 {
			return false
		}
	}
	return true
}

// permutationPairs maps each Go index to the first unused equal Rust index.
func permutationPairs(gk, rk []string) []int {
	used := make([]bool, len(rk))
	out := make([]int, len(gk))
	for i, k := range gk {
		for j := range rk {
			if !used[j] && rk[j] == k {
				used[j] = true
				out[i] = j
				break
			}
		}
	}
	return out
}

// alignOp pairs a Go element with a Rust element; -1 means "absent".
type alignOp struct {
	goIndex, rustIndex int
}

// align matches equal elements with a longest common subsequence, then pairs
// the elements between two matches by position, so one inserted or removed
// element does not turn every later element into a difference.
func align(gk, rk []string) []alignOp {
	anchors := lcsAnchors(gk, rk)
	var ops []alignOp
	gi, rj := 0, 0
	flush := func(gEnd, rEnd int) {
		for gi < gEnd && rj < rEnd {
			ops = append(ops, alignOp{gi, rj})
			gi++
			rj++
		}
		for ; gi < gEnd; gi++ {
			ops = append(ops, alignOp{gi, -1})
		}
		for ; rj < rEnd; rj++ {
			ops = append(ops, alignOp{-1, rj})
		}
	}
	for _, a := range anchors {
		flush(a.goIndex, a.rustIndex)
		ops = append(ops, a)
		gi, rj = a.goIndex+1, a.rustIndex+1
	}
	flush(len(gk), len(rk))
	return ops
}

// lcsAnchors returns the index pairs of a longest common subsequence of equal
// keys, or nothing when the arrays are too large to align.
func lcsAnchors(a, b []string) []alignOp {
	n, m := len(a), len(b)
	if n == 0 || m == 0 || n*m > maxAlignCells {
		return nil
	}
	table := make([]int32, (n+1)*(m+1))
	at := func(i, j int) *int32 { return &table[i*(m+1)+j] }
	for i := n - 1; i >= 0; i-- {
		for j := m - 1; j >= 0; j-- {
			switch {
			case a[i] == b[j]:
				*at(i, j) = *at(i+1, j+1) + 1
			case *at(i+1, j) >= *at(i, j+1):
				*at(i, j) = *at(i+1, j)
			default:
				*at(i, j) = *at(i, j+1)
			}
		}
	}
	var out []alignOp
	for i, j := 0, 0; i < n && j < m; {
		switch {
		case a[i] == b[j]:
			out = append(out, alignOp{i, j})
			i++
			j++
		case *at(i+1, j) >= *at(i, j+1):
			i++
		default:
			j++
		}
	}
	return out
}

func stringsJSON(list []string) json.RawMessage {
	raw, err := marshalJSON(list)
	if err != nil {
		return json.RawMessage("null")
	}
	return raw
}

// valueJSON marshals v for a mismatch record, falling back to null.
func valueJSON(v any) json.RawMessage {
	raw, err := marshalJSON(v)
	if err != nil {
		return json.RawMessage("null")
	}
	return raw
}
