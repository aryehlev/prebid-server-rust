package main

import (
	"bytes"
	"crypto/tls"
	"encoding/hex"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/net/http2"
	"golang.org/x/net/http2/hpack"
)

// h2Body is the bid request every case sends.
const h2Body = `{"id":"r"}`

// h2ContentType is the Content-Type bidder adapters send with it.
const h2ContentType = "application/json;charset=utf-8"

// h2Doc records the protocol of the bidder client in Go and, at the library level, the HTTP/2
// that pbs-http-client's bidder client speaks in its place:
//
//   - what router.go's Transport (the bidder client) and the same Transport set up as Go's own
//     HTTP/2 client negotiate with each kind of bidder server (selection);
//   - the h2c probe's bytes, as x/net/http2's Framer writes them, and what Go's servers answer
//     them with (probe);
//   - what an HTTP/1 Go server answers a bid request sent with prior knowledge, which is what an
//     h2c client sends to an origin that no longer speaks h2c (answers);
//   - the request Go's HTTP/2 client sends, as a raw HTTP/2 server decodes it (requests);
//   - gamma's bid request, whose adapter adds a connection-specific field, as router.go's
//     Transport writes it over HTTP/1.1 and as Go's HTTP/2 client sends it (gamma).
type h2Doc struct {
	Header    header        `json:"header"`
	Selection []h2Selection `json:"selection"`
	Probe     h2Probe       `json:"probe"`
	Answers   []h2Answer    `json:"answers"`
	Requests  []h2Request   `json:"requests"`
	Gamma     h2Gamma       `json:"gamma"`
}

// h2Selection is two bid requests in turn from one client to one kind of bidder server.
type h2Selection struct {
	// Client is "general" (router.go's Transport, router/router.go:143-158, which sets neither
	// ForceAttemptHTTP2 nor Protocols) or "http2" (that Transport with Protocols set for Go's
	// own HTTP/2 client: HTTP1 and HTTP2 over TLS, UnencryptedHTTP2 alone in cleartext, which is
	// prior knowledge).
	Client string `json:"client"`
	// Server is "https_h2" (TLS offering h2 and http/1.1 in ALPN), "https_http1" (TLS offering
	// http/1.1), "h2c" (cleartext HTTP/1 and unencrypted HTTP/2 on one port, Protocols HTTP1 and
	// UnencryptedHTTP2) or "http1" (cleartext HTTP/1 only, a HandlerFunc answering every
	// request).
	Server string `json:"server"`
	// Protos is the protocol each response came on (Response.Proto), or the request's error.
	Protos []string `json:"protos"`
	// Handled are the requests the server's handler was called with: method, request-target,
	// protocol and User-Agent.
	Handled []string `json:"handled"`
	// Connections is how many connections the server accepted.
	Connections int `json:"connections"`
}

// h2Probe is the h2c probe: what it writes, and what Go's servers answer.
type h2Probe struct {
	// Hello is what the probe writes on connecting, in hex: the client connection preface and
	// an empty SETTINGS frame.
	Hello string `json:"hello"`
	// Goodbye is the GOAWAY frame it writes once the server's SETTINGS has come, in hex: last
	// stream 0, NO_ERROR, no debug data.
	Goodbye string `json:"goodbye"`
	// Servers are the answers of Go's servers.
	Servers []h2ProbeAnswer `json:"servers"`
}

// h2ProbeAnswer is what one Go server answered the probe with.
type h2ProbeAnswer struct {
	// Server is "h2c" (Protocols HTTP1 and UnencryptedHTTP2), "http1_mux" (HTTP/1 only, a
	// ServeMux) or "http1_handler" (HTTP/1 only, a HandlerFunc answering every request).
	Server string `json:"server"`
	// First is the first nine bytes of the answer, in hex: from an h2c server, the header of a
	// SETTINGS frame on stream 0.
	First string `json:"first"`
	// Settings is the payload of that SETTINGS frame, in hex.
	Settings string `json:"settings,omitempty"`
	// Answer is an HTTP/1 server's whole answer up to its close, its Date values written as
	// DATE.
	Answer string `json:"answer,omitempty"`
	// Closed says the server closed the connection: an h2c server after the GOAWAY and the
	// probe's half-close, an HTTP/1 server on its own.
	Closed bool `json:"closed"`
	// Handled are the requests the server's handler was called with.
	Handled []string `json:"handled"`
}

// h2Answer is what an HTTP/1 Go server answers a bid request sent with prior knowledge: the
// client preface, an empty SETTINGS frame, and the request's HEADERS and DATA frames, written at
// once as an HTTP/2 client writes them.
type h2Answer struct {
	// Server is "http1_mux" or "http1_handler", as in the probe.
	Server string `json:"server"`
	// First is the first nine bytes of the answer, in hex, which an HTTP/2 client reads as a
	// frame header.
	First string `json:"first"`
	// Answer is the whole answer up to the server's close, its Date values written as DATE.
	Answer string `json:"answer"`
	// Closed says the server closed the connection.
	Closed bool `json:"closed"`
	// Handled are the requests the server's handler was called with: never the bid request.
	Handled []string `json:"handled"`
}

// h2Request is the bid request Go's HTTP/2 client sends (the "http2" client of the selection),
// as a raw HTTP/2 server decodes its HEADERS and DATA frames.
type h2Request struct {
	// Transport is "h2" (TLS, ALPN h2) or "h2c" (cleartext, prior knowledge).
	Transport string `json:"transport"`
	// Pseudo are the pseudo-header fields in the order they came, "name: value", the port
	// written as PORT.
	Pseudo []string `json:"pseudo"`
	// Fields are the other header fields, "name: value", sorted: Go writes them in the order
	// of its Header map, which is random.
	Fields []string `json:"fields"`
	// Body is the request body.
	Body string `json:"body"`
}

// h2Gamma is gamma's bid request: the adapter adds Connection: keep-alive
// (adapters/gamma/gamma.go:134), a connection-specific field, which HTTP/2 forbids (RFC 9113
// section 8.2.2).
type h2Gamma struct {
	// Field is the header line the adapter adds.
	Field string `json:"field"`
	// HTTP1 is the request head router.go's Transport writes over HTTP/1.1, line by line as a raw
	// server reads it, the port written as PORT: Request.write writes the field.
	HTTP1 []string `json:"http1"`
	// Requests are the requests Go's HTTP/2 client sends with it over h2 and h2c, as Requests
	// records them: they carry no connection-specific field (encodeHeaders drops them).
	Requests []h2Request `json:"requests"`
}

func h2Golden() h2Doc {
	pki := newTestPKI()
	return h2Doc{
		Header:    newHeader("golang.org/x/net"),
		Selection: h2Selections(pki),
		Probe:     h2ProbeGolden(),
		Answers:   h2Answers(),
		Requests:  []h2Request{h2RequestGolden("h2", pki, bidRequest), h2RequestGolden("h2c", pki, bidRequest)},
		Gamma:     h2GammaGolden(pki),
	}
}

// h2Target is a loopback server of one kind that records the requests its handler is called
// with and counts the connections it accepted.
type h2Target struct {
	srv     *httptest.Server
	mu      sync.Mutex
	handled []string
	conns   int
}

// newH2Target starts a server of kind: see h2Selection.Server and h2ProbeAnswer.Server.
func newH2Target(kind string, pki testPKI) *h2Target {
	t := &h2Target{}
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		t.mu.Lock()
		t.handled = append(t.handled, r.Method+" "+r.RequestURI+" "+r.Proto+" ua="+r.UserAgent()+" body="+strconv.Itoa(len(body)))
		t.mu.Unlock()
		_, _ = w.Write([]byte("ok"))
	})
	if kind == "http1_mux" {
		mux := http.NewServeMux()
		mux.Handle("/", handler)
		t.srv = httptest.NewUnstartedServer(mux)
	} else {
		t.srv = httptest.NewUnstartedServer(handler)
	}
	t.srv.Config.ConnState = func(_ net.Conn, state http.ConnState) {
		if state == http.StateNew {
			t.mu.Lock()
			t.conns++
			t.mu.Unlock()
		}
	}
	switch kind {
	case "https_h2":
		t.srv.EnableHTTP2 = true
		t.srv.TLS = &tls.Config{Certificates: []tls.Certificate{pki.leaf}, NextProtos: []string{"h2", "http/1.1"}}
		t.srv.StartTLS()
	case "https_http1":
		t.srv.TLS = &tls.Config{Certificates: []tls.Certificate{pki.leaf}, NextProtos: []string{"http/1.1"}}
		t.srv.StartTLS()
	case "h2c":
		t.srv.Config.Protocols = new(http.Protocols)
		t.srv.Config.Protocols.SetHTTP1(true)
		t.srv.Config.Protocols.SetUnencryptedHTTP2(true)
		t.srv.Start()
	default:
		t.srv.Config.Protocols = new(http.Protocols)
		t.srv.Config.Protocols.SetHTTP1(true)
		t.srv.Start()
	}
	return t
}

// stop closes the server and returns what its handler was called with and the connections it
// accepted.
func (t *h2Target) stop() ([]string, int) {
	t.srv.Close()
	t.mu.Lock()
	defer t.mu.Unlock()
	return nonNil(t.handled), t.conns
}

// h2Transport is router.go's Transport, or for client "http2" that Transport set up as Go's
// HTTP/2 client for server.
func h2Transport(client, server string, pki testPKI) *http.Transport {
	transport := generalTransport(pki.roots)
	if client == "http2" {
		transport.Protocols = new(http.Protocols)
		if strings.HasPrefix(server, "https") {
			transport.Protocols.SetHTTP1(true)
			transport.Protocols.SetHTTP2(true)
		} else {
			transport.Protocols.SetUnencryptedHTTP2(true)
		}
	}
	return transport
}

// bidRequest is a POST of h2Body to url with the Content-Type adapters set.
func bidRequest(url string) *http.Request {
	req, err := http.NewRequest(http.MethodPost, url, strings.NewReader(h2Body))
	if err != nil {
		panic(err)
	}
	req.Header.Set("Content-Type", h2ContentType)
	return req
}

// gammaField is the connection-specific header line gamma's adapter adds
// (adapters/gamma/gamma.go:134).
const gammaField = "Connection: keep-alive"

// gammaRequest is a bid request to url carrying gammaField, as gamma's adapter sends it.
func gammaRequest(url string) *http.Request {
	req := bidRequest(url)
	name, value, _ := strings.Cut(gammaField, ": ")
	req.Header.Add(name, value)
	return req
}

func h2Selections(pki testPKI) []h2Selection {
	var cases []h2Selection
	for _, server := range []string{"https_h2", "https_http1", "h2c", "http1"} {
		for _, client := range []string{"general", "http2"} {
			target := newH2Target(server, pki)
			transport := h2Transport(client, server, pki)
			c := h2Selection{Client: client, Server: server}
			for range 2 {
				resp, _, err := roundTrip(&http.Client{Transport: transport}, bidRequest(target.srv.URL+"/bid"))
				if err != nil {
					c.Protos = append(c.Protos, withPorts(err.Error(), map[string]string{target.port(): "PORT"}))
					continue
				}
				c.Protos = append(c.Protos, resp.Proto)
			}
			transport.CloseIdleConnections()
			c.Handled, c.Connections = target.stop()
			cases = append(cases, c)
		}
	}
	return cases
}

// port is the server's port.
func (t *h2Target) port() string {
	_, port, err := net.SplitHostPort(t.srv.Listener.Addr().String())
	if err != nil {
		panic(err)
	}
	return port
}

// frameBytes is what write writes with a Framer of its own.
func frameBytes(write func(fr *http2.Framer) error) []byte {
	var buf bytes.Buffer
	if err := write(http2.NewFramer(&buf, nil)); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

// probeHello is the client preface and an empty SETTINGS frame.
func probeHello() []byte {
	settings := frameBytes(func(fr *http2.Framer) error { return fr.WriteSettings() })
	return append([]byte(http2.ClientPreface), settings...)
}

// dates matches the values of Date header lines.
var dates = regexp.MustCompile(`(?m)^Date: [^\r\n]*`)

// withoutDates writes every Date value of an HTTP/1 answer as DATE.
func withoutDates(answer string) string {
	return dates.ReplaceAllString(answer, "Date: DATE")
}

// dialTarget connects to target with a deadline that bounds the whole exchange.
func dialTarget(target *h2Target) *net.TCPConn {
	conn, err := net.DialTimeout("tcp", target.srv.Listener.Addr().String(), 5*time.Second)
	if err != nil {
		panic(err)
	}
	if err := conn.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		panic(err)
	}
	return conn.(*net.TCPConn)
}

func h2ProbeGolden() h2Probe {
	goodbye := frameBytes(func(fr *http2.Framer) error { return fr.WriteGoAway(0, http2.ErrCodeNo, nil) })
	probe := h2Probe{Hello: hex.EncodeToString(probeHello()), Goodbye: hex.EncodeToString(goodbye)}
	for _, server := range []string{"h2c", "http1_mux", "http1_handler"} {
		target := newH2Target(server, testPKI{})
		conn := dialTarget(target)
		if _, err := conn.Write(probeHello()); err != nil {
			panic(err)
		}
		first := make([]byte, 9)
		if _, err := io.ReadFull(conn, first); err != nil {
			panic(err)
		}
		answer := h2ProbeAnswer{Server: server, First: hex.EncodeToString(first)}
		if server == "h2c" {
			settings := make([]byte, int(first[0])<<16|int(first[1])<<8|int(first[2]))
			if _, err := io.ReadFull(conn, settings); err != nil {
				panic(err)
			}
			answer.Settings = hex.EncodeToString(settings)
			if _, err := conn.Write(goodbye); err != nil {
				panic(err)
			}
			if err := conn.CloseWrite(); err != nil {
				panic(err)
			}
			_, err := io.Copy(io.Discard, conn)
			answer.Closed = err == nil
		} else {
			rest, err := io.ReadAll(conn)
			answer.Answer = withoutDates(string(first) + string(rest))
			answer.Closed = err == nil
		}
		if err := conn.Close(); err != nil {
			panic(err)
		}
		answer.Handled, _ = target.stop()
		probe.Servers = append(probe.Servers, answer)
	}
	return probe
}

// priorKnowledgeRequest is a bid request as an HTTP/2 client writes it to a connection it
// believes speaks h2c: the preface, an empty SETTINGS frame, and the request on stream 1.
func priorKnowledgeRequest(authority string) []byte {
	var block bytes.Buffer
	enc := hpack.NewEncoder(&block)
	for _, field := range []hpack.HeaderField{
		{Name: ":method", Value: http.MethodPost},
		{Name: ":scheme", Value: "http"},
		{Name: ":authority", Value: authority},
		{Name: ":path", Value: "/bid"},
		{Name: "content-type", Value: h2ContentType},
		{Name: "user-agent", Value: "Go-http-client/1.1"},
		{Name: "accept-encoding", Value: "gzip"},
		{Name: "content-length", Value: strconv.Itoa(len(h2Body))},
	} {
		if err := enc.WriteField(field); err != nil {
			panic(err)
		}
	}
	frames := frameBytes(func(fr *http2.Framer) error {
		if err := fr.WriteHeaders(http2.HeadersFrameParam{StreamID: 1, BlockFragment: block.Bytes(), EndHeaders: true}); err != nil {
			return err
		}
		return fr.WriteData(1, true, []byte(h2Body))
	})
	return append(probeHello(), frames...)
}

func h2Answers() []h2Answer {
	var answers []h2Answer
	for _, server := range []string{"http1_mux", "http1_handler"} {
		target := newH2Target(server, testPKI{})
		conn := dialTarget(target)
		if _, err := conn.Write(priorKnowledgeRequest(target.srv.Listener.Addr().String())); err != nil {
			panic(err)
		}
		all, err := io.ReadAll(conn)
		if err := conn.Close(); err != nil {
			panic(err)
		}
		answer := h2Answer{Server: server, Answer: withoutDates(string(all)), Closed: err == nil}
		answer.First = hex.EncodeToString(all[:min(9, len(all))])
		answer.Handled, _ = target.stop()
		answers = append(answers, answer)
	}
	return answers
}

// h2RequestGolden sends the bid request newRequest makes with Go's HTTP/2 client over transport
// ("h2" or "h2c") to a raw HTTP/2 server, which decodes it and answers 200.
func h2RequestGolden(transport string, pki testPKI, newRequest func(url string) *http.Request) h2Request {
	var ln net.Listener
	var err error
	if transport == "h2" {
		ln, err = tls.Listen("tcp", "127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{pki.leaf}, NextProtos: []string{"h2"}})
	} else {
		ln, err = net.Listen("tcp", "127.0.0.1:0")
	}
	if err != nil {
		panic(err)
	}
	_, port, err := net.SplitHostPort(ln.Addr().String())
	if err != nil {
		panic(err)
	}
	got := make(chan h2Request, 1)
	done := make(chan struct{})
	go func() {
		defer close(done)
		conn, err := ln.Accept()
		if err != nil {
			panic(err)
		}
		defer conn.Close()
		serveH2Request(conn, transport, port, got)
	}()
	scheme, server := "http", "h2c"
	if transport == "h2" {
		scheme, server = "https", "https_h2"
	}
	client := h2Transport("http2", server, pki)
	resp, body, err := roundTrip(&http.Client{Transport: client}, newRequest(scheme+"://127.0.0.1:"+port+"/bid"))
	if err != nil {
		panic(err)
	}
	if resp.Proto != "HTTP/2.0" || string(body) != "ok" {
		panic("unexpected answer " + resp.Proto + " " + string(body))
	}
	request := <-got
	client.CloseIdleConnections()
	<-done
	if err := ln.Close(); err != nil {
		panic(err)
	}
	return request
}

// h2GammaGolden sends gamma's bid request with router.go's Transport over HTTP/1.1 to a raw
// server, which reads its head and answers 200, and with Go's HTTP/2 client over h2 and h2c.
func h2GammaGolden(pki testPKI) h2Gamma {
	heads := make(chan []string, 1)
	server := newRawServer(func(c *rawConn) {
		req, ok := c.readRequest()
		if !ok {
			return
		}
		heads <- req.head
		c.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
		c.drain()
	})
	transport := generalTransport(pki.roots)
	resp, body, err := roundTrip(&http.Client{Transport: transport}, gammaRequest("http://"+server.addr()+"/bid"))
	if err != nil {
		panic(err)
	}
	if resp.Proto != "HTTP/1.1" || string(body) != "ok" {
		panic("unexpected answer " + resp.Proto + " " + string(body))
	}
	gamma := h2Gamma{Field: gammaField}
	for _, line := range <-heads {
		gamma.HTTP1 = append(gamma.HTTP1, withPorts(line, map[string]string{server.port: "PORT"}))
	}
	transport.CloseIdleConnections()
	server.close()
	gamma.Requests = []h2Request{h2RequestGolden("h2", pki, gammaRequest), h2RequestGolden("h2c", pki, gammaRequest)}
	return gamma
}

// serveH2Request serves one HTTP/2 connection: it reads the preface, sends an empty SETTINGS
// frame, acknowledges the client's, decodes the first request, hands it to got and answers it 200
// with the body "ok", then reads until the client goes.
func serveH2Request(conn net.Conn, transport, port string, got chan<- h2Request) {
	preface := make([]byte, len(http2.ClientPreface))
	if _, err := io.ReadFull(conn, preface); err != nil || string(preface) != http2.ClientPreface {
		panic("no client preface")
	}
	fr := http2.NewFramer(conn, conn)
	fr.ReadMetaHeaders = hpack.NewDecoder(4096, nil)
	if err := fr.WriteSettings(); err != nil {
		panic(err)
	}
	request := h2Request{Transport: transport, Pseudo: []string{}, Fields: []string{}}
	var body []byte
	var answered bool
	for {
		frame, err := fr.ReadFrame()
		if err != nil {
			if !answered {
				panic(err)
			}
			return
		}
		var ended bool
		switch f := frame.(type) {
		case *http2.SettingsFrame:
			if !f.IsAck() {
				if err := fr.WriteSettingsAck(); err != nil {
					panic(err)
				}
			}
		case *http2.MetaHeadersFrame:
			for _, field := range f.Fields {
				text := field.Name + ": " + withPorts(field.Value, map[string]string{port: "PORT"})
				if field.IsPseudo() {
					request.Pseudo = append(request.Pseudo, text)
				} else {
					request.Fields = append(request.Fields, text)
				}
			}
			ended = f.StreamEnded()
		case *http2.DataFrame:
			body = append(body, f.Data()...)
			ended = f.StreamEnded()
		}
		if ended && !answered {
			answered = true
			slices.Sort(request.Fields)
			request.Body = string(body)
			got <- request
			var block bytes.Buffer
			enc := hpack.NewEncoder(&block)
			for _, field := range []hpack.HeaderField{{Name: ":status", Value: "200"}, {Name: "content-length", Value: "2"}} {
				if err := enc.WriteField(field); err != nil {
					panic(err)
				}
			}
			if err := fr.WriteHeaders(http2.HeadersFrameParam{StreamID: frame.Header().StreamID, BlockFragment: block.Bytes(), EndHeaders: true}); err != nil {
				panic(err)
			}
			if err := fr.WriteData(frame.Header().StreamID, true, []byte("ok")); err != nil {
				panic(err)
			}
		}
	}
}
