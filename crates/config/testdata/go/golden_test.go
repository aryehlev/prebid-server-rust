package main

import (
	"bytes"
	"os"
	"runtime"
	"testing"
)

// TestGoldensAreCurrent regenerates every golden in memory and fails when the committed file
// differs: run `go run .` and commit the result after a change to this program, the pinned
// upstream or the Go modules. The goldens are amd64's (D-ARCH: mapstructure's float-to-uint
// conversions and the header differ on arm64), so on another architecture the test only checks
// that every golden is generated without failing.
func TestGoldensAreCurrent(t *testing.T) {
	for _, g := range goldens {
		want := encodeGolden(g.doc())
		if runtime.GOARCH != "amd64" {
			continue
		}
		got, err := os.ReadFile(g.path)
		if err != nil {
			t.Fatalf("%s: %v", g.path, err)
		}
		if !bytes.Equal(got, want) {
			t.Errorf("%s is stale: run `go run .` in crates/config/testdata/go", g.path)
		}
	}
	if runtime.GOARCH != "amd64" {
		t.Skipf("the goldens are amd64's (D-ARCH); generated them on %s without comparing", runtime.GOARCH)
	}
}
