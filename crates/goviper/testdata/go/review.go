package main

import (
	"strconv"
	"strings"
	"time"
)

// reviewScenarios are the adversarial review's cases: the behaviour Prebid's configuration
// loading leans on, at the edges the other scenarios do not reach (IsSet of default-only,
// env-only and parent keys, nested keys overridden by the environment, BindEnv with several
// names, replacers for keys with dashes, string slices and booleans read from the environment,
// maps merged across sources, key case in AllSettings, file search precedence and empty
// variables).
func reviewScenarios() []*Scenario {
	var s []*Scenario
	s = append(s, reviewIsSet()...)
	s = append(s, reviewNestedEnv()...)
	s = append(s, reviewBindEnv()...)
	s = append(s, reviewReplacer()...)
	s = append(s, reviewSlicesAndBools()...)
	s = append(s, reviewMerge()...)
	s = append(s, reviewCase()...)
	s = append(s, reviewSearch()...)
	s = append(s, reviewEmptyEnv()...)
	s = append(s, reviewOddKeys()...)
	s = append(s, reviewTypedEnv()...)
	s = append(s, reviewUnicodeKeys()...)
	s = append(s, reviewCastSweep()...)
	s = append(s, reviewRandomScenarios(20261118, 200)...)
	return s
}

func reviewIsSet() []*Scenario {
	keys := []string{
		"a", "a.b", "a.b.c", "a.b.c.d", "a.x", "A.B.C", "e", "e.f", "e.f.g", "env_only",
		"envonly.nested", "envonly", "bound", "bound.child", "bnested", "bnested.k", "nil_def",
		"empty_map", "empty_map.k", "empty_slice", "file_null", "file_empty_map", "file_list",
		"file_list.0", "file_list.5", "file_list.x", "dot.ted", "dot", "missing",
	}
	return []*Scenario{
		{Name: "review/is_set/defaults_env_and_parents", Env: map[string]string{
			"ENV_ONLY": "1", "ENVONLY_NESTED": "2", "BOUND_VAR": "3", "BNESTED_K": "4",
			"E_F": "scalar",
		}, Ops: ops(
			setDefault("a.b.c", 1), setDefault("e.f.g", true), setDefault("nil_def", nil),
			setDefault("empty_map", map[string]interface{}{}), setDefault("empty_slice", []string{}),
			bindEnv("bound", "BOUND_VAR"), bindEnv("bnested.k"),
			configType("yaml"),
			readConfig("file_null: ~\nfile_empty_map: {}\nfile_list: [1, {x: 2}]\ndot.ted: 5\n"),
			obsEach([]string{"is_set", "in_config", "get"}, keys),
			autoEnv(), replacer(".", "_"),
			obsEach([]string{"is_set", "in_config", "get"}, keys),
			allKeys(), allSettings(),
		)},
	}
}

func reviewNestedEnv() []*Scenario {
	keys := []string{"a", "a.b", "a.b.c", "a.b.d", "a.e", "x", "x.y", "x.y.z"}
	return []*Scenario{
		{Name: "review/nested_env/leaf_override", Env: map[string]string{
			"PBS_A_B_C": "9", "PBS_X_Y_Z": "env",
		}, Files: map[string]string{"pbs.yaml": "a:\n  b:\n    c: 3\n  e: file\nx:\n  y:\n    z: file\n"}, Ops: ops(
			setDefault("a.b.c", 1), setDefault("a.b.d", 2), setDefault("x.y.z", "def"),
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(),
			replacer(".", "_"), envPrefix("PBS"), autoEnv(),
			observe(keys...), unmarshal("map"), unmarshalKey("a", "map"), unmarshalKey("a.b", "map"),
			sub("a"), sub("a.b"), typeByDefault(true), observe(keys...), unmarshal("map"),
		)},
		{Name: "review/nested_env/parent_scalar_shadows", Env: map[string]string{
			"PBS_A_B": "scalar", "PBS_X": "top",
		}, Files: map[string]string{"pbs.json": `{"a": {"b": {"c": 3}, "e": "file"}, "x": {"y": {"z": 1}}}`}, Ops: ops(
			setDefault("a.b.c", 1), setDefault("a.b.d", 2), setDefault("x.y.z", "def"),
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(),
			replacer(".", "_"), envPrefix("PBS"), autoEnv(),
			observe(keys...), unmarshal("map"),
		)},
		{Name: "review/nested_env/bound_only", Env: map[string]string{
			"A_B": "scalar", "LEAF": "leaf",
		}, Ops: ops(
			setDefault("a.b.c", 1), bindEnv("a.b"), bindEnv("x.y", "LEAF"),
			observe(keys...), unmarshal("map"),
		)},
		{Name: "review/nested_env/env_map_json", Env: map[string]string{
			"PBS_M": `{"k":"v","n":{"x":1}}`, "PBS_L": `["a","b"]`,
		}, Ops: ops(
			setDefault("m", map[string]interface{}{"k": "d"}), setDefault("l", []string{"z"}),
			envPrefix("PBS"), autoEnv(), typeByDefault(true),
			observe("m", "m.k", "l", "l.0"), unmarshal("map"),
		)},
	}
}

func reviewBindEnv() []*Scenario {
	keys := []string{"k", "k2", "k3", "k4", "Mixed.Key"}
	return []*Scenario{
		{Name: "review/bind_env/precedence", Env: map[string]string{
			"FIRST": "1", "SECOND": "2", "THIRD": "", "lower_name": "lower", "LOWER_NAME": "upper",
			"K3": "auto", "P_K4": "prefixed", "K4": "plain", "MIXED_KEY": "mk", "P_MIXED.KEY": "dotted",
		}, Ops: ops(
			bindEnv("k", "THIRD", "SECOND", "FIRST"), bindEnv("k", "FIRST"),
			bindEnv("K2", "lower_name"), bindEnv("k3", "SECOND"),
			observe(keys...),
			envPrefix("p"), bindEnv("k4"), bindEnv("Mixed.Key"), observe(keys...),
			autoEnv(), observe(keys...),
			replacer(".", "_", "lower", "LOWER"), observe(keys...),
		)},
		{Name: "review/bind_env/appends", Env: map[string]string{"B": "b", "C": "c"}, Ops: ops(
			bindEnv("k", "A"), bindEnv("k", "B"), bindEnv("k", "C"), get("k"),
			bindEnv("j"), bindEnv("j", "C"), get("j"), allKeys(),
		)},
	}
}

func reviewReplacer() []*Scenario {
	env := map[string]string{
		"APP_MY_KEY_SUB_KEY": "underscored", "APP_MY-KEY_SUB-KEY": "dashed",
		"APP_MY__KEY": "double", "APP_A_B": "ab", "APP_A__B": "a__b",
		"APP_GPP-SUPPORTED": "raw", "APP_GPP_SUPPORTED": "cooked",
	}
	keys := []string{"my-key.sub-key", "my.key", "my-key", "a.b", "a-b", "a_b", "gpp-supported"}
	return []*Scenario{
		{Name: "review/replacer/dashes", Env: env, Ops: ops(
			envPrefix("app"), autoEnv(), obsEach([]string{"get", "is_set"}, keys),
			replacer(".", "_"), obsEach([]string{"get", "is_set"}, keys),
			replacer(".", "_", "-", "_"), obsEach([]string{"get", "is_set"}, keys),
			replacer("-", "_", ".", "__"), obsEach([]string{"get", "is_set"}, keys),
			replacer(".", "__", "-", "_", "_", "-"), obsEach([]string{"get", "is_set"}, keys),
			replacer("", "x"), obsEach([]string{"get"}, keys),
			replacer("APP", "APP"), obsEach([]string{"get"}, keys),
		)},
	}
}

func reviewSlicesAndBools() []*Scenario {
	slices := []string{
		"a, b", "a,b", " a  b ", "a\tb\nc", "", ",", "a,,b", " ", "a, b,", "\u00a0a\u2003b",
		"[a, b]", `["a","b"]`, "x",
	}
	bools := []string{
		"1", "0", "t", "T", "f", "F", "TRUE", "True", "true", "tRuE", "FALSE", "False", "false",
		"yes", "no", "on", "off", "y", "n", " true", "true ", "", "2", "-1", "1.0", "0x1",
	}
	var s []*Scenario
	sliceEnv := map[string]string{}
	var sliceOps []*Op
	for i, v := range slices {
		key := "sl" + strconv.Itoa(i)
		sliceEnv["S_"+strings.ToUpper(key)] = v
		sliceOps = append(sliceOps, setDefault("typed."+key, []string{"d"}), set("set"+key, v))
	}
	sliceOps = append(sliceOps, envPrefix("s"), autoEnv(), replacer("TYPED.", ""), allowEmptyEnv(true))
	for i := range slices {
		key := "sl" + strconv.Itoa(i)
		sliceOps = append(sliceOps, obs("get_string_slice", key), obs("get_string_slice", "set"+key),
			obs("get", "typed."+key), obs("get_string_slice", "typed."+key))
	}
	sliceOps = append(sliceOps, typeByDefault(true))
	for i := range slices {
		key := "sl" + strconv.Itoa(i)
		sliceOps = append(sliceOps, obs("get", "typed."+key), obs("get_string_slice", "typed."+key),
			unmarshalKey("typed."+key, "strings"), unmarshalKey("set"+key, "strings"))
	}
	sliceOps = append(sliceOps, allSettings(), unmarshal("map"))
	s = append(s, &Scenario{Name: "review/slices", Env: sliceEnv, Ops: sliceOps})

	boolEnv := map[string]string{}
	var boolOps []*Op
	for i, v := range bools {
		key := "b" + strconv.Itoa(i)
		boolEnv["B_"+strings.ToUpper(key)] = v
		boolEnv["B_TYPED_"+strings.ToUpper(key)] = v
		boolOps = append(boolOps, setDefault("typed."+key, false), set("set"+key, v))
	}
	boolOps = append(boolOps, envPrefix("b"), replacer(".", "_"), autoEnv())
	for i := range bools {
		key := "b" + strconv.Itoa(i)
		boolOps = append(boolOps, obs("get_bool", key), obs("get_bool", "set"+key),
			obs("get", "typed."+key), unmarshalKey(key, "bool_ptr"))
	}
	boolOps = append(boolOps, typeByDefault(true))
	for i := range bools {
		key := "b" + strconv.Itoa(i)
		boolOps = append(boolOps, obs("get", "typed."+key), obs("get_bool", "typed."+key),
			unmarshalKey("typed."+key, "bool_ptr"))
	}
	s = append(s, &Scenario{Name: "review/bools", Env: boolEnv, Ops: boolOps})
	s = append(s, &Scenario{Name: "review/bools_yaml", Ops: ops(
		configType("yaml"),
		readConfig("a: yes\nb: on\nc: \"true\"\nd: 1\ne: True\nf: TRUE\ng: tRue\nh: y\ni: ~\nj: 0\nk: 1.0\n"),
		obsEach([]string{"get", "get_bool", "get_string"}, []string{"a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"}),
	)})
	return s
}

func reviewMerge() []*Scenario {
	keys := []string{"m", "m.a", "m.b", "m.c", "m.n", "m.n.x", "m.n.y", "m.n.z", "o", "o.p", "o.q"}
	return []*Scenario{
		{Name: "review/merge/default_map_and_file_map", Files: map[string]string{
			"pbs.yaml": "M:\n  B: file\n  C: file\n  N:\n    Y: file\n    Z: [1, 2]\nO: scalar\n",
		}, Ops: ops(
			setDefault("m", map[string]interface{}{"A": "def", "B": "def", "N": map[string]interface{}{"X": 1, "Y": 2}}),
			setDefault("o", map[interface{}]interface{}{"P": 1, "q": map[interface{}]interface{}{"R": 2}}),
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(),
			observe(keys...), unmarshal("map"), unmarshalKey("m", "map"), unmarshalKey("m.n", "map"),
			sub("m"), sub("m.n"), sub("o"),
			set("m.n.w", "over"), set("O.P", "over"), observe(keys...), unmarshal("map"),
			set("m", map[string]interface{}{"Only": "override"}), observe(keys...), unmarshal("map"),
		)},
		{Name: "review/merge/default_map_file_scalar", Files: map[string]string{
			"pbs.json": `{"m": "scalar", "o": {"p": null}}`,
		}, Ops: ops(
			setDefault("m.a", 1), setDefault("m", map[string]interface{}{"b": 2}),
			setDefault("o.p", 3), setDefault("o.q", 4),
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(),
			observe(keys...), unmarshal("map"),
		)},
	}
}

func reviewCase() []*Scenario {
	return []*Scenario{
		{Name: "review/case/all_settings", Files: map[string]string{
			"pbs.yaml": "Top:\n  MiXed: 1\n  List:\n    - Name: A\n      Inner: {DeepKey: 1}\n    - [X, {Y: 1}]\n  Dotted.Key: 2\nUPPER: {LOWER: {Mid: 3}}\n",
		}, Env: map[string]string{"TOP_ENVKEY": "env"}, Ops: ops(
			setDefault("Def.CamelCase", map[string]interface{}{"InnerKey": []interface{}{map[string]interface{}{"SliceKey": 1}}}),
			set("Over.Ride", map[interface{}]interface{}{"MapKey": map[string]interface{}{"Deep": 1}, 7: "seven"}),
			bindEnv("Top.EnvKey"),
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(),
			allKeys(), allSettings(), unmarshal("map"),
			observe("top", "Top.Mixed", "top.list", "top.list.0.name", "top.list.0.Name",
				"top.list.0.inner.deepkey", "top.list.1.1.y", "top.dotted.key", "upper.lower.mid",
				"def.camelcase.innerkey", "over.ride.mapkey.deep", "over.ride.7", "top.envkey"),
			sub("top"), sub("Over.Ride"),
		)},
	}
}

func reviewSearch() []*Scenario {
	return []*Scenario{
		{Name: "review/search/json_and_yaml", Files: map[string]string{
			"pbs.json": `{"from": "json", "j": 1}`, "pbs.yaml": "from: yaml\ny: 1\n",
		}, Ops: ops(
			configName("pbs"), addConfigPath("."), addConfigPath("/etc/config"), readInConfig(),
			configFileUsed(), get("from"), get("y"), isSet("y"), allSettings(),
		)},
		{Name: "review/search/yaml_in_first_path_json_in_second", Files: map[string]string{
			"one/pbs.yaml": "from: yaml\n", "two/pbs.json": `{"from": "json"}`,
		}, Ops: ops(
			configName("pbs"), addConfigPath("one"), addConfigPath("two"), readInConfig(),
			configFileUsed(), get("from"),
		)},
		{Name: "review/search/dangling_and_dir_links", Files: map[string]string{
			"pbs.yml": "from: yml\n",
		}, Dirs: []string{"d"}, Links: map[string]string{"pbs.json": "nowhere.json", "pbs.yaml": "d"}, Ops: ops(
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(), configFileUsed(), get("from"),
		)},
		{Name: "review/search/link_to_file", Files: map[string]string{
			"real.yaml": "from: link\n",
		}, Links: map[string]string{"pbs.yaml": "real.yaml"}, Ops: ops(
			configName("pbs"), addConfigPath("@TMP@"), readInConfig(), configFileUsed(), get("from"),
		)},
		{Name: "review/search/home_unset", Files: map[string]string{
			"pbs.yaml": "from: cwd\n",
		}, Ops: ops(
			configName("pbs"), addConfigPath("$HOME"), addConfigPath("."), readInConfig(),
			configFileUsed(), get("from"),
		)},
		{Name: "review/search/name_change_and_retry", Files: map[string]string{
			"a.yaml": "from: a\n", "b.json": `{"from": "b"}`,
		}, Ops: ops(
			configName("missing"), addConfigPath("@TMP@"), readInConfig(), configFileUsed(),
			configName("a"), readInConfig(), configFileUsed(), get("from"),
			configName("b"), readInConfig(), configFileUsed(), get("from"),
			configType("yaml"), configName("a"), readInConfig(), get("from"),
		)},
		{Name: "review/search/empty_and_odd_names", Files: map[string]string{
			".yaml": "from: dot\n", "pbs.yaml.json": `{"from": "double"}`,
		}, Ops: ops(
			configName(""), addConfigPath("@TMP@"), addConfigPath(""), readInConfig(),
			configFileUsed(), get("from"), configName("pbs.yaml"), readInConfig(),
			configFileUsed(), get("from"),
		)},
	}
}

func reviewEmptyEnv() []*Scenario {
	keys := []string{"a", "a.b", "c", "d", "e"}
	env := map[string]string{"A": "", "A_B": "", "C": "", "D": " ", "BOUND": "", "E": "set"}
	return []*Scenario{
		{Name: "review/empty_env", Env: env, Ops: ops(
			setDefault("a.b", "def"), setDefault("c", 5), setDefault("d", true),
			bindEnv("e", "BOUND", "E"), replacer(".", "_"), autoEnv(),
			observe(keys...), unmarshal("map"),
			typeByDefault(true), observe(keys...),
			allowEmptyEnv(true), observe(keys...), unmarshal("map"),
			allowEmptyEnv(false), observe(keys...),
		)},
	}
}

func reviewOddKeys() []*Scenario {
	keys := []string{"", ".", "a.", ".a", "a..b", "a", "a.b", "ä", "Ä", "ÄB.C", "k-ey.x_y", " sp ", "sp"}
	return []*Scenario{
		{Name: "review/odd_keys", Env: map[string]string{
			"A_": "under", "_A": "lead", "ÄB_C": "umlaut", "K-EY_X_Y": "dash", " SP ": "space",
		}, Ops: ops(
			setDefault("a.", 1), setDefault(".a", 2), setDefault("a..b", 3), setDefault("", 4),
			setDefault("Ä", 5), setDefault(" sp ", 6),
			observe(keys...),
			replacer(".", "_"), autoEnv(), observe(keys...), unmarshal("map"),
		)},
	}
}

// reviewCastSweep sets every string of one or two characters over an alphabet of the
// characters cast's parsers care about (and every three-character string over a smaller one),
// and reads each with the scalar getters.
func reviewCastSweep() []*Scenario {
	var strs []string
	wide := strings.Split("0189x_.-+eEbBoOtTfF sm", "")
	for _, a := range wide {
		strs = append(strs, a)
		for _, b := range wide {
			strs = append(strs, a+b)
		}
	}
	narrow := strings.Split("0x1_.-e", "")
	for _, a := range narrow {
		for _, b := range narrow {
			for _, c := range narrow {
				strs = append(strs, a+b+c)
			}
		}
	}
	getters := []string{"get_bool", "get_int", "get_int32", "get_uint", "get_uint32", "get_float64",
		"get_duration", "get_string_slice", "get_size_in_bytes"}
	var out []*Op
	for i, s := range strs {
		key := "c" + strconv.Itoa(i)
		out = append(out, set(key, s))
		for _, g := range getters {
			out = append(out, obs(g, key))
		}
	}
	return []*Scenario{{Name: "review/cast_sweep", Ops: out}}
}

// reviewUnicodeKeys has keys whose case mappings differ between Go's simple Unicode mappings
// and full case conversion (final sigma, ß, dotted capital I, the Kelvin sign, titlecase
// digraphs).
func reviewUnicodeKeys() []*Scenario {
	names := []string{"ΣΑΣ", "σας", "ß", "ẞ", "İx", "ıx", "\u212aelvin", "ǅx", "ǆx", "ﬁle", "Ωmega", "ǰ"}
	env := map[string]string{}
	var setup []*Op
	var keys []string
	for i, n := range names {
		env[strings.ToUpper(n)] = "env" + strconv.Itoa(i)
		env[strings.ToUpper("u."+n)] = "nested" + strconv.Itoa(i)
		setup = append(setup, setDefault("d."+n, i))
		keys = append(keys, n, strings.ToLower(n), strings.ToUpper(n), "d."+n, "d."+strings.ToUpper(n),
			"u."+n, "f."+n, "f."+strings.ToLower(n))
	}
	var file strings.Builder
	file.WriteString("f:\n")
	for i, n := range names {
		file.WriteString("  \"" + n + "\": " + strconv.Itoa(i) + "\n")
	}
	return []*Scenario{{Name: "review/unicode_keys", Env: env, Ops: ops(
		setup, configType("yaml"), readConfig(file.String()), observe(keys...),
		replacer(".", "_"), autoEnv(), obsEach([]string{"get", "is_set"}, keys), allKeys(),
		allSettings(),
	)}}
}

func reviewTypedEnv() []*Scenario {
	ints := []string{"08", "0x10", "1_000", "1.0", "1.5", " 5", "+5", "-0", "0b11", "0o17", "017", "1e3", "9223372036854775808", "-1", "abc"}
	floats := []string{"1e3", "inf", "-Inf", "NaN", "0x1p-2", "1_000.5", "1_000", ".5", "5.", "1e400", "0x10", "abc", "+3.25"}
	durations := []string{"5", "5s", "1h2m", "1.5", "-3", "1d", "5S", "abc", "1µs", "1μs", "0"}
	var ops []*Op
	env := map[string]string{}
	for i, v := range ints {
		k := "i" + strconv.Itoa(i)
		env["T_"+strings.ToUpper(k)] = v
		env["T_U"+strings.ToUpper(k)] = v
		env["T_U32"+strings.ToUpper(k)] = v
		env["T_I64"+strings.ToUpper(k)] = v
		env["T_I32"+strings.ToUpper(k)] = v
		ops = append(ops, setDefault(k, 7), setDefault("u"+k, uint(7)), setDefault("u32"+k, uint32(7)),
			setDefault("i64"+k, int64(7)), setDefault("i32"+k, int32(7)))
	}
	for i, v := range floats {
		k := "f" + strconv.Itoa(i)
		env["T_"+strings.ToUpper(k)] = v
		ops = append(ops, setDefault(k, 1.5))
	}
	for i, v := range durations {
		k := "d" + strconv.Itoa(i)
		env["T_"+strings.ToUpper(k)] = v
		ops = append(ops, setDefault(k, time.Duration(0)))
	}
	ops = append(ops, envPrefix("t"), autoEnv(), typeByDefault(true))
	for i := range ints {
		k := "i" + strconv.Itoa(i)
		ops = append(ops, get(k), get("u"+k), get("u32"+k), get("i64"+k), get("i32"+k),
			obs("get_size_in_bytes", k))
	}
	for i := range floats {
		ops = append(ops, get("f"+strconv.Itoa(i)))
	}
	for i := range durations {
		k := "d" + strconv.Itoa(i)
		ops = append(ops, get(k), unmarshalKey(k, "duration"))
	}
	ops = append(ops, allSettings())
	return []*Scenario{{Name: "review/typed_env", Env: env, Ops: ops}}
}

// obsEach makes each observation of each key.
func obsEach(observations []string, keys []string) []*Op {
	var out []*Op
	for _, k := range keys {
		for _, o := range observations {
			out = append(out, obs(o, k))
		}
	}
	return out
}
