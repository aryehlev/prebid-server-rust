// Package diff is the parity harness differ: it compares what the Go server
// (the source of truth) and the Rust server observed for one case and turns
// every difference into an attributed parity.MismatchRecord.
//
// Pipeline, per side:
//
//  1. Parse every value into a tree that keeps both its semantic value and its
//     lexical form (scalar spelling, member order, whitespace). Stage entries
//     use their exact raw bytes; HTTP bodies are decoded from their wire bytes.
//  2. Normalize with the allowlist in parity/schema/normalization.json, and
//     nothing else (see Rules). Each rule cites the Go source of a
//     nondeterminism; everything the allowlist does not cover is a mismatch.
//
// Then both sides are compared:
//
//   - semantically: objects by member name, numbers by value, strings by
//     content, arrays by position after LCS alignment (array order is
//     significant unless a rule sorted the array);
//   - lexically, on semantically equal values both sides produced as exact
//     bytes: escaping, number spelling, member order, whitespace and the
//     trailing newline (kind lexical, never normalized);
//   - headers: names case-insensitively with value order kept (header_names),
//     plus, for wire captures, name spelling, name order, framing and client
//     defaults (kind wire, never normalized);
//   - errors (error_code / error_text), stage presence (stage_missing /
//     stage_extra), decisions, metric deltas and outbound calls.
//
// Finally every difference gets a domain and requirement (stage registry,
// refined by JSON path), a signature for clustering, and primary or
// consequential marking: the lowest-ordered diverging stage is primary, later
// differences are consequential of it.
//
// JSON paths are relative to the compared value: $.imp[0].ext for a stage
// value or an HTTP body, $[2].request.headers['User-Agent'] for http_out.
// Parts of a stage entry or HTTP message outside the value use '@' members:
// $['@errors'][0].code, $['@status'], $['@headers']['Content-Type'].
package diff

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// Engine compares parity documents against the stage registry and the
// normalization allowlist. It is safe for concurrent use.
type Engine struct {
	stages  map[parity.StageID]schema.StageSpec
	domains map[parity.Domain]schema.DomainSpec
	owner   map[string]parity.Domain
	rules   *Rules
}

// New returns an engine over the embedded stages.json and normalization.json.
func New() (*Engine, error) {
	stages, err := schema.Stages()
	if err != nil {
		return nil, fmt.Errorf("parity diff: loading stage registry: %w", err)
	}
	rules, err := LoadRules()
	if err != nil {
		return nil, err
	}
	return NewEngine(stages, rules)
}

// NewEngine returns an engine over an explicit stage registry and rule set.
func NewEngine(stages schema.StageRegistry, rules *Rules) (*Engine, error) {
	if rules == nil {
		return nil, errors.New("parity diff: no normalization rules")
	}
	e := &Engine{
		stages:  make(map[parity.StageID]schema.StageSpec, len(stages.Stages)),
		domains: make(map[parity.Domain]schema.DomainSpec, len(stages.Domains)),
		owner:   map[string]parity.Domain{},
		rules:   rules,
	}
	for _, d := range stages.Domains {
		e.domains[d.ID] = d
		for _, req := range d.Requirements {
			e.owner[req] = d.ID
		}
	}
	for _, s := range stages.Stages {
		e.stages[s.ID] = s
	}
	for _, id := range parity.AllStages() {
		if _, ok := e.stages[id]; !ok {
			return nil, fmt.Errorf("parity diff: stage registry lacks stage %q", id)
		}
	}
	return e, nil
}

// Rules returns the active normalization allowlist.
func (e *Engine) Rules() *Rules { return e.rules }

// Case identifies the compared case and what its caller supplied.
type Case struct {
	CaseID      string
	FixturePath string
	Endpoint    string
	Profile     string
	// Envelope is the input both servers received. Values it contains are
	// never treated as server-generated UUIDs.
	Envelope *parity.Envelope
	// KnownInputs is any other caller-supplied data the servers read during
	// the case (stored requests, stored responses, account files). UUIDs in
	// it are never treated as generated either.
	KnownInputs [][]byte
}

// Result is the comparison of one case.
type Result struct {
	// Doc is the schema document written as diff.json.
	Doc parity.DiffDocument
	// Notes are diagnostics about the inputs (unparseable values, entries
	// without raw bytes); they are not mismatches.
	Notes []string
}

// Primary returns the primary mismatches of the result.
func (r Result) Primary() []parity.MismatchRecord {
	var out []parity.MismatchRecord
	for _, m := range r.Doc.Mismatches {
		if m.Primary {
			out = append(out, m)
		}
	}
	return out
}

// CompareTraces compares the Go trace (reference) with the Rust trace of one
// case. For an A/A self-test pass two Go traces.
func (e *Engine) CompareTraces(c Case, goTrace, rustTrace *parity.Trace) (Result, error) {
	if goTrace == nil || rustTrace == nil {
		return Result{}, errors.New("parity diff: both traces are required")
	}
	if err := checkDocument(goTrace.Schema, goTrace.SchemaVersion, parity.SchemaTrace); err != nil {
		return Result{}, fmt.Errorf("parity diff: go trace: %w", err)
	}
	if err := checkDocument(rustTrace.Schema, rustTrace.SchemaVersion, parity.SchemaTrace); err != nil {
		return Result{}, fmt.Errorf("parity diff: rust trace: %w", err)
	}
	if c.Endpoint == "" {
		c.Endpoint = endpointOf(c, goTrace)
	}
	inputs := caseInputs(c)
	gv, rv := buildView(goTrace, parity.ImplGo), buildView(rustTrace, parity.ImplRust)
	e.normalize(gv, newUUIDSet(e.rules, inputs))
	e.normalize(rv, newUUIDSet(e.rules, inputs))
	findings := e.compareViews(gv, rv)
	return Result{Doc: e.document(c, findings), Notes: append(gv.notes, rv.notes...)}, nil
}

// CompareTraceJSON decodes two trace documents and compares them.
func (e *Engine) CompareTraceJSON(c Case, goDoc, rustDoc []byte) (Result, error) {
	var g, r parity.Trace
	if err := json.Unmarshal(goDoc, &g); err != nil {
		return Result{}, fmt.Errorf("parity diff: decoding go trace: %w", err)
	}
	if err := json.Unmarshal(rustDoc, &r); err != nil {
		return Result{}, fmt.Errorf("parity diff: decoding rust trace: %w", err)
	}
	return e.CompareTraces(c, &g, &r)
}

// EncodeDiff serializes a diff document as diff.json: indented, with a
// trailing newline and without HTML escaping, so that the go and rust values
// of lexical records (for example "\u0026" against "&") read as recorded.
func EncodeDiff(doc parity.DiffDocument) ([]byte, error) {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(doc); err != nil {
		return nil, fmt.Errorf("parity diff: encoding diff document %q: %w", doc.CaseID, err)
	}
	return b.Bytes(), nil
}

// checkDocument rejects documents of another type or an unknown major version.
func checkDocument(name, version, want string) error {
	if name != want {
		return fmt.Errorf("schema %q, want %q", name, want)
	}
	major, _, _ := strings.Cut(parity.SchemaVersion, ".")
	if got, _, _ := strings.Cut(version, "."); version != "" && got != major {
		return fmt.Errorf("schema_version %q has an unknown major version (want %s.x.y)", version, major)
	}
	return nil
}

func endpointOf(c Case, t *parity.Trace) string {
	switch {
	case c.Envelope != nil && c.Envelope.Request.Path != "":
		return c.Envelope.Request.Path
	case t.Endpoint != "":
		return t.Endpoint
	case t.HTTPIn != nil:
		return t.HTTPIn.Path
	}
	return ""
}

// caseInputs is the caller-supplied text of a case.
func caseInputs(c Case) string {
	var b bytes.Buffer
	if c.Envelope != nil {
		if raw, err := json.Marshal(c.Envelope); err == nil {
			b.Write(raw)
		}
		if body, err := c.Envelope.Request.Body.DecodedBytes(); err == nil {
			b.WriteByte('\n')
			b.Write(body)
		}
	}
	for _, in := range c.KnownInputs {
		b.WriteByte('\n')
		b.Write(in)
	}
	return b.String()
}

// compareViews compares two normalized views stage by stage in registry order.
func (e *Engine) compareViews(g, r *view) []finding {
	var out []finding
	for _, stage := range parity.AllStages() {
		spec := e.stages[stage]
		if spec.TraceField == traceFieldIntrospection {
			continue
		}
		gs, rs := g.effectiveStatus(stage, spec.TraceField), r.effectiveStatus(stage, spec.TraceField)
		if f, decided := stageStatusFinding(stage, gs, rs); decided {
			if f != nil {
				out = append(out, *f)
			}
			continue
		}
		out = append(out, e.compareStage(stage, spec.TraceField, g, r)...)
	}
	return append(out, e.compareDecisions(g.decisions, r.decisions)...)
}

// stageStatusFinding decides a stage from its statuses. It returns decided =
// false when both sides captured the stage and its content must be compared.
func stageStatusFinding(stage parity.StageID, gs, rs parity.StageStatus) (*finding, bool) {
	statusJSON := func(s parity.StageStatus) json.RawMessage { return json.RawMessage(jsonQuote(string(s))) }
	switch {
	case gs == parity.StageCaptured && rs == parity.StageCaptured:
		return nil, false
	case gs == parity.StageCaptured:
		return &finding{
			stage: stage, key: parity.UnkeyedStage, kind: parity.KindStageMissing, path: Root,
			goVal: statusJSON(gs), rustVal: statusJSON(rs), instrumentation: rs == parity.StageUnimplemented,
		}, true
	case rs == parity.StageCaptured:
		return &finding{
			stage: stage, key: parity.UnkeyedStage, kind: parity.KindStageExtra, path: Root,
			goVal: statusJSON(gs), rustVal: statusJSON(rs),
		}, true
	}
	return nil, true
}

func (e *Engine) compareStage(stage parity.StageID, field string, g, r *view) []finding {
	c := e.newComparer(stage, parity.UnkeyedStage)
	switch field {
	case traceFieldHTTPIn:
		e.compareMessage(c, g.httpIn, r.httpIn, false)
	case traceFieldHTTPResponse:
		e.compareMessage(c, g.httpResp, r.httpResp, true)
	case traceFieldHTTPOut:
		e.compareCalls(c, g.httpOut, r.httpOut)
	case traceFieldAnalytics:
		c.value(Root, g.analytics, r.analytics, fieldPlain)
	case traceFieldMetrics:
		e.compareMetrics(c, g.metrics, r.metrics)
	default:
		return e.compareEntries(stage, g.entries[stage], r.entries[stage])
	}
	return c.findings
}

func (e *Engine) newComparer(stage parity.StageID, key string) *comparer {
	return &comparer{stage: stage, key: key, memberOrderSemantic: !e.rules.Active(RuleJSONMemberOrder, stage)}
}

// compareEntries compares the keyed entries of one stage.
func (e *Engine) compareEntries(stage parity.StageID, g, r map[string]*entry) []finding {
	keys := map[string]bool{}
	for k := range g {
		keys[k] = true
	}
	for k := range r {
		keys[k] = true
	}
	var out []finding
	for _, key := range sortedSet(keys) {
		c := e.newComparer(stage, key)
		ge, re := g[key], r[key]
		switch {
		case re == nil:
			c.add(parity.KindMissingInRust, "", Root, render(ge.value), json.RawMessage("null"))
		case ge == nil:
			c.add(parity.KindExtraInRust, "", Root, json.RawMessage("null"), render(re.value))
		default:
			e.compareEntry(c, stage, ge, re)
		}
		out = append(out, c.findings...)
	}
	return out
}

func (e *Engine) compareEntry(c *comparer, stage parity.StageID, g, r *entry) {
	c.document(Root, g.value, r.value)
	c.wsReported = false
	c.value(meta(Root, MetaErrors), g.errors, r.errors, fieldPlain)
	c.value(meta(Root, MetaWarnings), g.warnings, r.warnings, fieldPlain)
	if g.writes != r.writes {
		c.add(parity.KindValue, "", meta(Root, MetaWrites), valueJSON(g.writes), valueJSON(r.writes))
	}
	if !e.rules.Active(RuleStageEntrySeq, stage) && g.seq != r.seq {
		c.add(parity.KindValue, "", meta(Root, MetaSeq), valueJSON(g.seq), valueJSON(r.seq))
	}
}

// compareMessage compares http_in (request) or http_response.
func (e *Engine) compareMessage(c *comparer, g, r *message, response bool) {
	switch {
	case g == nil && r == nil:
		return
	case r == nil:
		c.add(parity.KindMissingInRust, "", Root, json.RawMessage(`"captured"`), json.RawMessage("null"))
		return
	case g == nil:
		c.add(parity.KindExtraInRust, "", Root, json.RawMessage("null"), json.RawMessage(`"captured"`))
		return
	}
	if response && g.status != r.status {
		c.add(parity.KindStatus, "", meta(Root, MetaStatus), valueJSON(g.status), valueJSON(r.status))
	}
	if !response {
		for _, f := range []struct{ name, g, r string }{
			{MetaMethod, g.method, r.method}, {MetaPath, g.path, r.path}, {MetaQuery, g.query, r.query},
		} {
			if f.g != f.r {
				c.add(parity.KindValue, "", meta(Root, f.name), json.RawMessage(jsonQuote(f.g)), json.RawMessage(jsonQuote(f.r)))
			}
		}
	}
	c.headers(meta(Root, MetaHeaders), g.headers, r.headers, e.headerOptions(c.stage, response, response, !response))
	c.bodyDocument(Root, g.body, r.body)
}

// headerOptions builds the header comparison mode of a stage.
func (e *Engine) headerOptions(stage parity.StageID, response, wire, request bool) headerOptions {
	opt := headerOptions{caseInsensitive: e.rules.Active(RuleHeaderNames, stage), wire: wire, request: request}
	if response && e.rules.Active(RuleDateHeader, stage) {
		opt.ignore = map[string]bool{"date": true}
	}
	return opt
}

// compareCalls aligns http_out calls by (kind, bidder, orig_url) and compares
// each pair.
func (e *Engine) compareCalls(c *comparer, g, r []*call) {
	gk, rk := callIdentities(g), callIdentities(r)
	for _, op := range align(gk, rk) {
		switch {
		case op.goIndex >= 0 && op.rustIndex >= 0:
			e.compareCall(c, index(Root, op.goIndex), g[op.goIndex], r[op.rustIndex])
		case op.goIndex >= 0:
			c.outbound = g[op.goIndex].src.Kind
			c.add(parity.KindMissingInRust, "", index(Root, op.goIndex), callSummary(g[op.goIndex]), json.RawMessage("null"))
		default:
			c.outbound = r[op.rustIndex].src.Kind
			c.add(parity.KindExtraInRust, "", index(Root, op.rustIndex), json.RawMessage("null"), callSummary(r[op.rustIndex]))
		}
	}
	c.outbound = ""
	e.unexpectedOutbound(c, g, r)
}

func callIdentities(calls []*call) []string {
	out := make([]string, len(calls))
	for i, c := range calls {
		out[i] = c.src.Kind + "\x00" + c.src.Bidder + "\x00" + c.origURL
	}
	return out
}

func callSummary(c *call) json.RawMessage {
	return valueJSON(map[string]string{"kind": c.src.Kind, "bidder": c.src.Bidder, "orig_url": c.origURL, "method": c.src.Request.Method})
}

func (e *Engine) compareCall(c *comparer, base string, g, r *call) {
	c.outbound = g.src.Kind
	scalar := func(name, gv, rv string) {
		if gv != rv {
			c.add(parity.KindValue, "", name, json.RawMessage(jsonQuote(gv)), json.RawMessage(jsonQuote(rv)))
		}
	}
	scalar(child(base, "kind"), g.src.Kind, r.src.Kind)
	scalar(child(base, "bidder"), g.src.Bidder, r.src.Bidder)
	scalar(child(base, "orig_url"), g.origURL, r.origURL)
	if !e.rules.Active(RuleHTTPOutOrder, parity.StageHTTPOut) && g.src.Seq != r.src.Seq {
		c.add(parity.KindValue, "", child(base, "seq"), valueJSON(g.src.Seq), valueJSON(r.src.Seq))
	}
	wire := g.src.Wire && r.src.Wire
	if g.src.Wire != r.src.Wire {
		c.add(parity.KindWire, parity.WireOther, child(base, "wire"), valueJSON(g.src.Wire), valueJSON(r.src.Wire))
	}
	req := child(base, "request")
	scalar(child(req, "method"), g.src.Request.Method, r.src.Request.Method)
	scalar(child(req, "url"), g.url, r.url)
	if wire && g.src.Request.Proto != r.src.Request.Proto {
		c.add(parity.KindWire, parity.WireHTTPVersion, child(req, "proto"),
			json.RawMessage(jsonQuote(g.src.Request.Proto)), json.RawMessage(jsonQuote(r.src.Request.Proto)))
	}
	c.headers(child(req, "headers"), g.reqHeaders, r.reqHeaders, e.headerOptions(parity.StageHTTPOut, false, wire, true))
	c.bodyDocument(child(req, "body"), g.reqBody, r.reqBody)
	if gm, rm := valueJSON(g.src.Mock), valueJSON(r.src.Mock); !bytes.Equal(gm, rm) {
		c.add(parity.KindValue, "", child(base, "mock"), gm, rm)
	}
	e.compareCallResponse(c, child(base, "response"), g, r, wire)
	compareCallError(c, child(base, "error"), g.src.Error, r.src.Error)
}

func (e *Engine) compareCallResponse(c *comparer, path string, g, r *call, wire bool) {
	gr, rr := g.src.Response, r.src.Response
	switch {
	case gr == nil && rr == nil:
		return
	case rr == nil:
		c.add(parity.KindMissingInRust, "", path, valueJSON(gr), json.RawMessage("null"))
		return
	case gr == nil:
		c.add(parity.KindExtraInRust, "", path, json.RawMessage("null"), valueJSON(rr))
		return
	}
	if gr.Status != rr.Status {
		c.add(parity.KindValue, "", child(path, "status"), valueJSON(gr.Status), valueJSON(rr.Status))
	}
	if gr.DelayMS != rr.DelayMS {
		c.add(parity.KindValue, "", child(path, "delay_ms"), valueJSON(gr.DelayMS), valueJSON(rr.DelayMS))
	}
	c.headers(child(path, "headers"), g.respHeaders, r.respHeaders, e.headerOptions(parity.StageHTTPOut, true, wire, false))
	c.bodyDocument(child(path, "body"), g.respBody, r.respBody)
}

// compareCallError reports a categorical timeout difference as kind timing
// and any other error difference as a value.
func compareCallError(c *comparer, path string, g, r *string) {
	gv, rv := derefOr(g, ""), derefOr(r, "")
	if gv == rv && (g == nil) == (r == nil) {
		return
	}
	kind := parity.KindValue
	if (gv == parity.OutboundErrTimeout) != (rv == parity.OutboundErrTimeout) {
		kind = parity.KindTiming
	}
	c.add(kind, "", path, valueJSON(g), valueJSON(r))
}

func derefOr(s *string, def string) string {
	if s == nil {
		return def
	}
	return *s
}

// unexpectedOutbound reports wire calls the oracle answered without any mock.
func (e *Engine) unexpectedOutbound(c *comparer, g, r []*call) {
	for _, side := range []struct {
		calls  []*call
		goSide bool
	}{{g, true}, {r, false}} {
		for i, call := range side.calls {
			if !call.src.Wire || call.src.Mock != nil {
				continue
			}
			c.outbound = call.src.Kind
			summary := callSummary(call)
			if side.goSide {
				c.add(parity.KindUnexpectedOutbound, "", index(Root, i), summary, json.RawMessage("null"))
			} else {
				c.add(parity.KindUnexpectedOutbound, "", index(Root, i), json.RawMessage("null"), summary)
			}
		}
	}
	c.outbound = ""
}

// compareMetrics compares metric deltas by (name, labels). Timing histograms
// compare their observation count only (timing_histograms rule).
func (e *Engine) compareMetrics(c *comparer, g, r []parity.MetricDelta) {
	countOnly := e.rules.Active(RuleTimingHistograms, parity.StageMetricsDelta)
	gm, rm := metricProjections(g, countOnly), metricProjections(r, countOnly)
	keys := map[string]bool{}
	for k := range gm {
		keys[k] = true
	}
	for k := range rm {
		keys[k] = true
	}
	for _, k := range sortedSet(keys) {
		gp, rp := gm[k], rm[k]
		if gp != nil && rp != nil && bytes.Equal(gp.json, rp.json) {
			continue
		}
		name := ""
		gj, rj := json.RawMessage("null"), json.RawMessage("null")
		if gp != nil {
			name, gj = gp.name, gp.json
		}
		if rp != nil {
			name, rj = rp.name, rp.json
		}
		c.add(parity.KindMetric, "", child(Root, name), gj, rj)
	}
}

// withoutMetrics drops the series whose name drop reports from deltas.
func withoutMetrics(deltas []parity.MetricDelta, drop func(name string) bool) []parity.MetricDelta {
	out := make([]parity.MetricDelta, 0, len(deltas))
	for _, d := range deltas {
		if !drop(d.Name) {
			out = append(out, d)
		}
	}
	return out
}

type metricProjection struct {
	name string
	json json.RawMessage
}

func metricProjections(deltas []parity.MetricDelta, countOnly bool) map[string]*metricProjection {
	out := make(map[string]*metricProjection, len(deltas))
	for _, d := range deltas {
		p := map[string]any{"name": d.Name, "type": d.Type, "labels": d.Labels}
		if countOnly && isTimingMetric(d.Name, d.Type) {
			p["count_delta"] = d.CountDelta
		} else {
			p["delta"], p["count_delta"], p["bucket_deltas"] = d.Delta, d.CountDelta, d.BucketDeltas
		}
		out[metricKey(d)] = &metricProjection{name: d.Name, json: valueJSON(p)}
	}
	return out
}

func metricKey(d parity.MetricDelta) string {
	names := make([]string, 0, len(d.Labels))
	for k := range d.Labels {
		names = append(names, k)
	}
	sort.Strings(names)
	var b strings.Builder
	b.WriteString(d.Name)
	for _, k := range names {
		b.WriteString("\x00" + k + "=" + d.Labels[k])
	}
	return b.String()
}

// decisionStages maps each decision point to the stage it belongs to.
var decisionStages = map[parity.DecisionID]parity.StageID{
	parity.DecisionFloorsSkip:                parity.StageFloorsEnrich,
	parity.DecisionFloorsDataRate:            parity.StageFloorsEnrich,
	parity.DecisionFloorsModelGroup:          parity.StageFloorsEnrich,
	parity.DecisionFloorsEnforce:             parity.StageFloorsEnforce,
	parity.DecisionCategoryDedupe:            parity.StageCategoryMapping,
	parity.DecisionPriceGranularityTimesplit: parity.StageAuctionWinners,
	parity.DecisionUsersyncShuffle:           parity.StageHTTPResponse,
}

// compareDecisions compares, per decision point, the multiset of consulted
// (value, source) pairs, values by JSON value. A point consulted on one side
// only, a different count, value or source is kind decision.
func (e *Engine) compareDecisions(g, r []parity.Decision) []finding {
	gm, rm := groupDecisions(g), groupDecisions(r)
	points := map[string]bool{}
	for p := range gm {
		points[p] = true
	}
	for p := range rm {
		points[p] = true
	}
	var out []finding
	for _, point := range sortedSet(points) {
		gs, rs := gm[point], rm[point]
		if equalStrings(decisionKeys(gs), decisionKeys(rs)) {
			continue
		}
		stage, ok := decisionStages[parity.DecisionID(point)]
		if !ok {
			stage = parity.StageHTTPResponse
		}
		out = append(out, finding{
			stage: stage, key: parity.UnkeyedStage, kind: parity.KindDecision,
			path: child(Root, point), goVal: decisionList(gs), rustVal: decisionList(rs),
		})
	}
	return out
}

// consultation is one decision consulted: its comparison key and its display.
type consultation struct {
	key    string
	Value  json.RawMessage `json:"value"`
	Source string          `json:"source"`
}

func groupDecisions(list []parity.Decision) map[string][]consultation {
	out := map[string][]consultation{}
	for _, d := range list {
		value, key := json.RawMessage("null"), "null"
		if n, err := parseSemantic(d.Value); err == nil {
			value, key = render(n), canonical(n, nil)
		}
		point := string(d.Point)
		out[point] = append(out[point], consultation{key: key + "\x00" + d.Source, Value: value, Source: d.Source})
	}
	return out
}

// decisionKeys returns the sorted comparison keys of a point's consultations.
func decisionKeys(list []consultation) []string {
	keys := make([]string, len(list))
	for i, c := range list {
		keys[i] = c.key
	}
	sort.Strings(keys)
	return keys
}

// decisionList renders a point's consultations, or null when it was not consulted.
func decisionList(list []consultation) json.RawMessage {
	if list == nil {
		return json.RawMessage("null")
	}
	return valueJSON(list)
}

func sortedSet(set map[string]bool) []string {
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
