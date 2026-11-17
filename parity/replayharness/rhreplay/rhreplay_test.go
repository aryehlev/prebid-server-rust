package rhreplay_test

import (
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

const flowYAML = `
replays:
  - name: save then use
    steps:
      - method: get
        path: /setuid
        query:
          bidder: appnexus
          uid: [a b, "c&d"]
        save:
          cookie: header:Set-Cookie
          code: status
      - method: POST
        path: /cookie_sync
        headers:
          Cookie: "{{cookie}}"
        json: {bidders: [appnexus], limit: 2, note: "<{{code}}>"}
        expect:
          status: [200, 4xx]
`

func mustParse(t *testing.T, doc string) []rhreplay.Replay {
	t.Helper()

	replays, err := rhreplay.Parse([]byte(doc))
	if err != nil {
		t.Fatalf("Parse: %v", err)
	}

	return replays
}

func TestParseForms(t *testing.T) {
	t.Parallel()

	replays := mustParse(t, flowYAML)
	if len(replays) != 1 || len(replays[0].Steps) != 2 {
		t.Fatalf("got %d replays", len(replays))
	}

	if got := replays[0].Steps[0].Method; got != http.MethodGet {
		t.Errorf("method %q, want GET", got)
	}

	body, ok := replays[0].Steps[1].JSON.(map[string]any)
	if !ok {
		t.Fatalf("json body is %T", replays[0].Steps[1].JSON)
	}

	if _, ok := body["limit"].(float64); !ok {
		t.Errorf("limit is %T, want float64 as after ReplayHarness's JSON round trip", body["limit"])
	}

	if got := replays[0].Steps[1].Expect.StatusClasses(); strings.Join(got, ",") != "200,4xx" {
		t.Errorf("status classes %v", got)
	}

	for _, doc := range []string{
		`{"name": "one", "steps": [{"method": "GET", "path": "/status"}]}`,
		`[{"name": "one", "steps": [{"method": "GET", "path": "/status"}]}]`,
		"flows:\n  - name: one\n    steps:\n      - {method: GET, path: /status}\n",
	} {
		if got := mustParse(t, doc); len(got) != 1 || got[0].Name != "one" {
			t.Errorf("Parse(%q) = %+v", doc, got)
		}
	}
}

func TestParseRejects(t *testing.T) {
	t.Parallel()

	for name, doc := range map[string]string{
		"unknown field":  `{"name": "x", "steps": [{"method": "GET", "path": "/", "nope": 1}]}`,
		"no name":        `{"steps": [{"method": "GET", "path": "/"}]}`,
		"no steps":       `{"name": "x", "steps": []}`,
		"json and body":  `{"name": "x", "steps": [{"method": "POST", "path": "/", "json": {}, "body": "x"}]}`,
		"bad status":     `{"name": "x", "steps": [{"method": "GET", "path": "/", "expect": {"status": 99}}]}`,
		"bad save":       `{"name": "x", "steps": [{"method": "GET", "path": "/", "save": {"a": "body"}}]}`,
		"relative path":  `{"name": "x", "steps": [{"method": "GET", "path": "status"}]}`,
		"missing method": `{"name": "x", "steps": [{"path": "/"}]}`,
	} {
		if _, err := rhreplay.Parse([]byte(doc)); !errors.Is(err, rhreplay.ErrBadReplay) {
			t.Errorf("%s: err %v, want ErrBadReplay", name, err)
		}
	}
}

const headerA = "X-A"

func flowVars() map[string]any {
	return map[string]any{"cookie": "uids=x; Path=/", "code": float64(200)}
}

func TestBuildRequestQuery(t *testing.T) {
	t.Parallel()

	first, err := rhreplay.BuildRequest(mustParse(t, flowYAML)[0].Steps[0], flowVars())
	if err != nil {
		t.Fatal(err)
	}

	if first.Path != "/setuid" || first.Query != "bidder=appnexus&uid=a+b&uid=c%26d" || first.Headers != nil {
		t.Errorf("first step: %+v", first)
	}
}

func TestBuildRequestJSONBody(t *testing.T) {
	t.Parallel()

	steps := mustParse(t, flowYAML)[0].Steps

	second, err := rhreplay.BuildRequest(steps[1], flowVars())
	if err != nil {
		t.Fatal(err)
	}

	// ReplayHarness encodes a json body with json.Marshal: keys sorted, the
	// saved number written as Scalar writes it, and < and > escaped.
	want, err := json.Marshal(map[string]any{"bidders": []string{"appnexus"}, "limit": 2, "note": "<200>"})
	if err != nil {
		t.Fatal(err)
	}

	if second.Body != string(want) || strings.ContainsAny(second.Body, "<>") {
		t.Errorf("body %s, want %s", second.Body, want)
	}

	if second.Headers["Content-Type"] != "application/json" || second.Headers["Cookie"] != "uids=x; Path=/" {
		t.Errorf("headers %v", second.Headers)
	}

	if _, err := rhreplay.BuildRequest(steps[1], nil); !errors.Is(err, rhreplay.ErrNotSendable) {
		t.Errorf("unknown variable: err %v, want ErrNotSendable", err)
	}
}

func TestBuildRequestEscapesThePath(t *testing.T) {
	t.Parallel()

	pathStep := rhreplay.Step{Method: http.MethodGet, Path: "/info/bidders/{{name}}?x={{name}}"}

	req, err := rhreplay.BuildRequest(pathStep, map[string]any{"name": "a b/c"})
	if err != nil {
		t.Fatal(err)
	}

	if req.Path != "/info/bidders/a%20b%2Fc" || req.Query != "x=a+b%2Fc" {
		t.Errorf("path %q query %q", req.Path, req.Query)
	}
}

func TestToWire(t *testing.T) {
	t.Parallel()

	w, err := rhreplay.ToWire(rhreplay.Request{
		Method: http.MethodGet, Path: "/a%20b", Query: "q=1",
		Headers: map[string]string{"user-agent": "mine", "x-one": "1"},
	}, "ReplayHarness/test")
	if err != nil {
		t.Fatal(err)
	}

	if w.Path != "/a%20b" || w.Query != "q=1" || w.Header.Get("User-Agent") != "mine" || w.Header.Get("X-One") != "1" {
		t.Errorf("wire %+v", w)
	}

	for name, req := range map[string]rhreplay.Request{
		"colliding headers": {Method: http.MethodGet, Path: "/", Headers: map[string]string{"x-a": "1", headerA: "2"}},
		"bad header name":   {Method: http.MethodGet, Path: "/", Headers: map[string]string{"bad name": "1"}},
		"newline in value":  {Method: http.MethodGet, Path: "/", Headers: map[string]string{headerA: "1\r\nX-B: 2"}},
		"bad method":        {Method: "GE T", Path: "/"},
	} {
		if _, err := rhreplay.ToWire(req, "ua"); !errors.Is(err, rhreplay.ErrNotSendable) {
			t.Errorf("%s: err %v, want ErrNotSendable", name, err)
		}
	}
}

const recording = `{"request":{"method":"GET","path":"/a"},"response":{"status":200,"headers":{"Set-Cookie":"uids=1"},` +
	`"body":"{\"a\":[{\"b\":\"x\"}],\"length\":7,\"k.k\":true}"}}

{"request":{"method":"POST","path":"/b","body":"aGk=","bodyEncoding":"base64"},"response":{"status":204}}
`

func TestReadRecording(t *testing.T) {
	t.Parallel()

	exs, err := rhreplay.ReadRecording(strings.NewReader(recording))
	if err != nil {
		t.Fatal(err)
	}

	if len(exs) != 2 || exs[0].Line != 1 || exs[1].Line != 3 || string(exs[1].Request.BodyBytes()) != "hi" {
		t.Fatalf("exchanges %+v", exs)
	}

	if _, err := rhreplay.ReadRecording(strings.NewReader(`{"request":{"path":"/"}}`)); !errors.Is(err, rhreplay.ErrBadRecording) {
		t.Errorf("no method: err %v, want ErrBadRecording", err)
	}
}

func TestExtract(t *testing.T) {
	t.Parallel()

	exs, err := rhreplay.ReadRecording(strings.NewReader(recording))
	if err != nil {
		t.Fatal(err)
	}

	resp := exs[0].Response
	for src, want := range map[string]any{
		"status": float64(200), "header:set-cookie": "uids=1", "$.a[0].b": "x", "a.length": float64(1),
		"$.length": float64(7), `$["k.k"]`: true,
	} {
		if got, ok := rhreplay.Extract(resp, src); !ok || got != want {
			t.Errorf("Extract(%s) = %v, %v; want %v", src, got, ok, want)
		}
	}

	for _, src := range []string{"header:X-Missing", "$.a[1]", "$.a[99999999999999999999]", "$.nope"} {
		if got, ok := rhreplay.Extract(resp, src); ok {
			t.Errorf("Extract(%s) = %v, want nothing", src, got)
		}
	}
}

func TestBuildAll(t *testing.T) {
	t.Parallel()

	replays := mustParse(t, flowYAML+`
  - name: builtins
    steps:
      - {method: GET, path: "/x/{{unique}}", save: {v: status}}
      - {method: GET, path: "/y/{{v}}"}
  - name: cannot send
    steps:
      - {method: GET, path: "/a", headers: {"bad name": "1"}}
      - {method: GET, path: "/b"}
`)

	setCookie := rhreplay.Response{Status: http.StatusOK, Headers: map[string]string{"Set-Cookie": "uids=abc"}}
	answers := func(w rhreplay.Wire) (rhreplay.Response, bool) {
		return setCookie, w.Path == "/setuid"
	}

	built := rhreplay.BuildAll(replays, "ReplayHarness/test", answers)
	if len(built) != 6 {
		t.Fatalf("built %d steps, want 6", len(built))
	}

	if b := built[1]; b.NotSent != "" || b.Wire.Header.Get("Cookie") != "uids=abc" {
		t.Errorf("the step using the saved cookie: %+v", b)
	}

	if !strings.Contains(built[2].NotSent, "{{unique}}") || !strings.Contains(built[3].NotSent, "{{v}}") {
		t.Errorf("builtins: %q, %q", built[2].NotSent, built[3].NotSent)
	}

	if built[4].NotSent == "" || !strings.Contains(built[5].NotSent, "ends the replay at step 1") {
		t.Errorf("unsendable step: %q, %q", built[4].NotSent, built[5].NotSent)
	}

	none := rhreplay.BuildAll(replays[:1], "ReplayHarness/test", func(rhreplay.Wire) (rhreplay.Response, bool) {
		return rhreplay.Response{}, false
	})
	if !strings.Contains(none[1].NotSent, "holds no answer") {
		t.Errorf("without saved answers: %q", none[1].NotSent)
	}
}

func TestAnswerKey(t *testing.T) {
	t.Parallel()

	base := rhreplay.AnswerKey("get", "/a", "q=1", map[string]string{"User-Agent": "one", "x-a": "1", "Connection": "close"}, nil)
	same := rhreplay.AnswerKey("GET", "/a", "q=1", map[string]string{"User-Agent": "two", "X-A": "1"}, nil)
	other := rhreplay.AnswerKey("GET", "/a", "q=1", map[string]string{"X-A": "1"}, []byte("x"))

	if base != same || base == other {
		t.Errorf("keys %s %s %s", base, same, other)
	}
}

func TestLoadDir(t *testing.T) {
	t.Parallel()

	// Block style: like ReplayHarness, Parse reads a document that starts
	// with { or [ as JSON.
	one := "name: %s\nsteps:\n  - {method: GET, path: /status}\n"
	fsys := fstest.MapFS{
		"r/b.yaml":      {Data: []byte(strings.ReplaceAll(one, "%s", "b"))},
		"r/a.json":      {Data: []byte(`{"name": "a", "steps": [{"method": "GET", "path": "/"}]}`)},
		"r/notes.md":    {Data: []byte("not a replay")},
		"r/sub/c.yaml":  {Data: []byte(strings.ReplaceAll(one, "%s", "c"))},
		"r/d.yml":       {Data: []byte(strings.ReplaceAll(one, "%s", "d"))},
		"r/e.yaml/x.md": {Data: []byte("a directory named like a replay")},
	}

	replays, err := rhreplay.LoadDir(fsys, "r")
	if err != nil {
		t.Fatal(err)
	}

	got := make([]string, 0, len(replays))
	for _, r := range replays {
		got = append(got, r.File+":"+r.Name)
	}

	if strings.Join(got, " ") != "a.json:a b.yaml:b d.yml:d" {
		t.Errorf("loaded %v", got)
	}
}
