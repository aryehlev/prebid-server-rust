package main

import (
	"reflect"
	"sort"
	"strings"

	"github.com/spf13/viper"

	"github.com/prebid/prebid-server/v4/config"
)

// matrixBidder is the bidder whose adapters.<bidder>.* keys the matrix covers.
const matrixBidder = "appnexus"

// matrixRow is one key set to one value through one channel.
type matrixRow struct {
	Key string `json:"key"`
	// Via is "env" (PBS_<KEY>) or "file" (pbs.yaml with the key nested as written).
	Via string `json:"via"`
	// Input is the variable's value or the YAML text of the value.
	Input string `json:"input"`
	// Outcomes holds the one outcome of the load, or every outcome seen when the result
	// depends on Go's map order.
	Outcomes []matrixOutcome `json:"outcomes"`
}

// matrixOutcome is what one load of a row gave.
type matrixOutcome struct {
	// Get and IsSet are what Viper answers for the key after the load.
	Get   any  `json:"get"`
	IsSet bool `json:"is_set"`
	// Leaf is the decoded value at the key (encode.go's leaf), absent when the decode failed.
	Leaf any `json:"leaf,omitempty"`
	// Error is v.Unmarshal's error, empty when it decoded.
	Error string `json:"error"`
}

// matrixDoc is matrix.json.
type matrixDoc struct {
	Header header      `json:"header"`
	Notes  []string    `json:"notes"`
	Rows   []matrixRow `json:"rows"`
}

// matrixKeys are every key Viper knows after SetupViper with the matrix bidder, in order.
func matrixKeys() []string {
	v := viper.New()
	config.SetupViper(v, "", config.BidderInfos{matrixBidder: {}})
	keys := v.AllKeys()
	sort.Strings(keys)
	return keys
}

// inputs are the values one key is set to: a typed sample first, then values the field's type
// cannot take as they are. They follow the Go type of the field the key decodes into, and the
// type of the key's default when no field has the key.
type inputs struct {
	env  []string
	file []string
}

func inputsFor(t reflect.Type) inputs {
	if t.Kind() == reflect.Ptr && t.Elem().Kind() != reflect.Struct {
		return inputsFor(t.Elem())
	}
	switch t.Kind() {
	case reflect.Bool:
		return inputs{env: []string{"true", "yes", "0", "T"}, file: []string{"true", "yes", "1", "\"t\""}}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return inputs{env: []string{"42", "4x2", "0x10", "300"}, file: []string{"42", "\"4x2\"", "4.5", "300"}}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return inputs{env: []string{"42", "-1", "70000"}, file: []string{"42", "-1", "70000"}}
	case reflect.Float32, reflect.Float64:
		return inputs{env: []string{"0.25", "x", "1_0"}, file: []string{"0.25", "x", "1"}}
	case reflect.String:
		return inputs{env: []string{"s1", "a b"}, file: []string{"s1", "[a]", "1.5", "true"}}
	case reflect.Slice, reflect.Array:
		switch e := t.Elem(); e.Kind() {
		case reflect.String:
			return inputs{env: []string{"a b", "a,b"}, file: []string{"[a, b]", "a", "{a: 1}", "[1, true]"}}
		case reflect.Slice:
			return inputs{env: []string{"a b"}, file: []string{"[[a, b]]", "[{a: 1}]", "[a]"}}
		case reflect.Struct, reflect.Ptr:
			return inputs{env: []string{"x"}, file: []string{"[{}]", "[x]", "{}"}}
		case reflect.Uint8:
			return inputs{env: []string{"x"}, file: []string{"x", "[1]"}}
		}
		return inputs{env: []string{"x"}, file: []string{"[1]", "x"}}
	case reflect.Struct, reflect.Ptr, reflect.Map:
		return inputs{env: []string{"x"}, file: []string{"{}", "5", "[]"}}
	case reflect.Interface:
		return inputs{env: []string{"x"}, file: []string{"1", "{a: 1}"}}
	}
	panic("inputsFor: kind " + t.Kind().String())
}

// inputsForKey picks a key's inputs by its field, else by its default.
func inputsForKey(key string, defaults *viper.Viper) inputs {
	if t, ok := fieldType(reflect.TypeOf(config.Configuration{}), strings.Split(key, ".")); ok {
		return inputsFor(t)
	}
	if d := defaults.Get(key); d != nil {
		return inputsFor(reflect.TypeOf(d))
	}
	return inputs{env: []string{"x"}, file: []string{"x"}}
}

// nestedYAML writes value at key as nested YAML mappings.
func nestedYAML(key, value string) string {
	var b strings.Builder
	parts := strings.Split(key, ".")
	for i, p := range parts {
		b.WriteString(strings.Repeat("  ", i))
		b.WriteString(p)
		b.WriteString(":")
		if i == len(parts)-1 {
			b.WriteString(" ")
			b.WriteString(value)
		}
		b.WriteString("\n")
	}
	return b.String()
}

// envName is the PBS_ variable Viper reads for a key (config/config.go:1318-1321).
func envName(key string) string {
	return strings.NewReplacer(".", "_").Replace(strings.ToUpper("PBS_" + key))
}

func matrixGolden() matrixDoc {
	checkEtcConfig()
	doc := matrixDoc{
		Header: newHeader("github.com/spf13/viper", "github.com/mitchellh/mapstructure", "github.com/spf13/cast", "gopkg.in/yaml.v3"),
		Notes: []string{
			"Written by crates/config/testdata/go (go run .); do not edit.",
			"One row per key Viper knows after SetupViper with the bidder appnexus, and per value: via env sets PBS_<KEY> alone, via file writes pbs.yaml with the key nested as written; each load decodes as config.New does.",
			"The values follow the Go type of the field the key decodes into (the default's type when no field has it): a typed sample first, then values the type cannot take as they are.",
			"outcomes holds the load's outcome, or every outcome of 200 runs when the result depends on Go's map order (a YAML mapping at a key with a default).",
			"get is v.Get(key) in the compact form of encode.go; leaf is the decoded value at the key ({\"value\": v} or {\"absent\": true}).",
		},
	}
	defaults := viper.New()
	config.SetupViper(defaults, "", config.BidderInfos{matrixBidder: {}})
	for _, key := range matrixKeys() {
		in := inputsForKey(key, defaults)
		for _, value := range in.env {
			doc.Rows = append(doc.Rows, matrixRun(key, "env", value, loadInput{
				Name: key, Env: map[string]string{envName(key): value}, Bidders: []string{matrixBidder},
			}, false))
		}
		for _, value := range in.file {
			unordered := strings.HasPrefix(value, "{") && value != "{}" && defaults.Get(key) != nil
			doc.Rows = append(doc.Rows, matrixRun(key, "file", value, loadInput{
				Name: key, Files: map[string]string{"pbs.yaml": nestedYAML(key, value)}, Bidders: []string{matrixBidder},
			}, unordered))
		}
	}
	return doc
}

// matrixRun loads one row. A YAML mapping set at a key with a default gives Viper's AllSettings
// both the key (its default's cast of the mapping) and the keys under it, and the map order picks
// which one the tree keeps; such a row runs unorderedRuns times and records every outcome. Any
// other row runs twice and must give the same outcome both times.
func matrixRun(key, via, value string, in loadInput, unordered bool) matrixRow {
	row := matrixRow{Key: key, Via: via, Input: value}
	runs := 2
	if unordered {
		runs = unorderedRuns
	}
	seen := map[string]matrixOutcome{}
	for i := 0; i < runs; i++ {
		var out matrixOutcome
		withViper(in, func(l loaded) {
			c, err := decode(l.v)
			if err != nil {
				out.Error = err.Error()
			} else {
				out.Leaf = leaf(reflect.ValueOf(c), key)
			}
			out.Get = dyn(l.v.Get(key))
			out.IsSet = l.v.IsSet(key)
		})
		seen[string(encodeGolden(out))] = out
	}
	if !unordered && len(seen) != 1 {
		panic("matrix row " + key + " " + via + " " + value + " depends on Go's map order")
	}
	for _, k := range sortedKeys(seen) {
		row.Outcomes = append(row.Outcomes, seen[k])
	}
	return row
}
