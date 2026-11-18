package wiring

import (
	"crypto/subtle"
	"encoding/json"
	"net"
	"net/http"
	"strings"
	"sync"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
)

// RoutesFactory builds the handler for /__parity/* (implemented by
// parity/server). dispatch is exactly what the main listener serves for normal
// traffic, compression included; app is the same chain without compression.
type RoutesFactory func(deps Deps, dispatch http.Handler, app http.Handler) http.Handler

var (
	routesMu      sync.Mutex
	routesFactory RoutesFactory
)

// RegisterRoutes installs the /__parity/* routes factory. parity/server calls it
// from init; the binary links it with a blank import in main.go.
func RegisterRoutes(f RoutesFactory) {
	routesMu.Lock()
	defer routesMu.Unlock()
	routesFactory = f
}

func registeredRoutes() RoutesFactory {
	routesMu.Lock()
	defer routesMu.Unlock()
	return routesFactory
}

// Mount returns app unchanged unless parity is enabled. In parity mode it
// intercepts /__parity/* before NoCache and CORS, checks that the caller is on
// loopback (or on the unix socket that is the main listener) and sent the
// right X-Parity-Token, and hands the request to the registered routes. No
// setting lifts the loopback check. Every other path goes to app.
func Mount(cfg *config.Configuration, deps Deps, app, dispatch http.Handler) http.Handler {
	if cfg == nil || !cfg.Parity.Enabled {
		return app
	}
	routes := http.Handler(http.HandlerFunc(notLinked))
	if f := registeredRoutes(); f != nil {
		routes = f(deps, dispatch, app)
	}
	return &interceptor{
		app:        app,
		routes:     routes,
		token:      []byte(cfg.Parity.Token),
		unixSocket: cfg.UnixSocketEnable,
	}
}

type interceptor struct {
	app        http.Handler
	routes     http.Handler
	token      []byte
	unixSocket bool
}

func (i *interceptor) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !IsParityPath(r.URL.Path) {
		i.app.ServeHTTP(w, r)
		return
	}
	if !i.localCaller(r.RemoteAddr) {
		WriteError(w, http.StatusForbidden, "parity: /__parity/* only accepts loopback callers")
		return
	}
	if !i.tokenMatches(r.Header.Get(parity.TokenHeader)) {
		WriteError(w, http.StatusUnauthorized, "parity: missing or invalid "+parity.TokenHeader)
		return
	}
	i.routes.ServeHTTP(w, r)
}

func (i *interceptor) tokenMatches(got string) bool {
	return len(i.token) > 0 && subtle.ConstantTimeCompare([]byte(got), i.token) == 1
}

// localCaller accepts loopback TCP peers, and unix-socket peers (whose
// RemoteAddr is not host:port) when the main listener is a unix socket.
func (i *interceptor) localCaller(remoteAddr string) bool {
	host, _, err := net.SplitHostPort(remoteAddr)
	if err != nil {
		return i.unixSocket
	}
	return config.IsLoopbackHost(host)
}

// IsParityPath reports whether path belongs to the parity route namespace.
func IsParityPath(path string) bool {
	return path == strings.TrimSuffix(parity.PathPrefix, "/") || strings.HasPrefix(path, parity.PathPrefix)
}

// WriteError writes the parity error document {"error": msg}.
func WriteError(w http.ResponseWriter, status int, msg string) {
	body, err := json.Marshal(map[string]string{"error": msg})
	if err != nil {
		body = []byte(`{"error":"parity: internal error"}`)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	w.Write(body)
}

func notLinked(w http.ResponseWriter, _ *http.Request) {
	WriteError(w, http.StatusNotImplemented, "parity: /__parity/* routes are not linked into this binary")
}
