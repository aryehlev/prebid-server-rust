package gen

import (
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"regexp/syntax"
	"strconv"
	"strings"
	"unicode/utf8"
)

// The bidder-params generator instantiates the JSON Schemas of
// static/bidder-params. It understands the subset those schemas use: type
// (single or list), properties, required, enum, const, oneOf, anyOf, allOf,
// not (with required), if/then/else (with required), dependencies,
// additionalProperties, minLength, maxLength, pattern, format (uuid, hostname,
// email, uri), minimum, maximum, exclusiveMinimum, exclusiveMaximum, items,
// minItems, maxItems and uniqueItems. Results are checked with the production
// params validator by the bidder catalog, which retries or falls back when a
// draw misses.

// paramSchema is one parsed bidder params schema.
type paramSchema struct {
	root obj
}

// parseParamSchema decodes a schema document.
func parseParamSchema(raw []byte) (*paramSchema, error) {
	root, err := decodeObject(raw)
	if err != nil {
		return nil, fmt.Errorf("gen: params schema: %w", err)
	}
	return &paramSchema{root: root}, nil
}

// maxSchemaDepth bounds recursion into nested schemas.
const maxSchemaDepth = 6

// optionalPropertyChance is the chance of emitting a non-required property.
const optionalPropertyChance = 0.35

// instance draws a value meant to satisfy the schema.
func (s *paramSchema) instance(r *RNG) any {
	return schemaGen{r: r}.value(s.root, 0, nil)
}

// schemaGen draws instances of schema nodes.
type schemaGen struct {
	r *RNG
}

// value draws an instance of node. avoid lists properties that must stay out
// of a generated object (the required properties of sibling oneOf branches).
func (g schemaGen) value(node obj, depth int, avoid map[string]bool) any {
	node = g.resolveCombinators(node, &avoid)
	if c, ok := node["const"]; ok {
		return deepCopy(c)
	}
	if enum, ok := node["enum"].([]any); ok && len(enum) > 0 {
		return deepCopy(Pick(g.r, enum))
	}
	if depth > maxSchemaDepth {
		return nil
	}
	switch g.pickType(node) {
	case "object":
		return g.object(node, depth, avoid)
	case "array":
		return g.array(node, depth)
	case "integer":
		return g.integer(node)
	case "number":
		return g.number(node)
	case "boolean":
		return g.r.Chance(0.5)
	case "null":
		return nil
	default:
		return g.str(node)
	}
}

// resolveCombinators merges allOf and one chosen oneOf/anyOf branch into node.
func (g schemaGen) resolveCombinators(node obj, avoid *map[string]bool) obj {
	if all, ok := node["allOf"].([]any); ok {
		merged := without(node, "allOf")
		for _, sub := range all {
			if so, ok := sub.(obj); ok {
				merged = mergeSchema(merged, so)
			}
		}
		node = merged
	}
	if one, ok := node["oneOf"].([]any); ok && len(one) > 0 {
		i := g.r.Intn(len(one))
		branch, _ := one[i].(obj)
		chosen := stringSet(requiredOf(branch))
		next := map[string]bool{}
		for k, v := range *avoid {
			next[k] = v
		}
		for j, other := range one {
			if j == i {
				continue
			}
			oo, _ := other.(obj)
			for _, k := range requiredOf(oo) {
				if !chosen[k] {
					next[k] = true
				}
			}
		}
		*avoid = next
		node = mergeSchema(without(node, "oneOf"), branch)
	}
	if anyOf, ok := node["anyOf"].([]any); ok && len(anyOf) > 0 {
		branch, _ := Pick(g.r, anyOf).(obj)
		node = mergeSchema(without(node, "anyOf"), branch)
	}
	return node
}

// pickType chooses one of the declared types, inferring one when absent.
func (g schemaGen) pickType(node obj) string {
	types := typesOf(node)
	if len(types) > 0 {
		return Pick(g.r, types)
	}
	switch {
	case node["properties"] != nil || node["required"] != nil:
		return "object"
	case node["items"] != nil:
		return "array"
	case node["minimum"] != nil || node["maximum"] != nil:
		return "integer"
	}
	return "string"
}

// object draws an object: every required property, some optional ones, then
// the properties dependencies and if/then rules add; the not rule is honoured
// by dropping an optional property.
func (g schemaGen) object(node obj, depth int, avoid map[string]bool) obj {
	props, _ := node["properties"].(obj)
	required := stringSet(requiredOf(node))
	present := map[string]bool{}
	for k := range required {
		present[k] = true
	}
	for _, k := range sortedKeys(props) {
		if !required[k] && !avoid[k] && g.r.Chance(optionalPropertyChance) {
			present[k] = true
		}
	}
	for i := 0; i < 3; i++ {
		addDependencies(node, present)
	}
	dropForNot(node, present, required)
	out := obj{}
	for _, k := range sortedKeys(present) {
		sub, _ := props[k].(obj)
		out[k] = g.value(sub, depth+1, nil)
	}
	return out
}

// addDependencies adds the properties dependencies and if/then rules require.
func addDependencies(node obj, present map[string]bool) {
	if deps, ok := node["dependencies"].(obj); ok {
		for _, k := range sortedKeys(deps) {
			if !present[k] {
				continue
			}
			switch d := deps[k].(type) {
			case []any:
				for _, name := range d {
					if s, ok := name.(string); ok {
						present[s] = true
					}
				}
			case obj:
				for _, s := range requiredOf(d) {
					present[s] = true
				}
			}
		}
	}
	cond, _ := node["if"].(obj)
	if cond == nil {
		return
	}
	holds := true
	for _, k := range requiredOf(cond) {
		holds = holds && present[k]
	}
	branch := "else"
	if holds {
		branch = "then"
	}
	if b, ok := node[branch].(obj); ok {
		for _, s := range requiredOf(b) {
			present[s] = true
		}
	}
}

// dropForNot removes one optional property when every property of a
// not.required list is present.
func dropForNot(node obj, present, required map[string]bool) {
	not, _ := node["not"].(obj)
	names := requiredOf(not)
	if len(names) == 0 {
		return
	}
	for _, k := range names {
		if !present[k] {
			return
		}
	}
	for _, k := range names {
		if !required[k] {
			delete(present, k)
			return
		}
	}
}

func (g schemaGen) array(node obj, depth int) []any {
	lo, hi := intBound(node, "minItems", 1), intBound(node, "maxItems", 3)
	if lo == 0 && hi > 0 {
		lo = 1
	}
	if hi > 3 && lo <= 3 {
		hi = 3
	}
	n := g.r.Range(lo, hi)
	out := make([]any, 0, n)
	seen := map[string]bool{}
	unique, _ := node["uniqueItems"].(bool)
	for attempts := 0; len(out) < n && attempts < 4*n+4; attempts++ {
		item := obj{}
		switch it := node["items"].(type) {
		case obj:
			item = it
		case []any:
			if len(out) < len(it) {
				item, _ = it[len(out)].(obj)
			}
		}
		v := g.value(item, depth+1, nil)
		key, _ := encodeTree(v)
		if unique && seen[string(key)] {
			continue
		}
		seen[string(key)] = true
		out = append(out, v)
	}
	return out
}

func (g schemaGen) integer(node obj) json.Number {
	lo, hi := numericBounds(node, true)
	span := hi - lo
	if span > 1e6 {
		span = 1e6
	}
	return num(int64(lo) + int64(g.r.Uint64n(uint64(span)+1)))
}

func (g schemaGen) number(node obj) json.Number {
	lo, hi := numericBounds(node, false)
	if hi-lo > 1000 {
		hi = lo + 1000
	}
	v := lo + float64(g.r.Intn(int((hi-lo)*4)+1))/4
	if v > hi {
		v = hi
	}
	return fnum(v)
}

// numericBounds returns the inclusive [lo, hi] range of an integer or number
// node. Unbounded integers default to positive IDs.
func numericBounds(node obj, integer bool) (float64, float64) {
	lo, hi := 1.0, 99999.0
	if !integer {
		lo, hi = 0.5, 100
	}
	step := 1.0
	if !integer {
		step = 0.25
	}
	if v, ok := floatOf(node["minimum"]); ok {
		lo = v
		if excl, _ := node["exclusiveMinimum"].(bool); excl {
			lo += step
		}
		if hi < lo {
			hi = lo + 1000
		}
	}
	if v, ok := floatOf(node["exclusiveMinimum"]); ok {
		lo = v + step
		if hi < lo {
			hi = lo + 1000
		}
	}
	if v, ok := floatOf(node["maximum"]); ok {
		hi = v
		if excl, _ := node["exclusiveMaximum"].(bool); excl {
			hi -= step
		}
	}
	if v, ok := floatOf(node["exclusiveMaximum"]); ok {
		hi = v - step
	}
	if integer {
		lo, hi = math.Ceil(lo), math.Floor(hi)
	}
	if hi < lo {
		hi = lo
	}
	return lo, hi
}

// str draws a string honouring format, pattern and length.
func (g schemaGen) str(node obj) string {
	minLen := intBound(node, "minLength", 0)
	maxLen := intBound(node, "maxLength", -1)
	if format, _ := node["format"].(string); format != "" {
		if s, ok := g.formatted(format); ok {
			return s
		}
	}
	if pattern, _ := node["pattern"].(string); pattern != "" {
		if s, ok := synthPattern(g.r, pattern, minLen, maxLen); ok {
			return s
		}
	}
	s := Pick(g.r, []string{"parity", "pbs", "slot", "zone", "site"}) + strconv.Itoa(g.r.Range(1, 99999))
	return fitLength(s, minLen, maxLen)
}

func (g schemaGen) formatted(format string) (string, bool) {
	switch format {
	case "uuid":
		return g.r.UUID(), true
	case "hostname":
		return fmt.Sprintf("ads%d.parity.test", g.r.Range(1, 99)), true
	case "email":
		return fmt.Sprintf("user%d@parity.test", g.r.Range(1, 99)), true
	case "uri", "uri-reference":
		return fmt.Sprintf("https://parity.test/p/%d", g.r.Range(1, 99)), true
	}
	return "", false
}

// fitLength pads or truncates s to [minLen, maxLen] runes (maxLen < 0: none).
func fitLength(s string, minLen, maxLen int) string {
	for utf8.RuneCountInString(s) < minLen {
		s += "x"
	}
	if maxLen >= 0 && utf8.RuneCountInString(s) > maxLen {
		s = string([]rune(s)[:maxLen])
	}
	return s
}

// synthPattern draws a string matching pattern within [minLen, maxLen].
func synthPattern(r *RNG, pattern string, minLen, maxLen int) (string, bool) {
	re, err := syntax.Parse(pattern, syntax.Perl)
	if err != nil {
		return "", false
	}
	re = re.Simplify()
	check, err := regexp.Compile(pattern)
	if err != nil {
		return "", false
	}
	for attempt := 0; attempt < 8; attempt++ {
		var b strings.Builder
		synthNode(r, re, &b, attempt)
		s := b.String()
		n := utf8.RuneCountInString(s)
		if check.MatchString(s) && n >= minLen && (maxLen < 0 || n <= maxLen) {
			return s, true
		}
	}
	return "", false
}

// synthNode writes one string matched by re. grow raises repetition counts so
// later attempts can reach a minimum length.
func synthNode(r *RNG, re *syntax.Regexp, b *strings.Builder, grow int) {
	switch re.Op {
	case syntax.OpLiteral:
		b.WriteString(string(re.Rune))
	case syntax.OpCharClass:
		b.WriteRune(classRune(r, re.Rune))
	case syntax.OpAnyChar, syntax.OpAnyCharNotNL:
		b.WriteByte("abcxyz0129"[r.Intn(10)])
	case syntax.OpCapture:
		synthNode(r, re.Sub[0], b, grow)
	case syntax.OpConcat:
		for _, sub := range re.Sub {
			synthNode(r, sub, b, grow)
		}
	case syntax.OpAlternate:
		synthNode(r, Pick(r, re.Sub), b, grow)
	case syntax.OpStar, syntax.OpPlus, syntax.OpQuest, syntax.OpRepeat:
		lo, hi := repeatBounds(re, grow)
		for i, n := 0, r.Range(lo, hi); i < n; i++ {
			synthNode(r, re.Sub[0], b, grow)
		}
	}
}

func repeatBounds(re *syntax.Regexp, grow int) (int, int) {
	switch re.Op {
	case syntax.OpStar:
		return grow, 3 + grow*2
	case syntax.OpPlus:
		return 1 + grow, 3 + grow*2
	case syntax.OpQuest:
		return 0, 1
	}
	hi := re.Max
	if hi < 0 {
		hi = re.Min + 2 + grow*2
	}
	return re.Min, hi
}

// classRune picks a rune of a character class, preferring ASCII letters and
// digits, then printable ASCII.
func classRune(r *RNG, ranges []rune) rune {
	var alnum, printable []rune
	for i := 0; i+1 < len(ranges); i += 2 {
		lo, hi := ranges[i], ranges[i+1]
		for c := max(lo, 0x21); c <= min(hi, 0x7e); c++ {
			printable = append(printable, c)
			if (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') {
				alnum = append(alnum, c)
			}
		}
	}
	switch {
	case len(alnum) > 0:
		return Pick(r, alnum)
	case len(printable) > 0:
		return Pick(r, printable)
	case len(ranges) > 0:
		return ranges[0]
	}
	return 'a'
}

// paramsMutant is one near-miss candidate.
type paramsMutant struct {
	label string
	value any
}

// nearMisses returns violation candidates for a valid instance, shuffled,
// ending with the top-level type violation that every object schema rejects.
func (s *paramSchema) nearMisses(r *RNG, valid any) []paramsMutant {
	root := flattenSchema(s.root)
	props, _ := root["properties"].(obj)
	var out []paramsMutant
	instance, _ := valid.(obj)
	for _, k := range sortedKeys(instance) {
		sub, _ := props[k].(obj)
		for _, m := range propertyViolations(r, sub, instance[k]) {
			out = append(out, paramsMutant{label: m.label + ":" + k, value: withMember(instance, k, m.value)})
		}
		out = append(out, paramsMutant{label: "missing:" + k, value: withoutMember(instance, k)})
	}
	for _, k := range requiredOf(root) {
		if _, ok := instance[k]; !ok {
			continue
		}
		out = append(out, paramsMutant{label: "missing_required:" + k, value: withoutMember(instance, k)})
	}
	if extra, ok := root["additionalProperties"].(bool); ok && !extra {
		out = append(out, paramsMutant{label: "additional", value: withMember(instance, "parityUnknownParam", "x")})
	}
	if names := requiredOf(mapOf(root["not"])); len(names) > 0 {
		v := withMember(instance, "", nil)
		for _, k := range names {
			sub, _ := props[k].(obj)
			v[k] = schemaGen{r: r}.value(sub, 1, nil)
		}
		out = append(out, paramsMutant{label: "not", value: v})
	}
	Shuffle(r, out)
	top := Pick(r, []any{"params", []any{}, num(1), true})
	return append(out, paramsMutant{label: "top_type", value: top})
}

// propertyViolations lists values that break one keyword of a property schema.
func propertyViolations(r *RNG, sub obj, current any) []paramsMutant {
	var out []paramsMutant
	types := typesOf(sub)
	if len(types) > 0 {
		out = append(out, paramsMutant{label: "type", value: wrongType(r, types)})
	}
	if enum, ok := sub["enum"].([]any); ok && len(enum) > 0 {
		out = append(out, paramsMutant{label: "enum", value: offEnum(enum, current)})
	}
	if pattern, _ := sub["pattern"].(string); pattern != "" {
		if v, ok := offPattern(pattern); ok {
			out = append(out, paramsMutant{label: "pattern", value: v})
		}
	}
	if n := intBound(sub, "minLength", 0); n > 0 {
		out = append(out, paramsMutant{label: "min_length", value: strings.Repeat("x", n-1)})
	}
	if n := intBound(sub, "maxLength", -1); n >= 0 {
		out = append(out, paramsMutant{label: "max_length", value: strings.Repeat("x", n+1)})
	}
	if v, ok := floatOf(sub["minimum"]); ok {
		out = append(out, paramsMutant{label: "minimum", value: fnum(v - 1)})
	}
	if v, ok := floatOf(sub["maximum"]); ok {
		out = append(out, paramsMutant{label: "maximum", value: fnum(v + 1)})
	}
	if n := intBound(sub, "minItems", 0); n > 0 {
		out = append(out, paramsMutant{label: "min_items", value: []any{}})
	}
	return out
}

// wrongType returns a value of a JSON type none of types admits.
func wrongType(r *RNG, types []string) any {
	allowed := stringSet(types)
	candidates := []struct {
		typ string
		v   any
	}{
		{"string", "parity-wrong-type"},
		{"integer", num(42)},
		{"boolean", true},
		{"object", obj{"x": num(1)}},
		{"array", []any{"x"}},
	}
	var out []any
	for _, c := range candidates {
		if allowed[c.typ] || (c.typ == "integer" && allowed["number"]) {
			continue
		}
		out = append(out, c.v)
	}
	if len(out) == 0 {
		return nil
	}
	return Pick(r, out)
}

func offEnum(enum []any, current any) any {
	if _, isString := current.(string); isString || len(enum) == 0 {
		return "parity-not-in-enum"
	}
	return num(987654)
}

// offPattern finds a short string the pattern rejects.
func offPattern(pattern string) (string, bool) {
	re, err := regexp.Compile(pattern)
	if err != nil {
		return "", false
	}
	for _, s := range []string{"", "!", " ", "-", "parity value", "x.y", "0"} {
		if !re.MatchString(s) {
			return s, true
		}
	}
	return "", false
}

// flattenSchema merges allOf and every oneOf/anyOf branch into one node so
// the near-miss generator sees every property declaration.
func flattenSchema(node obj) obj {
	out := without(node, "allOf", "oneOf", "anyOf")
	for _, key := range []string{"allOf", "oneOf", "anyOf"} {
		list, _ := node[key].([]any)
		for _, sub := range list {
			if so, ok := sub.(obj); ok {
				out = mergeSchema(out, without(so, "required"))
			}
		}
	}
	return out
}

// mergeSchema combines two schema nodes: properties merge member-wise,
// required lists union, any other keyword of b wins.
func mergeSchema(a, b obj) obj {
	out := without(a)
	for k, v := range b {
		switch k {
		case "properties":
			props := obj{}
			if pa, ok := out["properties"].(obj); ok {
				for pk, pv := range pa {
					props[pk] = pv
				}
			}
			if pb, ok := v.(obj); ok {
				for pk, pv := range pb {
					props[pk] = pv
				}
			}
			out["properties"] = props
		case "required":
			seen := stringSet(requiredOf(out))
			req := strList(requiredOf(out)...)
			for _, s := range requiredOf(b) {
				if !seen[s] {
					req = append(req, s)
					seen[s] = true
				}
			}
			out["required"] = req
		default:
			out[k] = v
		}
	}
	return out
}

// without returns a shallow copy of o minus keys.
func without(o obj, keys ...string) obj {
	out := make(obj, len(o))
	for k, v := range o {
		out[k] = v
	}
	for _, k := range keys {
		delete(out, k)
	}
	return out
}

// withMember returns a shallow copy of o with key set (key "" sets nothing).
func withMember(o obj, key string, v any) obj {
	out := without(o)
	if key != "" {
		out[key] = v
	}
	return out
}

func withoutMember(o obj, key string) obj {
	return without(o, key)
}

func mapOf(v any) obj {
	o, _ := v.(obj)
	return o
}

func typesOf(node obj) []string {
	switch t := node["type"].(type) {
	case string:
		return []string{t}
	case []any:
		var out []string
		for _, v := range t {
			if s, ok := v.(string); ok {
				out = append(out, s)
			}
		}
		return out
	}
	return nil
}

func requiredOf(node obj) []string {
	list, _ := node["required"].([]any)
	out := make([]string, 0, len(list))
	for _, v := range list {
		if s, ok := v.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

func stringSet(xs []string) map[string]bool {
	out := make(map[string]bool, len(xs))
	for _, x := range xs {
		out[x] = true
	}
	return out
}

func floatOf(v any) (float64, bool) {
	n, ok := v.(json.Number)
	if !ok {
		return 0, false
	}
	f, err := n.Float64()
	return f, err == nil
}

// intBound returns the integer keyword, or def when absent.
func intBound(node obj, key string, def int) int {
	if f, ok := floatOf(node[key]); ok {
		return int(f)
	}
	return def
}
