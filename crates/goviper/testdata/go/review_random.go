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

// reviewSegments build the review's random keys: every path of one to three of them (with a
// dash and an underscore, which replacers rewrite).
var reviewSegments = []string{"a", "b-c", "d_e"}

// reviewKeys are every path of one to three reviewSegments.
func reviewKeys() []string {
	var keys []string
	var walk func(prefix string, depth int)
	walk = func(prefix string, depth int) {
		for _, s := range reviewSegments {
			k := s
			if prefix != "" {
				k = prefix + "." + s
			}
			keys = append(keys, k)
			if depth < 3 {
				walk(k, depth+1)
			}
		}
	}
	walk("", 1)
	return keys
}

// reviewEnvValues are the variables' values: numbers, booleans, durations, lists, JSON and
// empty strings.
var reviewEnvValues = []string{
	"", "0", "1", "-5", "0x1f", "08", "2.50", "true", "F", "yes", "a b", "a,b", " x ", "5s",
	"100", `{"k":"v"}`, `["l"]`, "10kb",
}

func reviewScalar(r *rand.Rand) interface{} {
	switch r.Intn(12) {
	case 0:
		return r.Intn(100) - 20
	case 1:
		return int64(r.Intn(1 << 30))
	case 2:
		return uint(r.Intn(50))
	case 3:
		return uint32(r.Intn(50))
	case 4:
		return float64(r.Intn(400)) / 16
	case 5:
		return r.Intn(2) == 0
	case 6:
		return time.Duration(r.Intn(4000)) * time.Millisecond
	case 7:
		return []string{pick(r, reviewEnvValues), pick(r, reviewEnvValues)}
	case 8:
		return []int{r.Intn(9), r.Intn(9) - 4}
	case 9:
		return nil
	case 10:
		return int32(r.Intn(100))
	}
	return pick(r, reviewEnvValues)
}

// reviewValue is a value for SetDefault and Set: a scalar or a map of scalars (keys recased).
func reviewValue(r *rand.Rand) interface{} {
	if r.Intn(5) != 0 {
		return reviewScalar(r)
	}
	m := map[string]interface{}{}
	for _, i := range r.Perm(len(reviewSegments))[:1+r.Intn(len(reviewSegments))] {
		m[recase(r, reviewSegments[i])] = reviewScalar(r)
	}
	return m
}

// reviewFileValue is what a configuration file can hold.
func reviewFileValue(r *rand.Rand) interface{} {
	switch r.Intn(7) {
	case 0:
		return r.Intn(100) - 20
	case 1:
		return float64(r.Intn(400)) / 16
	case 2:
		return r.Intn(2) == 0
	case 3:
		return []interface{}{pick(r, reviewEnvValues), r.Intn(5)}
	case 4:
		return nil
	}
	return pick(r, reviewEnvValues)
}

func reviewFile(r *rand.Rand, keys []string, ext string) string {
	tree := map[string]interface{}{}
	for _, i := range r.Perm(len(keys))[:1+r.Intn(7)] {
		deepPut(tree, strings.Split(keys[i], "."), reviewFileValue(r))
	}
	if r.Intn(5) == 0 {
		// A key holding a dot, as a file can write it.
		tree[pick(r, reviewSegments)+"."+pick(r, reviewSegments)] = reviewFileValue(r)
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

// reviewRandomScenarios are seeded random combinations aimed at the environment: prefixes,
// replacers for keys with dashes and underscores, bound names, empty values, typed defaults,
// and variables that shadow nested keys.
func reviewRandomScenarios(seed int64, n int) []*Scenario {
	r := rand.New(rand.NewSource(seed))
	keys := reviewKeys()
	var out []*Scenario
	for i := 0; i < n; i++ {
		out = append(out, reviewRandomScenario(r, keys, fmt.Sprintf("review/random/%d", i)))
	}
	return out
}

func reviewRandomScenario(r *rand.Rand, keys []string, name string) *Scenario {
	s := &Scenario{Name: name, Files: map[string]string{}, Env: map[string]string{}, random: true}
	var setup []*Op
	for i := r.Intn(7); i > 0; i-- {
		setup = append(setup, setDefault(recase(r, pick(r, keys)), reviewValue(r)))
	}
	for i := r.Intn(3); i > 0; i-- {
		setup = append(setup, set(recase(r, pick(r, keys)), reviewValue(r)))
	}
	prefix := pick(r, []string{"", "P", "pbs"})
	var pairs []string
	switch r.Intn(5) {
	case 0, 1:
		pairs = []string{".", "_"}
	case 2:
		pairs = []string{".", "_", "-", "_"}
	case 3:
		pairs = []string{"-", "_"}
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
	for _, i := range r.Perm(len(keys))[:r.Intn(7)] {
		s.Env[envName(prefix, keys[i], pairs)] = pick(r, reviewEnvValues)
		if r.Intn(3) == 0 {
			// The same key as another replacer (or none) would name it.
			s.Env[envName(prefix, keys[i], nil)] = pick(r, reviewEnvValues)
		}
	}
	// Bind leaves only (three segments), so that no bound key is a prefix of another.
	leaves := keys[:0:0]
	for _, k := range keys {
		if strings.Count(k, ".") == 2 {
			leaves = append(leaves, k)
		}
	}
	for _, i := range r.Perm(len(leaves))[:r.Intn(4)] {
		key := leaves[i]
		switch r.Intn(3) {
		case 0:
			setup = append(setup, bindEnv(recase(r, key)))
			s.Env[envName(prefix, key, pairs)] = pick(r, reviewEnvValues)
		case 1:
			alt := fmt.Sprintf("ALT_%d", i)
			setup = append(setup, bindEnv(key, alt, alt+"_B"))
			if r.Intn(2) == 0 {
				s.Env[alt] = pick(r, reviewEnvValues)
			}
			s.Env[alt+"_B"] = pick(r, reviewEnvValues)
		default:
			setup = append(setup, bindEnv(key))
		}
	}
	r.Shuffle(len(setup), func(i, j int) { setup[i], setup[j] = setup[j], setup[i] })

	var config []*Op
	switch r.Intn(4) {
	case 0, 1:
		ext := pick(r, []string{"json", "yaml", "yml"})
		s.Files["pbs."+ext] = reviewFile(r, keys, ext)
		if r.Intn(3) == 0 {
			other := pick(r, []string{"json", "yaml", "yml"})
			s.Files["etc/pbs."+other] = reviewFile(r, keys, other)
		}
		config = append(config, configName("pbs"), addConfigPath("."), addConfigPath("@TMP@/etc"),
			readInConfig(), configFileUsed())
	case 2:
		ext := pick(r, []string{"json", "yaml"})
		config = append(config, configType(ext), readConfig(reviewFile(r, keys, ext)))
	}
	at := r.Intn(len(setup) + 1)
	setup = append(setup[:at], append(config, setup[at:]...)...)

	ops := append([]*Op{}, setup...)
	for _, k := range keys {
		ops = append(ops, get(k), isSet(k), inConfig(k))
	}
	for _, i := range r.Perm(len(keys))[:6] {
		for _, g := range []string{"get_string", "get_bool", "get_int", "get_uint", "get_float64", "get_duration", "get_string_slice", "get_int_slice", "get_string_map", "get_string_map_string"} {
			ops = append(ops, obs(g, keys[i]))
		}
	}
	for _, seg := range reviewSegments {
		ops = append(ops, sub(seg), unmarshalKey(seg, "map"))
	}
	// Last: Go's AllSettings can write into the maps of the sources (see pbs-goviper's "Not
	// ported" notes), which would change what later reads see.
	ops = append(ops, allKeys(), allSettings(), unmarshal("map"))
	s.Ops = ops
	return s
}
