package diff

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
)

// view is one side's trace in comparable form: every value parsed into nodes,
// bodies decoded, then normalized by the active rules.
type view struct {
	entries   map[parity.StageID]map[string]*entry
	status    map[parity.StageID]parity.StageStatus
	httpIn    *message
	httpResp  *message
	httpOut   []*call
	analytics *node
	metrics   []parity.MetricDelta
	decisions []parity.Decision
	notes     []string
}

// entry is one stage entry. errors and warnings are nil for a null list.
type entry struct {
	value    *node
	errors   *node
	warnings *node
	seq      int
	writes   int
}

// message is http_in or http_response.
type message struct {
	method, path, query string
	status              int
	headers             parity.HeaderList
	body                *node
}

// call is one http_out entry.
type call struct {
	src         parity.OutboundCall
	url         string
	origURL     string
	reqHeaders  parity.HeaderList
	reqBody     *node
	respHeaders parity.HeaderList
	respBody    *node
}

func (v *view) note(format string, args ...any) {
	v.notes = append(v.notes, fmt.Sprintf(format, args...))
}

// buildView converts a trace into a view. side names the implementation in notes.
func buildView(t *parity.Trace, side string) *view {
	v := &view{
		entries: map[parity.StageID]map[string]*entry{},
		status:  map[parity.StageID]parity.StageStatus{},
	}
	for stage, st := range t.StageStatus {
		v.status[stage] = st
	}
	for stage, keyed := range t.Stages {
		v.entries[stage] = make(map[string]*entry, len(keyed))
		for key, e := range keyed {
			v.entries[stage][key] = v.buildEntry(side, stage, key, e)
		}
	}
	if t.HTTPIn != nil {
		v.httpIn = &message{method: t.HTTPIn.Method, path: t.HTTPIn.Path, query: t.HTTPIn.Query,
			headers: copyHeaders(t.HTTPIn.Headers), body: v.bodyNode(side, "http_in", t.HTTPIn.Body)}
	}
	if t.HTTPResponse != nil {
		v.httpResp = &message{status: t.HTTPResponse.Status, headers: copyHeaders(t.HTTPResponse.Headers),
			body: v.bodyNode(side, "http_response", t.HTTPResponse.Body)}
	}
	for i, c := range t.HTTPOut {
		v.httpOut = append(v.httpOut, v.buildCall(side, i, c))
	}
	v.analytics = v.analyticsNode(side, t.Analytics)
	v.metrics = append(v.metrics, t.MetricsDelta...)
	v.decisions = append(v.decisions, t.Decisions...)
	return v
}

func (v *view) buildEntry(side string, stage parity.StageID, key string, e parity.StageEntry) *entry {
	out := &entry{seq: e.Seq, writes: e.Writes}
	out.value = v.entryValue(side, stage, key, e)
	for _, sel := range bodyMembers[stage] {
		for _, n := range selectNodes(out.value, sel) {
			v.decodeBodyMember(side, stage, n)
		}
	}
	if e.Errors != nil {
		out.errors = v.typedNode(side, e.Errors)
	}
	if e.Warnings != nil {
		out.warnings = v.typedNode(side, e.Warnings)
	}
	return out
}

// entryValue parses the exact raw bytes when present (so lexical differences
// survive), else the semantic json member, marked unrecorded.
func (v *view) entryValue(side string, stage parity.StageID, key string, e parity.StageEntry) *node {
	switch {
	case e.Raw != nil:
		return v.exactOrText(side, fmt.Sprintf("stage %s[%s] raw", stage, key), []byte(*e.Raw))
	case e.RawB64 != nil:
		raw, err := base64.StdEncoding.DecodeString(*e.RawB64)
		if err != nil {
			v.note("%s: stage %s[%s] raw_b64: %v", side, stage, key, err)
			return newString(*e.RawB64)
		}
		return v.exactOrText(side, fmt.Sprintf("stage %s[%s] raw_b64", stage, key), raw)
	case len(e.JSON) > 0 && string(e.JSON) != "null":
		n, err := parseSemantic(e.JSON)
		if err != nil {
			v.note("%s: stage %s[%s] json: %v", side, stage, key, err)
			return newString(string(e.JSON))
		}
		v.note("%s: stage %s[%s] has no raw bytes; lexical comparison skipped", side, stage, key)
		n.unrecorded = true
		return n
	}
	return nil
}

// exactOrText parses exact bytes as JSON, or keeps non-JSON bytes as a string.
func (v *view) exactOrText(side, what string, raw []byte) *node {
	n, err := parseExact(raw)
	if err == nil {
		return n
	}
	if !utf8.Valid(raw) {
		return newString(base64.StdEncoding.EncodeToString(raw))
	}
	v.note("%s: %s is not JSON (%v); compared as text", side, what, err)
	return newString(string(raw))
}

// typedNode converts a typed trace member to a non-exact node.
func (v *view) typedNode(side string, value any) *node {
	n, err := fromValue(value)
	if err != nil {
		v.note("%s: %v", side, err)
		return nil
	}
	return n
}

// bodyNode decodes a recorded body. text and b64 are exact bytes; a body with
// only json is compared semantically, and marked unrecorded so that a lexical
// comparison it cannot take part in is reported rather than skipped.
func (v *view) bodyNode(side, what string, b *parity.Body) *node {
	if b == nil {
		return nil
	}
	content, err := b.DecodedBytes()
	var n *node
	switch {
	case err != nil:
		v.note("%s: %s body: %v", side, what, err)
		n = newString(bodyRepr(b))
	case b.Text == nil && b.B64 == nil:
		n = v.semanticOrText(side, what, content)
		n.unrecorded = true
	default:
		n = v.exactOrText(side, what+" body", content)
	}
	n.body = true
	n.encoding = strings.ToLower(strings.TrimSpace(b.ContentEncoding))
	return n
}

func (v *view) semanticOrText(side, what string, content []byte) *node {
	n, err := parseSemantic(content)
	if err != nil {
		v.note("%s: %s body json: %v", side, what, err)
		return newString(string(content))
	}
	return n
}

func bodyRepr(b *parity.Body) string {
	switch {
	case b.B64 != nil:
		return *b.B64
	case b.Text != nil:
		return *b.Text
	default:
		return string(b.JSON)
	}
}

// decodeBodyMember replaces a parity Body object inside a stage value with its
// decoded content, in place.
func (v *view) decodeBodyMember(side string, stage parity.StageID, n *node) {
	if n.kind != kindObject {
		return
	}
	var b parity.Body
	for _, k := range n.keys() {
		val := n.get(k)
		switch {
		case k == "text" && val.kind == kindString:
			s := val.str
			b.Text = &s
		case k == "b64" && val.kind == kindString:
			s := val.str
			b.B64 = &s
		case k == "content_encoding" && val.kind == kindString:
			b.ContentEncoding = val.str
		case k == "json":
			b.JSON = json.RawMessage(render(val))
		default:
			return
		}
	}
	decoded := v.bodyNode(side, "stage "+string(stage), &b)
	if decoded != nil {
		*n = *decoded
	}
}

func (v *view) buildCall(side string, i int, c parity.OutboundCall) *call {
	what := fmt.Sprintf("http_out[%d]", i)
	out := &call{
		src: c, url: c.Request.URL, origURL: c.OrigURL,
		reqHeaders: copyHeaders(c.Request.Header),
		reqBody:    v.bodyNode(side, what+" request", c.Request.Body),
	}
	if c.Response != nil {
		out.respHeaders = copyHeaders(c.Response.Headers)
		out.respBody = v.bodyNode(side, what+" response", c.Response.Body)
	}
	return out
}

func (v *view) analyticsNode(side string, records []parity.AnalyticsRecord) *node {
	if records == nil {
		records = []parity.AnalyticsRecord{}
	}
	return v.typedNode(side, records)
}

func copyHeaders(list parity.HeaderList) parity.HeaderList {
	return append(parity.HeaderList{}, list...)
}

// hasData reports whether the side recorded anything for stage.
func (v *view) hasData(stage parity.StageID, field string) bool {
	switch field {
	case traceFieldHTTPIn:
		return v.httpIn != nil
	case traceFieldHTTPResponse:
		return v.httpResp != nil
	case traceFieldHTTPOut:
		return len(v.httpOut) > 0
	case traceFieldAnalytics:
		return v.analytics != nil && len(v.analytics.elems) > 0
	case traceFieldMetrics:
		return len(v.metrics) > 0
	default:
		return len(v.entries[stage]) > 0
	}
}

// effectiveStatus is captured when the side recorded data, else its declared
// status, else not_reached.
func (v *view) effectiveStatus(stage parity.StageID, field string) parity.StageStatus {
	if v.hasData(stage, field) {
		return parity.StageCaptured
	}
	if st, ok := v.status[stage]; ok {
		return st
	}
	return parity.StageNotReached
}

// Trace fields of stages.json.
const (
	traceFieldStages        = "stages"
	traceFieldHTTPIn        = "http_in"
	traceFieldHTTPOut       = "http_out"
	traceFieldAnalytics     = "analytics"
	traceFieldMetrics       = "metrics_delta"
	traceFieldHTTPResponse  = "http_response"
	traceFieldIntrospection = "introspection"
)

// normalize applies the allowlist to a view: structural rules, UUID
// collection, ordering rules, then UUID numbering.
func (e *Engine) normalize(v *view, uuids *uuidSet) {
	e.structuralView(v)
	e.collectUUIDs(v, uuids)
	e.sortView(v, uuids)
	e.numberView(v, uuids)
}

func (e *Engine) structuralView(v *view) {
	for stage, keyed := range v.entries {
		for _, en := range keyed {
			e.rules.structural(stage, en.value)
		}
	}
	e.rules.structural(parity.StageAnalytics, v.analytics)
	if v.httpIn != nil {
		e.rules.structural(parity.StageHTTPIn, v.httpIn.body)
	}
	if v.httpResp != nil {
		e.rules.structural(parity.StageHTTPResponse, v.httpResp.body)
	}
	for _, c := range v.httpOut {
		e.rules.structural(parity.StageHTTPOut, c.reqBody)
		e.rules.structural(parity.StageHTTPOut, c.respBody)
	}
	if e.rules.Active(RuleGoRuntimeMetrics, parity.StageMetricsDelta) {
		v.metrics = withoutGoRuntime(v.metrics)
	}
}

// requestIDStages are where Go writes a generated request id (request $.id);
// responseIDStages are where the response echoes it.
var requestIDStages = []parity.StageID{
	parity.StageHooksEntrypoint, parity.StageHooksRawAuction, parity.StageRequestMerged,
	parity.StageRequestNormalized, parity.StageRequestValidated, parity.StageHooksProcessedAuction,
	parity.StageAuctionSetup, parity.StageFloorsEnrich, parity.StageBidderRequest,
	parity.StageHooksBidderRequest, parity.StageAdapterRequests, parity.StageAdapterRequestsFinal,
	parity.StageAnalytics,
}

// cacheTargetingSelectors locate targeting maps whose hb_cache_id* / hb_uuid*
// values are cache UUIDs: bid targeting, the AMP targeting map, or the value
// itself (amp.targeting_response).
var cacheTargetingSelectors = []string{"seatbid[*].bid[*].ext.prebid.targeting", "targeting", ""}

func (e *Engine) collectUUIDs(v *view, u *uuidSet) {
	for _, stage := range requestIDStages {
		for _, root := range e.stageRoots(v, stage) {
			for _, req := range anchorsOf(root, requestAnchors[stage]) {
				if !isRequestShaped(req) {
					continue
				}
				u.collect(RuleRequestIDUUID, stage, req.get("id"))
				u.collectAll(RuleTIDUUID, stage, req, []string{""}, "source.tid", "imp[*].ext.tid")
			}
		}
	}
	for _, stage := range parity.AllStages() {
		if responseAnchors[stage] == nil {
			continue
		}
		for _, root := range e.stageRoots(v, stage) {
			for _, resp := range anchorsOf(root, responseAnchors[stage]) {
				if isResponseShaped(resp) {
					u.collect(RuleRequestIDUUID, stage, resp.get("id"))
				}
				u.collectAll(RuleGeneratedBidID, stage, resp, []string{""}, "seatbid[*].bid[*].ext.prebid.bidid")
				u.collectAll(RuleCacheUUID, stage, resp, []string{""},
					"seatbid[*].bid[*].ext.prebid.cache.bids.cacheId", "seatbid[*].bid[*].ext.prebid.cache.vastXml.cacheId")
				collectCacheTargeting(u, stage, resp)
			}
		}
	}
	for _, c := range v.httpOut {
		switch c.src.Kind {
		case parity.OutboundBidder:
			if isRequestShaped(c.reqBody) {
				u.collect(RuleRequestIDUUID, parity.StageHTTPOut, c.reqBody.get("id"))
				u.collectAll(RuleTIDUUID, parity.StageHTTPOut, c.reqBody, []string{""}, "source.tid", "imp[*].ext.tid")
			}
		case parity.OutboundCache:
			u.collectAll(RuleCacheUUID, parity.StageHTTPOut, c.respBody, []string{""}, "responses[*].uuid")
		}
	}
	for _, root := range e.stageRoots(v, parity.StageCacheResult) {
		// stagedata.CacheResult: puts[*].id is the UUID the cache returned,
		// puts[*].assigned the cache ID the auction kept for the bid.
		u.collectAll(RuleCacheUUID, parity.StageCacheResult, root, []string{""}, "puts[*].id", "puts[*].assigned")
	}
	for _, root := range e.stageRoots(v, parity.StageAMPTargetingResponse) {
		collectCacheTargeting(u, parity.StageAMPTargetingResponse, root)
	}
}

func collectCacheTargeting(u *uuidSet, stage parity.StageID, resp *node) {
	for _, targeting := range anchorsOf(resp, cacheTargetingSelectors) {
		collectCacheKeys(u, stage, targeting)
	}
}

func collectCacheKeys(u *uuidSet, stage parity.StageID, targeting *node) {
	if targeting == nil || targeting.kind != kindObject {
		return
	}
	for _, m := range targeting.members {
		if strings.HasPrefix(m.key, "hb_cache_id") || strings.HasPrefix(m.key, "hb_uuid") {
			u.collect(RuleCacheUUID, stage, m.val)
		}
	}
}

// stageRoots returns the values a side recorded for stage.
func (e *Engine) stageRoots(v *view, stage parity.StageID) []*node {
	switch stage {
	case parity.StageAnalytics:
		return []*node{v.analytics}
	case parity.StageHTTPResponse:
		if v.httpResp != nil {
			return []*node{v.httpResp.body}
		}
		return nil
	}
	keyed := v.entries[stage]
	out := make([]*node, 0, len(keyed))
	for _, k := range sortedEntryKeys(keyed) {
		if keyed[k].value != nil {
			out = append(out, keyed[k].value)
		}
	}
	return out
}

func sortedEntryKeys(keyed map[string]*entry) []string {
	keys := make([]string, 0, len(keyed))
	for k := range keyed {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func (e *Engine) sortView(v *view, u *uuidSet) {
	for stage, keyed := range v.entries {
		for _, en := range keyed {
			e.rules.sortRules(stage, en.value, u.maskText)
			e.rules.sortEntryLists(stage, en, u.maskText)
		}
	}
	e.rules.sortRules(parity.StageAnalytics, v.analytics, u.maskText)
	if v.httpResp != nil {
		e.rules.sortRules(parity.StageHTTPResponse, v.httpResp.body, u.maskText)
	}
	for _, stage := range []parity.StageID{parity.StageAdapterBids, parity.StageHooksRawBidderResponse} {
		if e.rules.Active(RuleAdapterBidsCallKey, stage) {
			v.entries[stage] = rekeyCalls(v.entries[stage], u.maskText)
		}
	}
	if e.rules.Active(RuleCachePutsOrder, parity.StageHTTPOut) {
		for _, c := range v.httpOut {
			if c.src.Kind == parity.OutboundCache {
				sortCachePuts(c, u.maskText)
			}
		}
	}
	if e.rules.Active(RuleHTTPOutOrder, parity.StageHTTPOut) {
		sortCalls(v.httpOut, u.maskText)
	}
}

// rekeyCalls re-derives the call index of bidder#call keys from the request
// each entry belongs to (request_data.uri, request_data.body), so the keys do
// not depend on response arrival order.
func rekeyCalls(keyed map[string]*entry, mask func(string) string) map[string]*entry {
	if len(keyed) == 0 {
		return keyed
	}
	groups := map[string][]*entry{}
	out := make(map[string]*entry, len(keyed))
	for _, key := range sortedEntryKeys(keyed) {
		bidder, _, ok := cutLast(key, "#")
		if !ok {
			out[key] = keyed[key]
			continue
		}
		groups[bidder] = append(groups[bidder], keyed[key])
	}
	for bidder, entries := range groups {
		sort.SliceStable(entries, func(i, j int) bool {
			return callSortKey(entries[i], mask) < callSortKey(entries[j], mask)
		})
		for i, en := range entries {
			out[bidder+"#"+strconv.Itoa(i)] = en
		}
	}
	return out
}

func callSortKey(en *entry, mask func(string) string) string {
	if en.value == nil {
		return ""
	}
	return sortKeyText(memberPath(en.value, "request_data.uri"), mask) + "\x00" +
		sortKeyText(memberPath(en.value, "request_data.body"), mask) + "\x00" + canonical(en.value, mask)
}

func cutLast(s, sep string) (string, string, bool) {
	i := strings.LastIndex(s, sep)
	if i < 0 {
		return s, "", false
	}
	return s[:i], s[i+len(sep):], true
}

// sortCalls orders http_out by (kind, bidder, orig_url, sha256(request body)).
func sortCalls(calls []*call, mask func(string) string) {
	keys := make(map[*call]string, len(calls))
	for _, c := range calls {
		sum := sha256.Sum256([]byte(canonical(c.reqBody, mask)))
		keys[c] = strings.Join([]string{c.src.Kind, c.src.Bidder, mask(c.origURL), hex.EncodeToString(sum[:])}, "\x00")
	}
	sort.SliceStable(calls, func(i, j int) bool { return keys[calls[i]] < keys[calls[j]] })
}

// numberView replaces generated UUIDs in a canonical traversal: stages in
// registry order, keys sorted, members sorted, arrays in normalized order.
func (e *Engine) numberView(v *view, u *uuidSet) {
	if u.count() == 0 {
		return
	}
	for _, stage := range parity.AllStages() {
		switch e.stages[stage].TraceField {
		case traceFieldStages:
			keyed := v.entries[stage]
			for _, k := range sortedEntryKeys(keyed) {
				en := keyed[k]
				u.numberNode(stage, en.value)
				u.numberNode(stage, en.errors)
				u.numberNode(stage, en.warnings)
			}
		case traceFieldHTTPIn:
			numberMessage(u, stage, v.httpIn)
		case traceFieldHTTPOut:
			for _, c := range v.httpOut {
				c.origURL = u.number(stage, c.origURL)
				c.url = u.number(stage, c.url)
				u.numberHeaders(stage, c.reqHeaders)
				u.numberNode(stage, c.reqBody)
				u.numberHeaders(stage, c.respHeaders)
				u.numberNode(stage, c.respBody)
			}
		case traceFieldAnalytics:
			u.numberNode(stage, v.analytics)
		case traceFieldHTTPResponse:
			numberMessage(u, stage, v.httpResp)
		}
	}
}

func numberMessage(u *uuidSet, stage parity.StageID, m *message) {
	if m == nil {
		return
	}
	m.path = u.number(stage, m.path)
	m.query = u.number(stage, m.query)
	u.numberHeaders(stage, m.headers)
	u.numberNode(stage, m.body)
}
