package diff

import (
	"encoding/json"
	"errors"
	"fmt"

	"github.com/prebid/prebid-server/v4/parity"
)

// Comparisons of the non-trace parity documents: the adapter endpoint outputs
// (stages adapter.requests and adapter.bids, keyed by bidder) and the
// introspection dumps (config.resolved, bidders.resolved). The producer
// identity members (schema, schema_version, impl, case_id) are not compared;
// everything else is, including null versus [] (the contract requires the
// lists to be present).
// Adapter outputs are compared without ordering or UUID rules: MakeRequests
// and MakeBids are deterministic (the 8 adapters with internal randomness run
// on the deterministic fakes), so only the header and member-order rules apply.

// CompareAdapterRequests compares two outputs of POST /__parity/adapter/{bidder}/requests.
func (e *Engine) CompareAdapterRequests(c Case, g, r *parity.AdapterRequestsOutput) (Result, error) {
	if g == nil || r == nil {
		return Result{}, errors.New("parity diff: both adapter requests outputs are required")
	}
	for side, doc := range []*parity.AdapterRequestsOutput{g, r} {
		if err := checkDocument(doc.Schema, doc.SchemaVersion, parity.SchemaAdapterRequestsOutput); err != nil {
			return Result{}, fmt.Errorf("parity diff: %s adapter requests output: %w", sideName(side), err)
		}
	}
	type comparable struct {
		Bidder   string                      `json:"bidder"`
		Requests []parity.AdapterRequestData `json:"requests"`
		Errors   []parity.ErrorRecord        `json:"errors"`
	}
	project := func(doc *parity.AdapterRequestsOutput) any {
		return comparable{Bidder: doc.Bidder, Requests: doc.Requests, Errors: doc.Errors}
	}
	return e.compareDocuments(c, parity.StageAdapterRequests, g.Bidder, project(g), project(r))
}

// CompareAdapterBids compares two outputs of POST /__parity/adapter/{bidder}/bids.
func (e *Engine) CompareAdapterBids(c Case, g, r *parity.AdapterBidsOutput) (Result, error) {
	if g == nil || r == nil {
		return Result{}, errors.New("parity diff: both adapter bids outputs are required")
	}
	for side, doc := range []*parity.AdapterBidsOutput{g, r} {
		if err := checkDocument(doc.Schema, doc.SchemaVersion, parity.SchemaAdapterBidsOutput); err != nil {
			return Result{}, fmt.Errorf("parity diff: %s adapter bids output: %w", sideName(side), err)
		}
	}
	type comparable struct {
		Bidder         string                        `json:"bidder"`
		BidderResponse *parity.AdapterBidderResponse `json:"bidder_response"`
		Errors         []parity.ErrorRecord          `json:"errors"`
	}
	project := func(doc *parity.AdapterBidsOutput) any {
		return comparable{Bidder: doc.Bidder, BidderResponse: doc.BidderResponse, Errors: doc.Errors}
	}
	return e.compareDocuments(c, parity.StageAdapterBids, g.Bidder, project(g), project(r))
}

// CompareConfig compares two GET /__parity/config dumps key path by key path.
func (e *Engine) CompareConfig(c Case, g, r *parity.ConfigDump) (Result, error) {
	if g == nil || r == nil {
		return Result{}, errors.New("parity diff: both config dumps are required")
	}
	for side, doc := range []*parity.ConfigDump{g, r} {
		if err := checkDocument(doc.Schema, doc.SchemaVersion, parity.SchemaConfigDump); err != nil {
			return Result{}, fmt.Errorf("parity diff: %s config dump: %w", sideName(side), err)
		}
	}
	return e.compareDocuments(c, parity.StageConfigResolved, parity.UnkeyedStage, g.Config, r.Config)
}

// CompareBidders compares two GET /__parity/bidders dumps.
func (e *Engine) CompareBidders(c Case, g, r *parity.BiddersDump) (Result, error) {
	if g == nil || r == nil {
		return Result{}, errors.New("parity diff: both bidders dumps are required")
	}
	for side, doc := range []*parity.BiddersDump{g, r} {
		if err := checkDocument(doc.Schema, doc.SchemaVersion, parity.SchemaBiddersDump); err != nil {
			return Result{}, fmt.Errorf("parity diff: %s bidders dump: %w", sideName(side), err)
		}
	}
	type comparable struct {
		Bidders  json.RawMessage   `json:"bidders"`
		Active   []string          `json:"active"`
		Disabled map[string]string `json:"disabled"`
	}
	project := func(doc *parity.BiddersDump) any {
		return comparable{Bidders: doc.Bidders, Active: doc.Active, Disabled: doc.Disabled}
	}
	return e.compareDocuments(c, parity.StageBiddersResolved, parity.UnkeyedStage, project(g), project(r))
}

// compareDocuments compares two projected documents as the value of one
// stage entry.
func (e *Engine) compareDocuments(c Case, stage parity.StageID, key string, g, r any) (Result, error) {
	gn, err := documentNode(g)
	if err != nil {
		return Result{}, fmt.Errorf("parity diff: go %s: %w", stage, err)
	}
	rn, err := documentNode(r)
	if err != nil {
		return Result{}, fmt.Errorf("parity diff: rust %s: %w", stage, err)
	}
	var notes []string
	for _, n := range []*node{gn, rn} {
		for _, sel := range bodyMembers[stage] {
			for _, body := range selectNodes(n, sel) {
				v := &view{}
				v.decodeBodyMember("document", stage, body)
				notes = append(notes, v.notes...)
			}
		}
		if e.rules.Active(RuleHeaderNames, stage) {
			for _, list := range anchorsOf(n, headerLists[stage]) {
				if semantic := headerListNode(list); semantic != nil {
					*list = *semantic
				}
			}
		}
	}
	cmp := e.newComparer(stage, key)
	cmp.value(Root, gn, rn, fieldPlain)
	return Result{Doc: e.document(c, cmp.findings), Notes: notes}, nil
}

func documentNode(v any) (*node, error) {
	if raw, ok := v.(json.RawMessage); ok {
		if len(raw) == 0 {
			return parseSemantic([]byte("null"))
		}
		return parseSemantic(raw)
	}
	return fromValue(v)
}

func sideName(i int) string {
	if i == 0 {
		return parity.ImplGo
	}
	return parity.ImplRust
}
