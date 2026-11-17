package wiring

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/analytics"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prebid/prebid-server/v4/privacy"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

const testToken = "0123456789abcdef"

func enabledConfig(oracleURL string) *config.Configuration {
	return &config.Configuration{
		Host:   "127.0.0.1",
		Parity: config.ParityConfig{Enabled: true, Token: testToken, OracleURL: oracleURL},
	}
}

var appHandler = http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
	w.WriteHeader(http.StatusTeapot)
	_, _ = io.WriteString(w, "app")
})

func TestMountDisabledReturnsApp(t *testing.T) {
	tests := []struct {
		name string
		cfg  *config.Configuration
	}{
		{name: "nil config", cfg: nil},
		{name: "disabled", cfg: &config.Configuration{}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := Mount(tt.cfg, Deps{}, appHandler, appHandler)
			rec := httptest.NewRecorder()
			got.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/__parity/health", nil))
			assert.Equal(t, http.StatusTeapot, rec.Code, "with parity disabled /__parity/* is ordinary app traffic")
		})
	}
}

func TestMountGuards(t *testing.T) {
	RegisterRoutes(nil)
	t.Cleanup(func() { RegisterRoutes(nil) })

	tests := []struct {
		name       string
		cfg        func() *config.Configuration
		routes     RoutesFactory
		path       string
		remoteAddr string
		token      string
		wantStatus int
		wantBody   string
	}{
		{name: "non parity path passes through", path: "/openrtb2/auction", remoteAddr: "203.0.113.1:1", wantStatus: http.StatusTeapot, wantBody: "app"},
		{name: "remote caller forbidden", path: "/__parity/health", remoteAddr: "203.0.113.1:1", token: testToken, wantStatus: http.StatusForbidden, wantBody: "loopback"},
		{name: "bare prefix is guarded", path: "/__parity", remoteAddr: "203.0.113.1:1", wantStatus: http.StatusForbidden},
		{name: "missing token unauthorized", path: "/__parity/health", remoteAddr: "127.0.0.1:5", wantStatus: http.StatusUnauthorized, wantBody: parity.TokenHeader},
		{name: "wrong token unauthorized", path: "/__parity/health", remoteAddr: "[::1]:5", token: "0123456789abcdeX", wantStatus: http.StatusUnauthorized},
		{name: "routes not linked", path: "/__parity/health", remoteAddr: "127.0.0.1:5", token: testToken, wantStatus: http.StatusNotImplemented, wantBody: "not linked"},
		{
			name: "registered routes receive authorized calls",
			path: "/__parity/health", remoteAddr: "127.0.0.1:5", token: testToken,
			routes: func(deps Deps, dispatch, app http.Handler) http.Handler {
				return http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
					_, _ = io.WriteString(w, "routes:"+deps.Cfg.Host)
				})
			},
			wantStatus: http.StatusOK, wantBody: "routes:127.0.0.1",
		},
		{
			name: "allow_non_loopback accepts remote callers",
			cfg: func() *config.Configuration {
				c := enabledConfig("")
				c.Parity.AllowNonLoopback = true
				return c
			},
			path: "/__parity/health", remoteAddr: "203.0.113.1:1", token: testToken,
			wantStatus: http.StatusNotImplemented,
		},
		{
			name: "unix socket peers are local when the listener is a socket",
			cfg: func() *config.Configuration {
				c := enabledConfig("")
				c.UnixSocketEnable = true
				return c
			},
			path: "/__parity/health", remoteAddr: "@", token: testToken,
			wantStatus: http.StatusNotImplemented,
		},
		{name: "unparseable peer is not local over tcp", path: "/__parity/health", remoteAddr: "@", token: testToken, wantStatus: http.StatusForbidden},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			RegisterRoutes(tt.routes)
			cfg := enabledConfig("")
			if tt.cfg != nil {
				cfg = tt.cfg()
			}
			h := Mount(cfg, NewDeps(cfg, nil), appHandler, appHandler)
			req := httptest.NewRequest(http.MethodGet, tt.path, nil)
			req.RemoteAddr = tt.remoteAddr
			if tt.token != "" {
				req.Header.Set(parity.TokenHeader, tt.token)
			}
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, req)
			assert.Equal(t, tt.wantStatus, rec.Code)
			assert.Contains(t, rec.Body.String(), tt.wantBody)
			if rec.Code >= 400 && rec.Code != http.StatusTeapot {
				var doc map[string]string
				require.NoError(t, json.Unmarshal(rec.Body.Bytes(), &doc), "errors use the parity error document")
				assert.NotEmpty(t, doc["error"])
				assert.Equal(t, "application/json", rec.Header().Get("Content-Type"))
			}
		})
	}
}

func TestEmptyTokenNeverMatches(t *testing.T) {
	cfg := enabledConfig("")
	cfg.Parity.Token = ""
	h := Mount(cfg, Deps{}, appHandler, appHandler)
	req := httptest.NewRequest(http.MethodGet, "/__parity/health", nil)
	req.RemoteAddr = "127.0.0.1:1"
	req.Header.Set(parity.TokenHeader, "")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	assert.Equal(t, http.StatusUnauthorized, rec.Code)
}

func TestWrapClient(t *testing.T) {
	original := &http.Transport{}
	tests := []struct {
		name     string
		cfg      *config.Configuration
		client   *http.Client
		wantKind string
	}{
		{name: "disabled leaves transport", cfg: &config.Configuration{}, client: &http.Client{Transport: original}},
		{name: "nil config leaves transport", cfg: nil, client: &http.Client{Transport: original}},
		{name: "enabled wraps", cfg: enabledConfig(""), client: &http.Client{Transport: original}, wantKind: parity.OutboundCache},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			WrapClient(tt.client, parity.OutboundCache, tt.cfg)
			if tt.wantKind == "" {
				assert.Same(t, original, tt.client.Transport)
				return
			}
			pt, ok := tt.client.Transport.(*transport.Transport)
			require.True(t, ok)
			assert.Equal(t, tt.wantKind, pt.Kind())
		})
	}
	assert.NotPanics(t, func() { WrapClient(nil, parity.OutboundCache, enabledConfig("")) })
}

// TestGeneralClientReachesOracle mirrors the startup GVL preload: a call on the
// wrapped general client with no case in flight goes to the oracle, labelled
// vendorlist by the classifier.
func TestGeneralClientReachesOracle(t *testing.T) {
	var mu sync.Mutex
	var metas []parity.OutboundMeta
	var uris []string
	oracle := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		meta, err := transport.DecodeMeta(r.Header.Get(parity.MetaHeader))
		mu.Lock()
		if err == nil {
			metas = append(metas, meta)
		}
		uris = append(uris, r.RequestURI)
		mu.Unlock()
		_, _ = io.WriteString(w, "{}")
	}))
	defer oracle.Close()

	cfg := enabledConfig(oracle.URL)
	cfg.StoredRequests.HTTP.Endpoint = "http://stored.example/requests"
	client := &http.Client{Transport: &http.Transport{}, Timeout: 5 * time.Second}
	WrapClient(client, parity.OutboundGeneral, cfg)

	for _, u := range []string{VendorListURLPrefix + "v3/vendor-list.json", "http://stored.example/requests?request-ids=1", "https://module.example/x"} {
		resp, err := client.Get(u)
		require.NoError(t, err)
		require.NoError(t, resp.Body.Close())
	}
	mu.Lock()
	defer mu.Unlock()
	require.Len(t, metas, 3)
	assert.Equal(t, []string{parity.OutboundVendorList, parity.OutboundStored, parity.OutboundGeneral},
		[]string{metas[0].Kind, metas[1].Kind, metas[2].Kind})
	assert.Equal(t, "", metas[0].Case, "startup calls carry an empty case id")
	assert.Equal(t, "http://vendor-list.consensu.org/v3/vendor-list.json", uris[0])
}

func TestGeneralClassifierRules(t *testing.T) {
	cfg := &config.Configuration{}
	cfg.StoredRequests.HTTP.Endpoint = "http://sr/req"
	cfg.StoredRequests.HTTP.AmpEndpoint = "http://sr/amp"
	cfg.Accounts.HTTP.Endpoint = "http://sr/accounts"
	cfg.CategoryMapping.HTTPEvents.Endpoint = "http://sr/cat-events"
	rules := GeneralClassifierRules(cfg)
	want := []transport.PrefixRule{
		{Prefix: VendorListURLPrefix, Kind: parity.OutboundVendorList},
		{Prefix: "http://sr/req", Kind: parity.OutboundStored},
		{Prefix: "http://sr/amp", Kind: parity.OutboundStored},
		{Prefix: "http://sr/cat-events", Kind: parity.OutboundStored},
		{Prefix: "http://sr/accounts", Kind: parity.OutboundStored},
	}
	assert.Equal(t, want, rules)
}

func TestBootstrap(t *testing.T) {
	originalDefault := http.DefaultTransport
	t.Cleanup(func() {
		http.DefaultTransport = originalDefault
		fakes.Uninstall()
		transport.SetProcessConfig(nil)
	})
	fakes.Uninstall()
	own := &http.Transport{}

	Bootstrap(&config.Configuration{})
	Bootstrap(nil)
	assert.Same(t, originalDefault, http.DefaultTransport, "disabled bootstrap changes nothing")
	assert.Nil(t, fakes.Active())
	assert.Same(t, own, transport.WrapProcess(own, parity.OutboundGeneral), "disabled bootstrap leaves per-call transports alone")

	Bootstrap(enabledConfig(""))
	assert.NotNil(t, fakes.Active(), "fakes are installed in parity mode")
	pt, ok := http.DefaultTransport.(*transport.Transport)
	require.True(t, ok)
	assert.Equal(t, parity.OutboundDefault, pt.Kind())
	wrapped, ok := transport.WrapProcess(own, parity.OutboundGeneral).(*transport.Transport)
	require.True(t, ok, "parity bootstrap routes per-call transports through the parity transport")
	assert.Equal(t, parity.OutboundGeneral, wrapped.Kind())
}

func TestNewDeps(t *testing.T) {
	cfg := enabledConfig("")
	cfg.BidderInfos = config.BidderInfos{"appnexus": {Endpoint: "http://a"}}
	deps := NewDeps(cfg, nil)
	assert.Same(t, cfg, deps.Cfg)
	assert.Equal(t, cfg.BidderInfos, deps.BidderInfos)
	assert.Nil(t, deps.Gatherer)
	assert.Equal(t, Deps{}, NewDeps(nil, nil))
}

// countingRunner is a fake analytics.Runner recording delegated calls.
type countingRunner struct {
	calls []string
}

func (c *countingRunner) LogAuctionObject(*analytics.AuctionObject, privacy.ActivityControl) {
	c.calls = append(c.calls, "auction")
}
func (c *countingRunner) LogVideoObject(*analytics.VideoObject, privacy.ActivityControl) {
	c.calls = append(c.calls, "video")
}
func (c *countingRunner) LogCookieSyncObject(*analytics.CookieSyncObject) {
	c.calls = append(c.calls, "cookie_sync")
}
func (c *countingRunner) LogSetUIDObject(*analytics.SetUIDObject) {
	c.calls = append(c.calls, "setuid")
}
func (c *countingRunner) LogAmpObject(*analytics.AmpObject, privacy.ActivityControl) {
	c.calls = append(c.calls, "amp")
}
func (c *countingRunner) LogNotificationEventObject(*analytics.NotificationEvent, privacy.ActivityControl) {
	c.calls = append(c.calls, "notification")
}
func (c *countingRunner) Shutdown() { c.calls = append(c.calls, "shutdown") }

func TestWrapAnalyticsDisabledIsIdentity(t *testing.T) {
	next := &countingRunner{}
	assert.Same(t, next, WrapAnalytics(next, &config.Configuration{}))
	assert.Same(t, next, WrapAnalytics(next, nil))
}

func TestAnalyticsRecorder(t *testing.T) {
	next := &countingRunner{}
	runner := WrapAnalytics(next, enabledConfig(""))
	ac := privacy.ActivityControl{}

	logAll := func() {
		runner.LogAuctionObject(&analytics.AuctionObject{
			Status:         200,
			Errors:         []error{&errortypes.BadInput{Message: "bad"}, errors.New("plain")},
			RequestWrapper: &openrtb_ext.RequestWrapper{BidRequest: &openrtb2.BidRequest{ID: "r1"}},
			Response:       &openrtb2.BidResponse{ID: "r1"},
			Account:        &config.Account{ID: "acct"},
			StartTime:      time.Now(),
		}, ac)
		runner.LogAmpObject(&analytics.AmpObject{Status: 200, Origin: "https://o"}, ac)
		runner.LogVideoObject(&analytics.VideoObject{Status: 400}, ac)
		runner.LogCookieSyncObject(&analytics.CookieSyncObject{Status: 200})
		runner.LogSetUIDObject(&analytics.SetUIDObject{Status: 200, Bidder: "appnexus", UID: "u", Success: true})
		runner.LogNotificationEventObject(&analytics.NotificationEvent{Request: &analytics.EventRequest{BidID: "b"}}, ac)
		runner.LogAuctionObject(nil, ac)
	}

	logAll() // no session: only delegation
	sess, end := trace.Begin(&parity.Envelope{CaseID: "analytics"})
	logAll()
	snap := sess.Snapshot()
	end()
	runner.Shutdown()

	assert.Equal(t, []string{
		"auction", "amp", "video", "cookie_sync", "setuid", "notification", "auction",
		"auction", "amp", "video", "cookie_sync", "setuid", "notification", "auction",
		"shutdown",
	}, next.calls, "every call is delegated unchanged")

	kinds := make([]string, 0, len(snap.Analytics))
	for _, rec := range snap.Analytics {
		kinds = append(kinds, rec.Kind)
	}
	assert.Equal(t, []string{
		parity.AnalyticsAuction, parity.AnalyticsAmp, parity.AnalyticsVideo,
		parity.AnalyticsCookieSync, parity.AnalyticsSetUID, parity.AnalyticsNotification,
	}, kinds, "nil objects are not recorded")

	var auction map[string]any
	require.NoError(t, json.Unmarshal(snap.Analytics[0].Object, &auction))
	assert.Equal(t, float64(200), auction["status"])
	assert.Equal(t, "acct", auction["account_id"])
	assert.Equal(t, true, auction["start_time_set"])
	assert.Equal(t, "r1", auction["request"].(map[string]any)["id"])
	errs := auction["errors"].([]any)
	require.Len(t, errs, 2)
	assert.Equal(t, float64(errortypes.BadInputErrorCode), errs[0].(map[string]any)["code"])
	assert.Equal(t, float64(errortypes.UnknownErrorCode), errs[1].(map[string]any)["code"])
	assert.True(t, strings.Contains(string(snap.Analytics[4].Object), `"success":true`))
	assert.Equal(t, parity.StageCaptured, snap.StageStatus[parity.StageAnalytics])
}

func TestIsParityPath(t *testing.T) {
	tests := []struct {
		path string
		want bool
	}{
		{path: "/__parity/exec", want: true},
		{path: "/__parity", want: true},
		{path: "/__parity/", want: true},
		{path: "/__parityx", want: false},
		{path: "/openrtb2/auction", want: false},
		{path: "/x/__parity/exec", want: false},
	}
	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			assert.Equal(t, tt.want, IsParityPath(tt.path))
		})
	}
}
