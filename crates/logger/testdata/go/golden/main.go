// Command golden writes the golden files replayed by the pbs-logger Rust tests.
//
// Every output line is a tab-separated list of fields, each quoted with strconv.Quote. The
// program drives github.com/golang/glog v1.2.5 (its flags, its stderr line format and its log
// files) and github.com/chasex/glog (the file logger of the filesystem analytics module), the
// two logging libraries Prebid Server uses.
//
// Run from the directory of this module (crates/logger/testdata/go) with the toolchain the
// Dockerfile builds with:
//
//	go run ./golden ../golden
package main

import (
	"fmt"
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
	genFlags()
	genLines()
	genFiles()
	genChasex()
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

// errText renders an error for golden files: "" for nil, otherwise its text.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}
