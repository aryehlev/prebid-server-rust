package transport

import (
	"net/http"
	"sync/atomic"

	"github.com/prebid/prebid-server/v4/config"
)

// processConfig is the parity configuration of this process, set by
// wiring.Bootstrap in parity mode and nil otherwise.
var processConfig atomic.Pointer[config.ParityConfig]

// SetProcessConfig records the process's parity configuration, which makes
// WrapProcess wrap the transports handed to it. nil turns that off again.
// wiring.Bootstrap calls it in parity mode only.
func SetProcessConfig(cfg *config.ParityConfig) {
	if cfg == nil {
		processConfig.Store(nil)
		return
	}
	c := *cfg
	processConfig.Store(&c)
}

// WrapProcess is the seam for product code that builds its own transport per
// call instead of using one of the clients the router wraps (for example the
// /optout recaptcha check in pbs/usersync.go). Outside parity mode it returns rt
// unchanged at the cost of one atomic load; in parity mode it returns rt wrapped
// with kind, so the call goes to the oracle and never to the real network.
func WrapProcess(rt http.RoundTripper, kind string) http.RoundTripper {
	cfg := processConfig.Load()
	if cfg == nil {
		return rt
	}
	return New(rt, Options{Kind: kind}, *cfg)
}
