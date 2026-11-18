// Command wire writes testdata/golden/wire.golden for pbs-gohttp-server: the raw bytes Go's
// net/http server sends back for a generated corpus of raw HTTP/1.x requests (cases.go), with
// github.com/julienschmidt/httprouter and github.com/rs/cors wrapped the way Prebid Server's
// main.go, router/router.go, router/admin.go and server/server.go configure them (handlers.go).
// It also writes testdata/golden/shutdown.golden: what connections in every state see when
// Server.Shutdown runs, and what Serve and Shutdown return (drain.go).
// Run it from the directory of this module (crates/gohttp-server/testdata/go) with the toolchain
// of the production Dockerfile:
//
//	GOTOOLCHAIN=go1.26.2 go run ./wire
//
// The header lines both goldens start with still name the generator by its path in the former
// fork layout (rust/crates/...): they are golden content, kept so the goldens regenerate
// byte for byte.
//
// tests/go_wire replays every case against pbs-gohttp-server serving the same handlers and
// compares the bytes. Date header values and multipart boundaries are normalized; everything else
// is compared exactly, together with how the connection ended: "eof" (the server closed it),
// "reset", or "open" (still open after the case's quiet period). The corpus is run twice and the
// program fails if Go itself answers a case differently the second time.
//
// Both goldens record the GOOS the program ran on ("goos"). A case whose bytes depend on the GOOS
// has one expect block per GOOS the generator knows Go's values for: the block of the GOOS it ran
// on is observed, the others are derived from Go's source (goos.go), and the wire module's test
// checks the block of the GOOS it runs on against real Go.
package main

import (
	"bytes"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
)

// parallel is how many cases run at once.
const parallel = 64

type target struct {
	network, address string
}

type result struct {
	response []byte
	end      string
}

func main() {
	out := flag.String("out", defaultOut("wire.golden"), "wire golden file to write")
	drainOut := flag.String("shutdown-out", defaultOut("shutdown.golden"), "shutdown golden file to write")
	flag.Parse()
	if err := run(*out, *drainOut); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func defaultOut(name string) string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..", "golden", name)
}

func run(out, drainOut string) error {
	dir, err := os.MkdirTemp("", "gohttp-wire")
	if err != nil {
		return err
	}
	defer os.RemoveAll(dir)
	static := filepath.Join(dir, "static")
	if err := writeStatic(static); err != nil {
		return err
	}
	targets, stop, err := startServers(static, filepath.Join(dir, "prebid.sock"))
	if err != nil {
		return err
	}
	defer stop()

	cases := allCases()
	first := runAll(targets, cases)
	second := runAll(targets, cases)
	for i := range cases {
		if !bytes.Equal(first[i].response, second[i].response) || first[i].end != second[i].end {
			return fmt.Errorf("case %q is not deterministic:\n%q %s\n%q %s", cases[i].name,
				first[i].response, first[i].end, second[i].response, second[i].end)
		}
	}
	byGOOS, err := runByGOOS(targets, cases, first)
	if err != nil {
		return err
	}
	if err := writeGolden(out, cases, first, byGOOS); err != nil {
		return err
	}

	scenarios := drainScenarios()
	drained, err := runDrainScenarios(dir, scenarios)
	if err != nil {
		return err
	}
	again, err := runDrainScenarios(dir, scenarios)
	if err != nil {
		return err
	}
	if err := sameDrainResults(drained, again); err != nil {
		return err
	}
	return writeDrainGolden(drainOut, scenarios, drained)
}

// monitoredListener stands for server/listener.go's monitorableListener: its connections
// embed net.Conn only, so the server cannot CloseWrite them (closeWriteAndWait just waits).
// With counts, it counts the RecordConnectionAccept and RecordConnectionClose calls
// monitorableListener makes.
type monitoredListener struct {
	net.Listener
	counts *connCounts
}

type monitoredConn struct {
	net.Conn
	counts *connCounts
}

func (l monitoredListener) Accept() (net.Conn, error) {
	c, err := l.Listener.Accept()
	l.counts.recordAccept(err == nil)
	if err != nil {
		return c, err
	}
	return &monitoredConn{c, l.counts}, nil
}

func (c *monitoredConn) Close() error {
	err := c.Conn.Close()
	c.counts.recordClose(err == nil)
	return err
}

// connCounts counts the metrics calls of a monitoredListener by their success argument.
type connCounts struct {
	accepted, acceptFailed, closed, closeFailed atomic.Int64
}

func (c *connCounts) recordAccept(success bool) {
	switch {
	case c == nil:
	case success:
		c.accepted.Add(1)
	default:
		c.acceptFailed.Add(1)
	}
}

func (c *connCounts) recordClose(success bool) {
	switch {
	case c == nil:
	case success:
		c.closed.Add(1)
	default:
		c.closeFailed.Add(1)
	}
}

func (c *connCounts) String() string {
	return fmt.Sprintf("accepted %d accept_failed %d closed %d close_failed %d",
		c.accepted.Load(), c.acceptFailed.Load(), c.closed.Load(), c.closeFailed.Load())
}

// serverSpec is one server startServers starts. The connections of a copy for a goos report
// what Go reports on that GOOS (goosListener).
type serverSpec struct {
	name      string
	network   string
	address   string
	monitored bool
	goos      string
	srv       *http.Server
}

// startServers starts Prebid's listeners: the main server (15s read and write timeouts, as
// server.go's newMainServer, on a listener with connection metrics), the same handler on a
// Unix socket (newSocketServer, also with metrics), the admin server (no timeouts, no metrics,
// newAdminServer), the main handler on a listener without metrics ("plain"), and a server with
// short timeouts for the timeout cases. For every GOOS of goosValues it also starts a copy of the
// Unix socket server that answers as Go does there ("unix@<goos>").
func startServers(static, socket string) (map[string]target, func(), error) {
	quiet := log.New(io.Discard, "", 0)
	public := newMainHandler(static)
	servers := []serverSpec{
		{"main", "tcp", "127.0.0.1:0", true, "", &http.Server{Handler: public, ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, ErrorLog: quiet}},
		{"unix", "unix", socket, true, "", &http.Server{Handler: public, ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, ErrorLog: quiet}},
		{"plain", "tcp", "127.0.0.1:0", false, "", &http.Server{Handler: public, ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, ErrorLog: quiet}},
		{"admin", "tcp", "127.0.0.1:0", false, "", &http.Server{Handler: newAdminHandler(), ErrorLog: quiet}},
		{"short", "tcp", "127.0.0.1:0", false, "", &http.Server{Handler: newShortHandler(), ReadTimeout: 300 * time.Millisecond, WriteTimeout: time.Second, ErrorLog: quiet}},
	}
	for _, goos := range goosNames() {
		address := strings.TrimSuffix(socket, ".sock") + "-" + goos + ".sock"
		servers = append(servers, serverSpec{goosTarget("unix", goos), "unix", address, true, goos,
			&http.Server{Handler: public, ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, ErrorLog: quiet}})
	}
	targets := map[string]target{}
	var started []*http.Server
	stop := func() {
		for _, s := range started {
			s.Close()
		}
	}
	for _, s := range servers {
		l, err := net.Listen(s.network, s.address)
		if err != nil {
			stop()
			return nil, nil, err
		}
		targets[s.name] = target{network: s.network, address: l.Addr().String()}
		if s.goos != "" {
			l = goosListener{Listener: l, peer: goosValues[s.goos].unnamedUnixPeer}
		}
		if s.monitored {
			l = monitoredListener{Listener: l}
		}
		started = append(started, s.srv)
		go s.srv.Serve(l)
	}
	return targets, stop, nil
}

func runAll(targets map[string]target, cases []wireCase) []result {
	results := make([]result, len(cases))
	sem := make(chan struct{}, parallel)
	var wg sync.WaitGroup
	for i, c := range cases {
		wg.Add(1)
		sem <- struct{}{}
		go func() {
			defer wg.Done()
			defer func() { <-sem }()
			results[i] = runCase(targets[c.server], c)
		}()
	}
	wg.Wait()
	return results
}

// runCase sends the case's steps on a new connection while reading the reply, until the
// server closes the connection or nothing arrives for c.wait milliseconds after the last step.
func runCase(t target, c wireCase) result {
	conn, err := net.Dial(t.network, t.address)
	if err != nil {
		return result{end: "dial error"}
	}
	defer conn.Close()

	var mu sync.Mutex
	var sentAt time.Time
	done := false
	go func() {
		for _, s := range c.steps {
			if s.pause > 0 {
				time.Sleep(time.Duration(s.pause) * time.Millisecond)
				continue
			}
			data := []byte(strings.Repeat(s.data, s.repeat))
			if _, err := conn.Write(data); err != nil {
				break
			}
		}
		mu.Lock()
		sentAt, done = time.Now(), true
		mu.Unlock()
	}()

	wait := time.Duration(c.wait) * time.Millisecond
	var got bytes.Buffer
	buf := make([]byte, 64<<10)
	lastRead := time.Now()
	for {
		mu.Lock()
		quietFrom, finished := sentAt, done
		mu.Unlock()
		if lastRead.After(quietFrom) {
			quietFrom = lastRead
		}
		if finished && time.Since(quietFrom) >= wait {
			return result{response: normalize(got.Bytes()), end: "open"}
		}
		deadline := time.Now().Add(wait)
		if finished {
			deadline = quietFrom.Add(wait)
		}
		conn.SetReadDeadline(deadline)
		n, err := conn.Read(buf)
		got.Write(buf[:n])
		if n > 0 {
			lastRead = time.Now()
		}
		switch {
		case err == nil, errors.Is(err, os.ErrDeadlineExceeded):
		case errors.Is(err, io.EOF):
			return result{response: normalize(got.Bytes()), end: "eof"}
		case errors.Is(err, syscall.ECONNRESET):
			return result{response: normalize(got.Bytes()), end: "reset"}
		default:
			return result{response: normalize(got.Bytes()), end: "error"}
		}
	}
}

// normalize replaces the value of every Date header line (the current time) with <date>, and
// every multipart boundary (random) with <boundary>.
func normalize(b []byte) []byte {
	out := append([]byte(nil), b...)
	const prefix = "\r\nDate: "
	for i := 0; ; {
		j := bytes.Index(out[i:], []byte(prefix))
		if j < 0 {
			break
		}
		start := i + j + len(prefix)
		end := start + len("Mon, 02 Jan 2006 15:04:05 GMT")
		if end+2 <= len(out) && isHTTPDate(out[start:end]) && string(out[end:end+2]) == "\r\n" {
			out = append(out[:start], append([]byte("<date>"), out[end:]...)...)
		}
		i = start
	}
	const marker = "boundary="
	var boundaries []string
	for i := 0; ; {
		j := bytes.Index(out[i:], []byte(marker))
		if j < 0 {
			break
		}
		start := i + j + len(marker)
		end := start + 60
		if end <= len(out) && isLowerHex(out[start:end]) {
			boundaries = append(boundaries, string(out[start:end]))
		}
		i = start
	}
	for _, bd := range boundaries {
		out = bytes.ReplaceAll(out, []byte(bd), []byte("<boundary>"))
	}
	return out
}

// isHTTPDate reports whether b has the shape of http.TimeFormat.
func isHTTPDate(b []byte) bool {
	const shape = "Aaa, 00 Aaa 0000 00:00:00 GMT"
	if len(b) != len(shape) {
		return false
	}
	for i := 0; i < len(b); i++ {
		switch shape[i] {
		case 'A':
			if b[i] < 'A' || b[i] > 'Z' {
				return false
			}
		case 'a':
			if b[i] < 'a' || b[i] > 'z' {
				return false
			}
		case '0':
			if b[i] < '0' || b[i] > '9' {
				return false
			}
		default:
			if b[i] != shape[i] {
				return false
			}
		}
	}
	return true
}

func isLowerHex(b []byte) bool {
	for _, ch := range b {
		if !(ch >= '0' && ch <= '9' || ch >= 'a' && ch <= 'f') {
			return false
		}
	}
	return true
}

// goosHeader is the line of both goldens that records the GOOS the generator ran on.
func goosHeader() string {
	return "goos " + runtime.GOOS + "\n"
}

// writeGolden writes the cases with their results; a case of byGOOS (by its index) gets one
// expect block per GOOS instead.
func writeGolden(path string, cases []wireCase, results []result, byGOOS map[int]map[string]result) error {
	var b strings.Builder
	b.WriteString("# Generated by rust/crates/gohttp-server/testdata/go/wire; do not edit.\n")
	b.WriteString("# GOTOOLCHAIN=go1.26.2 go run ./rust/crates/gohttp-server/testdata/go/wire\n")
	b.WriteString("# A case with expect blocks answers differently by GOOS: the block of the goos below was\n")
	b.WriteString("# observed, the others are derived from Go's source (goos.go of the generator).\n")
	b.WriteString(goosHeader())
	for i, c := range cases {
		fmt.Fprintf(&b, "\ncase %s\nserver %s\nwait %d\n", c.name, c.server, c.wait)
		for _, s := range c.steps {
			switch {
			case s.pause > 0:
				fmt.Fprintf(&b, "pause %d\n", s.pause)
			case s.repeat > 1:
				fmt.Fprintf(&b, "send %s * %d\n", strconv.Quote(s.data), s.repeat)
			default:
				fmt.Fprintf(&b, "send %s\n", strconv.Quote(s.data))
			}
		}
		if perGOOS, ok := byGOOS[i]; ok {
			for _, goos := range goosNames() {
				r := perGOOS[goos]
				fmt.Fprintf(&b, "expect %s\nresponse %s\nend %s\n", goos, strconv.Quote(string(r.response)), r.end)
			}
			continue
		}
		fmt.Fprintf(&b, "response %s\nend %s\n", strconv.Quote(string(results[i].response)), results[i].end)
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}
