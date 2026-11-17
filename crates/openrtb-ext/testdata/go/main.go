// Command golden writes the parity goldens that pbs-openrtb-ext's tests replay byte for byte.
//
// Every Prebid extension object of prebid-server's test corpora (the adapter fixtures under every
// fixture root of rust/testdata/go_fixture_manifest.json, exchange tests, endpoint sample
// requests, first-party data tests) is decoded as the openrtb_ext type prebid-server decodes it
// into: request.ext and request.ext.prebid, imp.ext, imp.ext.prebid, imp.ext.data and the bidder
// params of imp.ext.bidder, imp.ext.prebid.bidder.<bidder> and imp.ext.<bidder> (as each
// ExtImp/ImpExt type that the bidder's adapter uses), site, app, publisher, device, user, regs and
// source ext and the keys request_wrapper.go decodes from them, response.ext, bid.ext and
// bid.ext.prebid, and video endpoint payloads. Generated mutations of every ported type
// (mutations.go) follow. Each document is decoded twice, with jsonutil.Unmarshal (jsoniter, as
// prebid-server decodes) and with encoding/json.Unmarshal, and each result, also after a failed
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
// functions.tsv holds the results of the package's functions and methods over generated inputs
// (functions.go), one call per line:
//
//	function label arg... "->" result...
//
// constants.tsv holds the name, Go type and value of every exported constant declared without a
// type (the metadata in rust/testdata/go_struct_meta.json lists the typed ones), checked against
// the package's source (constants.go).
//
// review.tsv (cases as in corpus.tsv), review_functions.tsv and review_aliases.tsv (calls as in
// functions.tsv) hold the generated edge cases of review.go: the types with their own JSON
// decoding at every nesting depth, the StringInt, IntString and json.Number fields, multibid,
// deal tiers, ATTS values, floors, bidder name folding, and random (also truncated or corrupted)
// documents of every ported type. review_aliases.tsv is written last, as it registers aliases.
// -review-scale and -review-seed change the random documents of review.tsv; the committed
// goldens use the defaults.
//
// Run it from the repository root (the toolchain the other goldens use):
//
//	GOTOOLCHAIN=go1.26.2 go run ./rust/crates/openrtb-ext/testdata/go
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"hash/fnv"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

func init() {
	// As main.go does.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

// inlineMax is the longest output written inline; longer ones are written as a hash.
const inlineMax = 64

// wrapperTypes are the ported types of request_wrapper.go, which its own port tests: they hold
// unexported state, not JSON.
var wrapperTypes = map[string]bool{
	"openrtb_ext.RequestWrapper": true,
	"openrtb_ext.ImpWrapper":     true,
	"openrtb_ext.UserExt":        true,
	"openrtb_ext.RequestExt":     true,
	"openrtb_ext.DeviceExt":      true,
	"openrtb_ext.AppExt":         true,
	"openrtb_ext.DOOHExt":        true,
	"openrtb_ext.RegExt":         true,
	"openrtb_ext.SiteExt":        true,
	"openrtb_ext.SourceExt":      true,
	"openrtb_ext.ImpExt":         true,
}

func main() {
	out := flag.String("out", "rust/crates/openrtb-ext/testdata/golden", "directory to write the goldens to")
	flag.Parse()
	decodable := decodableTypes()
	types := map[string]reflect.Type{}
	for _, t := range decodable {
		types[t.String()] = t
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		log.Fatal(err)
	}

	c := newCorpus(types, bidderParamTypes(types))
	c.adapters()
	c.exchange()
	c.endpoints()
	c.firstPartyData()
	write(filepath.Join(*out, "corpus.tsv"), c.roots, c.files, c.cases)

	var mutations []testCase
	for _, t := range decodable {
		mutations = append(mutations, mutate(t)...)
	}
	write(filepath.Join(*out, "mutations.tsv"), nil, nil, mutations)

	fns := functionCases()
	writeFunctions(filepath.Join(*out, "functions.tsv"), fns)

	consts := untypedConstantRows()
	writeConstants(filepath.Join(*out, "constants.tsv"), consts)

	review := reviewDecodeCases(types, decodable)
	write(filepath.Join(*out, "review.tsv"), nil, nil, review)
	reviewFns := reviewFunctionCases()
	writeFunctions(filepath.Join(*out, "review_functions.tsv"), reviewFns)
	// Last: registering aliases changes the package's state for good.
	aliases := reviewAliasCases()
	writeFunctions(filepath.Join(*out, "review_aliases.tsv"), aliases)

	log.Printf("corpus: %d roots, %d files, %d cases; mutations: %d cases over %d types; "+
		"functions: %d calls; constants: %d; review: %d cases, %d calls, %d alias calls",
		len(c.roots), len(c.files), len(c.cases), len(mutations), len(decodable), len(fns), len(consts),
		len(review), len(reviewFns), len(aliases))
}

// decodableTypes are the ported types that JSON documents decode into, in generation order:
// every ported type but functions, interfaces and request_wrapper.go's, then the unexported and
// unnamed struct types of their fields (which the port also names), in the order found.
func decodableTypes() []reflect.Type {
	var out []reflect.Type
	seen := map[reflect.Type]bool{}
	for _, p := range portedTypes {
		t := p.goType
		if t.Kind() == reflect.Func || t.Kind() == reflect.Interface || wrapperTypes[t.String()] {
			continue
		}
		seen[t] = true
		out = append(out, t)
	}
	var walk func(t reflect.Type)
	walk = func(t reflect.Type) {
		for t.Name() == "" && (t.Kind() == reflect.Ptr || t.Kind() == reflect.Slice || t.Kind() == reflect.Map) {
			t = t.Elem()
		}
		if t.Name() == "" && t.Kind() != reflect.Struct {
			return
		}
		if t.Name() != "" && !strings.HasSuffix(t.PkgPath(), "/openrtb_ext") {
			return
		}
		if t.Name() != "" && t.Name()[0] >= 'A' && t.Name()[0] <= 'Z' {
			return
		}
		if seen[t] {
			return
		}
		seen[t] = true
		out = append(out, t)
		if t.Kind() == reflect.Struct {
			for i := 0; i < t.NumField(); i++ {
				walk(t.Field(i).Type)
			}
		}
	}
	for _, t := range append([]reflect.Type(nil), out...) {
		if t.Kind() == reflect.Struct {
			for i := 0; i < t.NumField(); i++ {
				walk(t.Field(i).Type)
			}
		}
	}
	return out
}

// bidderParamTypes are the bidder params types of each adapter directory: the types that
// openrtb_ext declares in its imp_*.go files and the adapter's non-test files name.
func bidderParamTypes(types map[string]reflect.Type) map[string][]string {
	declared := map[string]bool{}
	fset := token.NewFileSet()
	paths, err := filepath.Glob(filepath.Join("openrtb_ext", "imp_*.go"))
	if err != nil {
		log.Fatal(err)
	}
	for _, path := range paths {
		if strings.HasSuffix(path, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			log.Fatal(err)
		}
		for _, decl := range file.Decls {
			gen, ok := decl.(*ast.GenDecl)
			if !ok || gen.Tok != token.TYPE {
				continue
			}
			for _, spec := range gen.Specs {
				name := spec.(*ast.TypeSpec).Name.Name
				if _, ported := types["openrtb_ext."+name]; ported {
					declared[name] = true
				}
			}
		}
	}
	named := regexp.MustCompile(`openrtb_ext\.([A-Za-z0-9_]+)`)
	dirs, err := os.ReadDir("adapters")
	if err != nil {
		log.Fatal(err)
	}
	out := map[string][]string{}
	for _, dir := range dirs {
		if !dir.IsDir() {
			continue
		}
		files, err := filepath.Glob(filepath.Join("adapters", dir.Name(), "*.go"))
		if err != nil {
			log.Fatal(err)
		}
		found := map[string]bool{}
		for _, path := range files {
			if strings.HasSuffix(path, "_test.go") {
				continue
			}
			data, err := os.ReadFile(path)
			if err != nil {
				log.Fatal(err)
			}
			for _, m := range named.FindAllSubmatch(data, -1) {
				if declared[string(m[1])] {
					found["openrtb_ext."+string(m[1])] = true
				}
			}
		}
		names := make([]string, 0, len(found))
		for name := range found {
			names = append(names, name)
		}
		sort.Strings(names)
		if len(names) > 0 {
			out[dir.Name()] = names
		}
	}
	return out
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
	// paramTypes are the bidder params types of each adapter directory.
	paramTypes map[string][]string
	roots      []string
	files      []corpusFile
	cases      []testCase
	seen       map[string]bool
}

func newCorpus(types map[string]reflect.Type, paramTypes map[string][]string) *corpus {
	return &corpus{types: types, paramTypes: paramTypes, seen: map[string]bool{}}
}

func (c *corpus) typ(name string) reflect.Type {
	t, ok := c.types[name]
	if !ok {
		log.Fatalf("type %s is not ported", name)
	}
	return t
}

// add records raw (a byte range of the current file) as a document of type name, once per
// distinct (type, bytes).
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
	tc := testCase{
		ref:   fmt.Sprintf("@%d:%d:%d", file, off, len(raw)),
		data:  raw,
		typ:   c.typ(name),
		label: label,
	}
	key := tc.typ.String() + "\x00" + string(tc.data)
	if c.seen[key] {
		return
	}
	c.seen[key] = true
	c.cases = append(c.cases, tc)
}

// members decodes an object into its members; nil when raw is not an object.
func members(raw json.RawMessage) map[string]json.RawMessage {
	if !isObject(raw) {
		return nil
	}
	var m map[string]json.RawMessage
	if json.Unmarshal(raw, &m) != nil {
		return nil
	}
	return m
}

func sortedKeys(m map[string]json.RawMessage) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// extKeys are the keys of an ext object that prebid-server decodes on their own (as
// request_wrapper.go does), with their types.
type extKey struct {
	key string
	typ string
}

// ext records an ext object as typ and each of keys present in it as its type.
func (c *corpus) ext(raw json.RawMessage, typ, label string, keys ...extKey) {
	c.add(raw, typ, label)
	m := members(raw)
	for _, k := range keys {
		if v, ok := m[k.key]; ok {
			c.add(v, k.typ, label+"."+k.key)
		}
	}
}

// request records the Prebid extensions of a PBS-format bid request; bidder is the adapter
// directory whose params imp.ext.bidder holds (adapter fixtures), or "".
func (c *corpus) request(raw json.RawMessage, label, bidder string) {
	m := members(raw)
	if m == nil {
		return
	}
	c.ext(m["ext"], "openrtb_ext.ExtRequest", label+".ext",
		extKey{"prebid", "openrtb_ext.ExtRequestPrebid"})
	var imps []json.RawMessage
	if json.Unmarshal(m["imp"], &imps) == nil {
		for i, imp := range imps {
			c.imp(members(imp)["ext"], fmt.Sprintf("%s.imp[%d].ext", label, i), bidder)
		}
	}
	for _, owner := range []string{"site", "app", "dooh"} {
		o := members(m[owner])
		if o == nil {
			continue
		}
		switch owner {
		case "site":
			c.add(o["ext"], "openrtb_ext.ExtSite", label+".site.ext")
		case "app":
			c.ext(o["ext"], "openrtb_ext.ExtApp", label+".app.ext",
				extKey{"prebid", "openrtb_ext.ExtAppPrebid"})
		}
		if p := members(o["publisher"]); p != nil {
			c.add(p["ext"], "openrtb_ext.ExtPublisher", label+"."+owner+".publisher.ext")
		}
	}
	if d := members(m["device"]); d != nil {
		c.ext(d["ext"], "openrtb_ext.ExtDevice", label+".device.ext",
			extKey{"prebid", "openrtb_ext.ExtDevicePrebid"})
	}
	if u := members(m["user"]); u != nil {
		c.ext(u["ext"], "openrtb_ext.ExtUser", label+".user.ext",
			extKey{"prebid", "openrtb_ext.ExtUserPrebid"},
			extKey{"ConsentedProvidersSettings", "openrtb_ext.ConsentedProvidersSettingsIn"},
			extKey{"consented_providers_settings", "openrtb_ext.ConsentedProvidersSettingsOut"})
	}
	if r := members(m["regs"]); r != nil {
		c.ext(r["ext"], "openrtb_ext.ExtRegs", label+".regs.ext",
			extKey{"dsa", "openrtb_ext.ExtRegsDSA"})
	}
	if s := members(m["source"]); s != nil {
		c.add(s["ext"], "openrtb_ext.ExtSource", label+".source.ext")
	}
}

// imp records an imp.ext: as a whole, its prebid and data, and the bidder params it holds.
func (c *corpus) imp(raw json.RawMessage, label, bidder string) {
	m := members(raw)
	if m == nil {
		return
	}
	c.add(raw, "openrtb_ext.ExtImp", label)
	c.add(m["prebid"], "openrtb_ext.ExtImpPrebid", label+".prebid")
	c.add(m["data"], "openrtb_ext.ExtImpData", label+".data")
	if bidder != "" {
		c.params(m["bidder"], bidder, label+".bidder")
	}
	for _, name := range sortedKeys(m) {
		c.params(m[name], name, label+"."+name)
	}
	prebidBidders := members(members(m["prebid"])["bidder"])
	for _, name := range sortedKeys(prebidBidders) {
		c.params(prebidBidders[name], name, label+".prebid.bidder."+name)
	}
}

// params records bidder params as each params type of the adapter directory named bidder.
func (c *corpus) params(raw json.RawMessage, bidder, label string) {
	for _, name := range c.paramTypes[bidder] {
		c.add(raw, name, label+" as "+strings.TrimPrefix(name, "openrtb_ext."))
	}
}

// response records the Prebid extensions of a bid response.
func (c *corpus) response(raw json.RawMessage, label string) {
	m := members(raw)
	if m == nil {
		return
	}
	c.add(m["ext"], "openrtb_ext.ExtBidResponse", label+".ext")
	var seatBids []json.RawMessage
	if json.Unmarshal(m["seatbid"], &seatBids) != nil {
		return
	}
	for i, sb := range seatBids {
		var bids []json.RawMessage
		if json.Unmarshal(members(sb)["bid"], &bids) != nil {
			continue
		}
		for j, bid := range bids {
			c.bid(bid, fmt.Sprintf("%s.seatbid[%d].bid[%d]", label, i, j))
		}
	}
}

// bid records the Prebid extensions of a bid.
func (c *corpus) bid(raw json.RawMessage, label string) {
	if m := members(raw); m != nil {
		c.ext(m["ext"], "openrtb_ext.ExtBid", label+".ext", extKey{"prebid", "openrtb_ext.ExtBidPrebid"})
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

// fixtureManifest is the adapter test manifest that pbs-devtools writes from the adapters'
// *_test.go files.
const fixtureManifest = "rust/testdata/go_fixture_manifest.json"

// fixtureRoots are the distinct fixture directories of every adapterstest.RunJSONBidderTest
// call, sorted.
func fixtureRoots() []string {
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

// adapterDir is the adapter directory of a fixture root (adapters/<dir>/...).
func adapterDir(root string) string {
	parts := strings.Split(root, "/")
	if len(parts) > 1 && parts[0] == "adapters" {
		return parts[1]
	}
	return ""
}

// adapters harvests the JSON files below every adapterstest.RunJSONBidderTest fixture root: the
// PBS-format request and the bids the adapter returns.
func (c *corpus) adapters() {
	for _, root := range fixtureRoots() {
		bidder := adapterDir(root)
		for _, path := range c.jsonFiles(filepath.FromSlash(root)) {
			var spec struct {
				MockBidRequest       json.RawMessage `json:"mockBidRequest"`
				ExpectedBidResponses []struct {
					Bids []struct {
						Bid json.RawMessage `json:"bid"`
					} `json:"bids"`
				} `json:"expectedBidResponses"`
			}
			if !c.open(path, &spec) {
				continue
			}
			c.request(spec.MockBidRequest, "mockBidRequest", bidder)
			for i, resp := range spec.ExpectedBidResponses {
				for j, bid := range resp.Bids {
					c.bid(bid.Bid, fmt.Sprintf("expectedBidResponses[%d].bids[%d].bid", i, j))
				}
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
				Ext  json.RawMessage `json:"ext"`
			} `json:"response"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.request(spec.IncomingRequest.OrtbRequest, "incomingRequest.ortbRequest", "")
		bidders := make([]string, 0, len(spec.OutgoingRequests))
		for b := range spec.OutgoingRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			o := spec.OutgoingRequests[b]
			if o.ExpectRequest != nil {
				c.request(o.ExpectRequest.OrtbRequest,
					fmt.Sprintf("outgoingRequests.%s.expectRequest.ortbRequest", b), "")
			}
			for i, sb := range o.MockResponse.SeatBids {
				for j, bid := range sb.Bids {
					c.bid(bid.Bid,
						fmt.Sprintf("outgoingRequests.%s.mockResponse.pbsSeatBids[%d].pbsBids[%d].ortbBid", b, i, j))
				}
			}
		}
		c.response(spec.Response.Bids, "response.bids")
		c.add(spec.Response.Ext, "openrtb_ext.ExtBidResponse", "response.ext")
	}
}

// endpoints harvests endpoints/openrtb2/sample-requests/**/*.json (test_utils.go's testCase).
func (c *corpus) endpoints() {
	root := filepath.Join("endpoints", "openrtb2", "sample-requests")
	for _, path := range c.jsonFiles(root) {
		var spec struct {
			BidRequest                 json.RawMessage            `json:"mockBidRequest"`
			RequestPayload             json.RawMessage            `json:"requestPayload"`
			ExpectedValidatedBidReq    json.RawMessage            `json:"expectedValidatedBidRequest"`
			ExpectedMockBidderRequests map[string]json.RawMessage `json:"expectedMockBidderRequests"`
			ExpectedBidResponse        json.RawMessage            `json:"expectedBidResponse"`
			ExpectedReqExt             json.RawMessage            `json:"expectedReqExt"`
			ExpectedImpExt             json.RawMessage            `json:"expectedImpExt"`
		}
		if !c.open(path, &spec) {
			continue
		}
		c.request(spec.BidRequest, "mockBidRequest", "")
		if strings.Contains(filepath.ToSlash(path), "/video/") {
			c.add(spec.BidRequest, "openrtb_ext.BidRequestVideo", "mockBidRequest")
			c.add(spec.RequestPayload, "openrtb_ext.BidRequestVideo", "requestPayload")
		}
		c.request(spec.ExpectedValidatedBidReq, "expectedValidatedBidRequest", "")
		bidders := make([]string, 0, len(spec.ExpectedMockBidderRequests))
		for b := range spec.ExpectedMockBidderRequests {
			bidders = append(bidders, b)
		}
		sort.Strings(bidders)
		for _, b := range bidders {
			c.request(spec.ExpectedMockBidderRequests[b], "expectedMockBidderRequests."+b, "")
		}
		c.response(spec.ExpectedBidResponse, "expectedBidResponse")
		c.ext(spec.ExpectedReqExt, "openrtb_ext.ExtRequest", "expectedReqExt",
			extKey{"prebid", "openrtb_ext.ExtRequestPrebid"})
		c.imp(spec.ExpectedImpExt, "expectedImpExt", "")
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
			c.request(spec.InputRequestData, "inputRequestData", "")
			if dir == "extractfpdforbidders" {
				c.request(spec.OutputRequestData, "outputRequestData", "")
				continue
			}
			perBidder := members(spec.OutputRequestData)
			for _, b := range sortedKeys(perBidder) {
				c.request(perBidder[b], "outputRequestData."+b, "")
			}
		}
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
	errText := decodeText(data, v.Interface(), decode)
	value := v.Elem().Interface()
	return errText, encodeAll(value)
}

// decodeText is the text of decode's error, empty on success, or "panic: " and the panic value
// where decode panics (jsonutil.StringInt's method does on a lone quote, which only a document
// cut short inside a string hands it), so that the row shows it.
func decodeText(data []byte, v interface{}, decode func([]byte, interface{}) error) (text string) {
	defer func() {
		if r := recover(); r != nil {
			text = fmt.Sprintf("panic: %v", r)
		}
	}()
	if err := decode(data, v); err != nil {
		return err.Error()
	}
	return ""
}

// encodeAll encodes value with jsonutil.Marshal, json.Marshal and an Encoder without HTML
// escaping.
func encodeAll(value interface{}) [3]result {
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
	return out
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
