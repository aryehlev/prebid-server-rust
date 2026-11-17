package server

import (
	"bytes"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	pbsserver "github.com/prebid/prebid-server/v4/server"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// mounted is the main-port handler main.go builds in parity mode, with the
// routes this package registers from init.
func mounted(t *testing.T) http.Handler {
	t.Helper()
	cfg := &config.Configuration{Parity: config.ParityConfig{Enabled: true, Token: testToken}}
	app := noCache(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { io.WriteString(w, "product") }))
	return wiring.Mount(cfg, wiring.Deps{Cfg: cfg}, app, pbsserver.WrapCompression(app, config.CompressionInfo{GZIP: true}))
}

func TestMountedRoutes(t *testing.T) {
	h := mounted(t)
	tests := []struct {
		name       string
		method     string
		path       string
		token      string
		remote     string
		body       []byte
		wantStatus int
		wantBody   string
	}{
		{name: "registered from init", method: http.MethodGet, path: PathHealth, token: testToken, remote: "127.0.0.1:1", wantStatus: http.StatusOK, wantBody: parity.SchemaHealth},
		{name: "IPv6 loopback", method: http.MethodGet, path: PathHealth, token: testToken, remote: "[::1]:1", wantStatus: http.StatusOK, wantBody: parity.SchemaHealth},
		{name: "missing token", method: http.MethodGet, path: PathHealth, remote: "127.0.0.1:1", wantStatus: http.StatusUnauthorized, wantBody: parity.TokenHeader},
		{name: "wrong token", method: http.MethodGet, path: PathHealth, token: "x" + testToken, remote: "127.0.0.1:1", wantStatus: http.StatusUnauthorized, wantBody: parity.TokenHeader},
		{name: "non-loopback caller", method: http.MethodGet, path: PathHealth, token: testToken, remote: "192.0.2.1:1", wantStatus: http.StatusForbidden, wantBody: "loopback"},
		{name: "product traffic passes through", method: http.MethodGet, path: "/status", remote: "192.0.2.1:1", wantStatus: http.StatusOK, wantBody: "product"},
		{name: "exec dispatches without the token", method: http.MethodPost, path: PathExec, token: testToken, remote: "127.0.0.1:1", body: []byte(`{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":"c","request":{"method":"GET","path":"/status","headers":[]}}`), wantStatus: http.StatusOK, wantBody: `"text":"product"`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(tt.method, tt.path, bytes.NewReader(tt.body))
			req.RemoteAddr = tt.remote
			if tt.token != "" {
				req.Header.Set(parity.TokenHeader, tt.token)
			}
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, req)
			assert.Equal(t, tt.wantStatus, rec.Code, rec.Body.String())
			assert.Contains(t, rec.Body.String(), tt.wantBody)
		})
	}
}

func TestRouting(t *testing.T) {
	h := newServer(wiring.Deps{}, http.NotFoundHandler())
	tests := []struct {
		name       string
		method     string
		path       string
		wantStatus int
		wantAllow  string
		wantError  string
	}{
		{name: "unknown route", method: http.MethodGet, path: parity.PathPrefix + "nope", wantStatus: http.StatusNotFound, wantError: "parity: no route /__parity/nope"},
		{name: "prefix only", method: http.MethodGet, path: parity.PathPrefix, wantStatus: http.StatusNotFound, wantError: "no route"},
		{name: "adapter without bidder", method: http.MethodPost, path: AdapterPrefix + "/requests", wantStatus: http.StatusNotFound, wantError: "no route"},
		{name: "exec is POST only", method: http.MethodGet, path: PathExec, wantStatus: http.StatusMethodNotAllowed, wantAllow: "POST", wantError: "does not accept GET"},
		{name: "health is GET only", method: http.MethodPost, path: PathHealth, wantStatus: http.StatusMethodNotAllowed, wantAllow: "GET", wantError: "does not accept POST"},
		{name: "amp takes GET and POST", method: http.MethodDelete, path: PathAMP, wantStatus: http.StatusMethodNotAllowed, wantAllow: "GET, POST", wantError: "does not accept DELETE"},
		{name: "reset is POST only", method: http.MethodGet, path: PathReset, wantStatus: http.StatusMethodNotAllowed, wantAllow: "POST", wantError: "does not accept GET"},
		{name: "empty exec body", method: http.MethodPost, path: PathExec, wantStatus: http.StatusBadRequest, wantError: "decoding document"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			rec := call(t, h, tt.method, tt.path, nil)
			assertError(t, rec, tt.wantStatus, tt.wantError)
			assert.Equal(t, tt.wantAllow, rec.Header().Get("Allow"))
		})
	}
}

func TestAdapterRoute(t *testing.T) {
	tests := []struct {
		path       string
		wantBidder string
		wantOp     string
		wantOK     bool
	}{
		{path: AdapterPrefix + "appnexus/requests", wantBidder: "appnexus", wantOp: adapterOpRequests, wantOK: true},
		{path: AdapterPrefix + "appnexus/bids", wantBidder: "appnexus", wantOp: adapterOpBids, wantOK: true},
		{path: AdapterPrefix + "appnexus/bids/extra"},
		{path: AdapterPrefix + "appnexus"},
		{path: AdapterPrefix + "/bids"},
		{path: PathExec},
	}
	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			bidder, op, ok := adapterRoute(tt.path)
			assert.Equal(t, tt.wantOK, ok)
			assert.Equal(t, tt.wantBidder, bidder)
			assert.Equal(t, tt.wantOp, op)
		})
	}
}

func TestCheckHeader(t *testing.T) {
	tests := []struct {
		name    string
		schema  string
		version string
		wantErr string
	}{
		{name: "current", schema: parity.SchemaEnvelope, version: parity.SchemaVersion},
		{name: "newer minor", schema: parity.SchemaEnvelope, version: "1.9.3"},
		{name: "other schema", schema: parity.SchemaTrace, version: parity.SchemaVersion, wantErr: "schema must be"},
		{name: "other major", schema: parity.SchemaEnvelope, version: "2.0.0", wantErr: "unsupported schema_version"},
		{name: "two parts", schema: parity.SchemaEnvelope, version: "1.0", wantErr: "unsupported schema_version"},
		{name: "empty", schema: parity.SchemaEnvelope, version: "", wantErr: "unsupported schema_version"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := checkHeader(tt.schema, tt.version, parity.SchemaEnvelope)
			if tt.wantErr == "" {
				assert.NoError(t, err)
				return
			}
			require.Error(t, err)
			assert.Contains(t, err.Error(), tt.wantErr)
			var doc *errDocument
			require.ErrorAs(t, err, &doc)
			assert.Equal(t, http.StatusBadRequest, doc.status)
		})
	}
}

func TestDecodeStrict(t *testing.T) {
	type doc struct {
		A int `json:"a"`
	}
	tests := []struct {
		raw     string
		wantErr string
	}{
		{raw: `{"a":1}`},
		{raw: `{"a":1}` + "\n"},
		{raw: ``, wantErr: "EOF"},
		{raw: `{"a":1,"b":2}`, wantErr: "unknown field"},
		{raw: `{"a":1}{"a":2}`, wantErr: "trailing data"},
		{raw: `[1]`, wantErr: "cannot unmarshal"},
	}
	for _, tt := range tests {
		t.Run(tt.raw, func(t *testing.T) {
			var v doc
			err := decodeStrict([]byte(tt.raw), &v)
			if tt.wantErr == "" {
				require.NoError(t, err)
				assert.Equal(t, 1, v.A)
				return
			}
			require.Error(t, err)
			assert.True(t, strings.Contains(err.Error(), tt.wantErr), err.Error())
		})
	}
}

func TestWriteJSONKeepsRawBytes(t *testing.T) {
	rec := httptest.NewRecorder()
	writeJSON(rec, http.StatusAccepted, map[string]string{"html": "<a&b>"})
	assert.Equal(t, http.StatusAccepted, rec.Code)
	assert.Equal(t, "application/json", rec.Header().Get("Content-Type"))
	assert.Equal(t, `{"html":"<a&b>"}`+"\n", rec.Body.String())
}
