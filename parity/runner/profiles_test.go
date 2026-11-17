package runner

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/spf13/viper"
	"gopkg.in/yaml.v3"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

var (
	repoOnce sync.Once
	repoDir  string
	repoErr  error
)

// testRepo returns the repository root.
func testRepo(t *testing.T) string {
	t.Helper()
	repoOnce.Do(func() {
		var repo *corpus.Repo
		if repo, repoErr = corpus.FindRepo("."); repoErr == nil {
			repoDir = repo.Root()
		}
	})
	if repoErr != nil {
		t.Fatalf("FindRepo: %v", repoErr)
	}
	return repoDir
}

func loadTestProfiles(t *testing.T) Profiles {
	t.Helper()
	root := testRepo(t)
	profiles, err := LoadProfiles(filepath.Join(root, defaultProfilesDir), func(p string) string {
		rel, _ := filepath.Rel(root, p)
		return filepath.ToSlash(rel)
	})
	if err != nil {
		t.Fatalf("LoadProfiles: %v", err)
	}
	return profiles
}

func TestProfileFixtures(t *testing.T) {
	profiles := loadTestProfiles(t)
	for _, name := range []string{"default", "gdpr-enforced", "floors", "account-required", "events-cache"} {
		f, ok := profiles[name]
		if !ok {
			t.Errorf("profile %s missing", name)
			continue
		}
		if f.Description == "" || f.Path != "parity/fixtures/profiles/"+name+".yaml" {
			t.Errorf("profile %s = %+v", name, f)
		}
		if _, err := profiles.Resolve(name); err != nil {
			t.Errorf("Resolve(%s): %v", name, err)
		}
	}
}

func TestResolveExtends(t *testing.T) {
	profiles := Profiles{
		"base":  {Name: "base", Config: map[string]any{"gdpr": map[string]any{"default_value": "0", "enabled": true}, "a": 1}},
		"child": {Name: "child", Extends: "base", Config: map[string]any{"gdpr": map[string]any{"default_value": "1"}}},
		"loop":  {Name: "loop", Extends: "loop"},
	}
	got, err := profiles.Resolve("child")
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	gdpr, _ := asMap(got["gdpr"])
	if gdpr["default_value"] != "1" || gdpr["enabled"] != true || got["a"] != 1 {
		t.Errorf("resolved = %v", got)
	}
	if base, _ := asMap(profiles["base"].Config["gdpr"]); base["default_value"] != "0" {
		t.Error("Resolve mutated the base profile")
	}
	for _, bad := range []string{"loop", "missing"} {
		if _, err := profiles.Resolve(bad); err == nil {
			t.Errorf("Resolve(%s) succeeded", bad)
		}
	}
}

func TestBuildConfig(t *testing.T) {
	enabled := true
	tests := []struct {
		name  string
		base  map[string]any
		prof  corpus.Profile
		host  HostConfig
		check func(t *testing.T, cfg map[string]any)
	}{
		{
			name: "runner keys win over the profile",
			base: map[string]any{"host": "0.0.0.0", "parity": map[string]any{"enabled": false}},
			host: HostConfig{OracleURL: "http://127.0.0.1:1", CurrencyPath: "/currency/latest.json"},
			check: func(t *testing.T, cfg map[string]any) {
				if cfg["host"] != "127.0.0.1" || yamlPath(cfg, "parity", "enabled") != true || yamlPath(cfg, "parity", "oracle_url") != "http://127.0.0.1:1" {
					t.Errorf("cfg = %v", cfg)
				}
				if yamlPath(cfg, "currency_converter", "fetch_url") != "http://currency.parity.test/currency/latest.json" {
					t.Errorf("fetch_url = %v", yamlPath(cfg, "currency_converter", "fetch_url"))
				}
				if yamlPath(cfg, "accounts", "filesystem", "directorypath") != "./stored" || yamlPath(cfg, "default_request") != nil {
					t.Errorf("stored or default request = %v", cfg)
				}
			},
		},
		{
			name: "case overrides apply and numbers stay numbers",
			base: map[string]any{"gdpr": map[string]any{"default_value": "0"}},
			prof: corpus.Profile{GDPREnabled: &enabled, GDPRDefaultValue: "1", AccountDefaultBidLimit: 3, DisabledAdapters: []string{"AppNexus"},
				BidderInfoOverrides: map[string]json.RawMessage{"rubicon": json.RawMessage(`{"openrtb":{"version":"2.6","gpp-supported":true}}`)}},
			host: HostConfig{OracleURL: "http://127.0.0.1:1", DefaultRequest: true},
			check: func(t *testing.T, cfg map[string]any) {
				if yamlPath(cfg, "gdpr", "default_value") != "1" || yamlPath(cfg, "gdpr", "enabled") != true {
					t.Errorf("gdpr = %v", cfg["gdpr"])
				}
				if yamlPath(cfg, "account_defaults", "default_bid_limit") != 3 {
					t.Errorf("default_bid_limit = %#v", yamlPath(cfg, "account_defaults", "default_bid_limit"))
				}
				if yamlPath(cfg, "adapters", "appnexus", "disabled") != true || yamlPath(cfg, "adapters", "rubicon", "openrtb", "version") != "2.6" {
					t.Errorf("adapters = %v", cfg["adapters"])
				}
				if yamlPath(cfg, "default_request", "type") != "file" || yamlPath(cfg, "default_request", "file", "name") != "./default_request.json" {
					t.Errorf("default_request = %v", cfg["default_request"])
				}
			},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			raw, err := BuildConfig(tc.base, tc.prof, tc.host)
			if err != nil {
				t.Fatalf("BuildConfig: %v", err)
			}
			var cfg map[string]any
			if err := yaml.Unmarshal(raw, &cfg); err != nil {
				t.Fatalf("decoding pbs.yaml: %v\n%s", err, raw)
			}
			tc.check(t, cfg)
		})
	}
}

// path walks nested YAML maps.
func yamlPath(m map[string]any, keys ...string) any {
	var cur any = m
	for _, k := range keys {
		mm, ok := cur.(map[string]any)
		if !ok {
			return nil
		}
		cur = mm[k]
	}
	return cur
}

// TestGeneratedConfigLoadsInGo loads every profile's pbs.yaml through the Go
// server's own config path (SetupViper + config.New, with the env the
// runner sets), so a profile the Go server would reject fails here.
func TestGeneratedConfigLoadsInGo(t *testing.T) {
	root := testRepo(t)
	infos, err := config.LoadBidderInfoFromDisk(filepath.Join(root, "static", "bidder-info"))
	if err != nil {
		t.Fatalf("LoadBidderInfoFromDisk: %v", err)
	}
	profiles := loadTestProfiles(t)
	t.Setenv("PBS_PORT", "18000")
	t.Setenv("PBS_ADMIN_PORT", "18001")
	t.Setenv("PBS_METRICS_PROMETHEUS_PORT", "18002")
	t.Setenv("PBS_PARITY_TOKEN", strings.Repeat("t", 32))
	enabled := false
	cases := map[string]corpus.Profile{
		"no case settings": {},
		"case settings":    {GDPREnabled: &enabled, AccountRequired: true, FloorsEnabled: true, DisabledAdapters: []string{"appnexus"}, DefaultRequest: json.RawMessage(`{"id":"default"}`)},
	}
	for _, name := range sortedKeys(profiles) {
		for label, prof := range cases {
			t.Run(name+"/"+label, func(t *testing.T) {
				base, err := profiles.Resolve(name)
				if err != nil {
					t.Fatalf("Resolve: %v", err)
				}
				raw, err := BuildConfig(base, prof, HostConfig{OracleURL: "http://127.0.0.1:7999", CurrencyPath: "/currency/latest.json", DefaultRequest: len(prof.DefaultRequest) > 0})
				if err != nil {
					t.Fatalf("BuildConfig: %v", err)
				}
				dir := t.TempDir()
				if _, err := PrepareWorkdir(WorkdirSpec{Dir: dir, StaticDir: filepath.Join(root, "static"), Config: raw, DefaultRequest: prof.DefaultRequest}); err != nil {
					t.Fatalf("PrepareWorkdir: %v", err)
				}
				v := viper.New()
				v.SetConfigFile(filepath.Join(dir, ConfigFileName))
				config.SetupViper(v, "", infos)
				cfg, err := config.New(v, infos, openrtb_ext.NormalizeBidderName)
				if err != nil {
					t.Fatalf("config.New rejected the generated pbs.yaml: %v\n%s", err, raw)
				}
				if !cfg.Parity.Enabled || cfg.Port != 18000 || cfg.AdminPort != 18001 || cfg.Metrics.Prometheus.Port != 18002 || cfg.Host != "127.0.0.1" {
					t.Errorf("cfg parity=%v port=%d admin=%d prometheus=%d host=%q", cfg.Parity.Enabled, cfg.Port, cfg.AdminPort, cfg.Metrics.Prometheus.Port, cfg.Host)
				}
				if !cfg.StoredRequests.Files.Enabled || !cfg.Accounts.Files.Enabled {
					t.Error("filesystem stored data is not enabled")
				}
				if prof.AccountRequired && !cfg.AccountRequired {
					t.Error("case override account_required was lost")
				}
			})
		}
	}
}

func TestPrepareWorkdir(t *testing.T) {
	root := testRepo(t)
	dir := t.TempDir()
	stored := corpus.Stored{
		Requests: map[string]json.RawMessage{"req-1": json.RawMessage(`{"id":"req-1"}`), "../escape": json.RawMessage(`{}`)},
		Accounts: map[string]json.RawMessage{"acct": json.RawMessage(`{"id":"acct"}`)},
	}
	w, err := PrepareWorkdir(WorkdirSpec{
		Dir: dir, StaticDir: filepath.Join(root, "static"), StoredFixtures: filepath.Join(root, defaultStoredDir),
		Config: []byte("host: 127.0.0.1\n"), Stored: stored, DefaultRequest: json.RawMessage(`{"id":"d"}`),
	})
	if err != nil {
		t.Fatalf("PrepareWorkdir: %v", err)
	}
	for _, p := range []string{
		"pbs.yaml", "static/bidder-info", "stored/stored_requests/req-1.json", "stored/accounts/acct.json",
		"stored/accounts/parity-account.json", "stored/stored_imps/parity-stored-imp.json", "stored/stored_responses", DefaultRequestFile,
	} {
		if _, err := os.Stat(filepath.Join(dir, filepath.FromSlash(p))); err != nil {
			t.Errorf("%s: %v", p, err)
		}
	}
	if len(w.Notes) != 1 || !strings.Contains(w.Notes[0], "../escape") {
		t.Errorf("notes = %v", w.Notes)
	}
	if _, err := os.Stat(filepath.Join(dir, "escape.json")); err == nil {
		t.Error("an unsafe stored ID escaped the stored tree")
	}
}
