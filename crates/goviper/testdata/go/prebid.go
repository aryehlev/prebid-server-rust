package main

import (
	"encoding/json"
	"log"
	"os"
	"reflect"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/spf13/viper"
)

// configDefaults is the part of rust/testdata/go_config_defaults.json that describes what
// config.SetupViper registers.
type configDefaults struct {
	ConfigFileName string `json:"config_file_name"`
	EnvPrefix      string `json:"env_prefix"`
	Keys           []struct {
		Key          string          `json:"key"`
		HasDefault   bool            `json:"has_default"`
		Default      json.RawMessage `json:"default"`
		GoType       string          `json:"go_type"`
		EnvFallbacks []struct {
			Env string `json:"env"`
		} `json:"env_fallbacks"`
	} `json:"keys"`
	BidderKeys struct {
		Bidders  []string `json:"bidders"`
		Suffixes []struct {
			Suffix string `json:"suffix"`
		} `json:"suffixes"`
	} `json:"bidder_keys"`
}

// namedTypes are the Go types of SetupViper's defaults that the Rust side holds as an empty
// []interface{}, with their renderings in dump.
var namedTypes = []string{"<[]config.AgmaAnalyticsAccount>", "<[]openrtb_ext.BidderName>"}

// normalize renders the named types of a dump as []interface {}.
func normalize(s string) string {
	for _, name := range namedTypes {
		s = strings.ReplaceAll(s, name, "<[]interface {}>")
	}
	return s
}

func loadDefaults(path string) *configDefaults {
	data, err := os.ReadFile(path)
	if err != nil {
		log.Fatal(err)
	}
	var d configDefaults
	if err := json.Unmarshal(data, &d); err != nil {
		log.Fatal(err)
	}
	return &d
}

// defaultValue rebuilds a SetupViper default from its JSON and Go type.
func defaultValue(key, goType string, raw json.RawMessage) interface{} {
	var target interface{}
	switch goType {
	case "string":
		target = new(string)
	case "int":
		target = new(int)
	case "bool":
		target = new(bool)
	case "float64":
		target = new(float64)
	case "[]string":
		target = new([]string)
	case "[][]string":
		target = new([][]string)
	case "[]config.AgmaAnalyticsAccount", "[]openrtb_ext.BidderName":
		// Only an empty slice of a named type stands as []interface{}.
		if string(raw) != "[]" {
			log.Fatalf("%s: a non-empty %s default", key, goType)
		}
		return []interface{}{}
	default:
		log.Fatalf("%s: unexpected default type %q", key, goType)
	}
	if err := json.Unmarshal(raw, target); err != nil {
		log.Fatalf("%s: %v", key, err)
	}
	return reflect.ValueOf(target).Elem().Interface()
}

// setupOps are the operations config.SetupViper(v, filename, bidderInfos) performs, rebuilt
// from go_config_defaults.json, in its order: the configuration file's name and paths, the
// defaults, the bindings made before the prefix, the environment settings, ReadInConfig, and a
// binding of each bidder setting for every bidder. (SetupViper sets some defaults after
// ReadInConfig; defaults do not change what ReadInConfig does.)
func setupOps(d *configDefaults, filename string, bidders []string) []*Op {
	var ops []*Op
	if filename != "" {
		ops = append(ops, configName(filename), addConfigPath("."), addConfigPath("/etc/config"))
	}
	for _, k := range d.Keys {
		switch {
		case k.HasDefault:
			ops = append(ops, setDefault(k.Key, defaultValue(k.Key, k.GoType, k.Default)))
		case k.GoType == "" && len(k.EnvFallbacks) == 0:
			// SetDefault(key, nil), as for host_schain_node.
			ops = append(ops, setDefault(k.Key, nil))
		}
	}
	for _, k := range d.Keys {
		if len(k.EnvFallbacks) > 0 {
			ops = append(ops, bindEnv(k.Key))
		}
	}
	ops = append(ops, replacer(".", "_"), typeByDefault(true), envPrefix(d.EnvPrefix), autoEnv(),
		readInConfig())
	if len(bidders) > 0 {
		lower := make([]string, len(bidders))
		for i, b := range bidders {
			lower[i] = strings.ToLower(b)
		}
		suffixes := make([]string, len(d.BidderKeys.Suffixes))
		for i, s := range d.BidderKeys.Suffixes {
			suffixes[i] = s.Suffix
		}
		// setBidderDefaults, for each bidder.
		ops = append(ops, &Op{Op: "bind_bidder_envs", Args: lower, Suffixes: suffixes})
	}
	return ops
}

// verifySetup checks that the operations built what config.SetupViper builds, in the same
// environment and directory.
func verifySetup(name, filename string, bidders []string) func(v *viper.Viper) {
	return func(v *viper.Viper) {
		infos := config.BidderInfos{}
		for _, b := range bidders {
			infos[b] = config.BidderInfo{}
		}
		orig := viper.New()
		config.SetupViper(orig, filename, infos)
		keys, origKeys := v.AllKeys(), orig.AllKeys()
		sort.Strings(keys)
		sort.Strings(origKeys)
		if !reflect.DeepEqual(keys, origKeys) {
			log.Fatalf("%s: the rebuilt operations give other keys than SetupViper", name)
		}
		for _, k := range keys {
			got, want := v.Get(k), orig.Get(k)
			g := dump(reflect.ValueOf(&got).Elem())
			w := normalize(dump(reflect.ValueOf(&want).Elem()))
			if g != w || v.IsSet(k) != orig.IsSet(k) {
				log.Fatalf("%s: %s is %s after the rebuilt operations, %s after SetupViper", name, k, g, w)
			}
		}
		if g, w := dumpOf(v.AllSettings()), normalize(dumpOf(orig.AllSettings())); g != w {
			log.Fatalf("%s: AllSettings is\n%s\nafter the rebuilt operations,\n%s\nafter SetupViper", name, g, w)
		}
	}
}

// pbsYAML is a host configuration in the shape Prebid's documentation and deployments use.
const pbsYAML = `host: 127.0.0.1
port: 8000
external_url: https://prebid.example.com
status_response: ok
gdpr:
  host_vendor_id: 52
  default_value: "1"
  eea_countries: [FRA, DEU]
  tcf2:
    purpose1:
      vendor_exceptions: [appnexus]
    special_feature1:
      vendor_exceptions: [rubicon]
host_cookie:
  ttl_days: 30
  domain: example.com
  cookie_name: uids
adapters:
  appnexus:
    endpoint: http://ib.adnxs.com/openrtb2
    platform_id: "5"
    usersync:
      key: adnxs
      redirect:
        url: https://sync.example.com/{{.GDPR}}
  Rubicon:
    disabled: false
    xapi:
      username: user
  newbidder:
    endpoint: http://new.example.com
stored_requests:
  filesystem:
    enabled: true
    directorypath: ./stored
accounts:
  filesystem:
    enabled: true
user_sync:
  priority_groups:
    - [appnexus, rubicon]
    - [ix]
analytics:
  agma:
    accounts:
      - code: abc
        publisher_id: "1"
request_validation:
  ipv4_private_networks: ["10.0.0.0/8"]
CamelCase:
  Key: value
host_schain_node:
  asi: example.com
  sid: "00001"
  hp: 1
metrics:
  prometheus:
    port: 9090
  influxdb:
    host: ""
max_request_size: 1e6
compression:
  response:
    enable_gzip: yes
`

// pbsEnv sets settings as PBS_* variables, in the ways operators do.
var pbsEnv = map[string]string{
	"PBS_PORT":                 "7777",
	"PBS_HOST_COOKIE_TTL_DAYS": "60",
	"PBS_EXPERIMENT_ADSCERT_INPROCESS_ORIGIN":      "not_an_endpoint",
	"PBS_REQUEST_VALIDATION_IPV4_PRIVATE_NETWORKS": "1.1.1.1/24 2.2.2.2/24",
	"PBS_GDPR_EEA_COUNTRIES":                       "FRA,DEU ITA",
	"GDPR_DEFAULT_VALUE":                           "0",
	"USER_SYNC_EXTERNAL_URL":                       "http://fallback.example.com",
	"PBS_USER_SYNC_COOP_SYNC_DEFAULT":              "true",
	"PBS_ADAPTERS_APPNEXUS_DISABLED":               "true",
	"PBS_ADAPTERS_33ACROSS_ENDPOINT":               "http://33across.example.com",
	"PBS_ADAPTERS_RUBICON_XAPI_PASSWORD":           "secret",
	"PBS_ADAPTERS_IX_OPENRTB_GPP-SUPPORTED":        "true",
	"PBS_ADAPTERS_IX_OPENRTB_GPP_SUPPORTED":        "false",
	"PBS_STATUS_RESPONSE":                          "",
	"PBS_HOST_COOKIE_MAX_COOKIE_SIZE_BYTES":        "abc",
	"PBS_CACHE_EXPECTED_MILLIS":                    "12.7",
	"PBS_EXTERNAL_URL":                             "https://env.example.com",
	"PBS_METRICS_PROMETHEUS_PORT":                  "0x10",
	"PBS_ACCOUNT_DEFAULTS_DISABLED":                "1",
	"PBS_GARBAGE_UNKNOWN":                          "1",
	"PBS_CAMELCASE_KEY":                            "from env",
	"PBS_HOST_SCHAIN_NODE":                         "flat",
}

// prebidObservations reads every key the defaults name, the settings of a few bidders, and the
// keys only the configuration file has.
func prebidObservations(d *configDefaults) []*Op {
	var keys []string
	for _, k := range d.Keys {
		keys = append(keys, k.Key)
	}
	for _, b := range []string{"appnexus", "rubicon", "33across", "ix", "newbidder"} {
		for _, s := range d.BidderKeys.Suffixes {
			keys = append(keys, "adapters."+b+"."+s.Suffix)
		}
	}
	keys = append(keys, "adapters", "adapters.appnexus", "adapters.rubicon", "camelcase",
		"camelcase.key", "host_schain_node.asi", "gdpr", "analytics.agma.accounts.0.code",
		"user_sync.priority_groups.0")
	var ops []*Op
	for _, k := range keys {
		ops = append(ops, get(k), isSet(k))
	}
	for _, k := range []string{
		"port", "host_cookie.ttl_days", "request_validation.ipv4_private_networks",
		"gdpr.eea_countries", "gdpr.default_value", "external_url", "status_response",
		"adapters.appnexus.disabled", "adapters.ix.openrtb.gpp-supported", "max_request_size",
		"cache.expected_millis", "metrics.prometheus.port", "account_defaults.disabled",
		"user_sync.priority_groups", "host_schain_node", "gdpr.tcf2.special_feature1.vendor_exceptions",
	} {
		for _, g := range getters[1:18] {
			ops = append(ops, obs(g, k))
		}
	}
	return append(ops, allKeys(), allSettings(),
		inConfig("gdpr.default_value"), inConfig("adapters.rubicon.disabled"),
		unmarshalKey("adapters.appnexus.disabled", "bool_ptr"),
		unmarshalKey("adapters.rubicon.disabled", "bool_ptr"),
		unmarshalKey("adapters.ix.disabled", "bool_ptr"),
		unmarshalKey("adapters.appnexus", "map"), unmarshalKey("gdpr", "map"),
		unmarshalKey("request_validation.ipv4_private_networks", "strings"),
		unmarshalKey("port", "int"), unmarshal("map"),
		sub("adapters"), sub("gdpr.tcf2"),
	)
}

// prebidScenarios run config.SetupViper's operations as main.loadConfig and main_test.go do.
func prebidScenarios(defaultsPath string) []*Scenario {
	d := loadDefaults(defaultsPath)
	if d.ConfigFileName != "pbs" || d.EnvPrefix != "PBS" {
		log.Fatalf("unexpected config file name %q or prefix %q", d.ConfigFileName, d.EnvPrefix)
	}
	bidders := d.BidderKeys.Bidders
	// A few bidders, for the scenarios that do not need every bidder's keys.
	few := []string{"appnexus", "rubicon", "33across", "ix"}
	scenario := func(name, filename string, bidders []string, files map[string]string, env map[string]string, extra ...*Op) *Scenario {
		ops := setupOps(d, filename, bidders)
		ops = append(ops, extra...)
		ops = append(ops, prebidObservations(d)...)
		return &Scenario{
			Name: name, Files: files, Env: env, Ops: ops,
			verify: verifySetup(name, filename, bidders),
		}
	}
	return []*Scenario{
		// main_test.go's TestViperInit: SetupViper(v, "", nil).
		scenario("prebid/TestViperInit", "", nil, nil, nil,
			get("external_url").is(`<string>"http://localhost:8000"`),
			get("accounts.filesystem.directorypath").is(`<string>"./stored_requests/data/by_id"`),
		),
		// main_test.go's TestViperEnv.
		scenario("prebid/TestViperEnv", "", nil, nil, map[string]string{
			"PBS_PORT": "7777",
			"PBS_EXPERIMENT_ADSCERT_INPROCESS_ORIGIN":      "not_an_endpoint",
			"PBS_HOST_COOKIE_TTL_DAYS":                     "60",
			"PBS_REQUEST_VALIDATION_IPV4_PRIVATE_NETWORKS": "1.1.1.1/24 2.2.2.2/24",
		},
			get("port").is("<int>7777"),
			get("experiment.adscert.inprocess.origin").is(`<string>"not_an_endpoint"`),
			get("host_cookie.ttl_days").is("<int>60"),
			get("request_validation.ipv4_private_networks").is(`<[]string>["1.1.1.1/24", "2.2.2.2/24"]`),
		),
		// main.loadConfig with every bidder, no file and no variables.
		scenario("prebid/defaults", "pbs", bidders, nil, nil),
		// main.loadConfig with a pbs.yaml in the working directory and PBS_* variables.
		scenario("prebid/pbs.yaml+env", "pbs", bidders, map[string]string{"pbs.yaml": pbsYAML}, pbsEnv),
		// A pbs.json is found before a pbs.yaml.
		scenario("prebid/pbs.json", "pbs", few, map[string]string{
			"pbs.json": `{"port": 9001, "Adapters": {"AppNexus": {"Disabled": true}}, "gdpr": {"default_value": 1}}`,
			"pbs.yaml": pbsYAML,
		}, nil),
		// SetupViper ignores ReadInConfig's error: a malformed file leaves the defaults.
		scenario("prebid/malformed", "pbs", few, map[string]string{"pbs.yaml": "port: [\n"}, pbsEnv),
	}
}
