package corpus

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// maxValidParamsPerBidder caps the valid params examples kept per bidder; the
// lifted auction cases already run every fixture's params.
const maxValidParamsPerBidder = 2

// BidderParamsSchema is one bidder params JSON schema file.
type BidderParamsSchema struct {
	Bidder string `json:"bidder"`
	Path   string `json:"path"`
	// TestOnly marks schemas from router/bidder_params_tests, which only the
	// router test serves (the server always reads static/bidder-params).
	TestOnly bool            `json:"test_only,omitempty"`
	Schema   json.RawMessage `json:"-"`
}

// ParamsExample is one imp[].ext.bidder value harvested from an adapter
// fixture and classified with the production params validator.
type ParamsExample struct {
	Bidder string          `json:"bidder"`
	Params json.RawMessage `json:"params"`
	// Source is "<fixture>#imp[<i>]" of the first fixture with these params.
	Source string `json:"source"`
	Valid  bool   `json:"valid"`
	// Error is the validator's error text for invalid params.
	Error string `json:"error,omitempty"`
}

// BidderParamsCorpus holds the schemas and the classified params examples.
type BidderParamsCorpus struct {
	Schemas  []BidderParamsSchema
	Examples []ParamsExample
}

// LoadBidderParamsSchemas reads static/bidder-params and
// router/bidder_params_tests.
func LoadBidderParamsSchemas(repo *Repo) ([]BidderParamsSchema, error) {
	var out []BidderParamsSchema
	for _, root := range []string{RootBidderParams, RootRouterParamsTests} {
		entries, err := repo.ReadDir(root)
		if err != nil {
			return nil, err
		}
		for _, e := range entries {
			if e.IsDir() || path.Ext(e.Name()) != ".json" {
				continue
			}
			rel := path.Join(root, e.Name())
			data, err := repo.ReadFile(rel)
			if err != nil {
				return nil, err
			}
			if !json.Valid(data) {
				return nil, fmt.Errorf("corpus: %s is not valid JSON", rel)
			}
			out = append(out, BidderParamsSchema{
				Bidder:   strings.TrimSuffix(e.Name(), ".json"),
				Path:     rel,
				TestOnly: root == RootRouterParamsTests,
				Schema:   data,
			})
		}
	}
	return out, nil
}

// LoadBidderParams loads the schemas and classifies every distinct params
// object of the adapter fixtures with openrtb_ext.NewBidderParamsValidator.
func LoadBidderParams(repo *Repo, adapters *AdapterCorpus) (*BidderParamsCorpus, error) {
	schemas, err := LoadBidderParamsSchemas(repo)
	if err != nil {
		return nil, err
	}
	dir, err := repo.Resolve(RootBidderParams)
	if err != nil {
		return nil, err
	}
	validator, err := openrtb_ext.NewBidderParamsValidator(dir)
	if err != nil {
		return nil, fmt.Errorf("corpus: building params validator: %w", err)
	}
	examples := harvestParams(adapters)
	for i := range examples {
		ex := &examples[i]
		if verr := validator.Validate(openrtb_ext.BidderName(ex.Bidder), ex.Params); verr != nil {
			ex.Error = verr.Error()
		} else {
			ex.Valid = true
		}
	}
	return &BidderParamsCorpus{Schemas: schemas, Examples: examples}, nil
}

// harvestParams collects distinct (bidder, params) pairs from the fixtures in
// fixture order. Fixtures without a resolved bidder are skipped.
func harvestParams(adapters *AdapterCorpus) []ParamsExample {
	seen := map[string]bool{}
	var out []ParamsExample
	for _, f := range adapters.Fixtures {
		bidder := f.Bidder()
		if bidder == "" {
			continue
		}
		for _, p := range fixtureParams(f.Spec.BidRequest) {
			key := bidder + "\x00" + string(compactJSON(p.params))
			if seen[key] {
				continue
			}
			seen[key] = true
			out = append(out, ParamsExample{Bidder: bidder, Params: p.params, Source: fmt.Sprintf("%s#imp[%d]", f.Path, p.imp)})
		}
	}
	return out
}

type impParams struct {
	imp    int
	params json.RawMessage
}

// fixtureParams returns imp[].ext.bidder of a request in imp order (absent
// and null values skipped).
func fixtureParams(body json.RawMessage) []impParams {
	var req struct {
		Imp []struct {
			Ext struct {
				Bidder json.RawMessage `json:"bidder"`
			} `json:"ext"`
		} `json:"imp"`
	}
	if err := json.Unmarshal(body, &req); err != nil {
		return nil
	}
	var out []impParams
	for i, imp := range req.Imp {
		if len(imp.Ext.Bidder) > 0 && string(imp.Ext.Bidder) != "null" {
			out = append(out, impParams{imp: i, params: imp.Ext.Bidder})
		}
	}
	return out
}

func compactJSON(raw []byte) []byte {
	var buf bytes.Buffer
	if err := json.Compact(&buf, raw); err != nil {
		return raw
	}
	return buf.Bytes()
}

// Cases returns the GET /bidders/params case plus one auction case per
// invalid example and up to maxValidParamsPerBidder valid ones per bidder.
// Each auction has one 300x250 banner imp carrying the params, so the
// comparison isolates params validation.
func (c *BidderParamsCorpus) Cases() []Case {
	id := caseID(RootBidderParams, "get:bidders-params")
	cases := []Case{{
		ID:          id,
		Kind:        KindBidderParams,
		Corpus:      CorpusBidderParams,
		FixturePath: RootBidderParams,
		Endpoint:    EndpointBidderParams,
		Envelope:    newEnvelope(id, RootBidderParams, DefaultProfile, getRequest(EndpointBidderParams, "")),
	}}
	valid := map[string]int{}
	for _, ex := range c.sortedExamples() {
		if ex.Valid {
			if valid[ex.Bidder] >= maxValidParamsPerBidder {
				continue
			}
			valid[ex.Bidder]++
		}
		cases = append(cases, paramsCase(ex))
	}
	return cases
}

// sortedExamples orders examples by bidder, then source.
func (c *BidderParamsCorpus) sortedExamples() []ParamsExample {
	out := append([]ParamsExample(nil), c.Examples...)
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Bidder != out[j].Bidder {
			return out[i].Bidder < out[j].Bidder
		}
		return out[i].Source < out[j].Source
	})
	return out
}

func paramsCase(ex ParamsExample) Case {
	variant := "params:valid"
	if !ex.Valid {
		variant = "params:invalid"
	}
	id := caseID(ex.Source, variant)
	body := paramsAuctionBody(ex, "parity-params-"+shortHash(10, id))
	cs := Case{
		ID:          id,
		Kind:        KindBidderParams,
		Corpus:      CorpusBidderParams,
		FixturePath: strings.SplitN(ex.Source, "#", 2)[0],
		Endpoint:    EndpointAuction,
		Envelope:    newEnvelope(id, ex.Source, DefaultProfile, jsonPostRequest(EndpointAuction, body)),
		Mocks:       MockPlan{ImpIDs: []string{"imp-1"}},
	}
	if !ex.Valid {
		cs.Expect.Status = 400
		cs.Notes = append(cs.Notes, "Go params validator: "+ex.Error)
	}
	return cs
}

// paramsAuctionBody builds the minimal auction for one params object; the
// params bytes are spliced in verbatim.
func paramsAuctionBody(ex ParamsExample, requestID string) []byte {
	var b bytes.Buffer
	b.WriteString(`{"id":`)
	writeJSONString(&b, requestID)
	b.WriteString(`,"imp":[{"id":"imp-1","banner":{"format":[{"w":300,"h":250}]},"ext":{"prebid":{"bidder":{`)
	writeJSONString(&b, ex.Bidder)
	b.WriteString(`:`)
	b.Write(compactJSON(ex.Params))
	b.WriteString(`}}}}],"site":{"page":"https://parity.prebid.test/params"}}`)
	return b.Bytes()
}

func writeJSONString(b *bytes.Buffer, s string) {
	raw, err := json.Marshal(s)
	if err != nil {
		// json.Marshal of a string cannot fail; keep the output valid anyway.
		raw = []byte(`""`)
	}
	b.Write(raw)
}
