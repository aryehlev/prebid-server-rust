package server

import (
	"encoding/json"
	"net/http"
	"sort"
	"testing"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	"github.com/prebid/prebid-server/v4/version"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestHealth(t *testing.T) {
	tests := []struct {
		name      string
		cfg       *config.Configuration
		wantTrack bool
	}{
		{name: "no config", cfg: nil},
		{name: "vtrack disabled", cfg: &config.Configuration{}},
		{name: "vtrack enabled", cfg: &config.Configuration{VTrack: config.VTrack{Enabled: true}}, wantTrack: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			h := newServer(wiring.Deps{Cfg: tt.cfg}, http.NotFoundHandler())
			var got parity.Health
			decodeDoc(t, call(t, h, http.MethodGet, PathHealth, nil), http.StatusOK, &got)
			assert.Equal(t, parity.SchemaHealth, got.Schema)
			assert.Equal(t, parity.SchemaVersion, got.SchemaVersion)
			assert.Equal(t, parity.ImplGo, got.Impl)
			assert.Equal(t, version.Rev, got.Build.GitSHA)
			assert.NotEmpty(t, got.Build.Version)
			assert.Equal(t, parity.AllStages(), got.StagesSupported)
			assert.Equal(t, parity.AllDecisions(), got.DecisionsSupported)
			assert.Equal(t, []string{parity.MockModeOracle, parity.MockModeInline}, got.MockModes)
			assert.Equal(t, []string{parity.MockLevelHTTP, parity.MockLevelBidderResp}, got.MockLevels)
			assert.True(t, got.Fakes, "TestMain installs the fakes")
			assert.Subset(t, got.Endpoints, []string{EndpointAuction, EndpointAMP, EndpointVideo, "/cookie_sync", "/setuid"})
			assert.Equal(t, tt.wantTrack, contains(got.Endpoints, "/vtrack"))
		})
	}
}

// TestHealthListsCacheEventRoutes: with cache_events on, the stored data
// event routes the main router registers are listed first.
func TestHealthListsCacheEventRoutes(t *testing.T) {
	cfg := &config.Configuration{}
	cfg.StoredRequests.CacheEvents = config.CacheEventsConfig{Enabled: true, Endpoint: "/storedrequests/openrtb2"}
	cfg.StoredRequestsAMP.CacheEvents = config.CacheEventsConfig{Enabled: true, Endpoint: "/storedrequests/amp"}
	cfg.Accounts.CacheEvents = config.CacheEventsConfig{Enabled: true, Endpoint: "/storedrequests/amp"}
	got := productEndpoints(cfg)
	assert.Equal(t, []string{"/storedrequests/openrtb2", "/storedrequests/amp", EndpointAuction}, got[:3])
	assert.Equal(t, productEndpoints(&config.Configuration{}), got[2:])
}

// dumpLookup walks a decoded dump by keys.
func dumpLookup(t *testing.T, doc map[string]any, keys ...string) (any, bool) {
	t.Helper()
	var cur any = doc
	for _, k := range keys {
		obj, ok := cur.(map[string]any)
		if !ok {
			return nil, false
		}
		if cur, ok = obj[k]; !ok {
			return nil, false
		}
	}
	return cur, true
}

func TestConfigDump(t *testing.T) {
	cfg := resolvedConfig(t, map[string]any{
		"host":             "127.0.0.1",
		"tmax_default":     1500,
		"cache.scheme":     "https",
		"recaptcha_secret": "host-secret",
		"parity.enabled":   true,
		"parity.token":     testToken,
	})
	h := newServer(wiring.Deps{Cfg: cfg}, http.NotFoundHandler())
	rec := call(t, h, http.MethodGet, PathConfig, nil)
	var got parity.ConfigDump
	decodeDoc(t, rec, http.StatusOK, &got)
	assert.Equal(t, parity.SchemaConfigDump, got.Schema)
	assert.Equal(t, parity.ImplGo, got.Impl)
	assert.NotContains(t, rec.Body.String(), testToken, "the parity token never leaves the server")

	var doc map[string]any
	require.NoError(t, json.Unmarshal(got.Config, &doc))
	tests := []struct {
		keys []string
		want any
	}{
		{keys: []string{"tmax_default"}, want: float64(1500)},
		{keys: []string{"gdpr", "default_value"}, want: testGDPRDefault},
		{keys: []string{"cache", "scheme"}, want: "https"},
		{keys: []string{"host"}, want: "127.0.0.1"},
		{keys: []string{"port"}, want: float64(8000)},
		{keys: []string{"http_client", "throttle", "enable_throttling"}, want: false},
		{keys: []string{"stored_requests", "in_memory_cache", "type"}, want: "none"},
		{keys: []string{"recaptcha_secret"}, want: "host-secret"},
	}
	for _, tt := range tests {
		v, ok := dumpLookup(t, doc, tt.keys...)
		if assert.True(t, ok, "config dump has %v", tt.keys) {
			assert.Equal(t, tt.want, v, "%v", tt.keys)
		}
	}
	for _, key := range []string{configKeyAdapters, configKeyParity} {
		assert.NotContains(t, doc, key)
	}
}

func TestConfigDumpWithoutConfig(t *testing.T) {
	h := newServer(wiring.Deps{}, http.NotFoundHandler())
	assertError(t, call(t, h, http.MethodGet, PathConfig, nil), http.StatusInternalServerError, "no resolved configuration")
}

func TestBiddersDump(t *testing.T) {
	infos := config.BidderInfos{
		"appnexus": {Endpoint: "http://ib.adnxs.com/openrtb2", GVLVendorID: 32, Maintainer: &config.MaintainerInfo{Email: "a@example.com"}, PlatformID: "5"},
		"zeta":     {Endpoint: "http://zeta.example", Syncer: &config.Syncer{Key: "z"}},
		"old":      {Disabled: true},
		"alias":    {AliasOf: "appnexus", Endpoint: "http://alias.example"},
	}
	h := newServer(wiring.Deps{BidderInfos: infos}, http.NotFoundHandler())
	var got parity.BiddersDump
	decodeDoc(t, call(t, h, http.MethodGet, PathBidders, nil), http.StatusOK, &got)
	assert.Equal(t, parity.SchemaBiddersDump, got.Schema)
	assert.Equal(t, []string{"alias", "appnexus", "zeta"}, got.Active, "active bidders are sorted")
	assert.Contains(t, got.Disabled, "old")
	assert.Contains(t, got.Disabled, "lifestreet", "removed bidders keep their warning")

	var doc map[string]any
	require.NoError(t, json.Unmarshal(got.Bidders, &doc))
	names := make([]string, 0, len(doc))
	for name := range doc {
		names = append(names, name)
	}
	sort.Strings(names)
	assert.Equal(t, []string{"alias", "appnexus", "old", "zeta"}, names)
	tests := []struct {
		keys []string
		want any
	}{
		{keys: []string{"appnexus", "endpoint"}, want: "http://ib.adnxs.com/openrtb2"},
		{keys: []string{"appnexus", "gvlVendorID"}, want: float64(32)},
		{keys: []string{"appnexus", "maintainer", "email"}, want: "a@example.com"},
		{keys: []string{"appnexus", "platform_id"}, want: "5"},
		{keys: []string{"appnexus", "capabilities"}, want: nil},
		{keys: []string{"alias", "aliasOf"}, want: "appnexus"},
		{keys: []string{"old", "disabled"}, want: true},
		{keys: []string{"zeta", "experiment", "adsCert", "enabled"}, want: false},
	}
	for _, tt := range tests {
		v, ok := dumpLookup(t, doc, tt.keys...)
		if assert.True(t, ok, "bidders dump has %v", tt.keys) {
			assert.Equal(t, tt.want, v, "%v", tt.keys)
		}
	}
	_, ok := dumpLookup(t, doc, "zeta", "userSync")
	assert.True(t, ok, "the syncer is keyed by its yaml name")
}

func TestBiddersDumpStaticInfos(t *testing.T) {
	infos := staticBidderInfos(t)
	h := newServer(wiring.Deps{BidderInfos: infos}, http.NotFoundHandler())
	var got parity.BiddersDump
	decodeDoc(t, call(t, h, http.MethodGet, PathBidders, nil), http.StatusOK, &got)
	var doc map[string]json.RawMessage
	require.NoError(t, json.Unmarshal(got.Bidders, &doc))
	assert.Len(t, doc, len(infos))
	assert.True(t, sort.StringsAreSorted(got.Active))
	for name, info := range infos {
		assert.Equal(t, info.IsEnabled(), contains(got.Active, name), name)
	}
}
