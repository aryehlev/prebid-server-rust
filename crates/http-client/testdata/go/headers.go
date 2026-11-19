package main

import (
	"net/http"
	"strings"
)

// headerOp is one change a caller makes to a request's headers.
type headerOp struct {
	// Op is set (Header.Set), add (Header.Add) or raw (the value appended to the map under the
	// name as written, which Header.Set and Header.Add would canonicalise; adapters build
	// http.Header literals that way).
	Op    string `json:"op"`
	Name  string `json:"name"`
	Value string `json:"value"`
}

// apply makes the change on h.
func (op headerOp) apply(h http.Header) {
	switch op.Op {
	case "set":
		h.Set(op.Name, op.Value)
	case "add":
		h.Add(op.Name, op.Value)
	case "raw":
		h[op.Name] = append(h[op.Name], op.Value)
	default:
		panic("unknown header op " + op.Op)
	}
}

// headerCase is one request whose head a loopback server records.
type headerCase struct {
	Name   string `json:"name"`
	Method string `json:"method"`
	// URL is the request URL, PORT standing for the capture server's port. The client dials the
	// capture server whatever host the URL names.
	URL     string     `json:"url"`
	Body    string     `json:"body,omitempty"`
	Headers []headerOp `json:"headers"`
	// Head is the request line and the header lines the server read, in order, the server's port
	// written as PORT.
	Head []string `json:"head"`
}

type headersDoc struct {
	Header header       `json:"header"`
	Cases  []headerCase `json:"cases"`
}

func headerCases() []headerCase {
	const local = "http://127.0.0.1:PORT/p?q=1"
	const body = `{"id":"r"}`
	return []headerCase{
		// The currency and vendor-list fetches: a GET with no header set.
		{Name: "get_bare", Method: http.MethodGet, URL: local},
		// A bidder request as most adapters build it (adapters/appnexus/appnexus.go).
		{Name: "post_json", Method: http.MethodPost, URL: local, Body: body, Headers: []headerOp{
			{"set", "Content-Type", "application/json;charset=utf-8"},
			{"set", "Accept", "application/json"},
		}},
		{Name: "caller_user_agent", Method: http.MethodGet, URL: local, Headers: []headerOp{
			{"set", "User-Agent", "Mozilla/5.0 (pbs test)"},
		}},
		// An adapter that copies an empty device.ua (adapters/vidoomy/vidoomy.go:70).
		{Name: "empty_user_agent", Method: http.MethodGet, URL: local, Headers: []headerOp{
			{"set", "User-Agent", ""},
		}},
		{Name: "two_user_agents", Method: http.MethodGet, URL: local, Headers: []headerOp{
			{"add", "User-Agent", "first"},
			{"add", "User-Agent", "second"},
		}},
		// Names through Header.Add and Header.Set (canonicalised) and put into the map directly
		// (sent as written).
		{Name: "header_case", Method: http.MethodPost, URL: local, Body: body, Headers: []headerOp{
			{"add", "x-openrtb-version", "2.5"},
			{"set", "dnt", "1"},
			{"raw", "x-raw-lower", "1"},
			{"raw", "X-RAW-UPPER", "2"},
			{"add", "Content-Type", "application/json"},
		}},
		{Name: "header_order", Method: http.MethodPost, URL: local, Body: body, Headers: []headerOp{
			{"add", "Zeta", "z"},
			{"add", "X-Forwarded-For", "203.0.113.7"},
			{"add", "Accept-Language", "en"},
			{"add", "Content-Type", "application/json"},
			{"add", "Alpha", "a"},
		}},
		{Name: "multi_value", Method: http.MethodGet, URL: local, Headers: []headerOp{
			{"add", "X-Multi", "a"},
			{"add", "X-Multi", "b"},
		}},
		// A Host entry in the header map (adapters/boldwin_rapid/boldwin_rapid.go:89).
		{Name: "host_header_entry", Method: http.MethodGet, URL: local, Headers: []headerOp{
			{"add", "Host", "other.test"},
		}},
		// The Host header as the endpoint URL writes it: upper case and the default port.
		{Name: "host_as_written", Method: http.MethodGet, URL: "http://Bidder.Example.COM:80/p"},
		{Name: "host_idna", Method: http.MethodGet, URL: "http://bücher.test:PORT/p"},
	}
}

func headersGolden() headersDoc {
	doc := headersDoc{Header: newHeader()}
	for _, c := range headerCases() {
		heads := make(chan []string, 1)
		srv := newRawServer(func(conn *rawConn) {
			req, ok := conn.readRequest()
			if !ok {
				panic("headers: no request")
			}
			conn.write("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
			heads <- req.head
		})
		var body *strings.Reader
		if c.Body != "" {
			body = strings.NewReader(c.Body)
		}
		var req *http.Request
		var err error
		url := strings.ReplaceAll(c.URL, "PORT", srv.port)
		if body != nil {
			req, err = http.NewRequest(c.Method, url, body)
		} else {
			req, err = http.NewRequest(c.Method, url, nil)
		}
		if err != nil {
			panic(err)
		}
		for _, op := range c.Headers {
			op.apply(req.Header)
		}
		transport := generalTransport(nil)
		transport.DialContext = dialTo(srv.addr())
		if _, _, err := roundTrip(&http.Client{Transport: transport}, req); err != nil {
			panic(err)
		}
		head := <-heads
		srv.close()
		for i, line := range head {
			head[i] = withPorts(line, map[string]string{srv.port: "PORT"})
		}
		c.Head = head
		doc.Cases = append(doc.Cases, c)
	}
	return doc
}
