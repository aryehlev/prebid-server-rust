package main

import (
	"fmt"
	"strings"
)

// budget is the scaled read and write budget of the "scaled" server: Prebid's main server with
// its 15 s ReadTimeout and WriteTimeout (server/server.go:103-104) set to this many
// milliseconds, so the timeout cases run in seconds. The pauses of those cases are fractions of
// it, a sixth of it at least away from every deadline they straddle.
const budget = 1000

// headerLimit is the most bytes of request line and headers net/http reads: its
// DefaultMaxHeaderBytes (1 MiB, which Prebid's servers keep) and the 4096 bytes of slack
// initialReadLimitSize adds.
const headerLimit = 1<<20 + 4096

// host is the Host header of the requests.
const host = "pbs.example"

// step is one thing the client does, in order: write Text, write Fill bytes of 'a' (in writes
// of Chunk bytes with ChunkPauseMs between them when Chunk is set), pause PauseMs, or close the
// connection (Close "fin" closes it, "rst" closes it with SO_LINGER 0, so the server reads a
// reset) and stop reading.
type step struct {
	Text         string `json:"text,omitempty"`
	Fill         int    `json:"fill,omitempty"`
	Chunk        int    `json:"chunk,omitempty"`
	ChunkPauseMs int    `json:"chunk_pause_ms,omitempty"`
	PauseMs      int    `json:"pause_ms,omitempty"`
	Close        string `json:"close,omitempty"`
}

// wireCase is one connection: the server it goes to, what the client sends, and how long the
// client keeps reading after its last step before it records the connection as open.
type wireCase struct {
	Group    string `json:"group"`
	Name     string `json:"name"`
	Server   string `json:"server"`
	Steps    []step `json:"steps"`
	WindowMs int    `json:"window_ms"`
}

func text(s string) step        { return step{Text: s} }
func pause(ms int) step         { return step{PauseMs: ms} }
func closeWith(how string) step { return step{Close: how} }
func fill(n int) step           { return step{Fill: n} }
func paced(n, chunk int) step   { return step{Fill: n, Chunk: chunk, ChunkPauseMs: 2} }

// get is a GET request for target that closes the connection after its answer.
func get(target string, headers ...string) string {
	return "GET " + target + " HTTP/1.1\r\nHost: " + host + "\r\n" + strings.Join(append(headers, "Connection: close"), "\r\n") + "\r\n\r\n"
}

// one is a case of a single request on a server that answers at once.
func one(group, name, server string, steps ...step) wireCase {
	return wireCase{Group: group, Name: name, Server: server, Steps: steps, WindowMs: 2000}
}

// bigHeader is a GET request to /echo whose request line and headers are exactly size bytes:
// one X-Big header pads it.
func bigHeader(size int, chunk int) []step {
	prefix := "GET /echo HTTP/1.1\r\nHost: " + host + "\r\nX-Big: "
	suffix := "\r\nConnection: close\r\n\r\n"
	n := size - len(prefix) - len(suffix)
	pad := fill(n)
	if chunk > 0 {
		pad = paced(n, chunk)
	}
	return []step{text(prefix), pad, text(suffix)}
}

// manyHeaders is a GET request to /echo with n headers besides Host and Connection.
func manyHeaders(n int) string {
	var b strings.Builder
	b.WriteString("GET /echo HTTP/1.1\r\nHost: " + host + "\r\n")
	for i := 1; i <= n; i++ {
		fmt.Fprintf(&b, "X-H%d: v\r\n", i)
	}
	b.WriteString("Connection: close\r\n\r\n")
	return b.String()
}

// longTarget is a GET request to /echo whose request target is exactly n bytes.
func longTarget(n int) []step {
	prefix := "GET /echo?q="
	return []step{text(prefix), fill(n - len("/echo?q=")), text(" HTTP/1.1\r\nHost: " + host + "\r\nConnection: close\r\n\r\n")}
}

// h2Preface is the client connection preface of HTTP/2 with prior knowledge (RFC 9113 section
// 3.4) and an empty SETTINGS frame.
const h2Preface = "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00"

func allCases() []wireCase {
	cs := []wireCase{
		one("headers", "handler_order", "main", text(get("/headers"))),
		one("headers", "typed", "main", text(get("/typed"))),

		one("framing", "bytes_0", "main", text(get("/bytes?n=0"))),
		one("framing", "bytes_100", "main", text(get("/bytes?n=100"))),
		one("framing", "bytes_2048", "main", text(get("/bytes?n=2048"))),
		one("framing", "bytes_2049", "main", text(get("/bytes?n=2049"))),
		one("framing", "bytes_3000", "main", text(get("/bytes?n=3000"))),
		one("framing", "bytes_3000_http10", "main", text("GET /bytes?n=3000 HTTP/1.0\r\nHost: "+host+"\r\n\r\n")),
		one("framing", "head_bytes_3000", "main", text("HEAD /bytes?n=3000 HTTP/1.1\r\nHost: "+host+"\r\nConnection: close\r\n\r\n")),
	}
	for _, kind := range []string{"json", "html", "xml", "text", "binary", "empty"} {
		cs = append(cs, one("sniffing", "sniff_"+kind, "main", text(get("/sniff?kind="+kind))))
	}

	cs = append(cs,
		one("header_limit", "header_600kib", "main", bigHeader(600<<10, 0)...),
		one("header_limit", "header_1mib", "main", bigHeader(1<<20, 0)...),
		one("header_limit", "header_at_limit", "main", bigHeader(headerLimit, 0)...),
		one("header_limit", "header_over_64kib_paced", "main", bigHeader(headerLimit+64<<10, 64<<10)...),
		one("header_limit", "header_2mib_paced", "main", bigHeader(2<<20, 64<<10)...),
		one("header_limit_edge", "header_one_over", "main",
			text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX-Big: "), fill(headerLimit-100-len("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX-Big: ")),
			pause(200), fill(101-len("\r\nConnection: close\r\n\r\n")), text("\r\nConnection: close\r\n\r\n")),

		one("header_count", "headers_100", "main", text(manyHeaders(98))),
		one("header_count", "headers_101", "main", text(manyHeaders(99))),
		one("header_count", "headers_1000", "main", text(manyHeaders(998))),

		one("long_target", "target_65534", "main", longTarget(65534)...),
		one("long_target", "target_65535", "main", longTarget(65535)...),
		one("long_target", "target_100000", "main", longTarget(100000)...),

		one("malformed", "missing_host", "main", text("GET /echo HTTP/1.1\r\nConnection: close\r\n\r\n")),
		one("malformed", "two_hosts", "main", text("GET /echo HTTP/1.1\r\nHost: a.example\r\nHost: b.example\r\nConnection: close\r\n\r\n")),
		one("malformed", "header_without_colon", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nBad Header Line\r\n\r\n")),
		one("malformed", "space_in_header_name", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX Y: z\r\n\r\n")),
		one("malformed", "space_before_colon", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX-A : z\r\n\r\n")),
		one("malformed", "nul_in_header_value", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX-A: a\x00b\r\n\r\n")),
		one("malformed", "folded_header", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nX-A: a\r\n b\r\nConnection: close\r\n\r\n")),
		one("malformed", "garbage", "main", text("garbage\r\n\r\n")),
		one("malformed", "lower_case_method", "main", text("get /echo HTTP/1.1\r\nHost: "+host+"\r\nConnection: close\r\n\r\n")),
		one("malformed", "unknown_version", "main", text("GET /echo HTTP/9.9\r\nHost: "+host+"\r\n\r\n")),
		one("malformed", "http_10_without_host", "main", text("GET /echo HTTP/1.0\r\n\r\n")),
		one("malformed", "bad_content_length", "main", text("POST /echo HTTP/1.1\r\nHost: "+host+"\r\nContent-Length: abc\r\n\r\n")),
		one("malformed", "two_content_lengths", "main", text("POST /echo HTTP/1.1\r\nHost: "+host+"\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab")),
		one("malformed", "unknown_transfer_encoding", "main", text("POST /echo HTTP/1.1\r\nHost: "+host+"\r\nTransfer-Encoding: gzip\r\n\r\n")),
		one("malformed", "chunked_and_length", "main", text("POST /echo HTTP/1.1\r\nHost: "+host+"\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\nConnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n")),
		one("malformed", "bad_chunk_size", "main", text("POST /echo HTTP/1.1\r\nHost: "+host+"\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nzz\r\nabc\r\n0\r\n\r\n")),
		one("malformed", "leading_blank_line", "main", text("\r\nGET /echo HTTP/1.1\r\nHost: "+host+"\r\nConnection: close\r\n\r\n")),
		one("malformed", "absolute_form", "main", text("GET http://other.example/echo HTTP/1.1\r\nHost: "+host+"\r\nConnection: close\r\n\r\n")),

		one("h2c", "prior_knowledge", "main", text(h2Preface)),
		one("h2c", "upgrade_header", "main", text("GET /echo HTTP/1.1\r\nHost: "+host+"\r\nConnection: Upgrade, HTTP2-Settings\r\nUpgrade: h2c\r\nHTTP2-Settings: AAMAAABkAARAAAAAAAIAAAAA\r\n\r\n")),

		wireCase{Group: "timeouts", Name: "slow_headers", Server: "scaled", Steps: []step{text("GET /echo HTTP/1.1\r\nHost: " + host + "\r\n")}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "slow_headers_admin", Server: "admin", Steps: []step{text("GET /echo HTTP/1.1\r\nHost: " + host + "\r\n")}, WindowMs: 3 * budget},
		// Every gap is short and the whole head is long: the budget passes 200 ms after X-A and
		// 200 ms before X-B.
		wireCase{Group: "timeouts", Name: "trickled_headers", Server: "scaled", Steps: []step{
			text("GET /echo HTTP/1.1\r\n"), pause(2 * budget / 5), text("Host: " + host + "\r\n"), pause(2 * budget / 5), text("X-A: 1\r\n"),
			pause(2 * budget / 5), text("X-B: 2\r\n"), pause(2 * budget / 5), text("Connection: close\r\n\r\n"),
		}, WindowMs: 2 * budget},
		wireCase{Group: "timeouts", Name: "idle_keep_alive", Server: "scaled", Steps: []step{text("GET /echo HTTP/1.1\r\nHost: " + host + "\r\n\r\n")}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "idle_keep_alive_admin", Server: "admin", Steps: []step{text("GET /echo HTTP/1.1\r\nHost: " + host + "\r\n\r\n")}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "stalled_body", Server: "scaled", Steps: []step{
			pause(budget / 2), text("POST /echo HTTP/1.1\r\nHost: " + host + "\r\nContent-Length: 10\r\n\r\nabcde"),
		}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "trickled_body", Server: "scaled", Steps: []step{
			pause(budget / 2), text("POST /echo HTTP/1.1\r\nHost: " + host + "\r\nContent-Length: 5\r\nConnection: close\r\n\r\na"),
			pause(budget / 3), text("b"), pause(budget / 3), text("c"), pause(budget / 3), text("d"), pause(budget / 3), text("e"),
		}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "quick_handler", Server: "scaled", Steps: []step{text(get(fmt.Sprintf("/sleep?ms=%d", budget/2)))}, WindowMs: 3 * budget},
		wireCase{Group: "timeouts", Name: "slow_handler", Server: "scaled", Steps: []step{text(get(fmt.Sprintf("/sleep?ms=%d", 3*budget)))}, WindowMs: 5 * budget},
		wireCase{Group: "timeouts", Name: "slow_handler_admin", Server: "admin", Steps: []step{text(get(fmt.Sprintf("/sleep?ms=%d", 3*budget)))}, WindowMs: 5 * budget},
	)

	// A client that goes away while its request is served: the handler runs on to its end (its
	// mark is set), and a second connection asks for the mark after it.
	for _, how := range []string{"fin", "rst"} {
		cs = append(cs,
			wireCase{Group: "client_gone", Name: "gone_" + how, Server: "main", Steps: []step{
				text(get(fmt.Sprintf("/sleep?ms=%d&mark=gone_%s", budget/2, how))), pause(budget / 10), closeWith(how),
			}, WindowMs: 1000},
			wireCase{Group: "client_gone", Name: "check_" + how, Server: "main", Steps: []step{
				pause(budget + budget/2), text(get("/mark?id=gone_" + how)),
			}, WindowMs: 2000},
		)
	}

	for _, accept := range []string{"gzip", ""} {
		var hs []string
		suffix := "_identity"
		if accept != "" {
			hs = []string{"Accept-Encoding: " + accept}
			suffix = "_gzip"
		}
		for _, target := range []string{"/bytes?n=10", "/bytes?n=1399", "/bytes?n=1400", "/bytes?n=3000", "/sniff?kind=empty", "/headers"} {
			name := strings.NewReplacer("/", "", "?", "_", "=", "_").Replace(target) + suffix
			cs = append(cs, one("gzip", name, "main_gzip", text(get(target, hs...))))
		}
	}

	for _, s := range []struct{ name, request string }{
		{"text_file", get("/static/notes.txt")},
		{"html_file", get("/static/optout.html")},
		{"svg_file", get("/static/pbs-logo.svg")},
		{"json_file", get("/static/bidder-params/appnexus.json")},
		{"no_extension", get("/static/noext")},
		{"unknown_extension", get("/static/data.unknownext")},
		{"directory_index", get("/static/")},
		{"index_file", get("/static/index.html")},
		{"directory_listing", get("/static/bidder-params/")},
		{"directory_without_slash", get("/static/bidder-params")},
		{"static_without_slash", get("/static")},
		{"file_with_slash", get("/static/notes.txt/")},
		{"missing_file", get("/static/missing.txt")},
		{"dot_dot", get("/static/../go.mod")},
		{"encoded_dot_dot", get("/static/%2e%2e/go.mod")},
		{"range", get("/static/notes.txt", "Range: bytes=0-4")},
		{"bad_range", get("/static/notes.txt", "Range: bytes=100-200")},
		{"not_modified", get("/static/notes.txt", "If-Modified-Since: Thu, 02 Jan 2020 03:04:05 GMT")},
		{"modified", get("/static/notes.txt", "If-Modified-Since: Wed, 01 Jan 2020 00:00:00 GMT")},
		{"head_file", "HEAD /static/notes.txt HTTP/1.1\r\nHost: " + host + "\r\nConnection: close\r\n\r\n"},
		{"post_file", "POST /static/notes.txt HTTP/1.1\r\nHost: " + host + "\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"},
	} {
		cs = append(cs, one("static", s.name, "static", text(s.request)))
	}
	return cs
}
