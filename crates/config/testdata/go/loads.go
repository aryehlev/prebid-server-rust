package main

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/spf13/viper"

	"github.com/prebid/prebid-server/v4/config"
)

// fallbackEnv are the variables the BindEnv calls SetupViper makes before SetEnvPrefix register
// without the PBS_ prefix (config/config.go:1199-1200, 1240, 1298-1299).
var fallbackEnv = []string{
	"ACCOUNT_DEFAULTS_PRIVACY_DSA_DEFAULT",
	"ACCOUNT_DEFAULTS_PRIVACY_DSA_GDPR_ONLY",
	"GDPR_DEFAULT_VALUE",
	"USER_SYNC_COOP_SYNC_DEFAULT",
	"USER_SYNC_EXTERNAL_URL",
}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

// clearEnv removes every variable a load could read: each PBS_ variable, the un-prefixed
// fallbacks, and the variables of extra.
func clearEnv(extra map[string]string) {
	for _, kv := range os.Environ() {
		name, _, _ := strings.Cut(kv, "=")
		if strings.HasPrefix(name, "PBS_") {
			must(os.Unsetenv(name))
		}
	}
	for _, name := range fallbackEnv {
		must(os.Unsetenv(name))
	}
	for name := range extra {
		must(os.Unsetenv(name))
	}
}

// checkEtcConfig fails the run when /etc/config holds a file SetupViper would read: the goldens
// must not depend on the machine.
func checkEtcConfig() {
	for _, ext := range viper.SupportedExts {
		if _, err := os.Stat(filepath.Join("/etc/config", "pbs."+ext)); err == nil {
			panic("/etc/config/pbs." + ext + " exists; the goldens would read it")
		}
	}
}

// loadInput is one load: the files of the working directory (Viper's first search path), those
// of a second directory that stands for /etc/config, the environment, the bidders SetupViper
// binds settings for, and the keys whose IsSet and Get the golden records. A file name ending in
// "/" makes a directory.
type loadInput struct {
	Name    string            `json:"name"`
	Files   map[string]string `json:"files,omitempty"`
	Second  map[string]string `json:"second,omitempty"`
	Env     map[string]string `json:"env,omitempty"`
	Bidders []string          `json:"bidders,omitempty"`
	// NoFile loads as SetupViper(v, "", ...) does: no configuration file is searched.
	NoFile bool     `json:"no_file,omitempty"`
	Probes []string `json:"probes,omitempty"`
	// FileBytes holds the bytes of each file of Files whose contents are not UTF-8, which a JSON
	// string cannot carry (encoding/json writes U+FFFD for each invalid byte).
	FileBytes map[string][]int `json:"file_bytes,omitempty"`
}

// probe is what v.IsSet and v.Get answer for a key after the load.
type probe struct {
	IsSet bool `json:"is_set"`
	Get   any  `json:"get"`
}

// keyResult is one UnmarshalKey call of config.New (config/config.go:886-889): the *bool it
// decoded (null for nil) or its error.
type keyResult struct {
	Value *bool  `json:"value"`
	Error string `json:"error"`
}

// loaded is a Viper set up for one load and what reading the file did.
type loaded struct {
	v         *viper.Viper
	readError string
	fileUsed  string
}

// writeFiles writes files into dir.
func writeFiles(dir string, files map[string]string) {
	for name, contents := range files {
		path := filepath.Join(dir, name)
		if strings.HasSuffix(name, "/") {
			must(os.MkdirAll(path, 0o755))
			continue
		}
		must(os.WriteFile(path, []byte(contents), 0o600))
	}
}

// withViper sets Viper up for in as main.go's loadConfig does (config.SetupViper(v, "pbs",
// bidderInfos), main.go:61-65) in a fresh working directory with exactly in.Env set, and calls
// use with it. With in.Second the file is searched in the working directory and then in a second
// temporary directory, which stands for /etc/config. ReadInConfig is called again after
// SetupViper to learn the error SetupViper ignores (config/config.go:1322); it reads the same
// file, so the settings do not change.
func withViper(in loadInput, use func(l loaded)) {
	clearEnv(in.Env)
	for name, value := range in.Env {
		must(os.Setenv(name, value))
	}
	defer clearEnv(in.Env)

	wd, err := os.Getwd()
	must(err)
	dir, err := os.MkdirTemp("", "pbs-config-golden-")
	must(err)
	defer func() { must(os.RemoveAll(dir)) }()
	writeFiles(dir, in.Files)
	second := ""
	if in.Second != nil {
		second, err = os.MkdirTemp("", "pbs-config-golden-etc-")
		must(err)
		defer func() { must(os.RemoveAll(second)) }()
		writeFiles(second, in.Second)
	}
	must(os.Chdir(dir))
	defer func() { must(os.Chdir(wd)) }()

	bidderInfos := config.BidderInfos{}
	for _, b := range in.Bidders {
		bidderInfos[b] = config.BidderInfo{}
	}
	v := viper.New()
	l := loaded{v: v}
	switch {
	case in.NoFile:
		config.SetupViper(v, "", bidderInfos)
	case second != "":
		config.SetupViper(v, "", bidderInfos)
		v.SetConfigName("pbs")
		v.AddConfigPath(".")
		v.AddConfigPath(second)
	default:
		config.SetupViper(v, "pbs", bidderInfos)
	}
	if !in.NoFile {
		if err := v.ReadInConfig(); err != nil {
			l.readError = sanitize(err.Error(), dir, second)
		}
		if used := v.ConfigFileUsed(); used != "" {
			where := "first"
			if second != "" && strings.HasPrefix(used, second) {
				where = "second"
			}
			l.fileUsed = where + ":" + filepath.Base(used)
		}
	}
	use(l)
}

// sanitize replaces the temporary directories in an error text with <wd> and <second>, so the
// golden does not depend on where they were made.
func sanitize(text, dir, second string) string {
	for _, d := range []struct{ path, name string }{{dir, "<wd>"}, {second, "<second>"}} {
		if d.path == "" {
			continue
		}
		if real, err := filepath.EvalSymlinks(d.path); err == nil {
			text = strings.ReplaceAll(text, real, d.name)
		}
		text = strings.ReplaceAll(text, d.path, d.name)
	}
	return text
}

// decode is config.New's decode (config/config.go:758). A panic, which crashes Prebid Server at
// startup, is recorded as an error: "panic: " and the panic's value.
func decode(v *viper.Viper) (c config.Configuration, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("panic: %v", r)
		}
	}()
	err = v.Unmarshal(&c, viper.DecodeHook(config.AccountModulesHookFunc()))
	return c, err
}

// loadResult is what one load produced.
type loadResult struct {
	// ReadError is ReadInConfig's error, which SetupViper ignores; empty when a file was read.
	ReadError string `json:"read_error"`
	// FileUsed is the file Viper read: first:<name> or second:<name>, empty when none.
	FileUsed string `json:"file_used"`
	// Error is v.Unmarshal's error, empty when it decoded.
	Error string `json:"error"`
	// Settings is v.AllSettings(), the tree v.Unmarshal decodes, as a diff against the defaults
	// load's.
	Settings treeDiff `json:"settings"`
	// Decoded is the decoded config.Configuration (typed) as a diff against the defaults load's;
	// absent when the decode failed.
	Decoded      *treeDiff            `json:"decoded,omitempty"`
	ProbeResults map[string]probe     `json:"probe_results,omitempty"`
	UnmarshalKey map[string]keyResult `json:"unmarshal_key,omitempty"`
}

// fullResult is a load's settings and decoded configuration in full.
type fullResult struct {
	settings any
	decoded  any
	result   loadResult
}

// runOnce performs one load in config.New's order: v.Unmarshal first, then the UnmarshalKey
// pair for every bidder of the decoded configuration (setConfigBidderInfoNillableFields), then the
// probes, and last v.AllSettings() for the golden (which v.Unmarshal computed first).
func runOnce(in loadInput) fullResult {
	var out fullResult
	withViper(in, func(l loaded) {
		out.result.ReadError = l.readError
		out.result.FileUsed = l.fileUsed
		c, err := decode(l.v)
		if err != nil {
			out.result.Error = err.Error()
		} else {
			out.decoded = typed(reflect.ValueOf(c))
		}
		if err == nil && len(c.BidderInfos) > 0 {
			out.result.UnmarshalKey = map[string]keyResult{}
			for bidder := range c.BidderInfos {
				for _, suffix := range []string{".disabled", ".modifyingvastxmlallowed"} {
					key := "adapters." + bidder + suffix
					var value *bool
					r := keyResult{}
					if err := l.v.UnmarshalKey(key, &value); err != nil {
						r.Error = err.Error()
					}
					r.Value = value
					out.result.UnmarshalKey[key] = r
				}
			}
		}
		if len(in.Probes) > 0 {
			out.result.ProbeResults = map[string]probe{}
			for _, key := range in.Probes {
				out.result.ProbeResults[key] = probe{IsSet: l.v.IsSet(key), Get: dyn(l.v.Get(key))}
			}
		}
		out.settings = dyn(l.v.AllSettings())
	})
	return out
}

// against fills r's diffs against the defaults load.
func (f fullResult) against(base fullResult) loadResult {
	r := f.result
	r.Settings = diffTrees(base.settings, f.settings)
	if f.decoded != nil {
		d := diffTrees(base.decoded, f.decoded)
		r.Decoded = &d
	}
	return r
}

// orderedRuns is how often a load runs to show its result does not depend on Go's randomised
// map order; unorderedRuns how often a load that does depend on it runs to see every outcome.
const (
	orderedRuns   = 8
	unorderedRuns = 200
)

// loadOutput is one load's golden.
type loadOutput struct {
	loadInput
	loadResult
}

// unorderedOutput is the golden of a load whose result depends on the order Go iterates maps
// in: every distinct outcome seen, in the order of their encoding.
type unorderedOutput struct {
	loadInput
	Outcomes []loadResult `json:"outcomes"`
}

// loadsDoc is loads.json.
type loadsDoc struct {
	Header        header            `json:"header"`
	Notes         []string          `json:"notes"`
	SupportedExts []string          `json:"supported_exts"`
	Defaults      defaultsOutput    `json:"defaults"`
	Loads         []loadOutput      `json:"loads"`
	Unordered     []unorderedOutput `json:"unordered"`
}

// defaultsOutput is the load with no file and no environment, in full: the base of every diff.
type defaultsOutput struct {
	loadInput
	Settings     any              `json:"settings"`
	Decoded      any              `json:"decoded"`
	Error        string           `json:"error"`
	ProbeResults map[string]probe `json:"probe_results"`
}

// defaultsInput is the base load.
var defaultsInput = loadInput{
	Name:    "defaults",
	NoFile:  true,
	Bidders: []string{"appnexus", "rubicon"},
	Probes:  []string{"gdpr.default_value", "port", "host_schain_node", "gdpr", "adapters.appnexus.disabled"},
}

func loadsGolden() loadsDoc {
	checkEtcConfig()
	base := runOnce(defaultsInput)
	doc := loadsDoc{
		Header: newHeader("github.com/spf13/viper", "github.com/mitchellh/mapstructure",
			"github.com/spf13/cast", "gopkg.in/yaml.v3", "github.com/pelletier/go-toml/v2"),
		Notes: []string{
			"Written by crates/config/testdata/go (go run .); do not edit.",
			"Each load sets Viper up as main.go does (config.SetupViper(v, \"pbs\", bidders)) in a fresh working directory holding files, with exactly env set, and decodes as config.New does (v.Unmarshal with AccountModulesHookFunc).",
			"settings is v.AllSettings() and decoded the decoded config.Configuration (its non-zero exported fields under their mapstructure keys, recursively), each as a diff against the defaults load: set lists the leaves (a path and a value) that differ from or are missing in the defaults, unset the defaults' leaf paths the load lacks. A leaf is a value that is not a non-empty object; tagged values ({\"$float64\": ...}) are leaves.",
			"Values are in the compact form of encode.go: an int is an integer, a float a number with a '.' or an exponent, int64 and uint64 are tagged.",
			"With second, the file is searched in the working directory and then a second directory standing for /etc/config.",
			"unordered holds loads whose result depends on the order Go iterates maps in (Viper's AllSettings), with every outcome seen in 200 runs.",
			"An error \"panic: ...\" is a panic of the decode, which crashes Prebid Server at startup.",
		},
		SupportedExts: viper.SupportedExts,
		Defaults: defaultsOutput{
			loadInput:    defaultsInput,
			Settings:     base.settings,
			Decoded:      base.decoded,
			Error:        base.result.Error,
			ProbeResults: base.result.ProbeResults,
		},
	}
	for _, in := range loadCases() {
		in = withDefaultBidders(in)
		first := runOnce(in)
		want := encodeGolden(first.against(base))
		for i := 1; i < orderedRuns; i++ {
			if !bytes.Equal(encodeGolden(runOnce(in).against(base)), want) {
				panic("load " + in.Name + " depends on Go's map order: move it to unorderedCases")
			}
		}
		doc.Loads = append(doc.Loads, loadOutput{loadInput: in, loadResult: first.against(base)})
	}
	for _, in := range unorderedCases() {
		in = withDefaultBidders(in)
		seen := map[string]loadResult{}
		for i := 0; i < unorderedRuns; i++ {
			r := runOnce(in).against(base)
			seen[string(encodeGolden(r))] = r
		}
		if len(seen) < 2 {
			panic("load " + in.Name + " gave one outcome in every run: move it to loadCases")
		}
		out := unorderedOutput{loadInput: in}
		for _, k := range sortedKeys(seen) {
			out.Outcomes = append(out.Outcomes, seen[k])
		}
		doc.Unordered = append(doc.Unordered, out)
	}
	return doc
}

// withDefaultBidders gives a load without bidders those of the defaults load, so its diff
// against the defaults is about its own input, and records the bytes of its files that are not
// UTF-8.
func withDefaultBidders(in loadInput) loadInput {
	if in.Bidders == nil {
		in.Bidders = defaultsInput.Bidders
	}
	for name, contents := range in.Files {
		if utf8.ValidString(contents) {
			continue
		}
		if in.FileBytes == nil {
			in.FileBytes = map[string][]int{}
		}
		for _, b := range []byte(contents) {
			in.FileBytes[name] = append(in.FileBytes[name], int(b))
		}
	}
	for name, contents := range in.Second {
		if !utf8.ValidString(contents) {
			panic("load " + in.Name + ": the second directory's " + name + " is not UTF-8")
		}
	}
	return in
}

// leafEntry is one leaf of a tree: its path and its value.
type leafEntry struct {
	Path  []string `json:"path"`
	Value any      `json:"value"`
}

// treeDiff is a tree against a base tree: the leaves that differ from or are missing in the
// base, and the base's leaf paths the tree lacks, each in path order.
type treeDiff struct {
	Set   []leafEntry `json:"set"`
	Unset [][]string  `json:"unset"`
}

// isTagged reports whether m is a tagged value of the compact form.
func isTagged(m map[string]any) bool {
	for k := range m {
		return len(m) == 1 && strings.HasPrefix(k, "$")
	}
	return false
}

// flatten collects the leaves of an encoded tree, keyed by their path joined with NUL.
func flatten(v any, path []string, out map[string]leafEntry) {
	if m, ok := v.(map[string]any); ok && len(m) > 0 && !isTagged(m) {
		for k, e := range m {
			flatten(e, append(append([]string(nil), path...), k), out)
		}
		return
	}
	out[strings.Join(path, "\x00")] = leafEntry{Path: path, Value: v}
}

// diffTrees is tree against base.
func diffTrees(base, tree any) treeDiff {
	b, t := map[string]leafEntry{}, map[string]leafEntry{}
	flatten(base, []string{}, b)
	flatten(tree, []string{}, t)
	d := treeDiff{Set: []leafEntry{}, Unset: [][]string{}}
	for _, k := range sortedKeys(t) {
		if old, ok := b[k]; !ok || !bytes.Equal(encodeGolden(old.Value), encodeGolden(t[k].Value)) {
			d.Set = append(d.Set, t[k])
		}
	}
	for _, k := range sortedKeys(b) {
		if _, ok := t[k]; !ok {
			d.Unset = append(d.Unset, b[k].Path)
		}
	}
	return d
}

// sortedKeys are the keys of m in byte order.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
