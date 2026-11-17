package main

import (
	"bytes"
	"compress/gzip"
	"fmt"
	"strings"
)

func reqOp() Op              { return Op{Op: "req"} }
func headOp() Op             { return Op{Op: "head"} }
func bodyOp() Op             { return Op{Op: "body"} }
func write(s string) Op      { return Op{Op: "write", Data: Bytes(s)} }
func writeBytes(b []byte) Op { return Op{Op: "write", Data: Bytes(b)} }
func sleepOp(n int) Op       { return Op{Op: "sleep", Ms: n} }
func closeOp() Op            { return Op{Op: "close"} }
func resetOp() Op            { return Op{Op: "reset"} }
func skipOp(n int) Op        { return Op{Op: "skip", Ms: n} }
func drainOp() Op            { return Op{Op: "drain"} }
func tlsOp(cert string) Op   { return Op{Op: "tls", Data: Bytes(cert)} }
func conn(ops ...Op) []Op    { return ops }
func srv(name string, conns ...[]Op) Server {
	return Server{Name: name, Conns: conns}
}

// ok is a 200 response with a Content-Length body.
func ok(body string) string {
	return fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n%s", len(body), body)
}

// response is a response with the given status line tail, extra header lines and a
// Content-Length body.
func response(status, headers, body string) string {
	return fmt.Sprintf("HTTP/1.1 %s\r\n%sContent-Length: %d\r\n\r\n%s", status, headers, len(body), body)
}

func gz(s string) []byte {
	var b bytes.Buffer
	zw := gzip.NewWriter(&b)
	_, _ = zw.Write([]byte(s))
	_ = zw.Close()
	return b.Bytes()
}

func gzipResponse(headers string, body []byte) []byte {
	head := fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n%sContent-Length: %d\r\n\r\n", headers, len(body))
	return append([]byte(head), body...)
}

func get(url string) Step {
	return Step{Req: Req{Method: "GET", URL: url, Body: Body{Kind: "none"}}, Read: "all"}
}

func do(method, url string, body Body) Step {
	return Step{Req: Req{Method: method, URL: url, Body: body}, Read: "all"}
}

func bytesBody(s string) Body { return Body{Kind: "bytes", Data: Bytes(s)} }
func readerBody(p ...string) Body {
	b := Body{Kind: "reader", Pieces: []Bytes{}}
	for _, s := range p {
		b.Pieces = append(b.Pieces, Bytes(s))
	}
	return b
}

func withHeader(st Step, kv ...string) Step {
	for i := 0; i+1 < len(kv); i += 2 {
		st.Req.Header = append(st.Req.Header, [2]string{kv[i], kv[i+1]})
	}
	return st
}

func sleepFirst(n int, st Step) Step {
	st.SleepMs = n
	return st
}

func settle(n int, st Step) Step {
	st.SettleMs = n
	return st
}

func async(st Step) Step {
	st.Async = true
	return st
}

// general is router.go's generalHttpClient with the http_client defaults.
func general() Client {
	return Client{
		Profile:                 "general",
		DialTimeoutMs:           30000,
		KeepAliveMs:             15000,
		MaxIdleConns:            400,
		MaxIdleConnsPerHost:     10,
		IdleConnTimeoutMs:       60000,
		TLSHandshakeTimeoutMs:   10000,
		ExpectContinueTimeoutMs: 1000,
		TLSConfig:               true,
		TrustCA:                 true,
	}
}

// cache is router.go's cacheHttpClient with the http_client_cache defaults.
func cache() Client {
	return Client{
		Profile:                 "cache",
		DialTimeoutMs:           30000,
		KeepAliveMs:             15000,
		MaxIdleConns:            10,
		MaxIdleConnsPerHost:     2,
		IdleConnTimeoutMs:       60000,
		TLSHandshakeTimeoutMs:   10000,
		ExpectContinueTimeoutMs: 1000,
	}
}

// floors is router.go's floorFechterHttpClient with the price_floors defaults.
func floors() Client {
	c := cache()
	c.Profile = "floors"
	c.MaxIdleConns = 40
	return c
}

func one(name string, client Client, servers []Server, steps ...Step) Case {
	if servers == nil {
		servers = []Server{}
	}
	return Case{Name: name, Client: client, Servers: servers, Steps: steps}
}

func cases() []Case {
	var all []Case
	all = append(all, wireCases()...)
	all = append(all, responseCases()...)
	all = append(all, gzipCases()...)
	all = append(all, redirectCases()...)
	all = append(all, timeoutCases()...)
	all = append(all, failureCases()...)
	all = append(all, poolCases()...)
	all = append(all, proxyCases()...)
	all = append(all, tlsCases()...)
	all = append(all, continueCases()...)
	all = append(all, requestErrorCases()...)
	all = append(all, edgeCases()...)
	return all
}

// chunked is a chunked response body with one chunk per piece.
func chunked(pieces ...string) string {
	var b strings.Builder
	for _, p := range pieces {
		fmt.Fprintf(&b, "%x\r\n%s\r\n", len(p), p)
	}
	b.WriteString("0\r\n\r\n")
	return b.String()
}

func edgeCases() []Case {
	withLength := func(n int64, st Step) Step {
		st.Req.ContentLength = &n
		return st
	}
	large := strings.Repeat("0123456789abcdef", 2500) // 40000 bytes: io.Copy reads 32 KiB at a time
	big := strings.Repeat("prebid server rust parity ", 1600)
	connsPerHost := general()
	connsPerHost.MaxConnsPerHost = 1
	waiting := get("http://{{A}}/2")
	waiting.Req.CtxTimeoutMs = 100
	return []Case{
		one("post_reader_large_piece", general(), []Server{srv("A", conn(reqOp(), write(ok("big"))))},
			do("POST", "http://{{A}}/large", readerBody(large, "tail"))),
		one("post_reader_large_length", general(), []Server{srv("A", conn(reqOp(), write(ok("big"))))},
			withLength(int64(len(large)), do("POST", "http://{{A}}/large", readerBody(large)))),
		one("post_reader_short_content_length", general(), []Server{srv("A", conn(reqOp()))},
			withLength(10, do("POST", "http://{{A}}/short", readerBody("abc", "def")))),
		one("post_reader_long_content_length", general(), []Server{srv("A", conn(reqOp()))},
			withLength(4, do("POST", "http://{{A}}/long", readerBody("abcdef")))),
		one("get_bytes_body", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("GET", "http://{{A}}/with-body", bytesBody("q=1"))),
		one("patch_bytes", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("PATCH", "http://{{A}}/item", bytesBody(`{"op":"x"}`))),
		one("get_userinfo", general(), []Server{srv("A", conn(reqOp(), write(ok("authed"))))},
			get("http://user:p%40ss@{{A}}/private")),
		one("redirect_userinfo", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", "")),
				reqOp(), write(ok("done"))))},
			get("http://user:pass@{{A}}/start")),
		one("err_userinfo_refused", general(), []Server{{Name: "X", Closed: true}}, get("http://user:secret@{{X}}/")),
		one("request_connection_close_header", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), write(ok("2"))))},
			withHeader(get("http://{{A}}/1"), "Connection", "close"), get("http://{{A}}/2")),
		one("resp_http10_chunked_ignored", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"+chunked("abc")), closeOp()))},
			get("http://{{A}}/")),
		one("resp_content_length_plus", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: +5\r\n\r\nhello"), closeOp()))},
			get("http://{{A}}/")),
		one("resp_te_and_content_length", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 99\r\n\r\n"+chunked("wins")),
				reqOp(), write(ok("reused"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
		one("resp_large_chunked", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"+
				chunked(big[:5000], big[5000:5001], big[5001:40000], big[40000:]))))},
			get("http://{{A}}/large")),
		one("resp_large_content_length", general(), []Server{srv("A", conn(reqOp(), write(ok(big))))},
			get("http://{{A}}/large")),
		one("gzip_large", general(), []Server{srv("A", conn(reqOp(), writeBytes(gzipResponse("", gz(big)))))},
			get("http://{{A}}/gz")),
		one("head_chunked_then_get", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"),
				reqOp(), write(ok("after"))))},
			do("HEAD", "http://{{A}}/h", Body{Kind: "none"}), get("http://{{A}}/after")),
		one("max_conns_per_host_wait_timeout", connsPerHost,
			[]Server{srv("A", conn(reqOp(), sleepOp(400), write(ok("1"))))},
			async(get("http://{{A}}/1")), sleepFirst(20, waiting)),
		one("proxy_http_reuse_other_host", general(),
			[]Server{srv("P", conn(reqOp(), write(ok("a")), reqOp(), write(ok("b"))))},
			get("http://origin.test/a"), get("http://other.test/b")),
		one("expect_100_continue_reader", general(),
			[]Server{srv("A", conn(headOp(), write("HTTP/1.1 100 Continue\r\n\r\n"), bodyOp(), write(ok("streamed"))))},
			withHeader(do("POST", "http://{{A}}/upload", readerBody("part1", "part2")), "Expect", "100-continue")),
		one("resp_status_line_only_eof", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\n"), closeOp()))}, get("http://{{A}}/")),
		one("resp_invalid_header_value", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nX-Bad: a\x00b\r\nContent-Length: 0\r\n\r\n"), closeOp()))},
			get("http://{{A}}/")),
	}
}

func wireCases() []Case {
	length := int64(6)
	withLength := do("POST", "http://{{A}}/len", readerBody("abc", "def"))
	withLength.Req.ContentLength = &length
	hostOverride := get("http://{{A}}/host")
	hostOverride.Req.Host = "virtual.test:8080"
	closing := get("http://{{A}}/close")
	closing.Req.Close = true
	trailer := do("POST", "http://{{A}}/trailer", readerBody("data"))
	trailer.Req.TransferEncoding = []string{"chunked"}
	trailer.Req.Trailer = [][2]string{{"X-Sum", "abc"}}
	raw := get("http://{{A}}/raw")
	raw.Req.RawHeader = [][2]string{{"x-raw-key", "kept as written"}}
	return []Case{
		one("get_basic", general(), []Server{srv("A", conn(reqOp(), write(ok("hello"))))},
			get("http://{{A}}/path?q=1&r=%20x")),
		one("get_headers", general(), []Server{srv("A", conn(reqOp(), write(ok("h"))))},
			withHeader(get("http://{{A}}/headers"),
				"X-Multi", "a", "x-multi", "b", "accept", "text/html",
				"User-Agent", "pbs-test/1.0", "Content-Type", "application/json",
				"X-Spaced", "  spaced value  ", "Referer", "http://ref.test/")),
		one("get_empty_user_agent", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			withHeader(get("http://{{A}}/"), "User-Agent", "")),
		one("get_raw_header_key", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))}, raw),
		one("get_host_override", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))}, hostOverride),
		one("get_escaped_path", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			get("http://{{A}}/a%2Fb/%7Euser/x;p?x=1&y=%E2%9C%93#frag")),
		one("post_bytes", general(), []Server{srv("A", conn(reqOp(), write(ok("posted"))))},
			withHeader(do("POST", "http://{{A}}/submit", bytesBody(`{"a":1}`)), "Content-Type", "application/json")),
		one("post_reader_chunked", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("POST", "http://{{A}}/stream", readerBody("hello ", "chunked ", "world"))),
		one("post_reader_length", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))}, withLength),
		one("get_reader_probe", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("GET", "http://{{A}}/probe", readerBody("xyz", "123"))),
		one("get_reader_empty", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("GET", "http://{{A}}/probe-empty", readerBody())),
		one("post_nobody", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("POST", "http://{{A}}/nobody", Body{Kind: "nobody"})),
		one("post_nil_body", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("POST", "http://{{A}}/nil", Body{Kind: "none"})),
		one("put_empty_bytes", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("PUT", "http://{{A}}/empty", bytesBody(""))),
		one("delete_nil_body", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))},
			do("DELETE", "http://{{A}}/item/1", Body{Kind: "none"})),
		one("head_then_get", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n"),
				reqOp(), write(ok("after head"))))},
			do("HEAD", "http://{{A}}/head", Body{Kind: "none"}), get("http://{{A}}/after")),
		one("request_close", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1"))), conn(reqOp(), write(ok("2"))))},
			closing, get("http://{{A}}/next")),
		one("post_chunked_trailer", general(), []Server{srv("A", conn(reqOp(), write(ok(""))))}, trailer),
		one("cache_client_post", cache(), []Server{srv("A", conn(reqOp(), write(ok(`{"responses":[]}`))))},
			withHeader(do("POST", "http://{{A}}/cache", bytesBody(`{"puts":[]}`)), "Content-Type", "application/json;charset=utf-8")),
		one("floors_client_get", floors(), []Server{srv("A", conn(reqOp(), write(ok(`{"data":{}}`))))},
			get("http://{{A}}/floors.json")),
	}
}

func responseCases() []Case {
	single := func(name, resp string) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), write(resp), closeOp()))}, get("http://{{A}}/"))
	}
	return []Case{
		single("resp_chunked_trailer", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-T\r\n\r\n"+
			"5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-T: done\r\nX-Other: 2\r\n\r\n"),
		single("resp_chunked_no_trailer", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"),
		one("resp_close_delimited", general(), []Server{srv("A", conn(reqOp(), write("HTTP/1.0 200 OK\r\nX-A: 1\r\n\r\nbody until close"), closeOp()))}, get("http://{{A}}/")),
		one("resp_http10_keepalive", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 2\r\n\r\nok"),
				reqOp(), write(ok("again"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
		one("resp_connection_close", general(),
			[]Server{srv("A", conn(reqOp(), write(response("200 OK", "Connection: close\r\n", "bye")), drainOp()),
				conn(reqOp(), write(ok("new"))))},
			get("http://{{A}}/"), get("http://{{A}}/2")),
		single("resp_204_with_length", "HTTP/1.1 204 No Content\r\nContent-Length: 10\r\n\r\n"),
		single("resp_304", "HTTP/1.1 304 Not Modified\r\nEtag: \"x\"\r\n\r\n"),
		single("resp_dup_content_length_same", "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\nabc"),
		single("resp_dup_content_length_diff", "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd"),
		single("resp_bad_content_length", "HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\n"),
		single("resp_malformed_status", "HTTP/1.1 abc OK\r\n\r\n"),
		single("resp_malformed_version", "HTTPS/1.1 200 OK\r\n\r\n"),
		single("resp_garbage", "garbage\r\n\r\n"),
		single("resp_bad_header_line", "HTTP/1.1 200 OK\r\nBad Header\r\n\r\n"),
		single("resp_header_key_space", "HTTP/1.1 200 OK\r\nBad Key: v\r\n\r\n"),
		single("resp_leading_space_header", "HTTP/1.1 200 OK\r\n X-Lead: 1\r\n\r\n"),
		single("resp_header_forms", "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\nx-lower-case: 1\r\n"+
			"X-UPPER: 2\r\nweird_key: 3\r\nX-Spaces:    padded   \r\nX-Fold: a\r\n  b\r\nX-Dup: 1\r\nx-dup: 2\r\n"+
			"Content-Length: 0\r\n\r\n"),
		single("resp_1xx_skipped", "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </s.css>\r\n\r\n"+
			ok("final")),
		single("resp_no_reason", "HTTP/1.1 200\r\nContent-Length: 2\r\n\r\nok"),
		single("resp_bare_lf", "HTTP/1.1 200 OK\nContent-Length: 2\nX-A: b\n\nok"),
		single("resp_te_identity", "HTTP/1.1 200 OK\r\nTransfer-Encoding: identity\r\nContent-Length: 2\r\n\r\nok"),
		single("resp_te_unsupported", "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n"),
		single("resp_pragma_no_cache", "HTTP/1.1 200 OK\r\nPragma: no-cache\r\nContent-Length: 0\r\n\r\n"),
		single("resp_status_500", response("500 Internal Server Error", "Content-Type: text/plain\r\n", "boom")),
	}
}

func gzipCases() []Case {
	payload := strings.Repeat("hello gzip world ", 20)
	good := gz(payload)
	badCRC := append([]byte{}, good...)
	badCRC[len(badCRC)-8] ^= 0xff
	single := func(name string, resp []byte) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), writeBytes(resp)))}, get("http://{{A}}/gz"))
	}
	chunkedGz := []byte("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n")
	chunkedGz = append(chunkedGz, []byte(fmt.Sprintf("%x\r\n", len(good)))...)
	chunkedGz = append(chunkedGz, good...)
	chunkedGz = append(chunkedGz, []byte("\r\n0\r\n\r\n")...)
	userAE := withHeader(get("http://{{A}}/gz"), "Accept-Encoding", "gzip")
	ranged := withHeader(get("http://{{A}}/gz"), "Range", "bytes=0-10")
	noCompression := general()
	noCompression.DisableCompression = true
	return []Case{
		single("gzip_ok", gzipResponse("", good)),
		single("gzip_uppercase_encoding", []byte(strings.Replace(string(gzipResponse("", good)), "gzip", "GZIP", 1))),
		single("gzip_chunked", chunkedGz),
		single("gzip_not_gzip", gzipResponse("", []byte("not gzip data at all"))),
		single("gzip_truncated", gzipResponse("", good[:len(good)/2])),
		single("gzip_truncated_trailer", gzipResponse("", good[:len(good)-3])),
		single("gzip_bad_checksum", gzipResponse("", badCRC)),
		single("gzip_multistream", gzipResponse("", append(gz("part one, "), gz("part two")...))),
		single("gzip_trailing_garbage", gzipResponse("", append(gz("member"), []byte("junk!!!!!!!!")...))),
		single("gzip_empty_content_length", []byte("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n")),
		one("gzip_close_delimited", general(), []Server{srv("A", conn(reqOp(), writeBytes(append([]byte("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nConnection: close\r\n\r\n"), good...)), closeOp()))}, get("http://{{A}}/gz")),
		one("gzip_user_accept_encoding", general(), []Server{srv("A", conn(reqOp(), writeBytes(gzipResponse("", good))))}, userAE),
		one("gzip_range_request", general(), []Server{srv("A", conn(reqOp(), writeBytes(gzipResponse("", good))))}, ranged),
		one("gzip_disable_compression", noCompression,
			[]Server{srv("A", conn(reqOp(), writeBytes(gzipResponse("", good))))}, get("http://{{A}}/gz")),
		one("gzip_head", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 30\r\n\r\n")))},
			do("HEAD", "http://{{A}}/gz", Body{Kind: "none"})),
		one("gzip_reuse", general(),
			[]Server{srv("A", conn(reqOp(), writeBytes(gzipResponse("", good)), reqOp(), write(ok("plain"))))},
			get("http://{{A}}/gz"), get("http://{{A}}/plain")),
	}
}

func redirectCases() []Case {
	var out []Case
	reasons := map[int]string{301: "Moved Permanently", 302: "Found", 303: "See Other", 307: "Temporary Redirect", 308: "Permanent Redirect"}
	for _, code := range []int{301, 302, 303, 307, 308} {
		status := fmt.Sprintf("%d %s", code, reasons[code])
		out = append(out,
			one(fmt.Sprintf("redirect_get_%d", code), general(),
				[]Server{srv("A", conn(reqOp(), write(response(status, "Location: /next\r\n", "")),
					reqOp(), write(ok("done"))))},
				withHeader(get("http://{{A}}/start"), "X-Keep", "1")),
			one(fmt.Sprintf("redirect_post_%d", code), general(),
				[]Server{srv("A", conn(reqOp(), write(response(status, "Location: /next\r\n", "moved")),
					reqOp(), write(ok("done"))))},
				withHeader(do("POST", "http://{{A}}/form", bytesBody("a=1&b=2")),
					"Content-Type", "application/x-www-form-urlencoded", "Content-Encoding", "identity",
					"Content-Language", "en", "X-Keep", "1")),
		)
	}
	// The other 3xx codes are returned to the caller even with a Location header.
	notFollowed := map[int]string{300: "Multiple Choices", 304: "Not Modified", 305: "Use Proxy", 306: "Switch Proxy"}
	for _, code := range []int{300, 304, 305, 306} {
		status := fmt.Sprintf("%d %s", code, notFollowed[code])
		out = append(out, one(fmt.Sprintf("redirect_not_followed_%d", code), general(),
			[]Server{srv("A", conn(reqOp(), write(response(status, "Location: /next\r\n", "")), reqOp(), write(ok("unused"))))},
			get("http://{{A}}/start"), get("http://{{A}}/again")))
	}
	redirects := conn()
	for i := 1; i <= 11; i++ {
		redirects = append(redirects, reqOp(), write(response("302 Found", fmt.Sprintf("Location: /r%d\r\n", i), "")))
	}
	hostOverride := get("http://{{A}}/start")
	hostOverride.Req.Host = "virtual.test"
	useLast := general()
	useLast.Redirect = "use_last"
	refuse := general()
	refuse.Redirect = "error"
	return append(out,
		one("redirect_post_307_reader", general(),
			[]Server{srv("A", conn(reqOp(), write(response("307 Temporary Redirect", "Location: /again\r\n", "no"))))},
			do("POST", "http://{{A}}/stream", readerBody("streamed"))),
		one("redirect_head_303", general(),
			[]Server{srv("A", conn(reqOp(), write(response("303 See Other", "Location: /other\r\n", "")),
				reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n")))},
			do("HEAD", "http://{{A}}/head", Body{Kind: "none"})),
		one("redirect_limit", general(), []Server{srv("A", redirects)}, get("http://{{A}}/r0")),
		one("redirect_missing_location", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "", "nowhere"))))}, get("http://{{A}}/")),
		one("redirect_bad_location", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: http://[::1\r\n", ""))))}, get("http://{{A}}/")),
		one("redirect_use_last_response", useLast,
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", "redirect body"))))},
			get("http://{{A}}/")),
		one("redirect_check_error", refuse,
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", "redirect body"))))},
			get("http://{{A}}/")),
		one("redirect_cross_host", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: http://other.test/landing\r\n", "")))),
				srv("P", conn(reqOp(), write(ok("landed"))))},
			withHeader(get("http://{{A}}/start"), "Authorization", "Bearer secret", "Cookie", "a=1",
				"Www-Authenticate", "Basic", "Proxy-Authorization", "Basic mine", "X-Keep", "1")),
		one("redirect_same_host_other_port", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: http://{{B}}/x\r\n", "")))),
				srv("B", conn(reqOp(), write(ok("b"))))},
			withHeader(get("http://{{A}}/start"), "Authorization", "Bearer secret", "Cookie", "a=1")),
		one("redirect_relative_forms", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: ../c?x=1\r\n", "")),
				reqOp(), write(response("302 Found", "Location: ?y=2\r\n", "")),
				reqOp(), write(response("302 Found", "Location: //{{B}}/d#frag\r\n", "")))),
				srv("B", conn(reqOp(), write(ok("d"))))},
			get("http://{{A}}/a/b")),
		one("redirect_large_body_not_slurped", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", strings.Repeat("x", 3000)))),
				conn(reqOp(), write(ok("fresh connection"))))},
			get("http://{{A}}/big")),
		one("redirect_chunked_body_slurped", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 302 Found\r\nLocation: /next\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nmove\r\n0\r\n\r\n"),
				reqOp(), write(ok("same connection"))))},
			get("http://{{A}}/chunked")),
		one("redirect_host_override_relative", general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", "")),
				reqOp(), write(response("302 Found", "Location: http://{{A}}/abs\r\n", "")),
				reqOp(), write(ok("ok"))))},
			hostOverride),
		one("redirect_https_to_http", general(),
			[]Server{srv("A", conn(tlsOp("ip"), reqOp(), write(response("302 Found", "Location: http://{{B}}/plain\r\n", "")))),
				srv("B", conn(reqOp(), write(ok("plain"))))},
			withHeader(get("https://{{A}}/secure"), "Authorization", "Bearer secret")),
	)
}

func timeoutCases() []Case {
	withTimeout := func(ms int) Client {
		c := general()
		c.TimeoutMs = ms
		return c
	}
	headerTimeout := general()
	headerTimeout.ResponseHeaderTimeoutMs = 150
	tlsTimeout := general()
	tlsTimeout.TLSHandshakeTimeoutMs = 150
	ctxStep := get("http://{{A}}/ctx")
	ctxStep.Req.CtxTimeoutMs = 150
	ctxBody := get("http://{{A}}/ctx-body")
	ctxBody.Req.CtxTimeoutMs = 200
	return []Case{
		one("timeout_client_headers", withTimeout(150),
			[]Server{srv("A", conn(reqOp(), sleepOp(600), write(ok("late"))))}, get("http://{{A}}/slow")),
		one("timeout_client_body", withTimeout(200),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello"), sleepOp(700), write("world")))},
			get("http://{{A}}/slow-body")),
		one("timeout_client_not_reached", withTimeout(2000),
			[]Server{srv("A", conn(reqOp(), sleepOp(50), write(ok("in time"))))}, get("http://{{A}}/")),
		one("timeout_response_header", headerTimeout,
			[]Server{srv("A", conn(reqOp(), sleepOp(600), write(ok("late"))))}, get("http://{{A}}/slow")),
		one("timeout_context", general(),
			[]Server{srv("A", conn(reqOp(), sleepOp(600), write(ok("late"))))}, ctxStep),
		one("timeout_context_body", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello"), sleepOp(700), write("world")))},
			ctxBody),
		one("timeout_tls_handshake", tlsTimeout, []Server{srv("A", conn(skipOp(800)))}, get("https://{{A}}/")),
	}
}

func failureCases() []Case {
	single := func(name string, ops ...Op) Case {
		return one(name, general(), []Server{srv("A", conn(ops...))}, get("http://{{A}}/"))
	}
	return []Case{
		single("reset_after_request", reqOp(), resetOp()),
		single("close_after_request", reqOp(), closeOp()),
		single("truncated_content_length_body", reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabcd"), closeOp()),
		single("truncated_chunked_body", reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel"), closeOp()),
		single("chunked_missing_terminator", reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n"), closeOp()),
		single("chunked_bad_size", reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n"), closeOp()),
		single("chunked_bad_trailer", reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\nBad Trailer\r\n\r\n"), closeOp()),
		single("reset_mid_body", reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial"), sleepOp(50), resetOp()),
		single("truncated_head", reqOp(), write("HTTP/1.1 200 OK\r\nContent-Le"), closeOp()),
		single("truncated_status_line", reqOp(), write("HTTP/1.1 2"), closeOp()),
		one("dial_refused", general(), []Server{{Name: "X", Closed: true}}, get("http://{{X}}/")),
	}
}

func poolCases() []Case {
	idlePerHost := general()
	idlePerHost.MaxIdleConnsPerHost = 1
	connsPerHost := general()
	connsPerHost.MaxConnsPerHost = 1
	idleTimeout := general()
	idleTimeout.IdleConnTimeoutMs = 100
	noKeepAlive := general()
	noKeepAlive.DisableKeepAlives = true
	noIdle := general()
	noIdle.MaxIdleConnsPerHost = -1
	maxIdle := general()
	maxIdle.MaxIdleConns = 1
	unread := get("http://{{A}}/unread")
	unread.Read = "none"
	return []Case{
		one("keepalive_reuse", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), write(ok("2")), reqOp(), write(ok("3"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2"), get("http://{{A}}/3")),
		one("keepalive_server_closed_idle", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), closeOp()), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), sleepFirst(100, get("http://{{A}}/2"))),
		one("keepalive_retry_get", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), closeOp()), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
		one("keepalive_no_retry_post", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), closeOp()))},
			get("http://{{A}}/1"), do("POST", "http://{{A}}/2", bytesBody("payload"))),
		one("keepalive_retry_idempotency_key", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), closeOp()), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), withHeader(do("POST", "http://{{A}}/2", bytesBody("payload")), "Idempotency-Key", "k1")),
		one("keepalive_reset_on_reuse", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("1")), reqOp(), resetOp()), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
		one("keepalive_unread_body", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("unread body"))), conn(reqOp(), write(ok("2"))))},
			unread, get("http://{{A}}/2")),
		one("max_idle_conns_per_host", idlePerHost,
			[]Server{srv("A", conn(reqOp(), sleepOp(100), write(ok("1")), reqOp(), write(ok("3"))),
				conn(reqOp(), sleepOp(200), write(ok("2"))))},
			async(get("http://{{A}}/1")), async(sleepFirst(20, get("http://{{A}}/2"))),
			sleepFirst(400, get("http://{{A}}/3"))),
		one("max_conns_per_host", connsPerHost,
			[]Server{srv("A", conn(reqOp(), sleepOp(100), write(ok("1")), reqOp(), write(ok("2"))))},
			async(get("http://{{A}}/1")), sleepFirst(20, get("http://{{A}}/2"))),
		one("max_idle_conns", maxIdle,
			[]Server{srv("A", conn(reqOp(), write(ok("a"))), conn(reqOp(), write(ok("a2")))),
				srv("B", conn(reqOp(), write(ok("b")), reqOp(), write(ok("b2"))))},
			get("http://{{A}}/"), get("http://{{B}}/"), get("http://{{B}}/again"), get("http://{{A}}/again")),
		one("idle_conn_timeout", idleTimeout,
			[]Server{srv("A", conn(reqOp(), write(ok("1"))), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), sleepFirst(300, get("http://{{A}}/2"))),
		one("disable_keep_alives", noKeepAlive,
			[]Server{srv("A", conn(reqOp(), write(ok("1"))), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
		one("negative_idle_per_host", noIdle,
			[]Server{srv("A", conn(reqOp(), write(ok("1"))), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2")),
	}
}

func proxyCases() []Case {
	return []Case{
		one("proxy_http", general(), []Server{srv("P", conn(reqOp(), write(ok("via proxy"))))},
			get("http://origin.test/path?x=1")),
		one("proxy_http_port", general(), []Server{srv("P", conn(reqOp(), write(ok("via proxy"))))},
			withHeader(get("http://origin.test:8080/p"), "Authorization", "Bearer token")),
		one("proxy_http_407", general(),
			[]Server{srv("P", conn(reqOp(), write(response("407 Proxy Authentication Required", "Proxy-Authenticate: Basic\r\n", ""))))},
			get("http://origin.test/")),
		one("proxy_connect_tls", general(),
			[]Server{srv("P", conn(reqOp(), write("HTTP/1.1 200 Connection established\r\n\r\n"), tlsOp("origin"),
				reqOp(), write(ok("tunneled")), reqOp(), write(ok("again"))))},
			get("https://origin.test/secure"), get("https://origin.test/again")),
		one("proxy_connect_407", general(),
			[]Server{srv("P", conn(reqOp(), write(response("407 Proxy Authentication Required", "", ""))))},
			get("https://origin.test/secure")),
		one("proxy_connect_no_reason", general(),
			[]Server{srv("P", conn(reqOp(), write("HTTP/1.1 503\r\nContent-Length: 0\r\n\r\n")))},
			get("https://origin.test/secure")),
		one("proxy_connect_eof", general(), []Server{srv("P", conn(reqOp(), closeOp()))}, get("https://origin.test/secure")),
		one("proxy_connect_bad_cert", general(),
			[]Server{srv("P", conn(reqOp(), write("HTTP/1.1 200 OK\r\n\r\n"), tlsOp("dnsonly")))},
			get("https://origin.test/secure")),
		one("proxy_redirect_to_loopback", general(),
			[]Server{srv("P", conn(reqOp(), write(response("302 Found", "Location: http://{{A}}/direct\r\n", "")))),
				srv("A", conn(reqOp(), write(ok("direct"))))},
			withHeader(get("http://origin.test/start"), "Authorization", "Bearer secret", "X-Keep", "1")),
	}
}

func tlsCases() []Case {
	untrusted := general()
	untrusted.TrustCA = false
	single := func(name string, client Client, cert string) Case {
		return one(name, client, []Server{srv("A", conn(tlsOp(cert), reqOp(), write(ok("secure"))))}, get("https://{{A}}/"))
	}
	return []Case{
		single("tls_ok", general(), "ip"),
		single("tls_unknown_authority", untrusted, "ip"),
		single("tls_no_ip_sans", general(), "dnsonly"),
		single("tls_ip_mismatch", general(), "otherip"),
		single("tls_expired", general(), "expired"),
		one("tls_keepalive_reuse", general(),
			[]Server{srv("A", conn(tlsOp("ip"), reqOp(), write(ok("1")), reqOp(), write(ok("2"))))},
			get("https://{{A}}/1"), get("https://{{A}}/2")),
		one("tls_http_response", general(),
			[]Server{srv("A", conn(skipOp(100), write("HTTP/1.1 400 Bad Request\r\n\r\n"), closeOp()))},
			get("https://{{A}}/")),
		one("tls_server_closes", general(), []Server{srv("A", conn(skipOp(50), closeOp()))}, get("https://{{A}}/")),
	}
}

func continueCases() []Case {
	expect := func() Step {
		return withHeader(do("POST", "http://{{A}}/upload", bytesBody("the body")), "Expect", "100-continue")
	}
	short := general()
	short.ExpectContinueTimeoutMs = 100
	zero := general()
	zero.ExpectContinueTimeoutMs = 0
	return []Case{
		one("expect_100_continue", general(),
			[]Server{srv("A", conn(headOp(), write("HTTP/1.1 100 Continue\r\n\r\n"), bodyOp(), write(ok("got it"))))},
			expect()),
		one("expect_417_keepalive", general(),
			[]Server{srv("A", conn(headOp(), write(response("417 Expectation Failed", "", "")), bodyOp(),
				reqOp(), write(ok("next"))))},
			expect(), get("http://{{A}}/next")),
		// The write goroutine reports WroteRequest for the skipped body after the response
		// has been handed over, so the trace is read once it has settled.
		one("expect_417_close", general(),
			[]Server{srv("A", conn(headOp(), write(response("417 Expectation Failed", "Connection: close\r\n", ""))))},
			settle(100, expect())),
		one("expect_timeout_sends_body", short,
			[]Server{srv("A", conn(headOp(), bodyOp(), write(ok("late body"))))}, expect()),
		one("expect_zero_timeout", zero,
			[]Server{srv("A", conn(headOp(), bodyOp(), write(ok("at once"))))}, expect()),
		one("expect_without_body", general(),
			[]Server{srv("A", conn(reqOp(), write(ok("no body"))))},
			withHeader(get("http://{{A}}/"), "Expect", "100-continue")),
	}
}

func requestErrorCases() []Case {
	badName := get("http://{{A}}/")
	badName.Req.RawHeader = [][2]string{{"Bad Name", "v"}}
	return []Case{
		one("err_unsupported_scheme", general(), nil, get("ftp://{{A}}/x")),
		one("err_no_host", general(), nil, get("http:///path")),
		one("err_invalid_header_value", general(), nil, withHeader(get("http://{{A}}/"), "X-Bad", "a\nb")),
		one("err_invalid_header_name", general(), nil, badName),
		one("err_invalid_method", general(), nil, do("BAD METHOD", "http://{{A}}/", Body{Kind: "none"})),
		one("err_bad_url", general(), nil, get("http://[::1")),
		one("err_invalid_host", general(), nil, get("http://exa mple.test/")),
	}
}
