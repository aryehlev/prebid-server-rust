// Command flags writes the goldens of Prebid Server's command line: what flag.Parse
// (main.go:28) does with the flags the binary registers. They are glog v1.2.5's nine
// (docs/DECISIONS.md, "getopts: glog's flags") and, when an argument starts with
// -httptest.serve= or --httptest.serve=, net/http/httptest's -httptest.serve: the binary links
// net/http/httptest, which endpoints/openrtb2/test_utils.go imports and so does testify's assert
// package, imported by hooks/hookexecution/test_utils.go and, through testify's mock package, by
// metrics/metrics_mock.go.
//
// flags.json is a matrix of command lines. Each runs in a child process of this program that
// imports the logger package and net/http/httptest, as main.go's imports do, and calls flag.Parse
// on flag.CommandLine with os.Args[0] set to prebid-server: the child exits 0 after -h or -help
// and 2 after an error, as flag.ExitOnError makes it, and otherwise prints every flag's value and
// the arguments left and exits 0. The golden records the outcome, the exit code, the first line
// written to stderr after an error, the usage listing and, when the arguments parse, the values
// and the arguments left; its httptest_rows are the command lines that hold -httptest.serve.
//
// flags_seeded.json holds digests of 50,000 command lines built from a seeded splitmix64, which
// tests/flags.rs rebuilds identically. They are parsed in this process by a ContinueOnError copy
// of glog's flags; the program checks that the copy and the child agree on every row of the
// matrix but the httptest rows, which the copy has no flag for, and on every 50th seeded command
// line before it writes anything.
//
// Nothing in the goldens depends on the clock, the machine or the architecture. From
// crates/logger/testdata/go, with GOTOOLCHAIN=go1.26.2 GOPROXY=off GOFLAGS=-mod=readonly TZ=UTC
// (and GOARCH=amd64, as D-ARCH asks of every golden):
//
//	go run ./flags          writes ../golden/flags.json and ../golden/flags_seeded.json
//	go run ./flags -check   exits 1 when either file differs from what this program writes
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	_ "net/http/httptest" // registers -httptest.serve when an argument asks for it, as in the binary
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"

	_ "github.com/prebid/prebid-server/v4/logger" // registers glog's flags, as main.go's imports do
)

// generator names this program in every header.
const generator = "crates/logger/testdata/go/flags"

// header heads both goldens: the toolchain and the module versions they were generated with.
type header struct {
	Generator string `json:"generator"`
	GoVersion string `json:"go_version"`
	Glog      string `json:"glog"`
	Upstream  string `json:"upstream"`
}

func newHeader() header {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		fail("the binary carries no build information")
	}
	h := header{Generator: generator, GoVersion: runtime.Version()}
	for _, dep := range info.Deps {
		switch dep.Path {
		case "github.com/golang/glog":
			h.Glog = dep.Version
		case "github.com/prebid/prebid-server/v4":
			h.Upstream = dep.Version
		}
	}
	return h
}

func main() {
	if os.Getenv(childEnv) != "" {
		child()
		return
	}
	// The program's own flags live in their own set: flag.CommandLine must hold glog's flags
	// alone, as Prebid Server's does when no argument starts with -httptest.serve=, for the
	// child's flag.Parse.
	opts := flag.NewFlagSet("flags", flag.ExitOnError)
	check := opts.Bool("check", false, "compare the goldens with what this program writes instead of writing them")
	out := opts.String("out", filepath.Join("..", "golden"), "the directory of the goldens")
	if err := opts.Parse(os.Args[1:]); err != nil {
		fail(err.Error())
	}
	h := newHeader()
	files := []struct {
		name string
		doc  any
	}{
		{"flags.json", buildMatrix(h)},
		{"flags_seeded.json", buildSeeded(h)},
	}
	stale := false
	for _, f := range files {
		path := filepath.Join(*out, f.name)
		data := encode(f.doc)
		if !*check {
			if err := os.WriteFile(path, data, 0o644); err != nil {
				fail(err.Error())
			}
			continue
		}
		committed, err := os.ReadFile(path)
		if err != nil {
			fail(err.Error())
		}
		if !bytes.Equal(committed, data) {
			fmt.Fprintf(os.Stderr, "%s is stale: run `go run ./flags` in crates/logger/testdata/go\n", path)
			stale = true
		}
	}
	if stale {
		os.Exit(1)
	}
}

// encode writes v as indented JSON with a final newline.
func encode(v any) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		fail(err.Error())
	}
	return buf.Bytes()
}

// fail ends the generator on an error of its own.
func fail(msg string) {
	fmt.Fprintln(os.Stderr, "flags:", msg)
	os.Exit(1)
}
