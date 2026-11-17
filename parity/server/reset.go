package server

import (
	"net/http"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
)

// stateItem is one piece of in-process state that can make a case depend on
// the cases before it (design risk R5).
type stateItem struct {
	name string
	// reset clears the state when it can. clean reports whether the state holds
	// nothing from earlier cases afterwards; reason says why not.
	reset func() (clean bool, reason string)
}

// reset serves POST /__parity/reset. It waits for the in-flight case (it takes
// the exec lock), clears what it can reach and reports the rest. reset names
// the state that is clean afterwards, either cleared now or empty because the
// configuration disables it; not_reset names the state that may still carry
// data from earlier cases. The order of both lists is fixed.
func (s *Server) reset(w http.ResponseWriter, r *http.Request) {
	_, end, err := trace.BeginContext(r.Context(), &parity.Envelope{})
	if err != nil {
		writeFailure(w, &errDocument{status: http.StatusServiceUnavailable, msg: err.Error()})
		return
	}
	result := resetState(stateItems(s.deps.Cfg))
	end()
	writeJSON(w, http.StatusOK, result)
}

func resetState(items []stateItem) parity.ResetResult {
	out := parity.ResetResult{
		Schema:        parity.SchemaReset,
		SchemaVersion: parity.SchemaVersion,
		Impl:          parity.ImplGo,
		Reset:         []string{},
		NotReset:      []parity.NotResetRow{},
	}
	for _, item := range items {
		if clean, reason := item.reset(); clean {
			out.Reset = append(out.Reset, item.name)
		} else {
			out.NotReset = append(out.NotReset, parity.NotResetRow{Name: item.name, Reason: reason})
		}
	}
	return out
}

// stateItems lists the cross-case state of this server. The parity routes can
// only reach the deterministic fakes; caches built inside the router are
// reported from the configuration, which tells whether they exist at all.
func stateItems(cfg *config.Configuration) []stateItem {
	items := []stateItem{{name: "parity.fakes", reset: resetFakes}}
	if cfg == nil {
		return items
	}
	for _, src := range storedSources(cfg) {
		items = append(items, src.cacheItem())
	}
	return append(items,
		configItem("price_floors.fetcher.cache", !cfg.PriceFloors.Enabled,
			"the floors fetcher keeps fetched floor data for its max age; set price_floors.enabled to false in parity profiles or restart the server"),
		configItem("bidder.health", !cfg.Client.Throttle.EnableThrottling,
			"bidder health scores live inside the exchange's bidder adapters; set http_client.throttle.enable_throttling to false in parity profiles"),
		configItem("gdpr.vendor_lists", false,
			"fetched GVL vendor lists stay cached for the process lifetime; restart the server for a clean cache"),
		configItem("currency.rates", false,
			"refreshed only by the currency converter ticker task; restart the server for a clean rate table"),
	)
}

// resetFakes restores the fakes.json defaults (nothing to do when the fakes are
// not installed).
func resetFakes() (bool, string) {
	if src := fakes.Active(); src != nil {
		src.Set(nil)
	}
	return true, ""
}

func configItem(name string, clean bool, reason string) stateItem {
	return stateItem{name: name, reset: func() (bool, string) { return clean, reason }}
}

// storedSource is one stored-data fetcher router.go builds. v2 marks the
// accounts source on the Fetchers 2.0 path (accounts.v2_enabled).
type storedSource struct {
	key string
	cfg config.StoredRequests
	v2  bool
}

// storedSources lists the stored-data fetchers by config key, in the order
// stored_requests/config NewStoredRequests builds them.
func storedSources(cfg *config.Configuration) []storedSource {
	return []storedSource{
		{key: "stored_requests", cfg: cfg.StoredRequests},
		{key: "stored_amp_req", cfg: cfg.StoredRequestsAMP},
		{key: "categories", cfg: cfg.CategoryMapping},
		{key: "stored_video_req", cfg: cfg.StoredVideo},
		{key: "accounts", cfg: cfg.Accounts, v2: cfg.Accounts.V2Enabled},
		{key: "stored_responses", cfg: cfg.StoredResponses},
	}
}

// cacheItem reports the source's cache. The legacy in-memory cache exists
// unless in_memory_cache.type is empty or none; the Fetchers 2.0 account cache
// exists unless cache.type is none.
func (src storedSource) cacheItem() stateItem {
	if src.v2 {
		return configItem(src.key+".cache", src.cfg.CacheV2.Type == "none",
			"the Fetchers 2.0 account cache is internal to the account fetcher; set "+src.key+".cache.type to none in parity profiles or restart the server")
	}
	t := src.cfg.InMemoryCache.Type
	return configItem(src.key+".in_memory_cache", t == "" || t == "none",
		"the in-memory cache is internal to the stored-data fetcher; set "+src.key+".in_memory_cache.type to none in parity profiles or restart the server")
}
