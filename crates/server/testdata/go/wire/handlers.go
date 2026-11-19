package main

import (
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"

	"github.com/julienschmidt/httprouter"

	"github.com/prebid/prebid-server/v4/router"
)

// standIns are the handlers the wire cases call. crates/server/tests/wire.rs serves the same
// paths on axum:
//
//	/headers  sets X-Zeta, Content-Type, X-Alpha and two X-Multi values, in that order, and
//	          writes {"ok":true}
//	/bytes    writes n pattern bytes (the query's n) with Content-Type text/plain; charset=utf-8
//	          and no Content-Length
//	/sniff    writes a body of the query's kind (json, html, xml, text, binary or empty) with no
//	          Content-Type
//	/typed    writes {"a":1} with Content-Type application/json
//	/echo     reads the body and writes "body=<bytes read>" and " error=timeout" when the read
//	          hit a deadline (or " error=<text>" for another error)
//	/sleep    sleeps the query's ms, sets the query's mark (when it has one) and writes "late"
//	/mark     writes "completed" when a /sleep set the query's id as its mark, else "pending"
func standIns() http.Handler {
	var marks sync.Map
	mux := http.NewServeMux()
	mux.HandleFunc("/headers", func(w http.ResponseWriter, _ *http.Request) {
		h := w.Header()
		h.Set("X-Zeta", "z")
		h.Set("Content-Type", "application/json")
		h.Set("X-Alpha", "a")
		h.Add("X-Multi", "1")
		h.Add("X-Multi", "2")
		io.WriteString(w, `{"ok":true}`)
	})
	mux.HandleFunc("/bytes", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		n, _ := strconv.Atoi(r.URL.Query().Get("n"))
		w.Write(pattern(n))
	})
	mux.HandleFunc("/sniff", func(w http.ResponseWriter, r *http.Request) {
		w.Write(sniffBodies[r.URL.Query().Get("kind")])
	})
	mux.HandleFunc("/typed", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"a":1}`)
	})
	mux.HandleFunc("/echo", func(w http.ResponseWriter, r *http.Request) {
		n, err := io.Copy(io.Discard, r.Body)
		out := fmt.Sprintf("body=%d", n)
		var ne net.Error
		switch {
		case err == nil:
		case errors.As(err, &ne) && ne.Timeout():
			out += " error=timeout"
		default:
			out += " error=" + err.Error()
		}
		io.WriteString(w, out)
	})
	mux.HandleFunc("/sleep", func(w http.ResponseWriter, r *http.Request) {
		ms, _ := strconv.Atoi(r.URL.Query().Get("ms"))
		time.Sleep(time.Duration(ms) * time.Millisecond)
		if mark := r.URL.Query().Get("mark"); mark != "" {
			marks.Store(mark, true)
		}
		io.WriteString(w, "late")
	})
	mux.HandleFunc("/mark", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := marks.Load(r.URL.Query().Get("id")); ok {
			io.WriteString(w, "completed")
			return
		}
		io.WriteString(w, "pending")
	})
	return mux
}

// sniffBodies are /sniff's bodies by kind.
var sniffBodies = map[string][]byte{
	"json":   []byte(`{"id":"req-1","imp":[]}`),
	"html":   []byte("<html><body>Prebid Server</body></html>"),
	"xml":    []byte(`<?xml version="1.0" encoding="UTF-8"?><VAST version="3.0"></VAST>`),
	"text":   []byte("ok"),
	"binary": {0x00, 0x01, 0x02, 0x03, 0xfe, 0xff},
	"empty":  {},
}

// staticFiles are the files of the static fixture, a small tree shaped like Prebid's static
// directory: index.html, optout.html and pbs-logo.svg at the root, a bidder-params directory
// with no index.html, three files whose types come from their bytes, and big.txt, 3,000
// pattern bytes, long enough for gzip.
var staticFiles = []struct {
	path    string
	content string
}{
	{"index.html", "<html><body>Prebid Server</body></html>\n"},
	{"optout.html", "<html><body>Opt out</body></html>\n"},
	{"pbs-logo.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>\n"},
	{"notes.txt", "hello, static\n"},
	{"bidder-params/appnexus.json", "{\"title\":\"AppNexus\"}\n"},
	{"noext", "\x00\x01\x02binary"},
	{"data.unknownext", "plain words\n"},
	{"big.txt", string(pattern(3000))},
}

// staticTime is the modification time of every fixture file, so Last-Modified is the same on
// every run.
var staticTime = time.Date(2020, time.January, 2, 3, 4, 5, 0, time.UTC)

// writeStatic writes the static fixture under dir, every file and directory modified at
// staticTime.
func writeStatic(dir string) error {
	for _, f := range staticFiles {
		path := filepath.Join(dir, filepath.FromSlash(f.path))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(path, []byte(f.content), 0o644); err != nil {
			return err
		}
	}
	return filepath.WalkDir(dir, func(path string, _ os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		return os.Chtimes(path, staticTime, staticTime)
	})
}

// staticHandler is Prebid's public handler with only its /static route (router/router.go:296,
// with the fixture for the static directory), wrapped as main.go wraps it.
func staticHandler(dir string) http.Handler {
	r := httprouter.New()
	r.ServeFiles("/static/*filepath", http.Dir(dir))
	return router.NoCache{Handler: router.SupportCORS(r)}
}
