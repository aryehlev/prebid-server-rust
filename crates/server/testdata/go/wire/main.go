// Command wire writes testdata/golden/wire.json for pbs-server: the bytes Prebid Server's
// servers answer on the wire, as server/server.go builds them (newMainServer, newSocketServer
// and newAdminServer, called through go:linkname, with Prebid's gziphandler wrapping when
// compression.response.enable_gzip is set), to requests at the edges of net/http: header order,
// case and framing, sniffed content types, the header limit, malformed requests, the HTTP/2
// preface, the read and write budgets, gzip's Vary and http.FileServer behind router.go's
// /static route. It also records the budgets each constructor sets.
//
// The servers run stand-in handlers (handlers.go) on loopback; every case is one connection, a
// client script of writes and pauses (cases.go) and what the client read until the server
// closed the connection or the case's window passed. crates/server/tests/wire.rs and
// crates/server/tests/static.rs run the same scripts against pbs-server.
//
// Run it from the directory of this module (crates/server/testdata/go) on amd64, as D-ARCH asks of
// every golden:
//
//	GOARCH=amd64 CGO_ENABLED=1 CC="clang -arch x86_64" go run ./wire
//
// wire_test.go regenerates the document and fails when it differs from the committed one.
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"hash/crc32"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"runtime/debug"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode/utf8"
	_ "unsafe" // for go:linkname

	"github.com/prebid/prebid-server/v4/config"
	_ "github.com/prebid/prebid-server/v4/server" // the constructors below
)

//go:linkname newMainServer github.com/prebid/prebid-server/v4/server.newMainServer
func newMainServer(cfg *config.Configuration, handler http.Handler) *http.Server

//go:linkname newSocketServer github.com/prebid/prebid-server/v4/server.newSocketServer
func newSocketServer(cfg *config.Configuration, handler http.Handler) *http.Server

//go:linkname newAdminServer github.com/prebid/prebid-server/v4/server.newAdminServer
func newAdminServer(cfg *config.Configuration, handler http.Handler) *http.Server

// generator names this program in the golden's header.
const generator = "crates/server/testdata/go/wire"

// upstreamModule is the module path of the Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// header heads the golden: the toolchain, the architecture and the module versions it was
// generated with.
type header struct {
	GoVersion string            `json:"go_version"`
	GoArch    string            `json:"goarch"`
	Upstream  string            `json:"upstream"`
	Modules   map[string]string `json:"modules"`
	Generator string            `json:"generator"`
}

// newHeader records the versions of modules as this binary was built with them.
func newHeader(modules ...string) header {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		panic("the binary carries no build information")
	}
	versions := map[string]string{}
	for _, dep := range info.Deps {
		versions[dep.Path] = dep.Version
	}
	h := header{
		GoVersion: runtime.Version(),
		GoArch:    runtime.GOARCH,
		Upstream:  versions[upstreamModule],
		Modules:   map[string]string{},
		Generator: generator,
	}
	for _, m := range modules {
		version, ok := versions[m]
		if !ok {
			panic("module " + m + " is not part of the build")
		}
		h.Modules[m] = version
	}
	return h
}

// budgets are the settings of one of Prebid's servers that bound a connection, in nanoseconds
// (0 is none) and bytes (0 is net/http's default).
type budgets struct {
	ReadTimeout       time.Duration `json:"read_timeout_ns"`
	ReadHeaderTimeout time.Duration `json:"read_header_timeout_ns"`
	WriteTimeout      time.Duration `json:"write_timeout_ns"`
	IdleTimeout       time.Duration `json:"idle_timeout_ns"`
	MaxHeaderBytes    int           `json:"max_header_bytes"`
	Gzip              bool          `json:"gzip"`
}

func budgetsOf(s *http.Server, gzip bool) budgets {
	return budgets{s.ReadTimeout, s.ReadHeaderTimeout, s.WriteTimeout, s.IdleTimeout, s.MaxHeaderBytes, gzip}
}

// result is a case and what the client read: the header block of the first answer (its Date
// value replaced by "<date>"), the bytes after it (as text when they are valid UTF-8 and at most
// 8 KiB, with their length and CRC-32 always), the gunzipped body of a gzip answer, and how the
// connection ended: "eof", "reset" or "open" (still open when the window passed).
type result struct {
	wireCase
	Head      string `json:"head"`
	Body      string `json:"body"`
	BodyLen   int    `json:"body_len"`
	BodyCRC   string `json:"body_crc32"`
	Gunzipped string `json:"gunzipped,omitempty"`
	End       string `json:"end"`
}

type doc struct {
	Header        header             `json:"header"`
	Host          string             `json:"host"`
	BudgetMs      int                `json:"budget_ms"`
	HeaderLimit   int                `json:"header_limit"`
	Servers       map[string]budgets `json:"servers"`
	StaticModTime string             `json:"static_mod_time"`
	Cases         []result           `json:"cases"`
}

var dateValue = regexp.MustCompile(`(?mi)^(Date: )[^\r]*`)

// run sends the steps of c to addr and reads until the server closes the connection or the
// window passes after the last step.
func run(addr string, c wireCase) result {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		panic(err)
	}
	defer conn.Close()
	total := time.Duration(c.WindowMs) * time.Millisecond
	for _, s := range c.Steps {
		total += time.Duration(s.PauseMs) * time.Millisecond
		if s.Chunk > 0 {
			total += time.Duration(s.Fill/s.Chunk*s.ChunkPauseMs) * time.Millisecond
		}
	}
	if err := conn.SetReadDeadline(time.Now().Add(total)); err != nil {
		panic(err)
	}
	go func() {
		// The server may close the connection before the client has written everything; the
		// write errors that follow are expected.
		for _, s := range c.Steps {
			switch {
			case s.Text != "":
				conn.Write([]byte(s.Text))
			case s.Fill > 0 && s.Chunk > 0:
				for left := s.Fill; left > 0; left -= s.Chunk {
					conn.Write(bytes.Repeat([]byte{'a'}, min(left, s.Chunk)))
					time.Sleep(time.Duration(s.ChunkPauseMs) * time.Millisecond)
				}
			case s.Fill > 0:
				conn.Write(bytes.Repeat([]byte{'a'}, s.Fill))
			}
			time.Sleep(time.Duration(s.PauseMs) * time.Millisecond)
		}
	}()
	var got bytes.Buffer
	_, err = io.Copy(&got, conn)
	end := "eof"
	var ne net.Error
	switch {
	case err == nil:
	case errors.As(err, &ne) && ne.Timeout():
		end = "open"
	case errors.Is(err, syscall.ECONNRESET):
		end = "reset"
	default:
		panic(fmt.Sprintf("%s: %v", c.Name, err))
	}
	return describe(c, got.Bytes(), end)
}

func describe(c wireCase, answer []byte, end string) result {
	r := result{wireCase: c, End: end}
	head, body, ok := bytes.Cut(answer, []byte("\r\n\r\n"))
	if !ok {
		head, body = nil, answer
	} else {
		head = append(head, "\r\n\r\n"...)
	}
	r.Head = dateValue.ReplaceAllString(string(head), "${1}<date>")
	if utf8.Valid(body) && len(body) <= 8<<10 {
		r.Body = string(body)
	}
	r.BodyLen = len(body)
	r.BodyCRC = fmt.Sprintf("%08x", crc32.ChecksumIEEE(body))
	if strings.Contains(strings.ToLower(r.Head), "\r\ncontent-encoding: gzip\r\n") {
		r.Gunzipped = gunzip(head, body)
	}
	return r
}

// gunzip decodes a gzip body, undoing the chunked framing first when the head says so.
func gunzip(head, body []byte) string {
	if bytes.Contains(bytes.ToLower(head), []byte("\r\ntransfer-encoding: chunked\r\n")) {
		var plain []byte
		for {
			line, rest, ok := bytes.Cut(body, []byte("\r\n"))
			if !ok {
				return "<bad chunked framing>"
			}
			var n int
			if _, err := fmt.Sscanf(string(line), "%x", &n); err != nil {
				return "<bad chunk size>"
			}
			if n == 0 {
				break
			}
			plain = append(plain, rest[:n]...)
			body = rest[n+2:]
		}
		body = plain
	}
	zr, err := gzip.NewReader(bytes.NewReader(body))
	if err != nil {
		return "<" + err.Error() + ">"
	}
	out, err := io.ReadAll(zr)
	if err != nil {
		return "<" + err.Error() + ">"
	}
	return fmt.Sprintf("%d/%08x", len(out), crc32.ChecksumIEEE(out))
}

// serve starts s on a loopback listener and returns its address.
func serve(s *http.Server) string {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	go s.Serve(ln)
	return ln.Addr().String()
}

// generate starts the servers, runs every case on its own connection (all at once, so the
// timeout cases overlap) and returns the document.
func generate() doc {
	dir, err := os.MkdirTemp("", "pbs-server-wire")
	if err != nil {
		panic(err)
	}
	defer os.RemoveAll(dir)
	static := filepath.Join(dir, "static")
	if err := writeStatic(static); err != nil {
		panic(err)
	}

	plain := &config.Configuration{}
	withGzip := &config.Configuration{}
	withGzip.Compression.Response.GZIP = true
	socket := &config.Configuration{UnixSocketEnable: true, UnixSocketName: filepath.Join(dir, "prebid.sock")}

	main := newMainServer(plain, standIns())
	mainGzip := newMainServer(withGzip, standIns())
	scaled := newMainServer(plain, standIns())
	scaled.ReadTimeout = budget * time.Millisecond
	scaled.WriteTimeout = budget * time.Millisecond
	admin := newAdminServer(plain, standIns())
	files := newMainServer(plain, staticHandler(static))
	d := doc{
		Header:      newHeader("github.com/NYTimes/gziphandler", "github.com/julienschmidt/httprouter", "github.com/rs/cors"),
		Host:        host,
		BudgetMs:    budget,
		HeaderLimit: headerLimit,
		Servers: map[string]budgets{
			"main":   budgetsOf(newMainServer(plain, standIns()), false),
			"socket": budgetsOf(newSocketServer(socket, standIns()), false),
			"admin":  budgetsOf(newAdminServer(plain, standIns()), false),
		},
		StaticModTime: staticTime.Format(time.RFC3339),
	}
	addrs := map[string]string{
		"main":      serve(main),
		"main_gzip": serve(mainGzip),
		"scaled":    serve(scaled),
		"admin":     serve(admin),
		"static":    serve(files),
	}
	cases := allCases()
	d.Cases = make([]result, len(cases))
	var wg sync.WaitGroup
	for i, c := range cases {
		wg.Add(1)
		go func() {
			defer wg.Done()
			d.Cases[i] = run(addrs[c.Server], c)
		}()
	}
	wg.Wait()
	for _, s := range []*http.Server{main, mainGzip, scaled, admin, files} {
		s.Close()
	}
	return d
}

// encode writes d as indented JSON without HTML escaping.
func encode(d doc) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(d); err != nil {
		panic(err)
	}
	return buf.Bytes()
}

// pattern is n bytes cycling through the alphabet.
func pattern(n int) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte('a' + i%26)
	}
	return b
}

func defaultOut() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..", "golden", "wire.json")
}

func main() {
	out := flag.String("out", defaultOut(), "the golden to write")
	flag.Parse()
	if err := os.WriteFile(*out, encode(generate()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
