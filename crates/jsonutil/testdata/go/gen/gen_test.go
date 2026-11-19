package main

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

// TestGoldensAreCurrent builds every golden twice, which must give the same bytes, and compares
// the result with the committed file (the header's goarch and go_version aside: go-tooling.sh
// runs on the host architecture and toolchain, and nothing recorded depends on either).
func TestGoldensAreCurrent(t *testing.T) {
	for _, g := range goldens() {
		first := encode(g.build())
		second := encode(g.build())
		if !bytes.Equal(first, second) {
			t.Errorf("%s: two runs of the generator differ", g.name)
		}
		committed, err := os.ReadFile(filepath.Join(goldenDir(), g.name))
		if err != nil {
			t.Errorf("%s: %v", g.name, err)
			continue
		}
		if !sameGolden(first, committed, false) {
			t.Errorf("%s differs from what the generator writes; run go run ./gen", g.name)
		}
	}
}
