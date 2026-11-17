package gen

import (
	"crypto/sha1"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
)

// Validity says what a step does to a request Go would otherwise accept.
type Validity int

const (
	// Preserves keeps a valid request valid.
	Preserves Validity = iota
	// Uncertain may or may not keep the request valid.
	Uncertain
	// Invalidates makes the request invalid on purpose (still well-formed).
	Invalidates
	// Malformed breaks the JSON body or the HTTP framing.
	Malformed
)

// String names the validity class.
func (v Validity) String() string {
	switch v {
	case Preserves:
		return "preserves"
	case Uncertain:
		return "uncertain"
	case Invalidates:
		return "invalidates"
	case Malformed:
		return "malformed"
	}
	return "unknown"
}

// Phase orders steps when a chain is drawn: request features first, then
// structural edits of the JSON tree, then byte-level edits of the body, then
// HTTP framing, and content encoding last. Replayed IDs apply steps in the
// order they list them.
type Phase int

// Step phases.
const (
	PhaseFeature Phase = iota + 1
	PhaseStructure
	PhaseRaw
	PhaseHTTP
	PhaseEncoding
)

// Generated IDs and the fixed size the oversized-body step exceeds: the Go
// default max_request_size (config.go SetDefault) plus a margin.
const (
	requestIDPrefix   = "parity-gen-"
	storedIDPrefix    = "parity-gen-"
	defaultMaxRequest = 262144
	oversizeMargin    = 1024
)

// queryParam is one raw "key=value" segment of a query string.
type queryParam struct {
	key, value string
	hasValue   bool
}

// draft is a case under construction.
type draft struct {
	g           *Generator
	id          ID
	occurrences map[string]int

	kind        corpus.Kind
	endpoint    string
	fixturePath string
	description string

	method     string
	path       string
	query      []queryParam
	headers    parity.HeaderList
	host       string
	remoteAddr string

	// body is the JSON tree of the HTTP body (auction, video); nil when the
	// body is held as bytes in raw.
	body any
	// raw holds the exact body bytes once a byte-level step ran (or when the
	// source body is not JSON); rawSet distinguishes an empty body.
	raw             []byte
	rawSet          bool
	contentEncoding string

	// amp is the AMP stored request (tag_id's content); ampTagAuto says the
	// tag_id query value is rewritten to amp's content-addressed ID.
	amp        obj
	ampTagAuto bool

	stored          corpus.Stored
	profile         corpus.Profile
	mocks           corpus.MockPlan
	mockLevel       string
	bidderResponses map[string]parity.BidderResponseMock
	decisions       map[string]json.RawMessage

	tags     []string
	notes    []string
	validity Validity
	phase    Phase
}

// req returns the OpenRTB-shaped object request-level steps edit: the auction
// body, the AMP stored request or the video body. It is nil once the body is
// raw bytes or when it is not a JSON object.
func (d *draft) req() obj {
	if d.endpoint == corpus.EndpointAMP {
		return d.amp
	}
	if d.rawSet {
		return nil
	}
	o, _ := d.body.(obj)
	return o
}

// hasBody reports whether the endpoint takes a request body.
func (d *draft) hasBody() bool {
	return d.endpoint != corpus.EndpointAMP
}

// bodyBytes returns the current body bytes.
func (d *draft) bodyBytes() ([]byte, error) {
	if d.rawSet {
		return d.raw, nil
	}
	if d.body == nil {
		return nil, nil
	}
	return encodeTree(d.body)
}

// setRaw replaces the body with exact bytes.
func (d *draft) setRaw(b []byte) {
	d.raw = b
	d.rawSet = true
	d.body = nil
}

// note records a human-readable remark on the case.
func (d *draft) note(format string, args ...any) {
	d.notes = append(d.notes, fmt.Sprintf(format, args...))
}

// setHeader replaces every header named name (case-insensitively).
func (d *draft) setHeader(name, value string) {
	d.dropHeader(name)
	d.headers = append(d.headers, [2]string{name, value})
}

// dropHeader removes every header named name.
func (d *draft) dropHeader(name string) {
	out := parity.HeaderList{}
	for _, h := range d.headers {
		if !strings.EqualFold(h[0], name) {
			out = append(out, h)
		}
	}
	d.headers = out
}

// setQuery sets (or appends) a query parameter; value is escaped.
func (d *draft) setQuery(key, value string) {
	escaped := queryEscape(value)
	for i := range d.query {
		if d.query[i].key == key {
			d.query[i].value, d.query[i].hasValue = escaped, true
			return
		}
	}
	d.query = append(d.query, queryParam{key: key, value: escaped, hasValue: true})
}

// dropQuery removes every parameter named key.
func (d *draft) dropQuery(key string) {
	var out []queryParam
	for _, q := range d.query {
		if q.key != key {
			out = append(out, q)
		}
	}
	d.query = out
}

func parseQuery(raw string) []queryParam {
	if raw == "" {
		return nil
	}
	var out []queryParam
	for _, part := range strings.Split(raw, "&") {
		k, v, ok := strings.Cut(part, "=")
		out = append(out, queryParam{key: k, value: v, hasValue: ok})
	}
	return out
}

func encodeQuery(params []queryParam) string {
	parts := make([]string, len(params))
	for i, q := range params {
		parts[i] = q.key
		if q.hasValue {
			parts[i] += "=" + q.value
		}
	}
	return strings.Join(parts, "&")
}

// storeAs adds content to one stored-data kind under a content-addressed ID
// and returns the ID, so equal content always shares one ID and different
// content never collides across cases.
func (d *draft) storeAs(kind string, content any) (string, error) {
	raw, err := encodeTree(content)
	if err != nil {
		return "", err
	}
	id := storedIDPrefix + kind + "-" + contentHash(raw)
	var dst *map[string]json.RawMessage
	switch kind {
	case "req":
		dst = &d.stored.Requests
	case "imp":
		dst = &d.stored.Imps
	case "resp":
		dst = &d.stored.Responses
	case "acct":
		dst = &d.stored.Accounts
	default:
		return "", fmt.Errorf("gen: unknown stored kind %q", kind)
	}
	if *dst == nil {
		*dst = map[string]json.RawMessage{}
	}
	(*dst)[id] = raw
	return id, nil
}

// contentHash is the first 12 hex digits of sha1(raw).
func contentHash(raw []byte) string {
	sum := sha1.Sum(raw)
	return hex.EncodeToString(sum[:])[:12]
}

// apply runs one step on the draft and appends it to the chain.
func (d *draft) apply(step Step) error {
	m, ok := lookupMutator(step.Name)
	if !ok {
		return fmt.Errorf("%w: unknown step %q", ErrInvalidID, step.Name)
	}
	if !m.endpoints.has(d.endpoint) {
		return fmt.Errorf("%w: step %q does not apply to %s", ErrInvalidID, step.Name, d.endpoint)
	}
	occurrence := d.occurrences[step.String()]
	d.occurrences[step.String()]++
	d.id.Chain = append(d.id.Chain, step)
	if !m.appliesTo(d) {
		d.note("step %s skipped: its precondition does not hold", step)
		return nil
	}
	r := Derive(d.id.Seed, "step", d.id.Source, u64(d.id.Index), step.Name, strconv.FormatUint(uint64(step.Variant), 10), strconv.Itoa(occurrence))
	if err := m.apply(d, r); err != nil {
		return fmt.Errorf("gen: step %s: %w", step, err)
	}
	d.worsen(m.validity)
	if m.phase > d.phase {
		d.phase = m.phase
	}
	return nil
}

// finish assembles the corpus case.
func (d *draft) finish() (corpus.Case, error) {
	id := d.id.String()
	if err := d.finishAMP(); err != nil {
		return corpus.Case{}, err
	}
	body, err := d.envelopeBody()
	if err != nil {
		return corpus.Case{}, err
	}
	env := &parity.Envelope{
		Schema:        parity.SchemaEnvelope,
		SchemaVersion: parity.SchemaVersion,
		CaseID:        id,
		FixturePath:   d.fixturePath,
		Profile:       d.profile.Name(),
		Request: parity.ExecRequest{
			Method:     d.method,
			Path:       d.path,
			Query:      encodeQuery(d.query),
			Host:       d.host,
			Headers:    append(parity.HeaderList{}, d.headers...),
			RemoteAddr: d.remoteAddr,
			Body:       body,
		},
		Mock:      parity.MockSpec{Mode: parity.MockModeOracle, Level: d.mockLevel, BidderResponses: d.bidderResponses},
		Decisions: d.pinnedDecisions(),
		Capture:   parity.CaptureSpec{Stages: json.RawMessage(`"*"`)},
	}
	d.mocks.ImpIDs = d.impIDs()
	return corpus.Case{
		ID:          id,
		Kind:        d.kind,
		Corpus:      CorpusGenerated,
		FixturePath: d.fixturePath,
		Description: d.description,
		Endpoint:    d.endpoint,
		Profile:     d.profile,
		Tags:        d.finalTags(),
		Notes:       append([]string(nil), d.notes...),
		Envelope:    env,
		Mocks:       d.mocks,
		Stored:      d.stored,
	}, nil
}

// finishAMP stores the AMP request under its content-addressed tag.
func (d *draft) finishAMP() error {
	if d.endpoint != corpus.EndpointAMP || d.amp == nil || !d.ampTagAuto {
		return nil
	}
	has := false
	for _, q := range d.query {
		has = has || q.key == "tag_id"
	}
	if !has {
		return nil
	}
	tag, err := d.storeAs("req", d.amp)
	if err != nil {
		return err
	}
	for i := range d.query {
		if d.query[i].key == "tag_id" {
			d.query[i].value, d.query[i].hasValue = tag, true
			break
		}
	}
	return nil
}

// envelopeBody encodes the body per the Body rules: text for UTF-8 identity
// bodies, b64 otherwise.
func (d *draft) envelopeBody() (*parity.Body, error) {
	raw, err := d.bodyBytes()
	if err != nil {
		return nil, err
	}
	if len(raw) == 0 {
		return nil, nil
	}
	if d.contentEncoding == "" && utf8.Valid(raw) {
		return parity.TextBody(string(raw)), nil
	}
	b64 := base64.StdEncoding.EncodeToString(raw)
	return &parity.Body{B64: &b64, ContentEncoding: d.contentEncoding}, nil
}

// pinnedDecisions pins every decision point the steps left open, so both
// servers draw nothing at random.
func (d *draft) pinnedDecisions() map[string]json.RawMessage {
	out := map[string]json.RawMessage{}
	for k, v := range d.decisions {
		out[k] = v
	}
	r := Derive(d.id.Seed, "decisions", d.id.Source, u64(d.id.Index))
	for _, point := range envelopeDecisions() {
		value := json.RawMessage(strconv.Itoa(r.Weighted([]int{3, 1})))
		switch parity.DecisionID(point) {
		case parity.DecisionFloorsModelGroup:
			value = json.RawMessage("0")
		case parity.DecisionUsersyncShuffle:
			value = json.RawMessage(strconv.Quote(parity.UsersyncShuffleIdentity))
		}
		if _, ok := out[point]; !ok {
			out[point] = value
		}
	}
	return out
}

// impIDs returns the imp IDs the inbound request will carry.
func (d *draft) impIDs() []string {
	if d.endpoint == corpus.EndpointVideo {
		return videoImpIDs(d.req())
	}
	req := d.req()
	if req == nil {
		return d.mocks.ImpIDs
	}
	var out []string
	for _, imp := range impsOf(req) {
		if s, ok := imp["id"].(string); ok {
			out = append(out, s)
		}
	}
	return out
}

// finalTags returns the source tags plus the generator tags.
func (d *draft) finalTags() []string {
	set := map[string]bool{TagGenerated: true}
	for _, t := range d.tags {
		set[t] = true
	}
	switch d.validity {
	case Preserves:
		set[TagValidIntent] = true
	case Uncertain:
		set[TagUncertain] = true
	case Invalidates:
		set[TagInvalidIntent] = true
	case Malformed:
		set[TagMalformed] = true
	}
	out := sortedKeys(set)
	sort.Strings(out)
	return out
}

// newDraft returns an empty draft for id on endpoint.
func (g *Generator) newDraft(id ID, endpoint string) *draft {
	d := &draft{
		g:           g,
		id:          id,
		occurrences: map[string]int{},
		kind:        corpus.KindEndpoint,
		endpoint:    endpoint,
		path:        endpoint,
		method:      http.MethodPost,
		host:        corpus.DefaultHost,
		remoteAddr:  corpus.DefaultRemoteAddr,
		headers:     parity.HeaderList{{"Content-Type", "application/json"}},
		mockLevel:   parity.MockLevelHTTP,
		decisions:   map[string]json.RawMessage{},
	}
	if endpoint == corpus.EndpointAMP {
		d.method = http.MethodGet
		d.headers = parity.HeaderList{}
		d.ampTagAuto = true
	}
	return d
}

// start builds the draft a chain applies to: a base request for generated IDs
// or a copy of the source case for derived IDs.
func (g *Generator) start(id ID) (*draft, error) {
	if !id.Generated() {
		src, err := g.sourceCase(id.Source)
		if err != nil {
			return nil, err
		}
		return g.fromCase(ID{Source: id.Source, Seed: id.Seed, Index: id.Index}, src)
	}
	base := id.Chain[0]
	if base.Variant != 0 {
		return nil, fmt.Errorf("%w: base %q takes no variant", ErrInvalidID, base)
	}
	r := Derive(id.Seed, "base", u64(id.Index), base.Name)
	start := ID{Seed: id.Seed, Index: id.Index, Chain: []Step{base}}
	switch base.Name {
	case BaseAuction:
		d := g.newDraft(start, corpus.EndpointAuction)
		return d, g.baseAuction(r, d)
	case BaseAMP:
		d := g.newDraft(start, corpus.EndpointAMP)
		return d, g.baseAMP(r, d)
	case BaseVideo:
		d := g.newDraft(start, corpus.EndpointVideo)
		return d, g.baseVideo(r, d)
	}
	return nil, fmt.Errorf("%w: unknown base %q", ErrInvalidID, base.Name)
}

// fromCase copies a corpus case into a draft.
func (g *Generator) fromCase(id ID, src *corpus.Case) (*draft, error) {
	d := g.newDraft(id, src.Endpoint)
	env := src.Envelope
	d.kind = src.Kind
	d.fixturePath = src.FixturePath
	d.description = src.Description
	d.method, d.path = env.Request.Method, env.Request.Path
	d.query = parseQuery(env.Request.Query)
	d.headers = append(parity.HeaderList{}, env.Request.Headers...)
	d.host, d.remoteAddr = env.Request.Host, env.Request.RemoteAddr
	d.profile = src.Profile
	d.mocks = copyMocks(src.Mocks)
	d.stored = copyStored(src.Stored)
	d.mockLevel = env.Mock.EffectiveLevel()
	if len(env.Mock.BidderResponses) > 0 {
		d.bidderResponses = map[string]parity.BidderResponseMock{}
		for k, v := range env.Mock.BidderResponses {
			d.bidderResponses[k] = v
		}
	}
	for k, v := range env.Decisions {
		d.decisions[k] = v
	}
	d.tags = append([]string(nil), src.Tags...)
	d.notes = append(append([]string(nil), src.Notes...), "derived from corpus case "+src.ID)
	d.validity = sourceValidity(src)
	if src.Endpoint == corpus.EndpointAMP {
		return d, d.loadAMPSource()
	}
	return d, d.loadBodySource(env.Request.Body)
}

// sourceValidity classifies a corpus case: invalid when its fixture expects a
// rejection, uncertain when Go's endpoint validation never judged its request
// (lifted adapter fixtures, exchangetest specs) or judged it with test-only
// wiring, valid otherwise.
func sourceValidity(src *corpus.Case) Validity {
	switch {
	case src.Expect.Status >= http.StatusBadRequest || src.Expect.ErrorContains != "" || src.HasTag(corpus.TagInvalidJSON):
		return Invalidates
	case src.Kind == corpus.KindLifted || src.Kind == corpus.KindExchange || src.Kind == corpus.KindBidderParams:
		return Uncertain
	case src.HasTag(corpus.TagTestHooks) || src.HasTag(corpus.TagMockParamsValidator) || src.HasTag(corpus.TagExchangeInternal):
		return Uncertain
	}
	return Preserves
}

// loadBodySource keeps a JSON body as a tree and anything else as bytes.
func (d *draft) loadBodySource(b *parity.Body) error {
	if b == nil {
		d.setRaw(nil)
		return nil
	}
	wire, err := b.WireBytes()
	if err != nil {
		return fmt.Errorf("gen: source body: %w", err)
	}
	if b.B64 != nil || b.ContentEncoding != "" {
		d.setRaw(wire)
		d.contentEncoding = b.ContentEncoding
		return nil
	}
	tree, err := decodeTree(wire)
	if err != nil {
		d.setRaw(wire)
		return nil
	}
	d.body = tree
	return nil
}

// loadAMPSource moves the source's AMP stored request into the draft so steps
// can edit it; finish stores it again under a new content-addressed tag.
func (d *draft) loadAMPSource() error {
	tag := ""
	for _, q := range d.query {
		if q.key == "tag_id" {
			tag = q.value
			break
		}
	}
	raw, ok := d.stored.Requests[tag]
	if !ok {
		d.ampTagAuto = false
		return nil
	}
	o, err := decodeObject(raw)
	if err != nil {
		d.ampTagAuto = false
		return nil
	}
	delete(d.stored.Requests, tag)
	d.amp = o
	return nil
}

func copyMocks(m corpus.MockPlan) corpus.MockPlan {
	out := corpus.MockPlan{ImpIDs: append([]string(nil), m.ImpIDs...), HTTP: append([]corpus.HTTPMock(nil), m.HTTP...)}
	if len(m.Bidders) > 0 {
		out.Bidders = make(map[string]corpus.MockBidder, len(m.Bidders))
		for k, v := range m.Bidders {
			out.Bidders[k] = v
		}
	}
	return out
}

func copyStored(s corpus.Stored) corpus.Stored {
	cp := func(m map[string]json.RawMessage) map[string]json.RawMessage {
		if m == nil {
			return nil
		}
		out := make(map[string]json.RawMessage, len(m))
		for k, v := range m {
			out[k] = v
		}
		return out
	}
	return corpus.Stored{Requests: cp(s.Requests), Imps: cp(s.Imps), Responses: cp(s.Responses), Accounts: cp(s.Accounts)}
}

// queryEscape escapes a query value the way url.QueryEscape does.
func queryEscape(s string) string {
	const hexDigits = "0123456789ABCDEF"
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z', c >= '0' && c <= '9', c == '-', c == '_', c == '.', c == '~':
			b.WriteByte(c)
		case c == ' ':
			b.WriteByte('+')
		default:
			b.WriteByte('%')
			b.WriteByte(hexDigits[c>>4])
			b.WriteByte(hexDigits[c&0x0f])
		}
	}
	return b.String()
}
