package oracle

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/prebid/go-gdpr/vendorlist2"
	"github.com/prebid/openrtb/v20/openrtb2"
	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/exchange"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

var (
	adapterCorpusOnce sync.Once
	adapterCorpus     *corpus.AdapterCorpus
	adapterCorpusErr  error
)

func testRepo(t *testing.T) *corpus.Repo {
	t.Helper()
	repo, err := corpus.FindRepo(".")
	require.NoError(t, err)
	return repo
}

// loadAdapterCorpus loads the adapter fixtures once per test binary.
func loadAdapterCorpus(t *testing.T) *corpus.AdapterCorpus {
	t.Helper()
	adapterCorpusOnce.Do(func() {
		repo, err := corpus.FindRepo(".")
		if err != nil {
			adapterCorpusErr = err
			return
		}
		adapterCorpus, adapterCorpusErr = corpus.LoadAdapterCorpus(repo)
	})
	require.NoError(t, adapterCorpusErr)
	return adapterCorpus
}

// startOracle starts an oracle over the repository fixtures.
func startOracle(t *testing.T, opts Options) *Oracle {
	t.Helper()
	opts.RepoRoot = testRepo(t).Root()
	o, err := Start(opts)
	require.NoError(t, err)
	t.Cleanup(func() { assert.NoError(t, o.Close()) })
	return o
}

// proxyClient is a Go client whose transport uses the oracle as its proxy.
func proxyClient(t *testing.T, o *Oracle, timeout time.Duration) *http.Client {
	t.Helper()
	proxy, err := url.Parse(o.URL())
	require.NoError(t, err)
	tr := http.DefaultTransport.(*http.Transport).Clone()
	tr.Proxy = http.ProxyURL(proxy)
	t.Cleanup(tr.CloseIdleConnections)
	return &http.Client{Transport: tr, Timeout: timeout}
}

func metaHeader(t *testing.T, meta parity.OutboundMeta) string {
	t.Helper()
	meta.V = transport.MetaVersion
	v, err := transport.EncodeMeta(meta)
	require.NoError(t, err)
	return v
}

// post sends a JSON POST through the oracle carrying meta.
func post(t *testing.T, client *http.Client, target string, meta parity.OutboundMeta, body string) (*http.Response, []byte, error) {
	t.Helper()
	req, err := http.NewRequest(http.MethodPost, target, strings.NewReader(body))
	require.NoError(t, err)
	req.Header.Set("Content-Type", "application/json")
	meta.OrigURL = target
	req.Header.Set(parity.MetaHeader, metaHeader(t, meta))
	resp, err := client.Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	require.NoError(t, err)
	return resp, raw, nil
}

func drain(t *testing.T, o *Oracle, caseID, impl string) []Capture {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	caps, err := o.Drain(ctx, caseID, impl)
	require.NoError(t, err)
	return caps
}

func TestCacheUUIDsAreContentAddressed(t *testing.T) {
	a := `{"type":"json","ttlseconds":300,"value":{"id":"bid-a","price":1.5}}`
	aReordered := `{ "value" : {"price":1.5,"id":"bid-a"}, "ttlseconds":300, "type":"json" }`
	b := `{"type":"xml","value":"<VAST version=\"3.0\"></VAST>"}`
	keyed := `{"type":"json","value":{},"key":"custom-key-1"}`

	first, err := CacheUUIDs([]byte(`{"puts":[` + a + `,` + b + `]}`))
	require.NoError(t, err)
	second, err := CacheUUIDs([]byte(`{"puts":[` + b + `,` + aReordered + `]}`))
	require.NoError(t, err)
	require.Len(t, first, 2)
	assert.Equal(t, first[0], second[1], "same put content gets the same uuid at any position and key order")
	assert.Equal(t, first[1], second[0])
	assert.NotEqual(t, first[0], first[1])

	lexical, err := CacheUUIDs([]byte(`{"puts":[{"type":"json","ttlseconds":300,"value":{"id":"bid-a","price":1.50}}]}`))
	require.NoError(t, err)
	assert.NotEqual(t, first[0], lexical[0], "number text is part of the content")

	withKey, err := CacheUUIDs([]byte(`{"puts":[` + keyed + `]}`))
	require.NoError(t, err)
	assert.Equal(t, []string{"custom-key-1"}, withKey)

	empty, err := CacheUUIDs([]byte(`{"puts":[]}`))
	require.NoError(t, err)
	assert.Empty(t, empty)
	_, err = CacheUUIDs([]byte(`{"puts":`))
	assert.Error(t, err)
}

func TestContentUUIDFormat(t *testing.T) {
	id := ContentUUID([]byte(`{}`))
	require.Len(t, id, 36)
	assert.Equal(t, byte('8'), id[14], "RFC 9562 version 8")
	assert.Contains(t, "89ab", string(id[19]), "RFC 9562 variant")
	assert.Equal(t, id, ContentUUID([]byte(`{}`)))
}

func TestCanonicalJSON(t *testing.T) {
	tests := []struct {
		in, want string
		wantErr  bool
	}{
		{`{"b":1,"a":[true,null,"x"]}`, `{"a":[true,null,"x"],"b":1}`, false},
		{` { "n" : 1.50e3 } `, `{"n":1.50e3}`, false},
		{`"<a>& é"`, "\"<a>& é\"", false},
		{`"\b\f\n\r\t\u0001\"\\/"`, `"\b\f\n\r\t\u0001\"\\/"`, false},
		{`{"é":1,"z":2,"A":3}`, `{"A":3,"z":2,"é":1}`, false},
		{`{"a":1} {"b":2}`, ``, true},
		{`{bad`, ``, true},
	}
	for _, tt := range tests {
		t.Run(tt.in, func(t *testing.T) {
			got, err := CanonicalJSON([]byte(tt.in))
			if tt.wantErr {
				assert.Error(t, err)
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, string(got))
		})
	}
}

func TestTemplatePrice(t *testing.T) {
	seen := map[float64]bool{}
	for i := 0; i < 50; i++ {
		p := TemplatePrice("case", "appnexus", fmt.Sprintf("imp-%d", i))
		assert.GreaterOrEqual(t, p, 0.01)
		assert.LessOrEqual(t, p, 20.0)
		assert.Equal(t, p, TemplatePrice("case", "appnexus", fmt.Sprintf("imp-%d", i)))
		assert.Equal(t, p, float64(int64(p*100+0.5))/100, "two decimals")
		seen[p] = true
	}
	assert.Greater(t, len(seen), 40, "prices spread over imps")
	assert.NotEqual(t, TemplatePrice("case-a", "appnexus", "1"), TemplatePrice("case-b", "appnexus", "1"))
}

func TestProxyCapturesGoTransportWire(t *testing.T) {
	o := startOracle(t, Options{})
	client := proxyClient(t, o, 5*time.Second)

	var gz bytes.Buffer
	zw := gzip.NewWriter(&gz)
	_, err := zw.Write([]byte(`{"puts":[{"type":"json","value":{"a":1}}]}`))
	require.NoError(t, err)
	require.NoError(t, zw.Close())
	req, err := http.NewRequest(http.MethodPost, "http://prebid-cache.example/cache", bytes.NewReader(gz.Bytes()))
	require.NoError(t, err)
	req.Header.Set("Content-Encoding", "gzip")
	req.Header["x-lower-case"] = []string{"kept"}
	req.Header.Set(parity.MetaHeader, metaHeader(t, parity.OutboundMeta{Case: "c1", Impl: parity.ImplGo, Kind: parity.OutboundCache, OrigURL: "https://prebid-cache.example/cache", Seq: 3}))
	resp, err := client.Do(req)
	require.NoError(t, err)
	body, err := io.ReadAll(resp.Body)
	require.NoError(t, err)
	resp.Body.Close()
	require.Equal(t, http.StatusOK, resp.StatusCode)
	var cacheResp struct {
		Responses []struct {
			UUID string `json:"uuid"`
		} `json:"responses"`
	}
	require.NoError(t, json.Unmarshal(body, &cacheResp))
	require.Len(t, cacheResp.Responses, 1)

	caps := drain(t, o, "c1", parity.ImplGo)
	require.Len(t, caps, 1)
	c := caps[0]
	assert.Equal(t, 3, c.Call.Seq)
	assert.Equal(t, parity.OutboundCache, c.Call.Kind)
	assert.Equal(t, "https://prebid-cache.example/cache", c.Call.OrigURL)
	assert.Equal(t, "http://prebid-cache.example/cache", c.Call.Request.URL)
	assert.Equal(t, "HTTP/1.1", c.Call.Request.Proto)
	assert.True(t, c.Call.Wire)
	ua, _ := headerValue(c.Call.Request.Header, "User-Agent")
	assert.Equal(t, "Go-http-client/1.1", ua)
	ae, _ := headerValue(c.Call.Request.Header, "Accept-Encoding")
	assert.Equal(t, "gzip", ae)
	_, hasMeta := headerValue(c.Call.Request.Header, parity.MetaHeader)
	assert.False(t, hasMeta, "X-Parity-Meta is stripped")
	assert.Contains(t, c.Call.Request.Header, [2]string{"x-lower-case", "kept"}, "header name case is kept")
	require.NotNil(t, c.Call.Request.Body)
	assert.NotNil(t, c.Call.Request.Body.B64, "content-encoded bodies keep their raw bytes")
	assert.JSONEq(t, `{"puts":[{"type":"json","value":{"a":1}}]}`, string(c.Call.Request.Body.JSON))
	require.NotNil(t, c.Decoded)
	assert.Equal(t, parity.MockSourceDefault, c.Call.Mock.Source)
	assert.Equal(t, cacheResp.Responses[0].UUID, mustUUIDs(t, `{"puts":[{"value":{"a":1},"type":"json"}]}`)[0])

	raw, err := json.Marshal(c.Call)
	require.NoError(t, err)
	assert.NoError(t, schema.ValidateDefinition(schema.FileCommon, "OutboundCall", raw))
}

func mustUUIDs(t *testing.T, body string) []string {
	t.Helper()
	ids, err := CacheUUIDs([]byte(body))
	require.NoError(t, err)
	return ids
}

// rawExchange writes raw request bytes and reads one response.
func rawExchange(t *testing.T, conn net.Conn, br *bufio.Reader, raw string) *http.Response {
	t.Helper()
	_, err := io.WriteString(conn, raw)
	require.NoError(t, err)
	resp, err := http.ReadResponse(br, nil)
	require.NoError(t, err)
	_, err = io.ReadAll(resp.Body)
	require.NoError(t, err)
	resp.Body.Close()
	return resp
}

func TestProxyKeepsRawHeaderOrderAndChunkedBodies(t *testing.T) {
	o := startOracle(t, Options{})
	conn, err := net.Dial("tcp", strings.TrimPrefix(o.URL(), "http://"))
	require.NoError(t, err)
	defer conn.Close()
	br := bufio.NewReader(conn)
	meta := metaHeader(t, parity.OutboundMeta{Case: "raw", Impl: parity.ImplRust, Kind: parity.OutboundCache, OrigURL: "http://cache.test/cache", Seq: 1})

	resp := rawExchange(t, conn, br, "POST http://cache.test/cache HTTP/1.1\r\n"+
		"host: cache.test\r\n"+
		"x-parity-meta: "+meta+"\r\n"+
		"accept: */*\r\n"+
		"Content-Type: application/json\r\n"+
		"transfer-encoding: chunked\r\n\r\n"+
		"5\r\n{\"put\r\n8;ext=1\r\ns\":[{}]}\r\n0\r\nX-Trailer: t\r\n\r\n")
	assert.Equal(t, 200, resp.StatusCode)

	meta2 := metaHeader(t, parity.OutboundMeta{Case: "raw", Impl: parity.ImplRust, Kind: parity.OutboundStored, OrigURL: "http://stored.test/x", Seq: 2})
	resp = rawExchange(t, conn, br, "GET http://stored.test/x HTTP/1.1\r\nHost: stored.test\r\nX-Parity-Meta: "+meta2+"\r\n\r\n")
	assert.Equal(t, 404, resp.StatusCode, "the keep-alive connection serves a second request")

	caps := drain(t, o, "raw", parity.ImplRust)
	require.Len(t, caps, 2)
	assert.Equal(t, parity.HeaderList{
		{"host", "cache.test"},
		{"accept", "*/*"},
		{"Content-Type", "application/json"},
		{"transfer-encoding", "chunked"},
	}, caps[0].Call.Request.Header)
	assert.True(t, caps[0].Chunked)
	require.NotNil(t, caps[0].Call.Request.Body)
	assert.Equal(t, `{"puts":[{}]}`, *caps[0].Call.Request.Body.Text)
	assert.True(t, caps[1].Unexpected)
	assert.Contains(t, caps[1].Call.Notes, NoteUnexpectedOutbound)
}

func TestProxyRejectsMalformedRequests(t *testing.T) {
	tests := []struct {
		name string
		raw  string
	}{
		{"bad request line", "NONSENSE\r\n\r\n"},
		{"folded header", "GET http://a/ HTTP/1.1\r\nX-A: 1\r\n continued\r\n\r\n"},
		{"bad content length", "POST http://a/ HTTP/1.1\r\nContent-Length: -1\r\n\r\n"},
		{"bad chunk", "POST http://a/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"},
		{"chunk longer than its size", "POST http://a/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabc\r\n0\r\n\r\n"},
		{"transfer coding not chunked", "POST http://a/ HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n"},
		{"http2 preface", "PRI * HTTP/2.0\r\n\r\n"},
	}
	o := startOracle(t, Options{})
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			conn, err := net.Dial("tcp", strings.TrimPrefix(o.URL(), "http://"))
			require.NoError(t, err)
			defer conn.Close()
			_, err = io.WriteString(conn, tt.raw)
			require.NoError(t, err)
			resp, err := http.ReadResponse(bufio.NewReader(conn), nil)
			require.NoError(t, err)
			resp.Body.Close()
			assert.Equal(t, http.StatusBadRequest, resp.StatusCode)
		})
	}
}

func TestProxyConnectIsRejected(t *testing.T) {
	o := startOracle(t, Options{})
	conn, err := net.Dial("tcp", strings.TrimPrefix(o.URL(), "http://"))
	require.NoError(t, err)
	defer conn.Close()
	resp := rawExchange(t, conn, bufio.NewReader(conn), "CONNECT bidder.test:443 HTTP/1.1\r\nHost: bidder.test:443\r\n\r\n")
	assert.Equal(t, http.StatusMethodNotAllowed, resp.StatusCode)
	caps := drain(t, o, "", "")
	require.Len(t, caps, 1)
	assert.True(t, caps[0].Unexpected)
}

func TestDelayedResponseRespectsClientDeadline(t *testing.T) {
	o := startOracle(t, Options{})
	require.NoError(t, o.Register(&Plan{CaseID: "slow", Rules: []Rule{{
		ID: "slow-bidder", Bidder: "appnexus",
		Response: parity.InlineResponse{Status: 200, Body: parity.TextBody(`{}`), DelayMS: 3000},
	}}}))
	client := proxyClient(t, o, 200*time.Millisecond)
	start := time.Now()
	_, _, err := post(t, client, "http://bidder.test/rtb", parity.OutboundMeta{Case: "slow", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "appnexus", Seq: 1}, `{}`)
	require.Error(t, err)
	assert.Less(t, time.Since(start), 2*time.Second, "the client deadline wins")
	caps := drain(t, o, "slow", parity.ImplGo)
	assert.Less(t, time.Since(start), 2500*time.Millisecond, "the oracle stops waiting when the client closes")
	require.Len(t, caps, 1)
	require.NotNil(t, caps[0].Call.Error)
	assert.Equal(t, parity.OutboundErrTimeout, *caps[0].Call.Error)
	assert.Nil(t, caps[0].Call.Response)
	assert.Equal(t, parity.MockRef{Source: parity.MockSourceRule, ID: "slow-bidder"}, *caps[0].Call.Mock)
}

func TestDelayedResponseIsDelivered(t *testing.T) {
	o := startOracle(t, Options{})
	require.NoError(t, o.Register(&Plan{CaseID: "delay", Rules: []Rule{{
		ID: "r", Response: parity.InlineResponse{Status: 200, Body: parity.TextBody(`{"ok":true}`), DelayMS: 150},
	}}}))
	client := proxyClient(t, o, 5*time.Second)
	start := time.Now()
	resp, body, err := post(t, client, "http://bidder.test/rtb", parity.OutboundMeta{Case: "delay", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "x"}, `{}`)
	require.NoError(t, err)
	assert.GreaterOrEqual(t, time.Since(start), 150*time.Millisecond)
	assert.Equal(t, 200, resp.StatusCode)
	assert.JSONEq(t, `{"ok":true}`, string(body))
	caps := drain(t, o, "delay", parity.ImplGo)
	require.Len(t, caps, 1)
	assert.Equal(t, 150, caps[0].Call.Response.DelayMS)
}

func TestFailures(t *testing.T) {
	tests := []string{parity.OutboundErrTimeout, parity.OutboundErrReset, parity.OutboundErrRefused, parity.OutboundErrOther}
	o := startOracle(t, Options{TimeoutHold: 2 * time.Second})
	for _, fail := range tests {
		t.Run(fail, func(t *testing.T) {
			f := fail
			caseID := "fail-" + fail
			require.NoError(t, o.Register(&Plan{CaseID: caseID, Rules: []Rule{{ID: f, Response: parity.InlineResponse{Fail: &f}}}}))
			client := proxyClient(t, o, 500*time.Millisecond)
			_, _, err := post(t, client, "http://bidder.test/rtb", parity.OutboundMeta{Case: caseID, Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "x"}, `{}`)
			assert.Error(t, err)
			caps := drain(t, o, caseID, parity.ImplGo)
			require.NotEmpty(t, caps)
			require.NotNil(t, caps[0].Call.Error)
			assert.Equal(t, f, *caps[0].Call.Error)
			assert.Nil(t, caps[0].Call.Response)
		})
	}
}

func TestStaticCurrencyAndGVL(t *testing.T) {
	o := startOracle(t, Options{})
	client := proxyClient(t, o, 5*time.Second)
	get := func(target string, meta *parity.OutboundMeta) (int, []byte) {
		req, err := http.NewRequest(http.MethodGet, target, nil)
		require.NoError(t, err)
		if meta != nil {
			m := *meta
			m.OrigURL = target
			req.Header.Set(parity.MetaHeader, metaHeader(t, m))
		}
		resp, err := client.Do(req)
		require.NoError(t, err)
		defer resp.Body.Close()
		body, err := io.ReadAll(resp.Body)
		require.NoError(t, err)
		return resp.StatusCode, body
	}
	startup := &parity.OutboundMeta{Impl: parity.ImplGo, Kind: parity.OutboundVendorList}

	status, body := get("http://vendor-list.consensu.org/v3/vendor-list.json", startup)
	require.Equal(t, 200, status)
	latest, err := vendorlist2.ParseEagerly(body)
	require.NoError(t, err)
	assert.Equal(t, uint16(3), latest.SpecVersion())

	status, body = get("http://vendor-list.consensu.org/v2/archives/vendor-list-v5.json", startup)
	require.Equal(t, 200, status)
	archive, err := vendorlist2.ParseEagerly(body)
	require.NoError(t, err)
	assert.Equal(t, uint16(5), archive.Version())
	assert.NotNil(t, archive.Vendor(32), "bidder-info gvlVendorIDs are vendors (appnexus is 32)")

	status, _ = get(fmt.Sprintf("http://vendor-list.consensu.org/v2/archives/vendor-list-v%d.json", latest.Version()+1), startup)
	assert.Equal(t, 404, status)

	status, body = get("http://127.0.0.1:1/currency/latest.json", &parity.OutboundMeta{Impl: parity.ImplGo, Kind: parity.OutboundCurrency})
	require.Equal(t, 200, status)
	assert.Contains(t, string(body), `"dataAsOf"`)

	profileURL, err := o.RegisterCurrency("p1", map[string]map[string]float64{"USD": {"MXN": 20}}, "2020-01-01")
	require.NoError(t, err)
	status, body = get(profileURL, &parity.OutboundMeta{Impl: parity.ImplGo, Kind: parity.OutboundCurrency})
	require.Equal(t, 200, status)
	assert.JSONEq(t, `{"dataAsOf":"2020-01-01","conversions":{"USD":{"MXN":20}}}`, string(body))

	status, _ = get("http://unknown.example/x", nil)
	assert.Equal(t, 404, status)

	caps := drain(t, o, "", parity.ImplGo)
	require.Len(t, caps, 5)
	assert.Equal(t, parity.MockSourceStatic, caps[0].Call.Mock.Source)
	assert.Contains(t, caps[2].Call.Notes, "gvl_version_missing")
	assert.False(t, caps[2].Unexpected)
	noMeta := drain(t, o, "", "")
	require.Len(t, noMeta, 1)
	assert.True(t, noMeta[0].Unexpected)
	assert.Contains(t, noMeta[0].Call.Notes, NoteNoMeta)
}

func TestKindDispatchWithoutRules(t *testing.T) {
	floors := parity.InlineResponse{Status: 200, Body: parity.TextBody(`{"data":{}}`)}
	tests := []struct {
		name       string
		kind       string
		plan       *Plan
		status     int
		source     string
		unexpected bool
	}{
		{"floors from plan", parity.OutboundFloors, &Plan{Floors: &floors}, 200, parity.MockSourceRule, false},
		{"floors missing", parity.OutboundFloors, &Plan{}, 404, parity.MockSourceDefault, false},
		{"stored unexpected", parity.OutboundStored, &Plan{}, 404, parity.MockSourceDefault, true},
		{"module unexpected", parity.OutboundModule, nil, 404, parity.MockSourceDefault, true},
		{"bidder default", parity.OutboundBidder, &Plan{DisableTemplates: true}, 204, parity.MockSourceDefault, false},
	}
	o := startOracle(t, Options{})
	client := proxyClient(t, o, 5*time.Second)
	for i, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			caseID := fmt.Sprintf("dispatch-%d", i)
			if tt.plan != nil {
				tt.plan.CaseID = caseID
				require.NoError(t, o.Register(tt.plan))
			}
			resp, _, err := post(t, client, "http://host.test/path", parity.OutboundMeta{Case: caseID, Impl: parity.ImplGo, Kind: tt.kind, Bidder: "nobidder"}, `{}`)
			require.NoError(t, err)
			assert.Equal(t, tt.status, resp.StatusCode)
			caps := drain(t, o, caseID, parity.ImplGo)
			require.Len(t, caps, 1)
			assert.Equal(t, tt.source, caps[0].Call.Mock.Source)
			assert.Equal(t, tt.unexpected, caps[0].Unexpected)
		})
	}
}

func TestInvalidMetaIsFlagged(t *testing.T) {
	o := startOracle(t, Options{})
	client := proxyClient(t, o, 5*time.Second)
	req, err := http.NewRequest(http.MethodGet, "http://bidder.test/", nil)
	require.NoError(t, err)
	req.Header.Set(parity.MetaHeader, "%%%not-base64")
	resp, err := client.Do(req)
	require.NoError(t, err)
	resp.Body.Close()
	assert.Equal(t, 404, resp.StatusCode)
	caps := drain(t, o, "", "")
	require.Len(t, caps, 1)
	assert.True(t, caps[0].Unexpected)
	assert.NotEmpty(t, caps[0].MetaError)
	assert.Nil(t, caps[0].Meta)
}

func TestRuleMatchers(t *testing.T) {
	plan := &Plan{CaseID: "rules", ImpIDs: []string{"imp-1", "imp-2"}, Rules: []Rule{
		{ID: "by-path", Bidder: "b", JSONEquals: []PathEquals{{Path: "$.imp[0].ext.bidder.placementId", Value: json.RawMessage(`12`)}}, Response: parity.InlineResponse{Status: 201}},
		{ID: "by-imps", Bidder: "b", ImpIDs: []string{"imp-2", "imp-1"}, Response: parity.InlineResponse{Status: 202}},
		{ID: "by-uri", Kind: parity.OutboundBidder, URIRegex: `^https://special\.test/`, Response: parity.InlineResponse{Status: 203}},
	}}
	o := startOracle(t, Options{})
	require.NoError(t, o.Register(plan))
	client := proxyClient(t, o, 5*time.Second)
	tests := []struct {
		name   string
		target string
		bidder string
		body   string
		status int
	}{
		{"json path", "http://b.test/", "b", `{"imp":[{"id":"imp-1","ext":{"bidder":{"placementId":12.0}}}]}`, 201},
		{"imp set", "http://b.test/", "b", `{"imp":[{"id":"imp-1"},{"id":"imp-2"}]}`, 202},
		{"textual imp ids", "http://b.test/?ids=imp-1,imp-2", "b", `not json`, 202},
		{"uri", "https://special.test/x", "other", `{}`, 203},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req, err := http.NewRequest(http.MethodPost, strings.Replace(tt.target, "https://", "http://", 1), strings.NewReader(tt.body))
			require.NoError(t, err)
			req.Header.Set(parity.MetaHeader, metaHeader(t, parity.OutboundMeta{Case: "rules", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: tt.bidder, OrigURL: tt.target}))
			resp, err := client.Do(req)
			require.NoError(t, err)
			resp.Body.Close()
			assert.Equal(t, tt.status, resp.StatusCode)
		})
	}
	drain(t, o, "rules", parity.ImplGo)
}

func TestPlanCompileErrors(t *testing.T) {
	bad := "explode"
	tests := []struct {
		name string
		plan *Plan
	}{
		{"no case", &Plan{}},
		{"bad regex", &Plan{CaseID: "c", Rules: []Rule{{ID: "r", URIRegex: "(", Response: parity.InlineResponse{Status: 200}}}}},
		{"bad fail", &Plan{CaseID: "c", Rules: []Rule{{ID: "r", Response: parity.InlineResponse{Fail: &bad}}}}},
		{"bad status", &Plan{CaseID: "c", Rules: []Rule{{ID: "r", Response: parity.InlineResponse{Status: 0}}}}},
		{"bad path", &Plan{CaseID: "c", Rules: []Rule{{ID: "r", JSONEquals: []PathEquals{{Path: "$.a[x]", Value: json.RawMessage(`1`)}}, Response: parity.InlineResponse{Status: 200}}}}},
		{"bad value", &Plan{CaseID: "c", Rules: []Rule{{ID: "r", JSONEquals: []PathEquals{{Path: "$.a", Value: json.RawMessage(`{`)}}, Response: parity.InlineResponse{Status: 200}}}}},
		{"bad floors", &Plan{CaseID: "c", Floors: &parity.InlineResponse{Status: 99}}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			assert.Error(t, tt.plan.compile())
		})
	}
}

func TestParsePath(t *testing.T) {
	doc, err := decodeUseNumber([]byte(`{"a":{"b":[{"c":1},[2,3]]}}`))
	require.NoError(t, err)
	tests := []struct {
		path string
		want string
		ok   bool
	}{
		{"$.a.b[0].c", "1", true},
		{"a.b[1][1]", "3", true},
		{"$.a.b[2]", "", false},
		{"$.a.x", "", false},
	}
	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			steps, err := parsePath(tt.path)
			require.NoError(t, err)
			v, ok := lookupPath(doc, steps)
			assert.Equal(t, tt.ok, ok)
			if ok {
				raw, err := json.Marshal(v)
				require.NoError(t, err)
				assert.Equal(t, tt.want, string(raw))
			}
		})
	}
	for _, bad := range []string{"", "$.", "a..b", "a[1", "a[1]x"} {
		_, err := parsePath(bad)
		assert.Error(t, err, bad)
	}
}

func TestTemplateRendering(t *testing.T) {
	tmpl := &Template{
		Bidder:  "b",
		Source:  "fixture#httpCalls[0]",
		Status:  200,
		Body:    []byte(`{"id":"tmpl-req","cur":"USD","seatbid":[{"seat":"s","bid":[{"id":"bid-1","impid":"tmpl-imp","price":0.5,"adm":"<div>&</div>","ext":{"k":12345678901234567890}}]}]}`),
		ImpIDs:  []string{"tmpl-imp"},
		OpenRTB: true,
	}
	price := 2.5
	out, err := tmpl.Render(RenderInput{CaseID: "c", Bidder: "b", RequestID: "req-9", ImpIDs: []string{"a", "b"}, Params: BidderParams{Price: &price, Currency: "EUR", DealID: "deal", Seat: "seat-x"}})
	require.NoError(t, err)
	assert.JSONEq(t, `{"id":"req-9","cur":"EUR","seatbid":[{"seat":"seat-x","bid":[
		{"id":"bid-1","impid":"a","price":2.5,"dealid":"deal","adm":"<div>&</div>","ext":{"k":12345678901234567890}},
		{"id":"bid-1-2","impid":"b","price":2.5,"dealid":"deal","adm":"<div>&</div>","ext":{"k":12345678901234567890}}]}]}`, string(out))
	assert.Contains(t, string(out), "12345678901234567890", "numbers keep their text")
	assert.Contains(t, string(out), "<div>&</div>", "no HTML escaping")

	generated, err := tmpl.Render(RenderInput{CaseID: "c", Bidder: "b", ImpIDs: []string{"a"}})
	require.NoError(t, err)
	assert.Contains(t, string(generated), fmt.Sprintf(`"price":%s`, formatPrice(TemplatePrice("c", "b", "a"))))
	assert.Contains(t, string(generated), `"id":"tmpl-req"`)

	text := &Template{Body: []byte(`<ads><ad imp="x1">"x1" "x2"</ad></ads>`), ImpIDs: []string{"x1", "x2"}}
	rendered, err := text.Render(RenderInput{ImpIDs: []string{"x2", "y"}})
	require.NoError(t, err)
	assert.Equal(t, `<ads><ad imp="x2">"x2" "y"</ad></ads>`, string(rendered), "replacements happen simultaneously")
}

// fixtureRun returns the first exemplary fixture of bidder and its run.
func fixtureRun(t *testing.T, ac *corpus.AdapterCorpus, bidder string) (*corpus.AdapterFixture, *corpus.AdapterRun) {
	t.Helper()
	for _, f := range ac.Fixtures {
		if f.Bidder() == bidder && f.Dir == corpus.DirExemplary && len(f.Spec.HTTPCalls) > 0 && f.Spec.HTTPCalls[0].Response.Status == 200 {
			return f, f.Runs[0]
		}
	}
	t.Fatalf("no exemplary fixture for %s", bidder)
	return nil, nil
}

func TestTemplatesParseThroughGoMakeBids(t *testing.T) {
	ac := loadAdapterCorpus(t)
	set := TemplatesFromCorpus(ac, nil)
	assert.Greater(t, set.Len(), 200)
	for _, bidder := range []string{"appnexus", "rubicon", "pubmatic"} {
		t.Run(bidder, func(t *testing.T) {
			tmpl, ok := set.Template(bidder)
			require.True(t, ok)
			require.True(t, tmpl.OpenRTB, tmpl.Source)
			f, run := fixtureRun(t, ac, bidder)
			var req openrtb2.BidRequest
			require.NoError(t, json.Unmarshal(f.Spec.BidRequest, &req))
			var wantIDs []string
			for i := range req.Imp {
				req.Imp[i].ID = fmt.Sprintf("parity-imp-%d", i+1)
				wantIDs = append(wantIDs, req.Imp[i].ID)
			}
			b, err := exchange.BuildParityBidder(openrtb_ext.BidderName(bidder),
				config.Adapter{Endpoint: run.AdapterConfig.Endpoint, ExtraAdapterInfo: run.AdapterConfig.ExtraInfo},
				config.Server{ExternalUrl: run.Server.ExternalURL, GvlID: run.Server.GvlID, DataCenter: run.Server.DataCenter},
				config.BidderInfo{}, false)
			require.NoError(t, err)
			reqs, errs := b.MakeRequests(&req, &adapters.ExtraRequestInfo{})
			require.Empty(t, errs)
			require.NotEmpty(t, reqs)
			in := newInbound(&wireRequest{method: http.MethodPost, target: reqs[0].Uri, body: reqs[0].Body, headers: parity.HeaderList{}})
			in.meta = parity.OutboundMeta{Case: "tmpl-case", Bidder: bidder}
			in.impIDs = findImpIDs(in, wantIDs)
			require.NotEmpty(t, in.impIDs)
			rep := renderReply(tmpl, in, nil)
			require.Equal(t, 200, rep.status, rep.notes)
			resp, errs := b.MakeBids(&req, reqs[0], &adapters.ResponseData{StatusCode: 200, Body: rep.body, Headers: http.Header{}})
			require.Empty(t, errs)
			require.NotNil(t, resp)
			require.NotEmpty(t, resp.Bids)
			for _, bid := range resp.Bids {
				assert.Contains(t, wantIDs, bid.Bid.ImpID)
				assert.Equal(t, TemplatePrice("tmpl-case", bidder, bid.Bid.ImpID), bid.Bid.Price)
			}
		})
	}
}

func TestSourcesFromRepoResolveAliases(t *testing.T) {
	repo := testRepo(t)
	aliases, err := BidderAliases(repo)
	require.NoError(t, err)
	require.NotEmpty(t, aliases)
	src, err := SourcesFromRepo(repo)
	require.NoError(t, err)
	var checked int
	for alias, parent := range aliases {
		parentTmpl, ok := src.Templates.Template(parent)
		if !ok {
			continue
		}
		aliasTmpl, ok := src.Templates.Template(alias)
		require.True(t, ok, alias)
		assert.Same(t, parentTmpl, aliasTmpl, "%s uses the template of %s", alias, parent)
		checked++
	}
	assert.Positive(t, checked)
	assert.Positive(t, src.Replay.Len())
}

func TestOutboundCalls(t *testing.T) {
	caps := []Capture{{Call: parity.OutboundCall{Seq: 1, Kind: parity.OutboundBidder, Wire: true}}, {Call: parity.OutboundCall{Seq: 2, Kind: parity.OutboundCache, Wire: true}}}
	calls := OutboundCalls(caps)
	require.Len(t, calls, 2)
	assert.Equal(t, parity.OutboundCache, calls[1].Kind)
	assert.Empty(t, OutboundCalls(nil))
}

func TestReplayIndex(t *testing.T) {
	ac := loadAdapterCorpus(t)
	idx := ReplayFromCorpus(ac)
	assert.Greater(t, idx.Len(), 3000)
	f, _ := fixtureRun(t, ac, "appnexus")
	call := f.Spec.HTTPCalls[0]
	doc, err := decodeUseNumber(call.Request.Body)
	require.NoError(t, err)
	reencoded, err := json.MarshalIndent(doc, "", "    ")
	require.NoError(t, err)
	e, ok := idx.lookup("appnexus", call.Request.URI, reencoded)
	require.True(t, ok, "semantically equal bodies replay")
	assert.Equal(t, call.Response.Status, e.response.Status)
	_, ok = idx.lookup("appnexus", call.Request.URI, []byte(`{"id":"other"}`))
	assert.False(t, ok)
	_, ok = idx.lookup("rubicon", call.Request.URI, reencoded)
	assert.False(t, ok)
}

func TestLiftedCaseThroughParityTransport(t *testing.T) {
	repo := testRepo(t)
	c, err := corpus.LoadAll(repo)
	require.NoError(t, err)
	cs, ok := c.Case("adapters/appnexus/appnexustest/exemplary/simple-banner.json#lift:auction")
	require.True(t, ok)
	o := startOracle(t, Options{Templates: TemplatesFromCorpus(c.Adapters, nil), Replay: ReplayFromCorpus(c.Adapters)})
	require.NoError(t, o.Register(PlanFromCase(cs)))

	sess, end := trace.Begin(cs.Envelope)
	defer end()
	require.NotNil(t, sess)
	client := &http.Client{Transport: transport.Wrap(http.DefaultTransport, parity.OutboundGeneral, config.ParityConfig{Enabled: true, OracleURL: o.URL()}), Timeout: 5 * time.Second}
	body := `{"id":"x","imp":[{"id":"test-imp-id","banner":{}}]}`
	req, err := http.NewRequestWithContext(trace.WithBidder(context.Background(), "appnexus"), http.MethodPost, "https://ib.adnxs.com/openrtb2/prebid", strings.NewReader(body))
	require.NoError(t, err)
	resp, err := client.Do(req)
	require.NoError(t, err)
	got, err := io.ReadAll(resp.Body)
	require.NoError(t, err)
	resp.Body.Close()
	require.Equal(t, cs.Mocks.HTTP[0].Response.Status, resp.StatusCode)
	want, err := cs.Mocks.HTTP[0].Response.Body.WireBytes()
	require.NoError(t, err)
	assert.JSONEq(t, string(want), string(got))

	caps := drain(t, o, cs.ID, parity.ImplGo)
	require.Len(t, caps, 1)
	assert.Equal(t, parity.MockSourceRule, caps[0].Call.Mock.Source)
	assert.Equal(t, "appnexus", caps[0].Call.Bidder)
	assert.Equal(t, "https://ib.adnxs.com/openrtb2/prebid", caps[0].Call.OrigURL)
	assert.Equal(t, "http://ib.adnxs.com/openrtb2/prebid", caps[0].Call.Request.URL)
}

func TestDrainWaitsForInFlightCalls(t *testing.T) {
	o := startOracle(t, Options{})
	require.NoError(t, o.Register(&Plan{CaseID: "inflight", Rules: []Rule{{ID: "r", Response: parity.InlineResponse{Status: 200, DelayMS: 300}}}}))
	client := proxyClient(t, o, 5*time.Second)
	done := make(chan struct{})
	go func() {
		defer close(done)
		_, _, err := post(t, client, "http://b.test/", parity.OutboundMeta{Case: "inflight", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "b"}, `{}`)
		assert.NoError(t, err)
	}()
	require.Eventually(t, func() bool {
		o.mu.Lock()
		defer o.mu.Unlock()
		return o.pending[captureKey{"inflight", parity.ImplGo}] > 0
	}, 2*time.Second, 5*time.Millisecond)
	caps := drain(t, o, "inflight", parity.ImplGo)
	assert.Len(t, caps, 1)
	<-done

	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	_, err := o.Drain(ctx, "nothing", parity.ImplGo)
	assert.ErrorIs(t, err, context.Canceled)
}

func TestStartRejectsNonLoopbackAndMissingFixtures(t *testing.T) {
	root := testRepo(t).Root()
	_, err := Start(Options{Addr: "0.0.0.0:0", RepoRoot: root})
	assert.Error(t, err)
	_, err = Start(Options{FixturesDir: filepath.Join(t.TempDir(), "missing")})
	assert.Error(t, err)
}

func TestPlanFromCase(t *testing.T) {
	price := 1.25
	cs := &corpus.Case{
		ID: "case-1",
		Mocks: corpus.MockPlan{
			ImpIDs:  []string{"i1"},
			HTTP:    []corpus.HTTPMock{{ID: "m1", Bidder: "appnexus", ImpIDs: []string{"i1"}, Response: parity.InlineResponse{Status: 200}}},
			Bidders: map[string]corpus.MockBidder{"appnexus": {Currency: "EUR", Price: &price, DealID: "d", Seat: "s"}},
		},
	}
	p := PlanFromCase(cs)
	require.NoError(t, p.compile())
	assert.Equal(t, []Rule{{ID: "m1", Kind: parity.OutboundBidder, Bidder: "appnexus", ImpIDs: []string{"i1"}, Response: parity.InlineResponse{Status: 200}}}, p.Rules)
	assert.Equal(t, BidderParams{Price: &price, Currency: "EUR", DealID: "d", Seat: "s"}, p.Bidders["appnexus"])
	inline := RuleFromInline(parity.InlineRule{ID: "x", Kind: "cache", Response: parity.InlineResponse{Status: 503}})
	assert.Equal(t, "cache", inline.Kind)
}
