package gen

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sort"
	"strconv"
	"strings"
)

// The generator edits JSON as a tree of map[string]any, []any, string,
// json.Number, bool and nil. Numbers stay json.Number so their text survives a
// decode/encode round trip, and encodeTree writes object members in sorted
// order, so equal trees always encode to equal bytes.

// obj is a JSON object node.
type obj = map[string]any

// decodeTree decodes exactly one JSON value.
func decodeTree(raw []byte) (any, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, fmt.Errorf("gen: decoding JSON: %w", err)
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return nil, errors.New("gen: decoding JSON: trailing data after the value")
	}
	return v, nil
}

// decodeObject decodes a JSON object.
func decodeObject(raw []byte) (obj, error) {
	v, err := decodeTree(raw)
	if err != nil {
		return nil, err
	}
	o, ok := v.(obj)
	if !ok {
		return nil, errors.New("gen: decoding JSON: not an object")
	}
	return o, nil
}

// encodeTree encodes v compactly with sorted object members and without HTML
// escaping (the generator controls where escapes appear).
func encodeTree(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, fmt.Errorf("gen: encoding JSON: %w", err)
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

// toTree converts any JSON-marshalable value into a tree.
func toTree(v any) (any, error) {
	raw, err := json.Marshal(v)
	if err != nil {
		return nil, fmt.Errorf("gen: encoding JSON: %w", err)
	}
	return decodeTree(raw)
}

// toObject converts a JSON-marshalable struct into an object tree.
func toObject(v any) (obj, error) {
	tree, err := toTree(v)
	if err != nil {
		return nil, err
	}
	o, ok := tree.(obj)
	if !ok {
		return nil, errors.New("gen: encoding JSON: not an object")
	}
	return o, nil
}

// deepCopy copies a tree.
func deepCopy(v any) any {
	switch t := v.(type) {
	case obj:
		out := make(obj, len(t))
		for k, val := range t {
			out[k] = deepCopy(val)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			out[i] = deepCopy(val)
		}
		return out
	default:
		return v
	}
}

// child returns parent[key] as an object, creating it (or replacing a
// non-object value) when needed.
func child(parent obj, key string) obj {
	if o, ok := parent[key].(obj); ok {
		return o
	}
	o := obj{}
	parent[key] = o
	return o
}

// path returns the object at the dotted path below root, creating objects on
// the way.
func path(root obj, dotted string) obj {
	cur := root
	for _, key := range strings.Split(dotted, ".") {
		cur = child(cur, key)
	}
	return cur
}

// lookup returns the value at the dotted path, if every step is an object.
func lookup(root obj, dotted string) (any, bool) {
	var cur any = root
	for _, key := range strings.Split(dotted, ".") {
		o, ok := cur.(obj)
		if !ok {
			return nil, false
		}
		if cur, ok = o[key]; !ok {
			return nil, false
		}
	}
	return cur, true
}

// lookupObj returns the object at the dotted path, if present.
func lookupObj(root obj, dotted string) (obj, bool) {
	v, ok := lookup(root, dotted)
	if !ok {
		return nil, false
	}
	o, ok := v.(obj)
	return o, ok
}

// lookupString returns the string at the dotted path, if present.
func lookupString(root obj, dotted string) string {
	v, _ := lookup(root, dotted)
	s, _ := v.(string)
	return s
}

// impsOf returns the imp objects of a request tree.
func impsOf(req obj) []obj {
	list, _ := req["imp"].([]any)
	out := make([]obj, 0, len(list))
	for _, v := range list {
		if o, ok := v.(obj); ok {
			out = append(out, o)
		}
	}
	return out
}

// impBidders returns imp.ext.prebid.bidder of an imp, creating it.
func impBidders(imp obj) obj {
	return path(imp, "ext.prebid.bidder")
}

// sortedKeys returns the keys of an object in sorted order.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// num returns an integer JSON number.
func num(n int64) json.Number {
	return json.Number(strconv.FormatInt(n, 10))
}

// fnum returns the shortest JSON number that round-trips f.
func fnum(f float64) json.Number {
	return json.Number(strconv.FormatFloat(f, 'f', -1, 64))
}

// strList converts strings to a JSON array.
func strList(xs ...string) []any {
	out := make([]any, len(xs))
	for i, x := range xs {
		out[i] = x
	}
	return out
}

// intList converts ints to a JSON array.
func intList(xs ...int) []any {
	out := make([]any, len(xs))
	for i, x := range xs {
		out[i] = num(int64(x))
	}
	return out
}

// nodeRef addresses one value in a tree for the structural mutators.
type nodeRef struct {
	parent any // obj or []any
	key    string
	index  int
	path   string
}

// get returns the referenced value.
func (n nodeRef) get() any {
	switch p := n.parent.(type) {
	case obj:
		return p[n.key]
	case []any:
		return p[n.index]
	}
	return nil
}

// set replaces the referenced value.
func (n nodeRef) set(v any) {
	switch p := n.parent.(type) {
	case obj:
		p[n.key] = v
	case []any:
		p[n.index] = v
	}
}

// nodes lists every value below root (root excluded) in a deterministic
// depth-first order: object members by sorted key, array elements by index.
func nodes(root any) []nodeRef {
	var out []nodeRef
	var walk func(v any, p string)
	walk = func(v any, p string) {
		switch t := v.(type) {
		case obj:
			for _, k := range sortedKeys(t) {
				cp := p + "." + k
				out = append(out, nodeRef{parent: t, key: k, path: cp})
				walk(t[k], cp)
			}
		case []any:
			for i := range t {
				cp := p + "[" + strconv.Itoa(i) + "]"
				out = append(out, nodeRef{parent: t, index: i, path: cp})
				walk(t[i], cp)
			}
		}
	}
	walk(root, "$")
	return out
}

// filterNodes keeps the refs that satisfy keep.
func filterNodes(refs []nodeRef, keep func(n nodeRef) bool) []nodeRef {
	var out []nodeRef
	for _, r := range refs {
		if keep(r) {
			out = append(out, r)
		}
	}
	return out
}
