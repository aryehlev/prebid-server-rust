package oracle

import (
	"bytes"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"math"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Template is one bidder's response template: the first exemplary 200
// mockResponse of its adapter fixtures (one with at least one OpenRTB bid when
// any exists).
type Template struct {
	Bidder string `json:"bidder"`
	// Source is "<fixture>#httpCalls[<i>]".
	Source  string            `json:"source"`
	Status  int               `json:"status"`
	Headers parity.HeaderList `json:"headers"`
	Body    []byte            `json:"-"`
	// ImpIDs are the imp IDs of the fixture request the template answered;
	// non-OpenRTB templates substitute them textually.
	ImpIDs []string `json:"imp_ids,omitempty"`
	// OpenRTB is true when the body is an OpenRTB response with a bid.
	OpenRTB bool `json:"openrtb"`
}

// TemplateSet maps bidders (and aliases) to templates.
type TemplateSet struct {
	byBidder map[string]*Template
	aliases  map[string]string
}

// TemplatesFromCorpus picks one template per bidder from the exemplary
// fixtures, in fixture path order. aliases maps alias bidder names to their
// parent (bidder-info aliasOf); aliases use the parent's template.
func TemplatesFromCorpus(ac *corpus.AdapterCorpus, aliases map[string]string) *TemplateSet {
	set := &TemplateSet{byBidder: map[string]*Template{}, aliases: map[string]string{}}
	fallback := map[string]*Template{}
	for _, f := range ac.Fixtures {
		bidder := f.Bidder()
		if bidder == "" || f.Dir != corpus.DirExemplary || set.byBidder[bidder] != nil {
			continue
		}
		for i, call := range f.Spec.HTTPCalls {
			t := templateOf(f, bidder, i, call)
			if t == nil {
				continue
			}
			if t.OpenRTB {
				set.byBidder[bidder] = t
				break
			}
			if fallback[bidder] == nil {
				fallback[bidder] = t
			}
		}
	}
	for bidder, t := range fallback {
		if set.byBidder[bidder] == nil {
			set.byBidder[bidder] = t
		}
	}
	for alias, parent := range aliases {
		set.aliases[alias] = parent
	}
	return set
}

// BidderAliases maps every static/bidder-info alias to its aliasOf parent.
func BidderAliases(repo *corpus.Repo) (map[string]string, error) {
	dir, err := repo.Resolve(corpus.RootBidderInfo)
	if err != nil {
		return nil, err
	}
	infos, err := config.LoadBidderInfoFromDisk(dir)
	if err != nil {
		return nil, fmt.Errorf("oracle: loading bidder info: %w", err)
	}
	out := map[string]string{}
	for name, info := range infos {
		if info.AliasOf != "" {
			out[name] = info.AliasOf
		}
	}
	return out, nil
}

// Sources are the bidder mock sources built from the adapter corpus.
type Sources struct {
	Templates *TemplateSet
	Replay    *ReplayIndex
}

// SourcesFromRepo loads the adapter corpus and bidder aliases and builds the
// template set and replay index (assign them to Options).
func SourcesFromRepo(repo *corpus.Repo) (*Sources, error) {
	ac, err := corpus.LoadAdapterCorpus(repo)
	if err != nil {
		return nil, err
	}
	aliases, err := BidderAliases(repo)
	if err != nil {
		return nil, err
	}
	return &Sources{Templates: TemplatesFromCorpus(ac, aliases), Replay: ReplayFromCorpus(ac)}, nil
}

// templateOf returns the template of one httpCall, or nil when it is not a
// non-empty 200 response.
func templateOf(f *corpus.AdapterFixture, bidder string, i int, call corpus.AdapterHTTPCall) *Template {
	body := []byte(call.Response.Body)
	if call.Response.Status != 200 || len(body) == 0 || string(body) == "null" {
		return nil
	}
	ids := call.Request.ImpIDs
	if len(ids) == 0 {
		ids = requestImpIDs(f.Spec.BidRequest)
	}
	headers := parity.HeaderList{}
	for _, name := range sortedHeaderNames(call.Response.Headers) {
		for _, v := range call.Response.Headers[name] {
			headers = append(headers, [2]string{name, v})
		}
	}
	return &Template{
		Bidder:  bidder,
		Source:  fmt.Sprintf("%s#httpCalls[%d]", f.Path, i),
		Status:  200,
		Headers: headers,
		Body:    body,
		ImpIDs:  ids,
		OpenRTB: hasOpenRTBBid(body),
	}
}

func sortedHeaderNames(h map[string][]string) []string {
	out := make([]string, 0, len(h))
	for k := range h {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// hasOpenRTBBid reports whether body is {"seatbid":[{"bid":[{...}]}]}.
func hasOpenRTBBid(body []byte) bool {
	var resp struct {
		SeatBid []struct {
			Bid []json.RawMessage `json:"bid"`
		} `json:"seatbid"`
	}
	if err := json.Unmarshal(body, &resp); err != nil || len(resp.SeatBid) == 0 || len(resp.SeatBid[0].Bid) == 0 {
		return false
	}
	return bytes.HasPrefix(bytes.TrimSpace(resp.SeatBid[0].Bid[0]), []byte("{"))
}

// requestImpIDs returns imp[].id of an OpenRTB request.
func requestImpIDs(body []byte) []string {
	var req struct {
		Imp []struct {
			ID string `json:"id"`
		} `json:"imp"`
	}
	if err := json.Unmarshal(body, &req); err != nil {
		return nil
	}
	var out []string
	for _, imp := range req.Imp {
		if imp.ID != "" {
			out = append(out, imp.ID)
		}
	}
	return out
}

// Template returns the template of a bidder or of its alias parent.
func (s *TemplateSet) Template(bidder string) (*Template, bool) {
	if s == nil {
		return nil, false
	}
	if t, ok := s.byBidder[bidder]; ok {
		return t, true
	}
	if parent, ok := s.aliases[bidder]; ok {
		t, ok := s.byBidder[parent]
		return t, ok
	}
	return nil, false
}

// Len returns the number of bidders with a template.
func (s *TemplateSet) Len() int {
	if s == nil {
		return 0
	}
	return len(s.byBidder)
}

// RenderInput are the substitution inputs of one templated response.
type RenderInput struct {
	CaseID string
	Bidder string
	// RequestID is the outgoing request's top-level id ("" keeps the template's).
	RequestID string
	// ImpIDs are the sorted imp IDs found in the outgoing request.
	ImpIDs []string
	Params BidderParams
}

// TemplatePrice is the deterministic generated price of one imp:
// round2(0.01 + (fnv1a64(case|bidder|imp) % 2000) / 100), so 0.01 to 20.00.
func TemplatePrice(caseID, bidder, impID string) float64 {
	h := fnv.New64a()
	h.Write([]byte(caseID + "|" + bidder + "|" + impID))
	return math.Round((0.01+float64(h.Sum64()%2000)/100)*100) / 100
}

// formatPrice writes a price as the shortest decimal that round-trips.
func formatPrice(p float64) json.Number {
	return json.Number(strconv.FormatFloat(p, 'f', -1, 64))
}

// Render produces the response body for in. OpenRTB templates keep the first
// seatbid with one copy of its first bid per imp (impid, price, dealid set; the
// bid id gets "-<n>" for the n-th imp after the first), the top-level id is the
// request's, and cur and seat follow the params. Other templates get their imp
// IDs replaced textually, position by position.
func (t *Template) Render(in RenderInput) ([]byte, error) {
	if !t.OpenRTB {
		return t.renderText(in.ImpIDs), nil
	}
	doc, err := decodeUseNumber(t.Body)
	if err != nil {
		return nil, fmt.Errorf("oracle: template %s: %w", t.Source, err)
	}
	resp, ok := doc.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("oracle: template %s is not a JSON object", t.Source)
	}
	if in.RequestID != "" {
		resp["id"] = in.RequestID
	}
	if in.Params.Currency != "" {
		resp["cur"] = in.Params.Currency
	}
	seat, base, err := firstBid(resp)
	if err != nil {
		return nil, fmt.Errorf("oracle: template %s: %w", t.Source, err)
	}
	bids := make([]any, 0, len(in.ImpIDs))
	for i, imp := range in.ImpIDs {
		bids = append(bids, renderBid(base, i, imp, in))
	}
	seat["bid"] = bids
	if in.Params.Seat != "" {
		seat["seat"] = in.Params.Seat
	}
	resp["seatbid"] = []any{seat}
	return encodeNoEscape(resp)
}

// firstBid returns seatbid[0] and seatbid[0].bid[0] of a decoded response.
func firstBid(resp map[string]any) (map[string]any, map[string]any, error) {
	seatbids, _ := resp["seatbid"].([]any)
	if len(seatbids) == 0 {
		return nil, nil, fmt.Errorf("no seatbid")
	}
	seat, ok := seatbids[0].(map[string]any)
	if !ok {
		return nil, nil, fmt.Errorf("seatbid[0] is not an object")
	}
	bids, _ := seat["bid"].([]any)
	if len(bids) == 0 {
		return nil, nil, fmt.Errorf("seatbid[0] has no bid")
	}
	base, ok := bids[0].(map[string]any)
	if !ok {
		return nil, nil, fmt.Errorf("seatbid[0].bid[0] is not an object")
	}
	return seat, base, nil
}

// renderBid copies the template bid for one imp.
func renderBid(base map[string]any, i int, imp string, in RenderInput) map[string]any {
	bid := copyObject(base)
	bid["impid"] = imp
	price := TemplatePrice(in.CaseID, in.Bidder, imp)
	if in.Params.Price != nil {
		price = *in.Params.Price
	}
	bid["price"] = formatPrice(price)
	if in.Params.DealID != "" {
		bid["dealid"] = in.Params.DealID
	}
	if id, ok := bid["id"].(string); ok && i > 0 {
		bid["id"] = id + "-" + strconv.Itoa(i+1)
	}
	return bid
}

// renderText replaces the template's quoted imp IDs with the request's.
func (t *Template) renderText(ids []string) []byte {
	var pairs []string
	for i, from := range t.ImpIDs {
		if i >= len(ids) {
			break
		}
		pairs = append(pairs, jsonQuote(from), jsonQuote(ids[i]))
	}
	if len(pairs) == 0 {
		return append([]byte(nil), t.Body...)
	}
	return []byte(strings.NewReplacer(pairs...).Replace(string(t.Body)))
}

// jsonQuote returns s as a canonical JSON string literal.
func jsonQuote(s string) string {
	var buf bytes.Buffer
	if err := writeCanonicalString(&buf, s); err != nil {
		return strconv.Quote(s)
	}
	return buf.String()
}

func decodeUseNumber(raw []byte) (any, error) {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, err
	}
	return v, nil
}

// copyObject deep-copies a decoded JSON object.
func copyObject(obj map[string]any) map[string]any {
	out := make(map[string]any, len(obj))
	for k, v := range obj {
		out[k] = deepCopy(v)
	}
	return out
}

// deepCopy copies decoded JSON values.
func deepCopy(v any) any {
	switch x := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(x))
		for k, item := range x {
			out[k] = deepCopy(item)
		}
		return out
	case []any:
		out := make([]any, len(x))
		for i, item := range x {
			out[i] = deepCopy(item)
		}
		return out
	default:
		return v
	}
}

// encodeNoEscape marshals without HTML escaping and without a trailing newline.
func encodeNoEscape(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, fmt.Errorf("oracle: encoding templated response: %w", err)
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

// ReplayIndex answers a bidder call with an adapter fixture's mockResponse
// when (bidder, uri, semantic body) equals that fixture's expectedRequest.
type ReplayIndex struct {
	entries map[string][]replayEntry
}

type replayEntry struct {
	id       string
	doc      any
	isJSON   bool
	raw      []byte
	response corpus.AdapterHTTPResponse
}

// ReplayFromCorpus indexes every httpCall of the fixtures with a bidder.
func ReplayFromCorpus(ac *corpus.AdapterCorpus) *ReplayIndex {
	idx := &ReplayIndex{entries: map[string][]replayEntry{}}
	for _, f := range ac.Fixtures {
		bidder := f.Bidder()
		if bidder == "" {
			continue
		}
		for i, call := range f.Spec.HTTPCalls {
			e := replayEntry{id: fmt.Sprintf("%s#httpCalls[%d]", f.Path, i), raw: call.Request.Body, response: call.Response}
			if doc, err := decodeUseNumber(call.Request.Body); err == nil {
				e.doc, e.isJSON = doc, true
			}
			key := bidder + "\x00" + call.Request.URI
			idx.entries[key] = append(idx.entries[key], e)
		}
	}
	return idx
}

// lookup returns the first fixture whose expected request matches.
func (r *ReplayIndex) lookup(bidder, uri string, body []byte) (*replayEntry, bool) {
	if r == nil {
		return nil, false
	}
	entries := r.entries[bidder+"\x00"+uri]
	if len(entries) == 0 {
		return nil, false
	}
	doc, err := decodeUseNumber(body)
	isJSON := err == nil
	for i := range entries {
		e := &entries[i]
		if isJSON && e.isJSON && jsonValueEqual(e.doc, doc) {
			return e, true
		}
		if !isJSON && !e.isJSON && bytes.Equal(e.raw, body) {
			return e, true
		}
	}
	return nil, false
}

// Len returns the number of indexed fixture calls.
func (r *ReplayIndex) Len() int {
	if r == nil {
		return 0
	}
	n := 0
	for _, e := range r.entries {
		n += len(e)
	}
	return n
}
