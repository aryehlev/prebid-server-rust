package main

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
)

// hop is one request of a redirect chain as the server saw it.
type hop struct {
	Method string `json:"method"`
	// Host is the Host header and Target the request-target, ports written as PA and PB.
	Host   string `json:"host"`
	Target string `json:"target"`
	// Headers are every header the request carried, under net/http's canonical names, ports
	// written as PA and PB.
	Headers map[string][]string `json:"headers"`
	Body    string              `json:"body"`
}

// redirectStep is the answer the server gives to one path: Status, with Location when it is not
// empty. Every other path is answered 200 with the body "done".
type redirectStep struct {
	Path     string `json:"path"`
	Status   int    `json:"status"`
	Location string `json:"location"`
}

// redirectCase is one request sent through a client with the default CheckRedirect
// (net/http/client.go: defaultCheckRedirect and Client.do), and every hop it made.
type redirectCase struct {
	Name   string `json:"name"`
	Method string `json:"method"`
	// URL is the first request's URL; PA and PB stand for the two servers' ports and every host
	// name reaches 127.0.0.1.
	URL     string         `json:"url"`
	Body    string         `json:"body,omitempty"`
	Headers []headerOp     `json:"headers"`
	Steps   []redirectStep `json:"steps"`
	// Loop answers every /r/N with a 302 to /r/N+1.
	Loop bool  `json:"loop,omitempty"`
	Hops []hop `json:"hops"`
	// Status is the final response's status (0 when Do returned an error), Error the error.
	Status int    `json:"status"`
	Error  string `json:"error"`
}

type redirectsDoc struct {
	Header header         `json:"header"`
	Cases  []redirectCase `json:"cases"`
}

func redirectCases() []redirectCase {
	const body = `{"id":"r"}`
	const start = "http://origin.test:PA/start"
	base := func() []headerOp {
		return []headerOp{
			{"set", "X-Custom", "1"},
			{"set", "Authorization", "Bearer secret"},
			{"set", "Cookie", "c=1"},
		}
	}
	with := func(ops ...headerOp) []headerOp { return append(base(), ops...) }
	to := func(location string) []redirectStep {
		return []redirectStep{{Path: "/start", Status: http.StatusFound, Location: location}}
	}
	var cases []redirectCase
	for _, method := range []string{http.MethodGet, http.MethodPost} {
		for _, code := range []int{301, 302, 303, 307, 308} {
			c := redirectCase{
				Name:    fmt.Sprintf("%s_%d", strings.ToLower(method), code),
				Method:  method,
				URL:     start,
				Headers: base(),
				Steps:   []redirectStep{{Path: "/start", Status: code, Location: "/final"}},
			}
			if method == http.MethodPost {
				c.Body = body
				c.Headers = with(headerOp{"set", "Content-Type", "application/json"})
			}
			cases = append(cases, c)
		}
	}
	return append(cases,
		redirectCase{Name: "hop_limit", Method: http.MethodGet, URL: "http://origin.test:PA/r/0", Headers: base(), Loop: true},
		redirectCase{Name: "cross_host", Method: http.MethodGet, URL: start, Headers: base(), Steps: to("http://other.test:PA/final")},
		redirectCase{Name: "subdomain", Method: http.MethodGet, URL: start, Headers: base(), Steps: to("http://sub.origin.test:PA/final")},
		redirectCase{Name: "port_change", Method: http.MethodGet, URL: start, Headers: base(), Steps: to("http://origin.test:PB/final")},
		redirectCase{Name: "cross_host_and_back", Method: http.MethodGet, URL: start, Headers: base(), Steps: []redirectStep{
			{Path: "/start", Status: http.StatusFound, Location: "http://other.test:PA/mid"},
			{Path: "/mid", Status: http.StatusFound, Location: "http://origin.test:PA/final"},
		}},
		redirectCase{Name: "post_307_cross_host", Method: http.MethodPost, URL: start, Body: body,
			Headers: with(headerOp{"set", "Content-Type", "application/json"}),
			Steps:   []redirectStep{{Path: "/start", Status: http.StatusTemporaryRedirect, Location: "http://other.test:PA/final"}}},
		redirectCase{Name: "explicit_referer", Method: http.MethodGet, URL: start,
			Headers: with(headerOp{"set", "Referer", "http://explicit.test/"}), Steps: to("/final")},
		redirectCase{Name: "put_302", Method: http.MethodPut, URL: start, Body: body,
			Headers: with(headerOp{"set", "Content-Type", "application/json"}), Steps: to("/final")},
		redirectCase{Name: "get_302_body_headers", Method: http.MethodGet, URL: start,
			Headers: with(headerOp{"set", "Content-Type", "application/json"}, headerOp{"set", "Content-Language", "en"}),
			Steps:   to("/final")},
		redirectCase{Name: "no_location", Method: http.MethodGet, URL: start, Headers: base(), Steps: to("")},
	)
}

// redirectServer serves one case on two loopback listeners and records every hop.
type redirectServer struct {
	c     *redirectCase
	ports map[string]string
	mu    sync.Mutex
	hops  []hop
}

func (s *redirectServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		panic(err)
	}
	h := hop{
		Method:  r.Method,
		Host:    withPorts(r.Host, s.ports),
		Target:  r.RequestURI,
		Headers: map[string][]string{},
		Body:    string(body),
	}
	for name, values := range r.Header {
		for _, v := range values {
			h.Headers[name] = append(h.Headers[name], withPorts(v, s.ports))
		}
	}
	s.mu.Lock()
	s.hops = append(s.hops, h)
	s.mu.Unlock()
	if s.c.Loop {
		if n, err := strconv.Atoi(strings.TrimPrefix(r.URL.Path, "/r/")); err == nil {
			w.Header().Set("Location", "/r/"+strconv.Itoa(n+1))
			w.WriteHeader(http.StatusFound)
			return
		}
	}
	for _, step := range s.c.Steps {
		if step.Path == r.URL.Path {
			if step.Location != "" {
				location := step.Location
				for port, name := range s.ports {
					location = strings.ReplaceAll(location, ":"+name, ":"+port)
				}
				w.Header().Set("Location", location)
			}
			w.WriteHeader(step.Status)
			return
		}
	}
	_, _ = io.WriteString(w, "done")
}

func redirectsGolden() redirectsDoc {
	doc := redirectsDoc{Header: newHeader()}
	for _, c := range redirectCases() {
		lnA, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			panic(err)
		}
		lnB, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			panic(err)
		}
		pa := strconv.Itoa(lnA.Addr().(*net.TCPAddr).Port)
		pb := strconv.Itoa(lnB.Addr().(*net.TCPAddr).Port)
		handler := &redirectServer{c: &c, ports: map[string]string{pa: "PA", pb: "PB"}}
		srvA := &http.Server{Handler: handler}
		srvB := &http.Server{Handler: handler}
		go func() { _ = srvA.Serve(lnA) }()
		go func() { _ = srvB.Serve(lnB) }()

		url := strings.NewReplacer(":PA", ":"+pa, ":PB", ":"+pb).Replace(c.URL)
		var req *http.Request
		if c.Body != "" {
			req, err = http.NewRequest(c.Method, url, strings.NewReader(c.Body))
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
		transport.DialContext = dialPort
		resp, _, err := roundTrip(&http.Client{Transport: transport}, req)
		if resp != nil {
			c.Status = resp.StatusCode
		}
		c.Error = withPorts(errText(err), handler.ports)
		transport.CloseIdleConnections()
		if err := srvA.Shutdown(context.Background()); err != nil {
			panic(err)
		}
		if err := srvB.Shutdown(context.Background()); err != nil {
			panic(err)
		}
		c.Hops = handler.hops
		doc.Cases = append(doc.Cases, c)
	}
	return doc
}
