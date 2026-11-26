package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"regexp"
	"strings"
	"sync"
	"time"

	pbscurrency "github.com/prebid/prebid-server/v4/currency"
)

// fetch.golden records the currency fetch as main.go:72 builds it: RateConverter.Run with Go's
// &http.Client{} (http.DefaultTransport: the proxy environment, no client timeout,
// Go-http-client/1.1, gzip), against loopback servers that answer as each scenario says. It
// holds the error Run returns, the lines Prebid's logger writes (glog's text after its header),
// the request heads the servers read and the bytes they answered with, so that the Rust tests
// (tests/fetch.rs) serve the same bytes. A port reads PORT.
//
// The proxy environment is set before the first request, as the server's process has it:
// HTTP_PROXY names a loopback proxy, which only the proxy scenario's host goes through, since
// Go never proxies a loopback address.

// fetchRates is the currency file the servers answer with.
const fetchRates = `{"dataAsOf":"2018-09-12","conversions":{"USD":{"GBP":0.77208},"GBP":{"USD":1.2952}}}`

// fetchAnswer is what a server does on one connection once it has read the request's head:
// it writes write, then, when wait is set, waits for the client to close the connection;
// then it closes the connection.
type fetchAnswer struct {
	write string
	wait  bool
}

type fetchScenario struct {
	name string
	// The answers, one per connection, the last one repeated. None: nothing listens.
	answers []fetchAnswer
	// The URL, where PORT is the server's port.
	url string
	// The converter's httpTimeout.
	timeout time.Duration
	// The converter's staleRatesThreshold.
	stale time.Duration
	// How many times Run is called.
	runs int
}

func respond(status int, headers, body string) fetchAnswer {
	return fetchAnswer{write: fmt.Sprintf("HTTP/1.1 %d %s\r\n%sContent-Length: %d\r\nConnection: close\r\n\r\n%s",
		status, http.StatusText(status), headers, len(body), body)}
}

// partial declares a longer body than it sends.
func partial(status, declared int, body string, wait bool) fetchAnswer {
	return fetchAnswer{write: fmt.Sprintf("HTTP/1.1 %d %s\r\nContent-Length: %d\r\n\r\n%s",
		status, http.StatusText(status), declared, body), wait: wait}
}

func gzipped(text string) string {
	var buf bytes.Buffer
	w := gzip.NewWriter(&buf)
	if _, err := w.Write([]byte(text)); err != nil {
		panic(err)
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	return buf.String()
}

const fetchURL = "http://127.0.0.1:PORT/latest.json"

func fetchScenarios() []fetchScenario {
	ok := respond(200, "", fetchRates)
	notFound := respond(404, "", `{"message": "Not Found"}`)
	scenarios := []fetchScenario{
		{name: "ok", answers: []fetchAnswer{ok}},
		{name: "status_399", answers: []fetchAnswer{respond(399, "", fetchRates)}},
		{name: "status_404", answers: []fetchAnswer{notFound}},
		{name: "status_500_empty", answers: []fetchAnswer{respond(500, "", "")}},
		{name: "empty_body", answers: []fetchAnswer{respond(200, "", "")}},
		{name: "invalid_json", answers: []fetchAnswer{respond(200, "", `{"message": Invalid-JSON-No-Surrounding-Quotes}`)}},
		{name: "gzip", answers: []fetchAnswer{respond(200, "Content-Encoding: gzip\r\n", gzipped(fetchRates))}},
		{name: "gzip_corrupt", answers: []fetchAnswer{respond(200, "Content-Encoding: gzip\r\n", "not gzip")}},
		{name: "body_cut_short", answers: []fetchAnswer{partial(200, 100, `{"conversions":`, false)}},
		{name: "body_cut_short_500", answers: []fetchAnswer{partial(500, 100, `{"message":`, false)}},
		{name: "timeout_awaiting_headers", answers: []fetchAnswer{{wait: true}}, timeout: 300 * time.Millisecond},
		{name: "timeout_reading_body", answers: []fetchAnswer{partial(200, 100, `{"conversions":`, true)}, timeout: 300 * time.Millisecond},
		{name: "timeout_reading_body_500", answers: []fetchAnswer{partial(500, 100, `{"message":`, true)}, timeout: 300 * time.Millisecond},
		{name: "server_closes", answers: []fetchAnswer{{}}},
		{name: "malformed_response", answers: []fetchAnswer{{write: "garbage\r\n\r\n"}}},
		{name: "connection_refused"},
		{name: "relative_url", url: "invalidurl"},
		{name: "unparsable_url", url: "http://[::1"},
		{name: "unsupported_scheme", url: "ftp://127.0.0.1/latest.json"},
		{name: "stale_fallback", answers: []fetchAnswer{ok, notFound}, stale: time.Nanosecond, runs: 2},
		{name: "kept_rates", answers: []fetchAnswer{ok, notFound}, stale: time.Hour, runs: 2},
		{name: "proxy", url: "http://currency.test/latest.json"},
		// Fetch URLs net/url and net/http read otherwise than the url crate and reqwest:
		// credentials, which Go sends as Basic authorization and masks in its errors
		// (currency.fetch-credentials); white space and control characters around or inside
		// the URL, which url.Parse refuses or escapes, a backslash, missing slashes, a malformed
		// escape and dot segments (currency.fetch-url); a control character in a URL neither
		// parses, which %q quotes (currency.fetch-error-texts); and credentials through the
		// proxy.
		{name: "url_credentials", answers: []fetchAnswer{ok}, url: "http://user:p%40ss@127.0.0.1:PORT/latest.json"},
		{name: "url_credentials_refused", url: "http://user:secret@127.0.0.1:PORT/latest.json"},
		{name: "url_surrounding_space", answers: []fetchAnswer{ok}, url: " http://127.0.0.1:PORT/latest.json "},
		{name: "url_trailing_space", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/latest.json "},
		{name: "url_trailing_newline", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/latest.json\n"},
		{name: "url_tab", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/lat\test.json"},
		{name: "url_backslash", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/rates\\latest.json"},
		{name: "url_no_slashes", answers: []fetchAnswer{ok}, url: "http:127.0.0.1:PORT/latest.json"},
		{name: "url_bad_escape", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/%zz.json"},
		{name: "url_dot_segments", answers: []fetchAnswer{ok}, url: "http://127.0.0.1:PORT/rates/../latest.json"},
		{name: "url_control_character", url: "http://[::1\x01]/latest.json"},
		{name: "proxy_credentials", url: "http://user:pass@currency.test/latest.json"},
	}
	for i := range scenarios {
		if scenarios[i].url == "" {
			scenarios[i].url = fetchURL
		}
		if scenarios[i].timeout == 0 {
			scenarios[i].timeout = 5 * time.Second
		}
		if scenarios[i].runs == 0 {
			scenarios[i].runs = 1
		}
	}
	return scenarios
}

// viaProxy reports whether a scenario's request goes through the proxy: its host is not a
// loopback address, so the proxy environment applies, and the proxy answers.
func viaProxy(sc fetchScenario) bool {
	return strings.HasPrefix(sc.name, "proxy")
}

// rawServer answers each connection with the next of its answers and keeps the request heads.
type rawServer struct {
	ln    net.Listener
	mu    sync.Mutex
	heads []string
	wg    sync.WaitGroup
}

func startRawServer(answers []fetchAnswer) *rawServer {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	srv := &rawServer{ln: ln}
	srv.wg.Add(1)
	go func() {
		defer srv.wg.Done()
		for n := 0; ; n++ {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			a := answers[min(n, len(answers)-1)]
			srv.wg.Add(1)
			go func() {
				defer srv.wg.Done()
				defer conn.Close()
				head := readHead(conn)
				srv.mu.Lock()
				srv.heads = append(srv.heads, head)
				srv.mu.Unlock()
				if _, err := io.WriteString(conn, a.write); err != nil {
					panic(err)
				}
				if a.wait {
					if err := conn.SetReadDeadline(time.Now().Add(10 * time.Second)); err != nil {
						panic(err)
					}
					_, _ = io.Copy(io.Discard, conn)
				}
			}()
		}
	}()
	return srv
}

func (srv *rawServer) port() string {
	return fmt.Sprint(srv.ln.Addr().(*net.TCPAddr).Port)
}

func (srv *rawServer) close() []string {
	if err := srv.ln.Close(); err != nil {
		panic(err)
	}
	srv.wg.Wait()
	return srv.heads
}

// readHead reads a request's head, through its blank line.
func readHead(conn net.Conn) string {
	r := bufio.NewReader(conn)
	var head strings.Builder
	for {
		line, err := r.ReadString('\n')
		head.WriteString(line)
		if err != nil || line == "\r\n" {
			return head.String()
		}
	}
}

// closedPort returns a loopback port nothing listens on.
func closedPort() string {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	port := fmt.Sprint(ln.Addr().(*net.TCPAddr).Port)
	if err := ln.Close(); err != nil {
		panic(err)
	}
	return port
}

var glogHeader = regexp.MustCompile(`^([IWEF])\d{4} \d{2}:\d{2}:\d{2}\.\d{6} +\d+ [^ \]]+:\d+\] `)

// captureLogs runs f with glog's stderr going to a pipe and returns the lines written, each as
// its severity letter and its message.
func captureLogs(f func()) [][2]string {
	saved := os.Stderr
	r, w, err := os.Pipe()
	if err != nil {
		panic(err)
	}
	done := make(chan []byte)
	go func() {
		data, err := io.ReadAll(r)
		if err != nil {
			panic(err)
		}
		done <- data
	}()
	os.Stderr = w
	f()
	os.Stderr = saved
	if err := w.Close(); err != nil {
		panic(err)
	}
	data := <-done
	if err := r.Close(); err != nil {
		panic(err)
	}
	var lines [][2]string
	for _, line := range strings.SplitAfter(string(data), "\n") {
		if line == "" {
			continue
		}
		m := glogHeader.FindStringSubmatch(line)
		if m == nil {
			panic(fmt.Sprintf("not a glog line: %q", line))
		}
		lines = append(lines, [2]string{m[1], strings.TrimSuffix(line[len(m[0]):], "\n")})
	}
	return lines
}

var loopbackPorts = regexp.MustCompile(`127\.0\.0\.1:\d+`)

// withPorts writes every loopback port as PORT.
func withPorts(text string) string {
	return loopbackPorts.ReplaceAllString(text, "127.0.0.1:PORT")
}

func writeFetch(dir string) {
	g := newGolden(dir, "fetch.golden")
	defer g.close()

	if err := flag.Set("logtostderr", "true"); err != nil {
		panic(err)
	}
	scenarios := fetchScenarios()
	var proxyAnswers []fetchAnswer
	for _, sc := range scenarios {
		if viaProxy(sc) {
			proxyAnswers = []fetchAnswer{respond(200, "", fetchRates)}
		}
	}
	proxy := startRawServer(proxyAnswers)
	for name, value := range map[string]string{
		"HTTP_PROXY": "http://127.0.0.1:" + proxy.port(), "http_proxy": "",
		"HTTPS_PROXY": "", "https_proxy": "", "NO_PROXY": "", "no_proxy": "", "REQUEST_METHOD": "",
	} {
		if err := os.Setenv(name, value); err != nil {
			panic(err)
		}
	}
	g.line("proxy", hx("http://127.0.0.1:PORT"))

	for _, sc := range scenarios {
		answers := sc.answers
		if viaProxy(sc) {
			answers = proxyAnswers
		}
		g.line("scenario", sc.name, hx(sc.url), fmt.Sprint(sc.timeout.Milliseconds()), fmt.Sprint(int64(sc.stale)), fmt.Sprint(sc.runs))
		for i, a := range answers {
			wait := "0"
			if a.wait {
				wait = "1"
			}
			g.line("answer", sc.name, fmt.Sprint(i), wait, hx(a.write))
		}
		var srv *rawServer
		port := closedPort()
		if len(sc.answers) > 0 {
			srv = startRawServer(sc.answers)
			port = srv.port()
		}
		url := strings.ReplaceAll(sc.url, "PORT", port)
		proxy.mu.Lock()
		proxied := len(proxy.heads)
		proxy.mu.Unlock()
		rc := pbscurrency.NewRateConverter(&http.Client{}, sc.timeout, url, sc.stale)
		for run := 0; run < sc.runs; run++ {
			var runErr error
			logs := captureLogs(func() { runErr = rc.Run() })
			if runErr == nil {
				g.line("result", sc.name, fmt.Sprint(run), "ok")
			} else {
				g.line("result", sc.name, fmt.Sprint(run), "err", hx(withPorts(runErr.Error())))
			}
			for _, l := range logs {
				g.line("log", sc.name, fmt.Sprint(run), l[0], hx(withPorts(l[1])))
			}
		}
		_, constant := rc.Rates().(*pbscurrency.ConstantRates)
		g.line("rates", sc.name, fmt.Sprint(constant))
		var heads []string
		if srv != nil {
			heads = srv.close()
		}
		if viaProxy(sc) {
			proxy.mu.Lock()
			heads = append(heads, proxy.heads[proxied:]...)
			proxy.mu.Unlock()
		}
		for i, head := range heads {
			g.line("head", sc.name, fmt.Sprint(i), hx(withPorts(head)))
		}
	}
	proxy.close()
}
