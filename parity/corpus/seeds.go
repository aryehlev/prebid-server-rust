package corpus

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io/fs"
	"path"
)

// Seed domains: domain-targeted inputs for the request generator (PH-12).
// Seeds are fragments, not runnable cases; their Go tests drive internal
// functions with no HTTP equivalent.
const (
	SeedCache             = "cache"
	SeedCustomCacheKey    = "custom_cache_key"
	SeedEventsCache       = "events_cache"
	SeedImpCustomCacheKey = "imp_custom_cache_key"
	SeedFPD               = "first_party_data"
	SeedHookOutcomes      = "hook_outcomes"
)

// seedSources maps each seed root to its domain and the member holding the
// request fragment ("" when the files only hold expectations).
var seedSources = []struct {
	root, domain, requestKey string
}{
	{RootCacheTest, SeedCache, "bidRequest"},
	{RootCustomCacheKey, SeedCustomCacheKey, "bidRequest"},
	{RootEventsCacheTest, SeedEventsCache, "bidRequest"},
	{RootImpCustomCacheKey, SeedImpCustomCacheKey, "bidRequest"},
	{RootFPDTests, SeedFPD, "inputRequestData"},
	{RootHookExecTest, SeedHookOutcomes, ""},
}

// Seed is one domain-targeted test file.
type Seed struct {
	Domain      string `json:"domain"`
	Path        string `json:"path"`
	Description string `json:"description,omitempty"`
	// Request is the request fragment of the file, nil for expectation-only
	// files (hook outcomes).
	Request json.RawMessage `json:"request,omitempty"`
	// Raw is the whole file.
	Raw json.RawMessage `json:"-"`
}

// LoadSeeds loads every JSON file below the seed roots.
func LoadSeeds(repo *Repo) ([]Seed, []error) {
	var out []Seed
	var errs []error
	for _, src := range seedSources {
		err := repo.Walk(src.root, func(rel string, _ fs.DirEntry) error {
			if path.Ext(rel) != ".json" {
				return nil
			}
			seed, err := loadSeed(repo, rel, src.domain, src.requestKey)
			if err != nil {
				errs = append(errs, err)
				return nil
			}
			out = append(out, seed)
			return nil
		})
		if err != nil {
			errs = append(errs, err)
		}
	}
	return out, errs
}

func loadSeed(repo *Repo, rel, domain, requestKey string) (Seed, error) {
	data, err := repo.ReadFile(rel)
	if err != nil {
		return Seed{}, err
	}
	seed := Seed{Domain: domain, Path: rel, Raw: data}
	if requestKey == "" {
		// Expectation-only files may be arrays or even empty (hook outcomes).
		if len(bytes.TrimSpace(data)) > 0 && !json.Valid(data) {
			return Seed{}, fmt.Errorf("corpus: %s: invalid JSON", rel)
		}
		return seed, nil
	}
	var members map[string]json.RawMessage
	if err := json.Unmarshal(data, &members); err != nil {
		return Seed{}, fmt.Errorf("corpus: %s: %w", rel, err)
	}
	if raw, ok := members["description"]; ok {
		if err := json.Unmarshal(raw, &seed.Description); err != nil {
			return Seed{}, fmt.Errorf("corpus: %s: description: %w", rel, err)
		}
	}
	seed.Request = nonNull(members[requestKey])
	if seed.Request == nil {
		return Seed{}, fmt.Errorf("corpus: %s: missing %s", rel, requestKey)
	}
	return seed, nil
}
