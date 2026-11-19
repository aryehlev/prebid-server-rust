// Command httpclientgolden writes the goldens that pbs-http-client's tests compare the crate's
// clients with. Each golden records what Go 1.26.2's net/http client does, set up as Prebid
// Server's router.go and main.go set up their clients (router/router.go:143-190, main.go:72),
// for the inputs one pin uses: the request heads it writes (headers.json), its gzip handling
// (compression.json), its redirects under the default CheckRedirect (redirects.json), the
// request-target and Host of every adapter fixture URI and bidder-info endpoint and of synthetic
// URIs holding what those lack (targets.json), its connection limits and timeouts (limits.json),
// its error texts and their timeout classification (errors.json), the httptrace events Prebid's
// connection metrics read (metrics.json), the proxy x/net/http/httpproxy picks from a proxy
// environment and what a proxy receives (proxy.json), which certificates of a certificates_file
// x509.CertPool takes (certificates_file.json), what the default client negotiates over TLS
// (default_client.json), the ClientHello router.go's Transports send (client_hello.json), and
// the protocols the bidder client speaks, with the HTTP/2 that pbs-http-client's bidder client
// speaks in their place at the library level (h2.json).
//
// Run it from the directory of this module (crates/http-client/testdata/go) on amd64, as D-ARCH
// asks of every golden:
//
//	GOTOOLCHAIN=go1.26.2 GOARCH=amd64 CGO_ENABLED=1 CC="clang -arch x86_64" TZ=UTC go run .
//	GOTOOLCHAIN=go1.26.2 GOARCH=amd64 go run ./x509adv
//
// It writes one JSON file per subject into ../golden. Nothing in them depends on the clock, the
// process or the machine: loopback ports are written as placeholders (PORT, PA, PB, PROXY), and
// durations are recorded only as comparisons with the limit they test.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
)

// generator names this program in every header.
const generator = "crates/http-client/testdata/go"

// upstreamModule is the module path of the Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// header heads every golden: the toolchain, the architecture and the module versions the
// golden was generated with.
type header struct {
	GoVersion string            `json:"go_version"`
	GoArch    string            `json:"goarch"`
	Upstream  string            `json:"upstream"`
	Modules   map[string]string `json:"modules"`
	Generator string            `json:"generator"`
}

// newHeader records the versions of modules, the library modules one golden exercises, as this
// binary was built with them.
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

// writeGolden writes v as indented JSON, without HTML escaping, to dir/name.
func writeGolden(dir, name string, v any) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		panic(err)
	}
	if err := os.WriteFile(filepath.Join(dir, name), buf.Bytes(), 0o644); err != nil {
		panic(err)
	}
}

// proxyVariables are the variables http.ProxyFromEnvironment reads, and ALL_PROXY, which it does
// not read.
var proxyVariables = []string{
	"HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "NO_PROXY", "no_proxy",
	"REQUEST_METHOD", "ALL_PROXY", "all_proxy",
}

func main() {
	out := flag.String("out", filepath.Join("..", "golden"), "the directory the goldens are written to")
	flag.Parse()
	// http.ProxyFromEnvironment reads the environment once per process; the goldens record a
	// client with no proxy unless a case sets one on its Transport, whatever the shell exports.
	for _, name := range proxyVariables {
		if err := os.Unsetenv(name); err != nil {
			panic(err)
		}
	}
	writeGolden(*out, "headers.json", headersGolden())
	writeGolden(*out, "compression.json", compressionGolden())
	writeGolden(*out, "redirects.json", redirectsGolden())
	writeGolden(*out, "targets.json", targetsGolden())
	writeGolden(*out, "limits.json", limitsGolden())
	writeGolden(*out, "errors.json", errorsGolden())
	writeGolden(*out, "metrics.json", metricsGolden())
	writeGolden(*out, "proxy.json", proxyGolden())
	writeGolden(*out, "certificates_file.json", certificatesFileGolden())
	writeGolden(*out, "default_client.json", defaultClientGolden())
	writeGolden(*out, "client_hello.json", clientHelloGolden())
	writeGolden(*out, "h2.json", h2Golden())
}
