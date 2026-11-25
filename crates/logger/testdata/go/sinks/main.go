// Command sinks writes ../golden/sinks.json: where Prebid Server's log lines go under glog
// v1.2.5's flags. Each run is a child process of this program that parses its arguments with
// flag.CommandLine, as main.go:28 does, then logs through Prebid's logger package (the facade
// every Prebid package calls) or the standard log package (net/http's server and
// go-metrics-influxdb write there). The golden records the exit code, what reached stderr and,
// for each log file glog made, its name, header and lines.
//
// Each child runs with TMPDIR set to an empty directory (glog's os.TempDir() fallback) and
// with -log_dir and -log_link, when given, in other empty directories. Nothing in the golden
// depends on the clock, the machine or the process: glog's time, thread id, host and user, file
// names' times and pids, and stack dumps' frames are replaced by placeholders. From
// crates/logger/testdata/go, with GOTOOLCHAIN=go1.26.2 GOPROXY=off GOFLAGS=-mod=readonly TZ=UTC
// GOARCH=amd64 (D-ARCH):
//
//	go run ./sinks          writes ../golden/sinks.json
//	go run ./sinks -check   exits 1 when the file differs from what this program writes
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
)

// generator names this program in the header.
const generator = "crates/logger/testdata/go/sinks"

// goldenPath is where the golden lives, from this module's directory.
var goldenPath = filepath.Join("..", "golden", "sinks.json")

type header struct {
	Generator string `json:"generator"`
	GoVersion string `json:"go_version"`
	GoArch    string `json:"goarch"`
	Glog      string `json:"glog"`
	Upstream  string `json:"upstream"`
}

func newHeader() header {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		fail("the binary carries no build information")
	}
	h := header{Generator: generator, GoVersion: runtime.Version(), GoArch: runtime.GOARCH}
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
	if scenario := os.Getenv(childEnv); scenario != "" {
		child(scenario)
		return
	}
	opts := flag.NewFlagSet("sinks", flag.ExitOnError)
	check := opts.Bool("check", false, "compare the golden with what this program writes instead of writing it")
	if err := opts.Parse(os.Args[1:]); err != nil {
		fail(err.Error())
	}
	data := encode(build(newHeader()))
	if !*check {
		if err := os.WriteFile(goldenPath, data, 0o644); err != nil {
			fail(err.Error())
		}
		return
	}
	committed, err := os.ReadFile(goldenPath)
	if err != nil {
		fail(err.Error())
	}
	if !bytes.Equal(committed, data) {
		fmt.Fprintf(os.Stderr, "%s is stale: run `go run ./sinks` in crates/logger/testdata/go\n", goldenPath)
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
	fmt.Fprintln(os.Stderr, "sinks:", msg)
	os.Exit(1)
}
