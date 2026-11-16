package parity

import "encoding/json"

// Trace is the response of POST /__parity/exec (trace.schema.json): the real
// HTTP response plus everything the server observed while producing it.
type Trace struct {
	Schema        string                            `json:"schema"`
	SchemaVersion string                            `json:"schema_version"`
	Impl          string                            `json:"impl"`
	Build         BuildInfo                         `json:"build"`
	CaseID        string                            `json:"case_id"`
	Endpoint      string                            `json:"endpoint"`
	HTTPIn        *HTTPRequest                      `json:"http_in"`
	HTTPResponse  *HTTPResponse                     `json:"http_response"`
	Stages        map[StageID]map[string]StageEntry `json:"stages"`
	StageStatus   map[StageID]StageStatus           `json:"stage_status"`
	HTTPOut       []OutboundCall                    `json:"http_out"`
	Decisions     []Decision                        `json:"decisions"`
	MetricsDelta  []MetricDelta                     `json:"metrics_delta"`
	Analytics     []AnalyticsRecord                 `json:"analytics"`
	Notes         []string                          `json:"notes"`
}

// BuildInfo identifies the binary that produced a trace.
type BuildInfo struct {
	GitSHA  string `json:"git_sha"`
	Version string `json:"version"`
}

// HTTPRequest is the inbound request as the handler chain received it (http_in).
type HTTPRequest struct {
	Method  string     `json:"method"`
	Path    string     `json:"path"`
	Query   string     `json:"query"`
	Headers HeaderList `json:"headers"`
	Body    *Body      `json:"body"`
}

// HTTPResponse is the response the handler chain wrote (http_response).
type HTTPResponse struct {
	Status  int        `json:"status"`
	Headers HeaderList `json:"headers"`
	Body    *Body      `json:"body"`
}

// StageEntry is what one implementation recorded for one (stage, key).
//
// JSON is the semantic value; Raw holds the exact bytes the implementation
// marshaled (Go: jsonutil.Marshal), so lexical differences such as HTML
// escaping or number formatting survive the outer document's re-encoding.
// Errors and Warnings are null when the stage records no error list and []
// when it recorded an empty one.
type StageEntry struct {
	Seq      int             `json:"seq"`
	Writes   int             `json:"writes"`
	JSON     json.RawMessage `json:"json"`
	Raw      *string         `json:"raw"`
	RawB64   *string         `json:"raw_b64,omitempty"`
	Errors   []ErrorRecord   `json:"errors"`
	Warnings []ErrorRecord   `json:"warnings"`
	Notes    []string        `json:"notes,omitempty"`
}

// ErrorRecord is the language-neutral form of a Go error. Code is the Go
// errortypes numeric code (error_codes.json); Kind is the canonical name of that
// code; Scope is general or debug; Fatal follows errortypes.FatalOnly.
type ErrorRecord struct {
	Code     int    `json:"code"`
	Kind     string `json:"kind"`
	Message  string `json:"message"`
	Scope    string `json:"scope"`
	Severity string `json:"severity"`
	Fatal    bool   `json:"fatal"`
}

// Error scopes and severities used in ErrorRecord.
const (
	ScopeGeneral     = "general"
	ScopeDebug       = "debug"
	SeverityFatal    = "fatal"
	SeverityWarning  = "warning"
	SeverityUnknown  = "unknown"
	ErrorKindUnknown = "Unknown"
)

// OutboundCall is one outbound HTTP exchange. In inline mode the server records
// it (Wire=false, the client-side view); in oracle mode the oracle records the
// wire-exact view (Wire=true) and the runner merges it into the trace.
type OutboundCall struct {
	Seq      int               `json:"seq"`
	Impl     string            `json:"impl,omitempty"`
	Kind     string            `json:"kind"`
	Bidder   string            `json:"bidder"`
	OrigURL  string            `json:"orig_url"`
	Request  OutboundRequest   `json:"request"`
	Wire     bool              `json:"wire"`
	Mock     *MockRef          `json:"mock"`
	Response *OutboundResponse `json:"response"`
	Error    *string           `json:"error"`
	Notes    []string          `json:"notes,omitempty"`
}

// OutboundRequest is the request half of an OutboundCall.
type OutboundRequest struct {
	Method string     `json:"method"`
	URL    string     `json:"url"`
	Proto  string     `json:"proto,omitempty"`
	Header HeaderList `json:"headers"`
	Body   *Body      `json:"body"`
}

// OutboundResponse is the response half of an OutboundCall.
type OutboundResponse struct {
	Status  int        `json:"status"`
	Headers HeaderList `json:"headers"`
	Body    *Body      `json:"body"`
	DelayMS int        `json:"delay_ms,omitempty"`
}

// MockRef says which mock answered an outbound call.
type MockRef struct {
	Source string `json:"source"`
	ID     string `json:"id"`
}

// OutboundMeta is the decoded X-Parity-Meta header (base64 of this JSON).
type OutboundMeta struct {
	V       int    `json:"v"`
	Case    string `json:"case"`
	Impl    string `json:"impl"`
	Kind    string `json:"kind"`
	Bidder  string `json:"bidder"`
	OrigURL string `json:"orig_url"`
	Seq     int    `json:"seq"`
}

// Decision records one consulted decision point.
type Decision struct {
	Point  DecisionID      `json:"point"`
	Value  json.RawMessage `json:"value"`
	Source string          `json:"source"`
	Notes  []string        `json:"notes,omitempty"`
}

// MetricDelta is the change of one Prometheus series across a case. Only series
// whose value changed are reported. For counters, gauges and untyped metrics
// Delta is the value change; for histograms and summaries Delta is the sum
// change and CountDelta/BucketDeltas carry the observation-count changes.
type MetricDelta struct {
	Name         string            `json:"name"`
	Type         string            `json:"type"`
	Labels       map[string]string `json:"labels"`
	Delta        float64           `json:"delta"`
	CountDelta   *int64            `json:"count_delta"`
	BucketDeltas map[string]int64  `json:"bucket_deltas"`
}

// Metric types used in MetricDelta.Type.
const (
	MetricCounter   = "counter"
	MetricGauge     = "gauge"
	MetricHistogram = "histogram"
	MetricSummary   = "summary"
	MetricUntyped   = "untyped"
)

// AnalyticsRecord is one analytics Log*Object call observed during the case.
type AnalyticsRecord struct {
	Kind   string          `json:"kind"`
	Object json.RawMessage `json:"object"`
}

// Analytics record kinds.
const (
	AnalyticsAuction      = "auction"
	AnalyticsAmp          = "amp"
	AnalyticsVideo        = "video"
	AnalyticsCookieSync   = "cookie_sync"
	AnalyticsSetUID       = "setuid"
	AnalyticsNotification = "notification"
)
