// Command rustgolden writes the goldens of the Rust pbs-parity crate
// (rust/crates/parity/testdata/golden) from the real parity packages of the
// parity-harness branch, and validates the documents the Rust crate produces
// (rust/crates/parity/testdata/documents) with the Go schema validator.
//
// run.sh in this directory builds it inside a temporary worktree of the
// parity-harness branch; it is not part of the Go module of this branch.
package main

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	jsoniter "github.com/json-iterator/go"
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/parity/schema"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
	"github.com/prometheus/client_golang/prometheus"
	dto "github.com/prometheus/client_model/go"
)

func main() {
	contractDir := flag.String("contract", "", "the vendored contract directory (rust/crates/parity/contract)")
	documentsDir := flag.String("documents", "", "the documents the Rust crate produced (rust/crates/parity/testdata/documents)")
	outDir := flag.String("out", "", "the golden directory to write (rust/crates/parity/testdata/golden)")
	ref := flag.String("ref", "", "the parity-harness commit the goldens are built from")
	flag.Parse()
	if *contractDir == "" || *documentsDir == "" || *outDir == "" || *ref == "" {
		fail(errors.New("-contract, -documents, -out and -ref are required"))
	}
	// The server's main.go registers this extension, so jsonutil.Marshal
	// behaves here as it does in the server.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})

	goldens := []struct {
		name  string
		build func() (any, error)
	}{
		{"contract.json", func() (any, error) { return contractGolden(*contractDir, *ref) }},
		{"ids.json", idsGolden},
		{"fakes.json", fakesGolden},
		{"sessions.json", sessionsGolden},
		{"envelopes.json", envelopesGolden},
		{"bodies.json", bodiesGolden},
		{"documents.json", documentsGolden},
		{"errors.json", errorsGolden},
		{"metrics.json", metricsGolden},
		{"meta.json", metaGolden},
		{"sessions_fuzz.json", fuzzSessionsGolden},
		{"documents_fuzz.json", fuzzDocumentsGolden},
		{"metrics_fuzz.json", fuzzMetricsGolden},
		{"bodies_fuzz.json", fuzzBodiesGolden},
		{"meta_fuzz.json", fuzzMetaGolden},
		// Installs the fakes (and uninstalls them when done), so it runs
		// after every golden that expects them uninstalled.
		{"cases.json", casesGolden},
		{"verdicts.json", func() (any, error) { return verdictsGolden(*documentsDir) }},
	}
	if err := os.MkdirAll(*outDir, 0o755); err != nil {
		fail(err)
	}
	for _, g := range goldens {
		v, err := g.build()
		if err != nil {
			fail(fmt.Errorf("%s: %w", g.name, err))
		}
		raw, err := json.MarshalIndent(map[string]any{"parity_ref": *ref, "golden": v}, "", "  ")
		if err != nil {
			fail(fmt.Errorf("%s: %w", g.name, err))
		}
		if err := os.WriteFile(filepath.Join(*outDir, g.name), append(raw, '\n'), 0o644); err != nil {
			fail(err)
		}
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "rustgolden:", err)
	os.Exit(1)
}

func sha(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}

// writeJSON is the parity server's document encoding (parity/server writeJSON).
func writeJSON(v any) (string, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	return buf.String(), nil
}

func marshalString(v any) (string, error) {
	raw, err := json.Marshal(v)
	return string(raw), err
}

// contractGolden fails unless every vendored contract file is byte-identical
// to the schema file the Go parity packages embed, and records their hashes.
func contractGolden(dir, ref string) (any, error) {
	names := []string{
		schema.FileCommon, schema.FileEnvelope, schema.FileTrace, schema.FileAdapterIO,
		schema.FileMismatch, schema.FileIndex, schema.FileIntrospection, schema.FileRegistries,
		schema.FileStages, schema.FileDecisions, schema.FileNormalization, schema.FileFakes,
		schema.FileErrorCodes,
	}
	examples, err := schema.Examples()
	if err != nil {
		return nil, err
	}
	for _, e := range examples {
		names = append(names, schema.ExamplesDir+"/"+e)
	}
	files := map[string]string{}
	for _, name := range names {
		embedded, err := schema.File(name)
		if err != nil {
			return nil, err
		}
		vendored, err := os.ReadFile(filepath.Join(dir, filepath.FromSlash(name)))
		if err != nil {
			return nil, fmt.Errorf("the vendored contract lacks %s: %w", name, err)
		}
		if !bytes.Equal(embedded, vendored) {
			return nil, fmt.Errorf("the vendored %s differs from parity-harness %s; refresh contract/", name, ref)
		}
		files[name] = sha(vendored)
	}
	var extra []string
	err = filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(path, ".json") {
			return err
		}
		rel, err := filepath.Rel(dir, path)
		if err != nil {
			return err
		}
		if _, ok := files[filepath.ToSlash(rel)]; !ok {
			extra = append(extra, rel)
		}
		return nil
	})
	if err != nil {
		return nil, err
	}
	if len(extra) > 0 {
		return nil, fmt.Errorf("vendored files parity-harness does not have: %v", extra)
	}
	return map[string]any{"files": files}, nil
}

func idsGolden() (any, error) {
	type code struct {
		Code int    `json:"code"`
		Kind string `json:"kind"`
	}
	codes := make([]code, 0)
	for c, k := range parity.ErrorCodes() {
		codes = append(codes, code{Code: c, Kind: k})
	}
	sort.Slice(codes, func(i, j int) bool { return codes[i].Code < codes[j].Code })
	domains := make([]string, 0)
	for _, d := range parity.AllDomains() {
		domains = append(domains, string(d))
	}
	return map[string]any{
		"schema_version": parity.SchemaVersion,
		"schemas": map[string]string{
			"envelope": parity.SchemaEnvelope, "trace": parity.SchemaTrace,
			"adapter_requests_input": parity.SchemaAdapterRequestsInput, "adapter_requests_output": parity.SchemaAdapterRequestsOutput,
			"adapter_bids_input": parity.SchemaAdapterBidsInput, "adapter_bids_output": parity.SchemaAdapterBidsOutput,
			"diff": parity.SchemaDiff, "index": parity.SchemaIndex, "health": parity.SchemaHealth,
			"config_dump": parity.SchemaConfigDump, "bidders_dump": parity.SchemaBiddersDump, "reset": parity.SchemaReset,
			"stages": parity.SchemaStages, "decisions": parity.SchemaDecisions, "normalization": parity.SchemaNormalization,
			"fakes": parity.SchemaFakes, "error_codes": parity.SchemaErrorCodes,
		},
		"impls":                     []string{parity.ImplGo, parity.ImplRust},
		"http":                      []string{parity.PathPrefix, parity.TokenHeader, parity.MetaHeader},
		"domains":                   domains,
		"stages":                    parity.AllStages(),
		"unkeyed_stage":             parity.UnkeyedStage,
		"stage_statuses":            []parity.StageStatus{parity.StageCaptured, parity.StageNotReached, parity.StageUnimplemented},
		"decisions":                 parity.AllDecisions(),
		"decision_sources":          []string{parity.DecisionSourceEnvelope, parity.DecisionSourceRNG},
		"usersync_shuffle_identity": parity.UsersyncShuffleIdentity,
		"outbound_kinds":            parity.AllOutboundKinds(),
		"mock_modes":                []string{parity.MockModeOracle, parity.MockModeInline},
		"mock_levels":               []string{parity.MockLevelHTTP, parity.MockLevelBidderResp},
		"mock_sources": []string{
			parity.MockSourceInline, parity.MockSourceRule, parity.MockSourceFixture,
			parity.MockSourceTemplate, parity.MockSourceDefault, parity.MockSourceStatic,
		},
		"outbound_errors": []string{
			parity.OutboundErrTimeout, parity.OutboundErrRefused, parity.OutboundErrReset,
			parity.OutboundErrNetDisabled, parity.OutboundErrOther,
		},
		"wraps":              []string{parity.WrapRaw, parity.WrapInfoAware},
		"scopes":             []string{parity.ScopeGeneral, parity.ScopeDebug},
		"severities":         []string{parity.SeverityFatal, parity.SeverityWarning, parity.SeverityUnknown},
		"error_kind_unknown": parity.ErrorKindUnknown,
		"metric_types": []string{
			parity.MetricCounter, parity.MetricGauge, parity.MetricHistogram,
			parity.MetricSummary, parity.MetricUntyped,
		},
		"analytics_kinds": []string{
			parity.AnalyticsAuction, parity.AnalyticsAmp, parity.AnalyticsVideo,
			parity.AnalyticsCookieSync, parity.AnalyticsSetUID, parity.AnalyticsNotification,
		},
		"meta_version": transport.MetaVersion,
		"error_codes":  codes,
	}, nil
}

func fakesGolden() (any, error) {
	d := fakes.Defaults()
	now := time.UnixMilli(d.NowUnixMS).UTC()
	type seqCase struct {
		Seed string `json:"seed"`
		N    uint64 `json:"n"`
		UUID string `json:"uuid"`
	}
	seqs := make([]seqCase, 0)
	for _, seed := range []string{fakes.DefaultUUID, "case-seed", "", "a", "b", "é", "x:1"} {
		for _, n := range []uint64{1, 2, 3, 10, 18446744073709551615} {
			seqs = append(seqs, seqCase{Seed: seed, N: n, UUID: fakes.SequenceUUID(seed, n).String()})
		}
	}
	sets := []*parity.FakeValues{
		nil,
		{},
		{NowUnixMS: 1000, Int63: 7, UUID: "u"},
		{Int63: 99},
		{NowUnixMS: -1},
		{Int63: -7},
		{UUID: "case-seed"},
		{NowUnixMS: 1, Int63: 9223372036854775807},
	}
	sources := make([]map[string]any, 0)
	for _, set := range sets {
		s := &fakes.Source{}
		s.Set(set)
		generated, err := s.Generate()
		if err != nil {
			return nil, err
		}
		intn := map[string]int{}
		for _, n := range []int{3, 0, -5, 7, 1, 1000} {
			intn[strconv.Itoa(n)] = s.Intn(n)
		}
		draws := make([]string, 0)
		for i := 0; i < 3; i++ {
			draws = append(draws, s.NewV4().String())
		}
		s.Set(set)
		restarted := s.NewV4().String()
		sources = append(sources, map[string]any{
			"set":           set,
			"values":        s.Values(),
			"now_unix_ms":   s.Now().UnixMilli(),
			"now_formatted": s.Now().Format("2006-01-02T15:04:05.000Z07:00"),
			"int63":         s.GenerateInt63(),
			"int":           s.Int(),
			"intn":          intn,
			"generate":      generated,
			"new_v4":        draws,
			"restarted":     restarted,
		})
	}
	return map[string]any{
		"defaults": d,
		"derived": map[string]any{
			"now_rfc3339":      now.Format("2006-01-02T15:04:05.000Z07:00"),
			"now_unix_seconds": now.Unix(),
			"now_weekday":      int(now.Weekday()),
			"now_year_day":     now.YearDay(),
		},
		"sequence": seqs,
		"sources":  sources,
	}, nil
}

// op is one step of a scripted session; the Rust test runs the same steps.
type op struct {
	Op        string          `json:"op"`
	Stage     string          `json:"stage,omitempty"`
	Key       string          `json:"key,omitempty"`
	Type      string          `json:"type,omitempty"`
	Value     json.RawMessage `json:"value,omitempty"`
	Text      string          `json:"text,omitempty"`
	RawB64    string          `json:"raw_b64,omitempty"`
	Errors    json.RawMessage `json:"errors,omitempty"`
	Status    string          `json:"status,omitempty"`
	Call      string          `json:"call,omitempty"`
	Kind      string          `json:"kind,omitempty"`
	Point     string          `json:"point,omitempty"`
	Def       json.RawMessage `json:"def,omitempty"`
	N         int             `json:"n,omitempty"`
	Offset    int             `json:"offset,omitempty"`
	Draw      bool            `json:"draw,omitempty"`
	Uncertain bool            `json:"uncertain,omitempty"`
}

// errSpec names a Go error value.
type errSpec struct {
	Type    string `json:"type"`
	Code    int    `json:"code,omitempty"`
	Message string `json:"message"`
}

func (e errSpec) build() (error, error) {
	switch e.Type {
	case "timeout":
		return &errortypes.Timeout{Message: e.Message}, nil
	case "tmax_timeout":
		return &errortypes.TmaxTimeout{Message: e.Message}, nil
	case "bad_input":
		return &errortypes.BadInput{Message: e.Message}, nil
	case "blocked_app":
		return &errortypes.BlockedApp{Message: e.Message}, nil
	case "account_disabled":
		return &errortypes.AccountDisabled{Message: e.Message}, nil
	case "acct_required":
		return &errortypes.AcctRequired{Message: e.Message}, nil
	case "bad_server_response":
		return &errortypes.BadServerResponse{Message: e.Message}, nil
	case "failed_to_request_bids":
		return &errortypes.FailedToRequestBids{Message: e.Message}, nil
	case "bidder_temporarily_disabled":
		return &errortypes.BidderTemporarilyDisabled{Message: e.Message}, nil
	case "bidder_throttled":
		return &errortypes.BidderThrottled{Message: e.Message}, nil
	case "malformed_acct":
		return &errortypes.MalformedAcct{Message: e.Message}, nil
	case "warning":
		return &errortypes.Warning{Message: e.Message, WarningCode: e.Code}, nil
	case "failed_to_unmarshal":
		return &errortypes.FailedToUnmarshal{Message: e.Message}, nil
	case "failed_to_marshal":
		return &errortypes.FailedToMarshal{Message: e.Message}, nil
	case "debug_warning":
		return &errortypes.DebugWarning{Message: e.Message, WarningCode: e.Code}, nil
	case "invalid_imp_first_party_data":
		return &errortypes.InvalidImpFirstPartyData{Message: e.Message}, nil
	case "generic":
		return errors.New(e.Message), nil
	}
	return nil, fmt.Errorf("unknown error type %q", e.Type)
}

func buildErrors(raw json.RawMessage) ([]error, error) {
	var specs []errSpec
	if err := json.Unmarshal(raw, &specs); err != nil {
		return nil, err
	}
	if specs == nil {
		return nil, nil
	}
	errs := make([]error, 0, len(specs))
	for _, spec := range specs {
		err, buildErr := spec.build()
		if buildErr != nil {
			return nil, buildErr
		}
		errs = append(errs, err)
	}
	return errs, nil
}

type regs struct {
	GDPR int             `json:"gdpr"`
	Ext  json.RawMessage `json:"ext,omitempty"`
}

type regsRecord struct {
	Regs *regs `json:"regs"`
}

// typedValue builds the Go value a record or analytics step marshals.
func typedValue(o op) (any, error) {
	switch o.Type {
	case "int":
		var v int
		return v, json.Unmarshal(o.Value, &v)
	case "float":
		var v float64
		return v, json.Unmarshal(o.Value, &v)
	case "float_bits":
		// The IEEE 754 bits in decimal, so every float64 (NaN payloads and
		// infinities included) reaches both sides exactly.
		bits, err := strconv.ParseUint(o.Text, 10, 64)
		return math.Float64frombits(bits), err
	case "bytes_string":
		// Arbitrary bytes, invalid UTF-8 included (base64 in the op).
		raw, err := base64.StdEncoding.DecodeString(o.Text)
		return string(raw), err
	case "bytes_map":
		var pairs [][2]string
		if err := json.Unmarshal(o.Value, &pairs); err != nil {
			return nil, err
		}
		m := make(map[string]string, len(pairs))
		for _, kv := range pairs {
			k, err := base64.StdEncoding.DecodeString(kv[0])
			if err != nil {
				return nil, err
			}
			v, err := base64.StdEncoding.DecodeString(kv[1])
			if err != nil {
				return nil, err
			}
			m[string(k)] = string(v)
		}
		return m, nil
	case "bytes_strings":
		var list []string
		if err := json.Unmarshal(o.Value, &list); err != nil {
			return nil, err
		}
		for i, s := range list {
			raw, err := base64.StdEncoding.DecodeString(s)
			if err != nil {
				return nil, err
			}
			list[i] = string(raw)
		}
		return list, nil
	case "nan":
		return math.NaN(), nil
	case "bool":
		var v bool
		return v, json.Unmarshal(o.Value, &v)
	case "string":
		var v string
		return v, json.Unmarshal(o.Value, &v)
	case "map":
		var v map[string]string
		return v, json.Unmarshal(o.Value, &v)
	case "strings":
		var v []string
		return v, json.Unmarshal(o.Value, &v)
	case "raw":
		return json.RawMessage(o.Text), nil
	case "regs_record":
		var v struct {
			GDPR int     `json:"gdpr"`
			Ext  *string `json:"ext"`
		}
		if err := json.Unmarshal(o.Value, &v); err != nil {
			return nil, err
		}
		r := &regs{GDPR: v.GDPR}
		if v.Ext != nil {
			r.Ext = json.RawMessage(*v.Ext)
		}
		return regsRecord{Regs: r}, nil
	case "nil_regs":
		return regsRecord{}, nil
	}
	return nil, fmt.Errorf("unknown value type %q", o.Type)
}

func runOp(s *trace.Session, o op) (any, error) {
	stage := parity.StageID(o.Stage)
	point := parity.DecisionID(o.Point)
	switch o.Op {
	case "record":
		v, err := typedValue(o)
		if err != nil {
			return nil, err
		}
		s.Record(stage, o.Key, v)
	case "record_raw":
		raw, err := base64.StdEncoding.DecodeString(o.RawB64)
		if err != nil {
			return nil, err
		}
		s.RecordRaw(stage, o.Key, raw)
	case "errors":
		errs, err := buildErrors(o.Errors)
		if err != nil {
			return nil, err
		}
		s.Errors(stage, o.Key, errs)
	case "mark":
		s.Mark(stage, parity.StageStatus(o.Status))
	case "note":
		s.Note("%s", o.Text)
	case "note_b64":
		raw, err := base64.StdEncoding.DecodeString(o.RawB64)
		if err != nil {
			return nil, err
		}
		s.Note("%s", string(raw))
	case "outbound":
		var call parity.OutboundCall
		if err := json.Unmarshal([]byte(o.Call), &call); err != nil {
			return nil, err
		}
		s.AddOutbound(call)
	case "analytics":
		v, err := typedValue(o)
		if err != nil {
			return nil, err
		}
		s.AddAnalytics(o.Kind, v)
	case "next_seq":
		return s.NextSeq(), nil
	case "wants":
		return s.Wants(stage), nil
	case "decide":
		var def int
		if err := json.Unmarshal(o.Def, &def); err != nil {
			return nil, err
		}
		return s.Decide(point, func() int { return def }), nil
	case "decide_bool":
		return s.DecideBool(point, func() bool { return o.Draw }), nil
	case "decide_chance":
		return s.DecideChance(point, o.Uncertain, func() bool { return o.Draw }), nil
	case "decide_string":
		var def string
		if err := json.Unmarshal(o.Def, &def); err != nil {
			return nil, err
		}
		return s.DecideString(point, func() string { return def }), nil
	case "intn":
		return s.Intn(point, o.N, func(n int) int { return n + o.Offset }), nil
	case "last_decision":
		if v, ok := s.LastDecision(point); ok {
			return string(v), nil
		}
		return nil, nil
	default:
		return nil, fmt.Errorf("unknown op %q", o.Op)
	}
	return nil, nil
}

func b64(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

// uEsc is the JSON escape of the code point hex (as parity/trace/session_test.go
// writes it, so the escape survives tools that rewrite escapes in sources).
func uEsc(hex string) string { return string(rune(0x5c)) + "u" + hex }

func rawJSON(s string) json.RawMessage { return json.RawMessage(s) }

func sessionScripts() []struct {
	name     string
	envelope string
	ops      []op
} {
	const auction = `{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":"%s","request":{"method":"POST","path":"/openrtb2/auction","query":"","headers":[]},"mock":{},"capture":%s%s}`
	env := func(caseID, capture, extra string) string { return fmt.Sprintf(auction, caseID, capture, extra) }
	call := `{"seq":1,"kind":"bidder","bidder":"appnexus","orig_url":"https://ib.adnxs.com/openrtb2/prebid?a=<b>&c","request":{"method":"POST","url":"http://127.0.0.1:9/appnexus","headers":[["Content-Type","application/json"],["X-Parity-Meta","e30="]],"body":{"text":"{\"id\":\"1\"}","json":{"id":"1"}}},"wire":false,"mock":{"source":"inline","id":"bidders.appnexus"},"response":{"status":200,"headers":[["Content-Type","application/json"]],"body":{"text":"{}","json":{}},"delay_ms":5},"error":null,"notes":["n"]}`
	failed := `{"seq":2,"kind":"cache","bidder":"","orig_url":"http://cache/x","request":{"method":"POST","url":"http://cache/x","headers":[],"body":null},"wire":false,"mock":null,"response":null,"error":"timeout"}`
	return []struct {
		name     string
		envelope string
		ops      []op
	}{
		{name: "empty", envelope: env("empty", `{}`, "")},
		{name: "lexical", envelope: env("lex", `{}`, ""), ops: []op{
			{Op: "record", Stage: "request.validated", Type: "map", Value: rawJSON(`{"adm":"<b>&"}`)},
			{Op: "record_raw", Stage: "http.response", RawB64: b64("{\"a\":1.0}\n")},
			{Op: "record_raw", Stage: "adapter.bids", Key: "appnexus#0", RawB64: b64("\xff")},
			{Op: "record", Stage: "bidder.request", Key: "appnexus", Type: "int", Value: rawJSON(`1`)},
			{Op: "record", Stage: "bidder.request", Key: "appnexus", Type: "int", Value: rawJSON(`2`)},
			{Op: "record", Stage: "auction.setup", Type: "string", Value: rawJSON(`"é` + uEsc("2028") + `<>&` + uEsc("0001") + `"`)},
			{Op: "record", Stage: "targeting", Type: "float", Value: rawJSON(`1e21`)},
			{Op: "record", Stage: "floors.enrich", Type: "float", Value: rawJSON(`0.000001`)},
			{Op: "record", Stage: "floors.enforce", Type: "float", Value: rawJSON(`-0.5`)},
			{Op: "record", Stage: "seat_non_bids", Type: "strings", Value: rawJSON(`null`)},
			{Op: "record", Stage: "cache.result", Type: "strings", Value: rawJSON(`[]`)},
			{Op: "record", Stage: "stored.fetch", Type: "raw", Text: `{"b": [1, 2], "c": "<"}`},
			{Op: "record", Stage: "stored.responses", Type: "raw", Text: `{"dsa":`},
			{Op: "record", Stage: "video.pods", Type: "nan"},
			{Op: "record", Stage: "category.mapping", Type: "bool", Value: rawJSON(`true`)},
			{Op: "record", Stage: "account.resolved", Type: "regs_record", Value: rawJSON(`{"gdpr":1,"ext":"{\"a\" : 1}"}`)},
			{Op: "record", Stage: "fpd.resolved", Type: "nil_regs"},
			{Op: "record_raw", Stage: "events.tracking", RawB64: ""},
			{Op: "record", Stage: "privacy.bidder", Key: "<&>", Type: "int", Value: rawJSON(`3`)},
			{Op: "next_seq"},
			{Op: "wants", Stage: "http.in"},
		}},
		{name: "errors", envelope: env("errs", `{}`, ""), ops: []op{
			{Op: "errors", Stage: "request.validated", Errors: rawJSON(`[{"type":"bad_input","message":"b"},{"type":"warning","code":10001,"message":"w"}]`)},
			{Op: "errors", Stage: "bidder.result", Key: "x", Errors: rawJSON(`[]`)},
			{Op: "errors", Stage: "bidder.result", Key: "y", Errors: rawJSON(`null`)},
			{Op: "record", Stage: "bidder.result", Key: "x", Type: "int", Value: rawJSON(`5`)},
			{Op: "errors", Stage: "privacy.bidder", Key: "appnexus", Errors: rawJSON(`[{"type":"generic","message":"plain <b>"},{"type":"debug_warning","code":10003,"message":"d"},{"type":"timeout","message":"t"},{"type":"bidder_temporarily_disabled","message":"bt"},{"type":"warning","code":42,"message":"odd code"}]`)},
			{Op: "errors", Stage: "privacy.bidder", Key: "appnexus", Errors: rawJSON(`[{"type":"bad_server_response","message":"s"}]`)},
			{Op: "record", Stage: "privacy.bidder", Key: "appnexus", Type: "int", Value: rawJSON(`1`)},
			{Op: "mark", Stage: "metrics_delta", Status: "captured"},
			{Op: "mark", Stage: "video.cache", Status: "unimplemented"},
			{Op: "mark", Stage: "privacy.bidder", Status: "not_reached"},
			{Op: "note", Text: "n1 %d <b>"},
			{Op: "note", Text: "n2"},
			{Op: "note_b64", RawB64: b64("bad \xff byte")},
		}},
		{name: "filter", envelope: env("filter", `{"stages":["http.in","made.up"],"analytics":false}`, ""), ops: []op{
			{Op: "record", Stage: "http.in", Type: "int", Value: rawJSON(`1`)},
			{Op: "record", Stage: "bidder.request", Key: "a", Type: "int", Value: rawJSON(`1`)},
			{Op: "record_raw", Stage: "bidder.request", Key: "a", RawB64: b64("1")},
			{Op: "errors", Stage: "bidder.request", Key: "a", Errors: rawJSON(`[{"type":"bad_input","message":"b"}]`)},
			{Op: "outbound", Call: call},
			{Op: "analytics", Kind: "auction", Type: "int", Value: rawJSON(`1`)},
			{Op: "mark", Stage: "bidder.request", Status: "captured"},
			{Op: "wants", Stage: "http.in"},
			{Op: "wants", Stage: "bidder.request"},
			{Op: "next_seq"},
		}},
		{name: "analytics_without_stage", envelope: env("analytics-stage", `{"stages":["analytics"],"analytics":false}`, ""), ops: []op{
			{Op: "analytics", Kind: "auction", Type: "int", Value: rawJSON(`1`)},
			{Op: "wants", Stage: "analytics"},
		}},
		{name: "bad_filter", envelope: env("bad-filter", `{"stages":"some"}`, ""), ops: []op{
			{Op: "record", Stage: "http.in", Type: "int", Value: rawJSON(`1`)},
			{Op: "wants", Stage: "bidder.request"},
		}},
		{name: "bad_filter_object", envelope: env("bad-filter-object", `{"stages":{"a":1}}`, ""), ops: []op{
			{Op: "wants", Stage: "http.in"},
		}},
		{name: "outbound_analytics", envelope: env("out", `{"stages":"*"}`, ""), ops: []op{
			{Op: "next_seq"},
			{Op: "outbound", Call: call},
			{Op: "outbound", Call: failed},
			{Op: "analytics", Kind: "auction", Type: "regs_record", Value: rawJSON(`{"gdpr":1,"ext":"{}"}`)},
			{Op: "analytics", Kind: "amp", Type: "regs_record", Value: rawJSON(`{"gdpr":1,"ext":"{\"dsa\":"}`)},
			{Op: "analytics", Kind: "video", Type: "map", Value: rawJSON(`{"<":">"}`)},
			{Op: "analytics", Kind: "cookie_sync", Type: "nan"},
			{Op: "analytics", Kind: "setuid", Type: "nil_regs"},
			{Op: "analytics", Kind: "notification", Type: "raw", Text: ` { "a" : [ 1 ] , "b" : "<&>" } `},
			{Op: "next_seq"},
		}},
		{name: "decisions", envelope: env("decide", `{}`, `,"decisions":{"floors.skip":1,"floors.data_rate":2,"floors.modelgroup":2,"floors.enforce":0,"category.dedupe":"1","pricegranularity.timesplit":null,"usersync.shuffle":"identity"}`), ops: []op{
			{Op: "decide", Point: "floors.skip", Def: rawJSON(`0`)},
			{Op: "decide", Point: "floors.data_rate", Def: rawJSON(`1`)},
			{Op: "intn", Point: "floors.modelgroup", N: 3, Offset: -3},
			{Op: "intn", Point: "floors.modelgroup", N: 2, Offset: -1},
			{Op: "decide_bool", Point: "floors.enforce", Draw: true},
			{Op: "decide", Point: "category.dedupe", Def: rawJSON(`0`)},
			{Op: "decide", Point: "pricegranularity.timesplit", Def: rawJSON(`1`)},
			{Op: "decide_string", Point: "usersync.shuffle", Def: rawJSON(`"rng"`)},
			{Op: "last_decision", Point: "floors.modelgroup"},
			{Op: "last_decision", Point: "floors.skip"},
			{Op: "last_decision", Point: "usersync.shuffle"},
		}},
		{name: "decisions_chance", envelope: env("chance", `{}`, `,"decisions":{"floors.skip":0,"usersync.shuffle":"reverse","floors.modelgroup":-1}`), ops: []op{
			{Op: "decide_chance", Point: "floors.skip", Uncertain: true, Draw: true},
			{Op: "decide_chance", Point: "floors.skip", Draw: true},
			{Op: "decide_chance", Point: "floors.enforce", Draw: false},
			{Op: "decide_chance", Point: "floors.data_rate", Uncertain: true, Draw: true},
			{Op: "decide_string", Point: "usersync.shuffle", Def: rawJSON(`"rng <&>"`)},
			{Op: "intn", Point: "floors.modelgroup", N: 5, Offset: -5},
			{Op: "decide", Point: "floors.modelgroup", Def: rawJSON(`3`)},
			{Op: "decide_string", Point: "floors.skip", Def: rawJSON(`"x"`)},
			{Op: "decide_bool", Point: "category.dedupe", Draw: false},
			{Op: "last_decision", Point: "category.dedupe"},
			{Op: "last_decision", Point: "pricegranularity.timesplit"},
		}},
	}
}

func sessionsGolden() (any, error) {
	cases := make([]map[string]any, 0)
	for _, script := range sessionScripts() {
		var env parity.Envelope
		if err := json.Unmarshal([]byte(script.envelope), &env); err != nil {
			return nil, fmt.Errorf("%s: %w", script.name, err)
		}
		s, end := trace.Begin(&env)
		results := make([]any, 0, len(script.ops))
		for _, o := range script.ops {
			r, err := runOp(s, o)
			if err != nil {
				end()
				return nil, fmt.Errorf("%s: %s: %w", script.name, o.Op, err)
			}
			results = append(results, r)
		}
		snap := s.Snapshot()
		end()
		snap.Impl = parity.ImplRust
		doc, err := writeJSON(snap)
		if err != nil {
			return nil, fmt.Errorf("%s: encoding the trace: %w", script.name, err)
		}
		if script.ops == nil {
			script.ops = []op{}
		}
		cases = append(cases, map[string]any{
			"name":     script.name,
			"envelope": script.envelope,
			"ops":      script.ops,
			"results":  results,
			"trace":    doc,
			// The exact bytes: a JSON string replaces invalid UTF-8 that
			// a json.RawMessage member passes through.
			"trace_b64": base64.StdEncoding.EncodeToString([]byte(doc)),
		})
	}
	return cases, nil
}

func envelopesGolden() (any, error) {
	base := func(extra string) string {
		return `{"schema":"pbs-parity-envelope/1","schema_version":"1.0.0","case_id":"c","request":{"method":"POST","path":"/openrtb2/auction","query":"a=1","headers":[]},"mock":{},"capture":{}` + extra + `}`
	}
	inputs := []struct{ name, input string }{
		{"minimal", base("")},
		{"capture_star", base(`,"capture":{"stages":"*"}`)},
		{"capture_null", base(`,"capture":{"stages":null}`)},
		{"capture_all_string", base(`,"capture":{"stages":"all"}`)},
		{"capture_escaped_string", base(`,"capture":{"stages":"` + uEsc("002a") + `"}`)},
		{"capture_list", base(`,"capture":{"stages":["http.in","made.up","http.in"]}`)},
		{"capture_empty_list", base(`,"capture":{"stages":[]}`)},
		{"capture_null_element", base(`,"capture":{"stages":[null,"http.in"]}`)},
		{"capture_number_list", base(`,"capture":{"stages":[1]}`)},
		{"capture_object", base(`,"capture":{"stages":{}}`)},
		{"capture_bool", base(`,"capture":{"stages":true}`)},
		{"capture_number", base(`,"capture":{"stages":5}`)},
		{"capture_flags", base(`,"capture":{"metrics":false,"analytics":true}`)},
		{"capture_flags_null", base(`,"capture":{"metrics":null,"analytics":false}`)},
		{"decisions", base(`,"decisions":{"floors.skip":1,"floors.data_rate":"1","floors.modelgroup":2.5,"floors.enforce":null,"category.dedupe":1e2,"pricegranularity.timesplit":-0,"usersync.shuffle":"identity","unknown.point":3}`)},
		{"decisions_extremes", base(`,"decisions":{"floors.modelgroup":9223372036854775807,"floors.skip":9223372036854775808,"floors.enforce":-9223372036854775808,"usersync.shuffle":"","floors.data_rate":true,"category.dedupe":[1]}`)},
		{"decisions_null", base(`,"decisions":null`)},
		{"headers", base(`,"request":{"method":"GET","path":"/p","query":"","headers":[["Content-Type","a"],["content-type","b"],["X","c"],["only"],["a","b","c"],[],null,["K",null]]}`)},
		{"headers_null", base(`,"request":{"method":"GET","path":"/p","query":"","headers":null}`)},
		{"headers_object", base(`,"request":{"method":"GET","path":"/p","query":"","headers":{}}`)},
		{"headers_pair_object", base(`,"request":{"method":"GET","path":"/p","query":"","headers":[{"a":1}]}`)},
		{"headers_pair_number", base(`,"request":{"method":"GET","path":"/p","query":"","headers":[[1,"x"]]}`)},
		{"headers_pair_string", base(`,"request":{"method":"GET","path":"/p","query":"","headers":["ab"]}`)},
		{"type_error", base(`,"case_id":5`)},
		{"nested_type_error", base(`,"mock":{"inline":{"rules":[{"response":{"status":"200"}}]}}`)},
		{"syntax_error", `{"case_id":`},
		{"trailing_garbage", base("") + ` x`},
		{"not_object", `[1]`},
		{"fakes", base(`,"fakes":{"now_unix_ms":1,"int63":2,"uuid":"x"}`)},
		{"fakes_zero", base(`,"fakes":{"now_unix_ms":0,"int63":0,"uuid":""}`)},
		{"mock_inline", base(`,"mock":{"mode":"inline","level":"http","inline":{"rules":[{"id":"r1","kind":"bidder","bidder":"appnexus","uri_regex":"^https://","response":{"status":204,"delay_ms":10}},{"response":{"fail":"timeout"}}],"bidders":{"appnexus":{"status":200,"headers":[["Content-Type","application/json"]],"body":{"json":{"id":"1"}}}},"default_bidder":{"status":204},"cache":{"status":200,"body":{"text":"{\"responses\":[]}"}},"kinds":{"currency":{"fail":"refused"},"floors":{"fail":""}}}}`)},
		{"mock_bidder_response", base(`,"mock":{"mode":"inline","level":"bidder_response","bidder_responses":{"appnexus":{"currency":"USD","bids":[{"bid":{"id":"b","impid":"i","price":1.5},"type":"banner","seat":"s","video":{"duration":5},"meta":{"x":1},"deal_priority":2}],"fledge_auction_configs":[{"a":1}],"errors":[{"code":2,"message":"m"}]},"rubicon":{"nil_response":true,"currency":"","bids":null}}}`)},
		{"body_variants", base(`,"request":{"method":"POST","path":"/p","query":"","headers":[],"body":{"text":"<a>","b64":"PGE+","json":{"a":"<"},"content_encoding":"br"},"remote_addr":"1.2.3.4:5","host":"h"}`)},
		{"unknown_fields", base(`,"extra":1,"request":{"method":"GET","path":"/p","query":"","headers":[],"extra":2}`)},
		{"case_insensitive_keys", base(`,"CASE_ID":"upper","Profile":"p"`)},
		{"duplicate_keys", base(`,"case_id":"a","case_id":"b"`)},
		{"html_in_strings", base(`,"case_id":"<&>","fixture_path":"a` + uEsc("2028") + `b"`)},
	}
	for _, name := range []string{"envelope.oracle.json", "envelope.inline.json", "envelope.bidder_response.json"} {
		raw, err := schema.Example(name)
		if err != nil {
			return nil, err
		}
		inputs = append(inputs, struct{ name, input string }{"example:" + name, string(raw)})
	}
	headerNames := []string{"content-type", "CONTENT-TYPE", "x", "missing", "only", "a", "k", ""}
	cases := make([]map[string]any, 0)
	for _, in := range inputs {
		c := map[string]any{"name": in.name, "input": in.input}
		var env parity.Envelope
		if err := json.Unmarshal([]byte(in.input), &env); err != nil {
			c["decode_error"] = err.Error()
			cases = append(cases, c)
			continue
		}
		c["decode_error"] = nil
		reencoded, err := marshalString(env)
		if err != nil {
			return nil, err
		}
		c["reencoded"] = reencoded
		filter, err := env.Capture.StageFilter()
		switch {
		case err != nil:
			c["stage_filter"] = map[string]string{"error": err.Error()}
		case filter == nil:
			c["stage_filter"] = nil
		default:
			ids := make([]string, 0, len(filter))
			for id := range filter {
				ids = append(ids, string(id))
			}
			sort.Strings(ids)
			c["stage_filter"] = ids
		}
		c["wants_metrics"] = env.Capture.WantsMetrics()
		c["wants_analytics"] = env.Capture.WantsAnalytics()
		c["effective_mode"] = env.Mock.EffectiveMode()
		c["effective_level"] = env.Mock.EffectiveLevel()
		ints := map[string]any{}
		strs := map[string]any{}
		for _, point := range parity.AllDecisions() {
			if n, ok := env.DecisionInt(point); ok {
				ints[string(point)] = n
			} else {
				ints[string(point)] = nil
			}
			if s, ok := env.DecisionString(point); ok {
				strs[string(point)] = s
			} else {
				strs[string(point)] = nil
			}
		}
		c["decision_int"] = ints
		c["decision_string"] = strs
		values := map[string][]string{}
		for _, name := range headerNames {
			v := env.Request.Headers.Values(name)
			if v == nil {
				v = []string{}
			}
			values[name] = v
		}
		c["header_values"] = values
		cases = append(cases, c)
	}
	return cases, nil
}

func gzipped(s string) []byte {
	var buf bytes.Buffer
	zw := gzip.NewWriter(&buf)
	_, _ = zw.Write([]byte(s))
	_ = zw.Close()
	return buf.Bytes()
}

func bodiesGolden() (any, error) {
	recorded := []struct {
		name string
		raw  []byte
		enc  string
	}{
		{"json", []byte(`{"a":1}`), ""},
		{"json_with_space", []byte(" {\"a\": [1, 2]}\n"), ""},
		{"text", []byte("hello <b>"), ""},
		{"invalid_utf8", []byte("\xff\xfe"), ""},
		{"invalid_utf8_json", []byte("\"\xff\""), ""},
		{"empty", []byte{}, ""},
		{"identity_spelled", []byte(`{"a":1}`), " IDENTITY "},
		{"unsupported", []byte(`{"a":1}`), "br"},
		{"upper_unsupported", []byte(`x`), "Deflate"},
		{"gzip", gzipped(`{"a":1}`), "gzip"},
		{"x_gzip", gzipped(`not json`), "X-GZIP"},
		{"gzip_corrupt", []byte(`{"a":1}`), "gzip"},
	}
	recordedCases := make([]map[string]any, 0)
	for _, r := range recorded {
		body, err := marshalString(parity.NewBody(r.raw, r.enc))
		if err != nil {
			return nil, err
		}
		recordedCases = append(recordedCases, map[string]any{
			"name": r.name, "raw_b64": base64.StdEncoding.EncodeToString(r.raw), "content_encoding": r.enc,
			"body": body, "gzip": strings.Contains(strings.ToLower(r.enc), "gzip"),
			// The exact bytes: a json member can pass invalid UTF-8 through.
			"body_b64": base64.StdEncoding.EncodeToString([]byte(body)),
		})
	}
	inputs := []struct{ name, body string }{
		{"text", `{"text":"hi <b>"}`},
		{"b64", `{"b64":"aGk="}`},
		{"b64_wins", `{"text":"ignored","b64":"aGk=","json":{"a":1}}`},
		{"b64_invalid", `{"b64":"!!"}`},
		{"b64_unpadded", `{"b64":"aGk"}`},
		{"b64_newlines", `{"b64":"aG\nk="}`},
		{"json", `{"json":{"a" : 1}}`},
		{"json_null", `{"json":null}`},
		{"none", `{}`},
		{"text_empty", `{"text":""}`},
		{"text_unsupported", `{"text":"hi","content_encoding":"br"}`},
		{"text_identity", `{"text":"hi","content_encoding":"Identity"}`},
		{"json_gzip", `{"json":{"a":1},"content_encoding":"gzip"}`},
		{"b64_gzip", `{"b64":"` + base64.StdEncoding.EncodeToString(gzipped(`{"a":1}`)) + `","content_encoding":"gzip"}`},
		{"b64_unsupported", `{"b64":"aGk=","content_encoding":"br"}`},
	}
	inputCases := make([]map[string]any, 0)
	for _, in := range inputs {
		var b parity.Body
		if err := json.Unmarshal([]byte(in.body), &b); err != nil {
			return nil, fmt.Errorf("%s: %w", in.name, err)
		}
		c := map[string]any{"name": in.name, "body": in.body, "gzip": strings.Contains(in.body, "gzip")}
		if wire, err := b.WireBytes(); err != nil {
			c["wire_error"] = err.Error()
		} else {
			c["wire_b64"] = base64.StdEncoding.EncodeToString(wire)
		}
		if decoded, err := b.DecodedBytes(); err != nil {
			c["decoded_error"] = err.Error()
		} else {
			c["decoded_b64"] = base64.StdEncoding.EncodeToString(decoded)
		}
		inputCases = append(inputCases, c)
	}
	text, err := marshalString(parity.TextBody("a<b"))
	if err != nil {
		return nil, err
	}
	return map[string]any{"recorded": recordedCases, "inputs": inputCases, "text_body": text}, nil
}

func documentsGolden() (any, error) {
	types := map[string]func() any{
		"envelope.oracle.json":          func() any { return &parity.Envelope{} },
		"envelope.inline.json":          func() any { return &parity.Envelope{} },
		"envelope.bidder_response.json": func() any { return &parity.Envelope{} },
		"trace.json":                    func() any { return &parity.Trace{} },
		"adapter_requests_input.json":   func() any { return &parity.AdapterRequestsInput{} },
		"adapter_requests_output.json":  func() any { return &parity.AdapterRequestsOutput{} },
		"adapter_bids_input.json":       func() any { return &parity.AdapterBidsInput{} },
		"adapter_bids_output.json":      func() any { return &parity.AdapterBidsOutput{} },
		"health.json":                   func() any { return &parity.Health{} },
		"config.json":                   func() any { return &parity.ConfigDump{} },
		"bidders.json":                  func() any { return &parity.BiddersDump{} },
		"reset.json":                    func() any { return &parity.ResetResult{} },
		"outbound_meta.json":            func() any { return &parity.OutboundMeta{} },
	}
	names := make([]string, 0, len(types))
	for name := range types {
		names = append(names, name)
	}
	sort.Strings(names)
	cases := make([]map[string]any, 0)
	for _, name := range names {
		raw, err := schema.Example(name)
		if err != nil {
			return nil, err
		}
		v := types[name]()
		if err := json.Unmarshal(raw, v); err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}
		encoded, err := writeJSON(v)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}
		cases = append(cases, map[string]any{"example": name, "encoded": encoded})
	}
	errorDoc, err := json.Marshal(map[string]string{"error": "parity: missing or invalid X-Parity-Token <&>"})
	if err != nil {
		return nil, err
	}
	return map[string]any{"examples": cases, "error_document": string(errorDoc)}, nil
}

func errorsGolden() (any, error) {
	specs := []errSpec{
		{Type: "timeout", Message: "t"},
		{Type: "tmax_timeout", Message: "tt"},
		{Type: "bad_input", Message: "b"},
		{Type: "blocked_app", Message: "ba"},
		{Type: "account_disabled", Message: "ad"},
		{Type: "acct_required", Message: "ar"},
		{Type: "bad_server_response", Message: "s"},
		{Type: "failed_to_request_bids", Message: "f"},
		{Type: "bidder_temporarily_disabled", Message: "btd"},
		{Type: "bidder_throttled", Message: "bth"},
		{Type: "malformed_acct", Message: "ma"},
		{Type: "warning", Code: errortypes.MultiBidWarningCode, Message: "w"},
		{Type: "warning", Code: 77, Message: "unregistered code"},
		{Type: "failed_to_unmarshal", Message: "fu"},
		{Type: "failed_to_marshal", Message: "fm"},
		{Type: "debug_warning", Code: errortypes.BidderLevelDebugDisabledWarningCode, Message: "d"},
		{Type: "invalid_imp_first_party_data", Message: "fpd"},
		{Type: "generic", Message: "plain"},
		{Type: "generic", Message: ""},
	}
	cases := make([]map[string]any, 0)
	all := make([]error, 0)
	for _, spec := range specs {
		err, buildErr := spec.build()
		if buildErr != nil {
			return nil, buildErr
		}
		all = append(all, err)
		record, encErr := marshalString(trace.ErrorRecordOf(err))
		if encErr != nil {
			return nil, encErr
		}
		cases = append(cases, map[string]any{"spec": spec, "record": record})
	}
	nilRecord, err := marshalString(trace.ErrorRecordOf(nil))
	if err != nil {
		return nil, err
	}
	fatal, warnings := trace.ErrorRecordsOf(all)
	split, err := marshalString(map[string]any{"errors": fatal, "warnings": warnings})
	if err != nil {
		return nil, err
	}
	emptyFatal, emptyWarnings := trace.ErrorRecordsOf(nil)
	empty, err := marshalString(map[string]any{"errors": emptyFatal, "warnings": emptyWarnings})
	if err != nil {
		return nil, err
	}
	return map[string]any{"cases": cases, "nil": nilRecord, "split": split, "split_empty": empty}, nil
}

// series is a neutral form of one gathered series, for the Rust test to rebuild.
type series struct {
	Name    string            `json:"name"`
	Type    string            `json:"type"`
	Labels  map[string]string `json:"labels"`
	Value   string            `json:"value"`
	Count   uint64            `json:"count"`
	Buckets []bucket          `json:"buckets"`
}

type bucket struct {
	UpperBound float64 `json:"le"`
	Count      uint64  `json:"count"`
}

func gather(reg *prometheus.Registry) ([]series, trace.MetricsSnapshot, error) {
	snap, err := trace.GatherMetrics(reg)
	if err != nil {
		return nil, snap, err
	}
	families, err := reg.Gather()
	if err != nil {
		return nil, snap, err
	}
	out := make([]series, 0)
	for _, family := range families {
		for _, m := range family.GetMetric() {
			s := series{Name: family.GetName(), Labels: map[string]string{}, Buckets: []bucket{}}
			for _, lp := range m.GetLabel() {
				s.Labels[lp.GetName()] = lp.GetValue()
			}
			var v float64
			switch family.GetType() {
			case dto.MetricType_COUNTER:
				s.Type, v = parity.MetricCounter, m.GetCounter().GetValue()
			case dto.MetricType_GAUGE:
				s.Type, v = parity.MetricGauge, m.GetGauge().GetValue()
			case dto.MetricType_HISTOGRAM:
				h := m.GetHistogram()
				s.Type, v, s.Count = parity.MetricHistogram, h.GetSampleSum(), h.GetSampleCount()
				for _, b := range h.GetBucket() {
					s.Buckets = append(s.Buckets, bucket{UpperBound: b.GetUpperBound(), Count: b.GetCumulativeCount()})
				}
			case dto.MetricType_SUMMARY:
				sum := m.GetSummary()
				s.Type, v, s.Count = parity.MetricSummary, sum.GetSampleSum(), sum.GetSampleCount()
			default:
				s.Type, v = parity.MetricUntyped, m.GetUntyped().GetValue()
			}
			s.Value = strconv.FormatFloat(v, 'g', -1, 64)
			out = append(out, s)
		}
	}
	return out, snap, nil
}

func metricsGolden() (any, error) {
	reg := prometheus.NewRegistry()
	counter := prometheus.NewCounterVec(prometheus.CounterOpts{Name: "requests"}, []string{"type", "z"})
	gauge := prometheus.NewGauge(prometheus.GaugeOpts{Name: "connections"})
	hist := prometheus.NewHistogram(prometheus.HistogramOpts{Name: "latency_seconds", Buckets: []float64{0.005, 0.1, 1, 2.5, 1e21}})
	nanHist := prometheus.NewHistogram(prometheus.HistogramOpts{Name: "nan_seconds", Buckets: []float64{1}})
	summary := prometheus.NewSummary(prometheus.SummaryOpts{Name: "sizes"})
	untypedValue := 1.5
	untyped := prometheus.NewUntypedFunc(prometheus.UntypedOpts{Name: "temperature"}, func() float64 { return untypedValue })
	unchanged := prometheus.NewCounter(prometheus.CounterOpts{Name: "unchanged"})
	reg.MustRegister(counter, gauge, hist, nanHist, summary, untyped, unchanged)
	counter.WithLabelValues("amp", "\"q\"").Add(2)
	unchanged.Inc()
	hist.Observe(0.05)
	beforeSeries, before, err := gather(reg)
	if err != nil {
		return nil, err
	}
	counter.WithLabelValues("amp", "\"q\"").Inc()
	counter.WithLabelValues("web", "").Add(3)
	gauge.Set(-2)
	hist.Observe(0.5)
	hist.Observe(5)
	nanHist.Observe(math.NaN())
	summary.Observe(7)
	untypedValue = 4
	afterSeries, after, err := gather(reg)
	if err != nil {
		return nil, err
	}
	delta, err := marshalString(trace.MetricsDelta(before, after))
	if err != nil {
		return nil, err
	}
	empty, err := marshalString(trace.MetricsDelta(trace.MetricsSnapshot{}, trace.MetricsSnapshot{}))
	if err != nil {
		return nil, err
	}
	_, nilErr := trace.GatherMetrics(nil)
	return map[string]any{
		"before":     beforeSeries,
		"after":      afterSeries,
		"delta":      delta,
		"empty":      empty,
		"nil_gather": nilErr.Error(),
	}, nil
}

func metaGolden() (any, error) {
	metas := []parity.OutboundMeta{
		{V: transport.MetaVersion, Case: "gen:42:7:drop(imp[0].banner)", Impl: parity.ImplRust, Kind: parity.OutboundBidder, Bidder: "appnexus", OrigURL: "https://ib.adnxs.com/x?a=<b>&c", Seq: 6},
		{},
	}
	encoded := make([]map[string]any, 0)
	for _, m := range metas {
		header, err := transport.EncodeMeta(m)
		if err != nil {
			return nil, err
		}
		encoded = append(encoded, map[string]any{"meta": m, "header": header})
	}
	headers := []string{
		encoded[0]["header"].(string),
		"!!",
		base64.StdEncoding.EncodeToString([]byte("not json")),
		base64.StdEncoding.EncodeToString([]byte(`{"v":2}`)),
		base64.StdEncoding.EncodeToString([]byte(`{"v":1,"seq":"x"}`)),
		base64.StdEncoding.EncodeToString([]byte(`{"v":1,"case":"c","extra":true}`)),
		"",
	}
	decoded := make([]map[string]any, 0)
	for _, h := range headers {
		c := map[string]any{"header": h}
		if m, err := transport.DecodeMeta(h); err != nil {
			c["error"] = err.Error()
		} else {
			c["meta"] = m
		}
		decoded = append(decoded, c)
	}
	return map[string]any{"encoded": encoded, "decoded": decoded}, nil
}

func verdictsGolden(dir string) (any, error) {
	verdicts := make([]map[string]any, 0)
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return verdicts, nil
	}
	if err != nil {
		return nil, err
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		doc, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			return nil, err
		}
		var verr error
		switch e.Name() {
		case "outbound_meta.json":
			verr = schema.ValidateDefinition(schema.FileCommon, "OutboundMeta", doc)
		case "error.json":
			verr = schema.ValidateDefinition(schema.FileIntrospection, "Error", doc)
		default:
			verr = schema.Validate(doc)
		}
		v := map[string]any{"file": e.Name(), "sha256": sha(doc), "error": nil}
		if verr != nil {
			v["error"] = verr.Error()
		}
		verdicts = append(verdicts, v)
	}
	return verdicts, nil
}
