package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

// A drainScenario exercises Server.Shutdown the way server.go's shutdownAfterSignals calls it:
// a fresh server with the main server's settings, on a listener with connection metrics, serves
// conns, each dialed at its own time; Shutdown starts shutdownAt milliseconds in, with a context
// that ends after timeout milliseconds (0: never).
type drainScenario struct {
	name       string
	network    string
	shutdownAt int
	timeout    int
	conns      []drainConn
}

// A drainConn is a wire case dialed at milliseconds into its scenario.
type drainConn struct {
	at int
	wireCase
}

// drainResult is what a scenario observed: each connection, what Serve and Shutdown returned,
// and, on a Unix socket, whether the socket file was left behind.
type drainResult struct {
	conns    []result
	serve    string
	shutdown string
	socket   string
}

func drainScenarios() []drainScenario {
	status := req("GET", "/status")
	hold := func(ms int) string { return req("GET", fmt.Sprintf("/hold/%d", ms)) }
	conn := func(name string, at, wait int, steps ...step) drainConn {
		return drainConn{at: at, wireCase: wireCase{name: name, server: "drain", wait: wait, steps: steps}}
	}
	return []drainScenario{
		{
			name: "graceful", network: "tcp", shutdownAt: 150,
			conns: []drainConn{
				// Idle after its reply: closed when Shutdown starts.
				conn("idle", 0, 400, send(status)),
				// Active: its reply is still written, with Connection: close.
				conn("inflight", 0, 600, send(hold(400))),
				// The second request is never read.
				conn("pipelined", 0, 600, send(hold(400)+status)),
				conn("http10 keep-alive", 0, 600, send(raw("GET /hold/400 HTTP/1.0", []string{"Connection: keep-alive"}, ""))),
				conn("cors in flight", 0, 600, send(req("GET", "/hold/400", "Origin: https://pub.example"))),
				// A new connection is not idle (for 5 seconds): its first request is served.
				conn("silent then request", 0, 400, pause(300), send(status)),
				conn("partial first request", 0, 400, send("GET /status HTTP/1.1\r\nHost: x\r\n"), pause(300), send("\r\n")),
				// Between requests a connection is idle, even with half a request read.
				conn("partial second request", 0, 400, send(status), pause(50), send("GET /status HTTP/1.1\r\nHost: x\r\n"), pause(250), send("\r\n")),
				// The listener is closed.
				conn("late dial", 250, 100),
			},
		},
		{
			name: "deadline", network: "tcp", shutdownAt: 100, timeout: 150,
			conns: []drainConn{
				conn("idle", 0, 300, send(status)),
				// Still active at the deadline: Shutdown gives up, the reply still comes.
				conn("inflight", 0, 800, send(hold(500))),
				conn("late dial", 200, 100),
			},
		},
		{
			name: "unix", network: "unix", shutdownAt: 150,
			conns: []drainConn{
				conn("idle", 0, 400, send(status)),
				conn("inflight", 0, 600, send(hold(400))),
				// The socket file is removed.
				conn("late dial", 250, 100),
			},
		},
	}
}

func runDrainScenarios(dir string, scenarios []drainScenario) ([]drainResult, error) {
	results := make([]drainResult, len(scenarios))
	for i, sc := range scenarios {
		r, err := runDrainScenario(dir, sc)
		if err != nil {
			return nil, fmt.Errorf("scenario %s: %w", sc.name, err)
		}
		results[i] = r
	}
	return results, nil
}

func runDrainScenario(dir string, sc drainScenario) (drainResult, error) {
	address := "127.0.0.1:0"
	if sc.network == "unix" {
		address = filepath.Join(dir, sc.name+".sock")
	}
	l, err := net.Listen(sc.network, address)
	if err != nil {
		return drainResult{}, err
	}
	t := target{network: sc.network, address: l.Addr().String()}
	srv := &http.Server{Handler: newDrainHandler(), ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, ErrorLog: log.New(io.Discard, "", 0)}
	defer srv.Close()
	served := make(chan error, 1)
	go func() { served <- srv.Serve(monitoredListener{l}) }()

	start := time.Now()
	res := drainResult{conns: make([]result, len(sc.conns)), socket: "-"}
	var wg sync.WaitGroup
	for i, c := range sc.conns {
		wg.Add(1)
		go func() {
			defer wg.Done()
			time.Sleep(time.Until(start.Add(time.Duration(c.at) * time.Millisecond)))
			res.conns[i] = runCase(t, c.wireCase)
		}()
	}

	time.Sleep(time.Until(start.Add(time.Duration(sc.shutdownAt) * time.Millisecond)))
	ctx, cancel := context.Background(), context.CancelFunc(func() {})
	if sc.timeout > 0 {
		ctx, cancel = context.WithTimeout(ctx, time.Duration(sc.timeout)*time.Millisecond)
	}
	res.shutdown = errText(srv.Shutdown(ctx))
	cancel()
	res.serve = errText(<-served)
	wg.Wait()
	if sc.network == "unix" {
		res.socket = "kept"
		if _, err := os.Stat(address); errors.Is(err, os.ErrNotExist) {
			res.socket = "removed"
		}
	}
	return res, nil
}

func errText(err error) string {
	if err == nil {
		return "<nil>"
	}
	return err.Error()
}

func sameDrainResults(a, b []drainResult) error {
	for i := range a {
		if a[i].serve != b[i].serve || a[i].shutdown != b[i].shutdown || a[i].socket != b[i].socket {
			return fmt.Errorf("drain scenario %d is not deterministic: %+v %+v", i, a[i], b[i])
		}
		for j := range a[i].conns {
			x, y := a[i].conns[j], b[i].conns[j]
			if !bytes.Equal(x.response, y.response) || x.end != y.end {
				return fmt.Errorf("drain scenario %d connection %d is not deterministic:\n%q %s\n%q %s", i, j, x.response, x.end, y.response, y.end)
			}
		}
	}
	return nil
}

func writeDrainGolden(path string, scenarios []drainScenario, results []drainResult) error {
	var b strings.Builder
	b.WriteString("# Generated by rust/crates/gohttp-server/testdata/go/wire; do not edit.\n")
	b.WriteString("# GOTOOLCHAIN=go1.26.2 go run ./rust/crates/gohttp-server/testdata/go/wire\n")
	for i, sc := range scenarios {
		r := results[i]
		fmt.Fprintf(&b, "\nscenario %s\nnetwork %s\nshutdown %d %d\n", sc.name, sc.network, sc.shutdownAt, sc.timeout)
		for j, c := range sc.conns {
			fmt.Fprintf(&b, "\ncase %s\nat %d\nwait %d\n", c.name, c.at, c.wait)
			for _, s := range c.steps {
				if s.pause > 0 {
					fmt.Fprintf(&b, "pause %d\n", s.pause)
				} else {
					fmt.Fprintf(&b, "send %s\n", strconv.Quote(s.data))
				}
			}
			fmt.Fprintf(&b, "response %s\nend %s\n", strconv.Quote(string(r.conns[j].response)), r.conns[j].end)
		}
		fmt.Fprintf(&b, "\nserve %s\nresult %s\nsocket %s\n", r.serve, r.shutdown, r.socket)
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}
