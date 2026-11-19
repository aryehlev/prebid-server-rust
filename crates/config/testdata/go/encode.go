package main

import (
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
)

// The goldens write Go values in a compact form that keeps their Go types where the loader must
// keep them apart:
//
//   - nil is null, a bool and a string are themselves;
//   - an int (64 bits on amd64) is a JSON integer;
//   - a float64 or float32 is a JSON number with a '.' or an exponent ("5.0", "1e+21"), and NaN
//     and the infinities are {"$float64": "NaN" | "+Inf" | "-Inf"};
//   - an int64 is {"$int64": n} and a uint64 {"$uint64": n} (go-toml reads integers as int64,
//     yaml.v3 reads integers above the int range as uint64);
//   - a time.Time is {"$time": RFC 3339 text} (go-toml's offset date-times), and go-toml's
//     local date, time and date-time types {"$toml": text};
//   - a []interface{}, []string or any other slice is a list, and map[string]interface{} an
//     object (no key of a golden input starts with '$', so the tags cannot clash);
//   - a map[interface{}]interface{} (yaml.v3 keeps one for a mapping whose keys are not all
//     strings) is {"$imap": [[key, value], ...]} in the order of the keys' text.

// floatValue is f in the compact form.
func floatValue(f float64) any {
	switch {
	case math.IsNaN(f):
		return map[string]any{"$float64": "NaN"}
	case math.IsInf(f, 1):
		return map[string]any{"$float64": "+Inf"}
	case math.IsInf(f, -1):
		return map[string]any{"$float64": "-Inf"}
	}
	s := strconv.FormatFloat(f, 'g', -1, 64)
	if !strings.ContainsAny(s, ".e") {
		s += ".0"
	}
	return json.RawMessage(s)
}

// checkKey panics on a map key the compact form could mistake for a tag.
func checkKey(k string) string {
	if strings.HasPrefix(k, "$") {
		panic("a golden input has the map key " + k)
	}
	return k
}

// dyn writes a value of Go's dynamic types, as Viper and yaml.v3 produce them.
func dyn(v any) any {
	switch x := v.(type) {
	case nil:
		return nil
	case bool, string:
		return x
	case int:
		return x
	case int64:
		return map[string]any{"$int64": x}
	case uint64:
		return map[string]any{"$uint64": x}
	case float64:
		return floatValue(x)
	case time.Time:
		return map[string]any{"$time": x.Format(time.RFC3339Nano)}
	case map[string]any:
		m := make(map[string]any, len(x))
		for k, e := range x {
			m[checkKey(k)] = dyn(e)
		}
		return m
	case map[any]any:
		type pair struct {
			text string
			kv   []any
		}
		pairs := make([]pair, 0, len(x))
		for k, e := range x {
			pairs = append(pairs, pair{fmt.Sprint(k), []any{dyn(k), dyn(e)}})
		}
		sort.Slice(pairs, func(i, j int) bool { return pairs[i].text < pairs[j].text })
		list := make([]any, len(pairs))
		for i, p := range pairs {
			list[i] = p.kv
		}
		return map[string]any{"$imap": list}
	}
	rv := reflect.ValueOf(v)
	if strings.HasPrefix(rv.Type().PkgPath(), "github.com/pelletier/go-toml") {
		// go-toml's LocalDate, LocalTime and LocalDateTime.
		return map[string]any{"$toml": fmt.Sprint(v)}
	}
	switch rv.Kind() {
	case reflect.Slice:
		if rv.IsNil() {
			// A typed nil slice, as cast.ToStringSlice returns for a value it cannot cast:
			// mapstructure leaves the field nil for it, as for nil itself.
			return nil
		}
		fallthrough
	case reflect.Array:
		// []interface{}, []string, [][]string and the empty slices of a named type that
		// SetupViper registers as defaults ([]config.AgmaAnalyticsAccount{},
		// []openrtb_ext.BidderName{}).
		list := make([]any, rv.Len())
		for i := range list {
			list[i] = dyn(rv.Index(i).Interface())
		}
		return list
	case reflect.String:
		// A named string type, such as openrtb_ext.BidderName.
		return rv.String()
	}
	panic(fmt.Sprintf("dyn: a %T", v))
}

// fieldKey is the key mapstructure looks a struct field up by: its tag name, else its Go name.
func fieldKey(f reflect.StructField) string {
	name, _, _ := strings.Cut(f.Tag.Get("mapstructure"), ",")
	if name == "" {
		name = f.Name
	}
	return name
}

// typed writes a value v.Unmarshal decoded: a struct as the object of its non-zero exported
// fields under their mapstructure keys (a field Go leaves at its zero value is absent, as is one
// decoded to zero), a pointer as its target, a map as an object keyed by fmt.Sprint of the key,
// a slice or array as a list (a []byte as {"$bytes": text}), an interface{} with dyn, and a nil
// pointer, map, slice or interface as null.
func typed(v reflect.Value) any {
	switch v.Kind() {
	case reflect.Struct:
		out := map[string]any{}
		t := v.Type()
		for i := 0; i < t.NumField(); i++ {
			f := t.Field(i)
			if f.PkgPath != "" || v.Field(i).IsZero() {
				continue
			}
			out[fieldKey(f)] = typed(v.Field(i))
		}
		return out
	case reflect.Ptr:
		if v.IsNil() {
			return nil
		}
		return typed(v.Elem())
	case reflect.Map:
		if v.IsNil() {
			return nil
		}
		out := make(map[string]any, v.Len())
		for _, k := range v.MapKeys() {
			out[checkKey(fmt.Sprint(k.Interface()))] = typed(v.MapIndex(k))
		}
		return out
	case reflect.Slice:
		if v.IsNil() {
			return nil
		}
		if v.Type().Elem().Kind() == reflect.Uint8 {
			return map[string]any{"$bytes": string(v.Bytes())}
		}
		fallthrough
	case reflect.Array:
		out := make([]any, v.Len())
		for i := range out {
			out[i] = typed(v.Index(i))
		}
		return out
	case reflect.Interface:
		if v.IsNil() {
			return nil
		}
		return dyn(v.Elem().Interface())
	case reflect.Bool:
		return v.Bool()
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return v.Int()
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return v.Uint()
	case reflect.Float32, reflect.Float64:
		return floatValue(v.Float())
	case reflect.String:
		return v.String()
	}
	panic(fmt.Sprintf("typed: kind %s", v.Kind()))
}

// atPath follows a Viper key through a decoded value as mapstructure placed it: struct fields
// by their key (matched with strings.EqualFold, as mapstructure matches), map entries by key,
// through pointers. ok is false when the path leaves the value (a nil pointer or map, a missing
// entry, a key no field has).
func atPath(v reflect.Value, path []string) (reflect.Value, bool) {
	for _, step := range path {
		for v.Kind() == reflect.Ptr {
			if v.IsNil() {
				return reflect.Value{}, false
			}
			v = v.Elem()
		}
		switch v.Kind() {
		case reflect.Struct:
			t := v.Type()
			found := false
			for i := 0; i < t.NumField() && !found; i++ {
				f := t.Field(i)
				if f.PkgPath == "" && strings.EqualFold(fieldKey(f), step) {
					v, found = v.Field(i), true
				}
			}
			if !found {
				return reflect.Value{}, false
			}
		case reflect.Map:
			if v.Type().Key().Kind() != reflect.String {
				return reflect.Value{}, false
			}
			e := v.MapIndex(reflect.ValueOf(step).Convert(v.Type().Key()))
			if !e.IsValid() {
				return reflect.Value{}, false
			}
			v = e
		default:
			return reflect.Value{}, false
		}
	}
	return v, true
}

// fieldType follows a Viper key through a type as atPath follows it through a value; ok is
// false when no field has the key.
func fieldType(t reflect.Type, path []string) (reflect.Type, bool) {
	for _, step := range path {
		for t.Kind() == reflect.Ptr {
			t = t.Elem()
		}
		switch t.Kind() {
		case reflect.Struct:
			var next reflect.Type
			for i := 0; i < t.NumField() && next == nil; i++ {
				f := t.Field(i)
				if f.PkgPath == "" && strings.EqualFold(fieldKey(f), step) {
					next = f.Type
				}
			}
			if next == nil {
				return nil, false
			}
			t = next
		case reflect.Map:
			if t.Key().Kind() != reflect.String {
				return nil, false
			}
			t = t.Elem()
		default:
			return nil, false
		}
	}
	return t, true
}

// leaf writes the decoded value at a Viper key: {"value": typed value} (a zero value included),
// or {"absent": true} when the path leaves the decoded configuration.
func leaf(root reflect.Value, key string) any {
	v, ok := atPath(root, strings.Split(key, "."))
	if !ok {
		return map[string]any{"absent": true}
	}
	return map[string]any{"value": typed(v)}
}
