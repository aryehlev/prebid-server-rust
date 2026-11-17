// Command metricsgolden generates the differential goldens for the pbs-metrics Rust crate.
//
// It builds the real Go Prometheus metrics engine (metrics/prometheus.NewMetrics) for every
// scenario of a script, replays the scenario's scripted MetricsEngine calls, scrapes the engine
// through the same promhttp handler server/prometheus.go uses, and writes the exposition text.
// The Rust tests replay the same script and compare byte for byte.
//
// Run from the repository root (the generated script is rewritten first, then every script is
// replayed):
//
//	go run ./rust/crates/metrics/testdata/go \
//	  -gen rust/crates/metrics/testdata/scenarios_generated.txt
//	go run ./rust/crates/metrics/testdata/go \
//	  -script rust/crates/metrics/testdata/scenarios.txt,rust/crates/metrics/testdata/scenarios_generated.txt \
//	  -out rust/crates/metrics/testdata/golden
//
// Script format (one directive per line, tokens separated by single spaces, every token value
// percent-decoded with url.PathUnescape, '#' starts a comment line):
//
//	scenario <name>                start a new scenario
//	digest_only                    pin only the digest of the scrape (no <scenario>.prom)
//	namespace <value>              config.PrometheusMetrics.Namespace
//	subsystem <value>              config.PrometheusMetrics.Subsystem
//	disabled <flag>=<bool>...      config.DisabledMetrics fields by mapstructure key
//	syncer <key>...                syncer keys passed to NewMetrics
//	module <name> <stage>...       one moduleStageNames entry
//	Record<Method> <key>=<value>...  one MetricsEngine call
//
// When NewMetrics panics (MustRegister rejects a family), the scrape is replaced by the line
// "panic: <message>" and no call is replayed.
//
// The Go collector (go_* families) reports Go runtime state and is removed from the output.
// Outputs written to -out:
//
//	<scenario>.prom        the scrape, reduced to the sample adapters (see sampleAdapters)
//	digests.txt            "<scenario> <length> <FNV-1a 64 hex>" of every complete scrape
//	content_type.txt       the Content-Type the promhttp handler answers with
//	core_bidder_names.txt  openrtb_ext.CoreBidderNames(), one per line
//	float_g.txt            strconv.FormatFloat(f, 'g', -1, 64) table
//	to_lower.txt           "<rune hex> <lower hex>" for every rune strings.ToLower changes
//	quote_ranges.txt       "<lo hex> <hi hex>" runs of runes strconv.Quote escapes
//	quote_samples.txt      "<rune hex> <strconv.Quote>" for sampled runes
package main

import (
	"bufio"
	"flag"
	"fmt"
	"hash/fnv"
	"log"
	"math"
	"math/rand"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/metrics"
	prometheusmetrics "github.com/prebid/prebid-server/v4/metrics/prometheus"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

type scenario struct {
	name       string
	digestOnly bool
	cfg        config.PrometheusMetrics
	disabled   config.DisabledMetrics
	syncers    []string
	modules    map[string][]string
	calls      []call
}

type call struct {
	line   int
	tokens []string
}

func main() {
	scriptPaths := flag.String("script", "", "comma-separated scenario scripts")
	outDir := flag.String("out", "", "output directory")
	genPath := flag.String("gen", "", "write the generated scenario script to this path and exit")
	flag.Parse()
	if *genPath != "" {
		if err := os.WriteFile(*genPath, []byte(generateScenarios()), 0o644); err != nil {
			log.Fatal(err)
		}
		return
	}
	if *scriptPaths == "" || *outDir == "" {
		log.Fatal("both -script and -out are required")
	}

	var scenarios []*scenario
	for _, path := range strings.Split(*scriptPaths, ",") {
		s, err := parseScript(path)
		if err != nil {
			log.Fatal(err)
		}
		scenarios = append(scenarios, s...)
	}
	if err := os.MkdirAll(*outDir, 0o755); err != nil {
		log.Fatal(err)
	}
	var digests strings.Builder
	for _, s := range scenarios {
		body, contentType, err := run(s)
		if err != nil {
			log.Fatalf("scenario %s: %v", s.name, err)
		}
		if !s.digestOnly {
			if err := os.WriteFile(filepath.Join(*outDir, s.name+".prom"), []byte(keepSampleAdapters(body)), 0o644); err != nil {
				log.Fatal(err)
			}
		}
		if contentType != "" {
			if err := os.WriteFile(filepath.Join(*outDir, "content_type.txt"), []byte(contentType+"\n"), 0o644); err != nil {
				log.Fatal(err)
			}
		}
		h := fnv.New64a()
		h.Write([]byte(body))
		fmt.Fprintf(&digests, "%s %d %016x\n", s.name, len(body), h.Sum64())
	}
	if err := os.WriteFile(filepath.Join(*outDir, "digests.txt"), []byte(digests.String()), 0o644); err != nil {
		log.Fatal(err)
	}
	if err := writeCoreBidderNames(filepath.Join(*outDir, "core_bidder_names.txt")); err != nil {
		log.Fatal(err)
	}
	if err := writeFloatTable(filepath.Join(*outDir, "float_g.txt")); err != nil {
		log.Fatal(err)
	}
	if err := writeToLowerTable(filepath.Join(*outDir, "to_lower.txt")); err != nil {
		log.Fatal(err)
	}
	if err := writeQuoteTables(filepath.Join(*outDir, "quote_ranges.txt"), filepath.Join(*outDir, "quote_samples.txt")); err != nil {
		log.Fatal(err)
	}
}

func parseScript(path string) ([]*scenario, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	var scenarios []*scenario
	var cur *scenario
	scanner := bufio.NewScanner(f)
	lineNo := 0
	for scanner.Scan() {
		lineNo++
		line := scanner.Text()
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		raw := strings.Split(line, " ")
		tokens := make([]string, len(raw))
		for i, t := range raw {
			tokens[i] = t
			if i > 0 && !strings.HasPrefix(raw[0], "Record") {
				d, err := url.PathUnescape(t)
				if err != nil {
					return nil, fmt.Errorf("line %d: %v", lineNo, err)
				}
				tokens[i] = d
			}
		}
		if tokens[0] == "scenario" {
			if len(tokens) != 2 {
				return nil, fmt.Errorf("line %d: scenario takes one name", lineNo)
			}
			cur = &scenario{name: tokens[1], modules: map[string][]string{}}
			scenarios = append(scenarios, cur)
			continue
		}
		if cur == nil {
			return nil, fmt.Errorf("line %d: directive before scenario", lineNo)
		}
		switch tokens[0] {
		case "digest_only":
			cur.digestOnly = true
		case "namespace":
			cur.cfg.Namespace = oneValue(tokens)
		case "subsystem":
			cur.cfg.Subsystem = oneValue(tokens)
		case "disabled":
			for _, kv := range tokens[1:] {
				k, v, ok := strings.Cut(kv, "=")
				if !ok {
					return nil, fmt.Errorf("line %d: bad flag %q", lineNo, kv)
				}
				b, err := strconv.ParseBool(v)
				if err != nil {
					return nil, fmt.Errorf("line %d: %v", lineNo, err)
				}
				if err := setDisabled(&cur.disabled, k, b); err != nil {
					return nil, fmt.Errorf("line %d: %v", lineNo, err)
				}
			}
		case "syncer":
			cur.syncers = append(cur.syncers, tokens[1:]...)
		case "module":
			if len(tokens) < 2 {
				return nil, fmt.Errorf("line %d: module needs a name", lineNo)
			}
			cur.modules[tokens[1]] = append([]string{}, tokens[2:]...)
		default:
			if !strings.HasPrefix(tokens[0], "Record") {
				return nil, fmt.Errorf("line %d: unknown directive %q", lineNo, tokens[0])
			}
			cur.calls = append(cur.calls, call{line: lineNo, tokens: tokens})
		}
	}
	return scenarios, scanner.Err()
}

func oneValue(tokens []string) string {
	if len(tokens) < 2 {
		return ""
	}
	return tokens[1]
}

func setDisabled(d *config.DisabledMetrics, key string, v bool) error {
	switch key {
	case "account_adapter_details":
		d.AccountAdapterDetails = v
	case "account_debug":
		d.AccountDebug = v
	case "account_stored_responses":
		d.AccountStoredResponses = v
	case "adapter_connections_metrics":
		d.AdapterConnectionMetrics = v
	case "adapter_connections_dial_metrics":
		d.AdapterConnectionDialMetrics = v
	case "adapter_buyeruid_scrubbed":
		d.AdapterBuyerUIDScrubbed = v
	case "adapter_gdpr_request_blocked":
		d.AdapterGDPRRequestBlocked = v
	case "account_modules_metrics":
		d.AccountModulesMetrics = v
	default:
		return fmt.Errorf("unknown disabled metric %q", key)
	}
	return nil
}

// newMetrics calls NewMetrics and returns the message of the panic MustRegister raises for a
// family it rejects.
func newMetrics(s *scenario) (m *prometheusmetrics.Metrics, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			panicked = fmt.Sprint(r)
		}
	}()
	return prometheusmetrics.NewMetrics(s.cfg, s.disabled, s.syncers, s.modules), ""
}

func run(s *scenario) (string, string, error) {
	s.cfg.Port = 8080
	m, panicked := newMetrics(s)
	if m == nil {
		return "panic: " + panicked + "\n", "", nil
	}
	for _, c := range s.calls {
		if err := replay(m, c); err != nil {
			return "", "", fmt.Errorf("line %d: %v", c.line, err)
		}
	}

	// Same handler options as server/prometheus.go (ErrorLog only logs).
	handler := promhttp.HandlerFor(m.Gatherer, promhttp.HandlerOpts{
		MaxRequestsInFlight: 5,
		Timeout:             10 * time.Second,
	})
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, httptest.NewRequest("GET", "/metrics", nil))
	if rec.Code != 200 {
		return "", "", fmt.Errorf("scrape returned %d", rec.Code)
	}
	prefix := ""
	if s.cfg.Namespace != "" {
		prefix += s.cfg.Namespace + "_"
	}
	if s.cfg.Subsystem != "" {
		prefix += s.cfg.Subsystem + "_"
	}
	return dropGoCollector(rec.Body.String(), prefix+"go_"), rec.Header().Get("Content-Type"), nil
}

// dropGoCollector removes every line that belongs to a family whose name starts with goPrefix.
func dropGoCollector(text, goPrefix string) string {
	var b strings.Builder
	for _, line := range strings.SplitAfter(text, "\n") {
		if line == "" {
			continue
		}
		name := line
		if strings.HasPrefix(line, "# HELP ") || strings.HasPrefix(line, "# TYPE ") {
			name = line[len("# HELP "):]
		}
		if strings.HasPrefix(name, goPrefix) {
			continue
		}
		b.WriteString(line)
	}
	return b.String()
}

// sampleAdapters are the preloaded core bidders whose series stay in the committed goldens. The
// full scrape repeats every adapter family for all core bidders (megabytes per scenario), so
// the committed .prom files keep only these core bidders plus every adapter value that is not a
// core bidder; digests.txt pins the length and FNV-1a hash of the complete scrape.
var sampleAdapters = map[string]bool{
	"33across":    true,
	"adkerneladn": true,
	"appnexus":    true,
	"pubmatic":    true,
	"rubicon":     true,
	"zmaticoo":    true,
}

func keepSampleAdapters(text string) string {
	core := map[string]bool{}
	for _, n := range openrtb_ext.CoreBidderNames() {
		core[strings.ToLower(string(n))] = true
	}
	var b strings.Builder
	for _, line := range strings.SplitAfter(text, "\n") {
		if a, ok := adapterLabel(line); ok && core[a] && !sampleAdapters[a] {
			continue
		}
		b.WriteString(line)
	}
	return b.String()
}

// adapterLabel returns the value of the adapter label of a sample line.
func adapterLabel(line string) (string, bool) {
	if strings.HasPrefix(line, "#") {
		return "", false
	}
	for _, marker := range []string{`{adapter="`, `,adapter="`} {
		if i := strings.Index(line, marker); i >= 0 {
			rest := line[i+len(marker):]
			if j := strings.IndexByte(rest, '"'); j >= 0 {
				return rest[:j], true
			}
		}
	}
	return "", false
}

type args struct {
	values map[string]string
	used   map[string]bool
	err    error
}

func parseArgs(tokens []string) (*args, error) {
	a := &args{values: map[string]string{}, used: map[string]bool{}}
	for _, t := range tokens {
		k, v, ok := strings.Cut(t, "=")
		if !ok {
			return nil, fmt.Errorf("argument %q is not key=value", t)
		}
		d, err := url.PathUnescape(v)
		if err != nil {
			return nil, err
		}
		if _, dup := a.values[k]; dup {
			return nil, fmt.Errorf("duplicate argument %q", k)
		}
		a.values[k] = d
	}
	return a, nil
}

func (a *args) str(k string) string {
	a.used[k] = true
	return a.values[k]
}

func (a *args) boolean(k string) bool {
	v := a.str(k)
	if v == "" {
		return false
	}
	b, err := strconv.ParseBool(v)
	if err != nil && a.err == nil {
		a.err = err
	}
	return b
}

func (a *args) integer(k string) int {
	v := a.str(k)
	if v == "" {
		return 0
	}
	i, err := strconv.Atoi(v)
	if err != nil && a.err == nil {
		a.err = err
	}
	return i
}

func (a *args) float(k string) float64 {
	v := a.str(k)
	if v == "" {
		return 0
	}
	f, err := strconv.ParseFloat(v, 64)
	if err != nil && a.err == nil {
		a.err = err
	}
	return f
}

func (a *args) duration(k string) time.Duration {
	v := a.str(k)
	if v == "" {
		return 0
	}
	i, err := strconv.ParseInt(v, 10, 64)
	if err != nil && a.err == nil {
		a.err = err
	}
	return time.Duration(i)
}

func (a *args) finish() error {
	if a.err != nil {
		return a.err
	}
	for k := range a.values {
		if !a.used[k] {
			return fmt.Errorf("unknown argument %q", k)
		}
	}
	return nil
}

func labels(a *args) metrics.Labels {
	return metrics.Labels{
		Source:        metrics.DemandSource(a.str("source")),
		RType:         metrics.RequestType(a.str("rtype")),
		PubID:         a.str("pub"),
		CookieFlag:    metrics.CookieFlag(a.str("cookie")),
		RequestStatus: metrics.RequestStatus(a.str("status")),
		RequestSize:   a.integer("size"),
	}
}

func adapterLabels(a *args) metrics.AdapterLabels {
	l := metrics.AdapterLabels{
		Source:      metrics.DemandSource(a.str("source")),
		RType:       metrics.RequestType(a.str("rtype")),
		Adapter:     openrtb_ext.BidderName(a.str("adapter")),
		PubID:       a.str("pub"),
		CookieFlag:  metrics.CookieFlag(a.str("cookie")),
		AdapterBids: metrics.AdapterBid(a.str("bids")),
	}
	if e := a.str("errors"); e != "" {
		l.AdapterErrors = map[metrics.AdapterError]struct{}{}
		for _, s := range strings.Split(e, ",") {
			l.AdapterErrors[metrics.AdapterError(s)] = struct{}{}
		}
	}
	return l
}

func moduleLabels(a *args) metrics.ModuleLabels {
	return metrics.ModuleLabels{Module: a.str("module"), Stage: a.str("stage"), AccountID: a.str("account")}
}

func storedDataLabels(a *args) metrics.StoredDataLabels {
	return metrics.StoredDataLabels{
		DataType:      metrics.StoredDataType(a.str("type")),
		DataFetchType: metrics.StoredDataFetchType(a.str("fetch")),
		Error:         metrics.StoredDataError(a.str("error")),
	}
}

func adapter(a *args) openrtb_ext.BidderName {
	return openrtb_ext.BidderName(a.str("adapter"))
}

func replay(m *prometheusmetrics.Metrics, c call) error {
	a, err := parseArgs(c.tokens[1:])
	if err != nil {
		return err
	}
	switch c.tokens[0] {
	case "RecordConnectionAccept":
		m.RecordConnectionAccept(a.boolean("success"))
	case "RecordTMaxTimeout":
		m.RecordTMaxTimeout()
	case "RecordConnectionClose":
		m.RecordConnectionClose(a.boolean("success"))
	case "RecordRequest":
		m.RecordRequest(labels(a))
	case "RecordImps":
		m.RecordImps(metrics.ImpLabels{
			BannerImps: a.boolean("banner"),
			VideoImps:  a.boolean("video"),
			AudioImps:  a.boolean("audio"),
			NativeImps: a.boolean("native"),
		})
	case "RecordRequestTime":
		m.RecordRequestTime(labels(a), a.duration("d"))
	case "RecordOverheadTime":
		m.RecordOverheadTime(metrics.OverheadType(a.str("type")), a.duration("d"))
	case "RecordAdapterRequest":
		m.RecordAdapterRequest(adapterLabels(a))
	case "RecordAdapterConnections":
		m.RecordAdapterConnections(adapter(a), a.boolean("reused"), a.duration("d"))
	case "RecordDNSTime":
		m.RecordDNSTime(a.duration("d"))
	case "RecordTLSHandshakeTime":
		m.RecordTLSHandshakeTime(a.duration("d"))
	case "RecordBidderServerResponseTime":
		m.RecordBidderServerResponseTime(a.duration("d"))
	case "RecordAdapterPanic":
		m.RecordAdapterPanic(adapterLabels(a))
	case "RecordAdapterBidReceived":
		l := adapterLabels(a)
		m.RecordAdapterBidReceived(l, openrtb_ext.BidType(a.str("bidtype")), a.boolean("adm"))
	case "RecordAdapterPrice":
		l := adapterLabels(a)
		m.RecordAdapterPrice(l, a.float("cpm"))
	case "RecordAdapterTime":
		l := adapterLabels(a)
		m.RecordAdapterTime(l, a.duration("d"))
	case "RecordCookieSync":
		m.RecordCookieSync(metrics.CookieSyncStatus(a.str("status")))
	case "RecordSyncerRequest":
		m.RecordSyncerRequest(a.str("key"), metrics.SyncerCookieSyncStatus(a.str("status")))
	case "RecordSetUid":
		m.RecordSetUid(metrics.SetUidStatus(a.str("status")))
	case "RecordSyncerSet":
		m.RecordSyncerSet(a.str("key"), metrics.SyncerSetUidStatus(a.str("status")))
	case "RecordStoredReqCacheResult":
		m.RecordStoredReqCacheResult(metrics.CacheResult(a.str("result")), a.integer("inc"))
	case "RecordStoredImpCacheResult":
		m.RecordStoredImpCacheResult(metrics.CacheResult(a.str("result")), a.integer("inc"))
	case "RecordAccountCacheResult":
		m.RecordAccountCacheResult(metrics.CacheResult(a.str("result")), a.integer("inc"))
	case "RecordStoredDataFetchTime":
		l := storedDataLabels(a)
		m.RecordStoredDataFetchTime(l, a.duration("d"))
	case "RecordStoredDataError":
		m.RecordStoredDataError(storedDataLabels(a))
	case "RecordFetcherResult":
		m.RecordFetcherResult(a.str("subsystem"), metrics.FetcherResult(a.str("result")))
	case "RecordFetcherBackendFetch":
		m.RecordFetcherBackendFetch(a.str("subsystem"), metrics.FetcherOperation(a.str("operation")),
			metrics.FetcherBackendResult(a.str("result")), a.duration("d"))
	case "RecordPrebidCacheRequestTime":
		m.RecordPrebidCacheRequestTime(a.boolean("success"), a.duration("d"))
	case "RecordRequestQueueTime":
		m.RecordRequestQueueTime(a.boolean("success"), metrics.RequestType(a.str("rtype")), a.duration("d"))
	case "RecordTimeoutNotice":
		m.RecordTimeoutNotice(a.boolean("success"))
	case "RecordRequestPrivacy":
		m.RecordRequestPrivacy(metrics.PrivacyLabels{
			CCPAEnforced:   a.boolean("ccpa_enforced"),
			CCPAProvided:   a.boolean("ccpa_provided"),
			COPPAEnforced:  a.boolean("coppa"),
			GDPREnforced:   a.boolean("gdpr"),
			GDPRTCFVersion: metrics.TCFVersionValue(a.str("tcf")),
			LMTEnforced:    a.boolean("lmt"),
		})
	case "RecordAdapterBuyerUIDScrubbed":
		m.RecordAdapterBuyerUIDScrubbed(adapter(a))
	case "RecordAdapterGDPRRequestBlocked":
		m.RecordAdapterGDPRRequestBlocked(adapter(a))
	case "RecordDebugRequest":
		m.RecordDebugRequest(a.boolean("enabled"), a.str("pub"))
	case "RecordStoredResponse":
		m.RecordStoredResponse(a.str("pub"))
	case "RecordGvlListRequest":
		m.RecordGvlListRequest()
	case "RecordLiveGVLFetch":
		m.RecordLiveGVLFetch(a.boolean("success"))
	case "RecordAdsCertReq":
		m.RecordAdsCertReq(a.boolean("success"))
	case "RecordAdsCertSignTime":
		m.RecordAdsCertSignTime(a.duration("d"))
	case "RecordBidValidationCreativeSizeError":
		m.RecordBidValidationCreativeSizeError(adapter(a), a.str("account"))
	case "RecordBidValidationCreativeSizeWarn":
		m.RecordBidValidationCreativeSizeWarn(adapter(a), a.str("account"))
	case "RecordBidValidationSecureMarkupError":
		m.RecordBidValidationSecureMarkupError(adapter(a), a.str("account"))
	case "RecordBidValidationSecureMarkupWarn":
		m.RecordBidValidationSecureMarkupWarn(adapter(a), a.str("account"))
	case "RecordModuleCalled":
		l := moduleLabels(a)
		m.RecordModuleCalled(l, a.duration("d"))
	case "RecordModuleFailed":
		m.RecordModuleFailed(moduleLabels(a))
	case "RecordModuleSuccessNooped":
		m.RecordModuleSuccessNooped(moduleLabels(a))
	case "RecordModuleSuccessUpdated":
		m.RecordModuleSuccessUpdated(moduleLabels(a))
	case "RecordModuleSuccessRejected":
		m.RecordModuleSuccessRejected(moduleLabels(a))
	case "RecordModuleExecutionError":
		m.RecordModuleExecutionError(moduleLabels(a))
	case "RecordModuleTimeout":
		m.RecordModuleTimeout(moduleLabels(a))
	case "RecordAdapterThrottled":
		m.RecordAdapterThrottled(adapter(a))
	case "RecordAdapterConnectionDialError":
		m.RecordAdapterConnectionDialError(adapter(a))
	case "RecordAdapterConnectionDialTime":
		m.RecordAdapterConnectionDialTime(adapter(a), a.duration("d"))
	default:
		return fmt.Errorf("unknown method %q", c.tokens[0])
	}
	return a.finish()
}

func writeCoreBidderNames(path string) error {
	var b strings.Builder
	for _, n := range openrtb_ext.CoreBidderNames() {
		b.WriteString(string(n))
		b.WriteByte('\n')
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// writeFloatTable writes "<IEEE-754 bits in hex> <strconv.FormatFloat(f, 'g', -1, 64)>" lines,
// the float formatting expfmt uses for every sample value and bucket bound.
func writeFloatTable(path string) error {
	values := []float64{
		0, math.Copysign(0, -1), 1, -1, 0.1, 0.05, 0.15, 0.2, 0.3, 0.75, 2.5, 15, 30, 250, 75000,
		1e5, 999999, 1e6, 1234567, 1e20, 1e21, 1e22, 1e-4, 1e-5, 1.5e-5, 0.00012, 5e-324,
		math.MaxFloat64, math.SmallestNonzeroFloat64, 1.0 / 3, 2.0 / 3, 0.1 + 0.2,
		9007199254740993, 18446744073709551615, 123456789012345678, -0.001, -123456.789,
		math.Inf(1), math.Inf(-1), math.NaN(),
	}
	r := rand.New(rand.NewSource(20261116))
	for i := 0; i < 1000; i++ {
		values = append(values, math.Float64frombits(r.Uint64()))
	}
	for i := 0; i < 1000; i++ {
		mantissa := float64(r.Int63n(10_000_000))
		values = append(values, mantissa*math.Pow10(r.Intn(50)-25))
	}
	for i := 0; i < 500; i++ {
		values = append(values, r.Float64()*float64(r.Intn(1_000_000)))
	}
	// Exact ties: in [2^50, 2^51) the spacing is 0.25, so n+0.25 and n+0.75 lie exactly halfway
	// between two shortest 17-digit decimals and Go rounds the last digit half to even.
	for i := 0; i < 500; i++ {
		n := float64(int64(1)<<50 + r.Int63n(int64(1)<<50))
		values = append(values, n+0.25, -(n + 0.75))
	}
	var b strings.Builder
	for _, v := range values {
		fmt.Fprintf(&b, "%016x %s\n", math.Float64bits(v), strconv.FormatFloat(v, 'g', -1, 64))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// writeToLowerTable writes every rune that strings.ToLower (unicode.ToLower, rune by rune)
// changes, with its lower case; every other rune maps to itself. The adapter label of every
// adapter family is strings.ToLower of the bidder name.
func writeToLowerTable(path string) error {
	var b strings.Builder
	for r := rune(0); r <= utf8.MaxRune; r++ {
		if !utf8.ValidRune(r) {
			continue
		}
		lower := []rune(strings.ToLower(string(r)))
		if len(lower) != 1 {
			return fmt.Errorf("strings.ToLower(%U) has %d runes", r, len(lower))
		}
		if lower[0] != r {
			fmt.Fprintf(&b, "%x %x\n", r, lower[0])
		}
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// writeQuoteTables writes the runs of runes that strconv.Quote (the %q verb of the registration
// error texts) escapes, and the exact quoting of every rune below 0x100, every run boundary and
// a random sample.
func writeQuoteTables(rangesPath, samplesPath string) error {
	escaped := func(r rune) bool { return strconv.Quote(string(r)) != `"`+string(r)+`"` }
	var runs [][2]rune
	for r := rune(0); r <= utf8.MaxRune; r++ {
		if !utf8.ValidRune(r) || !escaped(r) {
			continue
		}
		if n := len(runs); n > 0 && runs[n-1][1] == r-1 {
			runs[n-1][1] = r
		} else {
			runs = append(runs, [2]rune{r, r})
		}
	}
	var ranges, samples strings.Builder
	sample := func(r rune) {
		if utf8.ValidRune(r) {
			fmt.Fprintf(&samples, "%x %s\n", r, strconv.Quote(string(r)))
		}
	}
	for r := rune(0); r < 0x100; r++ {
		sample(r)
	}
	for _, run := range runs {
		fmt.Fprintf(&ranges, "%x %x\n", run[0], run[1])
		sample(run[0] - 1)
		sample(run[0])
		sample(run[1])
		sample(run[1] + 1)
	}
	rnd := rand.New(rand.NewSource(20261117))
	for i := 0; i < 3000; i++ {
		sample(rune(rnd.Int63n(utf8.MaxRune + 1)))
	}
	if err := os.WriteFile(rangesPath, []byte(ranges.String()), 0o644); err != nil {
		return err
	}
	return os.WriteFile(samplesPath, []byte(samples.String()), 0o644)
}

// generateScenarios returns the machine-generated script: every combination of the
// registration toggles, namespace, subsystem and module names that NewMetrics accepts or
// rejects, and a long randomized call sequence.
func generateScenarios() string {
	var b strings.Builder
	b.WriteString("# Generated by testdata/go/main.go -gen; regenerate instead of editing.\n")
	writeDisabledCombos(&b)
	writePrefixCases(&b)
	writeRandomCalls(&b, 1500)
	return b.String()
}

func esc(s string) string { return url.PathEscape(s) }

// writeDisabledCombos writes 32 scenarios: bits 0-3 toggle the four flags that decide which
// families NewMetrics registers, bit 4 and the low bits toggle the account flags that decide
// what is recorded. The dial calls are made only when Go registers the dial families (Go's only
// caller, exchange/bidder.go, makes them only then; otherwise Go dereferences nil).
func writeDisabledCombos(b *strings.Builder) {
	bit := func(i, n int) bool { return i>>n&1 == 1 }
	for i := 0; i < 32; i++ {
		conn, dial := bit(i, 0), bit(i, 1)
		fmt.Fprintf(b, "\nscenario disabled_combo_%02d\ndigest_only\nnamespace pbs\nsubsystem dc\n", i)
		fmt.Fprintf(b, "disabled adapter_connections_metrics=%t adapter_connections_dial_metrics=%t adapter_buyeruid_scrubbed=%t adapter_gdpr_request_blocked=%t account_adapter_details=%t account_debug=%t account_stored_responses=%t account_modules_metrics=%t\n",
			conn, dial, bit(i, 2), bit(i, 3), bit(i, 4), bit(i, 0) != bit(i, 4), bit(i, 1) != bit(i, 4), bit(i, 2))
		b.WriteString("syncer adnxs\nmodule mod_a entrypoint\n")
		b.WriteString(`RecordRequest rtype=openrtb2-web pub=acct status=ok cookie=no size=500
RecordDebugRequest enabled=true pub=acct
RecordDebugRequest enabled=true pub=unknown
RecordStoredResponse pub=acct
RecordStoredResponse pub=unknown
RecordAdapterConnections adapter=AppNexus reused=true d=1000
RecordAdapterConnections adapter=NotCore reused=false d=2000000
RecordAdapterBuyerUIDScrubbed adapter=AppNexus
RecordAdapterGDPRRequestBlocked adapter=PubMatic
RecordBidValidationCreativeSizeError adapter=AppNexus account=acct
RecordBidValidationCreativeSizeWarn adapter=AppNexus account=unknown
RecordBidValidationSecureMarkupError adapter=Rubicon account=acct
RecordBidValidationSecureMarkupWarn adapter=Rubicon account=acct
RecordModuleCalled module=mod_a stage=entrypoint d=1000000
`)
		if !conn && !dial {
			b.WriteString("RecordAdapterConnectionDialError adapter=AppNexus\nRecordAdapterConnectionDialTime adapter=AppNexus d=3000000000\n")
		}
	}
}

// writePrefixCases writes namespaces, subsystems and module names that make valid family names
// (pinned by digest) and names MustRegister rejects (pinned by the panic text, whose %q quoting
// escapes non-printable runes).
func writePrefixCases(b *strings.Builder) {
	type prefixCase struct {
		ns, sub string
		modules []string
	}
	cases := []prefixCase{
		{ns: "a:b", sub: "c:d"},
		{ns: "_", sub: "_"},
		{ns: ":"},
		{ns: "Ns9", sub: "1"},
		{sub: "sub_9", modules: []string{""}},
		{ns: "1x"},
		{sub: "9"},
		{ns: "x-y", sub: "sub"},
		{ns: "é"},
		{ns: "a b"},
		{ns: "q\"uote", sub: "back\\slash"},
		{ns: "tab\t"},
		{ns: "nl\n"},
		{ns: "del\x7f"},
		{ns: "c1\u0085"},
		{ns: "soft­hyphen"},
		{ns: "nbsp "},
		{ns: "unassigned͸"},
		{ns: "newᲉcase"},
		{ns: "ok", modules: []string{"bad-name"}},
		{ns: "ok", modules: []string{"fine", "bad.name"}},
		{ns: "ok", modules: []string{"é"}},
		{ns: "ok", modules: []string{"zero​width"}},
		{ns: "ok", modules: []string{"emoji\U0001f600"}},
		{ns: "ok", modules: []string{"tag\U000e0001"}},
		{ns: "ok", modules: []string{"private\U000f0000"}},
	}
	for i, c := range cases {
		fmt.Fprintf(b, "\nscenario prefix_%02d\n", i)
		if c.ns != "" {
			fmt.Fprintf(b, "namespace %s\n", esc(c.ns))
		}
		if c.sub != "" {
			fmt.Fprintf(b, "subsystem %s\n", esc(c.sub))
		}
		for _, m := range c.modules {
			fmt.Fprintf(b, "module %s stage%%20one\n", esc(m))
		}
		// A valid configuration is pinned by its digest only; a rejected one is one line.
		valid := true
		func() {
			defer func() {
				if recover() != nil {
					valid = false
				}
			}()
			modules := map[string][]string{}
			for _, m := range c.modules {
				modules[m] = []string{"stage one"}
			}
			prometheusmetrics.NewMetrics(config.PrometheusMetrics{Namespace: c.ns, Subsystem: c.sub}, config.DisabledMetrics{}, nil, modules)
		}()
		if valid {
			b.WriteString("digest_only\nRecordRequest rtype=amp pub=p status=ok\n")
			for _, m := range c.modules {
				fmt.Fprintf(b, "RecordModuleCalled module=%s stage=stage%%20one d=5\n", esc(m))
			}
		}
	}
}

// writeRandomCalls writes one scenario with n random calls spread over every method, with label
// values that need escaping or Unicode case mapping and values that exercise float formatting.
func writeRandomCalls(b *strings.Builder, n int) {
	r := rand.New(rand.NewSource(20261117))
	pick := func(values ...string) string { return values[r.Intn(len(values))] }
	adapters := []string{
		"AppNexus", "APPNEXUS", "pubmatic", "PubMatic", "Rubicon", "33ACROSS", "ZmatiCoo",
		"AdkernelADN", "NotCore", "Àccent", "Ᲊnew", "ΣIGMA", "İstanbul",
		"Straße", "Ⱟglagolitic", "q\"uote\\bidder", "new\nline", "\U00010d50garay",
	}
	accounts := []string{"acct1", "acct2", "unknown", "", "spécial", "q\"uote", "\U0001f600"}
	syncers := []string{"adnxs", "rubicon", "pubmatic", "unknown-syncer", "sp ace"}
	stages := []string{"entrypoint", "raw_auction_request", "new_stage"}
	modules := []string{"mod_a", "mod_b"}
	rtypes := []string{"", "openrtb2-web", "openrtb2-app", "openrtb2-dooh", "amp", "video"}
	statuses := []string{"", "ok", "badinput", "err", "networkerr", "blockedapp", "queuetimeout", "acctconfigerr"}
	cookies := []string{"", "exists", "no", "unknown"}
	sources := []string{"", "web", "app", "dooh", "unknown"}
	adapterErrors := []string{"badinput", "badserverresponse", "timeout", "failedtorequestbid", "validation", "tmaxtimeout", "unknown_error"}
	storedTypes := []string{"", "account", "amp", "category", "request", "video", "response"}
	boolean := func() string { return strconv.FormatBool(r.Intn(2) == 1) }
	duration := func() string {
		switch r.Intn(10) {
		case 0:
			return strconv.FormatInt(r.Int63n(400_000_000_000), 10)
		case 1:
			return strconv.FormatInt(-r.Int63n(5_000_000_000), 10)
		case 2:
			return strconv.FormatInt(r.Int63n(1000), 10)
		default:
			return strconv.FormatInt(r.Int63n(3_000_000_000), 10)
		}
	}
	cpm := func() string {
		var f float64
		switch r.Intn(6) {
		case 0:
			f = float64(r.Intn(9)+1) * 500
		case 1:
			f = r.Float64() * 1e-3
		case 2:
			f = float64(r.Intn(100000)) / 100
		default:
			f = r.Float64() * 5000
		}
		return strconv.FormatFloat(f, 'g', -1, 64)
	}
	adapterLabels := func() string {
		s := fmt.Sprintf("adapter=%s cookie=%s bids=%s source=%s rtype=%s pub=%s",
			esc(pick(adapters...)), pick(cookies...), pick("", "bid", "nobid"), pick(sources...), pick(rtypes...), esc(pick(accounts...)))
		if r.Intn(3) == 0 {
			var errs []string
			for _, e := range adapterErrors {
				if r.Intn(3) == 0 {
					errs = append(errs, e)
				}
			}
			if len(errs) > 0 {
				s += " errors=" + strings.Join(errs, ",")
			}
		}
		return s
	}
	calls := []func() string{
		func() string { return "RecordConnectionAccept success=" + boolean() },
		func() string { return "RecordConnectionClose success=" + boolean() },
		func() string { return "RecordTMaxTimeout" },
		func() string {
			return fmt.Sprintf("RecordRequest source=%s rtype=%s pub=%s cookie=%s status=%s size=%d",
				pick(sources...), pick(rtypes...), esc(pick(accounts...)), pick(cookies...), pick(statuses...), r.Intn(200000))
		},
		func() string {
			return fmt.Sprintf("RecordImps banner=%s video=%s audio=%s native=%s", boolean(), boolean(), boolean(), boolean())
		},
		func() string {
			return fmt.Sprintf("RecordRequestTime rtype=%s status=%s d=%s", pick(rtypes...), pick(statuses...), duration())
		},
		func() string {
			return fmt.Sprintf("RecordOverheadTime type=%s d=%s", pick("", "pre-bidder", "make-auction-response", "make-bidder-requests"), duration())
		},
		func() string { return "RecordAdapterRequest " + adapterLabels() },
		func() string {
			return fmt.Sprintf("RecordAdapterConnections adapter=%s reused=%s d=%s", esc(pick(adapters...)), boolean(), duration())
		},
		func() string { return "RecordDNSTime d=" + duration() },
		func() string { return "RecordTLSHandshakeTime d=" + duration() },
		func() string { return "RecordBidderServerResponseTime d=" + duration() },
		func() string { return "RecordAdapterPanic " + adapterLabels() },
		func() string {
			return fmt.Sprintf("RecordAdapterBidReceived %s bidtype=%s adm=%s", adapterLabels(), pick("banner", "video", "native", "audio"), boolean())
		},
		func() string { return fmt.Sprintf("RecordAdapterPrice %s cpm=%s", adapterLabels(), cpm()) },
		func() string { return fmt.Sprintf("RecordAdapterTime %s d=%s", adapterLabels(), duration()) },
		func() string {
			return "RecordCookieSync status=" + pick("ok", "bad_request", "opt_out", "gdpr_blocked_host_cookie", "acct_blocked", "acct_config_malformed", "acct_invalid")
		},
		func() string {
			return fmt.Sprintf("RecordSyncerRequest key=%s status=%s", esc(pick(syncers...)), pick("ok", "privacy_blocked", "already_synced", "rejected_by_filter"))
		},
		func() string {
			return "RecordSetUid status=" + pick("ok", "bad_request", "opt_out", "gdpr_blocked_host_cookie", "acct_blocked", "acct_config_malformed", "acct_invalid", "syncer_unknown")
		},
		func() string {
			return fmt.Sprintf("RecordSyncerSet key=%s status=%s", esc(pick(syncers...)), pick("ok", "cleared"))
		},
		func() string {
			return fmt.Sprintf("RecordStoredReqCacheResult result=%s inc=%d", pick("hit", "miss"), r.Intn(100000))
		},
		func() string {
			return fmt.Sprintf("RecordStoredImpCacheResult result=%s inc=%d", pick("hit", "miss"), r.Intn(100000))
		},
		func() string {
			return fmt.Sprintf("RecordAccountCacheResult result=%s inc=%d", pick("hit", "miss"), r.Intn(100000))
		},
		func() string {
			return fmt.Sprintf("RecordStoredDataFetchTime type=%s fetch=%s d=%s", pick(storedTypes...), pick("", "all", "delta"), duration())
		},
		func() string {
			return fmt.Sprintf("RecordStoredDataError type=%s error=%s", pick(storedTypes...), pick("", "network", "undefined"))
		},
		func() string {
			return fmt.Sprintf("RecordFetcherResult subsystem=%s result=%s", esc(pick("account", "stored_request", "sp ace")), pick("hit", "miss", "negative"))
		},
		func() string {
			return fmt.Sprintf("RecordFetcherBackendFetch subsystem=%s operation=%s result=%s d=%s",
				esc(pick("account", "stored_request")), pick("get", "start", "background_refresh"), pick("ok", "notfound", "error"), duration())
		},
		func() string {
			return fmt.Sprintf("RecordPrebidCacheRequestTime success=%s d=%s", boolean(), duration())
		},
		func() string {
			return fmt.Sprintf("RecordRequestQueueTime success=%s rtype=%s d=%s", boolean(), pick(rtypes...), duration())
		},
		func() string { return "RecordTimeoutNotice success=" + boolean() },
		func() string {
			return fmt.Sprintf("RecordRequestPrivacy ccpa_enforced=%s ccpa_provided=%s coppa=%s gdpr=%s tcf=%s lmt=%s",
				boolean(), boolean(), boolean(), boolean(), pick("", "err", "v2"), boolean())
		},
		func() string { return "RecordAdapterBuyerUIDScrubbed adapter=" + esc(pick(adapters...)) },
		func() string { return "RecordAdapterGDPRRequestBlocked adapter=" + esc(pick(adapters...)) },
		func() string {
			return fmt.Sprintf("RecordDebugRequest enabled=%s pub=%s", boolean(), esc(pick(accounts...)))
		},
		func() string { return "RecordStoredResponse pub=" + esc(pick(accounts...)) },
		func() string { return "RecordGvlListRequest" },
		func() string { return "RecordLiveGVLFetch success=" + boolean() },
		func() string { return "RecordAdsCertReq success=" + boolean() },
		func() string { return "RecordAdsCertSignTime d=" + duration() },
		func() string {
			return fmt.Sprintf("%s adapter=%s account=%s",
				pick("RecordBidValidationCreativeSizeError", "RecordBidValidationCreativeSizeWarn", "RecordBidValidationSecureMarkupError", "RecordBidValidationSecureMarkupWarn"),
				esc(pick(adapters...)), esc(pick(accounts...)))
		},
		func() string {
			return fmt.Sprintf("RecordModuleCalled module=%s stage=%s account=%s d=%s", pick(modules...), pick(stages...), esc(pick(accounts...)), duration())
		},
		func() string {
			return fmt.Sprintf("%s module=%s stage=%s",
				pick("RecordModuleFailed", "RecordModuleSuccessNooped", "RecordModuleSuccessUpdated", "RecordModuleSuccessRejected", "RecordModuleExecutionError", "RecordModuleTimeout"),
				pick(modules...), pick(stages...))
		},
		func() string { return "RecordAdapterThrottled adapter=" + esc(pick(adapters...)) },
		func() string { return "RecordAdapterConnectionDialError adapter=" + esc(pick(adapters...)) },
		func() string {
			return fmt.Sprintf("RecordAdapterConnectionDialTime adapter=%s d=%s", esc(pick(adapters...)), duration())
		},
	}
	b.WriteString("\nscenario random_calls\nnamespace rnd\n")
	b.WriteString("disabled account_adapter_details=false account_debug=false account_stored_responses=false adapter_connections_metrics=false adapter_connections_dial_metrics=false adapter_buyeruid_scrubbed=false adapter_gdpr_request_blocked=false\n")
	b.WriteString("syncer adnxs rubicon pubmatic sp%20ace\n")
	b.WriteString("module mod_a entrypoint raw_auction_request\nmodule mod_b entrypoint\n")
	for i := 0; i < n; i++ {
		b.WriteString(calls[r.Intn(len(calls))]())
		b.WriteByte('\n')
	}
}
