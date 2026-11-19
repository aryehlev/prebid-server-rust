package main

import (
	"bytes"
	"encoding/json"
	"os"
	"testing"
)

// withoutArch returns the golden with its header's goarch and go_version blanked, so a
// comparison covers everything else: go-tooling.sh runs on the host architecture and toolchain,
// and nothing recorded depends on either.
func withoutArch(b []byte) []byte {
	var d doc
	if err := json.Unmarshal(b, &d); err != nil {
		return b
	}
	d.Header.GoArch = ""
	d.Header.GoVersion = ""
	return encode(d)
}

// TestGoldenIsCurrent runs every case again and compares what the servers answer with the
// committed golden.
func TestGoldenIsCurrent(t *testing.T) {
	committed, err := os.ReadFile(defaultOut())
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(withoutArch(encode(generate())), withoutArch(committed)) {
		t.Error("wire.json differs from what the generator writes; run go run ./wire")
	}
}
