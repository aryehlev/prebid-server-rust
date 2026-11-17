package minimize

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Subject is what the minimizer reduces: the envelope both servers receive
// and the oracle mock plan of the case.
type Subject struct {
	Envelope parity.Envelope `json:"envelope"`
	Mocks    corpus.MockPlan `json:"mocks"`
}

// Clone returns a deep copy of s.
func (s Subject) Clone() (Subject, error) {
	raw, err := json.Marshal(s)
	if err != nil {
		return Subject{}, fmt.Errorf("minimize: copying subject: %w", err)
	}
	var out Subject
	if err := json.Unmarshal(raw, &out); err != nil {
		return Subject{}, fmt.Errorf("minimize: copying subject: %w", err)
	}
	return out, nil
}

// EditKind names one kind of reduction.
type EditKind string

// Edit kinds. Every edit only removes or shrinks something.
const (
	EditDropBidder         EditKind = "drop_bidder"
	EditDropHeader         EditKind = "drop_header"
	EditDropQueryParam     EditKind = "drop_query_param"
	EditDropMockRule       EditKind = "drop_mock_rule"
	EditDropMockBidder     EditKind = "drop_mock_bidder"
	EditDropBidderResponse EditKind = "drop_bidder_response"
	EditDropInlineRule     EditKind = "drop_inline_rule"
	EditDropDecision       EditKind = "drop_decision"
	EditDelete             EditKind = "delete"
	EditMinimalValue       EditKind = "minimal_value"
	EditShortenString      EditKind = "shorten_string"
	EditTruncateBody       EditKind = "truncate_body"
)

// Edit is one candidate reduction of a subject. Edits of one phase are all
// computed on the same base subject and compose: applying any subset is the
// union of their operations, so array indices never shift under each other.
type Edit struct {
	Kind EditKind `json:"kind"`
	// Target names what the edit removes: a body JSON path, a header index
	// and name, a bidder, a mock ID.
	Target string `json:"target"`
	ops    []op
}

// String is the edit's description, used in the reduction report.
func (e Edit) String() string { return string(e.Kind) + " " + e.Target }

type opKind uint8

const (
	opDeleteNode opKind = iota
	opMinimalNode
	opShortenNode
	opDropHeader
	opDropQuery
	opDropMockRule
	opDropMockBidder
	opDropBidderResponse
	opDropInlineRule
	opDropDecision
	opTruncateBody
)

type op struct {
	kind  opKind
	index int
	key   string
}

// Phases of one reduction round, coarse to fine.
const (
	phaseGroups = iota
	phaseDelete
	phaseValues
)

// base is a subject prepared for edit generation and application.
type base struct {
	subj     Subject
	body     *node
	rawBody  []byte
	encoding string
	query    []string
}

func newBase(s Subject) (*base, error) {
	b := &base{subj: s}
	if s.Envelope.Request.Body != nil {
		raw, err := s.Envelope.Request.Body.DecodedBytes()
		if err != nil {
			return nil, fmt.Errorf("minimize: decoding the request body: %w", err)
		}
		b.rawBody, b.encoding = raw, s.Envelope.Request.Body.ContentEncoding
		if tree, err := parseTree(raw); err == nil {
			b.body = tree
		}
	}
	if s.Envelope.Request.Query != "" {
		b.query = strings.Split(s.Envelope.Request.Query, "&")
	}
	return b, nil
}

// edits returns the candidate edits of one phase at one depth (depth is used
// by phaseDelete only), in a deterministic order.
func (b *base) edits(phase, depth int) []Edit {
	switch phase {
	case phaseGroups:
		return b.groupEdits()
	case phaseDelete:
		return b.deleteEdits(depth)
	default:
		return b.valueEdits()
	}
}

// maxDepth is the deepest node depth of the body (0 without a JSON body).
func (b *base) maxDepth() int {
	max := 0
	if b.body != nil {
		b.body.walk("$", func(n *node, _ string) {
			if n.depth > max {
				max = n.depth
			}
		})
	}
	return max
}

// groupEdits are the semantic reductions: drop a bidder everywhere, a header,
// a query parameter, a mock, a decision, or truncate a non-JSON body.
func (b *base) groupEdits() []Edit {
	var out []Edit
	out = append(out, b.bidderEdits()...)
	for i, h := range b.subj.Envelope.Request.Headers {
		out = append(out, Edit{Kind: EditDropHeader, Target: fmt.Sprintf("[%d] %s", i, h[0]), ops: []op{{kind: opDropHeader, index: i}}})
	}
	for i, q := range b.query {
		out = append(out, Edit{Kind: EditDropQueryParam, Target: fmt.Sprintf("[%d] %s", i, q), ops: []op{{kind: opDropQuery, index: i}}})
	}
	for i, m := range b.subj.Mocks.HTTP {
		out = append(out, Edit{Kind: EditDropMockRule, Target: m.ID, ops: []op{{kind: opDropMockRule, index: i}}})
	}
	for _, name := range sortedKeys(b.subj.Mocks.Bidders) {
		out = append(out, Edit{Kind: EditDropMockBidder, Target: name, ops: []op{{kind: opDropMockBidder, key: name}}})
	}
	for _, name := range sortedKeys(b.subj.Envelope.Mock.BidderResponses) {
		out = append(out, Edit{Kind: EditDropBidderResponse, Target: name, ops: []op{{kind: opDropBidderResponse, key: name}}})
	}
	if inline := b.subj.Envelope.Mock.Inline; inline != nil {
		for i, r := range inline.Rules {
			out = append(out, Edit{Kind: EditDropInlineRule, Target: fmt.Sprintf("[%d] %s", i, r.ID), ops: []op{{kind: opDropInlineRule, index: i}}})
		}
	}
	for _, point := range sortedKeys(b.subj.Envelope.Decisions) {
		out = append(out, Edit{Kind: EditDropDecision, Target: point, ops: []op{{kind: opDropDecision, key: point}}})
	}
	if b.body == nil && len(b.rawBody) > 1 {
		out = append(out, Edit{Kind: EditTruncateBody, Target: fmt.Sprintf("%d bytes", len(b.rawBody)), ops: []op{{kind: opTruncateBody}}})
	}
	return out
}

// nonBidderImpExt are imp.ext members that are not bidder names.
var nonBidderImpExt = map[string]bool{
	"prebid": true, "data": true, "context": true, "skadn": true, "gpid": true, "tid": true,
	"ae": true, "igs": true, "all": true, "general": true, "bidder": true,
}

// bidderEdits drops each bidder from every imp (imp.ext.prebid.bidder.<b>
// and the legacy imp.ext.<b>) together with its mocks.
func (b *base) bidderEdits() []Edit {
	nodes := b.bidderNodes()
	names := map[string]bool{}
	for name := range nodes {
		names[name] = true
	}
	for name := range b.subj.Mocks.Bidders {
		names[name] = true
	}
	for name := range b.subj.Envelope.Mock.BidderResponses {
		names[name] = true
	}
	var out []Edit
	for _, name := range sortedKeys(names) {
		e := Edit{Kind: EditDropBidder, Target: name}
		for _, id := range nodes[name] {
			e.ops = append(e.ops, op{kind: opDeleteNode, index: id})
		}
		for i, m := range b.subj.Mocks.HTTP {
			if m.Bidder == name {
				e.ops = append(e.ops, op{kind: opDropMockRule, index: i})
			}
		}
		e.ops = append(e.ops, op{kind: opDropMockBidder, key: name}, op{kind: opDropBidderResponse, key: name})
		out = append(out, e)
	}
	return out
}

// bidderNodes maps bidder names to the node IDs naming them in the imps.
func (b *base) bidderNodes() map[string][]int {
	out := map[string][]int{}
	imps, ok := b.body.member("imp")
	if !ok || imps.kind != kindArray {
		return out
	}
	for _, imp := range imps.elems {
		ext, ok := imp.member("ext")
		if !ok || ext.kind != kindObject {
			continue
		}
		for _, m := range ext.members {
			if !nonBidderImpExt[m.key] {
				out[m.key] = append(out[m.key], m.value.id)
			}
		}
		prebid, _ := ext.member("prebid")
		bidders, ok := prebid.member("bidder")
		if !ok || bidders.kind != kindObject {
			continue
		}
		for _, m := range bidders.members {
			out[m.key] = append(out[m.key], m.value.id)
		}
	}
	return out
}

// deleteEdits deletes each object member or array element at depth.
func (b *base) deleteEdits(depth int) []Edit {
	var out []Edit
	if b.body == nil || depth < 1 {
		return out
	}
	b.body.walk("$", func(n *node, path string) {
		if n.depth == depth {
			out = append(out, Edit{Kind: EditDelete, Target: path, ops: []op{{kind: opDeleteNode, index: n.id}}})
		}
	})
	return out
}

// valueEdits replace subtrees with the minimal value of their type and
// shorten strings.
func (b *base) valueEdits() []Edit {
	var out []Edit
	if b.body == nil {
		return out
	}
	b.body.walk("$", func(n *node, path string) {
		if n.depth == 0 || n.isMinimal() {
			return
		}
		out = append(out, Edit{Kind: EditMinimalValue, Target: path, ops: []op{{kind: opMinimalNode, index: n.id}}})
		if n.kind == kindString && len([]rune(n.str)) > 1 {
			out = append(out, Edit{Kind: EditShortenString, Target: path, ops: []op{{kind: opShortenNode, index: n.id}}})
		}
	})
	return out
}

// selection is the union of the operations of the applied edits.
type selection struct {
	nodes        changes
	headers      map[int]bool
	query        map[int]bool
	mockRules    map[int]bool
	mockBidders  map[string]bool
	bidderResps  map[string]bool
	inlineRules  map[int]bool
	decisions    map[string]bool
	truncateBody bool
}

func selectionOf(edits []Edit) selection {
	s := selection{
		nodes: newChanges(), headers: map[int]bool{}, query: map[int]bool{}, mockRules: map[int]bool{},
		mockBidders: map[string]bool{}, bidderResps: map[string]bool{}, inlineRules: map[int]bool{}, decisions: map[string]bool{},
	}
	for _, e := range edits {
		for _, o := range e.ops {
			s.add(o)
		}
	}
	return s
}

func (s *selection) add(o op) {
	switch o.kind {
	case opDeleteNode:
		s.nodes.del[o.index] = true
	case opMinimalNode:
		s.nodes.minimal[o.index] = true
	case opShortenNode:
		s.nodes.shorten[o.index] = true
	case opDropHeader:
		s.headers[o.index] = true
	case opDropQuery:
		s.query[o.index] = true
	case opDropMockRule:
		s.mockRules[o.index] = true
	case opDropMockBidder:
		s.mockBidders[o.key] = true
	case opDropBidderResponse:
		s.bidderResps[o.key] = true
	case opDropInlineRule:
		s.inlineRules[o.index] = true
	case opDropDecision:
		s.decisions[o.key] = true
	case opTruncateBody:
		s.truncateBody = true
	}
}

// apply returns the base subject with the edits applied.
func (b *base) apply(edits []Edit) (Subject, error) {
	out, err := b.subj.Clone()
	if err != nil {
		return Subject{}, err
	}
	sel := selectionOf(edits)
	b.applyBody(&out, sel)
	req := &out.Envelope.Request
	req.Headers = filterIndexed(req.Headers, sel.headers)
	if len(b.query) > 0 {
		req.Query = strings.Join(filterIndexed(b.query, sel.query), "&")
	}
	out.Mocks.HTTP = filterIndexed(out.Mocks.HTTP, sel.mockRules)
	out.Mocks.Bidders = filterKeys(out.Mocks.Bidders, sel.mockBidders)
	out.Envelope.Mock.BidderResponses = filterKeys(out.Envelope.Mock.BidderResponses, sel.bidderResps)
	out.Envelope.Decisions = filterKeys(out.Envelope.Decisions, sel.decisions)
	if inline := out.Envelope.Mock.Inline; inline != nil {
		inline.Rules = filterIndexed(inline.Rules, sel.inlineRules)
	}
	return out, nil
}

// applyBody re-encodes the edited body. The content encoding is kept: input
// bodies may combine text with content_encoding, and the server encodes it.
func (b *base) applyBody(out *Subject, sel selection) {
	var content []byte
	switch {
	case b.body != nil:
		var buf bytes.Buffer
		b.body.encode(&buf, sel.nodes)
		content = buf.Bytes()
	case sel.truncateBody:
		content = b.rawBody[:len(b.rawBody)/2]
	default:
		return
	}
	text := string(content)
	out.Envelope.Request.Body = &parity.Body{Text: &text, ContentEncoding: b.encoding}
}

func filterIndexed[T any](in []T, drop map[int]bool) []T {
	if len(drop) == 0 || in == nil {
		return in
	}
	out := make([]T, 0, len(in))
	for i, v := range in {
		if !drop[i] {
			out = append(out, v)
		}
	}
	return out
}

func filterKeys[V any](in map[string]V, drop map[string]bool) map[string]V {
	if len(drop) == 0 || in == nil {
		return in
	}
	out := make(map[string]V, len(in))
	for k, v := range in {
		if !drop[k] {
			out[k] = v
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// Size measures a subject for the reduction report.
type Size struct {
	BodyBytes   int `json:"body_bytes"`
	Imps        int `json:"imps"`
	Bidders     int `json:"bidders"`
	Headers     int `json:"headers"`
	QueryParams int `json:"query_params"`
	MockRules   int `json:"mock_rules"`
	BodyNodes   int `json:"body_nodes"`
}

// SizeOf measures s.
func SizeOf(s Subject) Size {
	b, err := newBase(s)
	if err != nil {
		return Size{}
	}
	sz := Size{BodyBytes: len(b.rawBody), Headers: len(s.Envelope.Request.Headers), QueryParams: len(b.query), MockRules: len(s.Mocks.HTTP)}
	if b.body != nil {
		b.body.walk("$", func(*node, string) { sz.BodyNodes++ })
		if imps, ok := b.body.member("imp"); ok && imps.kind == kindArray {
			sz.Imps = len(imps.elems)
		}
		sz.Bidders = len(b.bidderNodes())
	}
	return sz
}
