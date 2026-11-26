// Command request_wrapper writes the differential goldens of openrtb_ext's RequestWrapper and of
// convert_up.go and convert_down.go, which pbs-openrtb-ext's tests/parity/request_wrapper module
// replays, comparing JSON by its tokens and texts exactly.
//
// Every bid request of prebid-server's test corpora (the mockBidRequest and the ORTB bodies of
// the expected HTTP calls of the adapter fixtures under every fixture root of
// testdata/go_fixture_manifest.json, the incoming and expected outgoing requests of the
// exchange tests, the requests of the endpoint sample requests and of the first-party data tests)
// that jsonutil.Unmarshal decodes into an openrtb2.BidRequest, and generated mutations
// (mutations.go), is wrapped in a RequestWrapper and run through scripts of wrapper operations
// (steps.go): the ext getters, setters and touches (every member set to its own value), rebuilds,
// ConvertUpTo26, ConvertDownTo25, the Clear* functions, the impression wrappers' operations and
// the clones. corpus.tsv.gz holds the corpus requests, mutations.tsv.gz the mutations, and
// review.tsv.gz the review requests and scripts of review.go (the corpus requests run the review's
// corpus scripts there). Each script starts from a new decode of the request. After each step the
// step's observation is recorded: its error, the request as jsonutil.Marshal writes it, and what
// the getters return (each ext's members verbatim, its typed members as jsonutil.Marshal writes
// them, and its dirty flag).
//
// Each file starts with "#go" (the Go version), then lists the corpus directories walked on
// "#root" rows and every JSON file found there on "#file" rows (path and FNV-1a 64 of its bytes),
// also files without a request, so that a corpus change makes the test ask for regeneration.
// "#script" rows give each script's name and steps, and "#suite" rows the scripts of each suite.
// Each further row is one request:
//
//	doc label suite cell...
//
// with one cell per step of the suite's scripts, in order. doc is "@file:offset:length" (a byte
// range of the corpus file of that "#file" index), "'" and the document, or "^" for the document
// and label of the row before. A cell is "'" and the observation, "=N" (the same observation as
// cell N of the row), "!" and the panic text of a step that panicked, or "-" for the steps of a
// script after a panic. An observation is a sequence of parts, each its kind, its decimal length,
// ":" and its bytes (see obs). Fields are strconv.QuoteToASCII quoted and tab separated, and each
// file is gzip-compressed (without a name or a time, so that a run of the same toolchain writes
// the same bytes).
//
// Run it from the directory of this module, crates/openrtb-ext/testdata/go (the toolchain the
// other goldens use):
//
//	GOTOOLCHAIN=go1.26.2 go run ./request_wrapper
//
// The corpora are read from the upstream checkout the module builds against, by the
// repository-relative paths the goldens record (see enterUpstream).
package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"encoding/json"
	"flag"
	"fmt"
	"hash/fnv"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// As main.go does.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

func main() {
	out := flag.String("out", "request_wrapper", "directory to write the goldens to")
	manifest := flag.String("manifest", "../../../../testdata/go_fixture_manifest.json",
		"the adapter test manifest that pbs-devtools writes from the adapters' *_test.go files")
	flag.Parse()
	enterUpstream(out, manifest)

	c := &corpus{seen: map[string]bool{}}
	c.adapters(*manifest)
	c.exchange()
	c.endpoints()
	c.firstPartyData()
	corpusRows := make([]row, len(c.docs))
	for i, d := range c.docs {
		corpusRows[i] = row{doc: d, suite: corpusSuite}
	}
	write(filepath.Join(*out, "corpus.tsv.gz"), c.roots, c.files, corpusRows)

	mutationRows := mutations()
	write(filepath.Join(*out, "mutations.tsv.gz"), nil, nil, mutationRows)

	reviewRows := reviewRows(c.docs)
	write(filepath.Join(*out, "review.tsv.gz"), c.roots, c.files, reviewRows)

	log.Printf("corpus: %d roots, %d files, %d requests; mutations: %d rows; review: %d rows",
		len(c.roots), len(c.files), len(c.docs), len(mutationRows), len(reviewRows))
}

// doc is one bid request document.
type doc struct {
	// ref is "@file:offset:length" or "'" and the document.
	ref   string
	data  []byte
	label string
}

// row is one request and the suite of scripts it runs.
type row struct {
	doc   doc
	suite string
}

// corpusFile is a corpus file that documents point into.
type corpusFile struct {
	path string
	data []byte
}

type corpus struct {
	roots []string
	files []corpusFile
	docs  []doc
	seen  map[string]bool
}

// add records raw (a byte range of the current file) as a bid request, once per distinct bytes,
// when it is an object that jsonutil.Unmarshal decodes into an openrtb2.BidRequest.
func (c *corpus) add(raw json.RawMessage, label string) {
	raw = bytes.TrimSpace(raw)
	if !isObject(raw) || c.seen[string(raw)] {
		return
	}
	var req openrtb2.BidRequest
	if jsonutil.Unmarshal(raw, &req) != nil {
		return
	}
	c.seen[string(raw)] = true
	file := len(c.files) - 1
	off := bytes.Index(c.files[file].data, raw)
	if off < 0 {
		log.Fatalf("%s: %s is not a byte range of the file", c.files[file].path, label)
	}
	c.docs = append(c.docs, doc{
		ref:   fmt.Sprintf("@%d:%d:%d", file, off, len(raw)),
		data:  raw,
		label: label,
	})
}

func isObject(raw json.RawMessage) bool {
	trimmed := bytes.TrimSpace(raw)
	return len(trimmed) > 0 && trimmed[0] == '{'
}

// open reads a corpus file, records it and makes it the current one; it reports whether the
// file decodes into v. Every file is recorded, so that the replay can tell a new corpus file
// from one without requests.
func (c *corpus) open(path string, v interface{}) bool {
	data, err := os.ReadFile(path)
	if err != nil {
		log.Fatal(err)
	}
	c.files = append(c.files, corpusFile{path: filepath.ToSlash(path), data: data})
	return json.Unmarshal(data, v) == nil
}

// jsonFiles records root as walked and returns the JSON files below it, sorted.
func (c *corpus) jsonFiles(root string) []string {
	c.roots = append(c.roots, filepath.ToSlash(root))
	var paths []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() && strings.HasSuffix(path, ".json") {
			paths = append(paths, path)
		}
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	sort.Strings(paths)
	return paths
}

// fixtureRoots are the distinct fixture directories of every adapterstest.RunJSONBidderTest
// call in the adapter test manifest, sorted.
func fixtureRoots(fixtureManifest string) []string {
	data, err := os.ReadFile(fixtureManifest)
	if err != nil {
		log.Fatal(err)
	}
	var manifest struct {
		Runs []struct {
			FixtureRoot string `json:"fixture_root"`
		} `json:"runs"`
	}
	if err := json.Unmarshal(data, &manifest); err != nil {
		log.Fatalf("%s: %v", fixtureManifest, err)
	}
	seen := map[string]bool{}
	var roots []string
	for _, r := range manifest.Runs {
		if r.FixtureRoot != "" && !seen[r.FixtureRoot] {
			seen[r.FixtureRoot] = true
			roots = append(roots, r.FixtureRoot)
		}
	}
	sort.Strings(roots)
	return roots
}

// adapters harvests the JSON files below every adapterstest.RunJSONBidderTest fixture root: the
// PBS-format request and the bodies of the expected HTTP calls.
func (c *corpus) adapters(fixtureManifest string) {
	for _, root := range fixtureRoots(fixtureManifest) {
		for _, path := range c.jsonFiles(filepath.FromSlash(root)) {
			var spec struct {
				MockBidRequest json.RawMessage `json:"mockBidRequest"`
				HTTPCalls      []struct {
					ExpectedRequest struct {
						Body json.RawMessage `json:"body"`
					} `json:"expectedRequest"`
				} `json:"httpCalls"`
			}
			if !c.open(path, &spec) {
				continue
			}
			c.add(spec.MockBidRequest, "mockBidRequest")
			for i, call := range spec.HTTPCalls {
				c.add(call.ExpectedRequest.Body, fmt.Sprintf("httpCalls[%d].expectedRequest.body", i))
			}
		}
	}
}

// exchange harvests exchange/exchangetest/*.json (exchange_test.go's exchangeSpec).
func (c *corpus) exchange() {
	for _, path := range c.jsonFiles(filepath.Join("exchange", "exchangetest")) {
		var spec struct {
			IncomingRequest struct {
				OrtbRequest json.RawMessage `json:"ortbRequest"`
			} `json:"incomingRequest"`
			OutgoingRequests map[string]struct {
				ExpectRequest *struct {
					OrtbRequest json.RawMessage `json:"ortbRequest"`
				} `json:"expectRequest"`
			} `json:"outgoingRequests"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.add(spec.IncomingRequest.OrtbRequest, "incomingRequest.ortbRequest")
		bidders := make([]string, 0, len(spec.OutgoingRequests))
		for b := range spec.OutgoingRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			if o := spec.OutgoingRequests[b]; o.ExpectRequest != nil {
				c.add(o.ExpectRequest.OrtbRequest, "outgoingRequests."+b+".expectRequest.ortbRequest")
			}
		}
	}
}

// endpoints harvests endpoints/openrtb2/sample-requests/**/*.json (test_utils.go's testCase).
func (c *corpus) endpoints() {
	for _, path := range c.jsonFiles(filepath.Join("endpoints", "openrtb2", "sample-requests")) {
		var spec struct {
			BidRequest                 json.RawMessage            `json:"mockBidRequest"`
			ExpectedValidatedBidReq    json.RawMessage            `json:"expectedValidatedBidRequest"`
			ExpectedMockBidderRequests map[string]json.RawMessage `json:"expectedMockBidderRequests"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.add(spec.BidRequest, "mockBidRequest")
		c.add(spec.ExpectedValidatedBidReq, "expectedValidatedBidRequest")
		bidders := make([]string, 0, len(spec.ExpectedMockBidderRequests))
		for b := range spec.ExpectedMockBidderRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			c.add(spec.ExpectedMockBidderRequests[b], "expectedMockBidderRequests."+b)
		}
	}
}

// firstPartyData harvests the request documents of firstpartydata/tests/resolvefpd and
// extractfpdforbidders (first_party_data_test.go's fpdFileForResolveFPD and fpdFile).
func (c *corpus) firstPartyData() {
	for _, dir := range []string{"resolvefpd", "extractfpdforbidders"} {
		for _, path := range c.jsonFiles(filepath.Join("firstpartydata", "tests", dir)) {
			var spec struct {
				InputRequestData  json.RawMessage `json:"inputRequestData"`
				OutputRequestData json.RawMessage `json:"outputRequestData"`
			}
			if !c.open(path, &spec) {
				continue
			}
			c.add(spec.InputRequestData, "inputRequestData")
			if dir == "extractfpdforbidders" {
				c.add(spec.OutputRequestData, "outputRequestData")
				continue
			}
			var perBidder map[string]json.RawMessage
			if json.Unmarshal(spec.OutputRequestData, &perBidder) != nil {
				continue
			}
			bidders := make([]string, 0, len(perBidder))
			for b := range perBidder {
				bidders = append(bidders, b)
			}
			sort.Strings(bidders)
			for _, b := range bidders {
				c.add(perBidder[b], "outputRequestData."+b)
			}
		}
	}
}

// cells runs the scripts of a suite on a document and returns the cells of its row.
func cells(d doc, suite string) []string {
	var out []string
	var observations [][]byte
	for _, name := range suites[suite] {
		r := newRunner(d.data, d.label)
		stopped := false
		for _, step := range scripts[name] {
			if stopped {
				out = append(out, "-")
				observations = append(observations, nil)
				continue
			}
			o, panicked := r.step(step)
			if panicked {
				out = append(out, "!"+string(o.buf))
				observations = append(observations, nil)
				stopped = true
				continue
			}
			out = append(out, cell(o, observations))
			observations = append(observations, o.buf)
		}
	}
	return out
}

// cell is an observation as written: "=N" or "'" and the bytes.
func cell(o obs, earlier [][]byte) string {
	for i, e := range earlier {
		if e != nil && bytes.Equal(e, o.buf) {
			return "=" + strconv.Itoa(i+1)
		}
	}
	return "'" + string(o.buf)
}

func fnv64(data []byte) uint64 {
	h := fnv.New64a()
	_, _ = h.Write(data)
	return h.Sum64()
}

// rowWriter writes rows of tab-separated strconv.QuoteToASCII fields, gzip-compressed.
type rowWriter struct {
	f *os.File
	z *gzip.Writer
	w *bufio.Writer
}

func newRowWriter(path string) *rowWriter {
	f, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	z, err := gzip.NewWriterLevel(f, gzip.BestCompression)
	if err != nil {
		log.Fatal(err)
	}
	rw := &rowWriter{f: f, z: z, w: bufio.NewWriter(z)}
	rw.row("#go", runtime.Version())
	return rw
}

func (rw *rowWriter) row(fields ...string) {
	for i, field := range fields {
		if i > 0 {
			rw.w.WriteByte('\t')
		}
		rw.w.WriteString(strconv.QuoteToASCII(field))
	}
	rw.w.WriteByte('\n')
}

func (rw *rowWriter) close() {
	if err := rw.w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := rw.z.Close(); err != nil {
		log.Fatal(err)
	}
	if err := rw.f.Close(); err != nil {
		log.Fatal(err)
	}
}

// write writes a golden file: the corpus roots and files, the scripts and suites the rows use,
// and the rows.
func write(path string, roots []string, files []corpusFile, rows []row) {
	rw := newRowWriter(path)
	for _, root := range roots {
		rw.row("#root", root)
	}
	for _, cf := range files {
		rw.row("#file", cf.path, fmt.Sprintf("%016x", fnv64(cf.data)))
	}
	used := map[string]bool{}
	for _, r := range rows {
		used[r.suite] = true
	}
	suiteNames := make([]string, 0, len(used))
	for name := range used {
		suiteNames = append(suiteNames, name)
	}
	sort.Strings(suiteNames)
	written := map[string]bool{}
	for _, suite := range suiteNames {
		for _, name := range suites[suite] {
			if !written[name] {
				written[name] = true
				rw.row(append([]string{"#script", name}, scripts[name]...)...)
			}
		}
	}
	for _, suite := range suiteNames {
		rw.row(append([]string{"#suite", suite}, suites[suite]...)...)
	}
	previous := ""
	for _, r := range rows {
		ref, label := r.doc.ref, r.doc.label
		if ref == previous {
			ref, label = "^", ""
		} else {
			previous = ref
		}
		rw.row(append([]string{ref, label, r.suite}, cells(r.doc, r.suite)...)...)
	}
	rw.close()
}
