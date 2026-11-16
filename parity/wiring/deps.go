// Package wiring connects the parity harness to the Go server's bootstrap. Every
// entry point is a no-op unless parity.enabled is true, so the hook lines in
// main.go and router/router.go are behaviour-neutral in production.
package wiring

import (
	"net/http"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/logger"
	metricsConf "github.com/prebid/prebid-server/v4/metrics/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prometheus/client_golang/prometheus"
)

// Deps are the server internals the /__parity/* routes need.
type Deps struct {
	// Cfg is the resolved host configuration.
	Cfg *config.Configuration
	// Gatherer is the Prometheus registry, or nil when metrics.prometheus.port is 0.
	Gatherer prometheus.Gatherer
	// BidderInfos are the resolved bidder infos (config overrides and aliases applied).
	BidderInfos config.BidderInfos
}

// NewDeps collects Deps from the router's collaborators.
func NewDeps(cfg *config.Configuration, me *metricsConf.DetailedMetricsEngine) Deps {
	deps := Deps{Cfg: cfg}
	if cfg != nil {
		deps.BidderInfos = cfg.BidderInfos
	}
	if me != nil && me.PrometheusMetrics != nil && me.PrometheusMetrics.Gatherer != nil {
		deps.Gatherer = me.PrometheusMetrics.Gatherer
	}
	return deps
}

// Bootstrap prepares process-wide parity state before the router is built: it
// logs a warning banner, installs the deterministic adapter fakes and routes
// http.DefaultTransport (used by http.DefaultClient, for example by analytics
// modules) through the parity transport so nothing reaches the real network.
// It is a no-op unless parity is enabled.
func Bootstrap(cfg *config.Configuration) {
	if cfg == nil || !cfg.Parity.Enabled {
		return
	}
	logger.Warnf("[parity] PARITY MODE ENABLED: /__parity/* is served on the main port, outbound HTTP goes to %q and adapters use deterministic fakes. Never deploy this configuration.", cfg.Parity.OracleURL)
	fakes.Install()
	http.DefaultTransport = transport.Wrap(http.DefaultTransport, parity.OutboundDefault, cfg.Parity)
}
