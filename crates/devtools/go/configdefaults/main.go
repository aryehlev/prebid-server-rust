// Command configdefaults dumps the host configuration defaults to rust/testdata/go_config_defaults.json.
//
// It loads static/bidder-info and calls config.SetupViper on a fresh viper exactly as main.go's
// loadConfig does (config name "pbs"), from an empty working directory and with every PBS_*
// variable removed, so no file or environment value leaks in. It then records:
//
//   - every viper key with its default (viper Get with no file and no env), the default's Go type,
//     the environment variable viper reads for it, and whether that variable was verified to bind
//     (by setting a sentinel value and observing Get change);
//   - the per-bidder adapters.<bidder>.* keys bound by setBidderDefaults, folded into one suffix
//     table plus the list of bidders;
//   - the Configuration config.New produces from those defaults (encoding/json of the struct), the
//     account defaults JSON, and the validation errors config.New reports.
//
// Run it through `pbs-devtools go-dump config-defaults`.
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/spf13/viper"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/rust/crates/devtools/go/internal/dump"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// Same as main.go, so jsonutil.Marshal (account defaults JSON) behaves like the server binary.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

const (
	configFileName = "pbs" // main.go configFileName
	envPrefix      = "PBS"
)

type key struct {
	Key         string          `json:"key"`
	HasDefault  bool            `json:"has_default"`
	Default     json.RawMessage `json:"default"`
	GoType      string          `json:"go_type"`
	Env         string          `json:"env"`
	EnvVerified bool            `json:"env_verified"`
}

type bidderSuffix struct {
	Suffix      string `json:"suffix"`
	EnvSuffix   string `json:"env_suffix"`
	EnvVerified bool   `json:"env_verified"`
}

type bidderKeys struct {
	Note     string         `json:"note"`
	Bidders  []string       `json:"bidders"`
	Suffixes []bidderSuffix `json:"suffixes"`
}

type output struct {
	Generator           string          `json:"generator"`
	GoCommit            string          `json:"go_commit"`
	Notes               []string        `json:"notes"`
	ConfigFileName      string          `json:"config_file_name"`
	EnvPrefix           string          `json:"env_prefix"`
	Keys                []key           `json:"keys"`
	BidderKeys          bidderKeys      `json:"bidder_keys"`
	ConfigNewErrors     []string        `json:"config_new_errors"`
	ConfigNewError      string          `json:"config_new_error"`
	AccountDefaultsJSON string          `json:"account_defaults_json"`
	Configuration       json.RawMessage `json:"configuration"`
}

func main() {
	opts := dump.ParseFlags()
	dump.ClearPBSEnv()

	infoPath := filepath.Join(opts.Root, "static", "bidder-info")
	bidderInfos, err := config.LoadBidderInfoFromDisk(infoPath)
	if err != nil {
		dump.Fatalf("LoadBidderInfoFromDisk(%s): %v", infoPath, err)
	}

	// SetupViper searches "." and /etc/config for pbs.*; run from an empty directory.
	empty, err := os.MkdirTemp("", "pbs-devtools-configdefaults-")
	if err != nil {
		dump.Fatalf("creating temp dir: %v", err)
	}
	defer os.RemoveAll(empty)
	if err := os.Chdir(empty); err != nil {
		dump.Fatalf("chdir %s: %v", empty, err)
	}

	v := viper.New()
	config.SetupViper(v, configFileName, bidderInfos)
	if used := v.ConfigFileUsed(); used != "" {
		dump.Fatalf("viper picked up a config file (%s); remove it or run elsewhere", used)
	}

	bidders := make([]string, 0, len(bidderInfos))
	for name := range bidderInfos {
		bidders = append(bidders, strings.ToLower(name))
	}
	sort.Strings(bidders)

	keys, suffixes := collectKeys(v, bidders)

	cfg, newErr := config.New(v, bidderInfos, openrtb_ext.NormalizeBidderName)
	if cfg == nil {
		dump.Fatalf("config.New returned no configuration: %v", newErr)
	}
	// Struct field order is kept: encoding/json walks the struct, and the golden's encoder only
	// re-indents the raw value.
	cfgJSON := dump.Raw(cfg)

	newErrStr := ""
	if newErr != nil {
		newErrStr = newErr.Error()
	}

	out := output{
		Generator: "pbs-devtools go-dump config-defaults (rust/crates/devtools/go/configdefaults)",
		GoCommit:  opts.GoCommit,
		Notes: []string{
			"keys: sorted viper.AllKeys() after config.SetupViper(v, \"pbs\", bidderInfos), no config file, no PBS_* env; adapters.<bidder>.* keys are folded into bidder_keys.",
			"default: viper Get(key) encoded with encoding/json; null with has_default=false means the key is only env-bound (BindEnv) and has no default.",
			"go_type: fmt %T of the default; viper casts env and file values to this type (SetTypeByDefaultValue).",
			"env: strings.NewReplacer(\".\", \"_\") applied to strings.ToUpper(\"PBS_\" + key); env_verified means setting it changed Get(key).",
			"configuration: encoding/json of *config.Configuration returned by config.New with these defaults (untagged fields use Go names, unexported fields are absent).",
			"account_defaults_json: cfg.AccountDefaultsJSON(), the jsonutil.Marshal output used as the account merge-patch base.",
			"config_new_errors: the AggregateError entries config.New returned, in Go order; config_new_error is its Error() text.",
		},
		ConfigFileName:      configFileName,
		EnvPrefix:           envPrefix,
		Keys:                keys,
		BidderKeys:          bidderKeys{Note: "every bidder in bidders has every key adapters.<bidder>.<suffix>; the env var is PBS_ADAPTERS_<BIDDER>_<env_suffix> with the bidder upper-cased.", Bidders: bidders, Suffixes: suffixes},
		ConfigNewErrors:     errorStrings(newErr),
		ConfigNewError:      newErrStr,
		AccountDefaultsJSON: string(cfg.AccountDefaultsJSON()),
		Configuration:       cfgJSON,
	}
	dump.WriteJSON(opts.Out, out)
}

func envName(k string) string {
	return strings.NewReplacer(".", "_").Replace(strings.ToUpper(envPrefix + "_" + k))
}

// collectKeys splits viper's keys into the general list and the per-bidder suffix table, checking
// that every bidder has exactly the same suffixes.
func collectKeys(v *viper.Viper, bidders []string) ([]key, []bidderSuffix) {
	isBidder := make(map[string]bool, len(bidders))
	for _, b := range bidders {
		isBidder[b] = true
	}

	all := v.AllKeys()
	sort.Strings(all)

	var keys []key
	perBidder := map[string]map[string]bool{}
	for _, k := range all {
		if rest, ok := strings.CutPrefix(k, "adapters."); ok {
			bidder, suffix, found := strings.Cut(rest, ".")
			if found && isBidder[bidder] {
				if perBidder[bidder] == nil {
					perBidder[bidder] = map[string]bool{}
				}
				perBidder[bidder][suffix] = true
				continue
			}
		}
		keys = append(keys, describeKey(v, k))
	}

	var suffixes []string
	if len(bidders) > 0 {
		for s := range perBidder[bidders[0]] {
			suffixes = append(suffixes, s)
		}
		sort.Strings(suffixes)
	}
	for _, b := range bidders {
		if len(perBidder[b]) != len(suffixes) {
			dump.Fatalf("bidder %s has %d adapters.* keys, %s has %d", b, len(perBidder[b]), bidders[0], len(suffixes))
		}
		for _, s := range suffixes {
			if !perBidder[b][s] {
				dump.Fatalf("bidder %s lacks key adapters.%s.%s", b, b, s)
			}
		}
	}

	table := make([]bidderSuffix, 0, len(suffixes))
	for _, s := range suffixes {
		verified := true
		for _, b := range bidders {
			k := "adapters." + b + "." + s
			if v.IsSet(k) {
				dump.Fatalf("per-bidder key %s unexpectedly has a value", k)
			}
			if !envBinds(v, k, envName(k)) {
				verified = false
			}
		}
		table = append(table, bidderSuffix{
			Suffix:      s,
			EnvSuffix:   strings.NewReplacer(".", "_").Replace(strings.ToUpper(s)),
			EnvVerified: verified,
		})
	}
	return keys, table
}

func describeKey(v *viper.Viper, k string) key {
	val := v.Get(k)
	goType := ""
	if val != nil {
		goType = fmt.Sprintf("%T", val)
	}
	env := envName(k)
	return key{
		Key:         k,
		HasDefault:  v.IsSet(k),
		Default:     dump.Raw(val),
		GoType:      goType,
		Env:         env,
		EnvVerified: envBinds(v, k, env),
	}
}

// envBinds sets env to a sentinel of the default's type and reports whether viper's Get(k) changed.
func envBinds(v *viper.Viper, k, env string) bool {
	before := v.Get(k)
	sentinel := "__pbs_devtools_sentinel__"
	switch b := before.(type) {
	case bool:
		sentinel = fmt.Sprint(!b)
	case int, int8, int16, int32, int64, uint, uint8, uint16, uint32, uint64:
		sentinel = "987654321"
		if fmt.Sprint(b) == sentinel {
			sentinel = "123456789"
		}
	case float32, float64:
		sentinel = "0.123"
		if fmt.Sprint(b) == sentinel {
			sentinel = "0.456"
		}
	}
	if err := os.Setenv(env, sentinel); err != nil {
		dump.Fatalf("setenv %s: %v", env, err)
	}
	after := v.Get(k)
	if err := os.Unsetenv(env); err != nil {
		dump.Fatalf("unsetenv %s: %v", env, err)
	}
	return !reflect.DeepEqual(before, after)
}

func errorStrings(err error) []string {
	out := []string{}
	if err == nil {
		return out
	}
	var agg errortypes.AggregateError
	if errors.As(err, &agg) {
		for _, e := range agg.Errors {
			out = append(out, e.Error())
		}
		return out
	}
	return append(out, err.Error())
}
