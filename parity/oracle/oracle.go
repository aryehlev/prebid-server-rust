// Package oracle is the parity runner's mock oracle: a loopback HTTP/1.1
// forward proxy that both servers' parity transports send every outbound call
// to. It captures each request exactly as it arrived on the wire (header order
// and case, raw and decoded body), answers it from the case's plan, adapter
// fixtures, bidder templates, content-addressed cache UUIDs or static
// currency/GVL fixtures, and hands the captures to the runner per
// (case, implementation).
package oracle

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"net"
	"path/filepath"
	"sort"
	"sync"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
)

// Defaults of Options.
const (
	defaultAddr        = "127.0.0.1:0"
	defaultMaxBody     = 32 << 20
	defaultTimeoutHold = 30 * time.Second
	defaultFixturesDir = "parity/fixtures"
	readBufferSize     = 64 << 10
)

// Options configure an Oracle.
type Options struct {
	// Addr is the loopback listen address; default 127.0.0.1:0.
	Addr string
	// FixturesDir holds currency/latest.json and gvl/v{2,3}/vendor-list.json;
	// default <RepoRoot>/parity/fixtures.
	FixturesDir string
	// RepoRoot is used for the default FixturesDir; default ".".
	RepoRoot string
	// Templates and Replay are the bidder mock sources; nil disables them.
	Templates *TemplateSet
	Replay    *ReplayIndex
	// MaxBodyBytes caps request bodies; default 32 MiB.
	MaxBodyBytes int64
	// TimeoutHold caps how long fail=timeout holds a connection; default 30s.
	TimeoutHold time.Duration
	// Logf receives operational messages; nil discards them.
	Logf func(format string, args ...any)
}

// Capture is one outbound call as the oracle saw and answered it.
type Capture struct {
	// Call is the trace record (wire: true). Request headers exclude
	// X-Parity-Meta; response headers are exactly what was written.
	Call parity.OutboundCall `json:"call"`
	// Meta is the decoded X-Parity-Meta, nil when absent or invalid.
	Meta      *parity.OutboundMeta `json:"meta,omitempty"`
	MetaError string               `json:"meta_error,omitempty"`
	// Decoded is the content-decoded request body when Content-Encoding was set.
	Decoded *parity.Body `json:"decoded_body,omitempty"`
	// Chunked is true when the request body used chunked transfer coding.
	Chunked bool `json:"chunked,omitempty"`
	// Unexpected flags calls no mock source expected (unexpected_outbound).
	Unexpected bool `json:"unexpected_outbound,omitempty"`

	arrival int
}

// captureKey separates captures by case and implementation.
type captureKey struct {
	caseID string
	impl   string
}

// Oracle is a running mock oracle.
type Oracle struct {
	opts      Options
	ln        net.Listener
	static    *staticFiles
	templates *TemplateSet
	replay    *ReplayIndex
	logf      func(format string, args ...any)

	mu       sync.Mutex
	cond     *sync.Cond
	plans    map[string]*Plan
	captures map[captureKey][]Capture
	pending  map[captureKey]int
	arrivals int
	conns    map[net.Conn]struct{}
	closed   bool
	stop     chan struct{}
	wg       sync.WaitGroup
}

// Start loads the static fixtures, listens on loopback and serves until Close.
func Start(opts Options) (*Oracle, error) {
	opts = withDefaults(opts)
	static, err := loadStatic(opts.FixturesDir)
	if err != nil {
		return nil, err
	}
	ln, err := net.Listen("tcp", opts.Addr)
	if err != nil {
		return nil, fmt.Errorf("oracle: listening on %s: %w", opts.Addr, err)
	}
	if addr, ok := ln.Addr().(*net.TCPAddr); !ok || !addr.IP.IsLoopback() {
		ln.Close()
		return nil, fmt.Errorf("oracle: %s is not a loopback address", opts.Addr)
	}
	o := &Oracle{
		opts:      opts,
		ln:        ln,
		static:    static,
		templates: opts.Templates,
		replay:    opts.Replay,
		logf:      opts.Logf,
		plans:     map[string]*Plan{},
		captures:  map[captureKey][]Capture{},
		pending:   map[captureKey]int{},
		conns:     map[net.Conn]struct{}{},
		stop:      make(chan struct{}),
	}
	o.cond = sync.NewCond(&o.mu)
	o.wg.Add(1)
	go o.acceptLoop()
	return o, nil
}

func withDefaults(opts Options) Options {
	if opts.Addr == "" {
		opts.Addr = defaultAddr
	}
	if opts.RepoRoot == "" {
		opts.RepoRoot = "."
	}
	if opts.FixturesDir == "" {
		opts.FixturesDir = filepath.Join(opts.RepoRoot, filepath.FromSlash(defaultFixturesDir))
	}
	if opts.MaxBodyBytes <= 0 {
		opts.MaxBodyBytes = defaultMaxBody
	}
	if opts.TimeoutHold <= 0 {
		opts.TimeoutHold = defaultTimeoutHold
	}
	if opts.Logf == nil {
		opts.Logf = func(string, ...any) {}
	}
	return opts
}

// URL is the proxy URL for parity.oracle_url (http://127.0.0.1:<port>).
func (o *Oracle) URL() string {
	return "http://" + o.ln.Addr().String()
}

// Close stops accepting, closes open connections and waits for handlers.
func (o *Oracle) Close() error {
	o.mu.Lock()
	if o.closed {
		o.mu.Unlock()
		return nil
	}
	o.closed = true
	close(o.stop)
	for conn := range o.conns {
		conn.Close()
	}
	o.cond.Broadcast()
	o.mu.Unlock()
	err := o.ln.Close()
	o.wg.Wait()
	if err != nil && !errors.Is(err, net.ErrClosed) {
		return fmt.Errorf("oracle: closing listener: %w", err)
	}
	return nil
}

// Register installs (or replaces) the plan of plan.CaseID.
func (o *Oracle) Register(plan *Plan) error {
	if plan == nil {
		return errors.New("oracle: nil plan")
	}
	if err := plan.compile(); err != nil {
		return err
	}
	o.mu.Lock()
	o.plans[plan.CaseID] = plan
	o.mu.Unlock()
	return nil
}

// Unregister removes a case's plan; later calls for it get the default sources.
func (o *Oracle) Unregister(caseID string) {
	o.mu.Lock()
	delete(o.plans, caseID)
	o.mu.Unlock()
}

// RegisterCurrency serves rates for one profile and returns the URL to use as
// currency_converter.fetch_url.
func (o *Oracle) RegisterCurrency(profile string, rates map[string]map[string]float64, dataAsOf string) (string, error) {
	p, err := o.static.setProfileRates(profile, rates, dataAsOf)
	if err != nil {
		return "", err
	}
	return o.URL() + p, nil
}

// Captures returns a copy of the captures of (caseID, impl) sorted by
// X-Parity-Meta seq, then arrival. Calls without meta are stored under
// case "" and impl "".
func (o *Oracle) Captures(caseID, impl string) []Capture {
	o.mu.Lock()
	defer o.mu.Unlock()
	return sortedCaptures(o.captures[captureKey{caseID, impl}])
}

// Drain waits until no call of (caseID, impl) is in flight, then removes and
// returns its captures. It returns ctx.Err() with what was captured so far
// when ctx ends first.
func (o *Oracle) Drain(ctx context.Context, caseID, impl string) ([]Capture, error) {
	key := captureKey{caseID, impl}
	done := make(chan struct{})
	defer close(done)
	go func() {
		select {
		case <-ctx.Done():
			o.mu.Lock()
			o.cond.Broadcast()
			o.mu.Unlock()
		case <-done:
		}
	}()
	o.mu.Lock()
	defer o.mu.Unlock()
	for o.pending[key] > 0 && ctx.Err() == nil && !o.closed {
		o.cond.Wait()
	}
	out := sortedCaptures(o.captures[key])
	delete(o.captures, key)
	return out, ctx.Err()
}

// OutboundCalls returns the trace records of captures, for merging into the
// trace's http_out (each has wire: true).
func OutboundCalls(caps []Capture) []parity.OutboundCall {
	out := make([]parity.OutboundCall, 0, len(caps))
	for _, c := range caps {
		out = append(out, c.Call)
	}
	return out
}

func sortedCaptures(in []Capture) []Capture {
	out := append([]Capture(nil), in...)
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Call.Seq != out[j].Call.Seq {
			return out[i].Call.Seq < out[j].Call.Seq
		}
		return out[i].arrival < out[j].arrival
	})
	return out
}

func (o *Oracle) acceptLoop() {
	defer o.wg.Done()
	for {
		conn, err := o.ln.Accept()
		if err != nil {
			if !errors.Is(err, net.ErrClosed) {
				o.logf("[parity oracle] accept: %v", err)
			}
			return
		}
		if !o.track(conn) {
			conn.Close()
			return
		}
		o.wg.Add(1)
		go o.serveConn(conn)
	}
}

// track registers an open connection; false once the oracle is closed.
func (o *Oracle) track(conn net.Conn) bool {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.closed {
		return false
	}
	o.conns[conn] = struct{}{}
	return true
}

func (o *Oracle) untrack(conn net.Conn) {
	o.mu.Lock()
	delete(o.conns, conn)
	o.mu.Unlock()
}

// serveConn serves keep-alive requests on one connection.
func (o *Oracle) serveConn(conn net.Conn) {
	defer o.wg.Done()
	defer o.untrack(conn)
	defer conn.Close()
	br := bufio.NewReaderSize(conn, readBufferSize)
	for {
		req, err := readRequest(br, conn, o.opts.MaxBodyBytes)
		if err != nil {
			if errors.Is(err, errBadRequest) {
				o.logf("[parity oracle] %v", err)
				bad := &wireResponse{status: 400, headers: parity.HeaderList{}}
				// The connection closes right after; a write error changes nothing.
				_ = bad.write(conn, bad.finalHeaders(true), "")
			}
			return
		}
		if !o.handle(conn, br, req) {
			return
		}
	}
}

// handle answers one request and records it; false closes the connection.
func (o *Oracle) handle(conn net.Conn, br *bufio.Reader, req *wireRequest) bool {
	in := newInbound(req)
	key := captureKey{caseID: in.meta.Case, impl: in.meta.Impl}
	o.begin(key)
	defer o.end(key)
	var rep reply
	if req.method == "CONNECT" {
		rep = unexpectedReply("CONNECT tunnels are not supported; the parity transport sends absolute-form http requests")
		rep.status = 405
	} else {
		rep = o.resolve(in, o.planFor(in.meta.Case))
	}
	capture := newCapture(in, rep)
	keep := o.respond(conn, br, req, &rep, &capture) && req.keepAlive() && req.method != "CONNECT"
	o.record(key, capture)
	return keep
}

// begin marks a call of key in flight.
func (o *Oracle) begin(key captureKey) {
	o.mu.Lock()
	o.pending[key]++
	o.mu.Unlock()
}

// planFor returns the registered plan of a case (nil for case "").
func (o *Oracle) planFor(caseID string) *Plan {
	if caseID == "" {
		return nil
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	return o.plans[caseID]
}

func (o *Oracle) end(key captureKey) {
	o.mu.Lock()
	o.pending[key]--
	if o.pending[key] <= 0 {
		delete(o.pending, key)
	}
	o.cond.Broadcast()
	o.mu.Unlock()
}

func (o *Oracle) record(key captureKey, c Capture) {
	o.mu.Lock()
	o.arrivals++
	c.arrival = o.arrivals
	if c.Meta == nil && c.Call.Seq == 0 {
		c.Call.Seq = len(o.captures[key]) + 1
	}
	o.captures[key] = append(o.captures[key], c)
	o.mu.Unlock()
}

// respond performs the reply (delay, failure or response) and completes the
// capture's response half. It returns false when the connection must close.
func (o *Oracle) respond(conn net.Conn, br *bufio.Reader, req *wireRequest, rep *reply, c *Capture) bool {
	if rep.delay > 0 {
		c.Call.Response.DelayMS = int(rep.delay / time.Millisecond)
		if waitOrClientClose(conn, br, rep.delay, o.stop) {
			return o.clientGaveUp(c, "client closed the connection before the delayed response was sent")
		}
	}
	switch rep.fail {
	case parity.OutboundErrTimeout:
		failed(c, rep.fail)
		if !waitOrClientClose(conn, br, o.opts.TimeoutHold, o.stop) {
			c.Call.Notes = append(c.Call.Notes, "timeout hold elapsed before the client gave up")
		}
		return false
	case parity.OutboundErrReset:
		failed(c, rep.fail)
		resetConn(conn)
		return false
	case parity.OutboundErrRefused:
		failed(c, rep.fail)
		c.Call.Notes = append(c.Call.Notes, "connection closed without a response (an accepted connection cannot be refused)")
		return false
	case parity.OutboundErrOther:
		failed(c, rep.fail)
		if err := writeMalformed(conn); err != nil {
			c.Call.Notes = append(c.Call.Notes, err.Error())
		}
		return false
	}
	closing := !req.keepAlive()
	resp := &wireResponse{status: rep.status, headers: rep.headers, body: rep.body}
	headers := resp.finalHeaders(closing)
	c.Call.Response.Headers = headers
	if err := resp.write(conn, headers, req.method); err != nil {
		c.Call.Notes = append(c.Call.Notes, err.Error())
		return false
	}
	return !closing
}

// clientGaveUp records that the client stopped waiting.
func (o *Oracle) clientGaveUp(c *Capture, note string) bool {
	failed(c, parity.OutboundErrTimeout)
	c.Call.Notes = append(c.Call.Notes, note)
	return false
}

// failed turns the capture into a failed call.
func failed(c *Capture, kind string) {
	k := kind
	c.Call.Error = &k
	c.Call.Response = nil
}

// newCapture builds the capture of a request and its planned reply.
func newCapture(in *inbound, rep reply) Capture {
	req := in.req
	call := parity.OutboundCall{
		Seq:     in.meta.Seq,
		Impl:    in.meta.Impl,
		Kind:    in.kind,
		Bidder:  in.meta.Bidder,
		OrigURL: in.origURL(),
		Request: parity.OutboundRequest{
			Method: req.method,
			URL:    req.target,
			Proto:  req.proto,
			Header: in.headers,
			Body:   parity.NewBody(req.body, in.encoding),
		},
		Wire: true,
		Mock: &parity.MockRef{Source: rep.mock.Source, ID: rep.mock.ID},
		Response: &parity.OutboundResponse{
			Status:  rep.status,
			Headers: rep.headers,
			Body:    parity.NewBody(rep.body, responseEncoding(rep.headers)),
		},
		Notes: append([]string(nil), rep.notes...),
	}
	if call.Kind == "" {
		call.Kind = parity.OutboundGeneral
	}
	c := Capture{Call: call, Chunked: req.chunked, Unexpected: rep.unexpected}
	if in.hasMeta && in.metaErr == nil {
		meta := in.meta
		c.Meta = &meta
	}
	if in.metaErr != nil {
		c.MetaError = in.metaErr.Error()
	}
	if in.encoding != "" && in.decodeErr == nil && len(in.decoded) > 0 {
		c.Decoded = parity.NewBody(in.decoded, "")
	}
	if in.decodeErr != nil {
		c.Call.Notes = append(c.Call.Notes, "request body could not be decoded: "+in.decodeErr.Error())
	}
	return c
}

func responseEncoding(h parity.HeaderList) string {
	v, _ := headerValue(h, "Content-Encoding")
	return v
}
