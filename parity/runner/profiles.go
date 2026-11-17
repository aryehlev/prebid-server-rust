package runner

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"

	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// DefaultProfile is the base profile every run uses unless told otherwise.
const DefaultProfile = "default"

// Work-directory layout shared by both servers of a batch.
const (
	ConfigFileName     = "pbs.yaml"
	StaticDirName      = "static"
	StoredDirName      = "stored"
	DefaultRequestFile = "default_request.json"
)

// storedKinds are the stored-data directories of the Go file fetcher.
var storedKinds = []string{"stored_requests", "stored_imps", "stored_responses", "accounts"}

// Fixed hosts the runner configures. Every outbound call goes through the
// oracle proxy, so these names are never resolved.
const (
	currencyHost = "currency.parity.test"
)

// ProfileFixture is one parity/fixtures/profiles/<name>.yaml file.
type ProfileFixture struct {
	Name        string         `yaml:"name"`
	Description string         `yaml:"description"`
	Extends     string         `yaml:"extends"`
	Config      map[string]any `yaml:"config"`
	// Path is the repo-relative file the fixture was read from.
	Path string `yaml:"-"`
}

// Profiles are the loaded profile fixtures by name.
type Profiles map[string]*ProfileFixture

// LoadProfiles reads every *.yaml file of dir. The file name (without
// extension) must equal the profile's name. rel turns a path into the
// repo-relative form recorded as the profile's fixture path.
func LoadProfiles(dir string, rel func(string) string) (Profiles, error) {
	paths, err := filepath.Glob(filepath.Join(dir, "*.yaml"))
	if err != nil {
		return nil, fmt.Errorf("runner: listing profiles in %s: %w", dir, err)
	}
	if len(paths) == 0 {
		return nil, fmt.Errorf("runner: no profile fixtures in %s", dir)
	}
	out := Profiles{}
	for _, p := range paths {
		f, err := loadProfile(p)
		if err != nil {
			return nil, err
		}
		f.Path = rel(p)
		out[f.Name] = f
	}
	return out, nil
}

func loadProfile(path string) (*ProfileFixture, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("runner: reading profile: %w", err)
	}
	var f ProfileFixture
	if err := yaml.Unmarshal(raw, &f); err != nil {
		return nil, fmt.Errorf("runner: decoding profile %s: %w", path, err)
	}
	want := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	if f.Name != want {
		return nil, fmt.Errorf("runner: profile %s is named %q, want %q", path, f.Name, want)
	}
	if f.Config == nil {
		f.Config = map[string]any{}
	}
	return &f, nil
}

// Resolve merges the extends chain of name, base first, into one nested
// configuration map.
func (p Profiles) Resolve(name string) (map[string]any, error) {
	var chain []*ProfileFixture
	seen := map[string]bool{}
	for n := name; n != ""; {
		f, ok := p[n]
		if !ok {
			return nil, fmt.Errorf("runner: unknown profile %q", n)
		}
		if seen[n] {
			return nil, fmt.Errorf("runner: profile %q extends itself", name)
		}
		seen[n] = true
		chain = append(chain, f)
		n = f.Extends
	}
	out := map[string]any{}
	for i := len(chain) - 1; i >= 0; i-- {
		mergeInto(out, chain[i].Config)
	}
	return out, nil
}

// mergeInto deep-merges src into dst: nested maps merge, everything else
// replaces.
func mergeInto(dst, src map[string]any) {
	for k, v := range src {
		sv, ok := asMap(v)
		if !ok {
			dst[k] = cloneValue(v)
			continue
		}
		dv, ok := asMap(dst[k])
		if !ok {
			dv = map[string]any{}
			dst[k] = dv
		}
		mergeInto(dv, sv)
	}
}

// asMap returns v as a string-keyed map (YAML decodes nested mappings to
// map[string]any).
func asMap(v any) (map[string]any, bool) {
	m, ok := v.(map[string]any)
	return m, ok
}

func cloneValue(v any) any {
	switch t := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(t))
		mergeInto(out, t)
		return out
	case []any:
		out := make([]any, len(t))
		for i, e := range t {
			out[i] = cloneValue(e)
		}
		return out
	}
	return v
}

// setPath sets a dotted config key in a nested map, creating levels.
func setPath(m map[string]any, key string, v any) error {
	parts := strings.Split(key, ".")
	cur := m
	for i, part := range parts[:len(parts)-1] {
		next, ok := asMap(cur[part])
		if !ok {
			if _, exists := cur[part]; exists {
				return fmt.Errorf("runner: config key %s: %s is not a map", key, strings.Join(parts[:i+1], "."))
			}
			next = map[string]any{}
			cur[part] = next
		}
		cur = next
	}
	cur[parts[len(parts)-1]] = v
	return nil
}

// HostConfig is what the runner owns in pbs.yaml.
type HostConfig struct {
	// OracleURL is parity.oracle_url.
	OracleURL string
	// CurrencyPath is the oracle path of the profile's currency file.
	CurrencyPath string
	// DefaultRequest is true when the case profile carries a default request.
	DefaultRequest bool
}

// ownedKeys are the dotted keys BuildConfig sets after the profile; a
// profile or case cannot override them.
func ownedKeys(h HostConfig) map[string]any {
	stored := "./" + StoredDirName
	owned := map[string]any{
		"host":                                      "127.0.0.1",
		"parity.enabled":                            true,
		"parity.oracle_url":                         h.OracleURL,
		"currency_converter.fetch_url":              (&url.URL{Scheme: "http", Host: currencyHost, Path: h.CurrencyPath}).String(),
		"stored_requests.filesystem.enabled":        true,
		"stored_requests.filesystem.directorypath":  stored,
		"stored_video_req.filesystem.enabled":       true,
		"stored_video_req.filesystem.directorypath": stored,
		"stored_responses.filesystem.enabled":       true,
		"stored_responses.filesystem.directorypath": stored,
		"accounts.filesystem.enabled":               true,
		"accounts.filesystem.directorypath":         stored,
	}
	if h.DefaultRequest {
		owned["default_request.type"] = "file"
		owned["default_request.file.name"] = "./" + DefaultRequestFile
	}
	return owned
}

// BuildConfig returns pbs.yaml for one batch: the resolved base profile, the
// case profile's overrides (corpus.Profile.Overrides) and then the keys the
// runner owns.
func BuildConfig(base map[string]any, caseProfile corpus.Profile, h HostConfig) ([]byte, error) {
	cfg := map[string]any{}
	mergeInto(cfg, base)
	overrides, err := caseProfile.Overrides()
	if err != nil {
		return nil, err
	}
	for _, layer := range []map[string]any{overrides, ownedKeys(h)} {
		for _, k := range sortedKeys(layer) {
			if err := setPath(cfg, k, layer[k]); err != nil {
				return nil, err
			}
		}
	}
	out, err := yaml.Marshal(yamlValue(cfg))
	if err != nil {
		return nil, fmt.Errorf("runner: encoding %s: %w", ConfigFileName, err)
	}
	return out, nil
}

// yamlValue converts json.Number (from corpus overrides) to int64 or float64
// so YAML carries numbers, not quoted strings.
func yamlValue(v any) any {
	switch t := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, e := range t {
			out[k] = yamlValue(e)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, e := range t {
			out[i] = yamlValue(e)
		}
		return out
	case []string:
		out := make([]any, len(t))
		for i, e := range t {
			out[i] = e
		}
		return out
	case json.Number:
		if i, err := t.Int64(); err == nil {
			return i
		}
		if f, err := t.Float64(); err == nil {
			return f
		}
		return t.String()
	}
	return v
}

// Workdir is the prepared working directory of one batch. Both servers of
// every shard of the batch run in it.
type Workdir struct {
	Dir string
	// Notes describe stored data that could not be written.
	Notes []string
}

// WorkdirSpec describes a batch working directory.
type WorkdirSpec struct {
	Dir       string
	StaticDir string
	// StoredFixtures is parity/fixtures/stored (optional).
	StoredFixtures string
	Config         []byte
	Stored         corpus.Stored
	DefaultRequest json.RawMessage
}

// PrepareWorkdir creates the directory with pbs.yaml, the static symlink, the
// stored-data tree and the default request file.
func PrepareWorkdir(spec WorkdirSpec) (*Workdir, error) {
	w := &Workdir{Dir: spec.Dir}
	for _, kind := range storedKinds {
		if err := os.MkdirAll(filepath.Join(spec.Dir, StoredDirName, kind), 0o755); err != nil {
			return nil, fmt.Errorf("runner: creating stored dirs: %w", err)
		}
	}
	if err := os.WriteFile(filepath.Join(spec.Dir, ConfigFileName), spec.Config, 0o644); err != nil {
		return nil, fmt.Errorf("runner: writing %s: %w", ConfigFileName, err)
	}
	link := filepath.Join(spec.Dir, StaticDirName)
	if err := os.Symlink(spec.StaticDir, link); err != nil && !errors.Is(err, os.ErrExist) {
		return nil, fmt.Errorf("runner: linking static: %w", err)
	}
	if spec.StoredFixtures != "" {
		if err := copyStoredFixtures(spec.StoredFixtures, filepath.Join(spec.Dir, StoredDirName)); err != nil {
			return nil, err
		}
	}
	w.Notes = append(w.Notes, writeStored(filepath.Join(spec.Dir, StoredDirName), spec.Stored)...)
	if len(spec.DefaultRequest) > 0 {
		if err := os.WriteFile(filepath.Join(spec.Dir, DefaultRequestFile), spec.DefaultRequest, 0o644); err != nil {
			return nil, fmt.Errorf("runner: writing the default request: %w", err)
		}
	}
	return w, nil
}

// copyStoredFixtures copies the <kind>/<id>.json files of src into dst.
func copyStoredFixtures(src, dst string) error {
	for _, kind := range storedKinds {
		entries, err := os.ReadDir(filepath.Join(src, kind))
		if errors.Is(err, os.ErrNotExist) {
			continue
		}
		if err != nil {
			return fmt.Errorf("runner: reading stored fixtures: %w", err)
		}
		for _, e := range entries {
			if e.IsDir() || filepath.Ext(e.Name()) != ".json" {
				continue
			}
			raw, err := os.ReadFile(filepath.Join(src, kind, e.Name()))
			if err != nil {
				return fmt.Errorf("runner: reading stored fixture: %w", err)
			}
			if err := os.WriteFile(filepath.Join(dst, kind, e.Name()), raw, 0o644); err != nil {
				return fmt.Errorf("runner: writing stored fixture: %w", err)
			}
		}
	}
	return nil
}

// writeStored writes a case's stored data; IDs that are not plain file names
// are reported and skipped.
func writeStored(dir string, s corpus.Stored) []string {
	var notes []string
	sets := []struct {
		kind string
		data map[string]json.RawMessage
	}{{"stored_requests", s.Requests}, {"stored_imps", s.Imps}, {"stored_responses", s.Responses}, {"accounts", s.Accounts}}
	for _, set := range sets {
		for _, id := range sortedKeys(set.data) {
			if !plainFileName(id) {
				notes = append(notes, fmt.Sprintf("stored %s %q is not a plain file name; not written", set.kind, id))
				continue
			}
			if err := os.WriteFile(filepath.Join(dir, set.kind, id+".json"), set.data[id], 0o644); err != nil {
				notes = append(notes, fmt.Sprintf("stored %s %q: %v", set.kind, id, err))
			}
		}
	}
	return notes
}

func plainFileName(id string) bool {
	return id != "" && id != "." && id != ".." && !strings.ContainsAny(id, `/\`+"\x00")
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
