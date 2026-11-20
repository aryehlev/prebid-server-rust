package main

import (
	"os"
	"testing"
)

// TestGoldenIsCurrent writes the golden's text again and compares it with the committed file.
func TestGoldenIsCurrent(t *testing.T) {
	committed, err := os.ReadFile(defaultOut())
	if err != nil {
		t.Fatal(err)
	}
	if generate() != string(committed) {
		t.Error("gziphandler.golden differs from what the generator writes; run go run ./gziphandler")
	}
}
