package transport

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// capturedRequest is what the fake oracle saw.
type capturedRequest struct {
	method     string
	requestURI string
	host       string
	header     http.Header
	body       string
	proto      string
}

// fakeOracle is an httptest forward proxy that records every request and
// answers with a fixed body after an optional delay.
type fakeOracle struct {
	*httptest.Server
	mu       sync.Mutex
	requests []capturedRequest
	delay    time.Duration
}

func newFakeOracle(t *testing.T, delay time.Duration) *fakeOracle {
	t.Helper()
	o := &fakeOracle{delay: delay}
	o.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		o.mu.Lock()
		o.requests = append(o.requests, capturedRequest{
			method: r.Method, requestURI: r.RequestURI, host: r.Host,
			header: r.Header.Clone(), body: string(body), proto: r.Proto,
		})
		o.mu.Unlock()
		if o.delay > 0 {
			select {
			case <-time.After(o.delay):
			case <-r.Context().Done():
				return
			}
		}
		w.Header().Set("Content-Type", "application/json")
		_, _ = io.WriteString(w, `{"oracle":true}`)
	}))
	t.Cleanup(o.Close)
	return o
}

func (o *fakeOracle) captured() []capturedRequest {
	o.mu.Lock()
	defer o.mu.Unlock()
	return append([]capturedRequest(nil), o.requests...)
}

func parityCfg(oracleURL string) config.ParityConfig {
	return config.ParityConfig{Enabled: true, Token: "0123456789abcdef", OracleURL: oracleURL}
}

func TestOracleModeForwardsWireDefaults(t *testing.T) {
	oracle := newFakeOracle(t, 0)
	tests := []struct {
		name       string
		session    *parity.Envelope
		url        string
		bidder     string
		classify   func(*http.Request) string
		wantURI    string
		wantMeta   parity.OutboundMeta
		wantHeader map[string]string
	}{
		{
			name:    "https bidder call without session",
			url:     "https://ib.adnxs.com/openrtb2?member=1",
			bidder:  "appnexus",
			wantURI: "http://ib.adnxs.com/openrtb2?member=1",
			wantMeta: parity.OutboundMeta{V: 1, Case: "", Impl: parity.ImplGo, Kind: parity.OutboundBidder,
				Bidder: "appnexus", OrigURL: "https://ib.adnxs.com/openrtb2?member=1", Seq: 0},
		},
		{
			name:    "http call inside a session carries case and seq",
			session: &parity.Envelope{CaseID: "case-7"},
			url:     "http://cache.example/cache",
			wantURI: "http://cache.example/cache",
			wantMeta: parity.OutboundMeta{V: 1, Case: "case-7", Impl: parity.ImplGo, Kind: parity.OutboundCache,
				OrigURL: "http://cache.example/cache", Seq: 1},
		},
		{
			name:     "classifier refines the client label",
			url:      "https://vendor-list.consensu.org/v3/vendor-list.json",
			classify: PrefixClassifier([]PrefixRule{{Prefix: "https://vendor-list.consensu.org/", Kind: parity.OutboundVendorList}}),
			wantURI:  "http://vendor-list.consensu.org/v3/vendor-list.json",
			wantMeta: parity.OutboundMeta{V: 1, Impl: parity.ImplGo, Kind: parity.OutboundVendorList,
				OrigURL: "https://vendor-list.consensu.org/v3/vendor-list.json"},
		},
	}
	for i, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.session != nil {
				_, end := trace.Begin(tt.session)
				defer end()
			}
			kind := parity.OutboundCache
			if tt.classify != nil {
				kind = parity.OutboundGeneral
			}
			client := &http.Client{Transport: New(&http.Transport{}, Options{Kind: kind, Classify: tt.classify}, parityCfg(oracle.URL))}
			req, err := http.NewRequest(http.MethodPost, tt.url, strings.NewReader(`{"x":1}`))
			require.NoError(t, err)
			if tt.bidder != "" {
				req = req.WithContext(trace.WithBidder(req.Context(), tt.bidder))
			}
			resp, err := client.Do(req)
			require.NoError(t, err)
			body, err := io.ReadAll(resp.Body)
			require.NoError(t, err)
			require.NoError(t, resp.Body.Close())
			assert.Equal(t, `{"oracle":true}`, string(body))

			got := oracle.captured()
			require.Len(t, got, i+1)
			c := got[i]
			assert.Equal(t, tt.wantURI, c.requestURI, "the proxy must receive an absolute-form http request")
			assert.Equal(t, "Go-http-client/1.1", c.header.Get("User-Agent"))
			assert.Equal(t, "gzip", c.header.Get("Accept-Encoding"))
			assert.Equal(t, "HTTP/1.1", c.proto)
			assert.Equal(t, `{"x":1}`, c.body)
			meta, err := DecodeMeta(c.header.Get(parity.MetaHeader))
			require.NoError(t, err)
			assert.Equal(t, tt.wantMeta, meta)
		})
	}
}

func TestOracleDelayHonoursDeadline(t *testing.T) {
	oracle := newFakeOracle(t, 2*time.Second)
	client := &http.Client{Transport: Wrap(&http.Transport{}, parity.OutboundGeneral, parityCfg(oracle.URL))}
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "https://slow.example/", nil)
	require.NoError(t, err)

	start := time.Now()
	resp, err := client.Do(req)
	if resp != nil {
		resp.Body.Close()
	}
	require.Error(t, err)
	assert.ErrorIs(t, err, context.DeadlineExceeded)
	assert.Less(t, time.Since(start), time.Second, "the client deadline must cut the oracle delay short")
}

func TestNoOracleMeansNetworkDisabled(t *testing.T) {
	tests := []struct {
		name      string
		oracleURL string
		wantErr   string
	}{
		{name: "not configured", oracleURL: "", wantErr: "not configured"},
		{name: "https oracle rejected", oracleURL: "https://127.0.0.1:1", wantErr: "absolute http URL"},
		{name: "relative oracle rejected", oracleURL: "/oracle", wantErr: "absolute http URL"},
		{name: "unparseable oracle", oracleURL: "http://[::1", wantErr: "invalid parity.oracle_url"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client := &http.Client{Transport: Wrap(nil, parity.OutboundGeneral, parityCfg(tt.oracleURL))}
			resp, err := client.Post("https://example.com/", "application/json", strings.NewReader("{}"))
			if resp != nil {
				resp.Body.Close()
			}
			require.Error(t, err)
			assert.ErrorIs(t, err, ErrNetworkDisabled)
			assert.Contains(t, err.Error(), tt.wantErr)
		})
	}
}

func inlineEnvelope(mocks *parity.InlineMocks) *parity.Envelope {
	return &parity.Envelope{CaseID: "inline", Mock: parity.MockSpec{Mode: parity.MockModeInline, Inline: mocks}}
}

func failPtr(s string) *string { return &s }

func TestInlineResolution(t *testing.T) {
	mocks := &parity.InlineMocks{
		Rules: []parity.InlineRule{
			{ID: "bad-regex", URIRegex: "(", Response: parity.InlineResponse{Status: 500}},
			{ID: "rubicon-video", Kind: parity.OutboundBidder, Bidder: "rubicon", URIRegex: `video`, Response: parity.InlineResponse{Status: 201, Body: parity.TextBody("rule")}},
		},
		Bidders:       map[string]parity.InlineResponse{"rubicon": {Body: parity.TextBody("bidder")}},
		DefaultBidder: nil,
		Cache:         &parity.InlineResponse{Status: 200, Body: parity.TextBody(`{"responses":[]}`)},
		Kinds:         map[string]parity.InlineResponse{parity.OutboundFloors: {Status: 404}},
	}
	tests := []struct {
		name       string
		kind       string
		bidder     string
		url        string
		wantStatus int
		wantBody   string
		wantErr    error
		wantMock   parity.MockRef
		wantCallEr string
	}{
		{name: "rule match", kind: parity.OutboundGeneral, bidder: "rubicon", url: "https://r.example/video", wantStatus: 201, wantBody: "rule", wantMock: parity.MockRef{Source: parity.MockSourceRule, ID: "rubicon-video"}},
		{name: "bidder mock defaults to 200", kind: parity.OutboundGeneral, bidder: "rubicon", url: "https://r.example/banner", wantStatus: 200, wantBody: "bidder", wantMock: parity.MockRef{Source: parity.MockSourceInline, ID: "bidders.rubicon"}},
		{name: "unknown bidder gets builtin 204", kind: parity.OutboundGeneral, bidder: "appnexus", url: "https://a.example/", wantStatus: 204, wantMock: parity.MockRef{Source: parity.MockSourceDefault, ID: parity.OutboundBidder}},
		{name: "cache mock", kind: parity.OutboundCache, url: "https://cache.example/", wantStatus: 200, wantBody: `{"responses":[]}`, wantMock: parity.MockRef{Source: parity.MockSourceInline, ID: "cache"}},
		{name: "kind mock", kind: parity.OutboundFloors, url: "https://floors.example/", wantStatus: 404, wantMock: parity.MockRef{Source: parity.MockSourceInline, ID: "kinds.floors"}},
		{name: "unmocked kind is network disabled", kind: parity.OutboundCurrency, url: "https://fx.example/", wantErr: ErrNetworkDisabled, wantMock: parity.MockRef{Source: parity.MockSourceDefault, ID: parity.OutboundCurrency}, wantCallEr: parity.OutboundErrNetDisabled},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sess, end := trace.Begin(inlineEnvelope(mocks))
			defer end()
			rt := Wrap(nil, tt.kind, parityCfg(""))
			req, err := http.NewRequest(http.MethodPost, tt.url, strings.NewReader("payload"))
			require.NoError(t, err)
			if tt.bidder != "" {
				req = req.WithContext(trace.WithBidder(req.Context(), tt.bidder))
			}
			resp, err := rt.RoundTrip(req)
			if tt.wantErr != nil {
				assert.ErrorIs(t, err, tt.wantErr)
			} else {
				require.NoError(t, err)
				body, readErr := io.ReadAll(resp.Body)
				require.NoError(t, readErr)
				assert.Equal(t, tt.wantStatus, resp.StatusCode)
				assert.Equal(t, tt.wantBody, string(body))
			}

			out := sess.Snapshot().HTTPOut
			require.Len(t, out, 1)
			call := out[0]
			assert.False(t, call.Wire, "inline captures are the client view")
			assert.Equal(t, tt.url, call.OrigURL)
			assert.Equal(t, "payload", *call.Request.Body.Text)
			require.NotNil(t, call.Mock)
			assert.Equal(t, tt.wantMock, *call.Mock)
			if tt.wantCallEr != "" {
				require.NotNil(t, call.Error)
				assert.Equal(t, tt.wantCallEr, *call.Error)
				assert.Nil(t, call.Response)
			} else {
				assert.Nil(t, call.Error)
				require.NotNil(t, call.Response)
				assert.Equal(t, tt.wantStatus, call.Response.Status)
			}
		})
	}
}

func TestInlineIsDeterministic(t *testing.T) {
	mocks := &parity.InlineMocks{DefaultBidder: &parity.InlineResponse{
		Headers: parity.HeaderList{{"Content-Type", "application/json"}, {"X-Multi", "a"}, {"x-multi", "b"}},
		Body:    &parity.Body{JSON: []byte(`{"seatbid":[]}`)},
	}}
	var bodies []string
	for i := 0; i < 3; i++ {
		_, end := trace.Begin(inlineEnvelope(mocks))
		req, err := http.NewRequest(http.MethodGet, "https://b.example/", nil)
		require.NoError(t, err)
		req = req.WithContext(trace.WithBidder(req.Context(), "any"))
		resp, err := Wrap(nil, parity.OutboundGeneral, parityCfg("")).RoundTrip(req)
		require.NoError(t, err)
		body, err := io.ReadAll(resp.Body)
		require.NoError(t, err)
		assert.Equal(t, []string{"a", "b"}, resp.Header.Values("X-Multi"))
		assert.Equal(t, int64(len(body)), resp.ContentLength)
		bodies = append(bodies, string(body))
		end()
	}
	assert.Equal(t, []string{`{"seatbid":[]}`, `{"seatbid":[]}`, `{"seatbid":[]}`}, bodies)
}

func TestInlineFailures(t *testing.T) {
	tests := []struct {
		name      string
		response  parity.InlineResponse
		timeout   time.Duration
		check     func(t *testing.T, err error)
		wantClass string
	}{
		{
			name:     "refused",
			response: parity.InlineResponse{Fail: failPtr(parity.OutboundErrRefused)},
			check: func(t *testing.T, err error) {
				var opErr *net.OpError
				require.ErrorAs(t, err, &opErr)
				assert.ErrorIs(t, err, syscall.ECONNREFUSED)
			},
			wantClass: parity.OutboundErrRefused,
		},
		{
			name:     "reset",
			response: parity.InlineResponse{Fail: failPtr(parity.OutboundErrReset)},
			check: func(t *testing.T, err error) {
				assert.ErrorIs(t, err, syscall.ECONNRESET)
			},
			wantClass: parity.OutboundErrReset,
		},
		{
			name:     "generic error",
			response: parity.InlineResponse{Fail: failPtr("error")},
			check: func(t *testing.T, err error) {
				assert.Contains(t, err.Error(), "inline mock failure")
			},
			wantClass: parity.OutboundErrOther,
		},
		{
			name:     "timeout without deadline is a net timeout",
			response: parity.InlineResponse{Fail: failPtr(parity.OutboundErrTimeout)},
			check: func(t *testing.T, err error) {
				var netErr net.Error
				require.ErrorAs(t, err, &netErr)
				assert.True(t, netErr.Timeout())
			},
			wantClass: parity.OutboundErrTimeout,
		},
		{
			name:     "timeout waits for the deadline",
			response: parity.InlineResponse{Fail: failPtr(parity.OutboundErrTimeout)},
			timeout:  20 * time.Millisecond,
			check: func(t *testing.T, err error) {
				assert.ErrorIs(t, err, context.DeadlineExceeded)
			},
			wantClass: parity.OutboundErrTimeout,
		},
		{
			name:     "delay longer than the deadline times out",
			response: parity.InlineResponse{DelayMS: 2000},
			timeout:  20 * time.Millisecond,
			check: func(t *testing.T, err error) {
				assert.ErrorIs(t, err, context.DeadlineExceeded)
			},
			wantClass: parity.OutboundErrTimeout,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sess, end := trace.Begin(inlineEnvelope(&parity.InlineMocks{Kinds: map[string]parity.InlineResponse{parity.OutboundGeneral: tt.response}}))
			defer end()
			ctx := context.Background()
			if tt.timeout > 0 {
				var cancel context.CancelFunc
				ctx, cancel = context.WithTimeout(ctx, tt.timeout)
				defer cancel()
			}
			req, err := http.NewRequestWithContext(ctx, http.MethodGet, "https://x.example/", nil)
			require.NoError(t, err)
			start := time.Now()
			resp, err := Wrap(nil, parity.OutboundGeneral, parityCfg("")).RoundTrip(req)
			assert.Nil(t, resp)
			require.Error(t, err)
			tt.check(t, err)
			assert.Less(t, time.Since(start), time.Second)

			out := sess.Snapshot().HTTPOut
			require.Len(t, out, 1)
			require.NotNil(t, out[0].Error)
			assert.Equal(t, tt.wantClass, *out[0].Error)
		})
	}
}

func TestInlineDelayWithinDeadline(t *testing.T) {
	_, end := trace.Begin(inlineEnvelope(&parity.InlineMocks{Cache: &parity.InlineResponse{DelayMS: 10, Status: 200}}))
	defer end()
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://cache.example/", nil)
	require.NoError(t, err)
	start := time.Now()
	resp, err := Wrap(nil, parity.OutboundCache, parityCfg("")).RoundTrip(req)
	require.NoError(t, err)
	assert.Equal(t, 200, resp.StatusCode)
	assert.GreaterOrEqual(t, time.Since(start), 10*time.Millisecond)
}

func TestWrapUnwrapsParityTransport(t *testing.T) {
	base := &http.Transport{MaxIdleConns: 7}
	first := New(base, Options{Kind: parity.OutboundCache}, parityCfg(""))
	second := New(first, Options{}, parityCfg(""))
	assert.Equal(t, 7, second.base.MaxIdleConns, "wrapping a parity transport reuses its production template")
	assert.NotSame(t, first.base, second.base)
	assert.Equal(t, parity.OutboundGeneral, second.Kind(), "empty kind defaults to general")
	assert.Equal(t, parity.OutboundCache, first.Kind())
	assert.NotPanics(t, second.CloseIdleConnections)
}

func TestMetaRoundTrip(t *testing.T) {
	tests := []struct {
		name    string
		value   string
		want    parity.OutboundMeta
		wantErr string
	}{
		{name: "bad base64", value: "!!", wantErr: "base64"},
		{name: "bad json", value: "bm90IGpzb24=", wantErr: "JSON"},
		{name: "wrong version", value: mustEncode(t, parity.OutboundMeta{V: 2}), wantErr: "version 2"},
		{
			name:  "ok",
			value: mustEncode(t, parity.OutboundMeta{V: 1, Case: "c", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "b", OrigURL: "https://u", Seq: 4}),
			want:  parity.OutboundMeta{V: 1, Case: "c", Impl: parity.ImplGo, Kind: parity.OutboundBidder, Bidder: "b", OrigURL: "https://u", Seq: 4},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := DecodeMeta(tt.value)
			if tt.wantErr != "" {
				require.Error(t, err)
				assert.Contains(t, err.Error(), tt.wantErr)
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
}

func mustEncode(t *testing.T, meta parity.OutboundMeta) string {
	t.Helper()
	v, err := EncodeMeta(meta)
	require.NoError(t, err)
	return v
}

func TestPrefixClassifier(t *testing.T) {
	classify := PrefixClassifier([]PrefixRule{
		{Prefix: "https://vendor-list.consensu.org/", Kind: parity.OutboundVendorList},
		{Prefix: "http://stored.example/", Kind: parity.OutboundStored},
		{Prefix: "", Kind: parity.OutboundModule},
		{Prefix: "http://ignored/", Kind: ""},
	})
	tests := []struct {
		url  string
		want string
	}{
		{url: "https://vendor-list.consensu.org/v2/archives/vendor-list-v1.json", want: parity.OutboundVendorList},
		{url: "http://stored.example/requests?ids=1", want: parity.OutboundStored},
		{url: "http://ignored/x", want: ""},
		{url: "https://elsewhere/", want: ""},
	}
	for _, tt := range tests {
		t.Run(tt.url, func(t *testing.T) {
			req, err := http.NewRequest(http.MethodGet, tt.url, nil)
			require.NoError(t, err)
			assert.Equal(t, tt.want, classify(req))
		})
	}
}

func TestHeaderConversions(t *testing.T) {
	h := http.Header{}
	h.Add("x-b", "2")
	h.Add("X-B", "3")
	h.Add("Accept", "json")
	assert.Equal(t, parity.HeaderList{{"Accept", "json"}, {"X-B", "2"}, {"X-B", "3"}}, HeaderListOf(h))
	back := HTTPHeaderOf(parity.HeaderList{{"x-b", "2"}, {"X-B", "3"}})
	assert.Equal(t, []string{"2", "3"}, back.Values("X-B"))
	assert.Empty(t, HeaderListOf(nil))
}

func TestContextClass(t *testing.T) {
	tests := []struct {
		err  error
		want string
	}{
		{err: context.DeadlineExceeded, want: parity.OutboundErrTimeout},
		{err: context.Canceled, want: parity.OutboundErrOther},
		{err: errors.New("x"), want: parity.OutboundErrOther},
	}
	for _, tt := range tests {
		t.Run(tt.err.Error(), func(t *testing.T) {
			assert.Equal(t, tt.want, contextClass(tt.err))
		})
	}
}
