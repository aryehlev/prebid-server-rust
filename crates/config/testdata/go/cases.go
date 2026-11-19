package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"gopkg.in/yaml.v3"
)

// upstreamDir is the Go server's tree, fetched into .upstream at the repository root.
var upstreamDir = filepath.Join("..", "..", "..", "..", ".upstream")

// yamlFile is a load whose working directory holds pbs.yaml with contents.
func yamlFile(name, contents string) loadInput {
	return loadInput{Name: name, Files: map[string]string{"pbs.yaml": contents}}
}

func withEnv(in loadInput, env map[string]string) loadInput {
	in.Env = env
	return in
}

func withProbes(in loadInput, probes ...string) loadInput {
	in.Probes = probes
	return in
}

func withBidders(in loadInput, bidders ...string) loadInput {
	in.Bidders = bidders
	return in
}

// loadCases are every load of loads.json whose result does not depend on Go's map order.
func loadCases() []loadInput {
	cases := []loadInput{
		{Name: "defaults with other bidders", NoFile: true, Bidders: []string{"appnexus", "33across", "adkernelAdn"},
			Probes: []string{"adapters.adkerneladn.endpoint"}},
		{Name: "defaults without bidders", NoFile: true, Bidders: []string{}},
		{Name: "no file in the search paths", Probes: []string{"port"}},
	}
	cases = append(cases, fileCases()...)
	cases = append(cases, searchCases()...)
	cases = append(cases, keyCases()...)
	cases = append(cases, envCases()...)
	cases = append(cases, formatCases()...)
	cases = append(cases, yamlCases()...)
	cases = append(cases, decodeCases()...)
	return cases
}

// unorderedCases are loads whose result depends on Go's map order: Viper's AllSettings sets a
// key and a key under it in map order, so which one the tree keeps varies from run to run.
func unorderedCases() []loadInput {
	return []loadInput{
		// AllSettings gets port (cast to int, 0 from a map) and port.a (1).
		withProbes(yamlFile("section over a default value", "port:\n  a: 1\n"), "port", "port.a"),
		// AllSettings gets unix_socket_enable (cast to bool, false from a map) and
		// unix_socket_enable.a.
		withProbes(yamlFile("bool default from a map", "unix_socket_enable: {a: 1}\n"), "unix_socket_enable"),
		// gdpr is null in the file, so Get("gdpr") returns the defaults' gdpr map, and the
		// environment's gdpr.default_value is written into it or replaced by it.
		withProbes(withEnv(yamlFile("null section with env", "gdpr: ~\n"),
			map[string]string{"PBS_GDPR_ENABLED": "false", "PBS_GDPR_DEFAULT_VALUE": "1"}),
			"gdpr.enabled", "gdpr.default_value"),
		// AccountModulesHookFunc converts the vendors in map order and stops at the first whose
		// module does not marshal, so which one the error names varies.
		yamlFile("account modules failing in map order",
			"account_defaults:\n  hooks:\n    modules:\n      a:\n        m:\n          x: .nan\n      b:\n        m:\n          x: .inf\n"),
	}
}

// fileCases are the configurations Prebid ships and tests with: the samples in the upstream
// tree, the parity harness profiles (testdata/configs) and every configuration literal of
// config/config_test.go.
func fileCases() []loadInput {
	var cases []loadInput
	for _, sample := range []struct{ path, file string }{
		{"sample/001_banner/app.yaml", "pbs.yaml"},
		{"modules/fiftyonedegrees/devicedetection/sample/pbs.json", "pbs.json"},
	} {
		data, err := os.ReadFile(filepath.Join(upstreamDir, sample.path))
		must(err)
		cases = append(cases, loadInput{Name: sample.path, Files: map[string]string{sample.file: string(data)}})
	}
	profiles, err := filepath.Glob(filepath.Join("..", "configs", "*"))
	must(err)
	for _, path := range profiles {
		data, err := os.ReadFile(path)
		must(err)
		name := filepath.Base(path)
		cases = append(cases, loadInput{
			Name:  "testdata/configs/" + name,
			Files: map[string]string{"pbs" + filepath.Ext(name): string(data)},
		})
	}
	for _, lit := range configTestLiterals() {
		cases = append(cases, yamlFile(lit.name, lit.text))
	}
	for i := range cases {
		cases[i].Probes = []string{"gdpr.default_value"}
	}
	return cases
}

// literal is one configuration literal of config/config_test.go.
type literal struct {
	name string
	text string
}

// configTestLiterals are the []byte(`...`) literals of config/config_test.go that are YAML
// mappings, named by their line, plus the four configurations
// TestSpecialFeature1VendorExceptionMap builds by appending to its base.
func configTestLiterals() []literal {
	path := filepath.Join(upstreamDir, "config", "config_test.go")
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, path, nil, 0)
	must(err)
	var out []literal
	var base string
	ast.Inspect(file, func(n ast.Node) bool {
		call, ok := n.(*ast.CallExpr)
		if !ok || len(call.Args) != 1 {
			return true
		}
		arr, ok := call.Fun.(*ast.ArrayType)
		if !ok || arr.Len != nil {
			return true
		}
		if elt, ok := arr.Elt.(*ast.Ident); !ok || elt.Name != "byte" {
			return true
		}
		lit, ok := call.Args[0].(*ast.BasicLit)
		if !ok || lit.Kind != token.STRING {
			return true
		}
		text, err := strconv.Unquote(lit.Value)
		must(err)
		line := fset.Position(lit.Pos()).Line
		if strings.HasSuffix(text, "vendor_exceptions: ") {
			base = text
			return true
		}
		var m map[string]any
		if yaml.Unmarshal([]byte(text), &m) != nil || len(m) == 0 {
			return true
		}
		out = append(out, literal{name: fmt.Sprintf("config/config_test.go:%d", line), text: text})
		return true
	})
	if base == "" {
		panic("config_test.go: TestSpecialFeature1VendorExceptionMap's base is gone")
	}
	for _, suffix := range []string{`null`, `[]`, `["vendor1"]`, `["vendor1","vendor2"]`} {
		out = append(out, literal{name: "config/config_test.go:TestSpecialFeature1VendorExceptionMap " + suffix, text: base + suffix})
	}
	return out
}

// searchCases are Viper's file search: the extension order within one directory, the
// directories in order, a directory named like a file, the formats Rust does not read, and
// files that do not parse (whose error SetupViper ignores, so the defaults stay).
func searchCases() []loadInput {
	cases := []loadInput{
		{Name: "json before yaml", Files: map[string]string{
			"pbs.json": `{"port": 1001}`, "pbs.yaml": "port: 1002\n"}},
		{Name: "toml before yaml", Files: map[string]string{
			"pbs.toml": "port = 1003\n", "pbs.yaml": "port: 1004\n"}},
		{Name: "json before toml", Files: map[string]string{
			"pbs.json": `{"port": 1031}`, "pbs.toml": "port = 1032\n"}},
		{Name: "yaml before yml", Files: map[string]string{
			"pbs.yaml": "port: 1005\n", "pbs.yml": "port: 1006\n"}},
		{Name: "yml alone", Files: map[string]string{"pbs.yml": "port: 1007\n"}},
		{Name: "yml before properties", Files: map[string]string{
			"pbs.yml": "port: 1008\n", "pbs.properties": "port=1009\n"}},
		{Name: "a directory named pbs.json", Files: map[string]string{
			"pbs.json/": "", "pbs.yaml": "port: 1010\n"}},
		{Name: "the working directory first", Files: map[string]string{"pbs.yaml": "port: 1011\n"},
			Second: map[string]string{"pbs.json": `{"port": 1012}`}},
		{Name: "the second directory", Files: map[string]string{},
			Second: map[string]string{"pbs.yaml": "port: 1013\n"}},
		{Name: "a file in both directories", Files: map[string]string{"pbs.json": `{"port": 1033}`},
			Second: map[string]string{"pbs.json": `{"port": 1034, "host": "h"}`}},
		{Name: "an unread format in the working directory first", Files: map[string]string{"pbs.ini": "port=1014\n"},
			Second: map[string]string{"pbs.json": `{"port": 1015}`}},
		{Name: "invalid yaml is ignored", Files: map[string]string{"pbs.yaml": "port: [1017\n"}},
		{Name: "invalid json is ignored", Files: map[string]string{"pbs.json": `{"port": 1018`}},
		{Name: "invalid toml is ignored", Files: map[string]string{"pbs.toml": "port = = 1019\n"}},
		{Name: "duplicate yaml keys are ignored", Files: map[string]string{"pbs.yaml": "port: 1020\nport: 1021\n"}},
		{Name: "duplicate json keys", Files: map[string]string{"pbs.json": `{"port": 1022, "port": 1023}`}},
		{Name: "duplicate toml keys are ignored", Files: map[string]string{"pbs.toml": "port = 1024\nport = 1025\n"}},
		{Name: "a yaml list document is ignored", Files: map[string]string{"pbs.yaml": "- port\n"}},
		{Name: "a json list document is ignored", Files: map[string]string{"pbs.json": `[1]`}},
		{Name: "an empty yaml file", Files: map[string]string{"pbs.yaml": ""}},
		{Name: "an empty json file is ignored", Files: map[string]string{"pbs.json": ""}},
		{Name: "an empty toml file", Files: map[string]string{"pbs.toml": ""}},
		{Name: "a yaml null document", Files: map[string]string{"pbs.yaml": "~\n"}},
		{Name: "a json null document", Files: map[string]string{"pbs.json": "null"}},
		{Name: "a yaml comment only", Files: map[string]string{"pbs.yaml": "# nothing\n"}},
		{Name: "a yaml tab indent is ignored", Files: map[string]string{"pbs.yaml": "gdpr:\n\tdefault_value: \"1\"\n"}},
		{Name: "invalid utf-8 in a json string", Files: map[string]string{"pbs.json": "{\"host\": \"a\xffb\"}"}},
		{Name: "a lone surrogate in a json string", Files: map[string]string{"pbs.json": `{"port": 1041, "host": "a\ud800b"}`}},
		{Name: "json nested 200 levels", Files: map[string]string{
			"pbs.json": `{"port": 1042, "deep": ` + strings.Repeat(`{"a": `, 199) + "1" + strings.Repeat("}", 199) + "}",
		}},
		{Name: "module json spelling", Files: map[string]string{"pbs.json": `{"account_defaults": {"hooks": {"modules": {"vendor": {"module": {"html": "<a&b>", "big": 1e21, "wide": 1e20, "negzero": -0.0, "small": 1e-7, "whole": 3.0, "huge": 12345678901234567890}}}}}}`}},
		{Name: "invalid utf-8 in yaml is ignored", Files: map[string]string{"pbs.yaml": "host: a\xffb\n"}},
	}
	for _, ext := range []string{"properties", "props", "prop", "hcl", "tfvars", "dotenv", "env", "ini"} {
		contents := "port=1030\n"
		switch ext {
		case "hcl", "tfvars":
			contents = "port = 1030\n"
		case "ini":
			contents = "[default]\nport=1030\n"
		}
		cases = append(cases, loadInput{Name: "format " + ext, Files: map[string]string{"pbs." + ext: contents}})
	}
	for i := range cases {
		cases[i].Probes = append(cases[i].Probes, "port", "host")
	}
	return cases
}

// keyCases are key spellings in pbs.yaml: case, dots, nulls, a value where a section belongs,
// and empty sections.
func keyCases() []loadInput {
	return []loadInput{
		withProbes(yamlFile("key case Port", "Port: 9300\n"), "port"),
		withProbes(yamlFile("key case PORT", "PORT: 9301\n"), "port"),
		withProbes(yamlFile("key case nested", "GDPR:\n  Default_Value: \"1\"\n  TCF2:\n    Enabled: false\n"),
			"gdpr.default_value", "gdpr.tcf2.enabled"),
		withProbes(yamlFile("key case inside a list stays", "analytics:\n  agma:\n    accounts:\n      - Code: c1\n        PUBLISHER_ID: p1\n"),
			"analytics.agma.accounts"),
		withProbes(yamlFile("key case inside modules", "hooks:\n  modules:\n    Vendor:\n      Module:\n        CamelKey: 1\n        list:\n          - InnerKey: 2\n"),
			"hooks.modules"),
		withProbes(yamlFile("key case of a bidder", "adapters:\n  AppNexus:\n    Endpoint: https://a.example\n"),
			"adapters.appnexus.endpoint"),
		withProbes(yamlFile("dotted top-level key", "gdpr.default_value: \"1\"\nhttp_client.max_idle_connections: 7\n"),
			"gdpr.default_value", "http_client.max_idle_connections"),
		withProbes(yamlFile("dotted nested key", "gdpr:\n  tcf2.enabled: false\n  timeouts_ms.init_vendorlist_fetches: 9\n"),
			"gdpr.tcf2.enabled", "gdpr.timeouts_ms.init_vendorlist_fetches"),
		withProbes(yamlFile("dotted key beside the nested one", "gdpr:\n  tcf2.enabled: false\n  tcf2:\n    enabled: true\n"),
			"gdpr.tcf2.enabled"),
		withProbes(yamlFile("dotted keys in modules", "hooks:\n  modules:\n    vendor.module:\n      a.b: 1\n"),
			"hooks.modules", "hooks.modules.vendor.module.a.b"),
		withProbes(yamlFile("null value", "port: ~\nhost: null\n"), "port", "host"),
		withProbes(yamlFile("null bound key", "gdpr:\n  default_value: ~\n"), "gdpr.default_value"),
		withProbes(yamlFile("null section", "gdpr: ~\n"), "gdpr", "gdpr.enabled"),
		withProbes(yamlFile("value where a section belongs", "gdpr: 5\n"), "gdpr", "gdpr.enabled"),
		withProbes(yamlFile("list where a section belongs", "gdpr: [a]\n"), "gdpr"),
		withProbes(yamlFile("empty sections", "hooks:\n  modules: {}\nadapters:\n  appnexus: {}\naccount_defaults:\n  hooks:\n    modules: {}\n"),
			"hooks.modules", "adapters.appnexus", "account_defaults.hooks.modules"),
		withProbes(yamlFile("empty list", "blocked_apps: []\ncookie_sync:\n  disabled_iframe_bidders: []\n"),
			"blocked_apps", "cookie_sync.disabled_iframe_bidders"),
		withProbes(yamlFile("null inside a list", "blocked_apps: [a, ~]\ngdpr:\n  eea_countries: [~]\n"),
			"blocked_apps", "gdpr.eea_countries"),
		withProbes(yamlFile("unknown keys", "no_such_key: 1\ngdpr:\n  no_such_key: 2\n"),
			"no_such_key", "gdpr.no_such_key"),
	}
}

// envCases are PBS_ variables: precedence over the file, empty values, the un-prefixed
// fallbacks of the BindEnv calls made before SetEnvPrefix, bidder settings, keys only the file
// names, and a variable that names a section.
func envCases() []loadInput {
	return []loadInput{
		withProbes(withEnv(yamlFile("env over file", "port: 2001\n"), map[string]string{"PBS_PORT": "2002"}), "port"),
		withProbes(withEnv(yamlFile("empty env is ignored", "port: 2003\n"), map[string]string{"PBS_PORT": ""}), "port"),
		withProbes(withEnv(loadInput{Name: "env over default"}, map[string]string{"PBS_PORT": "2004"}), "port"),
		withProbes(withEnv(loadInput{Name: "fallback variable"}, map[string]string{"GDPR_DEFAULT_VALUE": "1"}),
			"gdpr.default_value"),
		withProbes(withEnv(loadInput{Name: "prefixed variable over the fallback"},
			map[string]string{"GDPR_DEFAULT_VALUE": "1", "PBS_GDPR_DEFAULT_VALUE": "0"}), "gdpr.default_value"),
		withProbes(withEnv(loadInput{Name: "empty prefixed variable before the fallback"},
			map[string]string{"GDPR_DEFAULT_VALUE": "1", "PBS_GDPR_DEFAULT_VALUE": ""}), "gdpr.default_value"),
		withProbes(withEnv(yamlFile("fallback over file", "gdpr:\n  default_value: \"0\"\n"),
			map[string]string{"GDPR_DEFAULT_VALUE": "1"}), "gdpr.default_value"),
		withProbes(withEnv(loadInput{Name: "every fallback"}, map[string]string{
			"ACCOUNT_DEFAULTS_PRIVACY_DSA_DEFAULT":   `{"dsarequired":1}`,
			"ACCOUNT_DEFAULTS_PRIVACY_DSA_GDPR_ONLY": "true",
			"USER_SYNC_COOP_SYNC_DEFAULT":            "true",
			"USER_SYNC_EXTERNAL_URL":                 "https://sync.example",
		}), "account_defaults.privacy.dsa.default", "account_defaults.privacy.dsa.gdpr_only",
			"user_sync.coop_sync.default", "user_sync.external_url"),
		withProbes(withEnv(loadInput{Name: "fallback spelled with dots is not read"},
			map[string]string{"GDPR.DEFAULT_VALUE": "1"}), "gdpr.default_value"),
		withProbes(withEnv(loadInput{Name: "lower-case variable is not read"},
			map[string]string{"pbs_port": "2005"}), "port"),
		withProbes(withEnv(loadInput{Name: "mixed-case variable is not read"},
			map[string]string{"PBS_Port": "2006"}), "port"),
		withProbes(withEnv(withBidders(loadInput{Name: "bidder variables"}, "appnexus", "33across", "adkernelAdn"),
			map[string]string{
				"PBS_ADAPTERS_APPNEXUS_ENDPOINT":                "https://ib.example",
				"PBS_ADAPTERS_APPNEXUS_DISABLED":                "true",
				"PBS_ADAPTERS_33ACROSS_MODIFYINGVASTXMLALLOWED": "false",
				"PBS_ADAPTERS_33ACROSS_GVLVENDORID":             "58",
				"PBS_ADAPTERS_APPNEXUS_USERSYNC_REDIRECT_URL":   "https://sync.example",
				"PBS_ADAPTERS_APPNEXUS_OPENRTB_GPP-SUPPORTED":   "true",
				"PBS_ADAPTERS_APPNEXUS_XAPI_USERNAME":           "u",
				"PBS_ADAPTERS_ADKERNELADN_ENDPOINT":             "https://ak.example",
			}), "adapters.appnexus.endpoint", "adapters.appnexus.disabled", "adapters.adkerneladn.endpoint"),
		withProbes(withEnv(loadInput{Name: "bidder variable without the bidder"},
			map[string]string{"PBS_ADAPTERS_OPENX_ENDPOINT": "https://ox.example"}), "adapters.openx.endpoint"),
		withProbes(withEnv(withBidders(yamlFile("bidder variable over the file", "adapters:\n  appnexus:\n    endpoint: https://file.example\n    platform_id: p\n"), "appnexus"),
			map[string]string{"PBS_ADAPTERS_APPNEXUS_ENDPOINT": "https://env.example"}), "adapters.appnexus.endpoint"),
		withProbes(withEnv(withBidders(loadInput{Name: "bidder bool from env"}, "appnexus", "rubicon"),
			map[string]string{"PBS_ADAPTERS_APPNEXUS_DISABLED": "", "PBS_ADAPTERS_RUBICON_DISABLED": "t"}),
			"adapters.appnexus.disabled", "adapters.rubicon.disabled"),
		withProbes(withEnv(withBidders(loadInput{Name: "bidder bool from env fails"}, "appnexus"),
			map[string]string{"PBS_ADAPTERS_APPNEXUS_DISABLED": "yes"}), "adapters.appnexus.disabled"),
		withProbes(withEnv(yamlFile("env for a key the file names", "account_defaults:\n  cookie_sync:\n    default_limit: 5\n"),
			map[string]string{"PBS_ACCOUNT_DEFAULTS_COOKIE_SYNC_DEFAULT_LIMIT": "7"}),
			"account_defaults.cookie_sync.default_limit"),
		withProbes(withEnv(loadInput{Name: "env for a key nothing names"},
			map[string]string{"PBS_ACCOUNT_DEFAULTS_COOKIE_SYNC_DEFAULT_LIMIT": "7"}),
			"account_defaults.cookie_sync.default_limit"),
		withProbes(withEnv(yamlFile("env names a section", "gdpr:\n  default_value: \"1\"\n"),
			map[string]string{"PBS_GDPR": "x"}), "gdpr", "gdpr.enabled", "gdpr.default_value"),
		withProbes(withEnv(loadInput{Name: "env names a section and a key in it"},
			map[string]string{"PBS_GDPR": "x", "PBS_GDPR_ENABLED": "false"}), "gdpr.enabled", "gdpr.host_vendor_id"),
		withProbes(withEnv(loadInput{Name: "env names an inner section"},
			map[string]string{"PBS_GDPR_TCF2": "x"}), "gdpr.tcf2.enabled", "gdpr.enabled"),
		withProbes(withEnv(loadInput{Name: "empty env for a section is ignored"},
			map[string]string{"PBS_GDPR": ""}), "gdpr.enabled"),
		withProbes(withEnv(loadInput{Name: "string slice from env"},
			map[string]string{"PBS_GDPR_EEA_COUNTRIES": " FRA  DEU\tITA\n", "PBS_BLOCKED_APPS": "a,b"}),
			"gdpr.eea_countries", "blocked_apps"),
		withProbes(withEnv(loadInput{Name: "nested slice from env"},
			map[string]string{"PBS_USER_SYNC_PRIORITY_GROUPS": "a b"}), "user_sync.priority_groups"),
		withProbes(withEnv(loadInput{Name: "struct slice from env"},
			map[string]string{"PBS_ANALYTICS_AGMA_ACCOUNTS": "x"}), "analytics.agma.accounts"),
		withProbes(withEnv(loadInput{Name: "struct from env"},
			map[string]string{"PBS_HOST_SCHAIN_NODE": "x"}), "host_schain_node"),
		withProbes(withEnv(loadInput{Name: "bidder name slice from env"},
			map[string]string{"PBS_GDPR_TCF2_SPECIAL_FEATURE1_VENDOR_EXCEPTIONS": "a b"}),
			"gdpr.tcf2.special_feature1.vendor_exceptions"),
		withProbes(withEnv(yamlFile("env over a file section", "host_schain_node:\n  asi: a.com\n"),
			map[string]string{"PBS_HOST_SCHAIN_NODE": "x"}), "host_schain_node"),
		withProbes(withEnv(loadInput{Name: "numbers from env"}, map[string]string{
			"PBS_PORT":                "0x1F",
			"PBS_ADMIN_PORT":          "1_000",
			"PBS_GDPR_HOST_VENDOR_ID": "12.0",
			"PBS_DEBUG_TIMEOUT_NOTIFICATION_SAMPLING_RATE": "1e-1",
			"PBS_ACCOUNT_DEFAULTS_DEFAULT_BID_LIMIT":       "-1",
		}), "port", "admin_port", "gdpr.host_vendor_id", "debug.timeout_notification.sampling_rate"),
		withProbes(withEnv(loadInput{Name: "bad numbers from env become zero"}, map[string]string{
			"PBS_PORT":       " 8001",
			"PBS_ADMIN_PORT": "8001.5",
			"PBS_DEBUG_TIMEOUT_NOTIFICATION_SAMPLING_RATE": "half",
		}), "port", "admin_port", "debug.timeout_notification.sampling_rate"),
		withProbes(withEnv(loadInput{Name: "bools from env"}, map[string]string{
			"PBS_UNIX_SOCKET_ENABLE":                     "T",
			"PBS_GDPR_TCF2_ENABLED":                      "yes",
			"PBS_ACCOUNT_DEFAULTS_DISABLED":              "1",
			"PBS_ACCOUNT_DEFAULTS_PRIVACY_DSA_GDPR_ONLY": "F",
		}), "unix_socket_enable", "gdpr.tcf2.enabled", "account_defaults.disabled"),
		withProbes(withEnv(loadInput{Name: "bool without a default from env fails"}, map[string]string{
			"PBS_ACCOUNT_DEFAULTS_PRIVACY_DSA_GDPR_ONLY": "on",
		}), "account_defaults.privacy.dsa.gdpr_only"),
	}
}

// formatCases read settings from JSON and TOML: numbers into bool, int and string keys, the
// list forms, and TOML's own types.
func formatCases() []loadInput {
	return []loadInput{
		withProbes(loadInput{Name: "json numbers", Files: map[string]string{"pbs.json": `{
  "port": 3001.9,
  "unix_socket_enable": 1,
  "status_response": 12,
  "gdpr": {"default_value": 0, "host_vendor_id": 7.0, "tcf2": {"enabled": 0}},
  "account_defaults": {"cookie_sync": {"default_limit": 3.7}, "price_floors": {"enforce_floors_rate": 1e2}},
  "debug": {"timeout_notification": {"sampling_rate": 1}},
  "host_schain_node": {"hp": 1, "asi": 5}
}`}}, "port", "unix_socket_enable", "status_response", "gdpr.default_value", "gdpr.tcf2.enabled"),
		withProbes(loadInput{Name: "json lists", Files: map[string]string{"pbs.json": `{
  "blocked_apps": ["a", 1, true, null],
  "gdpr": {"eea_countries": "FRA DEU", "non_standard_publishers": [1.5]},
  "cookie_sync": {"disabled_iframe_bidders": "one"},
  "user_sync": {"priority_groups": [["a", "b"], ["c"]]}
}`}}, "blocked_apps", "gdpr.eea_countries"),
		withProbes(loadInput{Name: "json keys", Files: map[string]string{"pbs.json": `{
  "Port": 3002, "GDPR": {"Default_Value": "1"}, "gdpr.host_vendor_id": 8
}`}}, "port", "gdpr.default_value", "gdpr.host_vendor_id"),
		withProbes(loadInput{Name: "json big numbers", Files: map[string]string{"pbs.json": `{
  "port": 1e400, "admin_port": 9223372036854775808, "gdpr": {"host_vendor_id": -1e19},
  "hooks": {"modules": {"vendor": {"module": {"big": 12345678901234567890, "small": 1e-320}}}}
}`}}, "port", "admin_port"),
		withProbes(loadInput{Name: "json modules", Files: map[string]string{"pbs.json": `{
  "hooks": {"enabled": true, "modules": {"vendor": {"module": {"enabled": true, "n": 5, "f": 1.5, "list": [1, "a", null], "obj": {"K": 1}}}}},
  "account_defaults": {"hooks": {"modules": {"vendor": {"module": {"n": 5, "s": "x"}}}}}
}`}}, "hooks.modules"),
		withProbes(loadInput{Name: "toml values", Files: map[string]string{"pbs.toml": `port = 3003
unix_socket_enable = 1
status_response = 12
blocked_apps = ["a", "b"]

[gdpr]
default_value = "1"
host_vendor_id = 9

[gdpr.tcf2]
enabled = false

[account_defaults.cookie_sync]
default_limit = 4

[debug.timeout_notification]
sampling_rate = 0.5

[hooks.modules.vendor.module]
n = 5
f = 1.5
on = true
`}}, "port", "unix_socket_enable", "status_response", "gdpr.default_value"),
		withProbes(loadInput{Name: "toml keys", Files: map[string]string{"pbs.toml": "Port = 3004\n\"gdpr.default_value\" = \"1\"\n"}},
			"port", "gdpr.default_value"),
		withProbes(loadInput{Name: "toml date-time", Files: map[string]string{"pbs.toml": "[gdpr]\ndefault_value = 1979-05-27T07:32:00Z\n"}},
			"gdpr.default_value"),
		withProbes(loadInput{Name: "toml local date in modules", Files: map[string]string{"pbs.toml": "[hooks.modules.vendor.module]\nday = 1979-05-27\n"}},
			"hooks.modules"),
		withProbes(loadInput{Name: "toml integer into a bool", Files: map[string]string{"pbs.toml": "[account_defaults.privacy.dsa]\ngdpr_only = 1\n"}},
			"account_defaults.privacy.dsa.gdpr_only"),
	}
}

// yamlCases are YAML features in pbs.yaml: anchors and merge keys, several documents, tags,
// CRLF line ends, a byte order mark, quoted keys, flow collections and YAML 1.1 forms.
func yamlCases() []loadInput {
	return []loadInput{
		withProbes(yamlFile("yaml anchors and merge keys", "base: &base\n  max_idle_connections: 5\nhttp_client:\n  <<: *base\n  max_idle_connections_per_host: 6\ncache:\n  scheme: &s https\n  host: *s\n"),
			"http_client.max_idle_connections", "cache.host"),
		withProbes(yamlFile("yaml several documents", "port: 4001\n---\nport: 4002\n"), "port"),
		withProbes(yamlFile("yaml document markers", "%YAML 1.2\n---\nport: 4003\n...\n"), "port"),
		withProbes(yamlFile("yaml tags", "port: !!str 4004\nhost: !!int 5\nstatus_response: !!float 1\nunix_socket_enable: !!bool true\n"),
			"port", "host", "status_response", "unix_socket_enable"),
		withProbes(yamlFile("yaml crlf", "port: 4005\r\nhost: a\r\n"), "port", "host"),
		withProbes(yamlFile("yaml byte order mark", "\ufeffport: 4006\n"), "port"),
		withProbes(yamlFile("yaml quoted keys", "\"Port\": 4007\n'gdpr': {'default_value': '1'}\n"), "port", "gdpr.default_value"),
		withProbes(yamlFile("yaml flow collections", "gdpr: {default_value: \"1\", enabled: false, eea_countries: [FRA, DEU]}\n"),
			"gdpr.enabled", "gdpr.eea_countries"),
		withProbes(yamlFile("yaml 1.1 bools stay strings", "unix_socket_enable: yes\ngdpr:\n  enabled: on\n  default_value: y\nhost: off\n"),
			"unix_socket_enable", "gdpr.enabled", "gdpr.default_value", "host"),
		withProbes(yamlFile("yaml numbers", "port: 0x1F\nadmin_port: 0o17\ngdpr:\n  host_vendor_id: 017\n  default_value: 1_0\nhost: 1e3\nstatus_response: .5\nunix_socket_name: +12\n"),
			"port", "admin_port", "gdpr.host_vendor_id", "gdpr.default_value", "host", "status_response", "unix_socket_name"),
		withProbes(yamlFile("yaml special floats", "debug:\n  timeout_notification:\n    sampling_rate: .nan\nhost: .inf\nstatus_response: -.Inf\n"),
			"debug.timeout_notification.sampling_rate", "host", "status_response"),
		withProbes(yamlFile("yaml timestamps", "host: 2001-12-14\nstatus_response: 2001-12-14t21:59:43.10-05:00\n"),
			"host", "status_response"),
		withProbes(yamlFile("yaml block scalars", "host: |\n  a\n  b\nstatus_response: >-\n  c\n  d\n"), "host", "status_response"),
		withProbes(yamlFile("yaml non-string keys in modules", "hooks:\n  modules:\n    vendor:\n      module:\n        1: a\n        true: b\n        0x1F: c\n"),
			"hooks.modules"),
		withProbes(yamlFile("yaml indented document", "  port: 4008\n  host: h\n"), "port", "host"),
	}
}

// decodeCases are mapstructure's conversions into Configuration's field types: a list where a
// map belongs (the adapters form of the 51Degrees sample), the account modules hook, interface
// values, and values that fail the decode.
func decodeCases() []loadInput {
	return []loadInput{
		yamlFile("adapters as a list", "adapters:\n  - appnexus:\n      endpoint: https://a.example\n  - rubicon:\n      disabled: true\n"),
		yamlFile("adapters as an empty list", "adapters: []\n"),
		yamlFile("account modules", "account_defaults:\n  hooks:\n    modules:\n      vendor:\n        module:\n          enabled: true\n          list: [1, \"a\"]\n          map:\n            k: v\n        other: 5\n      empty: ~\n"),
		yamlFile("account modules inner not a map", "account_defaults:\n  hooks:\n    modules:\n      vendor: 5\n"),
		yamlFile("account modules not a map", "account_defaults:\n  hooks:\n    modules: 5\n"),
		yamlFile("account modules as a list", "account_defaults:\n  hooks:\n    modules:\n      - vendor:\n          m:\n            a: 1\n"),
		yamlFile("host modules", "hooks:\n  enabled: true\n  modules:\n    vendor:\n      module:\n        enabled: true\n        n: 1.5\n        list: [1, a]\n"),
		yamlFile("host execution plan", "hooks:\n  host_execution_plan:\n    endpoints:\n      /openrtb2/auction:\n        stages:\n          entrypoint:\n            groups:\n              - timeout: 5\n                hook_sequence:\n                  - module_code: vendor.module\n                    hook_impl_code: code\n"),
		yamlFile("struct from a scalar", "host_schain_node: x\n"),
		yamlFile("struct fields", "host_schain_node:\n  asi: a.com\n  sid: \"1\"\n  hp: 300\n  rid: 7\n"),
		yamlFile("struct from an empty map", "host_schain_node: {}\n"),
		yamlFile("slice from a scalar", "cookie_sync:\n  disabled_iframe_bidders: one\n"),
		yamlFile("slice from a map", "cookie_sync:\n  disabled_iframe_bidders:\n    a: 1\n"),
		yamlFile("slice from an empty map", "cookie_sync:\n  disabled_iframe_bidders: {}\n"),
		yamlFile("map from a scalar", "adapters: x\n"),
		yamlFile("nested slices", "user_sync:\n  priority_groups:\n    - [a, b]\n    - c\n"),
		yamlFile("string from a list", "status_response: [a]\nhost_cookie:\n  family: [a]\n"),
		yamlFile("string from a bool and a float", "status_response: true\nhost_cookie:\n  family: 1.5\n"),
		yamlFile("int from a list", "port: [1]\naccount_defaults:\n  cookie_sync:\n    default_limit: [1]\n"),
		yamlFile("int from a bool and a float", "account_defaults:\n  cookie_sync:\n    default_limit: true\n    max_limit: 2.9\n"),
		yamlFile("bool without a default from a map", "account_defaults:\n  privacy:\n    dsa:\n      gdpr_only: {a: 1}\n"),
		yamlFile("several failures", "account_defaults:\n  cookie_sync:\n    default_limit: x\n    max_limit: y\n  disabled: z\nhost_schain_node: w\n"),
		// The values of the errors: floats as %v prints them and a string as strconv.Quote quotes it.
		yamlFile("failing values are printed", "account_defaults:\n  cookie_sync:\n    default_limit: [1000000.0, 0.00001]\n    max_limit: \"\u00a0x\"\n"),
		yamlFile("uint overflow wraps", "account_defaults:\n  default_bid_limit: -1\ntmax_adjustments:\n  bidder_network_latency_buffer_ms: -1\n"),
		yamlFile("bid adjustments", "account_defaults:\n  bidadjustments:\n    mediatype:\n      '*':\n        '*':\n          '*':\n            - adjtype: multiplier\n              value: 1.01\n              currency: USD\n"),
		yamlFile("bidder overrides", "adapters:\n  appnexus:\n    endpoint: https://a.example\n    disabled: true\n    modifyingVastXmlAllowed: false\n    gvlVendorID: 32\n    usersync:\n      key: adnxs\n      redirect:\n        url: https://s.example\n        user_macro: $UID\n    openrtb:\n      version: 2.6\n      gpp-supported: true\n    experiment:\n      adsCert:\n        enabled: true\n"),
		yamlFile("bidder bool spellings", "adapters:\n  appnexus:\n    disabled: \"\"\n  rubicon:\n    disabled: \"t\"\n    modifyingvastxmlallowed: 0\n"),
		yamlFile("bidder bool fails", "adapters:\n  appnexus:\n    disabled: \"yes\"\n"),
		yamlFile("bidder uint16 overflow", "adapters:\n  appnexus:\n    gvlVendorID: 70000\n  rubicon:\n    gvlVendorID: \"70000\"\n"),
		yamlFile("bidder not in the bidder infos", "adapters:\n  openx:\n    endpoint: https://ox.example\n    disabled: true\n"),
		yamlFile("unexported-looking fields", "gdpr:\n  eeacountriesmap:\n    FRA: {}\n"),
	}
}
