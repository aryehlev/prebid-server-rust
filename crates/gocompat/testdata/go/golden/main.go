// Command golden writes the golden files replayed by the pbs-gocompat Rust tests.
//
// Every output line is a tab-separated list of fields, each quoted with strconv.Quote.
// Large sweeps are summarised by an FNV-1a 64 checksum over their output, computed over
// values drawn from the splitmix64 generator that the Rust tests reimplement.
//
// Run from the repository root:
//
//	go run ./rust/crates/gocompat/testdata/go/golden rust/crates/gocompat/testdata/go/golden
package main

import (
	"fmt"
	"hash"
	"hash/fnv"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

var outDir string

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: golden <output dir>")
		os.Exit(2)
	}
	outDir = os.Args[1]
	genHTTP()
	genStrconv()
	genURL()
	genStrings()
	genBase64()
	genFmt()
	genTemplate()
	genTime()
}

// table accumulates golden rows for one file.
type table struct {
	b strings.Builder
}

func (t *table) row(fields ...string) {
	for i, f := range fields {
		if i > 0 {
			t.b.WriteByte('\t')
		}
		t.b.WriteString(strconv.Quote(f))
	}
	t.b.WriteByte('\n')
}

func (t *table) write(name string) {
	check(os.WriteFile(filepath.Join(outDir, name), []byte(t.b.String()), 0o644))
}

// check aborts the generator on an unexpected error.
func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// errText renders a result error for golden files: "" for nil, otherwise "error: " + text.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return "error: " + err.Error()
}

// splitmix64 is the deterministic generator shared with the Rust tests.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// sweep hashes newline-terminated outputs.
type sweep struct{ h hash.Hash64 }

func newSweep() *sweep { return &sweep{fnv.New64a()} }

func (s *sweep) add(v string) {
	s.h.Write([]byte(v))
	s.h.Write([]byte{'\n'})
}

func (s *sweep) sum() string { return fmt.Sprintf("%016x", s.h.Sum64()) }
