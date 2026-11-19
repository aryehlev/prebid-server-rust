// Command differencesgolden writes the goldens that pbs-differences' library pins compare the
// crates of the Rust stack with. Each golden records what the Go library Prebid Server builds on
// does with the inputs one pin uses: encoding/json and jsoniter (through util/jsonutil, with the
// RawMessage encoder main.go registers), strconv.ParseFloat, gopkg.in/yaml.v3, viper (through
// config.SetupViper), the net/http client, rs/cors (through router.SupportCORS),
// prometheus/client_golang, coocood/freecache, regexp, net/url, http.Cookie, time,
// compress/gzip, glog (through the logger package) and x/net/publicsuffix.
//
// Run it from the directory of this module (crates/differences/testdata/go) on amd64, as D-ARCH
// asks of every golden:
//
//	GOARCH=amd64 CGO_ENABLED=1 CC="clang -arch x86_64" go run .
//
// It writes one JSON file per library into ../golden. Nothing in them depends on the clock, the
// process or the machine: loopback ports and glog's time and pid are replaced by placeholders.
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
const generator = "crates/differences/testdata/go"

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

func main() {
	if os.Getenv(glogChildEnv) != "" {
		glogChild()
		return
	}
	out := flag.String("out", filepath.Join("..", "golden"), "the directory the goldens are written to")
	flag.Parse()
	// Anything the upstream packages log while the goldens are made goes to stderr, never to
	// glog's files in the temporary directory.
	if err := flag.Set("logtostderr", "true"); err != nil {
		panic(err)
	}
	writeGolden(*out, "json.json", jsonGolden())
	writeGolden(*out, "float_parse.json", floatParseGolden())
	writeGolden(*out, "yaml.json", yamlGolden())
	writeGolden(*out, "config.json", configGolden())
	writeGolden(*out, "http_client.json", httpClientGolden())
	writeGolden(*out, "server.json", serverGolden())
	writeGolden(*out, "metrics.json", metricsGolden())
	writeGolden(*out, "cache.json", cacheGolden())
	writeGolden(*out, "regex.json", regexGolden())
	writeGolden(*out, "url.json", urlGolden())
	writeGolden(*out, "cookie.json", cookieGolden())
	writeGolden(*out, "time.json", timeGolden())
	writeGolden(*out, "gzip.json", gzipGolden())
	writeGolden(*out, "logging.json", loggingGolden())
	writeGolden(*out, "psl.json", pslGolden())
}
