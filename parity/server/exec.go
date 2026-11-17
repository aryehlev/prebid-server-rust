package server

import (
	"net/http"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prebid/prebid-server/v4/version"
	"github.com/prometheus/client_golang/prometheus"
)

// Product endpoints the exec aliases dispatch to.
const (
	EndpointAuction = "/openrtb2/auction"
	EndpointAMP     = "/openrtb2/amp"
	EndpointVideo   = "/openrtb2/video"
)

// aliasSpec fixes the method and path of an exec alias.
type aliasSpec struct {
	name   string
	method string
	path   string
}

var (
	auctionAlias = aliasSpec{name: "auction", method: http.MethodPost, path: EndpointAuction}
	videoAlias   = aliasSpec{name: "video", method: http.MethodPost, path: EndpointVideo}
	ampAlias     = aliasSpec{name: "amp", method: http.MethodGet, path: EndpointAMP}
)

// exec serves POST /__parity/exec: one envelope through the full handler chain.
func (s *Server) exec(w http.ResponseWriter, r *http.Request) {
	env, err := readEnvelope(r, nil)
	if err != nil {
		writeFailure(w, err)
		return
	}
	s.run(w, r, env)
}

// alias serves POST /__parity/{auction,video,amp}: an envelope whose
// request.method and request.path may be omitted (the alias fixes them; given
// values must match), and whose schema, schema_version and case_id default to
// the current contract and "alias:<name>".
func (s *Server) alias(spec aliasSpec) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		env, err := readEnvelope(r, spec.apply)
		if err != nil {
			writeFailure(w, err)
			return
		}
		s.run(w, r, env)
	}
}

// ampQueryAlias serves GET /__parity/amp?<amp query>: the call's own query and
// headers (without X-Parity-Token) become the AMP request, with default mocks
// and capture. It is meant for manual use; runners post envelopes.
func (s *Server) ampQueryAlias(w http.ResponseWriter, r *http.Request) {
	header := r.Header.Clone()
	header.Del(parity.TokenHeader)
	env := &parity.Envelope{
		Request: parity.ExecRequest{
			Query:   r.URL.RawQuery,
			Host:    r.Host,
			Headers: transport.HeaderListOf(header),
		},
	}
	if err := ampAlias.apply(env); err != nil {
		writeFailure(w, err)
		return
	}
	if err := validateEnvelope(env); err != nil {
		writeFailure(w, err)
		return
	}
	s.run(w, r, env)
}

// apply fills the alias defaults and checks the fixed method and path.
func (a aliasSpec) apply(env *parity.Envelope) error {
	if env.Schema == "" {
		env.Schema = parity.SchemaEnvelope
	}
	if env.SchemaVersion == "" {
		env.SchemaVersion = parity.SchemaVersion
	}
	if env.CaseID == "" {
		env.CaseID = "alias:" + a.name
	}
	if env.Request.Method == "" {
		env.Request.Method = a.method
	}
	if env.Request.Path == "" {
		env.Request.Path = a.path
	}
	if env.Request.Method != a.method || env.Request.Path != a.path {
		return badDocument("the %s alias always sends %s %s, got %s %s", a.name, a.method, a.path, env.Request.Method, env.Request.Path)
	}
	return nil
}

// readEnvelope decodes and validates the envelope of an exec call. fill, when
// set, completes the envelope before validation (aliases).
func readEnvelope(r *http.Request, fill func(*parity.Envelope) error) (*parity.Envelope, error) {
	raw, err := readDocument(r)
	if err != nil {
		return nil, err
	}
	env := &parity.Envelope{}
	if err := decodeStrict(raw, env); err != nil {
		return nil, err
	}
	if fill != nil {
		if err := fill(env); err != nil {
			return nil, err
		}
	}
	if err := validateEnvelope(env); err != nil {
		return nil, err
	}
	return env, nil
}

// run executes one validated envelope and writes its trace. The inbound request
// is built before the exec lock is taken, so a malformed request never blocks
// other cases. A caller that gives up while waiting for the lock gets 503.
func (s *Server) run(w http.ResponseWriter, r *http.Request, env *parity.Envelope) {
	req, wire, err := buildRequest(r.Context(), env.Request)
	if err != nil {
		writeFailure(w, err)
		return
	}
	sess, end, err := trace.BeginContext(r.Context(), env)
	if err != nil {
		writeFailure(w, &errDocument{status: http.StatusServiceUnavailable, msg: err.Error()})
		return
	}
	defer end()
	writeJSON(w, http.StatusOK, s.execute(sess, req, wire))
}

// settleTimeout bounds the wait for case goroutines (trace.Session.Track) that
// still update metrics after the handler returned.
const settleTimeout = 5 * time.Second

// execute dispatches req through the handler chain while sess is current and
// assembles the trace: the recorded stages plus http_in, http_response and
// metrics_delta. A handler panic yields status 500 and a "panic: ..." note.
func (s *Server) execute(sess *trace.Session, req *http.Request, wire []byte) parity.Trace {
	env := sess.Envelope()
	metrics := beginMetrics(sess, s.deps.Gatherer)
	rec := newRecorder(req.Method == http.MethodHead)
	recovered := serveRecovering(s.dispatch, rec, req)
	resp := rec.result()
	if recovered != nil {
		sess.Note("panic: %v", recovered)
		resp = panicResponse()
	} else if rec.invalidCode != 0 {
		sess.Note("panic: invalid WriteHeader code %d", rec.invalidCode)
		resp = panicResponse()
	}
	if !sess.Settle(settleTimeout) {
		sess.Note("parity: case goroutines were still running %s after the handler returned; metrics_delta may miss their updates", settleTimeout)
	}
	deltas := metrics.end(sess)
	markCaptured(sess, parity.StageHTTPIn)
	markCaptured(sess, parity.StageHTTPResponse)

	tr := sess.Snapshot()
	tr.Build = buildInfo()
	tr.MetricsDelta = deltas
	if sess.Wants(parity.StageHTTPIn) {
		tr.HTTPIn = httpInOf(env.Request, wire)
	}
	if sess.Wants(parity.StageHTTPResponse) {
		tr.HTTPResponse = resp
	}
	return tr
}

// serveRecovering calls h and returns the value of a panic it raised, if any.
func serveRecovering(h http.Handler, w http.ResponseWriter, r *http.Request) (recovered any) {
	defer func() { recovered = recover() }()
	h.ServeHTTP(w, r)
	return nil
}

func panicResponse() *parity.HTTPResponse {
	return &parity.HTTPResponse{Status: http.StatusInternalServerError, Headers: parity.HeaderList{}}
}

// markCaptured marks a trace-level stage the exec handler fills itself.
func markCaptured(sess *trace.Session, stage parity.StageID) {
	if sess.Wants(stage) {
		sess.Mark(stage, parity.StageCaptured)
	}
}

// httpInOf is the http.in stage: the request exactly as dispatched. Headers are
// the envelope's list in its order; the body is the wire bytes, decoded for its
// json member with the request's Content-Encoding.
func httpInOf(in parity.ExecRequest, wire []byte) *parity.HTTPRequest {
	return &parity.HTTPRequest{
		Method:  in.Method,
		Path:    in.Path,
		Query:   in.Query,
		Headers: append(parity.HeaderList{}, in.Headers...),
		Body:    parity.NewBody(wire, firstValue(in.Headers, "Content-Encoding")),
	}
}

func firstValue(h parity.HeaderList, name string) string {
	if values := h.Values(name); len(values) > 0 {
		return values[0]
	}
	return ""
}

// buildInfo identifies this binary (the ldflags-populated version package).
func buildInfo() parity.BuildInfo {
	ver := version.Ver
	if ver == "" {
		ver = version.VerUnknown
	}
	return parity.BuildInfo{GitSHA: version.Rev, Version: ver}
}

// metricsCapture computes metrics_delta around one dispatch.
type metricsCapture struct {
	gatherer prometheus.Gatherer
	before   trace.MetricsSnapshot
	active   bool
}

// beginMetrics snapshots the Prometheus registry when the case captures
// metrics_delta. Without a registry (metrics.prometheus.port is 0) the stage
// stays not_reached and the trace carries a note.
func beginMetrics(sess *trace.Session, g prometheus.Gatherer) *metricsCapture {
	m := &metricsCapture{gatherer: g}
	if !sess.Wants(parity.StageMetricsDelta) || !sess.Envelope().Capture.WantsMetrics() {
		return m
	}
	before, err := trace.GatherMetrics(g)
	if err != nil {
		sess.Note("metrics_delta not captured: %v", err)
		return m
	}
	m.before, m.active = before, true
	return m
}

// end returns the changed series ([] when nothing was captured).
func (m *metricsCapture) end(sess *trace.Session) []parity.MetricDelta {
	if !m.active {
		return []parity.MetricDelta{}
	}
	after, err := trace.GatherMetrics(m.gatherer)
	if err != nil {
		sess.Note("metrics_delta not captured: %v", err)
		return []parity.MetricDelta{}
	}
	sess.Mark(parity.StageMetricsDelta, parity.StageCaptured)
	return trace.MetricsDelta(m.before, after)
}
