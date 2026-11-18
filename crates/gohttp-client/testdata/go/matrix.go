package main

import (
	"fmt"
	"strings"
)

// matrixCases are generated cases: request framing for every method and body kind, Host
// header forms, User-Agent and Accept-Encoding handling, Content-Encoding values, the redirect
// rules for every method and status, Client.Timeout against context deadlines, connection
// reuse after an unread body, unusual response versions and methods set after
// NewRequestWithContext.
func matrixCases() []Case {
	var all []Case
	all = append(all, framingCases()...)
	all = append(all, hostCases()...)
	all = append(all, userAgentCases()...)
	all = append(all, encodingCases()...)
	all = append(all, redirectMatrixCases()...)
	all = append(all, timeoutMatrixCases()...)
	all = append(all, reuseCases()...)
	all = append(all, versionCases()...)
	all = append(all, methodCases()...)
	all = append(all, headerWriteCases()...)
	all = append(all, locationCases()...)
	all = append(all, urlFormCases()...)
	all = append(all, tlsNameCases()...)
	return all
}

// tlsNameCases tunnel to the "origin" certificate (DNS name origin.test) under other spellings
// of its name, and to an IPv6 literal it does not cover.
func tlsNameCases() []Case {
	tunnel := func(name, url string) Case {
		return one(name, general(),
			[]Server{srv("P", conn(reqOp(), write("HTTP/1.1 200 Connection established\r\n\r\n"), tlsOp("origin"),
				reqOp(), write(ok("tunneled"))))},
			get(url))
	}
	return []Case{
		tunnel("tls_name_trailing_dot", "https://origin.test./secure"),
		tunnel("tls_name_upper_case", "https://ORIGIN.Test/secure"),
		tunnel("tls_name_explicit_port", "https://origin.test:443/secure"),
		tunnel("tls_name_ipv6_literal", "https://[2001:db8::1]/secure"),
	}
}

func headerWriteCases() []Case {
	single := func(name string, client Client, st Step) Case {
		return one(name, client, []Server{srv("A", conn(reqOp(), write(emptyOK)))}, st)
	}
	noKeepAlive := general()
	noKeepAlive.DisableKeepAlives = true
	excluded := withHeader(do("POST", "http://{{A}}/x", bytesBody("abc")),
		"Host", "evil.test", "Content-Length", "99", "Transfer-Encoding", "chunked", "Trailer", "X-T", "X-Keep", "1")
	withTrailer := func(st Step, kv ...string) Step {
		for i := 0; i+1 < len(kv); i += 2 {
			st.Req.Trailer = append(st.Req.Trailer, [2]string{kv[i], kv[i+1]})
		}
		return st
	}
	return []Case{
		single("header_write_excluded_keys", general(), excluded),
		single("header_disable_keep_alives_keep_alive_header", noKeepAlive,
			withHeader(get("http://{{A}}/x"), "Connection", "keep-alive")),
		single("header_disable_keep_alives_upgrade", noKeepAlive,
			withHeader(get("http://{{A}}/x"), "Connection", "Upgrade", "Upgrade", "websocket")),
		single("header_values_newlines_trimmed", general(),
			withHeader(get("http://{{A}}/x"), "X-Tabs", "\tvalue\t", "X-Empty", "")),
		single("trailer_with_length", general(),
			withTrailer(do("POST", "http://{{A}}/x", bytesBody("abc")), "X-Sum", "1")),
		single("trailer_reader_chunked", general(),
			withTrailer(do("PUT", "http://{{A}}/x", readerBody("ab", "cd")), "x-lower", "1", "X-Other", "2")),
		single("trailer_invalid_key", general(),
			withTrailer(do("POST", "http://{{A}}/x", readerBody("ab")), "Content-Length", "1")),
		single("trailer_invalid_value", general(),
			withTrailer(do("POST", "http://{{A}}/x", readerBody("ab")), "X-Bad", "a\r\nb")),
		one("resp_two_transfer_encodings", general(),
			[]Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n"+chunked("x")), closeOp()))},
			get("http://{{A}}/")),
	}
}

func locationCases() []Case {
	redirectTo := func(name, location string) Case {
		return one(name, general(),
			[]Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: "+location+"\r\n", "")),
				reqOp(), write(ok("landed")))),
				srv("B", conn(reqOp(), write(ok("b"))))},
			get("http://{{A}}/start/here?q=1"))
	}
	return []Case{
		redirectTo("location_space", "/a b"),
		redirectTo("location_non_ascii", "/ü?é=1"),
		redirectTo("location_bad_escape", "/%zz"),
		redirectTo("location_backslash", "/\\evil"),
		redirectTo("location_fragment_only", "#frag"),
		redirectTo("location_query_only", "?x=2"),
		redirectTo("location_dot_segments", "./../a/./b/../c"),
		redirectTo("location_upper_scheme", "HTTP://{{B}}/x"),
		redirectTo("location_userinfo", "http://u:p@{{B}}/x"),
		redirectTo("location_blank", "   "),
		redirectTo("location_encoded_slash", "/a%2Fb/%41"),
	}
}

func urlFormCases() []Case {
	single := func(name, url string) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), write(emptyOK)))}, get(url))
	}
	return []Case{
		single("url_path_space", "http://{{A}}/a b"),
		single("url_force_query", "http://{{A}}/p?"),
		single("url_no_path_query", "http://{{A}}?x=1"),
		single("url_encoded_query", "http://{{A}}/p?a=%2F&b=%zz&c=d+e"),
		single("url_non_ascii_path", "http://{{A}}/ü"),
		single("url_double_slash_path", "http://{{A}}//a//b"),
		single("url_dot_path", "http://{{A}}/a/./b/../c"),
	}
}

// emptyOK is a 200 response with an empty Content-Length body (also valid for HEAD).
const emptyOK = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"

func lengthStep(n int64, st Step) Step {
	st.Req.ContentLength = &n
	return st
}

func teStep(te string, st Step) Step {
	st.Req.TransferEncoding = []string{te}
	return st
}

func framingCases() []Case {
	bodies := []struct {
		name string
		body Body
	}{
		{"nil", Body{Kind: "none"}},
		{"nobody", Body{Kind: "nobody"}},
		{"bytes_empty", bytesBody("")},
		{"bytes", bytesBody("abc")},
		{"reader_empty", readerBody()},
		{"reader", readerBody("ab", "c")},
	}
	var out []Case
	for _, method := range []string{"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "PROPFIND", "get"} {
		for _, b := range bodies {
			name := fmt.Sprintf("framing_%s_%s", strings.ToLower(method), b.name)
			if method == "get" {
				name = "framing_lowercase_get_" + b.name
			}
			out = append(out, one(name, general(), []Server{srv("A", conn(reqOp(), write(emptyOK)))},
				do(method, "http://{{A}}/f", b.body)))
		}
	}
	single := func(name string, st Step) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), write(emptyOK)))}, st)
	}
	// slow bodies outlast the 200ms probe of methods that usually lack a body.
	slow := func(pieces ...string) Body {
		b := readerBody(pieces...)
		b.DelayMs = 400
		return b
	}
	return append(out,
		single("framing_get_slow_reader", do("GET", "http://{{A}}/f", slow("abc"))),
		single("framing_get_slow_reader_pieces", do("GET", "http://{{A}}/f", slow("ab", "cd"))),
		single("framing_get_slow_reader_empty", do("GET", "http://{{A}}/f", slow())),
		single("framing_post_slow_reader", do("POST", "http://{{A}}/f", slow("abc"))),
		single("framing_post_bytes_te_chunked", teStep("chunked", do("POST", "http://{{A}}/f", bytesBody("abc")))),
		single("framing_put_bytes_te_identity", teStep("identity", do("PUT", "http://{{A}}/f", bytesBody("abc")))),
		single("framing_delete_empty_te_identity", teStep("identity", do("DELETE", "http://{{A}}/f", bytesBody("")))),
		single("framing_options_nil_te_identity", teStep("identity", do("OPTIONS", "http://{{A}}/f", Body{Kind: "none"}))),
		single("framing_get_nil_te_identity", teStep("identity", do("GET", "http://{{A}}/f", Body{Kind: "none"}))),
		single("framing_get_nil_content_length", lengthStep(5, get("http://{{A}}/f"))),
		single("framing_post_nobody_content_length", lengthStep(5, do("POST", "http://{{A}}/f", Body{Kind: "nobody"}))),
		single("framing_post_reader_unknown_length", lengthStep(-1, do("POST", "http://{{A}}/f", readerBody("abc")))),
		single("framing_get_bytes_unknown_length", lengthStep(0, do("GET", "http://{{A}}/f", bytesBody("abc")))),
		single("framing_post_bytes_short_length", lengthStep(3, do("POST", "http://{{A}}/f", bytesBody("abcdef")))),
		single("framing_post_bytes_long_length", lengthStep(10, do("POST", "http://{{A}}/f", bytesBody("abc")))),
	)
}

func hostCases() []Case {
	viaProxy := func(name, url string) Case {
		return one(name, general(), []Server{srv("P", conn(reqOp(), write(emptyOK)))}, get(url))
	}
	withHost := func(host string, st Step) Step {
		st.Req.Host = host
		return st
	}
	direct := func(name, host string) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), write(emptyOK)))},
			withHost(host, get("http://{{A}}/h")))
	}
	return []Case{
		viaProxy("host_proxy_port_80", "http://origin.test:80/p"),
		viaProxy("host_proxy_empty_port", "http://origin.test:/p"),
		viaProxy("host_proxy_mixed_case", "http://ORIGIN.Test/p"),
		viaProxy("host_proxy_trailing_dot", "http://origin.test./p"),
		viaProxy("host_proxy_ipv6", "http://[2001:db8::1]/p"),
		viaProxy("host_proxy_ipv6_port", "http://[2001:db8::1]:8080/p"),
		viaProxy("host_proxy_ipv6_zone", "http://[fe80::1%25en0]:8080/p"),
		viaProxy("host_proxy_idn", "http://bücher.test/p"),
		viaProxy("host_proxy_punycode", "http://xn--bcher-kva.test/p"),
		viaProxy("host_proxy_no_path", "http://origin.test"),
		viaProxy("host_proxy_query_no_path", "http://origin.test?x=1#frag"),
		viaProxy("host_proxy_user_no_password", "http://user@origin.test/p"),
		viaProxy("host_proxy_upper_scheme", "HTTP://origin.test/p"),
		one("host_proxy_https_port_443", general(),
			[]Server{srv("P", conn(reqOp(), write(response("403 Forbidden", "", ""))))},
			get("https://origin.test:443/p")),
		one("host_proxy_https_ipv6", general(),
			[]Server{srv("P", conn(reqOp(), write(response("403 Forbidden", "", ""))))},
			get("https://[2001:db8::1]/p")),
		direct("host_override_port_80", "virtual.test:80"),
		direct("host_override_ipv6", "[2001:db8::2]:81"),
		direct("host_override_ipv6_zone", "[fe80::1%en0]:81"),
		direct("host_override_invalid", "bad host"),
		direct("host_override_slash", "a/b"),
		direct("host_override_idn", "bücher.test:81"),
		one("host_override_proxy", general(), []Server{srv("P", conn(reqOp(), write(emptyOK)))},
			withHost("virtual.test", get("http://origin.test/p"))),
		one("host_override_proxy_invalid", general(), []Server{srv("P", conn(reqOp(), write(emptyOK)))},
			withHost("bad host", get("http://origin.test/p"))),
		// Request.write checks the Host before it builds the transfer writer.
		one("host_override_proxy_invalid_before_framing", general(), []Server{srv("P", conn(reqOp(), write(emptyOK)))},
			withHost("bad host", lengthStep(5, get("http://origin.test/p")))),
	}
}

func userAgentCases() []Case {
	single := func(name string, st Step) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), write(emptyOK)))}, st)
	}
	rawLower := get("http://{{A}}/ua")
	rawLower.Req.RawHeader = [][2]string{{"user-agent", "raw lower"}}
	return []Case{
		single("ua_multiple_values", withHeader(get("http://{{A}}/ua"), "User-Agent", "first", "User-Agent", "second")),
		single("ua_padded", withHeader(get("http://{{A}}/ua"), "User-Agent", "  padded ua  ")),
		single("ua_spaces_only", withHeader(get("http://{{A}}/ua"), "User-Agent", "   ")),
		single("ua_tab_inside", withHeader(get("http://{{A}}/ua"), "User-Agent", "a\tb")),
		single("ua_raw_lowercase_key", rawLower),
	}
}

func encodingCases() []Case {
	payload := gz("compressed payload")
	single := func(name string, resp []byte, st Step) Case {
		return one(name, general(), []Server{srv("A", conn(reqOp(), writeBytes(resp)))}, st)
	}
	rawAE := get("http://{{A}}/gz")
	rawAE.Req.RawHeader = [][2]string{{"accept-encoding", "br"}}
	rawRange := get("http://{{A}}/gz")
	rawRange.Req.RawHeader = [][2]string{{"range", "bytes=0-1"}}
	out := []Case{
		single("ae_raw_lowercase_key", gzipResponse("", payload), rawAE),
		single("ae_raw_lowercase_range", gzipResponse("", payload), rawRange),
		single("ae_identity_gzip_response", gzipResponse("", payload),
			withHeader(get("http://{{A}}/gz"), "Accept-Encoding", "identity")),
		single("ae_empty_value", gzipResponse("", payload),
			withHeader(get("http://{{A}}/gz"), "Accept-Encoding", "")),
		single("ae_post_gzip_response", gzipResponse("", payload), do("POST", "http://{{A}}/gz", bytesBody("q"))),
		single("ce_204_gzip", []byte("HTTP/1.1 204 No Content\r\nContent-Encoding: gzip\r\n\r\n"), get("http://{{A}}/gz")),
		single("ce_duplicate_values", []byte(strings.Replace(string(gzipResponse("", payload)),
			"Content-Encoding: gzip\r\n", "Content-Encoding: gzip\r\nContent-Encoding: br\r\n", 1)), get("http://{{A}}/gz")),
	}
	for i, ce := range []string{"x-gzip", "gzip, identity", "Gzip", "gzip ", "\tgzip", "identity", "br"} {
		resp := strings.Replace(string(gzipResponse("", payload)), "Content-Encoding: gzip", "Content-Encoding: "+ce, 1)
		out = append(out, single(fmt.Sprintf("ce_value_%d", i), []byte(resp), get("http://{{A}}/gz")))
	}
	return out
}

func redirectMatrixCases() []Case {
	reasons := map[int]string{301: "Moved Permanently", 302: "Found", 303: "See Other", 307: "Temporary Redirect", 308: "Permanent Redirect"}
	redirect := func(code int, location string) string {
		return fmt.Sprintf("HTTP/1.1 %d %s\r\nLocation: %s\r\nContent-Length: 0\r\n\r\n", code, reasons[code], location)
	}
	var out []Case
	for _, method := range []string{"HEAD", "PUT", "PATCH", "DELETE", "OPTIONS"} {
		for _, code := range []int{301, 302, 303, 307, 308} {
			body, final := bytesBody("k=v"), ok("done")
			if method == "HEAD" {
				body, final = Body{Kind: "none"}, "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n"
			}
			st := withHeader(do(method, "http://{{A}}/from", body), "Content-Type", "text/plain", "X-Keep", "1")
			out = append(out, one(fmt.Sprintf("redirect_matrix_%s_%d", strings.ToLower(method), code), general(),
				[]Server{srv("A", conn(reqOp(), write(redirect(code, "/to")), reqOp(), write(final)))}, st))
		}
	}
	twoHops := func(name string, code int, st Step) Case {
		return one(name, general(),
			[]Server{srv("A", conn(reqOp(), write(redirect(code, "/to")), reqOp(), write(ok("done"))))}, st)
	}
	noGetBody := func() Step {
		st := withHeader(do("POST", "http://{{A}}/from", bytesBody("k=v")), "Content-Type", "text/plain")
		st.Req.NoGetBody = true
		return st
	}
	rawKeys := func() Step {
		st := do("POST", "http://{{A}}/from", bytesBody("k=v"))
		st.Req.RawHeader = [][2]string{{"authorization", "Bearer x"}, {"content-type", "text/plain"},
			{"cookie", "c=1"}, {"x-keep", "1"}}
		return st
	}
	return append(out,
		twoHops("redirect_post_307_no_get_body", 307, noGetBody()),
		twoHops("redirect_post_302_no_get_body", 302, noGetBody()),
		twoHops("redirect_post_307_nobody", 307, do("POST", "http://{{A}}/from", Body{Kind: "nobody"})),
		twoHops("redirect_post_308_nil", 308, do("POST", "http://{{A}}/from", Body{Kind: "none"})),
		twoHops("redirect_post_307_reader_empty", 307, do("POST", "http://{{A}}/from", readerBody())),
		twoHops("redirect_raw_keys_same_host_307", 307, rawKeys()),
		twoHops("redirect_explicit_referer", 302, withHeader(get("http://{{A}}/from"), "Referer", "http://explicit.test/")),
		one("redirect_raw_keys_cross_host", general(),
			[]Server{srv("A", conn(reqOp(), write(redirect(302, "http://other.test/landing")))),
				srv("P", conn(reqOp(), write(ok("landed"))))},
			rawKeys()),
	)
}

func timeoutMatrixCases() []Case {
	slowHead := func() []Server { return []Server{srv("A", conn(reqOp(), sleepOp(700), write(ok("late"))))} }
	slowBody := func() []Server {
		return []Server{srv("A", conn(reqOp(), write("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello"),
			sleepOp(800), write("world")))}
	}
	slowHop := func() []Server {
		return []Server{srv("A", conn(reqOp(), write(response("302 Found", "Location: /next\r\n", "")),
			reqOp(), sleepOp(700), write(ok("late"))))}
	}
	timed := func(name string, clientMs, ctxMs int, servers []Server, st Step) Case {
		c := general()
		c.TimeoutMs = clientMs
		st.Req.CtxTimeoutMs = ctxMs
		return one(name, c, servers, st)
	}
	expect := withHeader(do("POST", "http://{{A}}/upload", bytesBody("the body")), "Expect", "100-continue")
	return []Case{
		timed("timeout_both_client_first_headers", 150, 450, slowHead(), get("http://{{A}}/t")),
		timed("timeout_both_ctx_first_headers", 450, 150, slowHead(), get("http://{{A}}/t")),
		timed("timeout_both_client_first_body", 200, 500, slowBody(), get("http://{{A}}/t")),
		timed("timeout_both_ctx_first_body", 500, 200, slowBody(), get("http://{{A}}/t")),
		timed("timeout_client_redirect_hop", 200, 0, slowHop(), get("http://{{A}}/t")),
		timed("timeout_ctx_redirect_hop", 0, 200, slowHop(), get("http://{{A}}/t")),
		timed("timeout_client_tls_handshake", 150, 0, []Server{srv("A", conn(skipOp(800), closeOp()))},
			get("https://{{A}}/t")),
		// The write loop reports WroteRequest once the canceled connection closes, so the
		// trace is read after it settles.
		timed("timeout_client_expect_continue", 150, 0, []Server{srv("A", conn(headOp(), sleepOp(700)))},
			settle(150, expect)),
	}
}

func reuseCases() []Case {
	unread := func(method string) Step {
		st := do(method, "http://{{A}}/1", Body{Kind: "none"})
		st.Read = "none"
		return st
	}
	mk := func(name, method, first string) Case {
		return one(name, general(),
			[]Server{srv("A", conn(reqOp(), write(first), reqOp(), write(ok("2"))), conn(reqOp(), write(ok("2"))))},
			unread(method), get("http://{{A}}/2"))
	}
	return []Case{
		mk("reuse_unread_empty_length", "GET", emptyOK),
		mk("reuse_unread_204", "GET", "HTTP/1.1 204 No Content\r\n\r\n"),
		mk("reuse_unread_304", "GET", "HTTP/1.1 304 Not Modified\r\n\r\n"),
		mk("reuse_unread_empty_chunked", "GET", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
		mk("reuse_unread_gzip", "GET", string(gzipResponse("", gz("x")))),
		mk("reuse_unread_head", "HEAD", "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n"),
	}
}

func versionCases() []Case {
	twoConns := func(name, first string) Case {
		return one(name, general(),
			[]Server{srv("A", conn(reqOp(), write(first), reqOp(), write(ok("2"))), conn(reqOp(), write(ok("2"))))},
			get("http://{{A}}/1"), get("http://{{A}}/2"))
	}
	return []Case{
		twoConns("resp_http00_close", "HTTP/0.0 200 OK\r\nContent-Length: 2\r\n\r\nok"),
		twoConns("resp_http00_chunked", "HTTP/0.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"+chunked("ab")),
		twoConns("resp_http09_close", "HTTP/0.9 200 OK\r\nContent-Length: 2\r\n\r\nok"),
		twoConns("resp_http20_keepalive", "HTTP/2.0 200 OK\r\nContent-Length: 2\r\n\r\nok"),
		twoConns("resp_connection_close_mixed_case", "HTTP/1.1 200 OK\r\nConnection: Keep-Alive, CLOSE\r\nContent-Length: 1\r\n\r\nx"),
		twoConns("resp_connection_split_values", "HTTP/1.1 200 OK\r\nConnection: keep-alive\r\nConnection: close\r\nContent-Length: 1\r\n\r\nx"),
		twoConns("resp_http10_keepalive_mixed_case", "HTTP/1.0 200 OK\r\nConnection: Keep-Alive\r\nContent-Length: 1\r\n\r\nx"),
	}
}

func methodCases() []Case {
	override := func(name, method string) Case {
		st := get("http://{{A}}/m")
		st.Req.MethodOverride = method
		return one(name, general(), nil, st)
	}
	return []Case{
		override("err_method_non_ascii", "ÜBER"),
		override("err_method_control", "GE\tT"),
		override("err_method_space", "GE T"),
	}
}
