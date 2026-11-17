package runner

import (
	"net/http"
	"strings"
	"testing"

	"github.com/julienschmidt/httprouter"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// goRoutes are the routes the Go server's health document lists with vtrack
// on (parity/server/introspect.go productEndpoints), plus the routes of the
// stored-request event API.
var goRoutes = []string{
	"/openrtb2/auction", "/openrtb2/video", "/openrtb2/amp", "/info/bidders", "/info/bidders/:bidderName",
	"/bidders/params", "/cookie_sync", "/status", "/", "/version", "/static/*filepath", "/vtrack", "/event",
	"/setuid", "/getuids", "/optout", "/storedrequests/openrtb2", "/storedrequests/amp",
}

// routePaths are paths around every kind of pattern: exact, with and without
// a trailing slash, with other case, empty and nested parameters, the
// catch-all with and without its slash, and paths no route registers.
var routePaths = []string{
	"/", "//", "/openrtb2/auction", "/openrtb2/auction/", "/OPENRTB2/auction", "/openrtb2", "/openrtb2/",
	"/info/bidders", "/info/bidders/", "/info/bidders//", "/info/bidders/appnexus", "/info/bidders/appnexus/",
	"/info/bidders/appnexus/x", "/info/bidders/a%2Fb", "/info/biddersx", "/static", "/static/", "/static/x",
	"/static/a/b/c", "/staticx", "/status", "/status/", "/statusx", "/cookie_sync", "/vtrack", "/x", "/debug/pprof/",
	"/storedrequests/openrtb2", "/storedrequests", "/a//c", "/a/b/c", "/a/b/", "/a/b",
}

// TestRouteMatchIsHTTPRouterLookup: routeMatch finds a route exactly when
// httprouter v1.3.0, holding the same routes, finds a handle for the path.
func TestRouteMatchIsHTTPRouterLookup(t *testing.T) {
	routes := append([]string{"/a/:b/c"}, goRoutes...)
	router := httprouter.New()
	for _, r := range routes {
		r := r
		router.GET(r, func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) { _, _ = w.Write([]byte(r)) })
	}
	paths := append([]string{}, routePaths...)
	for _, cs := range corpus.LoadReplayHarness(mustRepo(t)).Cases {
		paths = append(paths, cs.Envelope.Request.Path)
	}
	for _, path := range paths {
		handle, _, _ := router.Lookup(http.MethodGet, path)
		got := matchingRoute(routes, path)
		if (handle != nil) != (got != "") {
			t.Errorf("%q: httprouter finds a handle: %v, matchingRoute: %q", path, handle != nil, got)
		}
	}
}

func TestRouteMatch(t *testing.T) {
	tests := []struct {
		pattern, path string
		want          bool
	}{
		{"/info/bidders/:bidderName", "/info/bidders/appnexus", true},
		{"/info/bidders/:bidderName", "/info/bidders/", false},
		{"/info/bidders/:bidderName", "/info/bidders/appnexus/", false},
		{"/a/:b/c", "/a//c", true},
		{"/static/*filepath", "/static/", true},
		{"/static/*filepath", "/static/a/b", true},
		{"/static/*filepath", "/static", false},
		{"/status", "/status/", false},
		{"/", "/", true},
		{"/", "//", false},
	}
	for _, tt := range tests {
		if got := routeMatch(tt.pattern, tt.path); got != tt.want {
			t.Errorf("routeMatch(%q, %q) = %v, want %v", tt.pattern, tt.path, got, tt.want)
		}
	}
}

func routeSide(name, impl string, routes ...string) *side {
	return &side{name: name, impl: impl, srv: &Server{Health: &parity.Health{Endpoints: routes}}}
}

func TestRouteSupport(t *testing.T) {
	full := goRoutes
	partial := []string{"/openrtb2/auction", "/status"}
	tests := []struct {
		name   string
		a, b   []string
		path   string
		reason string
	}{
		{name: "a route of both", a: full, b: full, path: "/info/bidders/appnexus"},
		{name: "unrouted, same routes", a: full, b: full, path: "/info/bidders/"},
		{name: "unrouted, a side lists every route", a: full, b: nil, path: "/x", reason: "go (a) exec cannot dispatch to /x: it lists no route matching it, and rust (b) lists no endpoints, so every route"},
		{name: "a route of one side", a: full, b: partial, path: "/info/bidders/appnexus",
			reason: "rust (b) exec cannot dispatch to /info/bidders/appnexus: it lists no route matching it, and go (a) lists /info/bidders/:bidderName"},
		{name: "unrouted, other routes", a: partial, b: append([]string{"/vtrack"}, partial...), path: "/status/",
			reason: "no route matches /status/, and only rust (b) lists /vtrack: the routers' own answers"},
		{name: "a side without a list serves every route", a: full, b: nil, path: "/status"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := routeSupport([2]*side{routeSide(sideA, parity.ImplGo, tt.a...), routeSide(sideB, parity.ImplRust, tt.b...)}, tt.path)
			if tt.reason == "" && got != "" || !strings.Contains(got, tt.reason) {
				t.Errorf("routeSupport = %q, want %q", got, tt.reason)
			}
		})
	}
}

func mustRepo(t *testing.T) *corpus.Repo {
	t.Helper()
	repo, err := corpus.OpenRepo(testRepo(t))
	if err != nil {
		t.Fatalf("OpenRepo: %v", err)
	}
	return repo
}
