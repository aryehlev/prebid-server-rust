package main

import (
	"context"
	"crypto/tls"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/http/httptrace"
	"strings"
	"sync"
	"time"
)

// metricsCase is a run of requests with the httptrace hooks bidder.go installs
// (exchange/bidder.go:732-798), recording which hooks fired for each request, in order.
type metricsCase struct {
	Name string `json:"name"`
	// URL is every request's URL, PORT standing for the server's port.
	URL string `json:"url"`
	// Concurrent says whether the requests went out at once on a fresh client rather than one
	// after the other.
	Concurrent bool `json:"concurrent"`
	// Events are the hooks each request fired: GetConn, DNSStart, DNSDone, ConnectStart,
	// ConnectDone, TLSHandshakeStart, TLSHandshakeDone and GotConn with its Reused flag. Only
	// lists the hooks the case keeps (see Kept).
	Events [][]string `json:"events"`
	// Kept names the hooks recorded; a hostname that resolves to both ::1 and 127.0.0.1 makes the
	// number of Connect hooks depend on the machine, so that case keeps the others only.
	Kept []string `json:"kept"`
}

type metricsDoc struct {
	Header header        `json:"header"`
	Cases  []metricsCase `json:"cases"`
}

var allHooks = []string{"GetConn", "DNSStart", "DNSDone", "ConnectStart", "ConnectDone",
	"TLSHandshakeStart", "TLSHandshakeDone", "GotConn"}

// traced sends one GET to url with the hooks installed and returns the hooks that fired.
func traced(client *http.Client, url string, kept []string) []string {
	var mu sync.Mutex
	var events []string
	record := func(name, detail string) {
		for _, k := range kept {
			if k == name {
				mu.Lock()
				events = append(events, strings.TrimSpace(name+" "+detail))
				mu.Unlock()
			}
		}
	}
	trace := &httptrace.ClientTrace{
		GetConn:           func(string) { record("GetConn", "") },
		GotConn:           func(info httptrace.GotConnInfo) { record("GotConn", fmt.Sprintf("reused=%t", info.Reused)) },
		DNSStart:          func(httptrace.DNSStartInfo) { record("DNSStart", "") },
		DNSDone:           func(httptrace.DNSDoneInfo) { record("DNSDone", "") },
		ConnectStart:      func(string, string) { record("ConnectStart", "") },
		ConnectDone:       func(_, _ string, err error) { record("ConnectDone", fmt.Sprintf("err=%t", err != nil)) },
		TLSHandshakeStart: func() { record("TLSHandshakeStart", "") },
		TLSHandshakeDone:  func(_ tls.ConnectionState, err error) { record("TLSHandshakeDone", fmt.Sprintf("err=%t", err != nil)) },
	}
	req, err := http.NewRequestWithContext(httptrace.WithClientTrace(context.Background(), trace), http.MethodGet, url, nil)
	if err != nil {
		panic(err)
	}
	if _, _, err := roundTrip(client, req); err != nil {
		panic(err)
	}
	mu.Lock()
	defer mu.Unlock()
	return events
}

func metricsGolden() metricsDoc {
	doc := metricsDoc{Header: newHeader()}
	plain := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("ok"))
	}))
	defer plain.Close()
	port := plain.Listener.Addr().String()[strings.LastIndex(plain.Listener.Addr().String(), ":")+1:]

	// Answers after 100ms, so two requests sent at once both dial before either connection is
	// free again.
	slow := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		time.Sleep(100 * time.Millisecond)
		_, _ = w.Write([]byte("ok"))
	}))
	defer slow.Close()
	slowPort := slow.Listener.Addr().String()[strings.LastIndex(slow.Listener.Addr().String(), ":")+1:]

	pki := newTestPKI()
	secure := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("ok"))
	}))
	secure.TLS = &tls.Config{Certificates: []tls.Certificate{pki.leaf}}
	secure.StartTLS()
	defer secure.Close()
	securePort := secure.Listener.Addr().String()[strings.LastIndex(secure.Listener.Addr().String(), ":")+1:]

	run := func(name, url, port string, concurrent bool, kept []string) {
		c := metricsCase{Name: name, URL: url, Concurrent: concurrent, Kept: kept}
		client := &http.Client{Transport: generalTransport(pki.roots)}
		full := strings.ReplaceAll(url, "PORT", port)
		if concurrent {
			c.Events = make([][]string, 2)
			var wg sync.WaitGroup
			for i := range 2 {
				wg.Add(1)
				go func() {
					defer wg.Done()
					c.Events[i] = traced(client, full, kept)
				}()
			}
			wg.Wait()
		} else {
			for range 2 {
				c.Events = append(c.Events, traced(client, full, kept))
			}
		}
		client.CloseIdleConnections()
		doc.Cases = append(doc.Cases, c)
	}
	run("http_ip", "http://127.0.0.1:PORT/m", port, false, allHooks)
	run("http_hostname", "http://localhost:PORT/m", port, false, []string{"GetConn", "DNSStart", "DNSDone", "GotConn"})
	run("https_ip", "https://127.0.0.1:PORT/m", securePort, false, allHooks)
	run("concurrent_new_connections", "http://127.0.0.1:PORT/m", slowPort, true, allHooks)
	return doc
}
