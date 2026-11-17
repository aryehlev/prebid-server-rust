package main

// Generated documents of every contract type the crate decodes and writes
// (documents_fuzz.json): random values of the Go types (nil, empty and filled
// slices, maps and pointers), marshaled, then rewritten the ways a runner may
// send them (member order, whitespace, escapes, key case, unknown, missing,
// repeated and null members, number forms, short and long header pairs, raw
// invalid UTF-8, wrong value kinds, broken syntax). Each case records what
// json.Unmarshal makes of the input: its error, or the document Go writes back.

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"sort"
	"strings"
	"unicode"

	"github.com/prebid/prebid-server/v4/parity"
)

// fuzzDocsPerType is the number of generated documents of each type.
const fuzzDocsPerType = 120

var (
	rawMessageType  = reflect.TypeOf(json.RawMessage(nil))
	stageIDType     = reflect.TypeOf(parity.StageID(""))
	decisionIDType  = reflect.TypeOf(parity.DecisionID(""))
	stageStatusType = reflect.TypeOf(parity.StageStatus(""))
)

// fill sets v to a random value of its type. The registry types (stage and
// decision IDs, stage statuses) take registered values: the Rust types are
// closed enums, a documented difference this golden does not exercise.
func (f *fuzzer) fill(v reflect.Value, depth int) error {
	t := v.Type()
	switch t {
	case rawMessageType:
		if f.chance(25) {
			return nil
		}
		v.SetBytes([]byte(f.jsonText(f.chance(30))))
		return nil
	case stageIDType:
		all := parity.AllStages()
		v.SetString(string(all[f.intn(len(all))]))
		return nil
	case decisionIDType:
		all := parity.AllDecisions()
		v.SetString(string(all[f.intn(len(all))]))
		return nil
	case stageStatusType:
		v.SetString(f.pick(string(parity.StageCaptured), string(parity.StageNotReached), string(parity.StageUnimplemented)))
		return nil
	}
	switch t.Kind() {
	case reflect.String:
		v.SetString(f.text())
	case reflect.Bool:
		v.SetBool(f.chance(50))
	case reflect.Int, reflect.Int64:
		v.SetInt(f.int64())
	case reflect.Float64:
		x := f.float()
		for math.IsNaN(x) || math.IsInf(x, 0) {
			x = f.float()
		}
		v.SetFloat(x)
	case reflect.Pointer:
		if depth > 6 || f.chance(35) {
			return nil
		}
		p := reflect.New(t.Elem())
		if err := f.fill(p.Elem(), depth+1); err != nil {
			return err
		}
		v.Set(p)
	case reflect.Slice:
		if f.chance(20) || depth > 6 {
			return nil
		}
		n := 0
		if !f.chance(25) {
			n = f.intn(3) + 1
		}
		s := reflect.MakeSlice(t, n, n)
		for i := 0; i < n; i++ {
			if err := f.fill(s.Index(i), depth+1); err != nil {
				return err
			}
		}
		v.Set(s)
	case reflect.Array:
		for i := 0; i < v.Len(); i++ {
			if err := f.fill(v.Index(i), depth+1); err != nil {
				return err
			}
		}
	case reflect.Map:
		if f.chance(20) || depth > 6 {
			return nil
		}
		m := reflect.MakeMap(t)
		n := 0
		if !f.chance(25) {
			n = f.intn(3) + 1
		}
		for i := 0; i < n; i++ {
			k := reflect.New(t.Key()).Elem()
			if err := f.fill(k, depth+1); err != nil {
				return err
			}
			e := reflect.New(t.Elem()).Elem()
			if err := f.fill(e, depth+1); err != nil {
				return err
			}
			m.SetMapIndex(k, e)
		}
		v.Set(m)
	case reflect.Struct:
		for i := 0; i < t.NumField(); i++ {
			if !t.Field(i).IsExported() {
				continue
			}
			if err := f.fill(v.Field(i), depth+1); err != nil {
				return err
			}
		}
	default:
		return fmt.Errorf("fill: unsupported kind %s (%s)", t.Kind(), t)
	}
	return nil
}

// frozenMembers hold registry values: rewriting may reorder, space and escape
// them but never changes what they decode to.
var frozenMembers = map[string]bool{
	"point": true, "stages_supported": true, "decisions_supported": true, "stage_status": true,
}

// rewriter writes a decoded document back with random changes.
type rewriter struct {
	f *fuzzer
	b *bytes.Buffer
}

func (w rewriter) space() {
	if w.f.chance(15) {
		w.b.WriteString(w.f.pick(" ", "\n", "\t", "\r\n  "))
	}
}

func (w rewriter) str(s string, frozen bool) {
	var sb strings.Builder
	w.f.writeString(&sb, s)
	out := sb.String()
	if !frozen && w.f.chance(3) {
		// Raw invalid UTF-8 at the start or the end of the string token.
		at := 1
		if w.f.chance(50) {
			at = len(out) - 1
		}
		bad := w.f.pick("\xff", "\xc3", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xc0\xaf", "\x80\x80", "\xe2\x82")
		out = out[:at] + bad + out[at:]
	}
	w.b.WriteString(out)
}

// number writes n, sometimes in another form of the same value.
func (w rewriter) number(n json.Number, frozen bool) {
	s := string(n)
	if !frozen && w.f.chance(6) {
		if !strings.ContainsAny(s, ".eE") {
			s = w.f.pick(s+".0", s+"e0", s+"E+0", s+".000")
		} else if !strings.ContainsAny(s, "eE") {
			s = w.f.pick(s+"0", s+"e0", s+"E-0")
		}
	}
	w.b.WriteString(s)
}

func (w rewriter) caseChange(k string) string {
	r := []rune(k)
	for i, c := range r {
		if w.f.chance(50) {
			if unicode.IsUpper(c) {
				r[i] = unicode.ToLower(c)
			} else {
				r[i] = unicode.ToUpper(c)
			}
		}
	}
	return string(r)
}

func (w rewriter) other() {
	w.b.WriteString(w.f.pick(`null`, `true`, `"x"`, `1`, `1.5`, `[]`, `{}`, `[1]`, `{"a":1}`, `-1`, `""`))
}

func (w rewriter) value(v any, member string, frozen bool, depth int) {
	w.space()
	switch x := v.(type) {
	case nil:
		w.b.WriteString("null")
	case bool:
		if x {
			w.b.WriteString("true")
		} else {
			w.b.WriteString("false")
		}
	case json.Number:
		w.number(x, frozen)
	case string:
		w.str(x, frozen)
	case []any:
		items := x
		if !frozen && len(items) == 2 && w.f.chance(12) {
			_, s0 := items[0].(string)
			_, s1 := items[1].(string)
			if s0 && s1 {
				// A header pair with a missing or an extra element.
				switch w.f.intn(3) {
				case 0:
					items = items[:1]
				case 1:
					items = append(items[:2:2], w.f.text())
				default:
					items = nil
				}
			}
		}
		w.b.WriteByte('[')
		for i, item := range items {
			if i > 0 {
				w.b.WriteByte(',')
			}
			w.value(item, member, frozen, depth+1)
		}
		w.space()
		w.b.WriteByte(']')
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		w.f.r.Shuffle(len(keys), func(i, j int) { keys[i], keys[j] = keys[j], keys[i] })
		keysFrozen := frozen || member == "stages"
		w.b.WriteByte('{')
		first := true
		emit := func(k string, frozenKey bool, val func()) {
			if !first {
				w.b.WriteByte(',')
			}
			first = false
			w.space()
			w.str(k, frozenKey)
			w.space()
			w.b.WriteByte(':')
			val()
		}
		for _, k := range keys {
			kf := frozen || frozenMembers[k]
			if !kf && w.f.chance(4) {
				continue // missing member
			}
			name := k
			if !keysFrozen && !kf && w.f.chance(5) {
				name = w.caseChange(k)
			}
			if !kf && w.f.chance(2) {
				// Repeated member: the first value is overwritten by the next.
				dup := k
				if !keysFrozen {
					dup = w.caseChange(k)
				}
				emit(dup, true, func() { w.other() })
			}
			frozenKey := keysFrozen || kf
			switch {
			case !kf && w.f.chance(4):
				emit(name, frozenKey, func() { w.b.WriteString("null") })
			case !kf && w.f.chance(1):
				emit(name, frozenKey, func() { w.other() })
			default:
				val := x[k]
				emit(name, frozenKey, func() { w.value(val, k, kf, depth+1) })
			}
		}
		if !keysFrozen && w.f.chance(5) {
			emit(w.f.pick("zz_unknown", "Unknown", "")+w.f.text(), false, func() { w.value(map[string]any{"a": json.Number("1")}, "", false, depth+1) })
		}
		w.space()
		w.b.WriteByte('}')
	default:
		panic(fmt.Sprintf("rewrite: unexpected %T", v))
	}
	w.space()
}

// rewrite returns doc written back with random changes, and sometimes broken.
func (f *fuzzer) rewrite(doc []byte) ([]byte, error) {
	dec := json.NewDecoder(bytes.NewReader(doc))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, err
	}
	var b bytes.Buffer
	rewriter{f: f, b: &b}.value(v, "", false, 0)
	out := b.Bytes()
	if f.chance(3) && len(out) > 0 {
		switch f.intn(3) {
		case 0:
			out = out[:f.intn(len(out))]
		case 1:
			at := f.intn(len(out))
			out = append(append(append([]byte(nil), out[:at]...), f.pick("x", "}", ",", "]", ":", "\"", "\x00")...), out[at:]...)
		default:
			out = append(out, f.pick(" x", "{}", ",", "\x00")...)
		}
	}
	return out, nil
}

// fuzzDocumentsGolden writes the generated documents and Go's decoding of each.
func fuzzDocumentsGolden() (any, error) {
	f := newFuzzer(fuzzSeed + 2)
	types := []struct {
		name string
		zero func() any
	}{
		{"envelope", func() any { return &parity.Envelope{} }},
		{"trace", func() any { return &parity.Trace{} }},
		{"adapter_requests_input", func() any { return &parity.AdapterRequestsInput{} }},
		{"adapter_requests_output", func() any { return &parity.AdapterRequestsOutput{} }},
		{"adapter_bids_input", func() any { return &parity.AdapterBidsInput{} }},
		{"adapter_bids_output", func() any { return &parity.AdapterBidsOutput{} }},
		{"health", func() any { return &parity.Health{} }},
		{"config", func() any { return &parity.ConfigDump{} }},
		{"bidders", func() any { return &parity.BiddersDump{} }},
		{"reset", func() any { return &parity.ResetResult{} }},
		{"outbound_meta", func() any { return &parity.OutboundMeta{} }},
	}
	cases := make([]map[string]any, 0, len(types)*fuzzDocsPerType)
	for _, typ := range types {
		for i := 0; i < fuzzDocsPerType; i++ {
			v := typ.zero()
			if err := f.fill(reflect.ValueOf(v).Elem(), 0); err != nil {
				return nil, err
			}
			input, err := json.Marshal(v)
			if err != nil {
				return nil, fmt.Errorf("%s %d: %w", typ.name, i, err)
			}
			if f.chance(75) {
				if input, err = f.rewrite(input); err != nil {
					return nil, fmt.Errorf("%s %d: %w", typ.name, i, err)
				}
			}
			c := map[string]any{"type": typ.name, "input_b64": base64.StdEncoding.EncodeToString(input)}
			out := typ.zero()
			if err := json.Unmarshal(input, out); err != nil {
				c["error"] = err.Error()
			} else {
				doc, err := writeJSON(out)
				if err != nil {
					return nil, fmt.Errorf("%s %d: encoding: %w", typ.name, i, err)
				}
				c["encoded_b64"] = base64.StdEncoding.EncodeToString([]byte(doc))
			}
			cases = append(cases, c)
		}
	}
	return cases, nil
}
