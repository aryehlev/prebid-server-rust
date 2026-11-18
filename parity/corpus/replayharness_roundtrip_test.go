package corpus_test

import (
	"bytes"
	"compress/gzip"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
	"github.com/prebid/prebid-server/v4/parity/server"
	"github.com/prebid/prebid-server/v4/parity/wiring"
)

// rhSeen is a request as a net/http handler receives it.
type rhSeen struct {
	Method        string
	RequestURI    string
	EscapedPath   string
	RawQuery      string
	Host          string
	Header        http.Header
	ContentLength int64
	Body          []byte
}

// rhCapture is a handler that hands every request it receives to the test.
type rhCapture chan rhSeen

func (c rhCapture) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		w.WriteHeader(http.StatusBadRequest)

		return
	}

	c <- rhSeen{
		Method: r.Method, RequestURI: r.RequestURI, EscapedPath: r.URL.EscapedPath(), RawQuery: r.URL.RawQuery,
		Host: r.Host, Header: r.Header.Clone(), ContentLength: r.ContentLength, Body: body,
	}

	w.WriteHeader(http.StatusNoContent)
}

func (c rhCapture) next(t *testing.T) rhSeen {
	t.Helper()

	select {
	case s := <-c:
		return s
	case <-time.After(10 * time.Second):
		t.Fatal("the handler received no request")

		return rhSeen{}
	}
}

// rhTrip is one request as a replay or a probe gives it to ReplayHarness.
type rhTrip struct {
	method string
	target string
	header map[string]string
	body   []byte
}

// send sends trip the way ReplayHarness's client does at the pinned commit
// (internal/runner/runner.go, Client.Do): a body reader only for a body or a
// Content-Type, its User-Agent, then each header with Header.Set, over an
// http.Transport that leaves compression on.
func (trip rhTrip) send(t *testing.T, base string) {
	t.Helper()

	var body io.Reader
	if len(trip.body) > 0 || trip.header["Content-Type"] != "" {
		body = bytes.NewReader(trip.body)
	}

	req, err := http.NewRequestWithContext(t.Context(), trip.method, base+trip.target, body)
	if err != nil {
		t.Fatal(err)
	}

	req.Header.Set("User-Agent", rhUA)

	for k, v := range trip.header {
		req.Header.Set(k, v)
	}

	client := &http.Client{Transport: &http.Transport{}}
	defer client.CloseIdleConnections()

	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}

	if err := resp.Body.Close(); err != nil {
		t.Fatal(err)
	}
}

// rhCredentialHeaders are the headers the pinned recorder leaves out unless
// it runs with --keep-secrets (internal/discover/traffic.go SensitiveHeaders).
func rhCredentialHeaders() []string {
	return []string{
		"Authorization", "Proxy-Authorization", "Cookie", "Set-Cookie",
		"X-Api-Key", "Api-Key", "X-Auth-Token", "X-Csrf-Token", "X-Xsrf-Token",
	}
}

// rhRecord is the line the pinned recorder writes for a request it received
// (internal/record/proxy.go and internal/discover/traffic.go
// SanitizeHeaders): the escaped path and the raw query, the headers by
// canonical name with their values joined by ", " and without the
// hop-by-hop and credential headers, and the body as text when it is UTF-8,
// else as base64.
func rhRecord(s rhSeen) rhLine {
	drop := append(rhreplay.HopHeaders(), rhCredentialHeaders()...)
	headers := map[string]string{}

	for k, v := range s.Header {
		if ck := http.CanonicalHeaderKey(k); !slices.Contains(drop, ck) && len(v) > 0 {
			headers[ck] = strings.Join(v, ", ")
		}
	}

	req := rhreplay.Request{Method: s.Method, Path: s.EscapedPath, Query: s.RawQuery, Headers: headers, Body: string(s.Body)}
	if !utf8.Valid(s.Body) {
		req.Body, req.BodyEncoding = base64.StdEncoding.EncodeToString(s.Body), "base64"
	}

	return rhLine{Request: req, Response: rhreplay.Response{Status: http.StatusNoContent}}
}

// rhRoundTripRepo writes a repository whose traffic.jsonl holds lines.
func rhRoundTripRepo(t *testing.T, lines []rhLine) *corpus.Repo {
	t.Helper()

	root := t.TempDir()
	dir := filepath.Join(root, "parity", "replayharness")

	if err := os.MkdirAll(dir, 0o750); err != nil {
		t.Fatal(err)
	}

	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte("module github.com/prebid/prebid-server/v4\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	if err := os.WriteFile(filepath.Join(dir, rhTraffic), []byte(rhJSONLines(t, lines)), 0o600); err != nil {
		t.Fatal(err)
	}

	repo, err := corpus.OpenRepo(root)
	if err != nil {
		t.Fatal(err)
	}

	return repo
}

// rhCaseOfLine returns the case built from line n of traffic.jsonl.
func rhCaseOfLine(t *testing.T, rh *corpus.ReplayHarness, n int) corpus.Case {
	t.Helper()

	note := rhNotePfx + rhTraffic + ":" + strconv.Itoa(n)
	for _, cs := range rh.Cases {
		if slices.Contains(cs.Notes, note) {
			return cs
		}
	}

	for _, nc := range rh.NotCompared {
		if slices.Contains(nc.Sources, rhTraffic+":"+strconv.Itoa(n)) {
			t.Fatalf("line %d is not compared: %s: %s", n, nc.Tag, nc.Reason)
		}
	}

	t.Fatalf("no case comes from line %d", n)

	return corpus.Case{}
}

// rhExec dispatches the case's envelope through /__parity/exec to h.
func rhExec(t *testing.T, h http.Handler, cs corpus.Case) {
	t.Helper()

	raw, err := json.Marshal(cs.Envelope)
	if err != nil {
		t.Fatal(err)
	}

	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequestWithContext(t.Context(), http.MethodPost, server.PathExec, bytes.NewReader(raw)))

	if rec.Code != http.StatusOK {
		t.Fatalf("exec answers %d: %s", rec.Code, rec.Body.String())
	}
}

func rhGzip(t *testing.T, s string) []byte {
	t.Helper()

	var b bytes.Buffer

	zw := gzip.NewWriter(&b)
	if _, err := zw.Write([]byte(s)); err != nil {
		t.Fatal(err)
	}

	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}

	return b.Bytes()
}

// TestReplayHarnessRoundTrip follows requests from ReplayHarness to the
// handler twice. Once as ReplayHarness sends them: its client, a net/http
// server, and the line the recorder writes for what that server received.
// Once as the parity runner sends them: the corpus loads that line, and
// /__parity/exec dispatches the case's envelope. The handler must receive
// the same request both times: method, request URI, escaped path, raw query,
// every header with its values, and the body bytes. Only Host differs, by
// design: exec uses 127.0.0.1, not the port the server listened on.
//
// ReplayHarness sets every header with Header.Set, so it never sends a header
// twice, and its client trims header values; neither can reach the corpus.
func TestReplayHarnessRoundTrip(t *testing.T) {
	t.Parallel()

	const jsonType, ctype = "application/json", "Content-Type"

	auction := []byte(`{"id":"rt","imp":[{"id":"i","banner":{"format":[{"w":300,"h":250}]}}]}`)
	query := "?enabledonly=true&a=1&a=2&b=%20x&c=&d=%2B+e&%E2%9C%93=%E2%9C%93&"
	trips := []rhTrip{
		{method: http.MethodGet, target: rhInfo + query, header: map[string]string{"accept": jsonType, "x-mixed-CASE": "  spaced  value  "}},
		{method: http.MethodGet, target: "/static/%2e%2e/a%2Fb%41"},
		{method: http.MethodPost, target: rhAuction, header: map[string]string{ctype: jsonType}, body: auction},
		{
			method: http.MethodPost, target: rhAuction, body: rhGzip(t, string(auction)),
			header: map[string]string{ctype: jsonType, "Content-Encoding": "gzip"},
		},
		{method: http.MethodPost, target: "/event", body: []byte{0xff, 0xfe, 0x00, 0x01}},
		{method: http.MethodPost, target: "/vtrack?a=rh-events-account", header: map[string]string{ctype: jsonType}},
		{method: http.MethodPut, target: "/vtrack", body: []byte(`{"puts":[]}`)},
		{method: http.MethodPatch, target: rhStatus},
		{method: http.MethodDelete, target: rhStatus},
		{
			method: http.MethodOptions, target: rhAuction,
			header: map[string]string{"Origin": "https://pub.example", "Access-Control-Request-Method": http.MethodPost},
		},
		{method: http.MethodHead, target: rhStatus},
		{method: http.MethodGet, target: "/static/index.html", header: map[string]string{"Range": "bytes=0-9"}},
	}

	direct := make(rhCapture, 1)
	srv := httptest.NewServer(direct)
	t.Cleanup(srv.Close)

	seen := make([]rhSeen, 0, len(trips))
	lines := make([]rhLine, 0, len(trips))

	for _, trip := range trips {
		trip.send(t, srv.URL)
		s := direct.next(t)
		seen = append(seen, s)
		lines = append(lines, rhRecord(s))
	}

	rh := rhLoad(t, rhRoundTripRepo(t, lines))
	dispatched := make(rhCapture, 1)
	exec := server.New(wiring.Deps{}, dispatched, dispatched)

	for i, want := range seen {
		cs := rhCaseOfLine(t, rh, i+1)
		rhExec(t, exec, cs)
		got := dispatched.next(t)

		if got.Host != server.DefaultHost {
			t.Errorf("%s %s: exec Host %q, want %q", want.Method, want.RequestURI, got.Host, server.DefaultHost)
		}

		got.Host = want.Host
		if !reflect.DeepEqual(got, want) {
			t.Errorf("%s %s: the handler receives\n  %+v\nthrough exec, but\n  %+v\nfrom ReplayHarness", want.Method, want.RequestURI, got, want)
		}
	}
}
