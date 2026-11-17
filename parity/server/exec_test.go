package server

import (
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/julienschmidt/httprouter"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prebid/prebid-server/v4/parity/wiring"
	pbsserver "github.com/prebid/prebid-server/v4/server"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// largeJSON is above gziphandler's minimum size, so the response is compressed.
var largeJSON = `{"pad":"` + strings.Repeat("x", 2048) + `"}`

// testApp is a minimal product router behind a NoCache-style wrapper and the
// main listener's gzip compression.
func testApp() http.Handler {
	r := httprouter.New()
	r.POST("/json", func(w http.ResponseWriter, req *http.Request, _ httprouter.Params) {
		body, _ := io.ReadAll(req.Body)
		w.Header().Set("Content-Type", "application/json")
		w.Header().Add("X-Multi", "b")
		w.Header().Add("X-Multi", "a")
		w.Write(body)
	})
	r.GET("/sniff", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		io.WriteString(w, "<html><body>sniffed</body></html>")
	})
	r.GET("/status-then-write", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		w.WriteHeader(http.StatusCreated)
		io.WriteString(w, "created")
	})
	r.GET("/late-header", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusAccepted)
		w.Header().Set("X-Late", "ignored")
		io.WriteString(w, `{}`)
	})
	r.GET("/no-content", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		w.Header().Set("Content-Type", "text/plain")
		w.WriteHeader(http.StatusNoContent)
		io.WriteString(w, "dropped")
	})
	r.GET("/not-modified", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		w.Header().Set("Content-Type", "text/plain")
		w.Header().Set("ETag", `"v1"`)
		w.WriteHeader(http.StatusNotModified)
	})
	r.GET("/flush", func(w http.ResponseWriter, _ *http.Request, _ httprouter.Params) {
		io.WriteString(w, "plain first")
		w.(http.Flusher).Flush()
		io.WriteString(w, "<html>after flush</html>")
	})
	r.GET("/empty", func(http.ResponseWriter, *http.Request, httprouter.Params) {})
	return pbsserver.WrapCompression(noCache(r), config.CompressionInfo{GZIP: true})
}

// noCache mimics router.NoCache: it sets headers before calling the router.
func noCache(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-cache, no-store, must-revalidate")
		w.Header().Set("Pragma", "no-cache")
		h.ServeHTTP(w, r)
	})
}

// framingHeaders are added by net/http's server on the wire; the http.response
// stage holds the handler chain's response without them.
var framingHeaders = []string{"Date", "Content-Length", "Transfer-Encoding", "Connection"}

type wireResponse struct {
	status int
	header http.Header
	body   []byte
}

// serveOverWire sends the request through a real net/http server running app
// (direct ServeHTTP by the listener) and returns what the client received,
// without framing headers. Client-side decompression is off.
func serveOverWire(t *testing.T, app http.Handler, method, path string, headers parity.HeaderList, body []byte) wireResponse {
	t.Helper()
	srv := httptest.NewServer(app)
	defer srv.Close()
	req, err := http.NewRequest(method, srv.URL+path, bytes.NewReader(body))
	require.NoError(t, err)
	for _, kv := range headers {
		req.Header.Add(kv[0], kv[1])
	}
	client := &http.Client{Transport: &http.Transport{DisableCompression: true}}
	resp, err := client.Do(req)
	require.NoError(t, err)
	defer resp.Body.Close()
	got, err := io.ReadAll(resp.Body)
	require.NoError(t, err)
	header := resp.Header.Clone()
	for _, name := range framingHeaders {
		header.Del(name)
	}
	return wireResponse{status: resp.StatusCode, header: header, body: got}
}

// serveDirect calls app.ServeHTTP with an httptest recorder.
func serveDirect(app http.Handler, method, path string, headers parity.HeaderList, body []byte) *http.Response {
	req := httptest.NewRequest(method, path, bytes.NewReader(body))
	for _, kv := range headers {
		req.Header.Add(kv[0], kv[1])
	}
	rec := httptest.NewRecorder()
	app.ServeHTTP(rec, req)
	return rec.Result()
}

func TestExecMatchesDirectServe(t *testing.T) {
	app := testApp()
	h := newServer(wiring.Deps{}, app)
	tests := []struct {
		name    string
		method  string
		path    string
		headers parity.HeaderList
		body    string
	}{
		{name: "json with multi-value header", method: http.MethodPost, path: "/json", headers: parity.HeaderList{{"Content-Type", "application/json"}}, body: `{"id":"a&b<c>"}`},
		{name: "gzip compressed", method: http.MethodPost, path: "/json", headers: parity.HeaderList{{"Accept-Encoding", "gzip"}}, body: largeJSON},
		{name: "gzip requested below min size", method: http.MethodPost, path: "/json", headers: parity.HeaderList{{"Accept-Encoding", "gzip"}}, body: `{"small":true}`},
		{name: "sniffed content type", method: http.MethodGet, path: "/sniff"},
		{name: "sniffed after explicit status", method: http.MethodGet, path: "/status-then-write"},
		{name: "headers after WriteHeader are ignored", method: http.MethodGet, path: "/late-header"},
		{name: "204 drops the body", method: http.MethodGet, path: "/no-content"},
		{name: "304 drops content type", method: http.MethodGet, path: "/not-modified"},
		{name: "flush sends the header", method: http.MethodGet, path: "/flush"},
		{name: "empty handler", method: http.MethodGet, path: "/empty"},
		{name: "router 404", method: http.MethodGet, path: "/missing"},
		{name: "router 405", method: http.MethodGet, path: "/json"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var body *parity.Body
			if tt.body != "" {
				body = parity.TextBody(tt.body)
			}
			tr := execTrace(t, h, testEnvelope(tt.method, tt.path, tt.headers, body))
			require.NotNil(t, tr.HTTPResponse)
			gotBody, err := tr.HTTPResponse.Body.WireBytes()
			require.NoError(t, err)

			wire := serveOverWire(t, app, tt.method, tt.path, tt.headers, []byte(tt.body))
			assert.Equal(t, wire.status, tr.HTTPResponse.Status, "status matches the wire")
			assert.Equal(t, wire.header, transport.HTTPHeaderOf(tr.HTTPResponse.Headers), "headers match the wire")
			assert.Equal(t, transport.HeaderListOf(wire.header), tr.HTTPResponse.Headers, "headers are in canonical order")
			assert.Equal(t, string(wire.body), string(gotBody), "body matches the wire")

			direct := serveDirect(app, tt.method, tt.path, tt.headers, []byte(tt.body))
			assert.Equal(t, direct.StatusCode, tr.HTTPResponse.Status, "status matches direct ServeHTTP")
			if bodyAllowedForStatus(direct.StatusCode) {
				// httptest.ResponseRecorder keeps a body net/http drops for 204
				// and 304; the wire comparison above covers those.
				directBody, err := io.ReadAll(direct.Body)
				require.NoError(t, err)
				assert.Equal(t, string(directBody), string(gotBody), "body matches direct ServeHTTP")
			}
		})
	}
}

func TestExecCompressedResponseBody(t *testing.T) {
	h := newServer(wiring.Deps{}, testApp())
	tr := execTrace(t, h, testEnvelope(http.MethodPost, "/json", parity.HeaderList{{"Accept-Encoding", "gzip"}}, parity.TextBody(largeJSON)))
	body := tr.HTTPResponse.Body
	require.NotNil(t, body)
	assert.Equal(t, "gzip", body.ContentEncoding)
	assert.Nil(t, body.Text, "a content-encoded body is recorded as b64")
	require.NotNil(t, body.B64)
	assert.JSONEq(t, largeJSON, string(body.JSON), "json is the decoded content")
}

// echoHandler reports what the handler chain received.
func echoHandler(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	out := map[string]any{
		"method":            r.Method,
		"path":              r.URL.Path,
		"raw_query":         r.URL.RawQuery,
		"request_uri":       r.RequestURI,
		"host":              r.Host,
		"remote_addr":       r.RemoteAddr,
		"header":            r.Header,
		"content_length":    r.ContentLength,
		"transfer_encoding": r.TransferEncoding,
		"body":              body,
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(out)
}

type echoed struct {
	Method           string      `json:"method"`
	Path             string      `json:"path"`
	RawQuery         string      `json:"raw_query"`
	RequestURI       string      `json:"request_uri"`
	Host             string      `json:"host"`
	RemoteAddr       string      `json:"remote_addr"`
	Header           http.Header `json:"header"`
	ContentLength    int64       `json:"content_length"`
	TransferEncoding []string    `json:"transfer_encoding"`
	Body             []byte      `json:"body"`
}

func gzipped(t *testing.T, s string) []byte {
	t.Helper()
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	_, err := zw.Write([]byte(s))
	require.NoError(t, err)
	require.NoError(t, zw.Close())
	return buf.Bytes()
}

func TestExecRequestReconstruction(t *testing.T) {
	h := newServer(wiring.Deps{}, http.HandlerFunc(echoHandler))
	gz := gzipped(t, `{"id":"gz"}`)
	gzB64 := parity.NewBody(gz, "gzip").B64
	tests := []struct {
		name  string
		edit  func(*parity.ExecRequest)
		check func(*testing.T, echoed)
	}{
		{
			name: "defaults",
			edit: func(*parity.ExecRequest) {},
			check: func(t *testing.T, e echoed) {
				assert.Equal(t, DefaultHost, e.Host)
				assert.Equal(t, DefaultRemoteAddr, e.RemoteAddr)
				assert.Equal(t, "/x", e.RequestURI)
				assert.Equal(t, int64(0), e.ContentLength)
				assert.Empty(t, e.Body)
			},
		},
		{
			name: "query, headers in order, host header and remote addr",
			edit: func(r *parity.ExecRequest) {
				r.Query = "a=1&b=%20"
				r.RemoteAddr = "198.51.100.7:41234"
				r.Headers = parity.HeaderList{{"x-multi", "1"}, {"Host", "pbs.example"}, {"X-Multi", "2"}, {"Cookie", "uids=abc"}}
			},
			check: func(t *testing.T, e echoed) {
				assert.Equal(t, "a=1&b=%20", e.RawQuery)
				assert.Equal(t, "/x?a=1&b=%20", e.RequestURI)
				assert.Equal(t, "pbs.example", e.Host)
				assert.Equal(t, "198.51.100.7:41234", e.RemoteAddr)
				assert.Equal(t, []string{"1", "2"}, e.Header["X-Multi"], "names are canonicalized and values keep their order")
				assert.NotContains(t, e.Header, "Host", "Host moves to Request.Host")
			},
		},
		{
			name: "explicit host wins over the header",
			edit: func(r *parity.ExecRequest) {
				r.Host = "override.example"
				r.Headers = parity.HeaderList{{"Host", "header.example"}}
			},
			check: func(t *testing.T, e echoed) { assert.Equal(t, "override.example", e.Host) },
		},
		{
			name: "b64 body is dispatched byte-exact",
			edit: func(r *parity.ExecRequest) {
				r.Method = http.MethodPost
				r.Headers = parity.HeaderList{{"Content-Encoding", "gzip"}, {"Content-Length", fmt.Sprint(len(gz))}}
				r.Body = &parity.Body{B64: gzB64}
			},
			check: func(t *testing.T, e echoed) {
				assert.Equal(t, gz, e.Body)
				assert.Equal(t, int64(len(gz)), e.ContentLength)
			},
		},
		{
			name: "text body with content_encoding is encoded by the server",
			edit: func(r *parity.ExecRequest) {
				r.Method = http.MethodPost
				r.Body = &parity.Body{Text: parity.TextBody(`{"id":"gz"}`).Text, ContentEncoding: "gzip"}
			},
			check: func(t *testing.T, e echoed) {
				zr, err := gzip.NewReader(bytes.NewReader(e.Body))
				require.NoError(t, err)
				plain, err := io.ReadAll(zr)
				require.NoError(t, err)
				assert.Equal(t, `{"id":"gz"}`, string(plain))
			},
		},
		{
			name: "chunked transfer encoding moves out of the header",
			edit: func(r *parity.ExecRequest) {
				r.Method = http.MethodPost
				r.Headers = parity.HeaderList{{"Transfer-Encoding", "chunked"}}
				r.Body = parity.TextBody(`{}`)
			},
			check: func(t *testing.T, e echoed) {
				assert.Equal(t, []string{"chunked"}, e.TransferEncoding)
				assert.Equal(t, int64(-1), e.ContentLength)
				assert.NotContains(t, e.Header, "Transfer-Encoding")
				assert.Equal(t, `{}`, string(e.Body))
			},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			env := testEnvelope(http.MethodGet, "/x", nil, nil)
			tt.edit(&env.Request)
			tr := execTrace(t, h, env)
			var got echoed
			require.NoError(t, json.Unmarshal(tr.HTTPResponse.Body.JSON, &got))
			assert.Equal(t, env.Request.Method, got.Method)
			assert.Equal(t, "/x", got.Path)
			tt.check(t, got)

			require.NotNil(t, tr.HTTPIn)
			assert.Equal(t, env.Request.Headers, tr.HTTPIn.Headers, "http_in keeps the envelope header list")
			wire, err := env.Request.Body.WireBytes()
			require.NoError(t, err)
			inWire, err := tr.HTTPIn.Body.WireBytes()
			require.NoError(t, err)
			assert.Equal(t, wire, inWire, "http_in holds the dispatched wire bytes")
		})
	}
}

func TestExecHTTPInDecodesContentEncoding(t *testing.T) {
	h := newServer(wiring.Deps{}, http.HandlerFunc(echoHandler))
	env := testEnvelope(http.MethodPost, "/x", parity.HeaderList{{"Content-Encoding", "gzip"}}, &parity.Body{Text: parity.TextBody(`{"id":"gz"}`).Text, ContentEncoding: "gzip"})
	tr := execTrace(t, h, env)
	require.NotNil(t, tr.HTTPIn.Body)
	assert.Equal(t, "gzip", tr.HTTPIn.Body.ContentEncoding)
	assert.JSONEq(t, `{"id":"gz"}`, string(tr.HTTPIn.Body.JSON))
}

// tracedApp records a stage, a decision, a metric and one inline-mocked
// outbound bidder call, then answers with the faked clock.
func tracedApp(t *testing.T, counter prometheus.Counter) http.Handler {
	t.Helper()
	client := &http.Client{}
	wiring.WrapClient(client, parity.OutboundGeneral, &config.Configuration{Parity: config.ParityConfig{Enabled: true, Token: testToken}})
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s := trace.Current()
		s.Record(parity.StageRequestValidated, parity.UnkeyedStage, map[string]string{"id": "req"})
		s.Errors(parity.StageRequestValidated, parity.UnkeyedStage, nil)
		skip := s.DecideBool(parity.DecisionFloorsSkip, func() bool { return true })
		counter.Inc()
		out, err := http.NewRequestWithContext(trace.WithBidder(r.Context(), "appnexus"), http.MethodPost, "https://bidder.example/bid", strings.NewReader(`{"id":"req"}`))
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		resp, err := client.Do(out)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}
		bidderBody, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintf(w, `{"now":%d,"skip":%t,"bidder":%s}`, fakes.Now().UnixMilli(), skip, bidderBody)
	})
}

func TestExecTrace(t *testing.T) {
	reg := prometheus.NewRegistry()
	counter := prometheus.NewCounter(prometheus.CounterOpts{Name: "requests", Help: "test"})
	reg.MustRegister(counter)
	h := newServer(wiring.Deps{Gatherer: reg}, tracedApp(t, counter))

	env := testEnvelope(http.MethodPost, EndpointAuction, parity.HeaderList{{"Content-Type", "application/json"}}, parity.TextBody(`{"id":"req"}`))
	env.Mock = parity.MockSpec{Mode: parity.MockModeInline, Inline: &parity.InlineMocks{Bidders: map[string]parity.InlineResponse{
		"appnexus": {Status: http.StatusOK, Headers: parity.HeaderList{{"Content-Type", "application/json"}}, Body: parity.TextBody(`{"seatbid":[]}`)},
	}}}
	env.Decisions = map[string]json.RawMessage{string(parity.DecisionFloorsSkip): json.RawMessage(`0`)}
	env.Fakes = &parity.FakeValues{NowUnixMS: 1700000000000}

	tr := execTrace(t, h, env)
	assert.Equal(t, parity.SchemaTrace, tr.Schema)
	assert.Equal(t, parity.ImplGo, tr.Impl)
	assert.Equal(t, env.CaseID, tr.CaseID)
	assert.Equal(t, EndpointAuction, tr.Endpoint)
	assert.Empty(t, tr.Notes)
	assert.JSONEq(t, `{"now":1700000000000,"skip":false,"bidder":{"seatbid":[]}}`, string(tr.HTTPResponse.Body.JSON))

	for _, stage := range []parity.StageID{parity.StageHTTPIn, parity.StageRequestValidated, parity.StageHTTPOut, parity.StageMetricsDelta, parity.StageHTTPResponse} {
		assert.Equal(t, parity.StageCaptured, tr.StageStatus[stage], "stage %s", stage)
	}
	assert.Equal(t, parity.StageNotReached, tr.StageStatus[parity.StageBidderRequest])
	assert.Len(t, tr.StageStatus, len(parity.AllStages()))

	entry := tr.Stages[parity.StageRequestValidated][parity.UnkeyedStage]
	assert.JSONEq(t, `{"id":"req"}`, string(entry.JSON))
	assert.Equal(t, []parity.ErrorRecord{}, entry.Errors)

	require.Len(t, tr.HTTPOut, 1)
	assert.Equal(t, parity.OutboundBidder, tr.HTTPOut[0].Kind)
	assert.Equal(t, "appnexus", tr.HTTPOut[0].Bidder)
	assert.False(t, tr.HTTPOut[0].Wire)

	require.Len(t, tr.Decisions, 1)
	assert.Equal(t, parity.DecisionSourceEnvelope, tr.Decisions[0].Source)

	require.Len(t, tr.MetricsDelta, 1)
	assert.Equal(t, "requests", tr.MetricsDelta[0].Name)
	assert.Equal(t, float64(1), tr.MetricsDelta[0].Delta)

	assert.Equal(t, fakes.Defaults(), fakes.Active().Values(), "the case's fakes are reset after the case")
	assert.Nil(t, trace.Current(), "the session is unpublished after the case")
}

func TestExecCapture(t *testing.T) {
	reg := prometheus.NewRegistry()
	counter := prometheus.NewCounter(prometheus.CounterOpts{Name: "requests", Help: "test"})
	reg.MustRegister(counter)
	off := false
	tests := []struct {
		name         string
		gatherer     prometheus.Gatherer
		capture      parity.CaptureSpec
		wantIn       bool
		wantResponse bool
		wantMetrics  parity.StageStatus
		wantNote     string
	}{
		{name: "everything by default", gatherer: reg, wantIn: true, wantResponse: true, wantMetrics: parity.StageCaptured},
		{name: "stage list", gatherer: reg, capture: parity.CaptureSpec{Stages: json.RawMessage(`["http.response"]`)}, wantResponse: true, wantMetrics: parity.StageNotReached},
		{name: "metrics off", gatherer: reg, capture: parity.CaptureSpec{Metrics: &off}, wantIn: true, wantResponse: true, wantMetrics: parity.StageNotReached},
		{name: "no prometheus registry", wantIn: true, wantResponse: true, wantMetrics: parity.StageNotReached, wantNote: "metrics_delta not captured"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			app := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { counter.Inc() })
			h := newServer(wiring.Deps{Gatherer: tt.gatherer}, app)
			env := testEnvelope(http.MethodGet, "/x", nil, nil)
			env.Capture = tt.capture
			tr := execTrace(t, h, env)
			assert.Equal(t, tt.wantIn, tr.HTTPIn != nil)
			assert.Equal(t, tt.wantResponse, tr.HTTPResponse != nil)
			assert.Equal(t, tt.wantMetrics, tr.StageStatus[parity.StageMetricsDelta])
			assert.Equal(t, tt.wantMetrics == parity.StageCaptured, len(tr.MetricsDelta) == 1)
			if tt.wantNote != "" {
				require.Len(t, tr.Notes, 1)
				assert.Contains(t, tr.Notes[0], tt.wantNote)
			} else {
				assert.Empty(t, tr.Notes)
			}
		})
	}
}

func TestExecPanic(t *testing.T) {
	tests := []struct {
		name     string
		app      http.HandlerFunc
		wantNote string
	}{
		{
			name: "handler panic",
			app: func(w http.ResponseWriter, _ *http.Request) {
				w.Header().Set("X-Before", "1")
				io.WriteString(w, "partial")
				panic("boom")
			},
			wantNote: "panic: boom",
		},
		{
			name:     "invalid WriteHeader code",
			app:      func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(42) },
			wantNote: "panic: invalid WriteHeader code 42",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			h := newServer(wiring.Deps{Gatherer: prometheus.NewRegistry()}, tt.app)
			for i := 0; i < 2; i++ {
				tr := execTrace(t, h, testEnvelope(http.MethodGet, "/x", nil, nil))
				assert.Equal(t, http.StatusInternalServerError, tr.HTTPResponse.Status)
				assert.Equal(t, parity.HeaderList{}, tr.HTTPResponse.Headers)
				assert.Nil(t, tr.HTTPResponse.Body)
				assert.Equal(t, []string{tt.wantNote}, tr.Notes)
				assert.Equal(t, parity.StageCaptured, tr.StageStatus[parity.StageHTTPResponse])
			}
			assert.Nil(t, trace.Current(), "the exec lock and session are released after a panic")
		})
	}
}

func TestExecHEADDropsBody(t *testing.T) {
	app := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { io.WriteString(w, "<html>head</html>") })
	tr := execTrace(t, newServer(wiring.Deps{}, app), testEnvelope(http.MethodHead, "/x", nil, nil))
	assert.Equal(t, http.StatusOK, tr.HTTPResponse.Status)
	assert.Nil(t, tr.HTTPResponse.Body)
	assert.Equal(t, parity.HeaderList{{"Content-Type", "text/html; charset=utf-8"}}, tr.HTTPResponse.Headers)
}

func TestExecInvalidEnvelope(t *testing.T) {
	h := newServer(wiring.Deps{}, http.HandlerFunc(echoHandler))
	valid := func() map[string]any {
		return map[string]any{
			"schema": parity.SchemaEnvelope, "schema_version": parity.SchemaVersion, "case_id": "c",
			"request": map[string]any{"method": "GET", "path": "/x", "query": "", "headers": [][2]string{}},
			"mock":    map[string]any{}, "capture": map[string]any{},
		}
	}
	request := func(doc map[string]any) map[string]any { return doc["request"].(map[string]any) }
	tests := []struct {
		name string
		edit func(map[string]any)
		raw  string
		want string
	}{
		{name: "not json", raw: `{`, want: "decoding document"},
		{name: "trailing data", raw: `{"schema":"x"} {}`, want: "trailing data"},
		{name: "unknown member", edit: func(d map[string]any) { d["extra"] = 1 }, want: "unknown field"},
		{name: "wrong schema", edit: func(d map[string]any) { d["schema"] = parity.SchemaTrace }, want: "schema must be"},
		{name: "unknown major version", edit: func(d map[string]any) { d["schema_version"] = "2.0.0" }, want: "unsupported schema_version"},
		{name: "malformed version", edit: func(d map[string]any) { d["schema_version"] = "1.0" }, want: "unsupported schema_version"},
		{name: "missing case id", edit: func(d map[string]any) { d["case_id"] = "" }, want: "case_id is required"},
		{name: "empty method", edit: func(d map[string]any) { request(d)["method"] = "" }, want: "request.method"},
		{name: "method with space", edit: func(d map[string]any) { request(d)["method"] = "G ET" }, want: "request.method"},
		{name: "relative path", edit: func(d map[string]any) { request(d)["path"] = "x" }, want: "must start with /"},
		{name: "invalid header name", edit: func(d map[string]any) { request(d)["headers"] = [][2]string{{"Bad Name", "v"}} }, want: "invalid header name"},
		{name: "header value with newline", edit: func(d map[string]any) { request(d)["headers"] = [][2]string{{"X-A", "a\r\nX-B: b"}} }, want: "CR, LF or NUL"},
		{name: "bad b64 body", edit: func(d map[string]any) { request(d)["body"] = map[string]any{"b64": "%%%"} }, want: "request.body"},
		{name: "unsupported body encoding", edit: func(d map[string]any) { request(d)["body"] = map[string]any{"text": "x", "content_encoding": "br"} }, want: "request.body"},
		{name: "content length mismatch", edit: func(d map[string]any) {
			request(d)["headers"] = [][2]string{{"Content-Length", "9"}}
			request(d)["body"] = map[string]any{"text": "x"}
		}, want: "Content-Length"},
		{name: "unsupported transfer encoding", edit: func(d map[string]any) { request(d)["headers"] = [][2]string{{"Transfer-Encoding", "gzip"}} }, want: "Transfer-Encoding"},
		{name: "bad mock mode", edit: func(d map[string]any) { d["mock"] = map[string]any{"mode": "live"} }, want: "mock.mode"},
		{name: "bad mock level", edit: func(d map[string]any) { d["mock"] = map[string]any{"level": "adapter"} }, want: "mock.level"},
		{name: "bad inline kind", edit: func(d map[string]any) {
			d["mock"] = map[string]any{"inline": map[string]any{"kinds": map[string]any{"ftp": map[string]any{}}}}
		}, want: "unknown outbound kind"},
		{name: "unknown decision", edit: func(d map[string]any) { d["decisions"] = map[string]any{"coin.flip": 1} }, want: "unknown decision point"},
		{name: "bad capture string", edit: func(d map[string]any) { d["capture"] = map[string]any{"stages": "all"} }, want: "capture.stages"},
		{name: "unknown capture stage", edit: func(d map[string]any) { d["capture"] = map[string]any{"stages": []string{"exchange.magic"}} }, want: "unknown stage"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			raw := []byte(tt.raw)
			if tt.edit != nil {
				doc := valid()
				tt.edit(doc)
				raw = mustJSON(t, doc)
			}
			assertError(t, call(t, h, http.MethodPost, PathExec, raw), http.StatusBadRequest, tt.want)
		})
	}
	t.Run("the valid base document is accepted", func(t *testing.T) {
		rec := call(t, h, http.MethodPost, PathExec, mustJSON(t, valid()))
		assert.Equal(t, http.StatusOK, rec.Code, rec.Body.String())
	})
}

func TestExecAliases(t *testing.T) {
	h := newServer(wiring.Deps{}, http.HandlerFunc(echoHandler))
	tests := []struct {
		name       string
		method     string
		path       string
		body       string
		wantStatus int
		wantCase   string
		wantMethod string
		wantPath   string
		wantQuery  string
		wantError  string
	}{
		{name: "auction with defaults", method: http.MethodPost, path: PathAuction, body: `{"request":{"headers":[],"body":{"text":"{}"}}}`, wantStatus: http.StatusOK, wantCase: "alias:auction", wantMethod: http.MethodPost, wantPath: EndpointAuction},
		{name: "video keeps the case id", method: http.MethodPost, path: PathVideo, body: `{"case_id":"mine","request":{"method":"POST","path":"/openrtb2/video","headers":[]}}`, wantStatus: http.StatusOK, wantCase: "mine", wantMethod: http.MethodPost, wantPath: EndpointVideo},
		{name: "amp envelope", method: http.MethodPost, path: PathAMP, body: `{"request":{"query":"tag_id=1","headers":[]}}`, wantStatus: http.StatusOK, wantCase: "alias:amp", wantMethod: http.MethodGet, wantPath: EndpointAMP, wantQuery: "tag_id=1"},
		{name: "amp query form", method: http.MethodGet, path: PathAMP + "?tag_id=2&w=300", wantStatus: http.StatusOK, wantCase: "alias:amp", wantMethod: http.MethodGet, wantPath: EndpointAMP, wantQuery: "tag_id=2&w=300"},
		{name: "path must match the alias", method: http.MethodPost, path: PathAuction, body: `{"request":{"path":"/openrtb2/video","headers":[]}}`, wantStatus: http.StatusBadRequest, wantError: "always sends POST /openrtb2/auction"},
		{name: "method must match the alias", method: http.MethodPost, path: PathAMP, body: `{"request":{"method":"POST","headers":[]}}`, wantStatus: http.StatusBadRequest, wantError: "always sends GET /openrtb2/amp"},
		{name: "auction rejects GET", method: http.MethodGet, path: PathAuction, wantStatus: http.StatusMethodNotAllowed, wantError: "does not accept GET"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(tt.method, tt.path, strings.NewReader(tt.body))
			req.Header.Set(parity.TokenHeader, testToken)
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, req)
			if tt.wantError != "" {
				assertError(t, rec, tt.wantStatus, tt.wantError)
				return
			}
			var tr parity.Trace
			decodeDoc(t, rec, tt.wantStatus, &tr)
			assert.Equal(t, tt.wantCase, tr.CaseID)
			var got echoed
			require.NoError(t, json.Unmarshal(tr.HTTPResponse.Body.JSON, &got))
			assert.Equal(t, tt.wantMethod, got.Method)
			assert.Equal(t, tt.wantPath, got.Path)
			assert.Equal(t, tt.wantQuery, got.RawQuery)
			assert.NotContains(t, got.Header, parity.TokenHeader, "the parity token is never forwarded")
		})
	}
}

func TestExecGivesUpWaitingForTheLock(t *testing.T) {
	_, end := trace.Begin(&parity.Envelope{CaseID: "in flight"})
	defer end()
	h := newServer(wiring.Deps{}, http.HandlerFunc(echoHandler))
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	req := httptest.NewRequest(http.MethodPost, PathExec, bytes.NewReader(mustJSON(t, testEnvelope(http.MethodGet, "/x", nil, nil)))).WithContext(ctx)
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	assertError(t, rec, http.StatusServiceUnavailable, "exec lock")
}
