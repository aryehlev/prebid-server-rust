package main

// The InfluxDB goldens: the bytes github.com/influxdata/influxdb1-client 8bf82d3c094d sends for
// Write and Ping and what it makes of the answers (wire.txt), and the batches the reporter of
// github.com/vrischmann/go-metrics-influxdb v0.1.1 sends and the lines it logs (reporter.txt).
// Both run against fakeEndpoint, a raw TCP HTTP/1.1 endpoint on 127.0.0.1 that the Rust tests
// reimplement, so the requests are compared byte for byte.
//
// Every field is strconv.Quote'd and fields are tab-separated. In requests, errors and log
// lines, the endpoint's address is written {addr} and an address that refuses connections
// {refused}.
//
// A wire.txt row is: name, URL, username, password, op (write or ping), database, the points
// (wirePoint.encode, joined by U+001D), the endpoint's responses (joined by U+001D), whether the
// case runs an endpoint, the requests the endpoint read (joined by U+001D), "error: <text>" or ""
// when the call returned no error, and the version Ping returned.

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"net"
	uurl "net/url"
	"os"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	client "github.com/influxdata/influxdb1-client"
	gometrics "github.com/rcrowley/go-metrics"
	influxdb "github.com/vrischmann/go-metrics-influxdb"
)

// fakeEndpoint reads HTTP/1.1 requests (a head and a Content-Length body), records their bytes
// and answers the n-th request of a connection with responses[n] (the last one once they run
// out). An empty response closes the connection without answering; a response that starts with
// U+0004 is written without it, then the connection is closed.
type fakeEndpoint struct {
	ln        net.Listener
	responses []string

	mu       sync.Mutex
	requests [][]byte
	arrived  chan struct{}
}

func newFakeEndpoint(responses ...string) (*fakeEndpoint, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	e := &fakeEndpoint{ln: ln, responses: responses, arrived: make(chan struct{}, 1024)}
	go e.serve()
	return e, nil
}

func (e *fakeEndpoint) addr() string { return e.ln.Addr().String() }

func (e *fakeEndpoint) close() { e.ln.Close() }

func (e *fakeEndpoint) serve() {
	for {
		conn, err := e.ln.Accept()
		if err != nil {
			return
		}
		go e.handle(conn)
	}
}

func (e *fakeEndpoint) handle(conn net.Conn) {
	defer conn.Close()
	r := bufio.NewReader(conn)
	for n := 0; ; n++ {
		req, err := readRequest(r)
		if err != nil {
			return
		}
		e.mu.Lock()
		e.requests = append(e.requests, req)
		e.mu.Unlock()
		e.arrived <- struct{}{}
		resp := e.responses[len(e.responses)-1]
		if n < len(e.responses) {
			resp = e.responses[n]
		}
		if resp == "" {
			return
		}
		last := strings.HasPrefix(resp, "\x04")
		if _, err := io.WriteString(conn, strings.TrimPrefix(resp, "\x04")); err != nil || last {
			return
		}
	}
}

// all returns every request the endpoint read, joined by U+001D.
func (e *fakeEndpoint) all() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	parts := make([]string, len(e.requests))
	for i, req := range e.requests {
		parts[i] = string(req)
	}
	return strings.Join(parts, "\x1d")
}

// wait returns the first request, waiting up to timeout for it.
func (e *fakeEndpoint) wait(timeout time.Duration) ([]byte, error) {
	select {
	case <-e.arrived:
	case <-time.After(timeout):
		return nil, errors.New("no request arrived")
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.requests[0], nil
}

// readRequest reads one request: the head up to the blank line and a body of the head's
// Content-Length.
func readRequest(r *bufio.Reader) ([]byte, error) {
	var b bytes.Buffer
	length := 0
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			return nil, err
		}
		b.WriteString(line)
		if line == "\r\n" {
			break
		}
		if k, v, ok := strings.Cut(strings.TrimRight(line, "\r\n"), ":"); ok && strings.EqualFold(k, "Content-Length") {
			if length, err = strconv.Atoi(strings.TrimSpace(v)); err != nil {
				return nil, err
			}
		}
	}
	body := make([]byte, length)
	if _, err := io.ReadFull(r, body); err != nil {
		return nil, err
	}
	b.Write(body)
	return b.Bytes(), nil
}

// refusedAddr returns an address nothing listens on.
func refusedAddr() (string, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return "", err
	}
	addr := ln.Addr().String()
	return addr, ln.Close()
}

// wirePoint is one point of a wire case: measurement, tags (key, value, key, value...), one
// field and the time in Unix nanoseconds (0 for the zero time).
type wirePoint struct {
	measurement string
	tags        []string
	key         string
	value       interface{}
	unixNano    int64
}

func (p wirePoint) point() client.Point {
	tags := map[string]string{}
	for i := 0; i+1 < len(p.tags); i += 2 {
		tags[p.tags[i]] = p.tags[i+1]
	}
	var t time.Time
	if p.unixNano != 0 {
		t = time.Unix(0, p.unixNano)
	}
	return client.Point{Measurement: p.measurement, Tags: tags, Fields: map[string]interface{}{p.key: p.value}, Time: t}
}

// encode writes the point as "<measurement> U+001F <tags joined by U+001E> U+001F <key>
// U+001F <kind> U+001F <value> U+001F <unix nano>", kind f (value as IEEE-754 hex) or i.
func (p wirePoint) encode() string {
	kind, value := "i", ""
	switch v := p.value.(type) {
	case float64:
		kind, value = "f", fmt.Sprintf("%016x", math.Float64bits(v))
	case int64:
		value = strconv.FormatInt(v, 10)
	default:
		panic(fmt.Sprintf("field type %T", p.value))
	}
	return strings.Join([]string{p.measurement, strings.Join(p.tags, "\x1e"), p.key, kind, value, strconv.FormatInt(p.unixNano, 10)}, "\x1f")
}

type wireCase struct {
	name     string
	url      string
	username string
	password string
	op       string
	db       string
	points   []wirePoint
	// responses the endpoint answers with; nil means the case needs no endpoint.
	responses []string
}

func gzipped(s string) string {
	var b bytes.Buffer
	w := gzip.NewWriter(&b)
	w.Write([]byte(s))
	w.Close()
	return b.String()
}

// answer is an HTTP/1.1 response with the given status line, extra header lines and a
// Content-Length body.
func answer(status, body string, headers ...string) string {
	var b strings.Builder
	b.WriteString("HTTP/1.1 " + status + "\r\n")
	for _, h := range headers {
		b.WriteString(h + "\r\n")
	}
	b.WriteString("Content-Length: " + strconv.Itoa(len(body)) + "\r\n\r\n" + body)
	return b.String()
}

func wireCases() []wireCase {
	counter := wirePoint{"prebidserver", nil, "prebidserver.active_connections.count", int64(42), 1731940000123456789}
	timer := wirePoint{"prebidserver", []string{"bucket", "p99"}, "prebidserver.request_time.timer", 1.5e6, 1731940000123456789}
	odd := wirePoint{"a b,c", []string{"k=", "v ,", "empty", ""}, "f,\"= ", math.NaN(), 0}
	ok204 := "HTTP/1.1 204 No Content\r\nX-Influxdb-Version: 1.8.10\r\n\r\n"
	both := []wirePoint{counter, timer}
	return []wireCase{
		{name: "write", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_credentials", url: "http://{addr}", username: "admin", password: "s3cr:et", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_url_userinfo", url: "http://u%20ser:p%40ss@{addr}", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_both_credentials", url: "http://u:p@{addr}", username: "admin", password: "", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_base_path", url: "http://{addr}/influx/", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_dot_path", url: "http://{addr}/a/../b/./c//", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_raw_path", url: "http://{addr}/a%2Fb%20c", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_query", url: "http://{addr}/?a=1&db=old&z=%20", op: "write", db: "pbs db&x", points: both, responses: []string{ok204}},
		{name: "write_fragment", url: "http://{addr}/p#frag", op: "write", db: "", points: both, responses: []string{ok204}},
		{name: "write_empty_batch", url: "http://{addr}", op: "write", db: "pbs", responses: []string{ok204}},
		{name: "write_point_error", url: "http://{addr}", op: "write", db: "pbs", points: []wirePoint{odd, counter}, responses: []string{ok204}},
		{name: "write_200", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("200 OK", "{}")}},
		{name: "write_500_format", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("500 Internal Server Error", "%d oops %s %%")}},
		{name: "write_404_empty", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("404 Not Found", "")}},
		{name: "write_400_json", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("400 Bad Request", "{\"error\":\"partial write: bad\\n\"}\n", "Content-Type: application/json")}},
		{name: "write_gzip_error", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("503 Service Unavailable", gzipped("database is down"), "Content-Encoding: gzip")}},
		{name: "write_chunked_error", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"}},
		{name: "write_closed", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{""}},
		{name: "write_refused", url: "http://{refused}", op: "write", db: "pbs", points: both},
		{name: "write_no_scheme", url: "localhost", op: "write", db: "pbs", points: both},
		{name: "write_no_scheme_port", url: "localhost:8086", op: "write", db: "pbs", points: both},
		{name: "write_ftp", url: "ftp://127.0.0.1:21", op: "write", db: "pbs", points: both},
		{name: "ping", url: "http://{addr}", op: "ping", responses: []string{ok204}},
		{name: "ping_credentials", url: "http://{addr}/base", username: "admin", password: "pw", op: "ping", responses: []string{ok204}},
		{name: "ping_500", url: "http://{addr}", op: "ping", responses: []string{answer("500 Internal Server Error", "fail", "X-Influxdb-Version: 2.0")}},
		{name: "ping_closed", url: "http://{addr}", op: "ping", responses: []string{""}},
		{name: "ping_refused", url: "http://{refused}", op: "ping"},
		{name: "ping_no_scheme", url: "localhost", op: "ping"},
		// Queries url.ParseQuery drops pairs of, statuses and bodies at the edges of the
		// transport, redirects (the request repeated, turned into a GET, looping, or to an
		// address that cannot be dialed), a port out of range, and the version header's case.
		{name: "write_query_odd", url: "http://{addr}/?a=1;b=2&c=%zz&&=e&f&g=1&g=2&db=x+y&rp=keep%2B", op: "write", db: "pbs", points: both, responses: []string{ok204}},
		{name: "write_201_format", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{answer("201 Created", "created %s %[2]d %! 100%")}},
		{name: "write_100_continue", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 100 Continue\r\n\r\n" + ok204}},
		{name: "write_204_body", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 204 No Content\r\nContent-Length: 5\r\n\r\nhello"}},
		{name: "write_http10", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.0 204 No Content\r\n\r\n"}},
		{name: "write_truncated_body", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"\x04HTTP/1.1 500 Internal Server Error\r\nContent-Length: 10\r\n\r\nabc"}},
		{name: "write_truncated_200", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"\x04HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"}},
		{name: "write_truncated_chunked", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"\x04HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel"}},
		{name: "write_body_until_close", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"\x04HTTP/1.1 500 Internal Server Error\r\n\r\nbody until close"}},
		{name: "write_malformed", url: "http://{addr}", op: "write", db: "pbs", points: both, responses: []string{"\x04garbage\r\n\r\n"}},
		{name: "write_307", url: "http://{addr}/base", username: "admin", password: "pw", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 307 Temporary Redirect\r\nLocation: /moved?x=1\r\nContent-Length: 0\r\n\r\n", ok204}},
		{name: "write_302", url: "http://{addr}", username: "admin", password: "pw", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 302 Found\r\nLocation: moved\r\nContent-Length: 0\r\n\r\n", ok204}},
		{name: "write_redirect_loop", url: "http://{addr}", op: "write", db: "pbs", points: []wirePoint{counter}, responses: []string{"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n"}},
		{name: "write_redirect_undialable", url: "http://{addr}", username: "admin", op: "write", db: "pbs", points: both, responses: []string{"HTTP/1.1 308 Permanent Redirect\r\nLocation: http://127.0.0.1:99999/elsewhere\r\nContent-Length: 0\r\n\r\n"}},
		{name: "write_invalid_port", url: "http://127.0.0.1:99999", op: "write", db: "pbs", points: both},
		{name: "ping_version_case", url: "http://{addr}", op: "ping", responses: []string{"HTTP/1.1 204 No Content\r\nx-influxdb-version: 1.8\r\nX-Influxdb-Version: 2.0\r\n\r\n"}},
		{name: "ping_redirect", url: "http://{addr}", op: "ping", responses: []string{"HTTP/1.1 301 Moved Permanently\r\nLocation: /ping2\r\nContent-Length: 0\r\n\r\n", "HTTP/1.1 204 No Content\r\nX-Influxdb-Version: 3\r\n\r\n"}},
	}
}

func writeWire(path string) error {
	refused, err := refusedAddr()
	if err != nil {
		return err
	}
	var b strings.Builder
	for _, c := range wireCases() {
		raw := strings.ReplaceAll(c.url, "{refused}", refused)
		var e *fakeEndpoint
		if c.responses != nil {
			if e, err = newFakeEndpoint(c.responses...); err != nil {
				return err
			}
			raw = strings.ReplaceAll(raw, "{addr}", e.addr())
		}
		u, err := uurl.Parse(raw)
		if err != nil {
			return fmt.Errorf("%s: %v", c.name, err)
		}
		cl, err := client.NewClient(client.Config{URL: *u, Username: c.username, Password: c.password})
		if err != nil {
			return fmt.Errorf("%s: %v", c.name, err)
		}
		var callErr error
		version := ""
		switch c.op {
		case "write":
			var pts []client.Point
			for _, p := range c.points {
				pts = append(pts, p.point())
			}
			_, callErr = cl.Write(client.BatchPoints{Points: pts, Database: c.db})
		case "ping":
			_, version, callErr = cl.Ping()
		default:
			return fmt.Errorf("%s: unknown op %q", c.name, c.op)
		}
		request := ""
		if e != nil {
			if _, err := e.wait(5 * time.Second); err != nil {
				return fmt.Errorf("%s: %v", c.name, err)
			}
			// Every request was read before the answer the call returned on.
			request = e.all()
		}
		errText := ""
		if callErr != nil {
			errText = "error: " + callErr.Error()
		}
		normalize := func(s string) string {
			if e != nil {
				s = strings.ReplaceAll(s, e.addr(), "{addr}")
			}
			return strings.ReplaceAll(s, refused, "{refused}")
		}
		var encoded []string
		for _, p := range c.points {
			encoded = append(encoded, p.encode())
		}
		fields := []string{c.name, c.url, c.username, c.password, c.op, c.db, strings.Join(encoded, "\x1d"),
			strings.Join(c.responses, "\x1d"), strconv.FormatBool(c.responses != nil), normalize(request), normalize(errText), version}
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			b.WriteString(strconv.Quote(f))
		}
		b.WriteByte('\n')
		if e != nil {
			e.close()
		}
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// reporterInputs is the registry the reporter goldens send: "<kind> <name> <values>" with kind
// counter (Inc each value), meter (Mark each value), timer (Update each value as a
// time.Duration), expdecay and uniform (a histogram over NewExpDecaySample(1028, 0.015) or
// NewUniformSample(1024), Update each value). Names are percent-encoded.
var reporterInputs = []string{
	"counter active_connections 5,-2",
	"counter zero",
	"meter requests.ok.auction 7,3",
	"meter idle",
	"timer request_time 1000000,2500000,30000000,17",
	"timer empty_timer",
	"expdecay adapter.appnexus.prices 0,1,1500,99,-4",
	"uniform requests.size.auction 1024,2048,512",
	"uniform empty_histogram",
	"counter name%20with%20space,comma=equals%22quote%5Cbackslash 1",
	"meter %C3%A9t%C3%A9.%F0%9F%98%80 9223372036854775807",
}

func buildReporterRegistry() (gometrics.Registry, error) {
	r := gometrics.NewPrefixedRegistry("prebidserver.")
	for _, in := range reporterInputs {
		parts := strings.Split(in, " ")
		name, err := uurl.PathUnescape(parts[1])
		if err != nil {
			return nil, err
		}
		var values []int64
		if len(parts) > 2 {
			for _, s := range strings.Split(parts[2], ",") {
				v, err := strconv.ParseInt(s, 10, 64)
				if err != nil {
					return nil, err
				}
				values = append(values, v)
			}
		}
		switch parts[0] {
		case "counter":
			c := gometrics.GetOrRegisterCounter(name, r)
			for _, v := range values {
				c.Inc(v)
			}
		case "meter":
			m := gometrics.GetOrRegisterMeter(name, r)
			for _, v := range values {
				m.Mark(v)
			}
		case "timer":
			t := gometrics.GetOrRegisterTimer(name, r)
			for _, v := range values {
				t.Update(time.Duration(v))
			}
		case "expdecay", "uniform":
			s := gometrics.NewExpDecaySample(1028, 0.015)
			if parts[0] == "uniform" {
				s = gometrics.NewUniformSample(1024)
			}
			h := gometrics.GetOrRegisterHistogram(name, r, s)
			for _, v := range values {
				h.Update(v)
			}
		default:
			return nil, fmt.Errorf("unknown kind %q", parts[0])
		}
	}
	return r, nil
}

// rateBuckets are the buckets whose values depend on the clock.
var rateBuckets = map[string]bool{"m1": true, "m5": true, "m15": true, "meanrate": true}

// normalizeLine replaces the timestamp of a line-protocol line with <ts>, and its value with
// <rate> when it is a meter or timer rate.
func normalizeLine(line string) (string, error) {
	sp := strings.LastIndexByte(line, ' ')
	if sp < 0 {
		return "", fmt.Errorf("line %q has no timestamp", line)
	}
	head := line[:sp]
	eq := strings.LastIndexByte(head, '=')
	key := strings.IndexByte(head, ' ')
	if eq < 0 || key < 0 {
		return "", fmt.Errorf("line %q has no field", line)
	}
	series, field := head[:key], head[key+1:eq]
	bucket := ""
	if i := strings.Index(series, ",bucket="); i >= 0 {
		bucket = series[i+len(",bucket="):]
	}
	if rateBuckets[bucket] || bucket == "mean" && strings.HasSuffix(field, ".meter") {
		head = head[:eq+1] + "<rate>"
	}
	return head + " <ts>", nil
}

// requestHead returns the head of a request with the Content-Length value written {length}.
func requestHead(req []byte) string {
	head, _, _ := strings.Cut(string(req), "\r\n\r\n")
	return contentLength.ReplaceAllString(head, "Content-Length: {length}") + "\r\n\r\n"
}

var contentLength = regexp.MustCompile(`Content-Length: \d+`)

// stdLogHeader matches the date and time Go's standard logger starts each line with.
var stdLogHeader = regexp.MustCompile(`(?m)^\d{4}/\d{2}/\d{2} \d{2}:\d{2}:\d{2} `)

// syncBuffer collects what the standard logger writes from several goroutines.
type syncBuffer struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *syncBuffer) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *syncBuffer) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

// writeReporter runs influxdb.InfluxDB (as metrics/config/metrics.go starts it) against fake
// endpoints and writes:
//
//	"input" <reporterInputs row>
//	"send" <request head> then "line" <normalized line>, sorted: the first batch
//	"aligned" <true|false>: with align, every timestamp of the first batch equal and a multiple
//	of the interval
//	"log" <case> <line>: what the standard logger printed, the date and time written <date>
func writeReporter(path string) error {
	logs := &syncBuffer{}
	log.SetOutput(logs)
	defer log.SetOutput(io.Discard)

	var b strings.Builder
	row := func(fields ...string) {
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			b.WriteString(strconv.Quote(f))
		}
		b.WriteByte('\n')
	}
	for _, in := range reporterInputs {
		row("input", in)
	}

	ok204 := "HTTP/1.1 204 No Content\r\n\r\n"
	send, err := newFakeEndpoint(ok204)
	if err != nil {
		return err
	}
	aligned, err := newFakeEndpoint(ok204)
	if err != nil {
		return err
	}
	failing, err := newFakeEndpoint(answer("500 Internal Server Error", "%d oops %v."), ok204)
	if err != nil {
		return err
	}
	refused, err := refusedAddr()
	if err != nil {
		return err
	}
	registries := make([]gometrics.Registry, 3)
	for i := range registries {
		if registries[i], err = buildReporterRegistry(); err != nil {
			return err
		}
	}
	go influxdb.InfluxDB(registries[0], 150*time.Millisecond, "http://"+send.addr(), "pbsdb", "pbs", "admin", "pw", false)
	go influxdb.InfluxDB(registries[1], time.Second, "http://"+aligned.addr(), "pbsdb", "pbs", "", "", true)
	go influxdb.InfluxDB(registries[2], 150*time.Millisecond, "http://"+failing.addr(), "pbsdb", "pbs", "", "", false)
	go influxdb.InfluxDB(registries[2], 0, "http://"+refused, "pbsdb", "pbs", "", "", false)
	go influxdb.InfluxDB(registries[2], time.Second, "http://[::1", "pbsdb", "pbs", "", "", false)
	go influxdb.InfluxDB(registries[2], time.Second, "%zz", "pbsdb", "pbs", "", "", false)

	first, err := send.wait(10 * time.Second)
	if err != nil {
		return fmt.Errorf("send: %v", err)
	}
	row("send", strings.ReplaceAll(requestHead(first), send.addr(), "{addr}"))
	_, body, _ := strings.Cut(string(first), "\r\n\r\n")
	var lines []string
	for _, line := range strings.Split(strings.TrimSuffix(body, "\n"), "\n") {
		n, err := normalizeLine(line)
		if err != nil {
			return err
		}
		lines = append(lines, n)
	}
	sort.Strings(lines)
	for _, line := range lines {
		row("line", line)
	}

	alignedReq, err := aligned.wait(10 * time.Second)
	if err != nil {
		return fmt.Errorf("aligned: %v", err)
	}
	_, alignedBody, _ := strings.Cut(string(alignedReq), "\r\n\r\n")
	allAligned := true
	stamp := ""
	for _, line := range strings.Split(strings.TrimSuffix(alignedBody, "\n"), "\n") {
		ts := line[strings.LastIndexByte(line, ' ')+1:]
		if stamp == "" {
			stamp = ts
		}
		n, err := strconv.ParseInt(ts, 10, 64)
		allAligned = allAligned && err == nil && ts == stamp && n%int64(time.Second) == 0
	}
	row("aligned", strconv.FormatBool(allAligned))

	// The failing endpoint's first answer is logged; the refused ping is logged after 5 s.
	deadline := time.Now().Add(20 * time.Second)
	for {
		text := logs.String()
		if strings.Contains(text, "unable to send") && strings.Contains(text, "got error while sending a ping") {
			break
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("reporter logs incomplete: %q", text)
		}
		time.Sleep(50 * time.Millisecond)
	}
	text := stdLogHeader.ReplaceAllString(logs.String(), "<date> ")
	text = strings.ReplaceAll(text, refused, "{refused}")
	text = strings.ReplaceAll(text, failing.addr(), "{addr}")
	var logged []string
	seen := map[string]bool{}
	for _, line := range strings.SplitAfter(text, "\n") {
		if line != "" && !seen[line] {
			seen[line] = true
			logged = append(logged, line)
		}
	}
	sort.Strings(logged)
	for _, line := range logged {
		row("log", line)
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}
