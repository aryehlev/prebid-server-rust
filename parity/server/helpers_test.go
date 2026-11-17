package server

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"testing"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	"github.com/spf13/viper"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

const (
	testToken       = "0123456789abcdef"
	bidderInfoDir   = "../../static/bidder-info"
	testGDPRDefault = "1"
)

// TestMain installs the deterministic fakes, as wiring.Bootstrap does in parity
// mode, so adapters built by the tests pick them up.
func TestMain(m *testing.M) {
	fakes.Install()
	code := m.Run()
	fakes.Uninstall()
	os.Exit(code)
}

var (
	infosOnce sync.Once
	infos     config.BidderInfos
	infosErr  error
)

// staticBidderInfos loads static/bidder-info once per test binary.
func staticBidderInfos(t *testing.T) config.BidderInfos {
	t.Helper()
	infosOnce.Do(func() { infos, infosErr = config.LoadBidderInfoFromDisk(bidderInfoDir) })
	require.NoError(t, infosErr)
	return infos
}

// resolvedConfig builds the configuration exactly like main.go loadConfig,
// with the given viper overrides.
func resolvedConfig(t *testing.T, overrides map[string]any) *config.Configuration {
	t.Helper()
	bidderInfos := staticBidderInfos(t)
	v := viper.New()
	config.SetupViper(v, "", bidderInfos)
	v.Set("gdpr.default_value", testGDPRDefault)
	for key, value := range overrides {
		v.Set(key, value)
	}
	cfg, err := config.New(v, bidderInfos, openrtb_ext.NormalizeBidderName)
	require.NoError(t, err)
	return cfg
}

// newServer builds the parity routes over dispatch.
func newServer(deps wiring.Deps, dispatch http.Handler) http.Handler {
	return New(deps, dispatch, dispatch)
}

// call sends one request to h and returns the recorded response.
func call(t *testing.T, h http.Handler, method, path string, body []byte) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, path, bytes.NewReader(body))
	req.RemoteAddr = "127.0.0.1:4242"
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

// mustJSON marshals v for a request body.
func mustJSON(t *testing.T, v any) []byte {
	t.Helper()
	raw, err := json.Marshal(v)
	require.NoError(t, err)
	return raw
}

// decodeDoc checks that rec holds a valid parity document of the given status
// and decodes it into v.
func decodeDoc(t *testing.T, rec *httptest.ResponseRecorder, status int, v any) {
	t.Helper()
	require.Equal(t, status, rec.Code, "body: %s", rec.Body.String())
	assert.Equal(t, "application/json", rec.Header().Get("Content-Type"))
	require.NoError(t, schema.Validate(rec.Body.Bytes()), "the document must validate against its schema")
	require.NoError(t, json.Unmarshal(rec.Body.Bytes(), v))
}

// assertError checks the parity error document.
func assertError(t *testing.T, rec *httptest.ResponseRecorder, status int, contains string) {
	t.Helper()
	assert.Equal(t, status, rec.Code, "body: %s", rec.Body.String())
	assert.Equal(t, "application/json", rec.Header().Get("Content-Type"))
	require.NoError(t, schema.ValidateDefinition(schema.FileIntrospection, "Error", rec.Body.Bytes()))
	var doc map[string]string
	require.NoError(t, json.Unmarshal(rec.Body.Bytes(), &doc))
	assert.Contains(t, doc["error"], contains)
}

// testEnvelope is a minimal valid envelope for method and path.
func testEnvelope(method, path string, headers parity.HeaderList, body *parity.Body) *parity.Envelope {
	if headers == nil {
		headers = parity.HeaderList{}
	}
	return &parity.Envelope{
		Schema:        parity.SchemaEnvelope,
		SchemaVersion: parity.SchemaVersion,
		CaseID:        "test:" + method + " " + path,
		Request:       parity.ExecRequest{Method: method, Path: path, Headers: headers, Body: body},
	}
}

// execTrace posts env to /__parity/exec on h and decodes the trace.
func execTrace(t *testing.T, h http.Handler, env *parity.Envelope) parity.Trace {
	t.Helper()
	rec := call(t, h, http.MethodPost, PathExec, mustJSON(t, env))
	var tr parity.Trace
	decodeDoc(t, rec, http.StatusOK, &tr)
	return tr
}
