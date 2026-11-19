package main

import (
	"context"
	"io"
	"net/http"
	"sync"
	"time"

	"golang.org/x/net/context/ctxhttp"
)

// limitCase is one run of requests against loopback servers that count the connections they
// accept.
type limitCase struct {
	Name string `json:"name"`
	// Settings are the Transport fields the case sets on router.go's general Transport, as
	// config.go's keys name them.
	Settings map[string]int `json:"settings"`
	// Statuses are the responses' statuses in request order (0 for an error) and Errors their
	// error texts.
	Statuses []int    `json:"statuses"`
	Errors   []string `json:"errors"`
	// Accepted is the number of connections each server accepted, MaxOpen the most a server had
	// open at once.
	Accepted []int `json:"accepted"`
	MaxOpen  []int `json:"max_open"`
	// Serialized says whether the concurrent requests took at least their number times the
	// server's delay, which they do only when they ran one after the other.
	Serialized *bool `json:"serialized,omitempty"`
	// Timeout says whether the error is a deadline under bidder.go's test
	// (exchange/bidder.go:619), and WithinLimit whether it came after the limit the case names
	// and before twice that.
	Timeout     *bool `json:"timeout,omitempty"`
	WithinLimit *bool `json:"within_limit,omitempty"`
}

type limitsDoc struct {
	Header header      `json:"header"`
	Cases  []limitCase `json:"cases"`
}

// keepAliveServer answers every request on a connection with 200 "ok" after delay.
func keepAliveServer(delay time.Duration) *rawServer {
	return newRawServer(func(conn *rawConn) {
		for {
			if _, ok := conn.readRequest(); !ok {
				return
			}
			time.Sleep(delay)
			conn.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
		}
	})
}

// limitTransport is router.go's general Transport with the case's settings.
func limitTransport(settings map[string]int) *http.Transport {
	t := generalTransport(nil)
	for key, value := range settings {
		switch key {
		case "max_connections_per_host":
			t.MaxConnsPerHost = value
		case "max_idle_connections":
			t.MaxIdleConns = value
		case "max_idle_connections_per_host":
			t.MaxIdleConnsPerHost = value
		case "idle_connection_timeout_seconds":
			t.IdleConnTimeout = time.Duration(value) * time.Second
		case "tls_handshake_timeout_seconds":
			t.TLSHandshakeTimeout = time.Duration(value) * time.Second
		default:
			panic("unknown setting " + key)
		}
	}
	return t
}

func get(client *http.Client, url string) (int, string) {
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		panic(err)
	}
	resp, _, err := roundTrip(client, req)
	if err != nil {
		return 0, err.Error()
	}
	return resp.StatusCode, ""
}

func boolPtr(b bool) *bool { return &b }

// concurrentCase sends n requests at once to a server that answers each after delay.
func concurrentCase(name string, settings map[string]int, n int) limitCase {
	const delay = 100 * time.Millisecond
	srv := keepAliveServer(delay)
	client := &http.Client{Transport: limitTransport(settings)}
	c := limitCase{Name: name, Settings: settings, Statuses: make([]int, n), Errors: make([]string, n)}
	start := time.Now()
	var wg sync.WaitGroup
	for i := range n {
		wg.Add(1)
		go func() {
			defer wg.Done()
			c.Statuses[i], c.Errors[i] = get(client, "http://"+srv.addr()+"/l")
		}()
	}
	wg.Wait()
	c.Serialized = boolPtr(time.Since(start) >= time.Duration(n)*delay)
	client.CloseIdleConnections()
	srv.close()
	accepted, maxOpen := srv.counts()
	c.Accepted, c.MaxOpen = []int{accepted}, []int{maxOpen}
	return c
}

// sequenceCase sends one GET to each server in order, sleeping pause before each.
func sequenceCase(name string, settings map[string]int, servers int, order []int, pause time.Duration) limitCase {
	var srvs []*rawServer
	for range servers {
		srvs = append(srvs, keepAliveServer(0))
	}
	client := &http.Client{Transport: limitTransport(settings)}
	c := limitCase{Name: name, Settings: settings}
	for i, n := range order {
		if i > 0 {
			time.Sleep(pause)
		}
		status, err := get(client, "http://"+srvs[n].addr()+"/l")
		c.Statuses = append(c.Statuses, status)
		c.Errors = append(c.Errors, err)
	}
	client.CloseIdleConnections()
	for _, s := range srvs {
		s.close()
		accepted, maxOpen := s.counts()
		c.Accepted = append(c.Accepted, accepted)
		c.MaxOpen = append(c.MaxOpen, maxOpen)
	}
	return c
}

// handshakeCase sends an https request to a server that accepts the connection and never answers
// the ClientHello, with the TLS handshake timeout of settings and a context deadline of deadline
// (none when zero); limit is the bound the case expects the error within.
func handshakeCase(name string, settings map[string]int, deadline, limit time.Duration) limitCase {
	srv := newRawServer(func(conn *rawConn) { conn.drain() })
	client := &http.Client{Transport: limitTransport(settings)}
	c := limitCase{Name: name, Settings: settings}
	req, err := http.NewRequest(http.MethodGet, "https://"+srv.addr()+"/l", nil)
	if err != nil {
		panic(err)
	}
	ctx := context.Background()
	if deadline > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, deadline)
		defer cancel()
	}
	start := time.Now()
	// The bidder's call (exchange/bidder.go:616).
	resp, err := ctxhttp.Do(ctx, client, req)
	elapsed := time.Since(start)
	if resp != nil {
		_, _ = io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
		c.Statuses = []int{resp.StatusCode}
	} else {
		c.Statuses = []int{0}
	}
	c.Errors = []string{withPorts(errText(err), map[string]string{srv.port: "PORT"})}
	c.Timeout = boolPtr(isDeadline(err))
	c.WithinLimit = boolPtr(elapsed >= limit && elapsed < 2*limit)
	client.CloseIdleConnections()
	srv.close()
	accepted, maxOpen := srv.counts()
	c.Accepted, c.MaxOpen = []int{accepted}, []int{maxOpen}
	return c
}

func limitsGolden() limitsDoc {
	return limitsDoc{Header: newHeader("golang.org/x/net"), Cases: []limitCase{
		// MaxConnsPerHost blocks further dials until a connection is free.
		concurrentCase("max_conns_per_host", map[string]int{"max_connections_per_host": 1}, 3),
		concurrentCase("unlimited", map[string]int{}, 3),
		// MaxIdleConns caps idle connections across hosts: the second server's idle connection
		// pushes the first's out, so the third request dials again.
		sequenceCase("max_idle_global", map[string]int{"max_idle_connections": 1}, 2, []int{0, 1, 0}, 0),
		sequenceCase("idle_timeout", map[string]int{"idle_connection_timeout_seconds": 1}, 1, []int{0, 0}, 1500*time.Millisecond),
		sequenceCase("no_idle_timeout", map[string]int{"idle_connection_timeout_seconds": 0}, 1, []int{0, 0}, 1500*time.Millisecond),
		sequenceCase("no_idle_per_host", map[string]int{"max_idle_connections_per_host": -1}, 1, []int{0, 0}, 0),
		// TLSHandshakeTimeout bounds a handshake the server never answers.
		handshakeCase("tls_handshake_timeout", map[string]int{"tls_handshake_timeout_seconds": 1}, 0, time.Second),
		// A deadline shorter than the handshake timeout ends it first.
		handshakeCase("handshake_within_deadline", map[string]int{}, 300*time.Millisecond, 300*time.Millisecond),
	}}
}
