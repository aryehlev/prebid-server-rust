package main

import (
	"fmt"
	"reflect"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/config"
)

// typeNode is one Go type as mapstructure sees it when Viper's Unmarshal decodes into
// config.Configuration: the reflect kind (with the integer and float sizes), the element, key and
// field types, and for a struct its fields with their mapstructure, encoding/json and yaml.v3
// names.
type typeNode struct {
	Kind   string      `json:"kind"`
	Bits   int         `json:"bits,omitempty"`
	Elem   string      `json:"elem,omitempty"`
	Key    string      `json:"key,omitempty"`
	Len    int         `json:"len,omitempty"`
	Fields []fieldNode `json:"fields,omitempty"`
	// Hook names the decode hook config.New registers for this type: "account_modules" for
	// config.AccountModules (config/account_module_decoder.go:16).
	Hook string `json:"hook,omitempty"`
	// RawMessage marks encoding/json's RawMessage, a []byte holding JSON text.
	RawMessage bool `json:"raw_message,omitempty"`
}

// fieldNode is one exported struct field. Name is the key mapstructure looks for: the name of
// the mapstructure tag, else the Go name; mapstructure matches it case-insensitively
// (strings.EqualFold, mapstructure.go:1393-1409), and Viper lower-cases every key. JSON is the
// key encoding/json writes, absent with json:"-"; YAML is the key yaml.v3 reads, absent with
// yaml:"-".
type fieldNode struct {
	Go       string  `json:"go"`
	Name     string  `json:"name"`
	JSON     *string `json:"json"`
	YAML     *string `json:"yaml"`
	Type     string  `json:"type"`
	Embedded bool    `json:"embedded,omitempty"`
	Squash   bool    `json:"squash,omitempty"`
}

// schemaDoc is go_types.json: every type reachable from the roots, keyed by reflect's String().
type schemaDoc struct {
	Header header              `json:"header"`
	Notes  []string            `json:"notes"`
	Roots  map[string]string   `json:"roots"`
	Types  map[string]typeNode `json:"types"`
}

// schemaRoots are the types the loader decodes into: the whole configuration (config.New's
// v.Unmarshal, config/config.go:758) and the *bool targets of its two UnmarshalKey calls
// (config/config.go:886-889).
var schemaRoots = map[string]reflect.Type{
	"configuration": reflect.TypeOf(config.Configuration{}),
	"unmarshal_key": reflect.TypeOf((*bool)(nil)),
}

// tagName is the name part of a struct tag value: "" when the tag is absent or has no name.
func tagName(tag reflect.StructTag, key string) (name string, present bool) {
	value, ok := tag.Lookup(key)
	if !ok {
		return "", false
	}
	name, _, _ = strings.Cut(value, ",")
	return name, true
}

// tagOptions are the comma-separated options after a tag's name.
func tagOptions(tag reflect.StructTag, key string) []string {
	value := tag.Get(key)
	_, opts, found := strings.Cut(value, ",")
	if !found {
		return nil
	}
	return strings.Split(opts, ",")
}

// jsonName is the key encoding/json uses for a field: nil when json:"-" hides it.
func jsonName(f reflect.StructField) *string {
	name, present := tagName(f.Tag, "json")
	if present && name == "-" && len(tagOptions(f.Tag, "json")) == 0 {
		return nil
	}
	if name == "" {
		name = f.Name
	}
	return &name
}

// yamlName is the key yaml.v3 uses for a field: its tag name, else the lower-cased Go name; nil
// when yaml:"-" hides it.
func yamlName(f reflect.StructField) *string {
	name, present := tagName(f.Tag, "yaml")
	if present && name == "-" {
		return nil
	}
	if name == "" {
		name = strings.ToLower(f.Name)
	}
	return &name
}

type schemaBuilder struct {
	types map[string]typeNode
	seen  map[string]reflect.Type
}

func (b *schemaBuilder) add(t reflect.Type) string {
	id := t.String()
	if prev, ok := b.seen[id]; ok {
		if prev != t {
			panic(fmt.Sprintf("two types print as %s: %s and %s", id, prev.PkgPath(), t.PkgPath()))
		}
		return id
	}
	b.seen[id] = t
	var node typeNode
	switch t.Kind() {
	case reflect.Bool:
		node.Kind = "bool"
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		node.Kind, node.Bits = "int", t.Bits()
		if t.PkgPath() == "time" {
			panic("a time.Duration field: config.New's decode hook replaces Viper's duration hook")
		}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		node.Kind, node.Bits = "uint", t.Bits()
	case reflect.Float32, reflect.Float64:
		node.Kind, node.Bits = "float", t.Bits()
	case reflect.String:
		node.Kind = "string"
	case reflect.Interface:
		if t.NumMethod() != 0 {
			panic(fmt.Sprintf("%s: an interface with methods", id))
		}
		node.Kind = "interface"
	case reflect.Ptr:
		node.Kind = "ptr"
		b.types[id] = node
		node.Elem = b.add(t.Elem())
	case reflect.Slice:
		node.Kind = "slice"
		node.RawMessage = t.PkgPath() == "encoding/json" && t.Name() == "RawMessage"
		b.types[id] = node
		node.Elem = b.add(t.Elem())
	case reflect.Array:
		node.Kind, node.Len = "array", t.Len()
		b.types[id] = node
		node.Elem = b.add(t.Elem())
	case reflect.Map:
		node.Kind = "map"
		if t == reflect.TypeOf(config.AccountModules{}) {
			node.Hook = "account_modules"
		}
		b.types[id] = node
		node.Key = b.add(t.Key())
		node.Elem = b.add(t.Elem())
	case reflect.Struct:
		node.Kind = "struct"
		b.types[id] = node
		for i := 0; i < t.NumField(); i++ {
			f := t.Field(i)
			if f.PkgPath != "" {
				continue
			}
			name, _ := tagName(f.Tag, "mapstructure")
			if name == "" {
				name = f.Name
			}
			field := fieldNode{
				Go:       f.Name,
				Name:     name,
				JSON:     jsonName(f),
				YAML:     yamlName(f),
				Embedded: f.Anonymous,
			}
			for _, opt := range tagOptions(f.Tag, "mapstructure") {
				switch opt {
				case "squash":
					field.Squash = true
				case "remain", "omitempty":
					panic(fmt.Sprintf("%s.%s: mapstructure option %q", id, f.Name, opt))
				}
			}
			field.Type = b.add(f.Type)
			node.Fields = append(node.Fields, field)
		}
	default:
		panic(fmt.Sprintf("%s: unsupported kind %s", id, t.Kind()))
	}
	b.types[id] = node
	return id
}

func schemaGolden() schemaDoc {
	b := &schemaBuilder{types: map[string]typeNode{}, seen: map[string]reflect.Type{}}
	roots := map[string]string{}
	names := make([]string, 0, len(schemaRoots))
	for name := range schemaRoots {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		roots[name] = b.add(schemaRoots[name])
	}
	return schemaDoc{
		Header: newHeader("github.com/spf13/viper", "github.com/mitchellh/mapstructure"),
		Notes: []string{
			"Written by crates/config/testdata/go (go run .); do not edit.",
			"types: every Go type reachable from the roots by reflection, keyed by reflect.Type.String().",
			"kind: the reflect kind as mapstructure groups it (int, uint and float carry their bits; int is 64 bits on amd64).",
			"fields: the exported fields in declaration order; name is mapstructure's key (tag name, else the Go name), json and yaml the encoding/json and yaml.v3 keys (null when the tag hides the field).",
			"hook: account_modules marks config.AccountModules, which AccountModulesHookFunc converts (config/account_module_decoder.go:16).",
		},
		Roots: roots,
		Types: b.types,
	}
}
