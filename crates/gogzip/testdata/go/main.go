// Command gogzipgolden writes the golden files that the pbs-gogzip Rust tests replay.
//
// It drives Go's compress/flate and compress/gzip writers and readers over the corpus of
// corpus.go (which the Rust tests regenerate), fixtures of the upstream server collected into
// ../corpus, and the test vectors of Go's own compress/flate and compress/gzip tests, read
// from GOROOT (gotests.go). Run it from the directory of this module
// (crates/gogzip/testdata/go) with a toolchain of the Go version the upstream go.mod declares
// (1.25):
//
//	GOTOOLCHAIN=go1.25.7 go run .
//
// The fixtures are read from the Go root (see goRoot): $PBS_GO_ROOT, else .upstream at the
// workspace root, which scripts/fetch-upstream.sh checks out.
//
// The outputs go next to this directory: ../golden (the golden files), ../corpus (the
// fixture inputs) and ../flate (Go's compress/flate testdata). With -out DIR they go under
// DIR instead, so a run with another toolchain, such as the Go 1.26.2 the Dockerfile builds
// with, can be compared:
//
//	GOTOOLCHAIN=go1.26.2 go run . -out /tmp/gogzip
//	diff -r ../golden /tmp/gogzip/golden
//
// Golden lines are tab separated. Outputs are recorded as LEN.FNV.CALLS: the byte count, the
// FNV-1a 64 hash and the number of Write calls the underlying writer saw. The *_edges.txt
// files (edges.go) add the edge cases of the port's review and generated cases, with the
// lengths of the Write calls too.
package main

import (
	"bufio"
	"flag"
	"fmt"
	"hash/fnv"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

// golden writes lines to one golden file.
type golden struct {
	f *os.File
	w *bufio.Writer
}

func newGolden(dir, name string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		panic(err)
	}
	return &golden{f: f, w: bufio.NewWriter(f)}
}

func (g *golden) line(fields ...string) {
	for _, field := range fields {
		if strings.ContainsAny(field, "\t\n") {
			panic(fmt.Sprintf("field %q has a tab or newline", field))
		}
	}
	if _, err := g.w.WriteString(strings.Join(fields, "\t") + "\n"); err != nil {
		panic(err)
	}
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		panic(err)
	}
	if err := g.f.Close(); err != nil {
		panic(err)
	}
}

func fnv64(b []byte) uint64 {
	h := fnv.New64a()
	h.Write(b)
	return h.Sum64()
}

func writeFile(path string, data []byte) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		panic(err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		panic(err)
	}
}

// moduleDir is the directory of this module, crates/gogzip/testdata/go.
func moduleDir() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("no caller information")
	}
	return filepath.Dir(file)
}

// fixtureCorpus concatenates JSON fixtures of the upstream checkout root, in path order, into
// 200000 bytes of real OpenRTB requests and exchange test cases.
func fixtureCorpus(root string) []byte {
	var paths []string
	for _, pattern := range []string{
		"endpoints/openrtb2/sample-requests/valid-whole/exemplary/*.json",
		"exchange/exchangetest/*.json",
	} {
		matches, err := filepath.Glob(filepath.Join(root, pattern))
		if err != nil {
			panic(err)
		}
		sort.Strings(matches)
		paths = append(paths, matches...)
	}
	const size = 200000
	var b []byte
	for _, p := range paths {
		data, err := os.ReadFile(p)
		if err != nil {
			panic(err)
		}
		b = append(b, data...)
		if len(b) >= size {
			return b[:size]
		}
	}
	panic("fixtures are smaller than the corpus size")
}

func main() {
	out := flag.String("out", "", "directory to write golden, corpus and flate under (default: next to this generator)")
	flag.Parse()

	root := goRoot()
	dir := *out
	if dir == "" {
		dir = filepath.Join(moduleDir(), "..")
	}
	goroot := runtime.GOROOT()
	corpusDir = filepath.Join(dir, "corpus")
	goldenDir := filepath.Join(dir, "golden")
	if err := os.MkdirAll(goldenDir, 0o755); err != nil {
		panic(err)
	}

	writeFile(filepath.Join(corpusDir, "fixtures.json"), fixtureCorpus(root))
	copyGoTestdata(goroot, dir)

	g := newGolden(goldenDir, "go_version.txt")
	g.line(runtime.Version())
	g.close()

	writeLevels(goldenDir)
	writeFlateWriter(goldenDir)
	writeFlateLarge(goldenDir)
	writeFlateDict(goldenDir)
	writeGzipWriter(goldenDir)
	writeFlateReader(goldenDir, goroot)
	writeGzipReader(goldenDir, goroot)
	writeBlocks(goldenDir, goroot)
	writeFlateEdges(goldenDir)
	writeGzipEdges(goldenDir)
	writeFlateReaderEdges(goldenDir)
	writeGzipReaderEdges(goldenDir)
}
