package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"testing"
	"time"

	"gopkg.in/yaml.v3"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
	"github.com/prebid/prebid-server/v4/parity/transport"
)

// The runner tests start the test binary itself as a fake parity server: with
// fakeModeEnv set, TestMain serves the parity routes instead of running tests.
const (
	fakeModeEnv = "PARITY_RUNNER_FAKE"
	fakeImplEnv = "PARITY_RUNNER_FAKE_IMPL"
	// Fake modes.
	fakeGo  = "go"  // a correct parity server
	fakeBug = "bug" // answers differently when the request body has "trigger"
	fakeNYI = "nyi" // a server without parity routes (501)
	// fakeOffContract answers like fakeGo with a trace that has a member the
	// trace schema does not know (a misspelt http_out), which decoding drops.
	fakeOffContract = "off_contract"
	// fakeTrigger is the request member that makes the bug mode diverge.
	fakeTrigger = `"trigger"`
)

func TestMain(m *testing.M) {
	if mode := os.Getenv(fakeModeEnv); mode != "" {
		os.Exit(runFakeServer(mode))
	}
	os.Exit(m.Run())
}

// fakeServer is a minimal parity server built on parity/trace.
type fakeServer struct {
	mode, impl, token, oracleURL string
	ports                        map[string]int
	oracle                       *http.Client
}

func runFakeServer(mode string) int {
	f, err := newFakeServer(mode)
	if err != nil {
		fmt.Fprintln(os.Stderr, "fake server:", err)
		return 2
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	srv := &http.Server{Addr: "127.0.0.1:" + strconv.Itoa(f.ports["port"]), Handler: f, ReadHeaderTimeout: 5 * time.Second}
	go func() {
		<-ctx.Done()
		shutdown, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_ = srv.Shutdown(shutdown)
	}()
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		fmt.Fprintln(os.Stderr, "fake server:", err)
		return 1
	}
	return 0
}

func newFakeServer(mode string) (*fakeServer, error) {
	f := &fakeServer{mode: mode, impl: os.Getenv(fakeImplEnv), token: os.Getenv("PBS_PARITY_TOKEN"), ports: map[string]int{}}
	if f.impl == "" {
		f.impl = parity.ImplGo
	}
	for key, env := range map[string]string{"port": "PBS_PORT", "admin_port": "PBS_ADMIN_PORT", "prometheus": "PBS_METRICS_PROMETHEUS_PORT"} {
		n, err := strconv.Atoi(os.Getenv(env))
		if err != nil {
			return nil, fmt.Errorf("%s: %w", env, err)
		}
		f.ports[key] = n
	}
	raw, err := os.ReadFile(ConfigFileName)
	if err != nil {
		return nil, err
	}
	var cfg struct {
		Parity struct {
			OracleURL string `yaml:"oracle_url"`
		} `yaml:"parity"`
	}
	if err := yaml.Unmarshal(raw, &cfg); err != nil {
		return nil, err
	}
	proxy, err := url.Parse(cfg.Parity.OracleURL)
	if err != nil {
		return nil, err
	}
	f.oracleURL = cfg.Parity.OracleURL
	f.oracle = &http.Client{Transport: &http.Transport{Proxy: http.ProxyURL(proxy)}, Timeout: 5 * time.Second}
	return f, nil
}

func (f *fakeServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	route, ok := strings.CutPrefix(r.URL.Path, parity.PathPrefix)
	switch {
	case !ok:
		http.NotFound(w, r)
		return
	case f.mode == fakeNYI || strings.HasPrefix(route, "adapter/"):
		writeFakeJSON(w, http.StatusNotImplemented, map[string]string{"error": "parity routes are not linked"})
		return
	case r.Header.Get(parity.TokenHeader) != f.token:
		writeFakeJSON(w, http.StatusForbidden, map[string]string{"error": "bad token"})
		return
	}
	handlers := map[string]func(*http.Request) (any, error){
		"health": f.health, "exec": f.exec, "reset": f.reset, "config": f.config, "bidders": f.bidders,
	}
	h, ok := handlers[route]
	if !ok {
		http.NotFound(w, r)
		return
	}
	v, err := h(r)
	if err != nil {
		writeFakeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	writeFakeJSON(w, http.StatusOK, v)
}

func writeFakeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func (f *fakeServer) health(*http.Request) (any, error) {
	return parity.Health{
		Schema: parity.SchemaHealth, SchemaVersion: parity.SchemaVersion, Impl: f.impl,
		Build: parity.BuildInfo{GitSHA: "fake", Version: "fake"}, StagesSupported: []parity.StageID{parity.StageRequestValidated},
		DecisionsSupported: []parity.DecisionID{}, Endpoints: []string{"/openrtb2/auction"},
		MockModes: []string{parity.MockModeOracle, parity.MockModeInline}, MockLevels: []string{parity.MockLevelHTTP},
	}, nil
}

func (f *fakeServer) reset(*http.Request) (any, error) {
	return parity.ResetResult{Schema: parity.SchemaReset, SchemaVersion: parity.SchemaVersion, Impl: f.impl, Reset: []string{}, NotReset: []parity.NotResetRow{}}, nil
}

func (f *fakeServer) config(*http.Request) (any, error) {
	cfg := map[string]any{
		"host": "127.0.0.1", "port": f.ports["port"], "admin_port": f.ports["admin_port"],
		"metrics": map[string]any{"prometheus": map[string]any{"port": f.ports["prometheus"]}},
	}
	raw, err := json.Marshal(cfg)
	return parity.ConfigDump{Schema: parity.SchemaConfigDump, SchemaVersion: parity.SchemaVersion, Impl: f.impl, Config: raw}, err
}

func (f *fakeServer) bidders(*http.Request) (any, error) {
	return parity.BiddersDump{
		Schema: parity.SchemaBiddersDump, SchemaVersion: parity.SchemaVersion, Impl: f.impl,
		Bidders: json.RawMessage(`{"appnexus":{"endpoint":"https://ib.adnxs.com/openrtb2"}}`), Active: []string{"appnexus"}, Disabled: map[string]string{},
	}, nil
}

// exec runs an envelope: one outbound call in oracle mode, one recorded
// stage and a JSON response echoing the case ID.
func (f *fakeServer) exec(r *http.Request) (any, error) {
	var env parity.Envelope
	if err := json.NewDecoder(r.Body).Decode(&env); err != nil {
		return nil, err
	}
	sess, end := trace.Begin(&env)
	defer end()
	body, err := env.Request.Body.WireBytes()
	if err != nil {
		return nil, err
	}
	sess.Record(parity.StageRequestValidated, "_", map[string]int{"body_bytes": len(body)})
	if env.Mock.EffectiveMode() == parity.MockModeOracle {
		if err := f.callOracle(r.Context(), env.CaseID, sess.NextSeq()); err != nil {
			sess.Note("oracle call: %v", err)
		}
	}
	resp := map[string]any{"id": env.CaseID}
	if f.mode == fakeBug && bytes.Contains(body, []byte(fakeTrigger)) {
		resp["bug"] = true
	}
	respBody, err := json.Marshal(resp)
	if err != nil {
		return nil, err
	}
	t := sess.Snapshot()
	t.Impl, t.Build = f.impl, parity.BuildInfo{GitSHA: "fake", Version: "fake"}
	t.HTTPIn = &parity.HTTPRequest{Method: env.Request.Method, Path: env.Request.Path, Query: env.Request.Query, Headers: env.Request.Headers, Body: env.Request.Body}
	t.HTTPResponse = &parity.HTTPResponse{Status: http.StatusOK, Headers: parity.HeaderList{{"Content-Type", "application/json"}}, Body: parity.NewBody(respBody, "")}
	if f.mode == fakeOffContract {
		return offContract(&t)
	}
	return t, nil
}

// offContract returns t as a JSON object with one member the trace schema
// does not allow.
func offContract(t *parity.Trace) (map[string]any, error) {
	raw, err := json.Marshal(t)
	if err != nil {
		return nil, err
	}
	var doc map[string]any
	if err := json.Unmarshal(raw, &doc); err != nil {
		return nil, err
	}
	doc["http_outs"] = []any{}
	return doc, nil
}

// callOracle makes one general outbound call through the oracle proxy.
func (f *fakeServer) callOracle(ctx context.Context, caseID string, seq int) error {
	const orig = "http://general.parity.test/ping"
	meta, err := transport.EncodeMeta(parity.OutboundMeta{V: transport.MetaVersion, Case: caseID, Impl: f.impl, Kind: parity.OutboundGeneral, OrigURL: orig, Seq: seq})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, orig, nil)
	if err != nil {
		return err
	}
	req.Header.Set(parity.MetaHeader, meta)
	resp, err := f.oracle.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	_, err = io.Copy(io.Discard, resp.Body)
	return err
}
