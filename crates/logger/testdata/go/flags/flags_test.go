package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"
)

// The child's flag.Parse needs flag.CommandLine without the testing package's flags, so these
// tests use the emulation, which `go run ./flags` checks against the child on every row of the
// matrix and every 50th seeded command line. The goldens are amd64's (D-ARCH); elsewhere the
// tests build the documents without comparing.

// TestSeededGoldenIsCurrent rebuilds flags_seeded.json in memory.
func TestSeededGoldenIsCurrent(t *testing.T) {
	want := encode(seeded(newHeader(), glogFlags(false), false))
	if runtime.GOARCH != "amd64" {
		t.Skipf("the goldens are amd64's (D-ARCH); built flags_seeded.json on %s without comparing", runtime.GOARCH)
	}
	path := filepath.Join("..", "..", "golden", "flags_seeded.json")
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Errorf("%s is stale: run `go run ./flags` in crates/logger/testdata/go", path)
	}
}

// TestMatrixRowsMatchEmulation parses every row of flags.json again and compares.
func TestMatrixRowsMatchEmulation(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "..", "golden", "flags.json"))
	if err != nil {
		t.Fatal(err)
	}
	var doc matrixDoc
	if err := json.Unmarshal(data, &doc); err != nil {
		t.Fatal(err)
	}
	flags := glogFlags(false)
	if usage := emulate(flags, []string{"-h"}).Stderr; usage != doc.Usage {
		t.Errorf("usage: got %q, want %q", usage, doc.Usage)
	}
	for _, golden := range doc.Rows {
		r := emulate(flags, golden.Args)
		got := matrixRow{Args: golden.Args, Outcome: r.Outcome}
		switch r.Outcome {
		case "help":
			code := 0
			got.Exit = &code
		case "error":
			code := 2
			got.Exit = &code
			got.ErrorLine = errorLine(r, doc.Usage)
		default:
			got.Values, got.Rest = r.Values, r.Rest
			if len(got.Rest) == 0 {
				got.Rest = nil // omitted from the golden
			}
		}
		if !reflect.DeepEqual(got, golden) {
			t.Errorf("%q: got %+v, want %+v", golden.Args, got, golden)
		}
	}
}
