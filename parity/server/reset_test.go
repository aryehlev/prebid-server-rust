package server

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// alwaysStale is the state reset never clears.
var alwaysStale = []string{"gdpr.vendor_lists", "currency.rates"}

func notResetNames(rows []parity.NotResetRow) []string {
	names := make([]string, 0, len(rows))
	for _, row := range rows {
		names = append(names, row.Name)
	}
	return names
}

func TestReset(t *testing.T) {
	cached := func() *config.Configuration {
		cfg := &config.Configuration{}
		cfg.StoredRequests.InMemoryCache.Type = "lru"
		cfg.StoredRequestsAMP.InMemoryCache.Type = "none"
		cfg.CategoryMapping.InMemoryCache.Type = "unbounded"
		cfg.Accounts.V2Enabled = true
		cfg.Accounts.CacheV2.Type = "lru"
		cfg.PriceFloors.Enabled = true
		cfg.Client.Throttle.EnableThrottling = true
		return cfg
	}
	tests := []struct {
		name         string
		cfg          *config.Configuration
		wantReset    []string
		wantNotReset []string
	}{
		{
			name:      "no config",
			wantReset: []string{"parity.fakes"},
		},
		{
			name: "nothing cached",
			cfg:  &config.Configuration{},
			wantReset: []string{
				"parity.fakes",
				"stored_requests.in_memory_cache", "stored_amp_req.in_memory_cache", "categories.in_memory_cache",
				"stored_video_req.in_memory_cache", "accounts.in_memory_cache", "stored_responses.in_memory_cache",
				"price_floors.fetcher.cache", "bidder.health",
			},
			wantNotReset: alwaysStale,
		},
		{
			name: "caches enabled",
			cfg:  cached(),
			wantReset: []string{
				"parity.fakes", "stored_amp_req.in_memory_cache", "stored_video_req.in_memory_cache", "stored_responses.in_memory_cache",
			},
			wantNotReset: append([]string{
				"stored_requests.in_memory_cache", "categories.in_memory_cache", "accounts.cache",
				"price_floors.fetcher.cache", "bidder.health",
			}, alwaysStale...),
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			fakes.Active().Set(&parity.FakeValues{NowUnixMS: 42})
			h := newServer(wiring.Deps{Cfg: tt.cfg}, http.NotFoundHandler())
			var got parity.ResetResult
			decodeDoc(t, call(t, h, http.MethodPost, PathReset, nil), http.StatusOK, &got)
			assert.Equal(t, parity.SchemaReset, got.Schema)
			assert.Equal(t, parity.ImplGo, got.Impl)
			assert.Equal(t, tt.wantReset, got.Reset)
			want := tt.wantNotReset
			if want == nil {
				want = []string{}
			}
			assert.Equal(t, want, notResetNames(got.NotReset))
			for _, row := range got.NotReset {
				assert.NotEmpty(t, row.Reason, row.Name)
			}
			assert.Equal(t, fakes.Defaults(), fakes.Active().Values(), "reset restores the default fakes")
		})
	}
}

func TestResetResolvedDefaults(t *testing.T) {
	h := newServer(wiring.Deps{Cfg: resolvedConfig(t, nil)}, http.NotFoundHandler())
	var got parity.ResetResult
	decodeDoc(t, call(t, h, http.MethodPost, PathReset, nil), http.StatusOK, &got)
	assert.Contains(t, got.Reset, "stored_requests.in_memory_cache", "the default in-memory cache type is none")
	assert.Equal(t, alwaysStale, notResetNames(got.NotReset), "a default host only keeps process-lifetime caches")
}

func TestResetWaitsForTheInFlightCase(t *testing.T) {
	_, end := trace.Begin(&parity.Envelope{CaseID: "in flight"})
	h := newServer(wiring.Deps{}, http.NotFoundHandler())

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	req := httptest.NewRequest(http.MethodPost, PathReset, nil).WithContext(ctx)
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	assertError(t, rec, http.StatusServiceUnavailable, "exec lock")

	end()
	rec = call(t, h, http.MethodPost, PathReset, nil)
	require.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
}
