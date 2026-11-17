package diff

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"sort"
	"strconv"
	"strings"
)

// maxDepth bounds JSON nesting so hostile or broken documents cannot exhaust
// the stack. Go's encoding/json uses the same limit.
const maxDepth = 10000

// nodeKind is the JSON type of a node. kindMasked is a value that a
// normalization rule replaced (key_set member values, shape-checked values).
type nodeKind uint8

const (
	kindNull nodeKind = iota
	kindBool
	kindNumber
	kindString
	kindObject
	kindArray
	kindMasked
)

// String returns the JSON type name of k.
func (k nodeKind) String() string {
	switch k {
	case kindNull:
		return "null"
	case kindBool:
		return "boolean"
	case kindNumber:
		return "number"
	case kindString:
		return "string"
	case kindObject:
		return "object"
	case kindArray:
		return "array"
	default:
		return "masked"
	}
}

// node is one JSON value that keeps both its semantic value and, when it was
// parsed from exact bytes, its lexical form: the source spelling of scalars,
// object member order and the insignificant whitespace inside containers.
type node struct {
	kind nodeKind
	// raw is the source spelling of a scalar token (strings include quotes).
	raw string
	// str is the decoded string value.
	str string
	// num is the exact value of a number.
	num *big.Rat
	b   bool
	// members keeps object members in source order.
	members []member
	elems   []*node
	// ws is the whitespace between the container's own tokens.
	ws string
	// lead and trail are the whitespace around a document root.
	lead, trail string
	// mask names the rule that masked this value (kindMasked only).
	mask string
	// exact is true when raw, member order and ws are the exact source bytes,
	// so lexical comparison is meaningful.
	exact bool
	// unrecorded marks a document root taken from a semantic json member
	// because its side recorded no exact bytes for it.
	unrecorded bool
	// body marks the decoded content of an HTTP body; encoding is its
	// Content-Encoding (compared separately from the content).
	body     bool
	encoding string
	// headerMap marks a header list converted by the header_names rule.
	headerMap bool
}

type member struct {
	key    string
	rawKey string
	val    *node
}

// parseExact parses data as an exact document whose lexical form is compared.
func parseExact(data []byte) (*node, error) {
	return parseDocument(data, true)
}

// parseSemantic parses data whose bytes are not the implementation's exact
// output (for example a json.RawMessage re-encoded by an outer document).
func parseSemantic(data []byte) (*node, error) {
	return parseDocument(data, false)
}

func parseDocument(data []byte, exact bool) (*node, error) {
	if !json.Valid(data) {
		return nil, errors.New("invalid JSON")
	}
	p := &parser{data: data, exact: exact}
	lead := p.skipWS()
	n, err := p.value(0)
	if err != nil {
		return nil, err
	}
	n.lead = lead
	n.trail = p.skipWS()
	if p.pos != len(p.data) {
		return nil, fmt.Errorf("trailing data at offset %d", p.pos)
	}
	return n, nil
}

type parser struct {
	data  []byte
	pos   int
	exact bool
}

func (p *parser) skipWS() string {
	start := p.pos
	for p.pos < len(p.data) {
		switch p.data[p.pos] {
		case ' ', '\t', '\n', '\r':
			p.pos++
		default:
			return string(p.data[start:p.pos])
		}
	}
	return string(p.data[start:p.pos])
}

func (p *parser) value(depth int) (*node, error) {
	if depth > maxDepth {
		return nil, errors.New("JSON nested too deeply")
	}
	if p.pos >= len(p.data) {
		return nil, errors.New("unexpected end of JSON")
	}
	switch c := p.data[p.pos]; {
	case c == '{':
		return p.object(depth)
	case c == '[':
		return p.array(depth)
	case c == '"':
		return p.stringNode()
	case c == 't' || c == 'f' || c == 'n':
		return p.literal()
	default:
		return p.number()
	}
}

func (p *parser) object(depth int) (*node, error) {
	n := &node{kind: kindObject, exact: p.exact, members: []member{}}
	var ws strings.Builder
	p.pos++ // '{'
	ws.WriteString(p.skipWS())
	if p.peek() == '}' {
		p.pos++
		n.ws = ws.String()
		return n, nil
	}
	for {
		keyNode, err := p.stringNode()
		if err != nil {
			return nil, err
		}
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
		p.pos++ // ':'
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
		val, err := p.value(depth + 1)
		if err != nil {
			return nil, err
		}
		n.members = append(n.members, member{key: keyNode.str, rawKey: keyNode.raw, val: val})
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
		c := p.peek()
		p.pos++ // ',' or '}'
		if c == '}' {
			n.ws = ws.String()
			return n, nil
		}
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
	}
}

func (p *parser) array(depth int) (*node, error) {
	n := &node{kind: kindArray, exact: p.exact, elems: []*node{}}
	var ws strings.Builder
	p.pos++ // '['
	ws.WriteString(p.skipWS())
	if p.peek() == ']' {
		p.pos++
		n.ws = ws.String()
		return n, nil
	}
	for {
		val, err := p.value(depth + 1)
		if err != nil {
			return nil, err
		}
		n.elems = append(n.elems, val)
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
		c := p.peek()
		p.pos++ // ',' or ']'
		if c == ']' {
			n.ws = ws.String()
			return n, nil
		}
		ws.WriteByte('|')
		ws.WriteString(p.skipWS())
	}
}

func (p *parser) peek() byte {
	if p.pos < len(p.data) {
		return p.data[p.pos]
	}
	return 0
}

func (p *parser) stringNode() (*node, error) {
	start := p.pos
	p.pos++ // opening quote
	for p.pos < len(p.data) {
		switch p.data[p.pos] {
		case '\\':
			p.pos += 2
		case '"':
			p.pos++
			raw := string(p.data[start:p.pos])
			var s string
			if err := json.Unmarshal([]byte(raw), &s); err != nil {
				return nil, fmt.Errorf("decoding string at offset %d: %w", start, err)
			}
			return &node{kind: kindString, raw: raw, str: s, exact: p.exact}, nil
		default:
			p.pos++
		}
	}
	return nil, errors.New("unterminated string")
}

func (p *parser) literal() (*node, error) {
	for _, lit := range []string{"true", "false", "null"} {
		if strings.HasPrefix(string(p.data[p.pos:min(len(p.data), p.pos+len(lit))]), lit) {
			p.pos += len(lit)
			switch lit {
			case "null":
				return &node{kind: kindNull, raw: lit, exact: p.exact}, nil
			default:
				return &node{kind: kindBool, raw: lit, b: lit == "true", exact: p.exact}, nil
			}
		}
	}
	return nil, fmt.Errorf("invalid literal at offset %d", p.pos)
}

func (p *parser) number() (*node, error) {
	start := p.pos
	for p.pos < len(p.data) && strings.IndexByte("+-0123456789.eE", p.data[p.pos]) >= 0 {
		p.pos++
	}
	raw := string(p.data[start:p.pos])
	r, ok := new(big.Rat).SetString(raw)
	if !ok {
		return nil, fmt.Errorf("invalid number %q at offset %d", raw, start)
	}
	return &node{kind: kindNumber, raw: raw, num: r, exact: p.exact}, nil
}

// newString builds a synthesized (non-exact) string node.
func newString(s string) *node {
	return &node{kind: kindString, raw: jsonQuote(s), str: s}
}

// jsonQuote returns s as a JSON string literal.
func jsonQuote(s string) string {
	raw, err := marshalJSON(s)
	if err != nil {
		// Marshaling a string cannot fail; keep a valid literal regardless.
		return `""`
	}
	return string(raw)
}

// get returns the value of member key (the last one wins, as in encoding/json).
func (n *node) get(key string) *node {
	if n == nil || n.kind != kindObject {
		return nil
	}
	for i := len(n.members) - 1; i >= 0; i-- {
		if n.members[i].key == key {
			return n.members[i].val
		}
	}
	return nil
}

// set replaces the value of member key, or appends the member.
func (n *node) set(key string, v *node) {
	for i := len(n.members) - 1; i >= 0; i-- {
		if n.members[i].key == key {
			n.members[i].val = v
			return
		}
	}
	n.members = append(n.members, member{key: key, rawKey: jsonQuote(key), val: v})
}

// remove deletes every member named key.
func (n *node) remove(key string) {
	kept := n.members[:0]
	for _, m := range n.members {
		if m.key != key {
			kept = append(kept, m)
		}
	}
	n.members = kept
}

// keys returns the distinct member names in source order (last occurrence wins
// for position, matching get).
func (n *node) keys() []string {
	seen := make(map[string]int, len(n.members))
	for i, m := range n.members {
		seen[m.key] = i
	}
	out := make([]string, 0, len(seen))
	for i, m := range n.members {
		if seen[m.key] == i {
			out = append(out, m.key)
		}
	}
	return out
}

// sortedKeys returns the distinct member names in ascending order.
func (n *node) sortedKeys() []string {
	ks := n.keys()
	sort.Strings(ks)
	return ks
}

// canonical returns a semantic serialization of n: member order, number
// spelling and string escaping do not affect it, so two semantically equal
// values have the same canonical text. rewrite, when set, is applied to every
// string value first (used to mask generated UUIDs in sort keys).
func canonical(n *node, rewrite func(string) string) string {
	var b strings.Builder
	writeCanonical(&b, n, rewrite)
	return b.String()
}

func writeCanonical(b *strings.Builder, n *node, rewrite func(string) string) {
	if n == nil {
		b.WriteString("<absent>")
		return
	}
	switch n.kind {
	case kindNull:
		b.WriteString("null")
	case kindBool:
		b.WriteString(strconv.FormatBool(n.b))
	case kindNumber:
		b.WriteString(n.num.RatString())
	case kindString:
		s := n.str
		if rewrite != nil {
			s = rewrite(s)
		}
		b.WriteString(strconv.Quote(s))
	case kindMasked:
		b.WriteString("<" + n.mask + ">")
	case kindObject:
		b.WriteByte('{')
		for i, k := range n.sortedKeys() {
			if i > 0 {
				b.WriteByte(',')
			}
			b.WriteString(strconv.Quote(k))
			b.WriteByte(':')
			writeCanonical(b, n.get(k), rewrite)
		}
		b.WriteByte('}')
	case kindArray:
		b.WriteByte('[')
		for i, e := range n.elems {
			if i > 0 {
				b.WriteByte(',')
			}
			writeCanonical(b, e, rewrite)
		}
		b.WriteByte(']')
	}
}

// semanticEqual reports whether a and b are the same JSON value.
func semanticEqual(a, b *node) bool {
	return canonical(a, nil) == canonical(b, nil)
}

// render returns n as JSON for a mismatch record. Scalars keep their source
// spelling; masked values render as a descriptive string; nil renders as null.
func render(n *node) json.RawMessage {
	if n == nil {
		return json.RawMessage("null")
	}
	var b strings.Builder
	writeRender(&b, n)
	return json.RawMessage(b.String())
}

func writeRender(b *strings.Builder, n *node) {
	switch n.kind {
	case kindNull, kindBool, kindNumber, kindString:
		b.WriteString(n.raw)
	case kindMasked:
		b.WriteString(jsonQuote("<" + n.mask + ">"))
	case kindObject:
		b.WriteByte('{')
		for i, m := range n.members {
			if i > 0 {
				b.WriteByte(',')
			}
			b.WriteString(m.rawKey)
			b.WriteByte(':')
			writeRender(b, m.val)
		}
		b.WriteByte('}')
	case kindArray:
		b.WriteByte('[')
		for i, e := range n.elems {
			if i > 0 {
				b.WriteByte(',')
			}
			writeRender(b, e)
		}
		b.WriteByte(']')
	}
}

// marshalJSON is json.Marshal without HTML escaping, so values the differ
// synthesizes for mismatch records (spellings, header values, URLs) read as
// they were recorded instead of with <, > and & escaped.
func marshalJSON(v any) ([]byte, error) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(b.Bytes(), []byte("\n")), nil
}

// fromValue converts any Go value to a synthesized (non-exact) node through
// encoding/json. It is used for typed trace members (errors, outbound calls)
// whose wire spelling is not an implementation output.
func fromValue(v any) (*node, error) {
	raw, err := marshalJSON(v)
	if err != nil {
		return nil, fmt.Errorf("marshaling %T: %w", v, err)
	}
	return parseSemantic(raw)
}

// walk calls fn for every node of the tree in canonical order (object members
// sorted by name), parents before children.
func walk(n *node, fn func(*node)) {
	if n == nil {
		return
	}
	fn(n)
	switch n.kind {
	case kindObject:
		for _, k := range n.sortedKeys() {
			walk(n.get(k), fn)
		}
	case kindArray:
		for _, e := range n.elems {
			walk(e, fn)
		}
	}
}
