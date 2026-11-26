package main

// The go-metrics goldens: the registry the go-metrics engine (metrics/go_metrics.go) holds after
// every scripted scenario, the log lines it writes, and the arithmetic of the go-metrics library
// (github.com/rcrowley/go-metrics cf1acfcdf475) and of the InfluxDB line protocol
// (github.com/influxdata/influxdb1-client 8bf82d3c094d) on inputs whose results do not depend on
// the clock or on math/rand.
//
// Outputs written to the -gometrics directory (every field is strconv.Quote'd unless noted, and
// fields are tab-separated; the rows of registry.txt and dumps.txt, which digests.txt hashes, are
// strconv.QuoteToASCII'd, whose escapes need no Unicode tables to reproduce):
//
//	exchanges.txt     the adapter list the scripted scenarios pass to NewMetrics, one per line
//	registry.txt      the complete dump of startup_viper_defaults: every name NewMetrics registers
//	dumps.txt         for every scenario, a "scenario" <name> row, then its log rows, then the
//	                  registry rows of the metrics it recorded into (count not 0)
//	digests.txt       "<scenario> <length> <FNV-1a 64 hex>" of every complete dump (unquoted);
//	                  <scenario>@core is startup_viper_defaults over all core bidders
//	ewma.txt          EWMA alphas and the rates after scripted Update/Tick sequences
//	samples.txt       the Sample* statistics of splitmix64-generated value slices
//	points.txt        client.Point.MarshalString for crafted and random points
//	truncate.txt      time.Time.Truncate for random times and durations (unquoted)
//	wire.txt          influxdb1-client Write and Ping against a fake endpoint (see influx.go)
//	redirects.txt     the same, redirected to a second endpoint on another port (see influx.go)
//	reporter.txt      the InfluxDB reporter's batches and log lines (see influx.go)
//	keys.txt, paths.txt, errorf.txt, urls.txt
//	                  the escaping, path, error-text and URL edge cases (see influxedge.go)
//
// A dump row is "log" <script line> <glog output with the header reduced to the severity>, or
// <metric name> <kind> <values>. Timers and histograms are read the way the InfluxDB
// reporter reads them: a snapshot, its percentiles (which sort the snapshot), then the other
// statistics, so every value is independent of the order in which the reservoir holds values.
// Meter rates depend on the clock and are not dumped.

import (
	"errors"
	"flag"
	"fmt"
	"hash/fnv"
	"io"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"time"

	client "github.com/influxdata/influxdb1-client"
	"github.com/influxdata/influxdb1-client/models"
	"github.com/prebid/prebid-server/v4/metrics"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	gometrics "github.com/rcrowley/go-metrics"
)

// reporterPercentiles are the percentiles the InfluxDB reporter sends.
var reporterPercentiles = []float64{0.5, 0.75, 0.95, 0.99, 0.999, 0.9999}

// goMetricsExchanges is the adapter list the scripted scenarios pass to NewMetrics: the sample
// core bidders in CoreBidderNames order, then the other adapters the scripts record (so that
// their price, timing and connection paths run), a duplicate after lower-casing, and names whose
// strings.ToLower is not ASCII. startup_viper_defaults is also run with all core bidders.
func goMetricsExchanges() []openrtb_ext.BidderName {
	var list []openrtb_ext.BidderName
	for _, n := range openrtb_ext.CoreBidderNames() {
		if sampleAdapters[strings.ToLower(string(n))] {
			list = append(list, n)
		}
	}
	for _, n := range []string{
		"AnyName", "TinyPrice", "negprice", "BigPrice", "neginfprice", "nanprice", "infprice",
		"Dialer", "AppNexus", "Àccent", "ΣIGMA", "İstanbul", "Ᲊnew",
	} {
		list = append(list, openrtb_ext.BidderName(n))
	}
	return list
}

func runGoMetrics(scenarios []*scenario, outDir string) error {
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return err
	}
	// The engine logs through glog; with -logtostderr glog writes each line to os.Stderr, which
	// the capture swaps for a temporary file.
	if err := flag.Set("logtostderr", "true"); err != nil {
		return err
	}
	capture, err := newStderrCapture()
	if err != nil {
		return err
	}
	defer capture.close()

	exchanges := goMetricsExchanges()
	var list strings.Builder
	for _, n := range exchanges {
		list.WriteString(string(n))
		list.WriteByte('\n')
	}
	if err := os.WriteFile(filepath.Join(outDir, "exchanges.txt"), []byte(list.String()), 0o644); err != nil {
		return err
	}

	var digests, dumps strings.Builder
	digest := func(name, dump string) {
		h := fnv.New64a()
		h.Write([]byte(dump))
		fmt.Fprintf(&digests, "%s %d %016x\n", name, len(dump), h.Sum64())
	}
	for _, s := range scenarios {
		dump, err := goMetricsScenario(s, exchanges, capture)
		if err != nil {
			return fmt.Errorf("scenario %s: %v", s.name, err)
		}
		if s.name == "startup_viper_defaults" {
			if err := os.WriteFile(filepath.Join(outDir, "registry.txt"), []byte(dump), 0o644); err != nil {
				return err
			}
		}
		fmt.Fprintf(&dumps, "%s\t%s\n", strconv.QuoteToASCII("scenario"), strconv.QuoteToASCII(s.name))
		dumps.WriteString(recordedRows(dump))
		digest(s.name, dump)
	}
	if err := os.WriteFile(filepath.Join(outDir, "dumps.txt"), []byte(dumps.String()), 0o644); err != nil {
		return err
	}
	// The production adapter list (router/router.go passes CoreBidderNames).
	for _, s := range scenarios {
		if s.name != "startup_viper_defaults" {
			continue
		}
		dump, err := goMetricsScenario(s, openrtb_ext.CoreBidderNames(), capture)
		if err != nil {
			return fmt.Errorf("scenario %s with all core bidders: %v", s.name, err)
		}
		digest(s.name+"@core", dump)
	}
	if err := os.WriteFile(filepath.Join(outDir, "digests.txt"), []byte(digests.String()), 0o644); err != nil {
		return err
	}

	for _, w := range []struct {
		name  string
		write func(string) error
	}{
		{"ewma.txt", writeEWMA},
		{"samples.txt", writeSamples},
		{"points.txt", writePoints},
		{"truncate.txt", writeTruncate},
		{"wire.txt", writeWire},
		{"redirects.txt", writeRedirects},
		{"reporter.txt", writeReporter},
		{"keys.txt", writeKeys},
		{"paths.txt", writePaths},
		{"errorf.txt", writeErrorf},
		{"urls.txt", writeURLs},
	} {
		if err := w.write(filepath.Join(outDir, w.name)); err != nil {
			return fmt.Errorf("%s: %v", w.name, err)
		}
	}
	return nil
}

// stderrCapture points os.Stderr at a temporary file and hands out what was written since the
// last take.
type stderrCapture struct {
	orig *os.File
	f    *os.File
	off  int64
}

func newStderrCapture() (*stderrCapture, error) {
	f, err := os.CreateTemp("", "gometrics-glog-*")
	if err != nil {
		return nil, err
	}
	c := &stderrCapture{orig: os.Stderr, f: f}
	os.Stderr = f
	return c, nil
}

func (c *stderrCapture) take() (string, error) {
	end, err := c.f.Seek(0, io.SeekCurrent)
	if err != nil {
		return "", err
	}
	buf := make([]byte, end-c.off)
	if _, err := c.f.ReadAt(buf, c.off); err != nil && !errors.Is(err, io.EOF) {
		return "", err
	}
	c.off = end
	return string(buf), nil
}

func (c *stderrCapture) close() {
	os.Stderr = c.orig
	name := c.f.Name()
	c.f.Close()
	os.Remove(name)
}

// glogHeader matches a glog header up to the message ("E1118 15:04:05.123456   123 logger.go:22] "):
// the site is Prebid's logger facade, which says nothing about the engine.
var glogHeader = regexp.MustCompile(`(?m)^([IWEF])\d{4} \d{2}:\d{2}:\d{2}\.\d{6} +\d+ [^\]\n]*\] `)

// normalizeGlog keeps the severity and the message of every glog line.
func normalizeGlog(s string) string {
	return glogHeader.ReplaceAllString(s, "$1 ")
}

func goMetricsScenario(s *scenario, exchanges []openrtb_ext.BidderName, capture *stderrCapture) (string, error) {
	registry := gometrics.NewPrefixedRegistry("prebidserver.")
	m := metrics.NewMetrics(registry, exchanges, s.disabled, s.syncers, s.modules)
	var b strings.Builder
	for _, c := range s.calls {
		if err := replayRecovering(m, c); err != nil {
			return "", fmt.Errorf("line %d: %v", c.line, err)
		}
		logged, err := capture.take()
		if err != nil {
			return "", err
		}
		if logged != "" {
			fmt.Fprintf(&b, "%s\t%s\t%s\n", strconv.QuoteToASCII("log"), strconv.QuoteToASCII(strconv.Itoa(c.line)), strconv.QuoteToASCII(normalizeGlog(logged)))
		}
	}
	b.WriteString(dumpRegistry(registry))
	return b.String(), nil
}

// recordedRows keeps the log rows of a dump and the registry rows whose count is not 0.
func recordedRows(dump string) string {
	var b strings.Builder
	for _, row := range strings.SplitAfter(dump, "\n") {
		if row == "" {
			continue
		}
		fields := strings.Split(strings.TrimSuffix(row, "\n"), "\t")
		values, err := strconv.Unquote(fields[len(fields)-1])
		if err != nil {
			panic(err)
		}
		if fields[0] != strconv.QuoteToASCII("log") && (values == "count=0" || strings.HasPrefix(values, "count=0 ")) {
			continue
		}
		b.WriteString(row)
	}
	return b.String()
}

// replayRecovering replays one call. Where the engine dereferences a nil metric (a label value
// outside the sets NewMetrics registers) Go panics; the call stops there, which is where the
// Rust engine returns.
func replayRecovering(m metrics.MetricsEngine, c call) (err error) {
	defer func() {
		if r := recover(); r != nil {
			re, ok := r.(runtime.Error)
			if !ok || !strings.Contains(re.Error(), "nil pointer dereference") {
				err = fmt.Errorf("unexpected panic: %v", r)
			}
		}
	}()
	return replay(m, c)
}

func formatG(f float64) string {
	return strconv.FormatFloat(f, 'g', -1, 64)
}

// statisticsSnapshot is what a timer or histogram snapshot offers the reporter.
type statisticsSnapshot interface {
	Count() int64
	Max() int64
	Mean() float64
	Min() int64
	Percentiles([]float64) []float64
	StdDev() float64
	Sum() int64
	Variance() float64
}

func statistics(ms statisticsSnapshot) string {
	ps := ms.Percentiles(reporterPercentiles)
	return fmt.Sprintf("count=%d max=%d mean=%s min=%d stddev=%s variance=%s sum=%d p50=%s p75=%s p95=%s p99=%s p999=%s p9999=%s",
		ms.Count(), ms.Max(), formatG(ms.Mean()), ms.Min(), formatG(ms.StdDev()), formatG(ms.Variance()), ms.Sum(),
		formatG(ps[0]), formatG(ps[1]), formatG(ps[2]), formatG(ps[3]), formatG(ps[4]), formatG(ps[5]))
}

func dumpRegistry(r gometrics.Registry) string {
	type entry struct {
		name   string
		metric interface{}
	}
	var entries []entry
	r.Each(func(name string, i interface{}) {
		entries = append(entries, entry{name, i})
	})
	sort.Slice(entries, func(i, j int) bool { return entries[i].name < entries[j].name })
	var b strings.Builder
	for _, e := range entries {
		var kind, values string
		switch m := e.metric.(type) {
		case gometrics.Counter:
			kind, values = "counter", fmt.Sprintf("count=%d", m.Snapshot().Count())
		case gometrics.Meter:
			kind, values = "meter", fmt.Sprintf("count=%d", m.Snapshot().Count())
		case gometrics.Timer:
			kind, values = "timer", statistics(m.Snapshot())
		case gometrics.Histogram:
			switch m.Sample().(type) {
			case *gometrics.ExpDecaySample:
				kind = "histogram/expdecay"
			case *gometrics.UniformSample:
				kind = "histogram/uniform"
			default:
				kind = "histogram/other"
			}
			values = statistics(m.Snapshot())
		default:
			kind = fmt.Sprintf("%T", m)
		}
		fmt.Fprintf(&b, "%s\t%s\t%s\n", strconv.QuoteToASCII(e.name), strconv.QuoteToASCII(kind), strconv.QuoteToASCII(values))
	}
	return b.String()
}

// writeEWMA writes "alpha <minutes> <bits>" for the three alphas (the expressions of ewma.go)
// and "ewma <minutes> <ops> <rates>" rows: ops is a comma-separated list of "u<n>" (Update(n))
// and "t" (Tick()), rates the bits of Rate() after each tick. Bits are IEEE-754 hex (unquoted).
func writeEWMA(path string) error {
	var b strings.Builder
	alphas := map[int]float64{
		1:  1 - math.Exp(-5.0/60.0/1),
		5:  1 - math.Exp(-5.0/60.0/5),
		15: 1 - math.Exp(-5.0/60.0/15),
	}
	ctors := map[int]func() gometrics.EWMA{1: gometrics.NewEWMA1, 5: gometrics.NewEWMA5, 15: gometrics.NewEWMA15}
	minutes := []int{1, 5, 15}
	for _, n := range minutes {
		fmt.Fprintf(&b, "alpha\t%d\t%016x\n", n, math.Float64bits(alphas[n]))
	}
	r := rand.New(rand.NewSource(20261118))
	value := func() int64 {
		switch r.Intn(6) {
		case 0:
			return 0
		case 1:
			return r.Int63n(10)
		case 2:
			return r.Int63n(1_000_000)
		case 3:
			return r.Int63n(1 << 40)
		case 4:
			return r.Int63n(1 << 62)
		default:
			return -r.Int63n(1000)
		}
	}
	for seq := 0; seq < 24; seq++ {
		for _, n := range minutes {
			e := ctors[n]()
			var ops, rates []string
			for k := 0; k < 40; k++ {
				if r.Intn(3) == 0 {
					e.Tick()
					ops = append(ops, "t")
					rates = append(rates, fmt.Sprintf("%016x", math.Float64bits(e.Rate())))
					if e.Snapshot().Rate() != e.Rate() {
						return fmt.Errorf("snapshot rate differs")
					}
				} else {
					v := value()
					e.Update(v)
					ops = append(ops, "u"+strconv.FormatInt(v, 10))
				}
			}
			fmt.Fprintf(&b, "ewma\t%d\t%s\t%s\n", n, strings.Join(ops, ","), strings.Join(rates, ","))
		}
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// splitmix64 is the generator the Rust test reproduces the sample inputs with.
type splitmix64 uint64

func (s *splitmix64) next() uint64 {
	*s += 0x9E3779B97F4A7C15
	z := uint64(*s)
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// sampleValues is the value slice of one samples.txt row.
func sampleValues(seed uint64) []int64 {
	r := splitmix64(seed)
	var n int
	switch seed % 10 {
	case 0:
		n = 0
	case 1:
		n = 1
	case 2:
		n = 2
	case 9:
		n = int(r.next() % 3000)
	default:
		n = int(r.next() % 60)
	}
	mode := r.next() % 5
	values := make([]int64, n)
	for i := range values {
		x := r.next()
		switch mode {
		case 0:
			values[i] = int64(x%201) - 100
		case 1:
			values[i] = int64(x)
		case 2:
			values[i] = math.MaxInt64 - int64(x%1000)
		case 3:
			values[i] = int64(x % 5_000_000_000)
		default:
			if x%2 == 0 {
				values[i] = math.MinInt64 + int64(x%1000)
			} else {
				values[i] = int64(x % 7)
			}
		}
	}
	return values
}

// writeSamples writes "<seed> <len> <FNV-1a of the values> <max> <min> <sum> <mean> <variance>
// <stddev> <percentiles> <variance after the percentiles sorted the slice>" (unquoted; floats as
// IEEE-754 hex, percentiles comma-separated).
func writeSamples(path string) error {
	ps := []float64{0.5, 0.75, 0.95, 0.99, 0.999, 0.9999, 0, 0.1, 0.25, 1}
	hex := func(f float64) string { return fmt.Sprintf("%016x", math.Float64bits(f)) }
	var b strings.Builder
	for seed := uint64(0); seed < 300; seed++ {
		values := sampleValues(seed)
		h := fnv.New64a()
		for _, v := range values {
			fmt.Fprintf(h, "%d\n", v)
		}
		max, min, sum := gometrics.SampleMax(values), gometrics.SampleMin(values), gometrics.SampleSum(values)
		mean, variance, stddev := gometrics.SampleMean(values), gometrics.SampleVariance(values), gometrics.SampleStdDev(values)
		var pcts []string
		for _, p := range gometrics.SamplePercentiles(values, ps) {
			pcts = append(pcts, hex(p))
		}
		sortedVariance := gometrics.SampleVariance(values)
		fmt.Fprintf(&b, "%d\t%d\t%016x\t%d\t%d\t%d\t%s\t%s\t%s\t%s\t%s\n", seed, len(values), h.Sum64(), max, min, sum,
			hex(mean), hex(variance), hex(stddev), strings.Join(pcts, ","), hex(sortedVariance))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// pointAlphabet is what random points are built from: the bytes the line protocol escapes and
// some that it does not.
var pointAlphabet = []string{"a", "Z", "0", ",", " ", "=", "\"", "\\", "é", "\n", "\t", "\x00", " ", "😀", ".", "_"}

// writePoints writes "<measurement> <tags> <field key> <kind> <value> <time> <MarshalString>"
// rows. Tags are key, value, key, value... joined by U+001F; kind is f (float64, value as
// IEEE-754 hex) or i (int64); time is "zero" or "<unix seconds>.<nanoseconds>". A string
// written "rep:<n>:<unit>" is <unit> repeated n times, and an output longer than 300 bytes is
// written "fnv:<FNV-1a 64 hex>:<length>".
func writePoints(path string) error {
	var b strings.Builder
	quote := func(fields ...string) {
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			b.WriteString(strconv.Quote(f))
		}
		b.WriteByte('\n')
	}
	decode := func(s string) string {
		if rest, ok := strings.CutPrefix(s, "rep:"); ok {
			n, unit, _ := strings.Cut(rest, ":")
			count, err := strconv.Atoi(n)
			if err != nil {
				panic(err)
			}
			return strings.Repeat(unit, count)
		}
		return s
	}
	add := func(meas string, tags []string, key string, v interface{}, t time.Time) {
		tagMap := map[string]string{}
		for i := 0; i+1 < len(tags); i += 2 {
			tagMap[decode(tags[i])] = decode(tags[i+1])
		}
		p := client.Point{Measurement: decode(meas), Tags: tagMap, Fields: map[string]interface{}{decode(key): v}, Time: t}
		out := p.MarshalString()
		if len(out) > 300 {
			h := fnv.New64a()
			h.Write([]byte(out))
			out = fmt.Sprintf("fnv:%016x:%d", h.Sum64(), len(out))
		}
		var kind, value string
		switch x := v.(type) {
		case float64:
			kind, value = "f", fmt.Sprintf("%016x", math.Float64bits(x))
		case int64:
			kind, value = "i", strconv.FormatInt(x, 10)
		default:
			panic(fmt.Sprintf("field type %T", v))
		}
		ts := "zero"
		if !t.IsZero() {
			ts = fmt.Sprintf("%d.%d", t.Unix(), t.Nanosecond())
		}
		quote(meas, strings.Join(tags, "\x1f"), key, kind, value, ts, out)
	}
	t := time.Unix(1731940000, 123456789)
	add("cpu", nil, "value", 1.5, t)
	add("cpu", []string{"host", "a", "region", "b"}, "v", int64(-3), t)
	add(" a,b\\", []string{"k k", "v,v", "e=", "=x", "empty", ""}, "f\"x y,z=w\\", 0.1, t)
	add("a\\,b", []string{"\\ ", "\\="}, "\\,", 2.0, t)
	add("", nil, "f", 1.0, t)
	add("m", nil, "", 1.0, t)
	add("m", []string{"", "v"}, "f", 1.0, t)
	for _, f := range []float64{
		0, math.Copysign(0, -1), 1, -1, 0.1, 1e21, 1e22, 1e-7, 123456789.125, 9007199254740993,
		math.MaxFloat64, math.SmallestNonzeroFloat64, math.NaN(), math.Inf(1), math.Inf(-1),
	} {
		add("pbs", []string{"bucket", "p99"}, "prebidserver.request_time.timer", f, t)
	}
	for _, i := range []int64{0, 1, -1, math.MaxInt64, math.MinInt64} {
		add("pbs", nil, "prebidserver.active_connections.count", i, t)
	}
	for _, tt := range []time.Time{
		{}, time.Unix(0, 0), time.Unix(-1, 999999999), time.Unix(0, models.MinNanoTime),
		time.Unix(0, models.MinNanoTime-1), time.Unix(0, models.MaxNanoTime),
		time.Unix(0, models.MaxNanoTime).Add(1), time.Unix(-10_000_000_000, 0), time.Unix(10_000_000_000, 0),
		time.Unix(253402300800, 0),
	} {
		add("pbs", nil, "f", int64(1), tt)
	}
	// The key length limit: len(escaped measurement and tags) + 4 + len(field key) <= 65535.
	add("rep:65530:m", nil, "f", 1.0, t)
	add("rep:65531:m", nil, "f", 1.0, t)
	add("rep:32765:,", nil, "f", 1.0, t)
	add("m", []string{"k", "rep:65520:v"}, "f", 1.0, t)
	add("m", nil, "rep:65530:f", 1.0, t)
	add("m", nil, "rep:65531:f", 1.0, t)

	r := rand.New(rand.NewSource(20261120))
	token := func(max int) string {
		n := r.Intn(max + 1)
		var s strings.Builder
		for i := 0; i < n; i++ {
			s.WriteString(pointAlphabet[r.Intn(len(pointAlphabet))])
		}
		return s.String()
	}
	for i := 0; i < 200; i++ {
		var tags []string
		for j := r.Intn(4); j > 0; j-- {
			tags = append(tags, token(4), token(4))
		}
		key := token(5)
		var v interface{}
		if r.Intn(2) == 0 {
			v = r.NormFloat64() * math.Pow(10, float64(r.Intn(40)-20))
		} else {
			v = r.Int63() - r.Int63()
		}
		add(token(5), tags, key, v, time.Unix(r.Int63n(1<<33), r.Int63n(1e9)))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// writeTruncate writes "<sec> <nsec> <d> <result sec> <result nsec>" rows (unquoted) for
// time.Unix(sec, nsec).Truncate(d).
func writeTruncate(path string) error {
	// metric_send_interval seconds times time.Second wraps like any int64 product.
	seconds := func(n int64) time.Duration { return time.Second * time.Duration(n) }
	ds := []time.Duration{
		math.MinInt64, -time.Second, -1, 0, 1, 2, 3, 7, 999, 1000, 1001, time.Microsecond,
		250 * time.Millisecond, 333 * time.Millisecond, time.Second, 7 * time.Second, 20 * time.Second,
		time.Minute, time.Hour, 24 * time.Hour, 1<<62 + 12345, math.MaxInt64,
		seconds(9223372037), seconds(-9223372037), seconds(1 << 40),
	}
	r := rand.New(rand.NewSource(20261121))
	var b strings.Builder
	for i := 0; i < 32; i++ {
		var sec int64
		switch i % 4 {
		case 0:
			sec = 1731940000 + r.Int63n(100_000_000)
		case 1:
			sec = r.Int63n(1<<40) - 1<<39
		case 2:
			sec = -62135596800 - r.Int63n(1_000_000_000)
		default:
			sec = int64(r.Uint64()>>2) - 1<<61
		}
		nsec := r.Int63n(1_000_000_000)
		t := time.Unix(sec, nsec)
		for _, d := range ds {
			tt := t.Truncate(d)
			fmt.Fprintf(&b, "%d\t%d\t%d\t%d\t%d\n", sec, nsec, int64(d), tt.Unix(), tt.Nanosecond())
		}
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}
