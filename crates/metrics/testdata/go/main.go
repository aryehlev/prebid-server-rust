// Command metricsgolden generates the differential goldens for the pbs-metrics Rust crate.
//
// It builds the real Go Prometheus metrics engine (metrics/prometheus.NewMetrics) for every
// scenario of a script, replays the scenario's scripted MetricsEngine calls, scrapes the engine
// through the same promhttp handler server/prometheus.go uses, and writes the exposition text.
// The Rust tests replay the same script and compare byte for byte.
//
// Run from the repository root:
//
//	go run ./rust/crates/metrics/testdata/go \
//	  -script rust/crates/metrics/testdata/scenarios.txt \
//	  -out rust/crates/metrics/testdata/golden
//
// Script format (one directive per line, tokens separated by single spaces, every token value
// percent-decoded with url.PathUnescape, '#' starts a comment line):
//
//	scenario <name>                start a new scenario
//	namespace <value>              config.PrometheusMetrics.Namespace
//	subsystem <value>              config.PrometheusMetrics.Subsystem
//	disabled <flag>=<bool>...      config.DisabledMetrics fields by mapstructure key
//	syncer <key>...                syncer keys passed to NewMetrics
//	module <name> <stage>...       one moduleStageNames entry
//	Record<Method> <key>=<value>...  one MetricsEngine call
//
// The Go collector (go_* families) reports Go runtime state and is removed from the output.
// Outputs written to -out:
//
//	<scenario>.prom        the scrape, reduced to the sample adapters (see sampleAdapters)
//	digests.txt            "<scenario> <length> <FNV-1a 64 hex>" of every complete scrape
//	content_type.txt       the Content-Type the promhttp handler answers with
//	core_bidder_names.txt  openrtb_ext.CoreBidderNames(), one per line
//	float_g.txt            strconv.FormatFloat(f, 'g', -1, 64) table
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

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/metrics"
	prometheusmetrics "github.com/prebid/prebid-server/v4/metrics/prometheus"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prometheus/client_golang/prometheus/promhttp"
)

type scenario struct {
	name     string
	cfg      config.PrometheusMetrics
	disabled config.DisabledMetrics
	syncers  []string
	modules  map[string][]string
	calls    []call
}

type call struct {
	line   int
	tokens []string
}

func main() {
	scriptPath := flag.String("script", "", "scenario script")
	outDir := flag.String("out", "", "output directory")
	flag.Parse()
	if *scriptPath == "" || *outDir == "" {
		log.Fatal("both -script and -out are required")
	}

	scenarios, err := parseScript(*scriptPath)
	if err != nil {
		log.Fatal(err)
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
		if err := os.WriteFile(filepath.Join(*outDir, s.name+".prom"), []byte(keepSampleAdapters(body)), 0o644); err != nil {
			log.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(*outDir, "content_type.txt"), []byte(contentType+"\n"), 0o644); err != nil {
			log.Fatal(err)
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

func run(s *scenario) (string, string, error) {
	s.cfg.Port = 8080
	m := prometheusmetrics.NewMetrics(s.cfg, s.disabled, s.syncers, s.modules)
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
