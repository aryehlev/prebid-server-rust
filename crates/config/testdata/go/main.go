// Command configgolden writes the goldens pbs-config's loader is checked against, and the Go
// type table the loader decodes with. Everything comes from the real Go code at the pinned
// upstream commit: config.SetupViper and the v.Unmarshal of config.New (Viper v1.12.0, with
// mapstructure v1.5.0 and cast v1.5.0 under it), and yaml.v3 as Viper and config/bidderinfo.go
// use it.
//
// Run it from the directory of this module (crates/config/testdata/go) on amd64, as D-ARCH asks
// of every golden:
//
//	TZ=UTC GOARCH=amd64 CGO_ENABLED=1 CC="clang -arch x86_64" go run .
//
// It writes ../../src/loader/go_types.json (the type table, which the loader embeds) and one
// JSON file per topic into ../golden. `go test ./...` regenerates them in memory and fails when
// a committed file differs. The process environment and the working directory never leak in:
// every PBS_ variable and every variable a case names is cleared before each load, and each
// load that reads a file runs in a fresh temporary directory.
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
const generator = "crates/config/testdata/go"

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

// encodeGolden is v as indented JSON without HTML escaping, as the files hold it.
func encodeGolden(v any) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

// golden is one file this program writes: its path relative to this module and its document.
type golden struct {
	path string
	doc  func() any
}

// goldens lists every file, in the order they are written.
var goldens = []golden{
	{filepath.Join("..", "..", "src", "loader", "go_types.json"), func() any { return schemaGolden() }},
	{filepath.Join("..", "golden", "loads.json"), func() any { return loadsGolden() }},
	{filepath.Join("..", "golden", "matrix.json"), func() any { return matrixGolden() }},
	{filepath.Join("..", "golden", "scalars.json"), func() any { return scalarsGolden() }},
	{filepath.Join("..", "golden", "bidder_info.json"), func() any { return bidderInfoGolden() }},
}

func main() {
	flag.Parse()
	for _, g := range goldens {
		if err := os.WriteFile(g.path, encodeGolden(g.doc()), 0o644); err != nil {
			panic(err)
		}
	}
}
