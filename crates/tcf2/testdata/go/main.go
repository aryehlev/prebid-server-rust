// Command tcf2golden records how github.com/prebid/go-gdpr (the exact version pinned by the
// upstream go.mod, which this module's go.mod requires too) and the Go standard library pieces it
// relies on behave for a large corpus of consent strings, base64 inputs, bit offsets and global
// vendor list documents.
//
// The pbs-tcf2 Rust tests replay the JSON files written by this program and require identical
// results, including byte-identical error text and the places where Go panics.
//
// Run it from the directory of this module (crates/tcf2/testdata/go) with the Go toolchain used
// by the production Dockerfile:
//
//	GOTOOLCHAIN=go1.26.2 go run . -out ..
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strings"
)

type header struct {
	GoVersion     string `json:"go_version"`
	GoGDPRVersion string `json:"go_gdpr_version"`
	Generator     string `json:"generator"`
}

func main() {
	out := flag.String("out", ".", "directory the golden JSON files are written to")
	corpus := flag.String("corpus", "", "directory holding corpus_*.txt (defaults to the directory of this program)")
	flag.Parse()

	corpusDir := *corpus
	if corpusDir == "" {
		corpusDir = filepath.Join(*out, "go")
	}

	hdr := header{
		GoVersion:     runtime.Version(),
		GoGDPRVersion: goGDPRVersion(),
		Generator:     "rust/crates/tcf2/testdata/go",
	}

	gogdpr := readLines(filepath.Join(corpusDir, "corpus_gogdpr.txt"))
	repo := readLines(filepath.Join(corpusDir, "corpus_repo.txt"))

	writeCases(filepath.Join(*out, "consent_goldens.json"), hdr, buildConsentCases(gogdpr, repo))
	writeCases(filepath.Join(*out, "bitutils_goldens.json"), hdr, buildBitutilsCases())
	writeCases(filepath.Join(*out, "base64_goldens.json"), hdr, buildBase64Cases())
	writeCases(filepath.Join(*out, "vendorlist_goldens.json"), hdr, buildVendorListCases())
	writeCases(filepath.Join(*out, "consent_fuzz_goldens.json"), hdr, buildFuzzConsentCases())
	writeCases(filepath.Join(*out, "vendorlist_fuzz_goldens.json"), hdr, buildFuzzVendorListCases())
}

func goGDPRVersion() string {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return "unknown"
	}
	for _, dep := range info.Deps {
		if dep.Path == "github.com/prebid/go-gdpr" {
			return dep.Version
		}
	}
	return "unknown"
}

func readLines(path string) []string {
	f, err := os.Open(path)
	if err != nil {
		fail(err)
	}
	defer f.Close()
	var lines []string
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := strings.TrimRight(sc.Text(), "\r")
		if line != "" {
			lines = append(lines, line)
		}
	}
	if err := sc.Err(); err != nil {
		fail(err)
	}
	return lines
}

// writeCases writes {"header":...,"cases":[...]} with one case per line so diffs stay readable.
func writeCases[T any](path string, hdr header, cases []T) {
	f, err := os.Create(path)
	if err != nil {
		fail(err)
	}
	w := bufio.NewWriter(f)
	h, err := json.Marshal(hdr)
	if err != nil {
		fail(err)
	}
	fmt.Fprintf(w, "{\"header\":%s,\n\"cases\":[\n", h)
	for i, c := range cases {
		b, err := json.Marshal(c)
		if err != nil {
			fail(err)
		}
		sep := ","
		if i == len(cases)-1 {
			sep = ""
		}
		fmt.Fprintf(w, "%s%s\n", b, sep)
	}
	fmt.Fprint(w, "]}\n")
	if err := w.Flush(); err != nil {
		fail(err)
	}
	if err := f.Close(); err != nil {
		fail(err)
	}
	fmt.Fprintf(os.Stderr, "wrote %d cases to %s\n", len(cases), path)
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}

// catch runs f and returns the panic message if it panicked.
func catch(f func()) (msg string, panicked bool) {
	defer func() {
		if r := recover(); r != nil {
			msg = fmt.Sprint(r)
			panicked = true
		}
	}()
	f()
	return "", false
}
