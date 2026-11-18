package server

import (
	"errors"
	"fmt"
	"net/http"
	"sort"

	"github.com/mitchellh/mapstructure"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Config subtrees left out of GET /__parity/config: adapters is compared
// through GET /__parity/bidders, and parity holds the harness token.
const (
	configKeyAdapters = "adapters"
	configKeyParity   = "parity"
)

var errNoConfig = errors.New("parity: the server has no resolved configuration")

// health serves GET /__parity/health. Go is the source of truth, so it supports
// every registered stage and decision point.
func (s *Server) health(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, parity.Health{
		Schema:             parity.SchemaHealth,
		SchemaVersion:      parity.SchemaVersion,
		Impl:               parity.ImplGo,
		Build:              buildInfo(),
		StagesSupported:    parity.AllStages(),
		DecisionsSupported: parity.AllDecisions(),
		Endpoints:          productEndpoints(s.deps.Cfg),
		MockModes:          []string{parity.MockModeOracle, parity.MockModeInline},
		MockLevels:         []string{parity.MockLevelHTTP, parity.MockLevelBidderResp},
		Fakes:              fakes.Active() != nil,
	})
}

// productEndpoints lists every route of the main router, which exec
// dispatches to, as router/router.go registers them (httprouter patterns,
// registration order): the stored data cache event routes
// (stored_requests/config/config.go newEventsAPI) and then the product
// routes. The runner compares a request that no route matches only when
// both sides list the same routes, since the router's own answer (a 404, a
// 405 or a redirect) depends on all of them.
func productEndpoints(cfg *config.Configuration) []string {
	var endpoints []string
	if cfg != nil {
		sections := []*config.StoredRequests{&cfg.StoredRequests, &cfg.StoredRequestsAMP, &cfg.CategoryMapping, &cfg.StoredVideo}
		if !cfg.Accounts.V2Enabled {
			sections = append(sections, &cfg.Accounts)
		}
		for _, sr := range append(sections, &cfg.StoredResponses) {
			if sr.CacheEvents.Enabled && !contains(endpoints, sr.CacheEvents.Endpoint) {
				endpoints = append(endpoints, sr.CacheEvents.Endpoint)
			}
		}
	}
	endpoints = append(endpoints,
		EndpointAuction, EndpointVideo, EndpointAMP,
		"/info/bidders", "/info/bidders/:bidderName", "/bidders/params",
		"/cookie_sync", "/status", "/", "/version", "/static/*filepath",
	)
	if cfg != nil && cfg.VTrack.Enabled {
		endpoints = append(endpoints, "/vtrack")
	}
	return append(endpoints, "/event", "/setuid", "/getuids", "/optout")
}

// contains reports whether list holds s.
func contains(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}

// dumpConfig serves GET /__parity/config: the resolved configuration as
// mapstructure.Decode dumps it (keys are the config keys; nested structs become
// objects, other values keep their JSON encoding), without the adapters and
// parity subtrees. Secrets other than the parity token stay in the dump: config
// parity is compared strictly, and the route is guarded by the token and the
// loopback check.
func (s *Server) dumpConfig(w http.ResponseWriter, _ *http.Request) {
	dump, err := configDump(s.deps.Cfg)
	if err != nil {
		writeFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, parity.ConfigDump{
		Schema:        parity.SchemaConfigDump,
		SchemaVersion: parity.SchemaVersion,
		Impl:          parity.ImplGo,
		Config:        dump,
	})
}

func configDump(cfg *config.Configuration) ([]byte, error) {
	if cfg == nil {
		return nil, errNoConfig
	}
	out := map[string]any{}
	if err := mapstructure.Decode(cfg, &out); err != nil {
		return nil, fmt.Errorf("parity: dumping config: %w", err)
	}
	delete(out, configKeyAdapters)
	delete(out, configKeyParity)
	raw, err := jsonutil.Marshal(out)
	if err != nil {
		return nil, fmt.Errorf("parity: encoding config: %w", err)
	}
	return raw, nil
}

// dumpBidders serves GET /__parity/bidders: every resolved bidder info (config
// overrides and aliases applied) dumped like the config, the sorted names of
// the active bidders, and the disabled-bidder warning messages.
func (s *Server) dumpBidders(w http.ResponseWriter, _ *http.Request) {
	dump, err := biddersDump(s.deps.BidderInfos)
	if err != nil {
		writeFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, parity.BiddersDump{
		Schema:        parity.SchemaBiddersDump,
		SchemaVersion: parity.SchemaVersion,
		Impl:          parity.ImplGo,
		Bidders:       dump,
		Active:        activeBidders(s.deps.BidderInfos),
		Disabled:      exchange.GetDisabledBidderWarningMessages(s.deps.BidderInfos),
	})
}

func biddersDump(infos config.BidderInfos) ([]byte, error) {
	out := make(map[string]map[string]any, len(infos))
	for name, info := range infos {
		entry := map[string]any{}
		if err := mapstructure.Decode(info, &entry); err != nil {
			return nil, fmt.Errorf("parity: dumping bidder %s: %w", name, err)
		}
		out[name] = entry
	}
	raw, err := jsonutil.Marshal(out)
	if err != nil {
		return nil, fmt.Errorf("parity: encoding bidders: %w", err)
	}
	return raw, nil
}

func activeBidders(infos config.BidderInfos) []string {
	active := exchange.GetActiveBidders(infos)
	names := make([]string, 0, len(active))
	for name := range active {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}
