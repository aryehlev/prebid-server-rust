package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"sync"
	"syscall"
	"time"

	"github.com/prebid/prebid-server/v4/config"
)

// shutdownAt is when the shutdown scenario calls Shutdown, in milliseconds after its
// connections start: after the idle connection's answer, halfway through the sleeping handlers
// and before the silent connection writes its request.
const shutdownAt = 500

// shutdownTimeout is the deadline shutdownAfterSignals gives Shutdown (server/server.go:199).
const shutdownTimeout = 10 * time.Second

// shutdownDoc is what a main server does when shutdownAfterSignals stops it while it holds a
// connection in each state: what each connection read, how it ended, whether a connection
// dialled after the shutdown began was accepted, and what Shutdown returned.
type shutdownDoc struct {
	AtMs      int      `json:"at_ms"`
	TimeoutMs int      `json:"timeout_ms"`
	Cases     []result `json:"cases"`
	LateDial  string   `json:"late_dial"`
	Err       string   `json:"err"`
}

// shutdownCases are the connections open when Shutdown is called: one idle after an answer,
// one whose handler is still running, one with a second request pipelined behind a running
// one, one that has sent nothing yet and writes its request after the shutdown began, one whose
// first request head is half written when the shutdown begins, and one that never sends
// anything (net/http treats a connection that has not sent a head as busy for 5 s,
// server.go:3207-3212, and drops a request it reads after the shutdown began, server.go:2000).
func shutdownCases() []wireCase {
	request := func(target string) string {
		return "GET " + target + " HTTP/1.1\r\nHost: " + host + "\r\n\r\n"
	}
	sleep := request(fmt.Sprintf("/sleep?ms=%d", 2*shutdownAt))
	window := 4 * shutdownAt
	return []wireCase{
		{Group: "shutdown", Name: "idle", Server: "shutdown", Steps: []step{text(request("/echo"))}, WindowMs: window},
		{Group: "shutdown", Name: "in_flight", Server: "shutdown", Steps: []step{text(sleep)}, WindowMs: window},
		{Group: "shutdown", Name: "pipelined", Server: "shutdown", Steps: []step{text(sleep + request("/echo"))}, WindowMs: window},
		{Group: "shutdown", Name: "silent", Server: "shutdown", Steps: []step{pause(shutdownAt + shutdownAt/2), text(request("/echo"))}, WindowMs: window},
		{Group: "shutdown", Name: "partial_head", Server: "shutdown", Steps: []step{
			text("GET /echo HTTP/1.1\r\n"), pause(shutdownAt + shutdownAt/2), text("Host: " + host + "\r\n\r\n"),
		}, WindowMs: window},
		{Group: "shutdown", Name: "never_sends", Server: "shutdown", Steps: []step{}, WindowMs: 6 * shutdownAt},
	}
}

// lateDial dials addr and says whether a server accepted the connection.
func lateDial(addr string) string {
	conn, err := net.Dial("tcp", addr)
	switch {
	case err == nil:
		conn.Close()
		return "accepted"
	case errors.Is(err, syscall.ECONNREFUSED):
		return "refused"
	default:
		return err.Error()
	}
}

// shutdownScenario serves the shutdown cases on a main server and stops it as
// shutdownAfterSignals does, shutdownAt after the cases start; a dial halfway between the
// shutdown and the end of the sleeping handlers checks the listener is closed.
func shutdownScenario() shutdownDoc {
	s := newMainServer(&config.Configuration{}, standIns())
	addr := serve(s)
	cases := shutdownCases()
	d := shutdownDoc{
		AtMs:      shutdownAt,
		TimeoutMs: int(shutdownTimeout / time.Millisecond),
		Cases:     make([]result, len(cases)),
	}
	var wg sync.WaitGroup
	for i, c := range cases {
		wg.Add(1)
		go func() {
			defer wg.Done()
			d.Cases[i] = run(addr, c)
		}()
	}
	time.Sleep(shutdownAt * time.Millisecond)
	stopped := make(chan error, 1)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
		defer cancel()
		stopped <- s.Shutdown(ctx)
	}()
	time.Sleep(shutdownAt / 2 * time.Millisecond)
	d.LateDial = lateDial(addr)
	d.Err = fmt.Sprint(<-stopped)
	wg.Wait()
	return d
}
