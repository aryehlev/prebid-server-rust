// Command gen writes the goldens pbs-jsonutil's tests compare with: util/jsonutil's FindElement
// and DropElement, MergeClone, gopkg.in/evanphx/json-patch.v5 MergePatch as jsonutil and the
// stored-request, account, first-party data and event merges call it, and the decode and encode
// behaviour of jsonutil.Unmarshal, UnmarshalValid and Marshal (with the RawMessage encoder
// main.go registers) that the crate's restored and ledgered differences pin.
//
// Run it from the directory of this module (crates/jsonutil/testdata/go) on amd64, as D-ARCH
// asks of every golden:
//
//	GOARCH=amd64 TZ=UTC go run ./gen
//
// It writes one JSON file per subject into ../golden. -check regenerates every golden in memory
// and fails when one differs from the committed file (the header's goarch excepted: nothing here
// depends on the architecture). -dump SET:BATCH prints the inputs and outcomes of one seeded batch
// (SET is element, merge_clone or merge_patch), which is how a mismatch the Rust tests report by
// batch is read.
//
// The seeded corpora are rebuilt from splitmix64 on both sides (the Rust tests implement the same
// generators), so the goldens hold per-batch SHA-256 digests of the inputs and of the outcomes;
// the curated cases and the corpus merges are recorded in full.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// generator names this program in every header.
const generator = "crates/jsonutil/testdata/go"

// upstreamModule is the module path of the Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

func init() {
	// main.go registers the extension at startup, so jsonutil.Marshal always writes RawMessage
	// fields through it (MergeClone's decoders are unaffected).
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// header heads every golden: the toolchain, the architecture and the module versions the golden
// was generated with.
type header struct {
	GoVersion string            `json:"go_version"`
	GoArch    string            `json:"goarch"`
	Upstream  string            `json:"upstream"`
	Modules   map[string]string `json:"modules"`
	Generator string            `json:"generator"`
}

// newHeader records the versions of modules, the library modules one golden exercises, as this
// binary was built with them.
func newHeader(modules ...string) header {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		panic("the binary carries no build information")
	}
	versions := map[string]string{}
	for _, dep := range info.Deps {
		versions[dep.Path] = dep.Version
	}
	h := header{
		GoVersion: runtime.Version(),
		GoArch:    runtime.GOARCH,
		Upstream:  versions[upstreamModule],
		Modules:   map[string]string{},
		Generator: generator,
	}
	for _, m := range modules {
		version, ok := versions[m]
		if !ok {
			panic("module " + m + " is not part of the build")
		}
		h.Modules[m] = version
	}
	return h
}

// moduleDir is the directory of this module (crates/jsonutil/testdata/go), wherever the program
// or its test runs from.
func moduleDir() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("no caller information")
	}
	return filepath.Dir(filepath.Dir(file))
}

// goldenDir is where the goldens live.
func goldenDir() string {
	return filepath.Join(moduleDir(), "..", "golden")
}

// golden is one file of ../golden and the function that builds its document.
type golden struct {
	name  string
	build func() any
}

// goldens lists every file the program writes.
func goldens() []golden {
	return []golden{
		{"element.json", func() any { return elementGolden() }},
		{"merge_clone.json", func() any { return mergeCloneGolden() }},
		{"merge_patch.json", func() any { return mergePatchGolden() }},
		{"decode.json", func() any { return decodeGolden() }},
	}
}

// encode writes v as indented JSON without HTML escaping.
func encode(v any) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

// withoutArch returns the golden with its header's goarch value blanked (and its go_version,
// when toolchain is false), so a comparison covers everything else.
func withoutArch(b []byte, toolchain bool) []byte {
	var doc map[string]json.RawMessage
	if err := json.Unmarshal(b, &doc); err != nil {
		return b
	}
	var h header
	if err := json.Unmarshal(doc["header"], &h); err != nil {
		return b
	}
	h.GoArch = ""
	if !toolchain {
		h.GoVersion = ""
	}
	raw, err := json.Marshal(h)
	if err != nil {
		panic(err)
	}
	doc["header"] = raw
	out, err := json.Marshal(doc)
	if err != nil {
		panic(err)
	}
	return out
}

// sameGolden reports whether two encodings of a golden agree, the header's goarch aside (and its
// go_version, when toolchain is false).
func sameGolden(a, b []byte, toolchain bool) bool {
	return bytes.Equal(withoutArch(a, toolchain), withoutArch(b, toolchain))
}

func main() {
	out := flag.String("out", goldenDir(), "the directory the goldens are written to")
	check := flag.Bool("check", false, "regenerate the goldens in memory and fail when one differs from its file")
	dump := flag.String("dump", "", "print the inputs and outcomes of one seeded batch: SET:BATCH")
	flag.Parse()
	if *dump != "" {
		if err := dumpBatch(*dump); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		return
	}
	failed := false
	for _, g := range goldens() {
		data := encode(g.build())
		path := filepath.Join(*out, g.name)
		if !*check {
			if err := os.WriteFile(path, data, 0o644); err != nil {
				panic(err)
			}
			continue
		}
		committed, err := os.ReadFile(path)
		if err != nil {
			fmt.Fprintf(os.Stderr, "%s: %v\n", path, err)
			failed = true
			continue
		}
		if !sameGolden(data, committed, true) {
			fmt.Fprintf(os.Stderr, "%s differs from what the generator writes\n", path)
			failed = true
		}
	}
	if failed {
		os.Exit(1)
	}
}

// dumpBatch prints one seeded batch: each input line, then its outcome line, tab-indented.
func dumpBatch(spec string) error {
	set, number, ok := strings.Cut(spec, ":")
	if !ok {
		return fmt.Errorf("-dump wants SET:BATCH, not %q", spec)
	}
	n, err := strconv.Atoi(number)
	if err != nil {
		return fmt.Errorf("-dump batch %q: %v", number, err)
	}
	var corpus seededCorpus
	switch set {
	case "element":
		corpus = elementCorpus()
	case "merge_clone":
		corpus = mergeCloneCorpus()
	case "merge_patch":
		corpus = mergePatchCorpus()
	default:
		return fmt.Errorf("-dump set %q: element, merge_clone or merge_patch", set)
	}
	if n < 0 || n >= corpus.count/corpus.batch {
		return fmt.Errorf("-dump batch %d: the set has %d batches", n, corpus.count/corpus.batch)
	}
	gen := splitmix64{state: corpus.seed}
	for i := 0; i < corpus.count; i++ {
		input, outcome := corpus.next(&gen)
		if i/corpus.batch == n {
			fmt.Printf("%d\t%q\n\t%q\n", i, input, outcome)
		}
	}
	return nil
}
