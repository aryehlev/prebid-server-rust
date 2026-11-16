package oracle

import (
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Plan is the per-case mock plan the runner registers before it sends a case
// to the servers. Both implementations get the same plan: the oracle separates
// their calls by X-Parity-Meta impl, never by mock.
type Plan struct {
	CaseID string `json:"case_id"`
	// ImpIDs are the inbound request's imp IDs; the template generator looks
	// for them in non-OpenRTB outgoing requests.
	ImpIDs []string `json:"imp_ids,omitempty"`
	// Rules are explicit responses; the first matching rule wins.
	Rules []Rule `json:"rules,omitempty"`
	// Bidders are template parameters per bidder (sample-request mockBidders).
	Bidders map[string]BidderParams `json:"bidders,omitempty"`
	// Floors answers kind=floors calls that no rule matched; nil means 404.
	Floors *parity.InlineResponse `json:"floors,omitempty"`
	// DisableReplay turns adapter-fixture replay off for this case.
	DisableReplay bool `json:"disable_replay,omitempty"`
	// DisableTemplates turns the template generator off for this case.
	DisableTemplates bool `json:"disable_templates,omitempty"`
}

// BidderParams fill the template substitutions for one bidder.
type BidderParams struct {
	// Price overrides the generated price when non-nil.
	Price    *float64 `json:"price,omitempty"`
	Currency string   `json:"currency,omitempty"`
	DealID   string   `json:"dealid,omitempty"`
	Seat     string   `json:"seat,omitempty"`
}

// Rule is one explicit response. Every set matcher must hold.
type Rule struct {
	ID string `json:"id"`
	// Kind is the outbound kind (bidder, cache, floors, ...); "" matches any.
	Kind string `json:"kind,omitempty"`
	// Bidder is matched against X-Parity-Meta bidder; "" matches any.
	Bidder string `json:"bidder,omitempty"`
	// URIRegex is matched against the original (pre-rewrite) URL.
	URIRegex string `json:"uri_regex,omitempty"`
	// JSONEquals are JSONPath-like paths into the decoded request body whose
	// values must equal the given JSON semantically.
	JSONEquals []PathEquals `json:"json_equals,omitempty"`
	// ImpIDs, when non-nil, must equal the set of imp IDs found in the request.
	ImpIDs   []string              `json:"imp_ids,omitempty"`
	Response parity.InlineResponse `json:"response"`

	uri *regexp.Regexp
}

// PathEquals pairs a path such as $.imp[0].ext.bidder.placementId with a value.
type PathEquals struct {
	Path  string          `json:"path"`
	Value json.RawMessage `json:"value"`
}

// validFailures are the InlineResponse.Fail values the oracle can produce.
var validFailures = map[string]bool{
	parity.OutboundErrTimeout: true,
	parity.OutboundErrRefused: true,
	parity.OutboundErrReset:   true,
	parity.OutboundErrOther:   true,
}

// compile validates the plan and compiles its regular expressions.
func (p *Plan) compile() error {
	if p.CaseID == "" {
		return errors.New("oracle: plan has no case_id")
	}
	for i := range p.Rules {
		r := &p.Rules[i]
		if r.URIRegex != "" {
			re, err := regexp.Compile(r.URIRegex)
			if err != nil {
				return fmt.Errorf("oracle: plan %s rule %q: uri_regex: %w", p.CaseID, r.ID, err)
			}
			r.uri = re
		}
		for _, pe := range r.JSONEquals {
			if _, err := parsePath(pe.Path); err != nil {
				return fmt.Errorf("oracle: plan %s rule %q: %w", p.CaseID, r.ID, err)
			}
			if !json.Valid(pe.Value) {
				return fmt.Errorf("oracle: plan %s rule %q: json_equals value for %s is not JSON", p.CaseID, r.ID, pe.Path)
			}
		}
		if err := validateResponse(r.Response); err != nil {
			return fmt.Errorf("oracle: plan %s rule %q: %w", p.CaseID, r.ID, err)
		}
	}
	if p.Floors != nil {
		if err := validateResponse(*p.Floors); err != nil {
			return fmt.Errorf("oracle: plan %s floors: %w", p.CaseID, err)
		}
	}
	return nil
}

func validateResponse(r parity.InlineResponse) error {
	if r.Fail != nil && !validFailures[*r.Fail] {
		return fmt.Errorf("unsupported fail %q", *r.Fail)
	}
	if r.Fail == nil && (r.Status < 100 || r.Status > 999) {
		return fmt.Errorf("status %d is not a valid HTTP status", r.Status)
	}
	if r.DelayMS < 0 {
		return fmt.Errorf("negative delay_ms %d", r.DelayMS)
	}
	if _, err := r.Body.WireBytes(); err != nil {
		return fmt.Errorf("response body: %w", err)
	}
	return nil
}

// match returns the first rule matching the call.
func (p *Plan) match(in *inbound) (*Rule, bool) {
	for i := range p.Rules {
		if p.Rules[i].matches(in) {
			return &p.Rules[i], true
		}
	}
	return nil, false
}

func (r *Rule) matches(in *inbound) bool {
	if r.Kind != "" && r.Kind != in.kind {
		return false
	}
	if r.Bidder != "" && r.Bidder != in.meta.Bidder {
		return false
	}
	if r.uri != nil && !r.uri.MatchString(in.origURL()) {
		return false
	}
	if r.ImpIDs != nil && !equalSets(r.ImpIDs, in.impIDs) {
		return false
	}
	for _, pe := range r.JSONEquals {
		if !in.jsonPathEquals(pe) {
			return false
		}
	}
	return true
}

func equalSets(a, b []string) bool {
	as, bs := sortedUnique(a), sortedUnique(b)
	if len(as) != len(bs) {
		return false
	}
	for i := range as {
		if as[i] != bs[i] {
			return false
		}
	}
	return true
}

func sortedUnique(in []string) []string {
	out := append([]string(nil), in...)
	sort.Strings(out)
	n := 0
	for i, s := range out {
		if i == 0 || s != out[n-1] {
			out[n] = s
			n++
		}
	}
	return out[:n]
}

// pathStep is one member name or array index of a parsed path.
type pathStep struct {
	key   string
	index int
	isIdx bool
}

// parsePath parses "$.a.b[0].c" (the "$." prefix is optional).
func parsePath(p string) ([]pathStep, error) {
	rest := strings.TrimPrefix(strings.TrimPrefix(p, "$"), ".")
	if rest == "" {
		return nil, fmt.Errorf("empty json path %q", p)
	}
	var steps []pathStep
	for _, part := range strings.Split(rest, ".") {
		name, idx, _ := strings.Cut(part, "[")
		if name == "" && idx == "" {
			return nil, fmt.Errorf("json path %q has an empty segment", p)
		}
		if name != "" {
			steps = append(steps, pathStep{key: name})
		}
		for idx != "" {
			num, after, ok := strings.Cut(idx, "]")
			n, err := strconv.Atoi(num)
			if !ok || err != nil || n < 0 {
				return nil, fmt.Errorf("json path %q has a bad index", p)
			}
			steps = append(steps, pathStep{index: n, isIdx: true})
			idx = strings.TrimPrefix(after, "[")
			if after != "" && !strings.HasPrefix(after, "[") {
				return nil, fmt.Errorf("json path %q has a bad index", p)
			}
		}
	}
	return steps, nil
}

// lookupPath returns the value at steps in a decoded JSON document.
func lookupPath(doc any, steps []pathStep) (any, bool) {
	cur := doc
	for _, s := range steps {
		if s.isIdx {
			arr, ok := cur.([]any)
			if !ok || s.index >= len(arr) {
				return nil, false
			}
			cur = arr[s.index]
			continue
		}
		obj, ok := cur.(map[string]any)
		if !ok {
			return nil, false
		}
		if cur, ok = obj[s.key]; !ok {
			return nil, false
		}
	}
	return cur, true
}

// semanticEqual compares two JSON texts as values: member order and
// whitespace are ignored and numbers compare by exact decimal value (12 equals
// 12.0 and 1.2e1), so matching never depends on lexical choices that the diff
// reports separately.
func semanticEqual(a, b []byte) bool {
	da, errA := decodeUseNumber(a)
	db, errB := decodeUseNumber(b)
	return errA == nil && errB == nil && jsonValueEqual(da, db)
}

// jsonValueEqual compares decoded (UseNumber) JSON values.
func jsonValueEqual(a, b any) bool {
	switch x := a.(type) {
	case map[string]any:
		y, ok := b.(map[string]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for k, v := range x {
			w, ok := y[k]
			if !ok || !jsonValueEqual(v, w) {
				return false
			}
		}
		return true
	case []any:
		y, ok := b.([]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for i := range x {
			if !jsonValueEqual(x[i], y[i]) {
				return false
			}
		}
		return true
	case json.Number:
		y, ok := b.(json.Number)
		return ok && numbersEqual(x, y)
	default:
		return a == b
	}
}

// numbersEqual compares JSON numbers by exact rational value.
func numbersEqual(a, b json.Number) bool {
	if a == b {
		return true
	}
	ra, okA := new(big.Rat).SetString(a.String())
	rb, okB := new(big.Rat).SetString(b.String())
	return okA && okB && ra.Cmp(rb) == 0
}

// PlanFromCase builds the oracle plan of a corpus case: its explicit HTTP
// mocks become rules (bidder plus imp-ID set) and its mockBidders become
// template parameters.
func PlanFromCase(cs *corpus.Case) *Plan {
	p := &Plan{CaseID: cs.ID, ImpIDs: append([]string(nil), cs.Mocks.ImpIDs...)}
	for _, m := range cs.Mocks.HTTP {
		rule := Rule{ID: m.ID, Kind: parity.OutboundBidder, Bidder: m.Bidder, URIRegex: m.URIRegex, Response: m.Response}
		if m.ImpIDs != nil {
			rule.ImpIDs = append([]string{}, m.ImpIDs...)
		}
		p.Rules = append(p.Rules, rule)
	}
	if len(cs.Mocks.Bidders) > 0 {
		p.Bidders = make(map[string]BidderParams, len(cs.Mocks.Bidders))
		for name, b := range cs.Mocks.Bidders {
			p.Bidders[name] = BidderParams{Price: b.Price, Currency: b.Currency, DealID: b.DealID, Seat: b.Seat}
		}
	}
	return p
}

// RuleFromInline converts an envelope inline rule so the same canned
// responses can be served in oracle mode.
func RuleFromInline(r parity.InlineRule) Rule {
	return Rule{ID: r.ID, Kind: r.Kind, Bidder: r.Bidder, URIRegex: r.URIRegex, Response: r.Response}
}
