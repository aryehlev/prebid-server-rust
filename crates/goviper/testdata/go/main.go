// Command golden writes the differential goldens that pbs-goviper's tests replay.
//
// A scenario is a list of operations on a fresh viper.New() (github.com/spf13/viper v1.12.0,
// with spf13/cast v1.5.0 and mapstructure v1.5.0), run in a new temporary directory that holds
// the scenario's files, with the scenario's variables as the whole process environment and the
// directory (or a subdirectory of it) as the working directory. Setup operations are viper's
// setters (SetDefault, Set, BindEnv, SetEnvPrefix, SetEnvKeyReplacer, AutomaticEnv,
// AllowEmptyEnv, SetTypeByDefaultValue, SetConfigName, SetConfigType, SetConfigFile,
// AddConfigPath, ReadInConfig, ReadConfig); observations are Get and every typed getter, IsSet,
// InConfig, AllKeys, AllSettings, Sub, Unmarshal, UnmarshalKey and UnmarshalExact. Each operation
// is written with Go's result in "want": values rendered by dump, errors by err.Error(), decoded
// values by dump and the error. Values passed to SetDefault and Set are written as specs (see
// spec). The temporary directory is written as @TMP@, in arguments and results.
//
// Every scenario runs several times, and its results must not change: Go's random map
// iteration order decides some outcomes (see pbs-goviper's "Not ported" notes), and a random
// scenario that meets such a case is left out.
//
//   - scenarios.jsonl: hand-written scenarios (ports of viper's own tests and of Prebid's
//     main_test.go) and seeded random ones, one per line.
//   - prebid.jsonl: config.SetupViper, written as the operations it performs (rebuilt from
//     rust/testdata/go_config_defaults.json; the generator checks that they give SetupViper's own
//     keys and values), with a pbs.yaml and PBS_* variables.
//
// Run it from the repository root, as amd64 code (Go's float-to-integer conversions, which cast
// uses, differ between architectures, and the crate reproduces amd64):
//
//	GOARCH=amd64 go run ./rust/crates/goviper/testdata/go
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/viper"
)

func init() {
	// cast.ToTime gives Unix times in Local; fix it so that the goldens do not depend on the
	// machine (Prebid's containers run in UTC, and pbs-gocompat's time treats Local as UTC).
	time.Local = time.UTC
}

// runs is how many times each scenario runs (enough that a random scenario whose outcome
// depends on Go's map order is very unlikely to look stable).
const runs = 24

func main() {
	out := flag.String("out", "rust/crates/goviper/testdata/golden", "golden directory")
	defaults := flag.String("defaults", "rust/testdata/go_config_defaults.json", "config.SetupViper's keys")
	flag.Parse()
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}
	scenarios := append(handScenarios(), randomScenarios(20261117, 64)...)
	writeScenarios(filepath.Join(*out, "scenarios.jsonl"), scenarios)
	writeScenarios(filepath.Join(*out, "prebid.jsonl"), prebidScenarios(*defaults))
	writeScenarios(filepath.Join(*out, "review.jsonl"), reviewScenarios())
}

// Scenario is one golden case.
type Scenario struct {
	Name string `json:"name"`
	// Files are written under the temporary directory, by relative path.
	Files map[string]string `json:"files,omitempty"`
	// Dirs are created under the temporary directory.
	Dirs []string `json:"dirs,omitempty"`
	// Links are symbolic links (path: target), created after the files.
	Links map[string]string `json:"links,omitempty"`
	// Env is the whole process environment (values may contain @TMP@).
	Env map[string]string `json:"env,omitempty"`
	// Cwd is the working directory, relative to the temporary directory.
	Cwd string `json:"cwd,omitempty"`
	Ops []*Op  `json:"ops"`

	// random marks a generated scenario, left out when its results are not stable.
	random bool
	// verify, if set, checks the viper the operations built.
	verify func(v *viper.Viper)
}

// Op is one operation and its outcome.
type Op struct {
	Op    string   `json:"op"`
	Key   string   `json:"key,omitempty"`
	Arg   string   `json:"arg,omitempty"`
	Args  []string `json:"args,omitempty"`
	Pairs []string `json:"pairs,omitempty"`
	// Suffixes are bind_bidder_envs's: BindEnv("adapters.<bidder>.<suffix>") for each of Args.
	Suffixes []string    `json:"suffixes,omitempty"`
	Value    interface{} `json:"value,omitempty"`
	Target   string      `json:"target,omitempty"`
	Want     interface{} `json:"want"`

	val    interface{} // the Go value of set and set_default
	expect interface{} // the outcome a ported Go test asserts, checked here
}

func writeScenarios(path string, scenarios []*Scenario) {
	f, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(f)
	// One scenario per line (Encode ends each with a newline), with <, > and & as themselves.
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	written, ops := 0, 0
	for _, s := range scenarios {
		wants := runScenario(s)
		unstable := -1
		for i := 1; i < runs && unstable < 0; i++ {
			again := runScenario(s)
			for j := range wants {
				if !reflect.DeepEqual(wants[j], again[j]) {
					unstable = j
					break
				}
			}
		}
		if unstable >= 0 {
			o := s.Ops[unstable]
			if !s.random {
				log.Fatalf("%s: %s %q changes from run to run", s.Name, o.Op, o.Key)
			}
			fmt.Printf("%s: left out, %s %q changes from run to run\n", s.Name, o.Op, o.Key)
			continue
		}
		for i, o := range s.Ops {
			o.Want = wants[i]
		}
		if err := enc.Encode(s); err != nil {
			log.Fatalf("%s: %v", s.Name, err)
		}
		written++
		ops += len(s.Ops)
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := f.Close(); err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s: %d scenarios, %d operations\n", path, written, ops)
}

// runScenario runs the operations and returns their outcomes.
func runScenario(s *Scenario) []interface{} {
	tmp, err := os.MkdirTemp("", "goviper")
	if err != nil {
		log.Fatal(err)
	}
	// The kernel reports the working directory with symbolic links resolved (/private/var on
	// macOS), so the placeholder stands for the resolved path.
	if tmp, err = filepath.EvalSymlinks(tmp); err != nil {
		log.Fatal(err)
	}
	defer os.RemoveAll(tmp)
	for _, dir := range s.Dirs {
		if err := os.MkdirAll(filepath.Join(tmp, dir), 0o755); err != nil {
			log.Fatal(err)
		}
	}
	for name, content := range s.Files {
		path := filepath.Join(tmp, name)
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			log.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			log.Fatal(err)
		}
	}
	for name, target := range s.Links {
		if err := os.Symlink(target, filepath.Join(tmp, name)); err != nil {
			log.Fatal(err)
		}
	}
	wd, err := os.Getwd()
	if err != nil {
		log.Fatal(err)
	}
	if err := os.Chdir(filepath.Join(tmp, s.Cwd)); err != nil {
		log.Fatal(err)
	}
	defer func() {
		if err := os.Chdir(wd); err != nil {
			log.Fatal(err)
		}
	}()
	env := os.Environ()
	os.Clearenv()
	defer restoreEnv(env)
	for k, v := range s.Env {
		if err := os.Setenv(k, strings.ReplaceAll(v, "@TMP@", tmp)); err != nil {
			log.Fatal(err)
		}
	}
	v := viper.New()
	wants := make([]interface{}, len(s.Ops))
	for i, o := range s.Ops {
		func() {
			defer func() {
				if r := recover(); r != nil {
					log.Fatalf("%s: %s %q panicked: %v", s.Name, o.Op, o.Key, r)
				}
			}()
			wants[i] = untmp(runOp(v, o, tmp), tmp)
		}()
		if o.expect != nil && !reflect.DeepEqual(o.expect, wants[i]) {
			log.Fatalf("%s: %s %q = %#v, the ported test expects %#v", s.Name, o.Op, o.Key, wants[i], o.expect)
		}
	}
	if s.verify != nil {
		s.verify(v)
	}
	return wants
}

func restoreEnv(env []string) {
	os.Clearenv()
	for _, kv := range env {
		if i := strings.IndexByte(kv, '='); i > 0 {
			if err := os.Setenv(kv[:i], kv[i+1:]); err != nil {
				log.Fatal(err)
			}
		}
	}
}

// untmp writes the temporary directory of a result as @TMP@.
func untmp(want interface{}, tmp string) interface{} {
	switch w := want.(type) {
	case string:
		return strings.ReplaceAll(w, tmp, "@TMP@")
	case []string:
		out := make([]string, len(w))
		for i, s := range w {
			out[i] = strings.ReplaceAll(s, tmp, "@TMP@")
		}
		return out
	}
	return want
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// runOp runs one operation: nil for a setter, else the outcome.
func runOp(v *viper.Viper, o *Op, tmp string) interface{} {
	arg := strings.ReplaceAll(o.Arg, "@TMP@", tmp)
	k := o.Key
	switch o.Op {
	case "set_default":
		v.SetDefault(k, o.val)
	case "set":
		v.Set(k, o.val)
	case "bind_env":
		return errText(v.BindEnv(o.Args...))
	case "bind_bidder_envs":
		for _, b := range o.Args {
			for _, s := range o.Suffixes {
				if err := v.BindEnv("adapters." + b + "." + s); err != nil {
					return err.Error()
				}
			}
		}
		return ""
	case "set_env_prefix":
		v.SetEnvPrefix(arg)
	case "set_env_key_replacer":
		// (A nil *strings.Replacer would make the next lookup panic.)
		v.SetEnvKeyReplacer(strings.NewReplacer(o.Pairs...))
	case "automatic_env":
		v.AutomaticEnv()
	case "allow_empty_env":
		v.AllowEmptyEnv(arg == "true")
	case "set_type_by_default_value":
		v.SetTypeByDefaultValue(arg == "true")
	case "set_config_name":
		v.SetConfigName(arg)
	case "set_config_type":
		v.SetConfigType(arg)
	case "set_config_file":
		v.SetConfigFile(arg)
	case "add_config_path":
		v.AddConfigPath(arg)
	case "read_in_config":
		return errText(v.ReadInConfig())
	case "read_config":
		return errText(v.ReadConfig(strings.NewReader(arg)))
	case "config_file_used":
		return v.ConfigFileUsed()
	case "get":
		x := v.Get(k)
		return dump(reflect.ValueOf(&x).Elem())
	case "get_string":
		return dumpOf(v.GetString(k))
	case "get_bool":
		return dumpOf(v.GetBool(k))
	case "get_int":
		return dumpOf(v.GetInt(k))
	case "get_int32":
		return dumpOf(v.GetInt32(k))
	case "get_int64":
		return dumpOf(v.GetInt64(k))
	case "get_uint":
		return dumpOf(v.GetUint(k))
	case "get_uint32":
		return dumpOf(v.GetUint32(k))
	case "get_uint64":
		return dumpOf(v.GetUint64(k))
	case "get_float64":
		return dumpOf(v.GetFloat64(k))
	case "get_time":
		return dumpOf(v.GetTime(k))
	case "get_duration":
		return dumpOf(v.GetDuration(k))
	case "get_int_slice":
		return dumpOf(v.GetIntSlice(k))
	case "get_string_slice":
		return dumpOf(v.GetStringSlice(k))
	case "get_string_map":
		return dumpOf(v.GetStringMap(k))
	case "get_string_map_string":
		return dumpOf(v.GetStringMapString(k))
	case "get_string_map_string_slice":
		return dumpOf(v.GetStringMapStringSlice(k))
	case "get_size_in_bytes":
		return dumpOf(v.GetSizeInBytes(k))
	case "is_set":
		return v.IsSet(k)
	case "in_config":
		return v.InConfig(k)
	case "all_keys":
		keys := v.AllKeys()
		sort.Strings(keys)
		return keys
	case "all_settings":
		return dumpOf(v.AllSettings())
	case "sub":
		sub := v.Sub(k)
		if sub == nil {
			return "nil"
		}
		return dumpOf(sub.AllSettings())
	case "unmarshal", "unmarshal_exact", "unmarshal_key":
		target := newTarget(o.Target)
		var err error
		switch o.Op {
		case "unmarshal":
			err = v.Unmarshal(target)
		case "unmarshal_exact":
			err = v.UnmarshalExact(target)
		default:
			err = v.UnmarshalKey(k, target)
		}
		return dump(reflect.ValueOf(target).Elem()) + " | " + errText(err)
	default:
		log.Fatalf("unknown operation %q", o.Op)
	}
	return nil
}

// ---- values ----------------------------------------------------------------------------------

var (
	timeType     = reflect.TypeOf(time.Time{})
	durationType = reflect.TypeOf(time.Duration(0))
)

// spec renders a value held in an interface{} for the Rust side: ["nil"]; [type, text] for a
// scalar (integers and floats as strconv formats them, a time.Time in RFC 3339 with
// nanoseconds, a time.Duration in nanoseconds, a bool as itself); ["slice", elem, items or
// null] and ["map", key, elem, [[k, v], ...] or null], each item a spec of its own, map entries
// sorted by their JSON.
func spec(v reflect.Value) interface{} {
	if v.Kind() == reflect.Interface {
		if v.IsNil() {
			return []interface{}{"nil"}
		}
		v = v.Elem()
	}
	t := v.Type()
	switch t {
	case durationType:
		return []interface{}{"time.Duration", strconv.FormatInt(v.Int(), 10)}
	case timeType:
		return []interface{}{"time.Time", v.Interface().(time.Time).Format(time.RFC3339Nano)}
	}
	switch v.Kind() {
	case reflect.Bool:
		return []interface{}{t.String(), v.Bool()}
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return []interface{}{t.String(), strconv.FormatInt(v.Int(), 10)}
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return []interface{}{t.String(), strconv.FormatUint(v.Uint(), 10)}
	case reflect.Float32:
		return []interface{}{t.String(), strconv.FormatFloat(v.Float(), 'g', -1, 32)}
	case reflect.Float64:
		return []interface{}{t.String(), strconv.FormatFloat(v.Float(), 'g', -1, 64)}
	case reflect.String:
		return []interface{}{t.String(), v.String()}
	case reflect.Slice:
		if v.IsNil() {
			return []interface{}{"slice", t.Elem().String(), nil}
		}
		items := make([]interface{}, v.Len())
		for i := range items {
			items[i] = spec(v.Index(i))
		}
		return []interface{}{"slice", t.Elem().String(), items}
	case reflect.Map:
		if v.IsNil() {
			return []interface{}{"map", t.Key().String(), t.Elem().String(), nil}
		}
		type entry struct {
			sortKey string
			pair    []interface{}
		}
		var entries []entry
		iter := v.MapRange()
		for iter.Next() {
			k := spec(iter.Key())
			sortKey, err := json.Marshal(k)
			if err != nil {
				log.Fatal(err)
			}
			entries = append(entries, entry{string(sortKey), []interface{}{k, spec(iter.Value())}})
		}
		sort.Slice(entries, func(i, j int) bool { return entries[i].sortKey < entries[j].sortKey })
		pairs := make([]interface{}, len(entries))
		for i, e := range entries {
			pairs[i] = e.pair
		}
		return []interface{}{"map", t.Key().String(), t.Elem().String(), pairs}
	}
	log.Fatalf("spec: unsupported type %s", t)
	return nil
}

func dumpOf(x interface{}) string {
	return dump(reflect.ValueOf(x))
}

// dump renders a value: scalars as Go literals (strconv.Quote, FormatInt, FormatFloat 'g' -1),
// pointers as & and the pointee, nil pointers, slices, maps and interfaces as nil, slices as
// [a, b], maps as {k: v, ...} sorted by their rendering, structs as {Name:value ...} with the
// exported non-zero fields, time.Time as time(t.String()), and an interface value as <its
// dynamic type> and the value (pbs-goyaml's goldens render values the same way).
func dump(v reflect.Value) string {
	var b strings.Builder
	dumpTo(&b, v)
	return b.String()
}

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

// ---- unmarshal targets -----------------------------------------------------------------------

// Inner is a decoding target with a field of each common kind; mapstructure matches the field
// names case-insensitively.
type Inner struct {
	Name    string
	Count   int
	Enabled bool
	Ratio   float64
	Tags    []string
	Labels  map[string]string
	Timeout time.Duration
	Ptr     *int
	Any     interface{}
}

// Outer nests Inner as a struct, a slice and a map.
type Outer struct {
	Host   string
	Port   int
	Flag   *bool
	Nums   []int
	Uints  uint32
	Inner  Inner
	Items  []Inner
	ByName map[string]Inner `mapstructure:"by_name"`
	Misc   map[string]interface{}
}

// Unused is viper's testUnmarshalExtra.
type Unused struct {
	Existing bool
}

// ViperConfig is the target of viper's TestUnmarshal.
type ViperConfig struct {
	Port     int
	Name     string
	Duration time.Duration
	Modes    []int
}

// newTarget is a pointer to a new decoding target.
func newTarget(name string) interface{} {
	switch name {
	case "outer":
		return new(Outer)
	case "inner":
		return new(Inner)
	case "unused":
		return new(Unused)
	case "viper_config":
		return new(ViperConfig)
	case "map":
		return new(map[string]interface{})
	case "strings":
		return new([]string)
	case "ints":
		return new([]int)
	case "int":
		return new(int)
	case "string":
		return new(string)
	case "bool_ptr":
		return new(*bool)
	case "duration":
		return new(time.Duration)
	case "string_map_string":
		return new(map[string]string)
	}
	log.Fatalf("unknown target %q", name)
	return nil
}
