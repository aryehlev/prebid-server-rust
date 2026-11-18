package main

import (
	"fmt"
	"math"
	"reflect"
	"sort"
	"strconv"
	"time"
)

// ---- operations ------------------------------------------------------------------------------

func setDefault(key string, val interface{}) *Op {
	return &Op{Op: "set_default", Key: key, Value: spec(reflect.ValueOf(&val).Elem()), val: val}
}

func set(key string, val interface{}) *Op {
	return &Op{Op: "set", Key: key, Value: spec(reflect.ValueOf(&val).Elem()), val: val}
}

func bindEnv(args ...string) *Op { return &Op{Op: "bind_env", Args: args} }

func envPrefix(prefix string) *Op { return &Op{Op: "set_env_prefix", Arg: prefix} }

func replacer(pairs ...string) *Op { return &Op{Op: "set_env_key_replacer", Pairs: pairs} }

func autoEnv() *Op { return &Op{Op: "automatic_env"} }

func allowEmptyEnv(allow bool) *Op {
	return &Op{Op: "allow_empty_env", Arg: strconv.FormatBool(allow)}
}

func typeByDefault(enable bool) *Op {
	return &Op{Op: "set_type_by_default_value", Arg: strconv.FormatBool(enable)}
}

func configName(name string) *Op { return &Op{Op: "set_config_name", Arg: name} }

func configType(typ string) *Op { return &Op{Op: "set_config_type", Arg: typ} }

func configFile(file string) *Op { return &Op{Op: "set_config_file", Arg: file} }

func addConfigPath(path string) *Op { return &Op{Op: "add_config_path", Arg: path} }

func readInConfig() *Op { return &Op{Op: "read_in_config"} }

func readConfig(data string) *Op { return &Op{Op: "read_config", Arg: data} }

func configFileUsed() *Op { return &Op{Op: "config_file_used"} }

func obs(op, key string) *Op { return &Op{Op: op, Key: key} }

func get(key string) *Op { return obs("get", key) }

func isSet(key string) *Op { return obs("is_set", key) }

func inConfig(key string) *Op { return obs("in_config", key) }

func sub(key string) *Op { return obs("sub", key) }

func allKeys() *Op { return &Op{Op: "all_keys"} }

func allSettings() *Op { return &Op{Op: "all_settings"} }

func unmarshal(target string) *Op { return &Op{Op: "unmarshal", Target: target} }

func unmarshalExact(target string) *Op { return &Op{Op: "unmarshal_exact", Target: target} }

func unmarshalKey(key, target string) *Op {
	return &Op{Op: "unmarshal_key", Key: key, Target: target}
}

// is records the outcome a ported Go test asserts; the generator fails if Go disagrees.
func (o *Op) is(want interface{}) *Op {
	o.expect = want
	return o
}

// getters are the observations made of each key by observe.
var getters = []string{
	"get", "get_string", "get_bool", "get_int", "get_int32", "get_int64", "get_uint",
	"get_uint32", "get_uint64", "get_float64", "get_time", "get_duration", "get_int_slice",
	"get_string_slice", "get_string_map", "get_string_map_string", "get_string_map_string_slice",
	"get_size_in_bytes", "is_set", "in_config",
}

// observe makes every observation of each key, then of all keys and settings.
func observe(keys ...string) []*Op {
	var ops []*Op
	for _, k := range keys {
		for _, g := range getters {
			ops = append(ops, obs(g, k))
		}
	}
	return append(ops, allKeys(), allSettings())
}

// ops flattens *Op and []*Op arguments.
func ops(items ...interface{}) []*Op {
	var out []*Op
	for _, item := range items {
		switch o := item.(type) {
		case *Op:
			out = append(out, o)
		case []*Op:
			out = append(out, o...)
		default:
			panic(fmt.Sprintf("ops: %T", item))
		}
	}
	return out
}

// ---- viper's test fixtures -------------------------------------------------------------------

// yamlExample is viper_yaml3_test.go's.
const yamlExample = `Hacker: true
name: steve
hobbies:
    - skateboarding
    - snowboarding
    - go
clothing:
    jacket: leather
    trousers: denim
    pants:
        size: large
age: 35
eyes : brown
beard: true
`

const yamlExampleWithExtras = `Existing: true
Bogus: true
`

const jsonExample = `{
"id": "0001",
"type": "donut",
"name": "Cake",
"ppu": 0.55,
"batters": {
        "batter": [
                { "type": "Regular" },
                { "type": "Chocolate" },
                { "type": "Blueberry" },
                { "type": "Devil's Food" }
            ]
    }
}`

const yamlDeepNestedSlices = `TV:
- title: "The expanse"
  seasons:
  - first_released: "December 14, 2015"
    episodes:
    - title: "Dulcinea"
      air_date: "December 14, 2015"
    - title: "The Big Empty"
      air_date: "December 15, 2015"
    - title: "Remember the Cant"
      air_date: "December 22, 2015"
  - first_released: "February 1, 2017"
    episodes:
    - title: "Safe"
      air_date: "February 1, 2017"
    - title: "Doors & Corners"
      air_date: "February 1, 2017"
    - title: "Static"
      air_date: "February 8, 2017"
  episodes:
    - ["Dulcinea", "The Big Empty", "Remember the Cant"]
    - ["Safe", "Doors & Corners", "Static"]
`

var yamlKeys = []string{
	"hacker", "name", "hobbies", "clothing", "clothing.jacket", "clothing.pants",
	"clothing.pants.size", "age", "eyes", "beard", "Clothing.Jacket", "missing", "clothing.hat",
}

var jsonKeys = []string{"id", "type", "name", "ppu", "batters", "batters.batter", "f"}

// handScenarios ports viper v1.12.0's tests (viper_test.go, overrides_test.go) and adds cases
// for the behaviour Prebid relies on.
func handScenarios() []*Scenario {
	s := []*Scenario{
		{Name: "TestDefault", Ops: ops(
			setDefault("age", 45), get("age").is("<int>45"),
			setDefault("clothing.jacket", "slacks"), get("clothing.jacket").is(`<string>"slacks"`),
			configType("yaml"), readConfig(yamlExample).is(""),
			get("clothing.jacket").is(`<string>"leather"`),
			observe(yamlKeys...),
		)},
		{Name: "TestUnmarshaling", Ops: ops(
			configType("yaml"), readConfig(yamlExample),
			inConfig("name").is(true), inConfig("clothing.jacket").is(true),
			inConfig("state").is(false), inConfig("clothing.hat").is(false),
			get("name").is(`<string>"steve"`),
			get("hobbies").is(`<[]interface {}>[<string>"skateboarding", <string>"snowboarding", <string>"go"]`),
			get("age").is("<int>35"),
			observe(yamlKeys...),
			sub("clothing"), sub("clothing.pants"), sub("clothing.pants.size").is("nil"),
			sub("missing.key").is("nil"), sub("hobbies").is("nil"),
		)},
		{Name: "TestUnmarshalExact", Ops: ops(
			configType("yaml"), readConfig(yamlExampleWithExtras),
			unmarshalExact("unused"), unmarshal("unused"),
		)},
		{Name: "TestOverrides", Ops: ops(
			configType("yaml"), readConfig(yamlExample),
			set("age", 40), get("age").is("<int>40"),
			get("state"), setDefault("state", "NYC"), get("state").is(`<string>"NYC"`),
			observe("age", "state"),
		)},
		{Name: "TestJSON", Ops: ops(
			configType("json"), readConfig(jsonExample).is(""),
			get("id").is(`<string>"0001"`), observe(jsonKeys...),
			obs("get_string", "batters.batter.0.type"), obs("get", "batters.batter.3"),
			sub("batters"), unmarshalKey("batters.batter", "map"),
		)},
		{Name: "TestEnv", Env: map[string]string{
			"ID": "13", "FOOD": "apple", "OLD_FOOD": "banana", "NAME": "crunk",
		}, Ops: ops(
			configType("json"), readConfig(jsonExample),
			bindEnv("id"), bindEnv("f", "FOOD", "OLD_FOOD"),
			get("id").is(`<string>"13"`), get("f").is(`<string>"apple"`),
			get("name").is(`<string>"Cake"`), allKeys(), allSettings(),
			autoEnv(), get("name").is(`<string>"crunk"`), observe(jsonKeys...),
		)},
		{Name: "TestMultipleEnv", Env: map[string]string{"OLD_FOOD": "banana"}, Ops: ops(
			configType("json"), readConfig(jsonExample),
			bindEnv("f", "FOOD", "OLD_FOOD"), get("f").is(`<string>"banana"`),
		)},
		{Name: "TestEmptyEnv", Env: map[string]string{"TYPE": ""}, Ops: ops(
			configType("json"), readConfig(jsonExample),
			bindEnv("type"), bindEnv("name"),
			get("type").is(`<string>"donut"`), get("name").is(`<string>"Cake"`),
			observe("type", "name"),
		)},
		{Name: "TestEmptyEnv_Allowed", Env: map[string]string{"TYPE": ""}, Ops: ops(
			configType("json"), readConfig(jsonExample), allowEmptyEnv(true),
			bindEnv("type"), bindEnv("name"),
			get("type").is(`<string>""`), get("name").is(`<string>"Cake"`),
			observe("type", "name"),
		)},
		{Name: "TestEnvPrefix", Env: map[string]string{
			"FOO_ID": "13", "FOOD": "apple", "FOO_NAME": "crunk",
		}, Ops: ops(
			configType("json"), readConfig(jsonExample),
			envPrefix("foo"), bindEnv("id"), bindEnv("f", "FOOD"),
			get("id").is(`<string>"13"`), get("f").is(`<string>"apple"`),
			get("name").is(`<string>"Cake"`),
			autoEnv(), get("name").is(`<string>"crunk"`), observe(jsonKeys...),
		)},
		{Name: "TestAutoEnv", Env: map[string]string{"FOO_BAR": "13"}, Ops: ops(
			autoEnv(), get("foo_bar").is(`<string>"13"`), observe("foo_bar", "foo.bar"),
		)},
		{Name: "TestAutoEnvWithPrefix", Env: map[string]string{"BAZ_BAR": "13"}, Ops: ops(
			autoEnv(), envPrefix("Baz"), get("bar").is(`<string>"13"`), observe("bar"),
		)},
		{Name: "TestSetEnvKeyReplacer", Env: map[string]string{"REFRESH_INTERVAL": "30s"}, Ops: ops(
			autoEnv(), replacer("-", "_"),
			get("refresh-interval").is(`<string>"30s"`), observe("refresh-interval"),
			replacer(), get("refresh-interval").is("nil"),
		)},
		{Name: "TestAllKeysWithEnv", Env: map[string]string{"ID": "13", "FOO_BAR": "baz"}, Ops: ops(
			bindEnv("id"), bindEnv("foo.bar"), replacer(".", "_"),
			allKeys().is([]string{"foo.bar", "id"}), allSettings(), observe("foo", "foo.bar"),
		)},
		{Name: "TestUnmarshal", Ops: ops(
			setDefault("port", 1313), set("name", "Steve"), set("duration", "1s1ms"),
			set("modes", []int{1, 2, 3}),
			unmarshal("viper_config").is("{Port:1313 Name:\"Steve\" Duration:1001000000 Modes:[1, 2, 3]} | "),
			set("port", 1234), unmarshal("viper_config"), allSettings(),
		)},
		{Name: "TestBoundCaseSensitivity", Env: map[string]string{"TURTLE_EYES": "blue"}, Ops: ops(
			configType("yaml"), readConfig(yamlExample),
			bindEnv("eYEs", "TURTLE_EYES"), get("eyes").is(`<string>"blue"`), allKeys(),
		)},
		{Name: "TestSizeInBytes", Ops: sizeInBytes()},
		{Name: "TestFindsNestedKeys", Files: map[string]string{"config.yaml": yamlExample}, Ops: ops(
			addConfigPath("."), readInConfig().is(""),
			set("super", map[string]interface{}{
				"deep": map[string]interface{}{"nested": "value"},
			}),
			observe("super", "super.deep", "super.deep.nested", "clothing", "clothing.jacket",
				"clothing.pants.size", "hobbies", "eyes", "age", "hacker", "beard"),
			configFileUsed().is("@TMP@/config.yaml"),
		)},
		{Name: "TestIsSet", Env: map[string]string{"FOO": "bar", "CLOTHING_HAT": "bowler"}, Ops: ops(
			configType("yaml"), readConfig(yamlExample),
			setDefault("clothing.shoes", "sneakers"),
			isSet("clothing").is(true), isSet("clothing.jacket").is(true),
			isSet("clothing.jackets").is(false), isSet("clothing.shoes").is(true),
			isSet("helloworld").is(false), set("helloworld", "fubar"), isSet("helloworld").is(true),
			replacer(".", "_"), bindEnv("eyes"), bindEnv("foo"), bindEnv("clothing.hat"),
			bindEnv("clothing.hats"),
			isSet("eyes").is(true), isSet("foo").is(true), isSet("clothing.hat").is(true),
			isSet("clothing.hats").is(false), allKeys(), allSettings(),
		)},
		{Name: "TestDirsSearch", Files: map[string]string{
			"a a/improbable.yaml": "key: value is a a\n",
			"b/improbable.yaml":   "key: value is b\n",
			"C_/improbable.yaml":  "key: value is C_\n",
			`d\d/improbable.yaml`: "key: value is d\\d\n",
		}, Ops: ops(
			// Go's test adds the directories in ioutil.ReadDir order (sorted by name).
			configName("improbable"), setDefault("key", "default"),
			addConfigPath("C_"), addConfigPath("a a"), addConfigPath("b"), addConfigPath(`d\d`),
			readInConfig().is(""), obs("get_string", "key").is(`"value is C_"`),
			configFileUsed().is("@TMP@/C_/improbable.yaml"),
		)},
		{Name: "TestWrongDirsSearchNotFound", Ops: ops(
			configName("improbable"), setDefault("key", "default"),
			addConfigPath("whattayoutalkingbout"), addConfigPath("thispathaintthere"),
			readInConfig().is(`Config File "improbable" Not Found in "[@TMP@/whattayoutalkingbout @TMP@/thispathaintthere]"`),
			obs("get_string", "key").is(`"default"`), configFileUsed().is(""),
		)},
		{Name: "TestSetConfigNameClearsFileCache", Ops: ops(
			configFile("/tmp/config.yaml"), configFileUsed().is("/tmp/config.yaml"),
			configName("default"), configFileUsed().is(""),
			readInConfig().is(`Config File "default" Not Found in "[]"`),
		)},
		{Name: "TestShadowedNestedValue", Ops: ops(
			configType("yaml"),
			readConfig("name: steve\nclothing:\n  jacket: leather\n  trousers: denim\n  pants:\n    size: large\n"),
			obs("get_string", "name").is(`"steve"`),
			setDefault("clothing.shirt", "polyester"), setDefault("clothing.jacket.price", 100),
			obs("get_string", "clothing.jacket").is(`"leather"`),
			get("clothing.jacket.price").is("nil"),
			obs("get_string", "clothing.shirt").is(`"polyester"`),
			observe("clothing", "clothing.jacket.price", "clothing.shirt"),
		)},
		{Name: "TestDotParameter", Ops: ops(
			configType("json"),
			readConfig(`{"batters": {"batter": [{"type": "Regular"}]}, "batters.batter": [ { "type": "Small" } ] }`),
			get("batters.batter").is(`<[]interface {}>[<map[string]interface {}>{"type": <string>"Small"}]`),
			observe("batters", "batters.batter", "batters.batter.0.type"),
		)},
		{Name: "TestCaseInsensitiveSet", Ops: ops(
			set("Given1", map[string]interface{}{
				"Foo": 32, "Bar": map[interface{}]interface{}{"ABc": "A", "cDE": "B"},
			}),
			set("Number1", 42),
			setDefault("Given2", map[string]interface{}{
				"Foo": 52, "Bar": map[interface{}]interface{}{"bCd": "A", "eFG": "B"},
			}),
			setDefault("Number2", 52),
			get("number2").is("<int>52"), get("given2.foo").is("<int>52"),
			get("given2.bar.bcd").is(`<string>"A"`), get("number1").is("<int>42"),
			get("given1.foo").is("<int>32"), get("given1.bar.abc").is(`<string>"A"`),
			observe("given1", "given1.bar", "given2.bar.efg"),
		)},
		{Name: "TestSliceIndexAccess", Ops: ops(
			configType("yaml"), readConfig(yamlDeepNestedSlices),
			obs("get_string", "tv.0.title").is(`"The expanse"`),
			obs("get_string", "tv.0.seasons.1.first_released").is(`"February 1, 2017"`),
			obs("get_string", "tv.0.seasons.1.episodes.2.title").is(`"Static"`),
			obs("get_string", "tv.0.seasons.0.episodes.1.air_date").is(`"December 15, 2015"`),
			obs("get_string", "tv.0.seasons.2.first_released").is(`""`),
			obs("get_string", "tv.0.episodes.1.2").is(`"Static"`),
			observe("tv", "tv.0", "tv.00.title", "tv.+0.title", "tv.0.seasons", "tv.1", "TV.0.Title"),
		)},
	}
	for _, typ := range []string{"yaml", "json"} {
		content := "\naBcD: 1\neF:\n  gH: 2\n  iJk: 3\n  Lm:\n    nO: 4\n    P:\n      Q: 5\n      R: 6\n"
		if typ == "json" {
			content = `{"aBcD": 1, "eF": {"iJk": 3, "Lm": {"P": {"Q": 5, "R": 6}, "nO": 4}, "gH": 2}}`
		}
		s = append(s, &Scenario{Name: "TestCaseInsensitive/" + typ, Ops: ops(
			configType(typ), readConfig(content), set("RfD", true),
			get("rfd").is("<bool>true"), get("rFD").is("<bool>true"),
			obs("get_int", "abcd").is("1"), obs("get_int", "Abcd").is("1"),
			obs("get_int", "ef.gh").is("2"), obs("get_int", "ef.ijk").is("3"),
			obs("get_int", "ef.lm.no").is("4"), obs("get_int", "ef.lm.p.q").is("5"),
			observe("abcd", "ef", "ef.lm.p", "ef.lm.p.r"),
		)})
	}
	s = append(s, nestedOverrides()...)
	s = append(s, configFileScenarios()...)
	s = append(s, envScenarios()...)
	s = append(s, valueScenarios()...)
	return s
}

// sizeInBytes ports TestSizeInBytes through GetSizeInBytes.
func sizeInBytes() []*Op {
	table := []struct {
		in   string
		want uint
	}{
		{"", 0}, {"b", 0}, {"12 bytes", 0}, {"200000000000gb", 0}, {"12 b", 12},
		{"43 MB", 43 * (1 << 20)}, {"10mb", 10 * (1 << 20)}, {"1gb", 1 << 30},
	}
	var out []*Op
	for i, c := range table {
		key := "size" + strconv.Itoa(i)
		out = append(out, set(key, c.in), obs("get_size_in_bytes", key).is(strconv.FormatUint(uint64(c.want), 10)))
	}
	for i, in := range []string{
		" 7 Kb ", "5b", "-3kb", "1.0kb", "0x10kb", "3 xb", "9GB", "1tb", "kb", " b", "2 Bb",
		"18446744073709551615kb", "9223372036854775807", "4 kB", "6 gB",
	} {
		key := "extra" + strconv.Itoa(i)
		out = append(out, set(key, in), obs("get_size_in_bytes", key))
	}
	return append(out,
		set("int_size", 42), obs("get_size_in_bytes", "int_size"),
		set("float_size", 1.5), obs("get_size_in_bytes", "float_size"),
		obs("get_size_in_bytes", "missing"),
	)
}

// nestedOverrides ports overrides_test.go's TestNestedOverrides: a first value on the default
// or override layer, then a Set.
func nestedOverrides() []*Scenario {
	cases := []struct {
		firstPath   string
		firstValue  interface{}
		secondPath  string
		secondValue interface{}
	}{
		{"tom", 10, "tom", 20},
		{"tom.age", 10, "tom.age", 20},
		{"sawyer.tom.age", 10, "sawyer.tom.age", 20},
		{"tom.age", 10, "tom", "boy"},
		{"tom", "boy", "tom.age", 10},
		{"tom.size", 4, "tom.age", 10},
		{"tom.size", 4, "tom", map[string]interface{}{"age": 10}},
		{"tom", []int{10, 20}, "tom", 30},
		{"tom.age", []int{10, 20}, "tom.age", 30},
		{"tom", []int{10, 20}, "tom", []int{30, 40}},
		{"tom.age", []int{10, 20}, "tom.age", []int{30, 40}},
	}
	var out []*Scenario
	for i, c := range cases {
		for _, layer := range []string{"default", "override"} {
			first := setDefault(c.firstPath, c.firstValue)
			if layer == "override" {
				first = set(c.firstPath, c.firstValue)
			}
			out = append(out, &Scenario{
				Name: fmt.Sprintf("TestNestedOverrides/%d/%s", i, layer),
				Ops: ops(
					first, get(c.firstPath), set(c.secondPath, c.secondValue),
					observe("tom", "tom.age", "tom.size", "sawyer", "sawyer.tom.age", c.firstPath, c.secondPath),
				),
			})
		}
	}
	return out
}

// configFileScenarios cover the search for and decoding of configuration files.
func configFileScenarios() []*Scenario {
	return []*Scenario{
		{Name: "search/extension_order", Files: map[string]string{
			"conf/app.yaml": "from: yaml\n",
			"conf/app.json": `{"from": "json"}`,
			"conf/app.yml":  "from: yml\n",
		}, Ops: ops(
			configName("app"), addConfigPath("@TMP@/conf"), readInConfig().is(""),
			get("from").is(`<string>"json"`), configFileUsed().is("@TMP@/conf/app.json"),
		)},
		{Name: "search/yaml_before_yml", Files: map[string]string{
			"conf/app.yml":  "from: yml\n",
			"conf/app.yaml": "from: yaml\n",
		}, Ops: ops(
			configName("app"), addConfigPath("@TMP@/conf"), readInConfig(), get("from"),
			configFileUsed(),
		)},
		{Name: "search/path_order", Files: map[string]string{
			"second/app.json": `{"from": "second"}`,
			"third/app.yaml":  "from: third\n",
		}, Dirs: []string{"first"}, Ops: ops(
			configName("app"), addConfigPath("first"), addConfigPath("@TMP@/second"),
			addConfigPath("./third"), addConfigPath("first/"), readInConfig(), get("from"),
			configFileUsed(),
		)},
		{Name: "search/directory_named_like_the_file", Files: map[string]string{
			"conf/app.yaml": "from: yaml\n",
		}, Dirs: []string{"conf/app.json"}, Ops: ops(
			configName("app"), addConfigPath("@TMP@/conf"), readInConfig(), get("from"),
			configFileUsed(),
		)},
		{Name: "search/bare_name_with_type", Files: map[string]string{
			"conf/app": "from: bare\n",
		}, Ops: ops(
			configName("app"), configType("yaml"), addConfigPath("@TMP@/conf"), readInConfig(),
			get("from"), configFileUsed(),
		)},
		{Name: "search/bare_name_without_type", Files: map[string]string{
			"conf/app": "from: bare\n",
		}, Ops: ops(
			configName("app"), addConfigPath("@TMP@/conf"), readInConfig(), get("from"),
			configFileUsed(),
		)},
		{Name: "search/no_paths", Ops: ops(readInConfig(), configFileUsed())},
		{Name: "search/cwd_relative", Cwd: "work", Files: map[string]string{
			"work/pbs.yaml": "port: 9000\n",
		}, Ops: ops(
			configName("pbs"), addConfigPath("."), addConfigPath("/nonexistent/config"),
			readInConfig(), get("port"), configFileUsed(),
		)},
		{Name: "search/paths_expanded", Env: map[string]string{
			"HOME": "@TMP@/home", "CONF": "@TMP@/c", "EMPTY": "",
		}, Files: map[string]string{"home/cfg/app.yaml": "from: home\n"}, Ops: ops(
			configName("app"), addConfigPath("$HOME/x"), addConfigPath("${CONF}/x/../y/"),
			addConfigPath("$HOME"), addConfigPath("$HOMEX/z"), addConfigPath("$EMPTY/abs"),
			addConfigPath("rel//dir/./"), addConfigPath("$"), addConfigPath("${}"),
			addConfigPath("$HOME/x"), readInConfig(),
			addConfigPath("$HOME/cfg"), readInConfig(), get("from"), configFileUsed(),
		)},
		{Name: "search/pwd_symlink", Cwd: "real", Env: map[string]string{"PWD": "@TMP@/link"},
			Files: map[string]string{"real/app.yaml": "from: real\n"},
			Links: map[string]string{"link": "real"}, Ops: ops(
				configName("app"), addConfigPath("."), readInConfig(), configFileUsed(),
				get("from"),
			)},
		{Name: "search/pwd_elsewhere", Cwd: "real", Env: map[string]string{"PWD": "@TMP@/other"},
			Files: map[string]string{"real/app.yaml": "from: real\n"},
			Dirs:  []string{"other"}, Ops: ops(
				configName("app"), addConfigPath("."), readInConfig(), configFileUsed(),
			)},
		{Name: "file/missing", Ops: ops(
			configFile("@TMP@/nope.yaml"), readInConfig(), configFileUsed(),
		)},
		{Name: "file/directory", Dirs: []string{"dir.yaml"}, Ops: ops(
			configFile("@TMP@/dir.yaml"), readInConfig(),
		)},
		{Name: "file/unsupported", Files: map[string]string{"conf.txt": "a: 1\n", "conf.YAML": "a: 1\n"}, Ops: ops(
			configFile("@TMP@/conf.txt"), readInConfig(), configFile("@TMP@/conf.YAML"),
			readInConfig(), configType("YAML"), readInConfig(), get("a"),
		)},
		{Name: "file/no_extension", Files: map[string]string{"conf": "a: 1\n"}, Ops: ops(
			configFile("@TMP@/conf"), readInConfig(), configType("yml"), readInConfig(), get("a"),
		)},
		{Name: "file/relative", Cwd: "sub", Files: map[string]string{"sub/conf/app.yml": "A: 1\n"}, Ops: ops(
			configFile("conf/app.yml"), readInConfig(), get("a"), configFileUsed(),
		)},
		{Name: "file/type_overrides_extension", Files: map[string]string{"app.json": "a: [1, 2]\n"}, Ops: ops(
			configFile("@TMP@/app.json"), configType("yaml"), readInConfig(), get("a"),
		)},
		{Name: "file/replaces_config", Files: map[string]string{
			"one.yaml": "a: 1\nb: 2\n", "two.json": `{"b": 3}`, "bad.yaml": "a: [\n",
		}, Ops: ops(
			configFile("@TMP@/one.yaml"), readInConfig(), allSettings(),
			configFile("@TMP@/bad.yaml"), readInConfig(), allSettings(),
			configFile("@TMP@/two.json"), readInConfig(), allSettings(),
		)},
		{Name: "parse/yaml_errors", Ops: ops(
			configType("yaml"),
			readConfig("a: [\n"), allSettings(),
			readConfig("just a string\n"), allSettings(),
			readConfig("- a\n- b\n"), allSettings(),
			readConfig("A: 1\nA: 2\nB: 3\n"), allSettings(), get("b"), inConfig("B"), allKeys(),
			readConfig("a: *missing\n"), allSettings(),
			readConfig("\t\n"), allSettings(),
			readConfig("null\n"), allSettings(),
			readConfig(""), allSettings(),
			readConfig("1: one\ntrue: yes\n2.5: x\nnull: y\n"), allSettings(), observe("1", "true", "2.5", ""),
		)},
		{Name: "parse/json_errors", Ops: ops(
			configType("json"),
			readConfig(`{"a": `), allSettings(),
			readConfig(`[1, 2]`), allSettings(),
			readConfig(`"str"`), allSettings(),
			readConfig(`null`), allSettings(),
			readConfig(``), allSettings(),
			readConfig(`{"A": 1e400, "B": {"C": 2}}`), allSettings(), allKeys(),
			readConfig(`{"a": 1, "a": 2, "A": 3}`), allSettings(),
			readConfig(`{"a": 1} trailing`), allSettings(),
			readConfig(`{"a": "\ud800"}`), get("a"),
		)},
		{Name: "parse/yaml_values", Ops: ops(
			configType("yml"),
			readConfig(`
Int: 7
Big: 18446744073709551615
Neg: -9223372036854775808
Float: 1.5e3
Inf: .inf
Bool: yes
OldBool: on
Str: "quoted"
Null: ~
Time: 2001-12-14t21:59:43.10-05:00
Date: 2002-12-14
Seq: [1, two, 3.0, {In: Seq}]
Map: {Mixed: Case, lower: case, 1: int key}
Anchor: &a {x: 1}
Alias: *a
Merge:
  <<: *a
  y: 2
Dotted.Key: dotted
dotted: {key: nested}
Binary: !!binary aGVsbG8=
`),
			observe("int", "big", "neg", "float", "inf", "bool", "oldbool", "str", "null", "time",
				"date", "seq", "seq.3.in", "map", "map.1", "map.mixed", "anchor.x", "alias.x",
				"merge", "dotted.key", "binary"),
			sub("map"), sub("merge"), unmarshal("map"),
		)},
		{Name: "parse/json_values", Ops: ops(
			configType("json"),
			readConfig(`{"Int": 7, "Big": 18446744073709551615, "Float": -0.0, "Str": "\u00e9\ud83d\ude00",
"Null": null, "Arr": [1, "a", null, true, {"In": "Arr"}], "Obj": {"Mixed": {"Deep": 1}},
"Dotted.Key": "dotted", "dotted": {"key": "nested"}, "Empty": {}, "EmptyArr": []}`),
			observe("int", "big", "float", "str", "null", "arr", "arr.4.in", "arr.4.In", "obj",
				"obj.mixed.deep", "dotted.key", "empty", "emptyarr"),
			unmarshal("map"),
		)},
		{Name: "read_config/no_type", Ops: ops(
			readConfig("a: 1\n"), allSettings(), get("a"),
		)},
		{Name: "read_config/type_from_found_file", Files: map[string]string{"conf/app.json": `{"x": 1}`}, Ops: ops(
			configName("app"), addConfigPath("@TMP@/conf"), readConfig(`{"from": "buffer"}`),
			configFileUsed(), allSettings(), readInConfig(), allSettings(),
		)},
	}
}

// envScenarios cover environment lookups as Prebid configures them.
func envScenarios() []*Scenario {
	return []*Scenario{
		{Name: "env/bind_before_prefix", Env: map[string]string{
			"GDPR_DEFAULT_VALUE": "1", "USER_SYNC_EXTERNAL_URL": "http://u", "PBS_USER_SYNC_EXTERNAL_URL": "",
		}, Ops: ops(
			bindEnv("gdpr.default_value"), bindEnv("user_sync.external_url"),
			replacer(".", "_"), typeByDefault(true), envPrefix("PBS"), autoEnv(),
			bindEnv("adapters.x.openrtb.gpp-supported"), bindEnv("Adapters.X.Disabled"),
			observe("gdpr.default_value", "user_sync.external_url", "gdpr", "adapters.x.disabled"),
			unmarshalKey("adapters.x.disabled", "bool_ptr"),
		)},
		{Name: "env/automatic_before_bound", Env: map[string]string{
			"GDPR_DEFAULT_VALUE": "1", "PBS_GDPR_DEFAULT_VALUE": "0",
			"PBS_ADAPTERS_X_OPENRTB_GPP-SUPPORTED": "true", "PBS_ADAPTERS_X_DISABLED": "TRUE",
		}, Ops: ops(
			bindEnv("gdpr.default_value"), replacer(".", "_"), envPrefix("PBS"), autoEnv(),
			bindEnv("adapters.x.openrtb.gpp-supported"), bindEnv("Adapters.X.Disabled"),
			observe("gdpr.default_value", "adapters.x.openrtb.gpp-supported", "adapters.x.disabled", "adapters.x"),
			unmarshalKey("adapters.x.disabled", "bool_ptr"), unmarshalKey("adapters.x", "map"),
		)},
		{Name: "env/without_replacer", Env: map[string]string{
			"APP_A.B": "dotted", "APP_A_B": "underscored", "A.B": "bound",
		}, Ops: ops(
			envPrefix("app"), autoEnv(), get("a.b"), bindEnv("x.y", "A.B"), get("x.y"),
			replacer(".", "_"), get("a.b"), get("x.y"),
		)},
		{Name: "env/prefix_rules", Env: map[string]string{
			"KEY": "plain", "P_KEY": "prefixed", "P__KEY": "double", "Q_KEY": "q",
		}, Ops: ops(
			envPrefix(""), autoEnv(), get("key"), envPrefix("p"), get("key"), envPrefix("p_"),
			get("key"), envPrefix("Q"), get("KEY"), bindEnv("k2"), bindEnv("key3"),
			envPrefix("p"), bindEnv("key4"), allKeys(),
		)},
		{Name: "env/shadowing", Env: map[string]string{"INNER": "scalar", "OUTER_A": "env"}, Ops: ops(
			setDefault("inner.name", "default"), setDefault("outer.a.b", 1),
			setDefault("outer.c", 2), autoEnv(),
			observe("inner", "inner.name", "outer", "outer.a", "outer.a.b", "outer.c"),
			unmarshal("outer"),
		)},
		{Name: "env/empty_values", Env: map[string]string{"A": "", "B": "x", "C": ""}, Ops: ops(
			setDefault("a", "da"), bindEnv("b", "A", "B"), bindEnv("c", "C"), autoEnv(),
			observe("a", "b", "c"), allowEmptyEnv(true), observe("a", "b", "c"),
		)},
		{Name: "env/bind_errors", Ops: ops(
			bindEnv().is("missing key to bind to"), bindEnv("x", "X1"), bindEnv("x", "X2", "X3"),
			bindEnv("x"), allKeys(), get("x"),
		)},
		// (Binding both a key and a key under it makes AllKeys depend on Go's map order.)
		{Name: "env/bound_keys_shadow", Env: map[string]string{"A": "1", "X_Y": "2"}, Ops: ops(
			bindEnv("a"), bindEnv("x.y"), replacer(".", "_"), setDefault("a.c", 3),
			setDefault("x", 1), setDefault("x.z", 4),
			observe("a", "a.c", "x", "x.y", "x.z"),
		)},
		{Name: "env/replacer_order", Env: map[string]string{"X__Y": "double", "X_Y": "single"}, Ops: ops(
			autoEnv(), replacer(".", "_", "-", "__"), get("x.y"), get("x-y"), get("x.-y"),
		)},
	}
}

// valueScenarios cover SetDefault, Set and the conversions of the typed getters.
func valueScenarios() []*Scenario {
	when := time.Date(2015, 3, 4, 5, 6, 7, 800, time.UTC)
	values := map[string]interface{}{
		"bool": true, "int": -7, "int8": int8(-8), "int16": int16(16), "int32": int32(-32),
		"int64": int64(1) << 40, "uint": uint(3000000000), "uint8": uint8(200),
		"uint16": uint16(65535), "uint32": uint32(4294967295), "uint64": uint64(1) << 63,
		"float32": float32(1.25), "float64": -2.5e9, "string": "08", "time": when,
		"duration": 90 * time.Second, "strings": []string{"a", "b c"}, "ints": []int{1, -2},
		"ifaces": []interface{}{1, "2", 3.5, true, nil}, "bytes": []byte("hi"),
		"smap": map[string]string{"K": "v"}, "imap": map[string]interface{}{"K": []string{"x"}},
		"gmap":  map[interface{}]interface{}{"K": "v", 2: []interface{}{"a"}},
		"ssmap": map[string][]string{"k": {"a", "b"}}, "nil_strings": []string(nil),
		"nil_map": map[string]interface{}(nil), "nil_smap": map[string]string(nil),
		"empty": "", "nested": [][]string{{"a"}, {}},
	}
	keys := make([]string, 0, len(values))
	setOps := make([]*Op, 0, len(values))
	defaultOps := make([]*Op, 0, len(values))
	for _, k := range sortedKeys(values) {
		keys = append(keys, k)
		setOps = append(setOps, set(k, values[k]))
		defaultOps = append(defaultOps, setDefault(k, values[k]))
	}
	strs := map[string]string{
		"bool": "t", "int": "0x1f", "int8": "-1", "int16": "1e3", "int32": "4294967296",
		"int64": "-12", "uint": "12", "uint8": "7", "uint16": "8", "uint32": "-1",
		"uint64": "9223372036854775807", "float32": "1.5", "float64": "NaN", "string": "plain",
		"time": "2006-01-02", "duration": "100", "strings": "x  y z", "ints": "1 2",
		"ifaces": "i", "bytes": "b", "smap": `{"a":"b"}`, "imap": `{"a":[1]}`, "gmap": "g",
		"ssmap": `{"k":["v"]}`, "nil_strings": "n", "nil_map": "null", "nil_smap": `{"a":1}`,
		"empty": "", "nested": "a b",
	}
	env := map[string]string{}
	for k, val := range strs {
		env["T_"+k] = val
	}
	return []*Scenario{
		{Name: "values/set", Ops: ops(setOps, observe(keys...), unmarshal("map"))},
		{Name: "values/default", Ops: ops(defaultOps, typeByDefault(true), observe(keys...))},
		{Name: "values/default_typed_env", Env: env, Ops: ops(
			defaultOps, envPrefix("T"), autoEnv(), typeByDefault(true), observe(keys...),
			unmarshal("map"),
		)},
		{Name: "values/default_untyped_env", Env: env, Ops: ops(
			defaultOps, envPrefix("T"), autoEnv(), observe(keys...),
		)},
		{Name: "values/nil_default", Ops: ops(
			setDefault("host_schain_node", nil), setDefault("a.b", nil), set("c", nil),
			observe("host_schain_node", "a", "a.b", "c"),
		)},
		{Name: "values/replace_intermediates", Ops: ops(
			setDefault("a", 1), setDefault("a.b", 2), setDefault("a.b.c", 3), set("x.y", 1),
			set("x", map[string]interface{}{"Z": map[interface{}]interface{}{"W": 1}}),
			set("x.z.v", 2), observe("a", "a.b", "a.b.c", "x", "x.y", "x.z", "x.z.w", "x.z.v"),
		)},
		{Name: "values/strings", Ops: stringConversions()},
		{Name: "values/numbers", Ops: numberConversions()},
	}
}

// stringConversions sets strings that the getters parse.
func stringConversions() []*Op {
	var out []*Op
	for i, s := range stringPool {
		key := "s" + strconv.Itoa(i)
		out = append(out, set(key, s))
		for _, g := range getters {
			out = append(out, obs(g, key))
		}
	}
	return out
}

// numberConversions sets numbers that the getters convert.
func numberConversions() []*Op {
	values := []interface{}{
		0, 1, -1, 2147483648, -2147483649, 4294967296, int64(-9223372036854775808),
		uint64(18446744073709551615), uint(9223372036854775808), 3e9, -3e9, 5e9, 1e19, 2e19,
		-0.5, 1.9, -1.9, float32(3.5e9), float32(-1.5), 1e300, uint8(255), int8(-128),
		uint16(65535), int16(-32768), uint32(3000000000), int32(-5),
		math.NaN(), math.Inf(1), math.Inf(-1), 2147483647.5, 4294967295.9, 9.3e18,
	}
	var out []*Op
	for i, n := range values {
		key := "n" + strconv.Itoa(i)
		out = append(out, set(key, n))
		for _, g := range getters {
			out = append(out, obs(g, key))
		}
	}
	return out
}

func sortedKeys(m map[string]interface{}) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// stringPool holds strings that cast parses as numbers, booleans, durations, times, slices
// and maps, or fails to.
var stringPool = []string{
	"", "0", "1", "-1", "42", "+7", "0x1F", "0X1f", "0o17", "017", "0b101", "1_000", "08",
	"1.0", "1.50", "-2.000", "1.", ".0", "1e3", "3.7", "-0", "0.0", "1e-7", "NaN", "Inf",
	"9223372036854775807", "9223372036854775808", "18446744073709551615",
	"-9223372036854775809", "4294967296", "2147483648", "-2147483649",
	"true", "TRUE", "True", "t", "F", "yes", "on", "no", " 1", "1 ",
	"1s", "1.5h", "-3m", "100", "1us", "1µs", "2h45m", "1d", "abc", "-", "1e3s",
	"2006-01-02", "2006-01-02T15:04:05Z", "2006-01-02T15:04:05+07:00",
	"2006-01-02T15:04:05.123456789-08:00", "2006-01-02T15:04:05", "Mon Jan  2 15:04:05 2006",
	"Mon Jan 2 15:04:05 MST 2006", "02 Jan 06 15:04 MST", "02 Jan 06 15:04 +0100",
	"Mon, 02 Jan 2006 15:04:05 UTC", "Monday, 02-Jan-06 15:04:05 PST",
	"2006-01-02 15:04:05.999999999 -0700 MST", "2016-03-06 15:28:01", "2016-03-06 15:28:01 +0000",
	"2006-01-02 15:04:05Z07:00", "1136214245", "15:04", "3:04PM", "Jan 2 15:04:05.000",
	"Jan 2, 2006", "02 Jan 2006",
	"a b  c", "a,b", "[1,2]", `{"a":"b"}`, `{"a":["x","y"]}`, `{"a":1}`, `{"a":"x y"}`,
	"{bad", "null", `[]`, `{}`, "10 KB", "3 xb", "5 kib", "1.5mb", "2GB",
}
