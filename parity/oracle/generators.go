package oracle

import (
	"encoding/json"
	"fmt"
	"net/url"
	"sort"
	"strings"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/transport"
)

// Notes the oracle attaches to captures.
const (
	// NoteUnexpectedOutbound flags a call no mock source expected.
	NoteUnexpectedOutbound = "unexpected_outbound"
	// NoteNoMeta flags a call without X-Parity-Meta.
	NoteNoMeta = "missing_parity_meta"
)

// Mock IDs of the built-in answers.
const (
	mockIDBidderDefault = "bidder:204"
	mockIDNotFound      = "not_found"
	mockIDCache         = "cache:content_uuid"
	mockIDFloorsMissing = "floors:not_in_plan"
)

// inbound is a received request with everything resolution needs.
type inbound struct {
	req       *wireRequest
	url       *url.URL
	meta      parity.OutboundMeta
	hasMeta   bool
	metaErr   error
	headers   parity.HeaderList
	encoding  string
	decoded   []byte
	decodeErr error
	doc       any
	kind      string
	impIDs    []string
}

// newInbound parses the target, strips and decodes X-Parity-Meta and decodes
// the body (gzip) and its JSON.
func newInbound(req *wireRequest) *inbound {
	in := &inbound{req: req, headers: parity.HeaderList{}}
	for _, kv := range req.headers {
		if strings.EqualFold(kv[0], parity.MetaHeader) {
			if !in.hasMeta {
				in.hasMeta = true
				in.meta, in.metaErr = transport.DecodeMeta(kv[1])
			}
			continue
		}
		in.headers = append(in.headers, kv)
	}
	in.url, _ = url.Parse(req.target)
	if in.url == nil {
		in.url = &url.URL{Path: req.target}
	}
	in.encoding, _ = headerValue(req.headers, "Content-Encoding")
	body := parity.NewBody(req.body, in.encoding)
	in.decoded, in.decodeErr = body.DecodedBytes()
	if len(in.decoded) > 0 {
		if doc, err := decodeUseNumber(in.decoded); err == nil {
			in.doc = doc
		}
	}
	in.kind = in.meta.Kind
	return in
}

// host returns the request host (from the absolute target, else Host header).
func (in *inbound) host() string {
	if in.url.Host != "" {
		return in.url.Hostname()
	}
	h, _ := headerValue(in.headers, "Host")
	return strings.Split(h, ":")[0]
}

// origURL is the pre-rewrite URL from the meta, else the request target.
func (in *inbound) origURL() string {
	if in.meta.OrigURL != "" {
		return in.meta.OrigURL
	}
	return in.req.target
}

func (in *inbound) jsonPathEquals(pe PathEquals) bool {
	if in.doc == nil {
		return false
	}
	steps, err := parsePath(pe.Path)
	if err != nil {
		return false
	}
	v, ok := lookupPath(in.doc, steps)
	if !ok {
		return false
	}
	raw, err := json.Marshal(v)
	return err == nil && semanticEqual(raw, pe.Value)
}

// requestID returns the top-level "id" of a JSON object body.
func (in *inbound) requestID() string {
	obj, _ := in.doc.(map[string]any)
	id, _ := obj["id"].(string)
	return id
}

// findImpIDs returns the sorted imp IDs of the outgoing request: imp[].id of
// an OpenRTB body, otherwise the plan's inbound imp IDs that appear in the body
// as a JSON string or in the URL as a query value (or comma-separated part).
func findImpIDs(in *inbound, candidates []string) []string {
	if obj, ok := in.doc.(map[string]any); ok {
		if imps, ok := obj["imp"].([]any); ok {
			var ids []string
			for _, imp := range imps {
				m, _ := imp.(map[string]any)
				if id, ok := m["id"].(string); ok && id != "" {
					ids = append(ids, id)
				}
			}
			return sortedUnique(ids)
		}
	}
	var ids []string
	body := string(in.decoded)
	query := queryValues(in.url)
	for _, id := range candidates {
		if id == "" {
			continue
		}
		if strings.Contains(body, jsonQuote(id)) || query[id] {
			ids = append(ids, id)
		}
	}
	return sortedUnique(ids)
}

// queryValues returns every query value and comma-separated part of one.
func queryValues(u *url.URL) map[string]bool {
	out := map[string]bool{}
	for _, values := range u.Query() {
		for _, v := range values {
			out[v] = true
			for _, part := range strings.Split(v, ",") {
				out[part] = true
			}
		}
	}
	return out
}

// reply is the oracle's answer to one call.
type reply struct {
	status     int
	headers    parity.HeaderList
	body       []byte
	delay      time.Duration
	fail       string
	mock       parity.MockRef
	unexpected bool
	notes      []string
}

func jsonReply(status int, body []byte, mock parity.MockRef) reply {
	return reply{status: status, headers: parity.HeaderList{{"Content-Type", "application/json"}}, body: body, mock: mock}
}

// notFound is a 404 answer; note is recorded on the capture when non-empty.
func notFound(source, id, note string) reply {
	r := reply{status: 404, headers: parity.HeaderList{}, mock: parity.MockRef{Source: source, ID: id}}
	if note != "" {
		r.notes = append(r.notes, note)
	}
	return r
}

// unexpectedReply is the 404 for calls no mock source expected.
func unexpectedReply(note string) reply {
	r := notFound(parity.MockSourceDefault, mockIDNotFound, NoteUnexpectedOutbound)
	r.unexpected = true
	if note != "" {
		r.notes = append(r.notes, note)
	}
	return r
}

// inlineReply converts a plan response.
func inlineReply(resp parity.InlineResponse, mock parity.MockRef) (reply, error) {
	body, err := resp.Body.WireBytes()
	if err != nil {
		return reply{}, fmt.Errorf("oracle: mock %s body: %w", mock.ID, err)
	}
	r := reply{status: resp.Status, headers: resp.Headers, body: body, delay: time.Duration(resp.DelayMS) * time.Millisecond, mock: mock}
	if r.headers == nil {
		r.headers = parity.HeaderList{}
	}
	if resp.Fail != nil {
		r.fail = *resp.Fail
	}
	return r, nil
}

// resolve picks the answer for a call. Order: calls without (valid) meta are
// served from static paths or flagged; then plan rules; then the per-kind
// sources (bidder: fixture replay, template, 204; cache: content UUIDs;
// currency and vendorlist: static; floors: plan or 404; others: flagged 404).
func (o *Oracle) resolve(in *inbound, plan *Plan) reply {
	if in.metaErr != nil {
		return unexpectedReply("invalid X-Parity-Meta: " + in.metaErr.Error())
	}
	if !in.hasMeta {
		if r, ok := o.static.serve(in.host(), in.url.Path); ok {
			r.notes = append(r.notes, NoteNoMeta)
			return r
		}
		return unexpectedReply(NoteNoMeta)
	}
	if plan != nil {
		in.impIDs = findImpIDs(in, plan.ImpIDs)
		if rule, ok := plan.match(in); ok {
			r, err := inlineReply(rule.Response, parity.MockRef{Source: parity.MockSourceRule, ID: rule.ID})
			if err != nil {
				return reply{status: 500, headers: parity.HeaderList{}, mock: r.mock, notes: []string{err.Error()}}
			}
			return r
		}
	} else {
		in.impIDs = findImpIDs(in, nil)
	}
	return o.resolveKind(in, plan)
}

func (o *Oracle) resolveKind(in *inbound, plan *Plan) reply {
	switch in.kind {
	case parity.OutboundBidder:
		return o.resolveBidder(in, plan)
	case parity.OutboundCache:
		return cacheReply(in)
	case parity.OutboundFloors:
		if plan != nil && plan.Floors != nil {
			r, err := inlineReply(*plan.Floors, parity.MockRef{Source: parity.MockSourceRule, ID: "floors"})
			if err == nil {
				return r
			}
		}
		return notFound(parity.MockSourceDefault, mockIDFloorsMissing, "")
	}
	if r, ok := o.static.serve(in.host(), in.url.Path); ok {
		return r
	}
	return unexpectedReply("")
}

// resolveBidder runs fixture replay, then the template generator, then 204.
func (o *Oracle) resolveBidder(in *inbound, plan *Plan) reply {
	bidder := in.meta.Bidder
	if plan == nil || !plan.DisableReplay {
		if e, ok := o.replay.lookup(bidder, in.origURL(), in.decoded); ok {
			r := reply{status: e.response.Status, headers: headerListOf(e.response.Headers), mock: parity.MockRef{Source: parity.MockSourceFixture, ID: e.id}}
			if len(e.response.Body) > 0 {
				r.body = append([]byte(nil), e.response.Body...)
			}
			return r
		}
	}
	if plan == nil || !plan.DisableTemplates {
		if t, ok := o.templates.Template(bidder); ok {
			return renderReply(t, in, plan)
		}
	}
	return reply{status: 204, headers: parity.HeaderList{}, mock: parity.MockRef{Source: parity.MockSourceDefault, ID: mockIDBidderDefault}}
}

// renderReply answers from a template keyed by (case, bidder, sorted imp IDs).
func renderReply(t *Template, in *inbound, plan *Plan) reply {
	input := RenderInput{CaseID: in.meta.Case, Bidder: in.meta.Bidder, RequestID: in.requestID(), ImpIDs: in.impIDs}
	if plan != nil {
		input.Params = plan.Bidders[in.meta.Bidder]
	}
	mock := parity.MockRef{Source: parity.MockSourceTemplate, ID: t.Source}
	body, err := t.Render(input)
	if err != nil {
		return reply{status: 500, headers: parity.HeaderList{}, mock: mock, notes: []string{err.Error()}}
	}
	r := reply{status: t.Status, headers: append(parity.HeaderList{}, t.Headers...), body: body, mock: mock}
	if len(in.impIDs) == 0 {
		r.notes = append(r.notes, "template: no imp ids found in the outgoing request")
	}
	return r
}

// cacheReply answers a Prebid Cache POST with content-addressed UUIDs.
func cacheReply(in *inbound) reply {
	mock := parity.MockRef{Source: parity.MockSourceDefault, ID: mockIDCache}
	if in.req.method != "POST" {
		r := unexpectedReply("cache call is not a POST")
		return r
	}
	uuids, err := CacheUUIDs(in.decoded)
	if err != nil {
		return reply{status: 400, headers: parity.HeaderList{{"Content-Type", "text/plain"}}, body: []byte(err.Error()), mock: mock, notes: []string{err.Error()}}
	}
	body, err := cacheResponseBody(uuids)
	if err != nil {
		return reply{status: 500, headers: parity.HeaderList{}, mock: mock, notes: []string{err.Error()}}
	}
	return jsonReply(200, body, mock)
}

// headerListOf flattens fixture headers sorted by name.
func headerListOf(h map[string][]string) parity.HeaderList {
	out := parity.HeaderList{}
	names := make([]string, 0, len(h))
	for name := range h {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		for _, v := range h[name] {
			out = append(out, [2]string{name, v})
		}
	}
	return out
}
