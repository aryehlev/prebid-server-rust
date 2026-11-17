package minimize

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
)

// nodeKind is the JSON type of a tree node.
type nodeKind uint8

const (
	kindObject nodeKind = iota
	kindArray
	kindString
	kindNumber
	kindBool
	kindNull
)

// node is one value of an order-preserving JSON tree. Objects keep their
// member order (and duplicate names) and numbers keep their spelling, so an
// unedited tree encodes to the same members in the same order: the minimizer
// only ever changes what an edit says.
type node struct {
	id      int
	depth   int
	kind    nodeKind
	members []member
	elems   []*node
	str     string
	num     json.Number
	boolean bool
}

type member struct {
	key   string
	value *node
}

// parseTree parses one JSON document and numbers its nodes in preorder.
func parseTree(raw []byte) (*node, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	ids := 0
	root, err := parseValue(dec, &ids, 0)
	if err != nil {
		return nil, err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errors.New("minimize: trailing data after the JSON body")
	}
	return root, nil
}

func parseValue(dec *json.Decoder, ids *int, depth int) (*node, error) {
	tok, err := dec.Token()
	if err != nil {
		return nil, fmt.Errorf("minimize: parsing body: %w", err)
	}
	n := &node{id: *ids, depth: depth}
	*ids++
	switch v := tok.(type) {
	case json.Delim:
		return parseContainer(dec, ids, n, v)
	case string:
		n.kind, n.str = kindString, v
	case json.Number:
		n.kind, n.num = kindNumber, v
	case bool:
		n.kind, n.boolean = kindBool, v
	case nil:
		n.kind = kindNull
	default:
		return nil, fmt.Errorf("minimize: unexpected JSON token %v", tok)
	}
	return n, nil
}

func parseContainer(dec *json.Decoder, ids *int, n *node, open json.Delim) (*node, error) {
	if open == '[' {
		n.kind, n.elems = kindArray, []*node{}
		for dec.More() {
			child, err := parseValue(dec, ids, n.depth+1)
			if err != nil {
				return nil, err
			}
			n.elems = append(n.elems, child)
		}
		_, err := dec.Token()
		return n, err
	}
	n.kind, n.members = kindObject, []member{}
	for dec.More() {
		keyTok, err := dec.Token()
		if err != nil {
			return nil, fmt.Errorf("minimize: parsing body: %w", err)
		}
		key, ok := keyTok.(string)
		if !ok {
			return nil, fmt.Errorf("minimize: object key %v is not a string", keyTok)
		}
		child, err := parseValue(dec, ids, n.depth+1)
		if err != nil {
			return nil, err
		}
		n.members = append(n.members, member{key: key, value: child})
	}
	_, err := dec.Token()
	return n, err
}

// changes are the node-level operations of a set of applied edits.
type changes struct {
	del     map[int]bool
	minimal map[int]bool
	shorten map[int]bool
}

func newChanges() changes {
	return changes{del: map[int]bool{}, minimal: map[int]bool{}, shorten: map[int]bool{}}
}

// encode writes the tree with ch applied, compact and without HTML escaping.
func (n *node) encode(b *bytes.Buffer, ch changes) {
	if ch.minimal[n.id] {
		b.WriteString(minimalLiteral(n.kind))
		return
	}
	switch n.kind {
	case kindObject:
		b.WriteByte('{')
		first := true
		for _, m := range n.members {
			if ch.del[m.value.id] {
				continue
			}
			if !first {
				b.WriteByte(',')
			}
			first = false
			writeString(b, m.key)
			b.WriteByte(':')
			m.value.encode(b, ch)
		}
		b.WriteByte('}')
	case kindArray:
		b.WriteByte('[')
		first := true
		for _, e := range n.elems {
			if ch.del[e.id] {
				continue
			}
			if !first {
				b.WriteByte(',')
			}
			first = false
			e.encode(b, ch)
		}
		b.WriteByte(']')
	default:
		n.encodeScalar(b, ch)
	}
}

func (n *node) encodeScalar(b *bytes.Buffer, ch changes) {
	switch n.kind {
	case kindString:
		s := n.str
		if ch.shorten[n.id] {
			s = shortened(s)
		}
		writeString(b, s)
	case kindNumber:
		b.WriteString(n.num.String())
	case kindBool:
		if n.boolean {
			b.WriteString("true")
		} else {
			b.WriteString("false")
		}
	default:
		b.WriteString("null")
	}
}

// minimalLiteral is the smallest value of a JSON type.
func minimalLiteral(k nodeKind) string {
	switch k {
	case kindObject:
		return "{}"
	case kindArray:
		return "[]"
	case kindString:
		return `""`
	case kindNumber:
		return "0"
	case kindBool:
		return "false"
	}
	return "null"
}

// isMinimal reports whether the node already is the minimal value of its type.
func (n *node) isMinimal() bool {
	switch n.kind {
	case kindObject:
		return len(n.members) == 0
	case kindArray:
		return len(n.elems) == 0
	case kindString:
		return n.str == ""
	case kindNumber:
		return n.num.String() == "0"
	case kindBool:
		return !n.boolean
	}
	return true
}

// shortened keeps the first half of s (rounded down, on a rune boundary).
func shortened(s string) string {
	r := []rune(s)
	return string(r[:len(r)/2])
}

// writeString writes s as a JSON string without HTML escaping.
func writeString(b *bytes.Buffer, s string) {
	var tmp bytes.Buffer
	enc := json.NewEncoder(&tmp)
	enc.SetEscapeHTML(false)
	// Encoding a Go string cannot fail.
	_ = enc.Encode(s)
	b.WriteString(strings.TrimSuffix(tmp.String(), "\n"))
}

// walk visits every node in preorder with its JSON path.
func (n *node) walk(path string, visit func(n *node, path string)) {
	visit(n, path)
	switch n.kind {
	case kindObject:
		for _, m := range n.members {
			m.value.walk(memberPath(path, m.key), visit)
		}
	case kindArray:
		for i, e := range n.elems {
			e.walk(fmt.Sprintf("%s[%d]", path, i), visit)
		}
	}
}

// member returns the first member named key of an object node.
func (n *node) member(key string) (*node, bool) {
	if n == nil || n.kind != kindObject {
		return nil, false
	}
	for _, m := range n.members {
		if m.key == key {
			return m.value, true
		}
	}
	return nil, false
}

// memberPath appends a member name to a path: .name for identifier-like
// names, ['name'] otherwise.
func memberPath(path, key string) string {
	if key != "" && strings.IndexFunc(key, func(r rune) bool {
		return !(r == '_' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9')
	}) < 0 {
		return path + "." + key
	}
	return path + "['" + strings.ReplaceAll(key, "'", `\'`) + "']"
}
