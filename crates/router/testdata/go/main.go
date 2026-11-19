// Command routing writes testdata/golden/routing.json for pbs-router: what Prebid Server's public
// handler answers, built as main.go builds it (router.NoCache around router.SupportCORS around the
// httprouter of router.New), to requests that reach the edges of httprouter v1.3.0 and of the
// net/http server under it: unknown paths, trailing-slash and fixed-path redirects, 405 with
// Allow, the OPTIONS answers, HEAD, percent-encoded paths, a body larger than axum's default
// limit, and the headers NoCache and SupportCORS add.
//
// The routes are router/router.go's (lines 282-318, /vtrack included as when vtrack.enabled is
// set), each answered by a stand-in that names the route, its parameters and the number of body
// bytes it read. crates/router/tests/routing.rs serves the same table on axum and sends the same
// bytes.
//
// Run it from the directory of this module (crates/router/testdata/go) on amd64, as D-ARCH asks of
// every golden:
//
//	GOARCH=amd64 CGO_ENABLED=1 CC="clang -arch x86_64" go run .
//
// golden_test.go regenerates the document and fails when it differs from the committed one.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"runtime/debug"
	"strings"
	"time"

	"github.com/julienschmidt/httprouter"

	"github.com/prebid/prebid-server/v4/router"
)

// generator names this program in the golden's header.
const generator = "crates/router/testdata/go"

// upstreamModule is the module path of the Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// host is the Host header of every request.
const host = "pbs.example"

// origin is the Origin of the cross-origin requests.
const origin = "https://publisher.example"

// header heads the golden: the toolchain, the architecture and the module versions it was
// generated with.
type header struct {
	GoVersion string            `json:"go_version"`
	GoArch    string            `json:"goarch"`
	Upstream  string            `json:"upstream"`
	Modules   map[string]string `json:"modules"`
	Generator string            `json:"generator"`
}

// newHeader records the versions of modules as this binary was built with them.
func newHeader(modules ...string) header {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		panic("the binary carries no build information")
	}
	versions := map[string]string{}
	for _, dep := range info.Deps {
		versions[dep.Path] = dep.Version
	}
	h := header{
		GoVersion: runtime.Version(),
		GoArch:    runtime.GOARCH,
		Upstream:  versions[upstreamModule],
		Modules:   map[string]string{},
		Generator: generator,
	}
	for _, m := range modules {
		version, ok := versions[m]
		if !ok {
			panic("module " + m + " is not part of the build")
		}
		h.Modules[m] = version
	}
	return h
}

// route is one route of router/router.go: its method, its httprouter path and the name its
// stand-in answers with.
type route struct {
	Method string `json:"method"`
	Path   string `json:"path"`
	Name   string `json:"name"`
}

// routes are router/router.go's routes in registration order (lines 282-318). /version is
// registered with r.Handler and /static/*filepath with r.ServeFiles; both are GET routes of the
// same tree.
var routes = []route{
	{"POST", "/openrtb2/auction", "auction"},
	{"POST", "/openrtb2/video", "video"},
	{"GET", "/openrtb2/amp", "amp"},
	{"GET", "/info/bidders", "bidders"},
	{"GET", "/info/bidders/:bidderName", "bidder"},
	{"GET", "/bidders/params", "params"},
	{"POST", "/cookie_sync", "cookie_sync"},
	{"GET", "/status", "status"},
	{"GET", "/", "index"},
	{"GET", "/version", "version"},
	{"GET", "/static/*filepath", "static"},
	{"POST", "/vtrack", "vtrack"},
	{"GET", "/event", "event"},
	{"GET", "/setuid", "setuid"},
	{"GET", "/getuids", "getuids"},
	{"POST", "/optout", "optout"},
	{"GET", "/optout", "optout"},
}

// stub answers for one route with a text/plain line: the route's name, its parameters in order
// and the number of body bytes it read.
func stub(name string) httprouter.Handle {
	return func(w http.ResponseWriter, r *http.Request, ps httprouter.Params) {
		n, err := io.Copy(io.Discard, r.Body)
		var b strings.Builder
		b.WriteString("route=" + name)
		for _, p := range ps {
			b.WriteString(" " + p.Key + "=" + p.Value)
		}
		fmt.Fprintf(&b, " body=%d", n)
		if err != nil {
			b.WriteString(" error=" + err.Error())
		}
		b.WriteString("\n")
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		io.WriteString(w, b.String())
	}
}

// prebidHandler is the public handler main.go serves (main.go:82-83): router.NoCache around
// router.SupportCORS around the router.
func prebidHandler() http.Handler {
	r := httprouter.New()
	for _, rt := range routes {
		r.Handle(rt.Method, rt.Path, stub(rt.Name))
	}
	return router.NoCache{Handler: router.SupportCORS(r)}
}

// spec is one request: the pin group it belongs to, its name, its request line and the header
// lines after Host, and the length of its body.
type spec struct {
	group, name, method, target string
	headers                     []string
	bodyLen                     int
}

// specs are the requests, by the pin of crates/router/tests/routing.rs that reads them.
var specs = []spec{
	{"not_found", "unknown_path", "GET", "/nope", nil, 0},
	{"not_found", "route_prefix", "GET", "/openrtb2", nil, 0},
	{"not_found", "extra_segment", "GET", "/info/bidders/appnexus/extra", nil, 0},
	{"not_found", "unknown_path_post", "POST", "/nope", nil, 2},

	{"redirects", "trailing_slash", "GET", "/status/", nil, 0},
	{"redirects", "trailing_slash_post", "POST", "/openrtb2/auction/", nil, 2},
	{"redirects", "trailing_slash_query", "GET", "/status/?a=1", nil, 0},
	{"redirects", "missing_slash", "GET", "/static", nil, 0},
	{"redirects", "case_fixed", "GET", "/STATUS", nil, 0},
	{"redirects", "case_fixed_post", "POST", "/OPENRTB2/AUCTION", nil, 2},
	{"redirects", "case_fixed_param", "GET", "/Info/Bidders/AppNexus", nil, 0},
	{"redirects", "double_slash", "GET", "//status", nil, 0},
	{"redirects", "dot_segments", "GET", "/info/../status", nil, 0},

	{"method_not_allowed", "put_get_route", "PUT", "/status", nil, 0},
	{"method_not_allowed", "delete_post_route", "DELETE", "/openrtb2/auction", nil, 0},
	{"method_not_allowed", "get_post_route", "GET", "/openrtb2/auction", nil, 0},
	{"method_not_allowed", "patch_get_post_route", "PATCH", "/optout", nil, 0},
	{"method_not_allowed", "post_param_route", "POST", "/info/bidders/appnexus", nil, 2},

	{"options", "options_get_route", "OPTIONS", "/status", nil, 0},
	{"options", "options_post_route", "OPTIONS", "/openrtb2/auction", nil, 0},
	{"options", "options_unknown", "OPTIONS", "/nope", nil, 0},
	{"options", "options_asterisk", "OPTIONS", "*", nil, 0},

	{"head", "head_get_route", "HEAD", "/status", nil, 0},
	{"head", "head_post_route", "HEAD", "/openrtb2/auction", nil, 0},
	{"head", "head_unknown", "HEAD", "/nope", nil, 0},

	{"decoded_path", "encoded_letter", "GET", "/st%61tus", nil, 0},
	{"decoded_path", "encoded_param", "GET", "/info/bidders/app%6Eexus", nil, 0},
	{"decoded_path", "encoded_slash_param", "GET", "/info/bidders/a%2Fb", nil, 0},
	{"decoded_path", "encoded_trailing_slash", "GET", "/status%2F", nil, 0},
	{"decoded_path", "encoded_query_mark", "GET", "/status%3Fa=1", nil, 0},
	{"decoded_path", "invalid_escape", "GET", "/status%zz", nil, 0},
	{"decoded_path", "static_root", "GET", "/static/", nil, 0},
	{"decoded_path", "static_file", "GET", "/static/a%20b/c.txt", nil, 0},

	{"body_limit", "auction_3mib", "POST", "/openrtb2/auction", nil, 3 << 20},
	{"body_limit", "auction_small", "POST", "/openrtb2/auction", nil, 100},

	{"middleware", "get_with_origin", "GET", "/status", []string{"Origin: " + origin}, 0},
	{"middleware", "post_without_origin", "POST", "/openrtb2/auction", nil, 2},
	{"middleware", "preflight", "OPTIONS", "/openrtb2/auction", []string{
		"Origin: " + origin, "Access-Control-Request-Method: POST",
		"Access-Control-Request-Headers: content-type",
	}, 0},
	{"middleware", "not_found_with_origin", "GET", "/nope", []string{"Origin: " + origin}, 0},
}

// result is one request and the server's answer. The request is requestHead followed by bodyLen
// bytes of pattern; the answer's Date value is replaced by "<date>".
type result struct {
	Group        string `json:"group"`
	Name         string `json:"name"`
	RequestHead  string `json:"request_head"`
	BodyLen      int    `json:"body_len"`
	ResponseHead string `json:"response_head"`
	Body         string `json:"body"`
}

type doc struct {
	Header header   `json:"header"`
	Host   string   `json:"host"`
	Origin string   `json:"origin"`
	Routes []route  `json:"routes"`
	Cases  []result `json:"cases"`
}

// requestHead is the request line and header block of s: Host, the spec's headers, the body's
// Content-Length when it has one, and Connection: close, so the server closes the connection
// after its answer.
func requestHead(s spec) string {
	var b strings.Builder
	b.WriteString(s.method + " " + s.target + " HTTP/1.1\r\nHost: " + host + "\r\n")
	for _, h := range s.headers {
		b.WriteString(h + "\r\n")
	}
	if s.bodyLen > 0 {
		fmt.Fprintf(&b, "Content-Length: %d\r\n", s.bodyLen)
	}
	b.WriteString("Connection: close\r\n\r\n")
	return b.String()
}

// pattern is n bytes cycling through the alphabet.
func pattern(n int) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte('a' + i%26)
	}
	return b
}

var dateLine = regexp.MustCompile(`(?m)^Date: [^\r]*`)

// exchange sends one request to addr and reads the answer until the server closes the
// connection.
func exchange(addr string, s spec) result {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		panic(err)
	}
	defer conn.Close()
	if err := conn.SetDeadline(time.Now().Add(20 * time.Second)); err != nil {
		panic(err)
	}
	head := requestHead(s)
	go func() {
		// The server may answer before it reads the whole body; a write error then is expected.
		conn.Write(append([]byte(head), pattern(s.bodyLen)...))
	}()
	answer, err := io.ReadAll(conn)
	if err != nil {
		panic(fmt.Sprintf("%s: %v", s.name, err))
	}
	responseHead, body, ok := bytes.Cut(answer, []byte("\r\n\r\n"))
	if !ok {
		panic(fmt.Sprintf("%s: no header block in %q", s.name, answer))
	}
	return result{
		Group:        s.group,
		Name:         s.name,
		RequestHead:  head,
		BodyLen:      s.bodyLen,
		ResponseHead: dateLine.ReplaceAllString(string(responseHead), "Date: <date>"),
		Body:         string(body),
	}
}

// generate serves prebidHandler on loopback and sends every spec to it.
func generate() doc {
	srv := httptest.NewServer(prebidHandler())
	defer srv.Close()
	addr := srv.Listener.Addr().String()
	d := doc{
		Header: newHeader("github.com/julienschmidt/httprouter", "github.com/rs/cors"),
		Host:   host,
		Origin: origin,
		Routes: routes,
	}
	for _, s := range specs {
		d.Cases = append(d.Cases, exchange(addr, s))
	}
	return d
}

// encode writes d as indented JSON without HTML escaping.
func encode(d doc) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(d); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

func defaultOut() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "golden", "routing.json")
}

func main() {
	out := flag.String("out", defaultOut(), "the golden to write")
	flag.Parse()
	if err := os.WriteFile(*out, encode(generate()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
