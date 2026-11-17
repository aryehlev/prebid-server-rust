package main

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptrace"
	"os"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
)

// slotNames are the listeners every case can script; P is also the proxy in the environment.
var slotNames = []string{"A", "B", "C", "P"}

// proxyEnv is the proxy environment of the run, with {{P}} standing for the P listener.
var proxyEnv = map[string]string{
	"HTTP_PROXY":  "http://user:pass@{{P}}",
	"HTTPS_PROXY": "http://tunnel:secret@{{P}}",
	"NO_PROXY":    "",
}

// runEnv is the state shared by every case of a run.
type runEnv struct {
	pki   *pki
	slots map[string]*slot
	// addrs maps placeholder names to the current case's addresses.
	addrs map[string]string
}

func newRunEnv(p *pki) (*runEnv, error) {
	env := &runEnv{pki: p, slots: map[string]*slot{}}
	for _, name := range slotNames {
		s, err := newSlot(name, env)
		if err != nil {
			return nil, err
		}
		env.slots[name] = s
	}
	env.resetAddrs()
	for _, k := range []string{"http_proxy", "https_proxy", "no_proxy", "REQUEST_METHOD"} {
		if err := os.Unsetenv(k); err != nil {
			return nil, err
		}
	}
	for k, v := range proxyEnv {
		if err := os.Setenv(k, string(env.subst([]byte(v)))); err != nil {
			return nil, err
		}
	}
	return env, nil
}

func (env *runEnv) resetAddrs() {
	env.addrs = map[string]string{}
	for name, s := range env.slots {
		env.addrs[name] = s.addr
	}
}

// subst replaces {{NAME}} placeholders with addresses.
func (env *runEnv) subst(b []byte) []byte {
	for name, addr := range env.addrs {
		b = bytes.ReplaceAll(b, []byte("{{"+name+"}}"), []byte(addr))
	}
	return b
}

var (
	loopbackRE = regexp.MustCompile(`127\.0\.0\.1:[0-9]+`)
	nowRE      = regexp.MustCompile(`current time [0-9T:+Z.-]+ is`)
)

// normBytes replaces addresses with their placeholders: the case's servers by name, any other
// loopback address (the client side of a connection) by {{LOCAL}}.
func (env *runEnv) normBytes(b []byte) []byte {
	names := map[string]string{}
	for name, addr := range env.addrs {
		names[addr] = name
	}
	b = loopbackRE.ReplaceAllFunc(b, func(m []byte) []byte {
		if name, ok := names[string(m)]; ok {
			return []byte("{{" + name + "}}")
		}
		return []byte("{{LOCAL}}")
	})
	return nowRE.ReplaceAll(b, []byte("current time {{NOW}} is"))
}

func (env *runEnv) norm(s string) string { return string(env.normBytes([]byte(s))) }

func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

func traceErr(err error) string {
	if err == nil {
		return "<nil>"
	}
	return err.Error()
}

// tracer records httptrace events in the order they happen.
type tracer struct {
	mu sync.Mutex
	ev []string
}

func (t *tracer) add(format string, args ...any) {
	t.mu.Lock()
	t.ev = append(t.ev, fmt.Sprintf(format, args...))
	t.mu.Unlock()
}

func (t *tracer) events() []string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return append([]string{}, t.ev...)
}

func (t *tracer) trace() *httptrace.ClientTrace {
	return &httptrace.ClientTrace{
		GetConn: func(hostPort string) { t.add("get_conn %s", hostPort) },
		GotConn: func(info httptrace.GotConnInfo) {
			t.add("got_conn reused=%v idle=%v", info.Reused, info.WasIdle)
		},
		PutIdleConn:          func(err error) { t.add("put_idle %s", traceErr(err)) },
		GotFirstResponseByte: func() { t.add("first_byte") },
		DNSStart:             func(info httptrace.DNSStartInfo) { t.add("dns_start %s", info.Host) },
		DNSDone: func(info httptrace.DNSDoneInfo) {
			addrs := make([]string, 0, len(info.Addrs))
			for _, a := range info.Addrs {
				addrs = append(addrs, a.String())
			}
			t.add("dns_done [%s] %s", strings.Join(addrs, " "), traceErr(info.Err))
		},
		ConnectStart: func(network, addr string) { t.add("connect_start %s %s", network, addr) },
		ConnectDone: func(network, addr string, err error) {
			t.add("connect_done %s %s %s", network, addr, traceErr(err))
		},
		TLSHandshakeStart: func() { t.add("tls_start") },
		TLSHandshakeDone: func(cs tls.ConnectionState, err error) {
			t.add("tls_done complete=%v version=%04x %s", cs.HandshakeComplete, cs.Version, traceErr(err))
		},
		WroteHeaders: func() { t.add("wrote_headers") },
		WroteRequest: func(info httptrace.WroteRequestInfo) { t.add("wrote_request %s", traceErr(info.Err)) },
	}
}

func ms(n int) time.Duration { return time.Duration(n) * time.Millisecond }

// newClient builds the client the way router.go builds Prebid Server's clients.
func newClient(cc Client, env *runEnv) *http.Client {
	tr := &http.Transport{
		Proxy: http.ProxyFromEnvironment,
		DialContext: (&net.Dialer{
			Timeout:   ms(cc.DialTimeoutMs),
			KeepAlive: ms(cc.KeepAliveMs),
		}).DialContext,
		MaxConnsPerHost:       cc.MaxConnsPerHost,
		MaxIdleConns:          cc.MaxIdleConns,
		MaxIdleConnsPerHost:   cc.MaxIdleConnsPerHost,
		IdleConnTimeout:       ms(cc.IdleConnTimeoutMs),
		TLSHandshakeTimeout:   ms(cc.TLSHandshakeTimeoutMs),
		ExpectContinueTimeout: ms(cc.ExpectContinueTimeoutMs),
		ResponseHeaderTimeout: ms(cc.ResponseHeaderTimeoutMs),
		DisableCompression:    cc.DisableCompression,
		DisableKeepAlives:     cc.DisableKeepAlives,
	}
	if cc.TLSConfig {
		pool := x509.NewCertPool()
		if cc.TrustCA {
			pool = env.pki.pool
		}
		tr.TLSClientConfig = &tls.Config{RootCAs: pool}
	}
	client := &http.Client{Transport: tr, Timeout: ms(cc.TimeoutMs)}
	switch cc.Redirect {
	case "use_last":
		client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	case "error":
		client.CheckRedirect = func(*http.Request, []*http.Request) error { return errors.New("redirect refused") }
	}
	return client
}

// pieces is a request body of unknown length that returns one piece per Read; its first Read
// waits for delay.
type pieces struct {
	p     [][]byte
	delay time.Duration
}

func (r *pieces) Read(b []byte) (int, error) {
	if r.delay > 0 {
		time.Sleep(r.delay)
		r.delay = 0
	}
	if len(r.p) == 0 {
		return 0, io.EOF
	}
	n := copy(b, r.p[0])
	r.p[0] = r.p[0][n:]
	if len(r.p[0]) == 0 {
		r.p = r.p[1:]
	}
	return n, nil
}

func headerText(h http.Header) string {
	if h == nil {
		return "<nil>"
	}
	keys := make([]string, 0, len(h))
	for k := range h {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var b strings.Builder
	for _, k := range keys {
		for _, v := range h[k] {
			b.WriteString(k + ": " + v + "\n")
		}
	}
	return b.String()
}

func runStep(client *http.Client, st Step, env *runEnv) Result {
	t := &tracer{}
	ctx := httptrace.WithClientTrace(context.Background(), t.trace())
	if st.Req.CtxTimeoutMs > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, ms(st.Req.CtxTimeoutMs))
		defer cancel()
	}
	var body io.Reader
	switch st.Req.Body.Kind {
	case "nobody":
		body = http.NoBody
	case "bytes":
		body = bytes.NewReader(st.Req.Body.Data)
	case "reader":
		p := &pieces{delay: ms(st.Req.Body.DelayMs)}
		for _, piece := range st.Req.Body.Pieces {
			p.p = append(p.p, append([]byte{}, piece...))
		}
		body = p
	}
	res := Result{Trace: []string{}, TransferEncoding: []string{}}
	req, err := http.NewRequestWithContext(ctx, st.Req.Method, string(env.subst([]byte(st.Req.URL))), body)
	if err != nil {
		res.Err = "new: " + env.norm(err.Error())
		return res
	}
	for _, h := range st.Req.Header {
		req.Header.Add(h[0], string(env.subst([]byte(h[1]))))
	}
	for _, h := range st.Req.RawHeader {
		req.Header[h[0]] = append(req.Header[h[0]], h[1])
	}
	if st.Req.ContentLength != nil {
		req.ContentLength = *st.Req.ContentLength
	}
	if st.Req.Host != "" {
		req.Host = string(env.subst([]byte(st.Req.Host)))
	}
	if st.Req.MethodOverride != "" {
		req.Method = st.Req.MethodOverride
	}
	if st.Req.NoGetBody {
		req.GetBody = nil
	}
	req.Close = st.Req.Close
	if len(st.Req.TransferEncoding) > 0 {
		req.TransferEncoding = st.Req.TransferEncoding
	}
	if len(st.Req.Trailer) > 0 {
		req.Trailer = http.Header{}
		for _, h := range st.Req.Trailer {
			req.Trailer.Add(h[0], h[1])
		}
	}
	resp, err := client.Do(req)
	res.Err = env.norm(errText(err))
	if resp != nil {
		res.Status = resp.Status
		res.Proto = resp.Proto
		res.Header = env.norm(headerText(resp.Header))
		res.ContentLength = resp.ContentLength
		if resp.TransferEncoding != nil {
			res.TransferEncoding = resp.TransferEncoding
		}
		res.Close = resp.Close
		res.Uncompressed = resp.Uncompressed
		if st.Read == "all" {
			b, err := io.ReadAll(resp.Body)
			res.Body = b
			res.BodyErr = env.norm(errText(err))
		}
		if err := resp.Body.Close(); err != nil {
			res.BodyErr += " close: " + env.norm(err.Error())
		}
		res.Trailer = env.norm(headerText(resp.Trailer))
		if resp.Request != nil && resp.Request.URL != nil {
			res.URL = env.norm(resp.Request.URL.String())
		}
		if resp.TLS != nil {
			res.TLS = fmt.Sprintf("complete=%v version=%04x proto=%q",
				resp.TLS.HandshakeComplete, resp.TLS.Version, resp.TLS.NegotiatedProtocol)
		}
	}
	if st.SettleMs > 0 {
		time.Sleep(ms(st.SettleMs))
	}
	for _, e := range t.events() {
		res.Trace = append(res.Trace, env.norm(e))
	}
	return res
}

// run executes one case and fills in its results and captures.
func run(c Case, env *runEnv) (Case, error) {
	env.resetAddrs()
	for i := range c.Servers {
		srv := &c.Servers[i]
		if srv.Closed {
			addr, err := refusedAddr()
			if err != nil {
				return c, err
			}
			env.addrs[srv.Name] = addr
			continue
		}
		s, ok := env.slots[srv.Name]
		if !ok {
			return c, fmt.Errorf("no listener %q", srv.Name)
		}
		s.start(srv)
	}
	client := newClient(c.Client, env)
	c.Results = make([]Result, len(c.Steps))
	var wg sync.WaitGroup
	for i, st := range c.Steps {
		if st.SleepMs > 0 {
			time.Sleep(ms(st.SleepMs))
		}
		if st.Async {
			wg.Add(1)
			go func() {
				defer wg.Done()
				c.Results[i] = runStep(client, st, env)
			}()
			continue
		}
		c.Results[i] = runStep(client, st, env)
	}
	wg.Wait()
	client.CloseIdleConnections()
	c.Captured = map[string][]Bytes{}
	for _, srv := range c.Servers {
		if !srv.Closed {
			c.Captured[srv.Name] = env.slots[srv.Name].finish(3 * time.Second)
		}
	}
	return c, nil
}
