package main

import (
	"bytes"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

// TestMain runs a child's scenario when the test binary is started as one.
func TestMain(m *testing.M) {
	if scenario := os.Getenv(childEnv); scenario != "" {
		child(scenario)
		os.Exit(0)
	}
	os.Exit(m.Run())
}

// TestGoldenIsCurrent rebuilds sinks.json in memory and fails when the committed file differs:
// run `go run ./sinks` and commit the result after a change to this program, the pinned
// upstream or glog. The golden is amd64's (D-ARCH: the header names the architecture), so on
// another architecture the test only builds it.
func TestGoldenIsCurrent(t *testing.T) {
	want := encode(build(newHeader()))
	if runtime.GOARCH != "amd64" {
		t.Skipf("the golden is amd64's (D-ARCH); built it on %s without comparing", runtime.GOARCH)
	}
	path := filepath.Join("..", "..", "golden", "sinks.json")
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Errorf("%s is stale: run `go run ./sinks` in crates/logger/testdata/go", path)
	}
}
