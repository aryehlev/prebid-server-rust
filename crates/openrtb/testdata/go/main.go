// Command golden writes the parity goldens that pbs-openrtb's tests replay byte for byte.
//
// Every OpenRTB JSON document of prebid-server's test corpora (the adapter fixtures under every
// fixture root of testdata/go_fixture_manifest.json, exchange tests, endpoint sample
// requests, first-party data tests, the native request and response markup they carry) and of
// the openrtb module's own testdata, generated mutations of every ported type (each field with a
// sample value, null, values of the wrong JSON type, out-of-range numbers, other key cases,
// duplicate keys; unknown fields, empty and malformed documents), and adversarial documents
// (adversarial.go) is decoded as its Go type twice: with jsonutil.Unmarshal (jsoniter, as
// prebid-server decodes) and with encoding/json.Unmarshal. Each result, also after a failed
// decode, is re-encoded with jsonutil.Marshal (jsoniter with the RawMessageExtension that main.go
// registers), json.Marshal, and a json.Encoder with SetEscapeHTML(false).
//
// Each case is one line of tab-separated strconv.QuoteToASCII fields:
//
//	doc type label jsoniterErr enc1 enc2 enc3 stdErr enc4 enc5 enc6
//
// doc is "@file:offset:length" (a byte range of the corpus file listed on the "#file" line with
// that index) or "'" and the document itself. The error fields are empty on success. An encoding
// is "~" (the document's own bytes), "=N" (the same bytes as encoding N), "+N" (encoding N and a
// newline), "'" and the bytes, "#length:fnv" (FNV-1a 64 of bytes over 64 long) or "!" and the
// error text. corpus.tsv lists every directory it walks on a "#root" line and every JSON file
// found there on a "#file" line, also files that hold no case, so that a new corpus file makes
// the parity test ask for regeneration.
//
// encode.tsv holds the encodings of values that no JSON document decodes to (NaN and infinite
// floats, invalid json.RawMessage bytes), built by encodeCases (encode.go): one line per value,
//
//	name enc1 enc2 enc3
//
// each encoding "'" and the bytes or "!" and the error text.
//
// shapes.jsonl holds the shape cases of every ported type (shapes.go): each JSON field with its
// sample, zero and null values and its integer extremes, decoded with both decoders and
// re-encoded with the matching encoder, and jsonutil.MergeClone of samples, null and values of
// the wrong JSON type into each field. pbs-openrtb's serde parity test (tests/parity/shapes.rs)
// compares its outputs semantically, so they are written in full; writeShapeCases documents the
// format. containers.jsonl holds the same cases, and edges of its own, in the same format, for
// p.Containers, a struct of the containers typegen maps through pbs-openrtb's gojson_bridge,
// which no OpenRTB type has (containers.go); pbs-openrtb's tests/parity/containers.rs replays
// it.
//
// Run it from the directory of this module, crates/openrtb/testdata/go (the toolchain the other
// goldens use):
//
//	GOTOOLCHAIN=go1.26.2 go run .
//
// The corpora are read from the upstream checkout the module builds against, by the
// repository-relative paths the goldens record (see enterUpstream).
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"hash/fnv"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"

	"github.com/prebid/prebid-server/v4/util/jsonutil"

	"github.com/prebid/openrtb/v20/openrtb2"
)

func init() {
	// As main.go does.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// inlineMax is the longest output written inline; longer ones are written as a hash.
const inlineMax = 64

func main() {
	out := flag.String("out", "../golden", "directory to write the goldens to")
	manifest := flag.String("manifest", "../../../../testdata/go_fixture_manifest.json",
		"the adapter test manifest that pbs-devtools writes from the adapters' *_test.go files")
	flag.Parse()

	dirs := moduleDirs()
	enterUpstream(dirs[upstreamModule], out, manifest)

	types := map[string]reflect.Type{}
	for _, p := range portedTypes {
		types[p.goType.String()] = p.goType
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}

	c := newCorpus(types)
	c.adapters(*manifest)
	c.exchange()
	c.endpoints()
	c.firstPartyData()
	write(filepath.Join(*out, "corpus.tsv"), c.roots, c.files, c.cases)

	m := newCorpus(types)
	m.module(dirs[openrtbModule])
	write(filepath.Join(*out, "module.tsv"), nil, nil, m.cases)

	var mutations []testCase
	for _, p := range portedTypes {
		mutations = append(mutations, mutate(p.goType)...)
	}
	write(filepath.Join(*out, "mutations.tsv"), nil, nil, mutations)

	adv := adversarial()
	write(filepath.Join(*out, "adversarial.tsv"), nil, nil, adv)

	enc := encodeCases()
	writeEncodings(filepath.Join(*out, "encode.tsv"), enc)

	shapes := writeShapes(filepath.Join(*out, "shapes.jsonl"))
	containers := writeContainers(filepath.Join(*out, "containers.jsonl"))

	log.Printf("corpus: %d roots, %d files, %d cases; module: %d cases; mutations: %d cases; "+
		"adversarial: %d cases; encode: %d values; shapes: %d cases; containers: %d cases",
		len(c.roots), len(c.files), len(c.cases), len(m.cases), len(mutations), len(adv), len(enc),
		shapes, containers)
}

// testCase is one document to decode as one Go type.
type testCase struct {
	ref   string
	data  []byte
	typ   reflect.Type
	label string
}

// corpusFile is a corpus file that cases point into.
type corpusFile struct {
	path string
	data []byte
}

type corpus struct {
	types map[string]reflect.Type
	roots []string
	files []corpusFile
	cases []testCase
	seen  map[string]bool
}

func newCorpus(types map[string]reflect.Type) *corpus {
	return &corpus{types: types, seen: map[string]bool{}}
}

func (c *corpus) typ(name string) reflect.Type {
	t, ok := c.types[name]
	if !ok {
		log.Fatalf("type %s is not ported", name)
	}
	return t
}

// add records raw (a byte range of the current file) as a document of type name, once per
// distinct (type, bytes), and the native markup it carries.
func (c *corpus) add(raw json.RawMessage, name, label string) {
	raw = bytes.TrimSpace(raw)
	if len(raw) == 0 || bytes.Equal(raw, []byte("null")) {
		return
	}
	file := len(c.files) - 1
	data := c.files[file].data
	off := bytes.Index(data, raw)
	if off < 0 {
		log.Fatalf("%s: %s is not a byte range of the file", c.files[file].path, label)
	}
	c.push(testCase{
		ref:   fmt.Sprintf("@%d:%d:%d", file, off, len(raw)),
		data:  raw,
		typ:   c.typ(name),
		label: label,
	})
	c.natives(raw, name, label)
}

// inline records a document that is not a byte range of a file (native markup, module files).
func (c *corpus) inline(data []byte, name, label string) {
	c.push(testCase{ref: "'" + string(data), data: data, typ: c.typ(name), label: label})
}

func (c *corpus) push(tc testCase) {
	key := tc.typ.String() + "\x00" + string(tc.data)
	if c.seen[key] {
		return
	}
	c.seen[key] = true
	c.cases = append(c.cases, tc)
}

// natives records the native request of every imp of a BidRequest and the native markup in
// every bid's adm, as prebid-server decodes them.
func (c *corpus) natives(raw json.RawMessage, name, label string) {
	switch name {
	case "openrtb2.BidRequest":
		var req openrtb2.BidRequest
		if json.Unmarshal(raw, &req) != nil {
			return
		}
		for i, imp := range req.Imp {
			if imp.Native != nil && strings.TrimSpace(imp.Native.Request) != "" {
				c.inline([]byte(imp.Native.Request), "request.Request",
					fmt.Sprintf("%s imp[%d].native.request", label, i))
			}
		}
	case "openrtb2.BidResponse":
		var resp openrtb2.BidResponse
		if json.Unmarshal(raw, &resp) != nil {
			return
		}
		for i, sb := range resp.SeatBid {
			for j := range sb.Bid {
				c.adm(sb.Bid[j].AdM, fmt.Sprintf("%s seatbid[%d].bid[%d].adm", label, i, j))
			}
		}
	case "openrtb2.Bid":
		var bid openrtb2.Bid
		if json.Unmarshal(raw, &bid) == nil {
			c.adm(bid.AdM, label+" adm")
		}
	}
}

func (c *corpus) adm(adm, label string) {
	trimmed := strings.TrimSpace(adm)
	if !strings.HasPrefix(trimmed, "{") {
		return
	}
	for _, key := range []string{`"assets"`, `"native"`, `"link"`, `"eventtrackers"`} {
		if strings.Contains(trimmed, key) {
			c.inline([]byte(adm), "response.Response", label)
			return
		}
	}
}

// open reads a corpus file, records it and makes it the current one; it reports whether the
// file decodes into v. Every file is recorded, so that the parity test can tell a new corpus
// file from one without cases.
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

// adapters harvests the JSON files below every adapterstest.RunJSONBidderTest fixture root.
func (c *corpus) adapters(fixtureManifest string) {
	var paths []string
	for _, root := range fixtureRoots(fixtureManifest) {
		paths = append(paths, c.jsonFiles(filepath.FromSlash(root))...)
	}
	for _, path := range paths {
		var spec struct {
			MockBidRequest json.RawMessage `json:"mockBidRequest"`
			HTTPCalls      []struct {
				ExpectedRequest struct {
					Body json.RawMessage `json:"body"`
				} `json:"expectedRequest"`
				MockResponse struct {
					Body json.RawMessage `json:"body"`
				} `json:"mockResponse"`
			} `json:"httpCalls"`
			ExpectedBidResponses []struct {
				Bids []struct {
					Bid json.RawMessage `json:"bid"`
				} `json:"bids"`
			} `json:"expectedBidResponses"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.add(spec.MockBidRequest, "openrtb2.BidRequest", "mockBidRequest")
		for i, call := range spec.HTTPCalls {
			if isObject(call.ExpectedRequest.Body) {
				c.add(call.ExpectedRequest.Body, "openrtb2.BidRequest",
					fmt.Sprintf("httpCalls[%d].expectedRequest.body", i))
			}
			if isObject(call.MockResponse.Body) {
				c.add(call.MockResponse.Body, "openrtb2.BidResponse",
					fmt.Sprintf("httpCalls[%d].mockResponse.body", i))
			}
		}
		for i, resp := range spec.ExpectedBidResponses {
			for j, bid := range resp.Bids {
				c.add(bid.Bid, "openrtb2.Bid", fmt.Sprintf("expectedBidResponses[%d].bids[%d].bid", i, j))
			}
		}
	}
}

func isObject(raw json.RawMessage) bool {
	trimmed := bytes.TrimSpace(raw)
	return len(trimmed) > 0 && trimmed[0] == '{'
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
				MockResponse struct {
					SeatBids []struct {
						Bids []struct {
							Bid json.RawMessage `json:"ortbBid"`
						} `json:"pbsBids"`
					} `json:"pbsSeatBids"`
				} `json:"mockResponse"`
			} `json:"outgoingRequests"`
			Response struct {
				Bids json.RawMessage `json:"bids"`
			} `json:"response"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.add(spec.IncomingRequest.OrtbRequest, "openrtb2.BidRequest", "incomingRequest.ortbRequest")
		bidders := make([]string, 0, len(spec.OutgoingRequests))
		for b := range spec.OutgoingRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			o := spec.OutgoingRequests[b]
			if o.ExpectRequest != nil {
				c.add(o.ExpectRequest.OrtbRequest, "openrtb2.BidRequest",
					fmt.Sprintf("outgoingRequests.%s.expectRequest.ortbRequest", b))
			}
			for i, sb := range o.MockResponse.SeatBids {
				for j, bid := range sb.Bids {
					c.add(bid.Bid, "openrtb2.Bid",
						fmt.Sprintf("outgoingRequests.%s.mockResponse.pbsSeatBids[%d].pbsBids[%d].ortbBid", b, i, j))
				}
			}
		}
		c.add(spec.Response.Bids, "openrtb2.BidResponse", "response.bids")
	}
}

// endpoints harvests endpoints/openrtb2/sample-requests/**/*.json (test_utils.go's testCase).
func (c *corpus) endpoints() {
	root := filepath.Join("endpoints", "openrtb2", "sample-requests")
	for _, path := range c.jsonFiles(root) {
		var spec struct {
			BidRequest                 json.RawMessage            `json:"mockBidRequest"`
			ExpectedValidatedBidReq    json.RawMessage            `json:"expectedValidatedBidRequest"`
			ExpectedMockBidderRequests map[string]json.RawMessage `json:"expectedMockBidderRequests"`
			ExpectedBidResponse        json.RawMessage            `json:"expectedBidResponse"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.add(spec.BidRequest, "openrtb2.BidRequest", "mockBidRequest")
		c.add(spec.ExpectedValidatedBidReq, "openrtb2.BidRequest", "expectedValidatedBidRequest")
		bidders := make([]string, 0, len(spec.ExpectedMockBidderRequests))
		for b := range spec.ExpectedMockBidderRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			c.add(spec.ExpectedMockBidderRequests[b], "openrtb2.BidRequest", "expectedMockBidderRequests."+b)
		}
		if isObject(spec.ExpectedBidResponse) {
			c.add(spec.ExpectedBidResponse, "openrtb2.BidResponse", "expectedBidResponse")
		}
	}
}

// firstPartyData harvests the request documents of firstpartydata/tests/resolvefpd and
// extractfpdforbidders (first_party_data_test.go's fpdFileForResolveFPD and fpdFile): the
// incoming request, and the outgoing request (per bidder for resolvefpd).
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
			c.add(spec.InputRequestData, "openrtb2.BidRequest", "inputRequestData")
			if dir == "extractfpdforbidders" {
				c.add(spec.OutputRequestData, "openrtb2.BidRequest", "outputRequestData")
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
				c.add(perBidder[b], "openrtb2.BidRequest", "outputRequestData."+b)
			}
		}
	}
}

// moduleTests are the openrtb module's marshaling tests: each testdata file with the type its
// test decodes it into.
var moduleTests = []struct {
	file string
	typ  string
}{
	{"adcom1/testdata/request-context.json", "adcom1.RequestContext"},
	{"adcom1/testdata/item-specifications.json", "adcom1.ItemSpec"},
	{"adcom1/testdata/media-response.json", "adcom1.BidMedia"},
	{"native1/request/testdata/v1.1/social-context.json", "request.Request"},
	{"native1/request/testdata/v1.1/content-context.json", "request.Request"},
	{"native1/request/testdata/v1.2/social-context.json", "request.Request"},
	{"native1/request/testdata/v1.2/content-context.json", "request.Request"},
	{"native1/request/testdata/v1.2/third-party.json", "request.Request"},
	{"native1/response/testdata/v1.1/clickout.json", "response.Response"},
	{"native1/response/testdata/v1.1/video.json", "response.Response"},
	{"native1/response/testdata/v1.2/clickout.json", "response.Response"},
	{"native1/response/testdata/v1.2/video.json", "response.Response"},
	{"native1/response/testdata/v1.2/third-party.json", "response.Response"},
	{"openrtb2/testdata/bid-request/2.5/simple-banner.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.5/expandable-creative.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.5/mobile.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.5/video.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.5/pmp-with-direct-deal.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.5/native-ad.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.6/simple-banner.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.6/expandable-creative.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.6/mobile.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.6/video.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-request/2.6/pmp-with-direct-deal.json", "openrtb2.BidRequest"},
	{"openrtb2/testdata/bid-response/2.5/ad-served-on-win-notice.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.5/vast-xml-document-returned-inline.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.5/direct-deal-ad-served-on-win-notice.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.5/native-markup-returned-inline.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.6/ad-served-on-win-notice.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.6/vast-xml-document-returned-inline.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.6/direct-deal-ad-served-on-win-notice.json", "openrtb2.BidResponse"},
	{"openrtb2/testdata/bid-response/2.6/native-markup-returned-inline.json", "openrtb2.BidResponse"},
	{"openrtb3/testdata/request.json", "openrtb3.Body"},
	{"openrtb3/testdata/response.json", "openrtb3.Body"},
}

// module records the openrtb module's testdata files, inline, labelled by file; files with the
// same bytes are kept, so that each of the module's tests has its row.
func (c *corpus) module(dir string) {
	for _, t := range moduleTests {
		data, err := os.ReadFile(filepath.Join(dir, filepath.FromSlash(t.file)))
		if err != nil {
			log.Fatal(err)
		}
		c.cases = append(c.cases, testCase{ref: "'" + string(data), data: data, typ: c.typ(t.typ), label: t.file})
	}
}

// result is one encoding: its bytes or its error.
type result struct {
	data []byte
	err  error
}

// decodeAndEncode decodes data into a new value of t with decode, then encodes the value the
// three ways.
func decodeAndEncode(data []byte, t reflect.Type, decode func([]byte, interface{}) error) (string, [3]result) {
	v := reflect.New(t)
	errText := ""
	if err := decode(data, v.Interface()); err != nil {
		errText = err.Error()
	}
	value := v.Elem().Interface()
	var out [3]result
	out[0].data, out[0].err = jsonutil.Marshal(value)
	out[1].data, out[1].err = json.Marshal(value)
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(value); err != nil {
		out[2].err = err
	} else {
		out[2].data = buf.Bytes()
	}
	return errText, out
}

// cell is an encoding as written: "~", "=N", "+N", "'bytes", "#len:fnv" or "!error".
func cell(r result, doc []byte, earlier []result) string {
	if r.err != nil {
		return "!" + r.err.Error()
	}
	if bytes.Equal(r.data, doc) {
		return "~"
	}
	for i, e := range earlier {
		if e.err == nil && bytes.Equal(e.data, r.data) {
			return "=" + strconv.Itoa(i+1)
		}
	}
	for i, e := range earlier {
		if e.err == nil && bytes.Equal(append(append([]byte(nil), e.data...), '\n'), r.data) {
			return "+" + strconv.Itoa(i+1)
		}
	}
	if len(r.data) <= inlineMax {
		return "'" + string(r.data)
	}
	return fmt.Sprintf("#%d:%016x", len(r.data), fnv64(r.data))
}

func fnv64(data []byte) uint64 {
	h := fnv.New64a()
	_, _ = h.Write(data)
	return h.Sum64()
}

// rowWriter writes rows of tab-separated strconv.QuoteToASCII fields.
type rowWriter struct {
	f *os.File
	w *bufio.Writer
}

func newRowWriter(path string) *rowWriter {
	f, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	rw := &rowWriter{f: f, w: bufio.NewWriter(f)}
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
	if err := rw.f.Close(); err != nil {
		log.Fatal(err)
	}
}

func write(path string, roots []string, files []corpusFile, cases []testCase) {
	rw := newRowWriter(path)
	row := rw.row
	for _, root := range roots {
		row("#root", root)
	}
	for _, cf := range files {
		row("#file", cf.path, fmt.Sprintf("%016x", fnv64(cf.data)))
	}
	for _, tc := range cases {
		iterErr, iterOut := decodeAndEncode(tc.data, tc.typ, jsonutil.Unmarshal)
		stdErr, stdOut := decodeAndEncode(tc.data, tc.typ, json.Unmarshal)
		all := append(iterOut[:], stdOut[:]...)
		cells := make([]string, len(all))
		for i, r := range all {
			cells[i] = cell(r, tc.data, all[:i])
		}
		row(tc.ref, tc.typ.String(), tc.label,
			iterErr, cells[0], cells[1], cells[2],
			stdErr, cells[3], cells[4], cells[5])
	}
	rw.close()
}
