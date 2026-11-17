package main

import (
	"encoding/json"
	"fmt"
	"log"
	"math/rand"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

// leaves are the keys of Outer's fields (and of Inner's, under inner and by_name).
var leaves = []string{
	"host", "port", "flag", "nums", "uints", "items",
	"inner.name", "inner.count", "inner.enabled", "inner.ratio", "inner.tags",
	"inner.labels.k", "inner.labels.other", "inner.timeout", "inner.ptr", "inner.any",
	"by_name.a.name", "by_name.a.count", "by_name.b.tags", "misc.x", "misc.y.z",
}

// parents are the maps on the leaves' paths, and other keys worth reading.
var parents = []string{
	"inner", "inner.labels", "by_name", "by_name.a", "misc", "misc.y", "items.0", "items.1.name",
	"Inner.Name", "INNER", "host.sub",
}

// innerFields are Inner's keys, for the maps of random values.
var innerFields = []string{"name", "count", "enabled", "ratio", "tags", "labels", "timeout", "ptr", "any"}

var words = []string{"alpha", "Beta", "a b", "x,y", "7", "true", "1s", "", "0x10", "2.5"}

// envPool holds environment values.
var envPool = []string{
	"7", "-3", "0x10", "3.5", "true", "FALSE", "a b c", "x,y", "1s", "1500ms", "", "x",
	"2006-01-02", `{"a":"b"}`, "10mb", " 42 ",
}

func pick(r *rand.Rand, from []string) string {
	return from[r.Intn(len(from))]
}

// recase writes a key in lower, title or upper case.
func recase(r *rand.Rand, key string) string {
	switch r.Intn(4) {
	case 0:
		return strings.ToUpper(key)
	case 1:
		if key == "" {
			return key
		}
		return strings.ToUpper(key[:1]) + key[1:]
	}
	return key
}

func randomScalar(r *rand.Rand) interface{} {
	switch r.Intn(6) {
	case 0:
		return r.Intn(200) - 50
	case 1:
		return pick(r, stringPool)
	case 2:
		return r.Intn(2) == 0
	case 3:
		return float64(r.Intn(1000)-200) / 8
	case 4:
		return pick(r, words)
	}
	return nil
}

// randomMap is a map[string]interface{} over some of Inner's keys, each in one case.
func randomMap(r *rand.Rand, depth int) map[string]interface{} {
	m := map[string]interface{}{}
	for _, i := range r.Perm(len(innerFields))[:1+r.Intn(3)] {
		m[recase(r, innerFields[i])] = randomValue(r, depth+1)
	}
	return m
}

// randomValue is a value for SetDefault and Set.
func randomValue(r *rand.Rand, depth int) interface{} {
	switch r.Intn(18) {
	case 0, 1:
		return r.Intn(200) - 50
	case 2, 3:
		return pick(r, stringPool)
	case 4:
		return r.Intn(2) == 0
	case 5:
		return float64(r.Intn(1000)) / 8
	case 6:
		return time.Duration(r.Intn(5000)) * time.Millisecond
	case 7:
		s := []string{}
		for i := r.Intn(4); i > 0; i-- {
			s = append(s, pick(r, words))
		}
		return s
	case 8:
		s := []int{}
		for i := r.Intn(4); i > 0; i-- {
			s = append(s, r.Intn(20)-5)
		}
		return s
	case 9:
		s := []interface{}{}
		for i := r.Intn(4); i > 0; i-- {
			s = append(s, randomScalar(r))
		}
		return s
	case 10, 11:
		if depth < 2 {
			return randomMap(r, depth)
		}
		return pick(r, words)
	case 12:
		return int64(r.Intn(1 << 20))
	case 13:
		return uint(r.Intn(1000))
	case 14:
		return map[string]string{recase(r, "k"): pick(r, words)}
	case 15:
		return int32(r.Intn(100) - 50)
	case 16:
		return []interface{}{randomMap(r, 2), randomMap(r, 2)}
	}
	return nil
}

// fileValue is a value for a configuration file (what JSON and YAML can hold).
func fileValue(r *rand.Rand) interface{} {
	switch r.Intn(10) {
	case 0, 1:
		return r.Intn(200) - 50
	case 2, 3:
		return pick(r, stringPool)
	case 4:
		return r.Intn(2) == 0
	case 5:
		return float64(r.Intn(1000)) / 8
	case 6:
		s := []interface{}{}
		for i := r.Intn(4); i > 0; i-- {
			s = append(s, randomScalar(r))
		}
		return s
	case 7:
		m := map[string]interface{}{}
		for _, i := range r.Perm(len(innerFields))[:1+r.Intn(3)] {
			m[innerFields[i]] = randomScalar(r)
		}
		return m
	case 8:
		return []interface{}{
			map[string]interface{}{"name": pick(r, words), "count": r.Intn(9)},
			map[string]interface{}{"Name": pick(r, words), "tags": []interface{}{pick(r, words)}},
		}
	}
	return nil
}

// deepPut sets path in a nested map unless a value that is not a map is on the way.
func deepPut(m map[string]interface{}, path []string, value interface{}) {
	for _, k := range path[:len(path)-1] {
		next, ok := m[k]
		if !ok {
			child := map[string]interface{}{}
			m[k] = child
			m = child
			continue
		}
		child, ok := next.(map[string]interface{})
		if !ok {
			return
		}
		m = child
	}
	if _, ok := m[path[len(path)-1]]; !ok {
		m[path[len(path)-1]] = value
	}
}

// recaseTree writes each key of a tree in a random case (each key once, so that no two keys
// of a map differ only in case).
func recaseTree(r *rand.Rand, v interface{}) interface{} {
	switch t := v.(type) {
	case map[string]interface{}:
		out := map[string]interface{}{}
		for k, item := range t {
			out[recase(r, k)] = recaseTree(r, item)
		}
		return out
	case []interface{}:
		out := make([]interface{}, len(t))
		for i, item := range t {
			out[i] = recaseTree(r, item)
		}
		return out
	}
	return v
}

// randomFile is the content of a configuration file in the format of ext.
func randomFile(r *rand.Rand, ext string) string {
	tree := map[string]interface{}{}
	if r.Intn(4) == 0 {
		// A scalar where other sources have a map.
		tree[pick(r, []string{"inner", "by_name", "misc"})] = randomScalar(r)
	}
	for _, i := range r.Perm(len(leaves))[:2+r.Intn(8)] {
		deepPut(tree, strings.Split(leaves[i], "."), fileValue(r))
	}
	out := recaseTree(r, tree)
	var data []byte
	var err error
	if ext == "json" {
		data, err = json.MarshalIndent(out, "", "  ")
	} else {
		data, err = yaml.Marshal(out)
	}
	if err != nil {
		log.Fatal(err)
	}
	return string(data)
}

// envName is the variable AutomaticEnv reads for key.
func envName(prefix, key string, pairs []string) string {
	name := strings.ToUpper(key)
	if prefix != "" {
		name = strings.ToUpper(prefix + "_" + key)
	}
	if len(pairs) > 0 {
		name = strings.NewReplacer(pairs...).Replace(name)
	}
	return name
}

// randomScenarios are seeded random combinations of every source.
func randomScenarios(seed int64, n int) []*Scenario {
	r := rand.New(rand.NewSource(seed))
	out := make([]*Scenario, 0, n)
	for i := 0; i < n; i++ {
		out = append(out, randomScenario(r, fmt.Sprintf("random/%d", i)))
	}
	return out
}

func randomScenario(r *rand.Rand, name string) *Scenario {
	s := &Scenario{Name: name, Files: map[string]string{}, Env: map[string]string{}, random: true}
	var setup []*Op

	for i := 3 + r.Intn(6); i > 0; i-- {
		key := pick(r, leaves)
		if r.Intn(4) == 0 {
			key = pick(r, parents[:6])
		}
		setup = append(setup, setDefault(recase(r, key), randomValue(r, 0)))
	}
	for i := r.Intn(5); i > 0; i-- {
		key := pick(r, leaves)
		if r.Intn(5) == 0 {
			key = pick(r, parents[:6])
		}
		setup = append(setup, set(recase(r, key), randomValue(r, 0)))
	}

	// The environment.
	prefix := pick(r, []string{"", "R", "app"})
	var pairs []string
	switch r.Intn(4) {
	case 0, 1:
		pairs = []string{".", "_"}
	case 2:
		pairs = []string{"-", "_", ".", "_"}
	}
	if prefix != "" {
		setup = append(setup, envPrefix(prefix))
	}
	if pairs != nil {
		setup = append(setup, replacer(pairs...))
	}
	if r.Intn(10) < 7 {
		setup = append(setup, autoEnv())
	}
	if r.Intn(10) < 3 {
		setup = append(setup, allowEmptyEnv(true))
	}
	if r.Intn(2) == 0 {
		setup = append(setup, typeByDefault(true))
	}
	for _, i := range r.Perm(len(leaves))[:r.Intn(6)] {
		key := leaves[i]
		s.Env[envName(prefix, key, pairs)] = pick(r, envPool)
		if r.Intn(4) == 0 {
			s.Env[envName("", key, pairs)] = pick(r, envPool)
		}
	}
	for _, i := range r.Perm(len(leaves))[:r.Intn(4)] {
		key := leaves[i]
		switch r.Intn(3) {
		case 0:
			setup = append(setup, bindEnv(recase(r, key)))
			s.Env[envName(prefix, key, pairs)] = pick(r, envPool)
		case 1:
			alt := fmt.Sprintf("ALT_%d", i)
			setup = append(setup, bindEnv(key, alt, alt+"_B"))
			if r.Intn(2) == 0 {
				s.Env[alt] = pick(r, envPool)
			}
			s.Env[alt+"_B"] = pick(r, envPool)
		default:
			setup = append(setup, bindEnv(key))
		}
	}
	r.Shuffle(len(setup), func(i, j int) { setup[i], setup[j] = setup[j], setup[i] })

	// The configuration file.
	var config []*Op
	switch r.Intn(5) {
	case 0, 1, 2:
		cfgName := pick(r, []string{"config", "app"})
		config = append(config, configName(cfgName))
		dirs := []string{"a", "b", "c", "@TMP@/b", "missing"}
		r.Shuffle(len(dirs), func(i, j int) { dirs[i], dirs[j] = dirs[j], dirs[i] })
		for _, dir := range dirs[:1+r.Intn(len(dirs))] {
			config = append(config, addConfigPath(dir))
		}
		for _, dir := range []string{"a", "b", "c"} {
			if r.Intn(2) == 0 {
				ext := pick(r, []string{"json", "yaml", "yml"})
				s.Files[dir+"/"+cfgName+"."+ext] = randomFile(r, ext)
			}
		}
		config = append(config, readInConfig(), configFileUsed())
	case 3:
		ext := pick(r, []string{"json", "yaml"})
		s.Files["conf."+ext] = randomFile(r, ext)
		config = append(config, configFile("@TMP@/conf."+ext), readInConfig())
	default:
		ext := pick(r, []string{"json", "yaml"})
		config = append(config, configType(ext), readConfig(randomFile(r, ext)))
	}
	at := r.Intn(len(setup) + 1)
	setup = append(setup[:at], append(config, setup[at:]...)...)

	// Interleave a few reads, then read everything.
	var ops []*Op
	keys := append(append([]string{}, leaves...), parents...)
	for _, o := range setup {
		ops = append(ops, o)
		if r.Intn(5) == 0 {
			ops = append(ops, get(pick(r, keys)))
		}
	}
	for _, k := range keys {
		ops = append(ops, get(k), isSet(k), inConfig(k))
	}
	for _, i := range r.Perm(len(keys))[:8] {
		for _, g := range getters[1:18] {
			ops = append(ops, obs(g, keys[i]))
		}
	}
	ops = append(ops, allKeys(), allSettings(),
		unmarshal("outer"), unmarshalExact("outer"), unmarshal("map"),
		unmarshalKey("inner", "inner"), unmarshalKey("by_name.a", "inner"),
		unmarshalKey("inner.tags", "strings"), unmarshalKey("nums", "ints"),
		unmarshalKey("port", "int"), unmarshalKey("host", "string"),
		unmarshalKey("flag", "bool_ptr"), unmarshalKey("inner.timeout", "duration"),
		unmarshalKey("inner.labels", "string_map_string"), unmarshalKey("items", "map"),
		sub("inner"), sub("by_name"), sub("misc"), sub("host"),
	)
	s.Ops = ops
	return s
}
