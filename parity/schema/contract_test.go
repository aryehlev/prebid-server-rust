package schema

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestGoTraceConformsToSchema drives the real Go recorders (session, inline
// transport, metrics delta) and validates the resulting trace document, so the
// Go side cannot drift from the contract the Rust side implements.
func TestGoTraceConformsToSchema(t *testing.T) {
	env := &parity.Envelope{
		Schema:        parity.SchemaEnvelope,
		SchemaVersion: parity.SchemaVersion,
		CaseID:        "contract#1",
		Request:       parity.ExecRequest{Method: http.MethodPost, Path: "/openrtb2/auction", Headers: parity.HeaderList{}},
		Mock: parity.MockSpec{
			Mode: parity.MockModeInline,
			Inline: &parity.InlineMocks{
				Bidders: map[string]parity.InlineResponse{
					"appnexus": {Status: http.StatusOK, Body: parity.TextBody(`{"id":"r1"}`)},
				},
			},
		},
		Decisions: map[string]json.RawMessage{string(parity.DecisionCategoryDedupe): json.RawMessage(`1`)},
	}
	envDoc, err := json.Marshal(env)
	require.NoError(t, err)
	require.NoError(t, Validate(envDoc), "Go-built envelope: %s", envDoc)

	registry := prometheus.NewRegistry()
	counter := prometheus.NewCounterVec(prometheus.CounterOpts{Name: "adapter_requests"}, []string{"adapter"})
	histogram := prometheus.NewHistogram(prometheus.HistogramOpts{Name: "request_time_seconds", Buckets: []float64{0.1, 1}})
	registry.MustRegister(counter, histogram)
	before, err := trace.GatherMetrics(registry)
	require.NoError(t, err)

	sess, end := trace.Begin(env)
	defer end()
	recordSampleStages(t, sess)
	sendInlineBidderCall(t)
	counter.WithLabelValues("appnexus").Inc()
	histogram.Observe(0.5)

	after, err := trace.GatherMetrics(registry)
	require.NoError(t, err)
	tr := sess.Snapshot()
	tr.Build = parity.BuildInfo{GitSHA: "test", Version: "test"}
	tr.MetricsDelta = trace.MetricsDelta(before, after)
	tr.HTTPIn = &parity.HTTPRequest{Method: http.MethodPost, Path: env.Request.Path, Headers: parity.HeaderList{}}
	tr.HTTPResponse = &parity.HTTPResponse{Status: http.StatusOK, Headers: parity.HeaderList{{"Content-Type", "application/json"}}, Body: parity.TextBody("{}\n")}

	doc, err := json.Marshal(tr)
	require.NoError(t, err)
	require.NoError(t, Validate(doc), "Go trace: %s", doc)
	require.Len(t, tr.HTTPOut, 1)
	assert.Len(t, tr.MetricsDelta, 2)
}

func recordSampleStages(t *testing.T, sess *trace.Session) {
	t.Helper()
	sess.Record(parity.StageRequestValidated, "", map[string]any{"id": "r1", "adm": "<b>"})
	sess.RecordRaw(parity.StageHTTPResponse, "", []byte{0xff, 0xfe})
	sess.Errors(parity.StageRequestValidated, "", []error{
		&errortypes.BadInput{Message: "bad"},
		&errortypes.Warning{Message: "warn", WarningCode: errortypes.InvalidPrivacyConsentWarningCode},
		errors.New("plain"),
	})
	sess.Errors(parity.StageBidderResult, "appnexus", nil)
	sess.Decide(parity.DecisionCategoryDedupe, func() int { return 0 })
	sess.Decide(parity.DecisionPriceGranularityTimesplit, func() int { return 1 })
	sess.DecideString(parity.DecisionUsersyncShuffle, func() string { return "rng" })
	sess.AddAnalytics(parity.AnalyticsAuction, map[string]any{"status": 200})
	sess.Mark(parity.StageMetricsDelta, parity.StageCaptured)
	sess.Note("contract test")
}

func sendInlineBidderCall(t *testing.T) {
	t.Helper()
	rt := transport.Wrap(nil, parity.OutboundGeneral, config.ParityConfig{})
	req, err := http.NewRequest(http.MethodPost, "https://ib.adnxs.com/openrtb2", strings.NewReader(`{"imp":[]}`))
	require.NoError(t, err)
	req = req.WithContext(trace.WithBidder(req.Context(), "appnexus"))
	resp, err := rt.RoundTrip(req)
	require.NoError(t, err)
	require.NoError(t, resp.Body.Close())
}

func TestGoMetaHeaderConformsToSchema(t *testing.T) {
	encoded, err := transport.EncodeMeta(parity.OutboundMeta{
		V: transport.MetaVersion, Case: "c", Impl: parity.ImplGo, Kind: parity.OutboundCache,
		OrigURL: "https://cache.example/cache", Seq: 3,
	})
	require.NoError(t, err)
	raw, err := base64.StdEncoding.DecodeString(encoded)
	require.NoError(t, err)
	require.NoError(t, ValidateDefinition(FileCommon, "OutboundMeta", raw))
}
