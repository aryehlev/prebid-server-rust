// Command golden writes the differential goldens that pbs-goyaml's tests replay byte for byte.
//
// Every input is decoded by gopkg.in/yaml.v3 v3.0.1 (yaml.Unmarshal, and a yaml.Decoder for
// streams and KnownFields) or by github.com/mitchellh/mapstructure v1.5.0, under viper v1.12.0's
// default decoder configuration among others. Values are written with dump, a rendering of Go
// values with the dynamic types of interface values, which tests/golden.rs reproduces from the
// Rust values; errors with err.Error(), and Go panics as "panic: " and the recovered value.
// Each field is written with strconv.QuoteToASCII, one tab-separated case per line.
//
//   - files.txt: every static/bidder-info/*.yaml and the repository's other YAML files, into
//     interface{}, into config.BidderInfo (also with KnownFields, and through mapstructure from
//     the interface{} tree, as viper decodes), and into the alias fields; and the interface{}
//     tree through jsonutil.Marshal and json.Marshal.
//   - corpus.txt: hand-written inputs, the YAML inputs of yaml.v3's own tests, every sample
//     scalar under every Kitchen key, and deterministic mutations of them, into interface{},
//     Kitchen and config.BidderInfo, and as a stream; and the interface{} tree through
//     jsonutil.Marshal and json.Marshal.
//   - mapstructure.txt: the input shapes of mapstructure's own tests, every sample value under
//     every Kitchen key, and config.AccountModules inputs, under the strict, weak and viper
//     configurations, and Prebid's (viper with config.AccountModulesHookFunc as the only hook).
//
// Run it from the repository root, as amd64 code (Go's float-to-integer conversions differ
// between architectures, and the crate reproduces amd64):
//
//	GOARCH=amd64 go run ./rust/crates/goyaml/testdata/go
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/mitchellh/mapstructure"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"gopkg.in/yaml.v3"
)

func init() {
	// yaml.v3 parses timestamps with time.Parse, which takes the Local zone for an offset that
	// matches it; fix Local so that the goldens do not depend on the machine.
	time.Local = time.UTC
}

func main() {
	out := flag.String("out", "rust/crates/goyaml/testdata/golden", "golden directory")
	repo := flag.String("repo", ".", "repository root")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	writeFiles(*out, *repo)
	writeCorpus(*out, *repo)
	writeMapstructure(*out)
}

// golden is one output file.
type golden struct {
	w     *bufio.Writer
	f     *os.File
	cases int
}

func create(dir, name string, columns ...string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		log.Fatal(err)
	}
	g := &golden{w: bufio.NewWriter(f), f: f}
	g.write("# " + strings.Join(columns, "\t") + "\n")
	return g
}

func (g *golden) write(s string) {
	if _, err := g.w.WriteString(s); err != nil {
		log.Fatal(err)
	}
}

// row writes one case: each field quoted, tab separated.
func (g *golden) row(fields ...string) {
	quoted := make([]string, len(fields))
	for i, field := range fields {
		quoted[i] = strconv.QuoteToASCII(field)
	}
	g.write(strings.Join(quoted, "\t") + "\n")
	g.cases++
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := g.f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s: %d cases\n", g.f.Name(), g.cases)
}

// panicked is a Go panic that the decoder did not recover.
type panicked struct{ value interface{} }

func (p panicked) Error() string { return "panic: " + fmt.Sprint(p.value) }

// guard runs f, returning a panic as a panicked error.
func guard(f func() error) (err error) {
	defer func() {
		if r := recover(); r != nil {
			err = panicked{r}
		}
	}()
	return f()
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func isPanic(err error) bool {
	var p panicked
	return errors.As(err, &p)
}

// outcome renders a decoded value and its error. A value interrupted by a panic is not written.
func outcome(v reflect.Value, err error) (string, string) {
	if isPanic(err) {
		return "", err.Error()
	}
	return dump(v), errText(err)
}

// dump renders a value: scalars as Go literals (strconv.Quote, FormatInt, FormatFloat 'g' -1),
// pointers as & and the pointee, nil pointers, slices, maps and interfaces as nil, slices as
// [a, b], maps as {k: v, ...} sorted by their rendering, structs as {Name:value ...} with the
// exported non-zero fields, time.Time as time(t.String()), and an interface value as <its
// dynamic type> and the value.
func dump(v reflect.Value) string {
	var b strings.Builder
	dumpTo(&b, v)
	return b.String()
}

var timeType = reflect.TypeOf(time.Time{})

func dumpTo(b *strings.Builder, v reflect.Value) {
	switch v.Kind() {
	case reflect.Interface:
		if v.IsNil() {
			b.WriteString("nil")
			return
		}
		b.WriteString("<" + v.Elem().Type().String() + ">")
		dumpTo(b, v.Elem())
	case reflect.Ptr:
		if v.IsNil() {
			b.WriteString("nil")
			return
		}
		b.WriteByte('&')
		dumpTo(b, v.Elem())
	case reflect.Slice:
		if v.IsNil() {
			b.WriteString("nil")
			return
		}
		b.WriteByte('[')
		for i := 0; i < v.Len(); i++ {
			if i > 0 {
				b.WriteString(", ")
			}
			dumpTo(b, v.Index(i))
		}
		b.WriteByte(']')
	case reflect.Map:
		if v.IsNil() {
			b.WriteString("nil")
			return
		}
		entries := make([]string, 0, v.Len())
		iter := v.MapRange()
		for iter.Next() {
			entries = append(entries, dump(iter.Key())+": "+dump(iter.Value()))
		}
		sort.Strings(entries)
		b.WriteString("{" + strings.Join(entries, ", ") + "}")
	case reflect.Struct:
		if v.Type() == timeType {
			b.WriteString("time(" + v.Interface().(time.Time).String() + ")")
			return
		}
		var fields []string
		for i := 0; i < v.NumField(); i++ {
			f := v.Type().Field(i)
			if f.PkgPath != "" || v.Field(i).IsZero() {
				continue
			}
			fields = append(fields, f.Name+":"+dump(v.Field(i)))
		}
		b.WriteString("{" + strings.Join(fields, " ") + "}")
	case reflect.String:
		b.WriteString(strconv.Quote(v.String()))
	case reflect.Bool:
		b.WriteString(strconv.FormatBool(v.Bool()))
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		b.WriteString(strconv.FormatInt(v.Int(), 10))
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		b.WriteString(strconv.FormatUint(v.Uint(), 10))
	case reflect.Float32:
		b.WriteString(strconv.FormatFloat(v.Float(), 'g', -1, 32))
	case reflect.Float64:
		b.WriteString(strconv.FormatFloat(v.Float(), 'g', -1, 64))
	default:
		log.Fatalf("dump: unsupported kind %s", v.Kind())
	}
}

// marshalJSON renders marshal(v): the JSON, "error: " and the error, or "panic: " and the
// recovered value. jsoniter's output can depend on Go's random map iteration (which of several
// unsupported values it reports, the order of keys with the same text), so v is marshaled many
// times, and outcomes that differ are written as "one of:" and each distinct outcome, sorted, on
// lines of their own.
func marshalJSON(marshal func(interface{}) ([]byte, error), v interface{}) string {
	const runs = 64
	seen := map[string]bool{}
	for i := 0; i < runs; i++ {
		seen[marshalOnce(marshal, v)] = true
	}
	outcomes := make([]string, 0, len(seen))
	for outcome := range seen {
		outcomes = append(outcomes, outcome)
	}
	if len(outcomes) == 1 {
		return outcomes[0]
	}
	sort.Strings(outcomes)
	return "one of:\n" + strings.Join(outcomes, "\n")
}

func marshalOnce(marshal func(interface{}) ([]byte, error), v interface{}) string {
	var data []byte
	err := guard(func() error {
		var err error
		data, err = marshal(v)
		return err
	})
	switch {
	case isPanic(err):
		return err.Error()
	case err != nil:
		return "error: " + err.Error()
	}
	return string(data)
}

// yamlInto decodes input with yaml.Unmarshal into out, a pointer.
func yamlInto(input []byte, out interface{}) (string, string, error) {
	err := guard(func() error { return yaml.Unmarshal(input, out) })
	value, text := outcome(reflect.ValueOf(out).Elem(), err)
	return value, text, err
}

// yamlKnown decodes input into out with a yaml.Decoder in KnownFields mode.
func yamlKnown(input []byte, out interface{}) string {
	dec := yaml.NewDecoder(bytes.NewReader(input))
	dec.KnownFields(true)
	return errText(guard(func() error { return dec.Decode(out) }))
}

// yamlStream decodes the documents of input into interface{} one by one, as a yaml.Decoder
// reads them, until io.EOF, a panic, or maxDocs calls: one "value | error" line per call, then
// EOF. A first line equal to first (yaml.Unmarshal's outcome) is written as "=".
func yamlStream(input []byte, first string) string {
	const maxDocs = 16
	dec := yaml.NewDecoder(bytes.NewReader(input))
	var lines []string
	for i := 0; i < maxDocs; i++ {
		var v interface{}
		err := guard(func() error { return dec.Decode(&v) })
		if err == io.EOF {
			lines = append(lines, "EOF")
			break
		}
		value, text := outcome(reflect.ValueOf(&v).Elem(), err)
		line := value + " | " + text
		if i == 0 && line == first {
			line = "="
		}
		lines = append(lines, line)
		if isPanic(err) {
			break
		}
	}
	return strings.Join(lines, "\n")
}

// msConfig is the mapstructure configuration of a golden: mapstructure.Decode ("strict"),
// mapstructure.WeakDecode ("weak"), or viper's defaultDecoderConfig ("viper"), alone or with
// ErrorUnused (viper's UnmarshalExact) or ZeroFields, or Prebid's ("prebid": config.New passes
// viper.DecodeHook(config.AccountModulesHookFunc()), which replaces viper's hooks).
func msConfig(name string, out interface{}) *mapstructure.DecoderConfig {
	c := &mapstructure.DecoderConfig{Result: out}
	switch name {
	case "strict":
	case "weak":
		c.WeaklyTypedInput = true
	case "viper", "viper-unused", "viper-zero":
		c.WeaklyTypedInput = true
		c.DecodeHook = mapstructure.ComposeDecodeHookFunc(
			mapstructure.StringToTimeDurationHookFunc(),
			mapstructure.StringToSliceHookFunc(","),
		)
		c.ErrorUnused = name == "viper-unused"
		c.ZeroFields = name == "viper-zero"
	case "prebid":
		c.WeaklyTypedInput = true
		c.DecodeHook = config.AccountModulesHookFunc()
	default:
		log.Fatalf("unknown config %q", name)
	}
	return c
}

// msDecode decodes input into a new target, after decoding prefill into it when there is one.
func msDecode(target, configName string, prefill *interface{}, input interface{}) (string, string) {
	newTarget, ok := msTargets[target]
	if !ok {
		log.Fatalf("unknown target %q", target)
	}
	out := newTarget()
	decode := func(data interface{}) error {
		dec, err := mapstructure.NewDecoder(msConfig(configName, out))
		if err != nil {
			log.Fatal(err)
		}
		return guard(func() error { return dec.Decode(data) })
	}
	if prefill != nil {
		_ = decode(*prefill)
	}
	return outcome(reflect.ValueOf(out).Elem(), decode(input))
}

// yamlFiles lists the repository's YAML files, slash-separated and sorted.
func yamlFiles(repo string) []string {
	skip := map[string]bool{".git": true, ".claude": true, "rust": true, "node_modules": true, "vendor": true}
	var paths []string
	err := filepath.WalkDir(repo, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			if skip[d.Name()] {
				return filepath.SkipDir
			}
			return nil
		}
		if ext := filepath.Ext(path); ext == ".yaml" || ext == ".yml" {
			rel, err := filepath.Rel(repo, path)
			if err != nil {
				return err
			}
			paths = append(paths, filepath.ToSlash(rel))
		}
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	sort.Strings(paths)
	return paths
}

func writeFiles(dir, repo string) {
	g := create(dir, "files.txt", "path", "input",
		"interface{} value", "interface{} error",
		"config.BidderInfo value", "error", "json.Marshal",
		"aliasNillableFields value", "error",
		"BidderInfo KnownFields error",
		"BidderInfo via mapstructure (viper) value", "error",
		"jsonutil.Marshal(interface{})", "json.Marshal(interface{})")
	for _, path := range yamlFiles(repo) {
		data, err := os.ReadFile(filepath.Join(repo, path))
		if err != nil {
			log.Fatal(err)
		}
		var tree interface{}
		treeValue, treeErr, treeFail := yamlInto(data, &tree)
		var info config.BidderInfo
		infoValue, infoErr, _ := yamlInto(data, &info)
		js, err := json.Marshal(info)
		if err != nil {
			log.Fatal(err)
		}
		var alias aliasNillableFields
		aliasValue, aliasErr, _ := yamlInto(data, &alias)
		known := yamlKnown(data, new(config.BidderInfo))
		msValue, msErr := "", ""
		if !isPanic(treeFail) {
			msValue, msErr = msDecode("BidderInfo", "viper", nil, tree)
		}
		g.row(path, string(data), treeValue, treeErr, infoValue, infoErr, string(js),
			aliasValue, aliasErr, known, msValue, msErr,
			marshalJSON(jsonutil.Marshal, tree), marshalJSON(json.Marshal, tree))
	}
	g.close()
}

func writeCorpus(dir, repo string) {
	g := create(dir, "corpus.txt", "name", "input",
		"interface{} value", "interface{} error",
		"Kitchen value", "Kitchen error",
		"stream (yaml.Decoder into interface{})",
		"config.BidderInfo value", "error",
		"jsonutil.Marshal(interface{})", "json.Marshal(interface{})")
	for _, c := range corpus(repo) {
		var tree interface{}
		treeValue, treeErr, _ := yamlInto(c.input, &tree)
		var kitchen Kitchen
		kitchenValue, kitchenErr, _ := yamlInto(c.input, &kitchen)
		stream := yamlStream(c.input, treeValue+" | "+treeErr)
		var info config.BidderInfo
		infoValue, infoErr, _ := yamlInto(c.input, &info)
		g.row(c.name, string(c.input), treeValue, treeErr, kitchenValue, kitchenErr, stream,
			infoValue, infoErr, marshalJSON(jsonutil.Marshal, tree), marshalJSON(json.Marshal, tree))
	}
	g.close()
}

func writeMapstructure(dir string) {
	columns := []string{"name", "target", "prefill", "input"}
	for _, name := range msConfigNames {
		columns = append(columns, name+" value", name+" error")
	}
	g := create(dir, "mapstructure.txt", columns...)
	for _, c := range msCases() {
		prefill := ""
		if c.prefill != nil {
			prefill = dump(reflect.ValueOf(c.prefill).Elem())
		}
		fields := []string{c.name, c.target, prefill, dump(reflect.ValueOf(&c.input).Elem())}
		results := map[string][2]string{}
		for _, name := range msConfigNames {
			value, text := msDecode(c.target, name, c.prefill, c.input)
			results[name] = [2]string{value, text}
		}
		for _, name := range msConfigNames {
			r := results[name]
			if name != "viper" && r == results["viper"] {
				r = [2]string{"=", ""}
			}
			fields = append(fields, r[0], r[1])
		}
		g.row(fields...)
	}
	g.close()
}
