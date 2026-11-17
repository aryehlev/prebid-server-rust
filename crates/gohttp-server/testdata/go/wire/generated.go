package main

import (
	"fmt"
	"strings"
	"unicode"
)

// The groups in this file are generated: each combines lists of inputs into many requests,
// where the groups of cases.go list their requests one by one. Their case names start with
// "gen ".

// genMethods are the methods the generated path cases rotate through.
var genMethods = []string{"GET", "POST", "OPTIONS", "HEAD", "PUT", "PATCH", "DELETE"}

// genQueries are the query strings the generated path cases rotate through.
var genQueries = []string{"", "?q=1", "?", "?a=%20b&c=%2F"}

// routeBases are paths that reach the main server's routes, plus a few that reach none.
var routeBases = []string{
	"/openrtb2/auction", "/openrtb2/video", "/openrtb2/amp", "/info/bidders",
	"/info/bidders/appnexus", "/bidders/params", "/cookie_sync", "/status", "/version",
	"/static/a.txt", "/static/sub/", "/vtrack", "/event", "/setuid", "/getuids", "/optout",
	"/t/req/a/b", "/t/form", "/t/redirect/302", "/nope/x", "/openrtb2/",
}

// adminBases are paths that reach the admin server's patterns, plus one that reaches none.
var adminBases = []string{
	"/version", "/debug/pprof/", "/debug/pprof/cmdline", "/currency/rates",
	"/debug/pprof/heap", "/nope",
}

// pathVariants lists p and its near misses: a toggled trailing slash, case changes, doubled
// slashes, dot segments, escaped bytes and slashes, and truncations.
func pathVariants(p string) []string {
	seen := map[string]bool{}
	var out []string
	add := func(v string) {
		if !seen[v] {
			seen[v] = true
			out = append(out, v)
		}
	}
	add(p)
	if len(p) > 1 && strings.HasSuffix(p, "/") {
		add(strings.TrimSuffix(p, "/"))
	} else {
		add(p + "/")
	}
	add(strings.ToUpper(p))
	add(strings.ToUpper(p) + "/")
	add(titleSegments(p))
	var letters, slashes []int
	for i := 0; i < len(p); i++ {
		switch {
		case p[i] == '/':
			slashes = append(slashes, i)
		case unicode.IsLetter(rune(p[i])):
			letters = append(letters, i)
		}
	}
	if len(letters) > 0 {
		for _, i := range []int{letters[0], letters[len(letters)/2], letters[len(letters)-1]} {
			add(p[:i] + strings.ToUpper(p[i:i+1]) + p[i+1:])
		}
	}
	for _, i := range slashes {
		add(p[:i] + "/" + p[i:])
		add(p[:i] + "/." + p[i:])
		add(p[:i] + "/x/.." + p[i:])
		add(p[:i] + "/%2e" + p[i:])
		add(p[:i] + "/.." + p[i:])
		if i > 0 {
			add(p[:i] + "%2F" + p[i+1:])
		}
	}
	add(p + "/.")
	add(p + "/..")
	add(p + "/./")
	add(p + "%2F")
	add(p + "%20")
	add(p + "x")
	if i := strings.LastIndex(p, "/"); i+1 < len(p) {
		add(p[:i+1] + fmt.Sprintf("%%%02X", p[i+1]) + p[i+2:])
		add(p[:i+1] + fmt.Sprintf("%%%02x", p[i+1]) + p[i+2:])
	}
	if len(p) > 1 {
		add(p[:len(p)-1])
	}
	return out
}

// titleSegments upper-cases the first letter of every segment of p.
func titleSegments(p string) string {
	b := []byte(p)
	for i := range b {
		if (i == 0 || b[i-1] == '/') && b[i] >= 'a' && b[i] <= 'z' {
			b[i] -= 'a' - 'A'
		}
	}
	return string(b)
}

// genPaths requests the variants of every base path. The first five variants (the path, its
// toggled trailing slash and its case changes) are requested with GET, POST and HEAD, which
// the router redirects with different codes; every other variant with one method and one
// query string, both rotating. A variant two bases share is requested once.
func genPaths(c *corpus, prefix, server string, bases []string, methods []string) {
	k := 0
	addOnce := func(m, target string) {
		if name := prefix + " " + m + " " + target; !c.names[name] {
			c.add(name, server, 0, send(req(m, target)))
		}
	}
	for _, base := range bases {
		for j, v := range pathVariants(base) {
			if j < 5 {
				for i, m := range []string{"GET", "POST", "HEAD"} {
					addOnce(m, v+genQueries[(j+i)%len(genQueries)])
				}
				continue
			}
			m := methods[k%len(methods)]
			q := genQueries[(k/len(methods))%len(genQueries)]
			k++
			addOnce(m, v+q)
		}
	}
}

// genRoutes covers the router's path cleaning, its trailing-slash and fixed-path redirects
// (codes and Location query strings), its automatic OPTIONS replies and its 405 Allow lists.
func genRoutes(c *corpus) {
	genPaths(c, "gen route", "main", routeBases, genMethods)
	for _, p := range []string{"/t/req/x", "/optout", "/t/form", "/t/redirect/302", "/status", "/nope", "*"} {
		for _, m := range []string{"TRACE", "PROPFIND", "OPTIONS", "CONNECT", "M-SEARCH", "head"} {
			c.one("gen allow "+m+" "+p, req(m, p))
		}
	}
}

// genAdmin covers the admin ServeMux's path cleaning and redirects.
func genAdmin(c *corpus) {
	genPaths(c, "gen admin", "admin", adminBases, []string{"GET", "POST", "HEAD", "OPTIONS", "PUT", "CONNECT"})
}

// genPlans lists the plans of /t/plan (see runPlan): writes around the 2048-byte buffer and
// the 4096-byte connection buffer, flushes between them, declared lengths, status codes,
// sniffed bodies and Connection headers.
func genPlans() []string {
	var plans []string
	for _, n := range []int{0, 1, 511, 512, 513, 2047, 2048, 2049, 4095, 4096, 4097, 8192} {
		plans = append(plans, fmt.Sprintf("w%d", n), fmt.Sprintf("w%d.f", n), fmt.Sprintf("f.w%d", n))
	}
	for _, a := range []int{1, 1024, 2047, 2048, 2049} {
		for _, b := range []int{0, 1, 2047, 2048} {
			plans = append(plans, fmt.Sprintf("w%d.w%d", a, b), fmt.Sprintf("w%d.f.w%d", a, b))
		}
	}
	for _, ln := range [][2]int{{2048, 2048}, {2048, 2047}, {2048, 2049}, {2049, 2049}, {4096, 4096}, {0, 0}, {0, 1}, {10, 5}, {5, 10}} {
		plans = append(plans,
			fmt.Sprintf("l%d.w%d", ln[0], ln[1]),
			fmt.Sprintf("l%d.x%d", ln[0], ln[1]),
			fmt.Sprintf("l%d.f.w%d", ln[0], ln[1]),
			fmt.Sprintf("l%d.w%d.x1", ln[0], ln[1]))
	}
	for _, b := range []string{"json", "html", "htmlws", "htmlopen", "bin", "xml", "text", "pdf", "bom", "gif", "lt", "ws", "empty"} {
		plans = append(plans, "b"+b, "f.b"+b, "b"+b+".f", "tnil.b"+b, "tempty.b"+b, "s404.b"+b)
	}
	plans = append(plans,
		"bht.bml", "bht.f.bml", "bempty.bhtml", "bempty.f.bhtml", "w600.bhtml", "tjson.w3000",
		"s200", "s204", "s304", "s404", "s500.f", "s204.x5", "s304.x5", "s404.w10", "w10.s404",
		"s201.s202.w1", "f.s404.w1", "s404.f.w1", "s103.w5", "s100.s200.w5", "s103.s404.w5",
		"x0", "x5", "x3000", "l3.x2.x1.x1", "l3.w3.f.x1", "cclose.w10", "cclose.w3000",
		"ckeep-alive.w10", "cUpgrade.w10", "s204.l5", "s304.l5.w5", "l-1.w5", "labc.w5",
	)
	return plans
}

// genPlanCases runs every plan on the main server (HTTP/1.1, HTTP/1.0 with and without
// keep-alive, and pipelined before another request) and on the admin server, where HEAD
// reaches the handler (GET, HEAD, and HEAD pipelined).
func genPlanCases(c *corpus) {
	for _, p := range genPlans() {
		target := "/t/plan/" + p
		c.one("gen plan get11 "+p, req("GET", target))
		c.one("gen plan get10 "+p, "GET "+target+" HTTP/1.0\r\n\r\n")
		c.one("gen plan get10ka "+p, raw("GET "+target+" HTTP/1.0", []string{"Connection: keep-alive"}, ""))
		c.one("gen plan pipeline "+p, req("GET", target)+req("GET", "/status"))
		c.add("gen plan admin get11 "+p, "admin", 0, send(req("GET", target)))
		c.add("gen plan admin head11 "+p, "admin", 0, send(req("HEAD", target)))
		c.add("gen plan admin head pipeline "+p, "admin", 0, send(req("HEAD", target)+req("GET", "/version")))
	}
}

// genBytes puts every ASCII byte into a header value, a header name, the request target and
// the method, and a few non-ASCII bytes into the name and the method (Go rejects those, so no
// non-UTF-8 text reaches a handler).
func genBytes(c *corpus) {
	for b := 0; b < 0x80; b++ {
		ch := string(rune(b))
		x := fmt.Sprintf("%02x", b)
		if b != '\r' && b != '\n' {
			c.one("gen byte value mid "+x, req("GET", "/t/req/x", "X-A: a"+ch+"b"))
			c.one("gen byte value edge "+x, req("GET", "/t/req/x", "X-A: "+ch+"a"+ch))
		}
		if b != ':' && b != '\r' && b != '\n' {
			c.one("gen byte name "+x, req("GET", "/t/req/x", "X-A"+ch+"b: v"))
		}
		if b != ' ' && b != '\r' && b != '\n' {
			c.one("gen byte target "+x, req("GET", "/t/req/a"+ch+"b"))
			c.one("gen byte method "+x, req("G"+ch+"T", "/t/req/x"))
		}
	}
	for _, b := range []byte{0x80, 0xc3, 0xff} {
		x := fmt.Sprintf("%02x", b)
		c.one("gen byte name "+x, req("GET", "/t/req/x", "X-A"+string([]byte{b})+"b: v"))
		c.one("gen byte method "+x, req("G"+string([]byte{b})+"T", "/t/req/x"))
	}
}

// genFolds covers obs-fold continuation lines: every combination of the folded header, the
// fold's leading whitespace and its content, plus folds of the headers the server reads.
func genFolds(c *corpus) {
	for _, h := range []string{"X-A: a", "X-A:", "X-A: a "} {
		for _, lead := range []string{" ", "\t", "  ", " \t "} {
			for _, rest := range []string{"b", "", "b ", ":c", "X-B: c"} {
				name := fmt.Sprintf("gen fold %q %q %q", h, lead, rest)
				c.one(name, raw("GET /t/req/x HTTP/1.1", []string{"Host: x", h, lead + rest, "X-Z: z"}, ""))
			}
		}
	}
	specials := []struct{ name, request string }{
		{"first line tab", "GET /t/req/x HTTP/1.1\r\n\tHost: x\r\n\r\n"},
		{"host", "GET /t/req/x HTTP/1.1\r\nHost: a\r\n b\r\n\r\n"},
		{"host empty", "GET /t/req/x HTTP/1.1\r\nHost:\r\n x\r\n\r\n"},
		{"content length", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length:\r\n 3\r\n\r\nabc"},
		{"content length split", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\n 0\r\n\r\n0123456789"},
		{"transfer encoding", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding:\r\n chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n"},
		{"connection close", "GET /status HTTP/1.1\r\nHost: x\r\nConnection: keep-alive,\r\n close\r\n\r\n" + req("GET", "/status")},
		{"expect", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect:\r\n 100-continue\r\nContent-Length: 3\r\n\r\nabc"},
		{"before end", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a\r\n \r\n\r\n"},
		{"lf only", "GET /t/req/x HTTP/1.1\nHost: x\nX-A: a\n b\n\n"},
		{"many", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a" + strings.Repeat("\r\n b", 50) + "\r\n\r\n"},
	}
	for _, s := range specials {
		c.one("gen fold "+s.name, s.request)
	}
}

// genRequestLines combines request-line separators, targets and protocol versions.
func genRequestLines(c *corpus) {
	protos := []string{
		"HTTP/1.1", "HTTP/1.0", "HTTP/1.2", "HTTP/1.9", "HTTP/0.9", "HTTP/2.0", "HTTP/9.9",
		"HTTP/1.a", "HTTP/a.1", "HTTP/1,1", "HTTP/11", "HTTP/1.1.1", "HTTP/ 1.1", "http/1.1",
		"Http/1.1", "HTTP/1.01", "HTTP/01.1", "HTTP/+.1", "HTTP/1.+", "HTTP/-.1", "HTTPS/1.1",
		"HTTP1.1", "HTTP/", "HTTP/1.1\t", "HTTP/1.1\x00",
	}
	for _, p := range protos {
		c.one(fmt.Sprintf("gen line proto %q host", p), "GET /t/req/x "+p+"\r\nHost: x\r\n\r\n")
		c.one(fmt.Sprintf("gen line proto %q", p), "GET /t/req/x "+p+"\r\n\r\n")
	}
	lines := []string{
		"GET /t/req/x  HTTP/1.1", "GET  /t/req/x  HTTP/1.1", " GET /t/req/x HTTP/1.1",
		"GET /t/req/x HTTP/1.1 ", "GET\t/t/req/x\tHTTP/1.1", "GET /t/req/x\tHTTP/1.1",
		"GET /t/req/x", "GET", "GET /t/req/x HTTP/1.1 extra", "/t/req/x HTTP/1.1",
		"GET /t/req/x HTTP/1.1\x0b", "GET\x0b/t/req/x HTTP/1.1",
	}
	for _, l := range lines {
		c.one(fmt.Sprintf("gen line %q", l), l+"\r\nHost: x\r\n\r\n")
	}
	targets := []string{
		"*", "t/req/x", "//t/req/x", "http://h/t/req/x", "http://h", "http:/t/req/x",
		"/t/req/x?", "/t/req/x??", "/t/req/x?a#b", "%2Ft%2Freq%2Fx", "/t/req/%", "/t/req/%2",
		"/t/req/%zz", "/t/req/%2f", "/t/req/%00", "/t/req/a;b", "/t/req/a,b", "/t/req/a@b",
		"/t/req/[x]", "/t/req/a\\b", "HTTP://h/t/req/x", "//h/t/req/x", "/t/req/x#", "?x",
		"/t/req/%7e%7E", "/t/req/%41%2f%42", "/t/req/+", "/t/req/%2B", "https://u@h:1/t/req/x",
		"http://[::1]/t/req/x", "http://h:/t/req/x", "http://h:99999/t/req/x", "/t/req/x?a=%zz",
	}
	for _, t := range targets {
		for _, m := range []string{"GET", "OPTIONS"} {
			c.one(fmt.Sprintf("gen target %s %q", m, t), m+" "+t+" HTTP/1.1\r\nHost: x\r\n\r\n")
		}
	}
}

// genExpect combines Expect values with body framings, handlers that read the body or not,
// and protocol versions.
func genExpect(c *corpus) {
	values := []string{
		"Expect: 100-continue", "Expect: 100-CONTINUE", "Expect:  100-continue", "Expect: 100-continue ",
		"Expect: 100-continue,foo", "Expect: foo, 100-continue", "Expect: 100-continuex",
		"Expect: 100", "Expect: continue", "Expect: \t100-continue",
		"Expect: foo\r\nExpect: 100-continue", "Expect: 100-continue\r\nExpect: foo",
	}
	bodies := []struct{ name, headers, body string }{
		{"cl", "Content-Length: 5", "hello"},
		{"chunked", "Transfer-Encoding: chunked", "5\r\nhello\r\n0\r\n\r\n"},
		{"cl0", "Content-Length: 0", ""},
	}
	for _, v := range values {
		for _, b := range bodies {
			for _, target := range []string{"/openrtb2/auction", "/t/body/none"} {
				name := fmt.Sprintf("gen expect %q %s %s", v, b.name, target)
				c.one(name, raw("POST "+target+" HTTP/1.1", []string{"Host: x", v, b.headers}, b.body)+req("GET", "/status"))
			}
		}
		c.one(fmt.Sprintf("gen expect %q http10", v), raw("POST /openrtb2/auction HTTP/1.0", []string{v, "Content-Length: 5"}, "hello"))
		c.add(fmt.Sprintf("gen expect %q paused", v), "main", 0,
			send(raw("POST /openrtb2/auction HTTP/1.1", []string{"Host: x", v, "Content-Length: 5"}, "")), pause(100), send("hello"))
	}
}

// genPipelines puts bytes between two pipelined requests: Go skips up to four CR or LF bytes
// only after a POST.
func genPipelines(c *corpus) {
	firsts := []struct{ name, request string }{
		{"get", req("GET", "/status")},
		{"post", post("/openrtb2/auction", "abc")},
		{"post chunked", chunked("/openrtb2/auction", "3\r\nabc\r\n0\r\n\r\n")},
		{"post unread", post("/t/body/none", "abc")},
		{"put", req("PUT", "/t/req/x", "Content-Length: 2") + "ab"},
	}
	for _, f := range firsts {
		for _, sep := range []string{"", "\r\n", "\n", "\r", "\r\n\r\n", "\n\n\n\n", "\r\n\r\n\r\n", " ", "\r\nx"} {
			c.one(fmt.Sprintf("gen pipeline %s %q", f.name, sep), f.request+sep+req("GET", "/version"))
		}
	}
}

// genBodies sends request bodies around the 256 KiB a handler that does not read its body
// may leave for the server to discard.
func genBodies(c *corpus) {
	for _, n := range []int{262143, 262144, 262145} {
		for _, target := range []string{"/t/body/none", "/openrtb2/auction"} {
			head := fmt.Sprintf("POST %s HTTP/1.1\r\nHost: x\r\nContent-Length: %d\r\n\r\n", target, n)
			c.add(fmt.Sprintf("gen body cl %d %s", n, target), "main", 0, send(head), sendN("b", n), send(req("GET", "/status")))
		}
		head := "POST /t/body/none HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n"
		c.add(fmt.Sprintf("gen body chunked %d", n), "main", 0,
			send(head+fmt.Sprintf("%x\r\n", n)), sendN("c", n), send("\r\n0\r\n\r\n"+req("GET", "/status")))
	}
}

// genCORS combines preflight request methods and request headers, and sends actual requests
// with every method and Origin form.
func genCORS(c *corpus) {
	acrm := []string{"GET", "HEAD", "POST", "PUT", "OPTIONS", "get", "Post", " POST", "POST ", "POST, GET", "PATCH"}
	for _, m := range acrm {
		for _, o := range []string{"Origin: https://a.example", "Origin: null"} {
			c.one(fmt.Sprintf("gen cors acrm %q %q", m, o), req("OPTIONS", "/openrtb2/auction", o, "Access-Control-Request-Method: "+m))
		}
	}
	acrh := []string{
		"Origin", "origin", "ORIGIN", "X-Requested-With", "x-requested-with", "Content-Type,Accept",
		"content-type, accept, origin, x-requested-with", " content-type", "content-type ",
		"content-type,,accept", ",content-type", "content-type,", "content-type , accept",
		"content-type\t,accept", "accept, accept", "authorization", "content-type, x-other",
		"x-other, content-type", "Content_Type", "content-type;", "*", "", " ", ",",
		"accept\r\nAccess-Control-Request-Headers: x-other", "x-other\r\nAccess-Control-Request-Headers: accept",
		"accept,origin,content-type,x-requested-with,accept", "cache-control", "content-type x",
	}
	for _, h := range acrh {
		c.one(fmt.Sprintf("gen cors acrh %q", h), req("OPTIONS", "/openrtb2/auction", "Origin: https://a.example", "Access-Control-Request-Method: POST", "Access-Control-Request-Headers: "+h))
	}
	origins := []struct{ name, header string }{
		{"none", ""}, {"one", "Origin: https://a.example"}, {"null", "Origin: null"},
		{"two", "Origin: https://a.example\r\nOrigin: https://b.example"}, {"empty", "Origin:"},
		{"upper", "Origin: HTTPS://A.EXAMPLE"}, {"empty then one", "Origin:\r\nOrigin: https://a.example"},
	}
	for _, o := range origins {
		for _, m := range []string{"GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "PURGE", "get", "HEAD"} {
			hs := []string{}
			if o.header != "" {
				hs = append(hs, o.header)
			}
			c.one(fmt.Sprintf("gen cors actual %s %s", o.name, m), req(m, "/t/req/x", hs...))
		}
		hs := []string{"Access-Control-Request-Method: POST"}
		if o.header != "" {
			hs = append(hs, o.header)
		}
		c.one("gen cors preflight origin "+o.name, req("OPTIONS", "/nope", hs...))
	}
}

// genHeaderLimit sends requests whose header, with its blank line, is one byte under, at or
// over the server's read limit for a header (MaxHeaderBytes plus 4096 bytes of bufio slack),
// as one long value, a long target or many lines, and a long value followed by a pipelined
// request.
func genHeaderLimit(c *corpus) {
	const limit = 1<<20 + 4096
	const line = "X-Line: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n"
	for _, delta := range []int{-1, 0, 1} {
		head := "GET /status HTTP/1.1\r\nHost: x\r\nX-Big: "
		pad := limit + delta - len(head) - len("\r\n\r\n")
		c.add(fmt.Sprintf("gen limit value %+d", delta), "main", 0, send(head), sendN("a", pad), send("\r\n\r\n"))
		c.add(fmt.Sprintf("gen limit value %+d pipelined", delta), "main", 0,
			send(head), sendN("a", pad), send("\r\n\r\n"+req("GET", "/status")))
		start, tail := "GET /status?", " HTTP/1.1\r\nHost: x\r\n\r\n"
		c.add(fmt.Sprintf("gen limit target %+d", delta), "main", 0,
			send(start), sendN("a", limit+delta-len(start)-len(tail)), send(tail))
		first := "GET /status HTTP/1.1\r\nHost: x\r\n"
		n := (limit - len(first) - len("\r\n")) / len(line)
		rest := limit + delta - len(first) - n*len(line) - len("\r\n")
		c.add(fmt.Sprintf("gen limit lines %+d", delta), "main", 0,
			send(first), sendN(line, n), send("X-Last: "+strings.Repeat("b", rest-len("X-Last: \r\n"))+"\r\n\r\n"))
	}
}

// genRanges requests a short and a long static file with Range headers built from lists of
// starts and ends, and with spacing, sign, overflow, list and multi-range variants.
func genRanges(c *corpus) {
	nums := []string{"", "0", "1", "5", "13", "14", "15", "2999", "3000"}
	var ranges []string
	for _, s := range nums {
		for _, e := range nums {
			ranges = append(ranges, "bytes="+s+"-"+e)
		}
	}
	ranges = append(ranges,
		"bytes= 0-4", "bytes=0 -4", "bytes=0- 4", "bytes=0-4 ", "bytes =0-4", "BYTES=0-4",
		"Bytes=0-4", "bytes=+0-4", "bytes=0-+4", "bytes=-+4", "bytes=--4", "bytes=0--4",
		"bytes=0-4-", "bytes=0x0-4", "bytes=00-04", "bytes=0-4,", "bytes=,0-4", "bytes=0-4,,5-6",
		"bytes=0-4, 5-6", "bytes=0-4 ,5-6", "bytes=0-4\t,5-6", "bytes=", "bytes=,",
		"bytes=0", "bytes=a-b", "bytes=1-a", "bytes=9223372036854775807-",
		"bytes=9223372036854775808-", "bytes=0-9223372036854775807",
		"bytes=0-9223372036854775808", "bytes=-9223372036854775807",
		"bytes=-9223372036854775808", "bytes=99999999999999999999-", "bytes=1_0-20",
		"bytes=0-0,0-0", "bytes=0-1,1-2,2-3,3-4,4-5", "bytes=0-13,0-0", "bytes=0-6,7-13",
		"bytes=0-6,7-14", "bytes=20-30,0-1", "bytes=20-30,40-50", "bytes=-0,0-1", "bytes=0-0,-0",
		"bytes=0-4;x", "bytes= 0-4", "bytes=-1,-1", "bytes=3000-3000,0-0", "bytes=0-4,5-",
		"bytes=0-0\r\nRange: bytes=5-6",
	)
	for _, r := range ranges {
		for _, f := range []string{"a.txt", "big.txt"} {
			c.one(fmt.Sprintf("gen range %s %q", f, r), req("GET", "/static/"+f, "Range: "+r))
		}
	}
}

// genConditionals requests a static file with If-Modified-Since, If-Unmodified-Since and
// If-Range dates in Go's three formats and near misses of them, with If-None-Match, If-Match
// and If-Range entity-tag lists, and with combinations of the conditions.
func genConditionals(c *corpus) {
	dates := []string{
		"Thu, 02 Jan 2020 03:04:05 GMT", "Thu, 02 Jan 2020 03:04:04 GMT",
		"Thu, 02 Jan 2020 03:04:06 GMT", "Wed, 01 Jan 2020 00:00:00 GMT",
		"Fri, 03 Jan 2020 00:00:00 GMT", "Mon, 02 Jan 2020 03:04:05 GMT",
		"thu, 02 jan 2020 03:04:05 GMT", "Thu, 02 Jan 2020 03:04:05 gmt",
		"Thu, 02 Jan 2020 03:04:05 UTC", "Thu, 02 Jan 2020 03:04:05 +0000",
		"Thu, 2 Jan 2020 03:04:05 GMT", "Thu, 02 Jan 20 03:04:05 GMT",
		"Thu,  02 Jan 2020 03:04:05 GMT", "Thu, 02 Jan 2020 03:04:05 GMT ",
		"Thu, 02 Jan 2020 3:04:05 GMT", "Thu, 02 Jan 2020 03:04:05", "Thu 02 Jan 2020 03:04:05 GMT",
		"Thursday, 02-Jan-20 03:04:05 GMT", "Thursday, 02-Jan-20 03:04:05 UTC",
		"Thursday, 02-Jan-2020 03:04:05 GMT", "Thu, 02-Jan-20 03:04:05 GMT",
		"Thursday, 02-Jan-20 03:04:05 +0100", "Thursday, 02-Jan-70 03:04:05 GMT",
		"Thursday, 02-Jan-68 03:04:05 GMT", "Thu Jan  2 03:04:05 2020", "Thu Jan 02 03:04:05 2020",
		"Thu Jan 2 03:04:05 2020", "Thu Jan  2 03:04:06 2020", "Thu, 02 Jan 2020 03:04:05.5 GMT",
		"Thu, 32 Jan 2020 03:04:05 GMT", "Thu, 02 Jan 2020 24:00:00 GMT",
		"Thu, 02 Foo 2020 03:04:05 GMT", "Fri, 29 Feb 2019 00:00:00 GMT",
		"Sat, 29 Feb 2020 00:00:00 GMT", "Thu, 02 Jan 1969 03:04:05 GMT",
		"Sat, 01 Jan 0000 00:00:00 GMT", "Fri, 31 Dec 9999 23:59:59 GMT", "garbage", "0",
	}
	for _, d := range dates {
		c.one(fmt.Sprintf("gen cond ims %q", d), req("GET", "/static/a.txt", "If-Modified-Since: "+d))
		c.one(fmt.Sprintf("gen cond ius %q", d), req("GET", "/static/a.txt", "If-Unmodified-Since: "+d))
		c.one(fmt.Sprintf("gen cond if-range %q", d), req("GET", "/static/a.txt", "Range: bytes=0-4", "If-Range: "+d))
	}
	etags := []string{
		`"x"`, `W/"x"`, `*`, ` *`, `"x", *`, `"a", "b"`, `"a","b"`, `,"a"`, `"a",`, `W/"a", W/"b"`,
		`x`, `"x`, `"x"y`, `""`, `W/""`, `w/"x"`, `"é"`, `"a b"`, `, `, `"x" "y"`, `"x",*`,
		`**`, `*"x"`,
	}
	for _, e := range etags {
		c.one(fmt.Sprintf("gen cond inm %q", e), req("GET", "/static/a.txt", "If-None-Match: "+e))
		c.one(fmt.Sprintf("gen cond im %q", e), req("GET", "/static/a.txt", "If-Match: "+e))
		c.one(fmt.Sprintf("gen cond if-range etag %q", e), req("GET", "/static/a.txt", "Range: bytes=0-4", "If-Range: "+e))
	}
	combos := [][]string{
		{"If-Match: *", "If-Unmodified-Since: Wed, 01 Jan 2020 00:00:00 GMT"},
		{`If-Match: "x"`, "If-Unmodified-Since: Fri, 03 Jan 2020 00:00:00 GMT"},
		{`If-None-Match: "x"`, "If-Modified-Since: Fri, 03 Jan 2020 00:00:00 GMT"},
		{"If-None-Match: *", "If-Modified-Since: Wed, 01 Jan 2020 00:00:00 GMT"},
		{"If-None-Match: *", "Range: bytes=0-4"},
		{"If-Modified-Since: Fri, 03 Jan 2020 00:00:00 GMT", "Range: bytes=0-4"},
		{"If-Modified-Since: Wed, 01 Jan 2020 00:00:00 GMT", "Range: bytes=0-4"},
		{"If-Unmodified-Since: Wed, 01 Jan 2020 00:00:00 GMT", "Range: bytes=0-4"},
		{"If-Match: *", "If-None-Match: *"},
		{"If-Range: Thu, 02 Jan 2020 03:04:05 GMT"},
		{"Range: bytes=0-4", "If-Range: Thu, 02 Jan 2020 03:04:05 GMT", "If-Range: \"x\""},
		{"If-Modified-Since: garbage", "If-Modified-Since: Fri, 03 Jan 2020 00:00:00 GMT"},
	}
	for i, hs := range combos {
		c.one(fmt.Sprintf("gen cond combo %d", i), req("GET", "/static/a.txt", hs...))
		c.one(fmt.Sprintf("gen cond combo %d head", i), req("HEAD", "/static/a.txt", hs...))
	}
}

// chunkedBody is a chunked request body in wire form. A body that is alone ends where Go stops
// reading it to reject it: nothing may follow it, as bytes the server leaves unread when it
// closes the connection make the close a reset or not by timing.
type chunkedBody struct {
	name, wire string
	alone      bool
}

// chunkedBodies combines chunk-size lines (every ASCII byte before, after and in the extension
// of a size, sizes whose overhead check wraps in Go's int64 arithmetic), chunk-data
// terminators, short and long chunks, last chunks and trailer sections, and lines and trailers
// at the 4096-byte limit of the buffer.
func chunkedBodies() []chunkedBody {
	var out []chunkedBody
	add := func(name, wire string) { out = append(out, chunkedBody{name: name, wire: wire}) }
	alone := func(name, wire string) { out = append(out, chunkedBody{name: name, wire: wire, alone: true}) }
	end := "\r\nhello\r\n0\r\n\r\n"
	sizes := []string{
		"5", "05", "0000000000000005", "00000000000000005", "0000000000000000z", "5 ", "5\t",
		"5 \t", " 5", "\t5", "5;", "5;a", "5;a=b", `5;a="b c"`, `5;a="b;c"`, "5 ;a", "5; a",
		"5;a ", "5;;", "5\v", "5\f", "+5", "-5", "0x5", "5.0", "5,", "5 5", "5\x00", "5\x80",
		"\xef\xbc\x95", "",
	}
	for _, s := range sizes {
		add(fmt.Sprintf("size %q", s), s+end)
	}
	// The overhead check computes 16+2*int64(n): sizes of 2^62 and up wrap.
	huge := []string{
		"1000000000000000", "3fffffffffffffff", "4000000000000000", "4000000000000001",
		"4000000000000002", "5000000000000000", "6000000000000000", "7fffffffffffdfff",
		"7fffffffffffe000", "7fffffffffffffff", "8000000000000000", "a000000000000000",
		"c000000000000000", "c000000000000001", "c000000000000002", "e000000000000000",
		"ffffffffffffdfff", "ffffffffffffe000", "ffffffffffffefff", "ffffffffffffffff",
		"FFFFFFFFFFFFFFFF", "6000000000000000;x",
	}
	for _, s := range huge {
		add(fmt.Sprintf("huge %q", s), s+end)
	}
	for b := 0; b < 0x80; b++ {
		ch := string(rune(b))
		add(fmt.Sprintf("byte %02x before size", b), ch+"5"+end)
		add(fmt.Sprintf("byte %02x after size", b), "5"+ch+end)
		add(fmt.Sprintf("byte %02x in ext", b), "5;"+ch+end)
		add(fmt.Sprintf("byte %02x in last", b), "5\r\nhello\r\n0"+ch+"\r\n\r\n")
	}
	terms := []string{
		"", "\r", "\n", "\n\r", "\r\r\n", " \r\n", "\r\n\r\n", "x\r\n", "\x00\r\n", "\r\x00",
		"\rx", "xx", "\t\n", "\r\n\n",
	}
	for _, t := range terms {
		add(fmt.Sprintf("term %q", t), "5\r\nhello"+t+"0\r\n\r\n")
	}
	for _, n := range []int{0, 1, 3, 4, 6, 7, 12} {
		add(fmt.Sprintf("declared %d for 5", n), fmt.Sprintf("%x\r\nhello\r\n0\r\n\r\n", n))
	}
	var many strings.Builder
	for i := 1; i <= 16; i++ {
		fmt.Fprintf(&many, "%x\r\n%s\r\n", i, strings.Repeat(string(rune('a'+i%26)), i))
	}
	add("sizes 1 to 16", many.String()+"0\r\n\r\n")
	add("bytes one by one", strings.Repeat("1\r\nz\r\n", 40)+"0\r\n\r\n")
	add("hex letters", "a\r\n0123456789\r\nA\r\n0123456789\r\n1f\r\n"+strings.Repeat("q", 31)+"\r\n0\r\n\r\n")
	add("empty first", "0\r\n\r\n")
	add("empty chunk inside", "5\r\nhello\r\n0\r\n\r\n5\r\nworld\r\n0\r\n\r\n")
	lasts := []string{
		"0", "00", "0000000000000000", "00000000000000000", "0;a=b", "0;", "0 ", "0\t", " 0",
		"0 ;a", `0;a="x"`,
	}
	for _, l := range lasts {
		add(fmt.Sprintf("last %q", l), "5\r\nhello\r\n"+l+"\r\n\r\n")
	}
	trailers := []string{
		"X-T: v\r\n", "X-T: v\r\nX-U: w\r\n", "x-t: v\r\n", "X-T:v\r\n", "X-T: \r\n", "X-T:\r\n",
		"X-T: v \r\n", "X-T: a\r\nX-T: b\r\n", "X-T: v\r\n folded\r\n", "X-T: v\r\n\tfolded\r\n",
		" X-T: v\r\n", "X-T v\r\n", "X T: v\r\n", "X-T : v\r\n", ": v\r\n", "X-T: v\x00\r\n",
		"X-T: v\x7f\r\n", "X-T: v\x80\r\n", "X-\xffT: v\r\n", "X-T: v\tw\r\n", "Host: h\r\n",
		"Content-Length: 5\r\n", "Transfer-Encoding: chunked\r\n", "Trailer: X-T\r\n",
		"Content-Type: text/plain\r\n", "Expect: 100-continue\r\n", "Connection: close\r\n",
		strings.Repeat("X-T: v\r\n", 100), "X-T: " + strings.Repeat("a", 4000) + "\r\n",
	}
	for _, t := range trailers {
		add(fmt.Sprintf("trailer %q", t), "5\r\nhello\r\n0\r\n"+t+"\r\n")
	}
	// A trailer section of 4096 bytes with its blank line, and 4096 bytes without one: Go
	// looks for the blank line within the buffer only.
	add("trailer 4096", "5\r\nhello\r\n0\r\nX-T: "+strings.Repeat("a", 4087)+"\r\n\r\n")
	alone("trailer over 4096", "5\r\nhello\r\n0\r\nX-T: "+strings.Repeat("a", 4088)+"\r\n\r")
	ends := []string{
		"X-T: v\n\n", "X-T: v\r\n\n", "X-T: v\n\r\n", "\n", "X-T: v\r\n", "X-T: v\r\n\r",
		"X-T: v\rX\r\n\r\n",
	}
	for _, e := range ends {
		add(fmt.Sprintf("trailer end %q", e), "5\r\nhello\r\n0\r\n"+e)
	}
	// A chunk line of 4096 bytes with its CRLF, and 4096 bytes without a line end.
	add("last line 4096", "0;"+strings.Repeat("e", 4092)+"\r\n\r\n")
	alone("last line over 4096", "0;"+strings.Repeat("e", 4094))
	add("line 4096", "1;"+strings.Repeat("e", 4092)+"\r\nX\r\n0\r\n\r\n")
	alone("line over 4096", "1;"+strings.Repeat("e", 4094))
	return out
}

// genChunked sends every chunked body, followed by a pipelined request unless it is alone, to
// a route that reads the whole body and reports it (or its error) and to one that does not
// read it, so the server discards it; and the bodies with more than one chunk or a trailer to
// routes that read some of the body, through a limit or a MaxBytesReader.
func genChunked(c *corpus) {
	next := req("GET", "/status")
	for _, b := range chunkedBodies() {
		after := next
		if b.alone {
			after = ""
		}
		c.one("gen chunked auction "+b.name, chunked("/openrtb2/auction", b.wire)+after)
		c.one("gen chunked none "+b.name, chunked("/t/body/none", b.wire)+after)
		if strings.HasPrefix(b.name, "trailer") || strings.HasPrefix(b.name, "last") ||
			strings.HasPrefix(b.name, "sizes") || strings.HasPrefix(b.name, "bytes") ||
			strings.HasPrefix(b.name, "hex") || strings.HasPrefix(b.name, "term") {
			for _, target := range []string{"/openrtb2/video", "/t/body/some", "/t/body/max"} {
				c.one(fmt.Sprintf("gen chunked %s %s", target, b.name), chunked(target, b.wire)+after)
			}
		}
	}
	for _, t := range []string{"X-T: v\r\n", "X-U: v\r\n", "\r\n" + "X-T: v\r\n", ""} {
		c.one(fmt.Sprintf("gen chunked declared trailer %q", t),
			chunked("/openrtb2/auction", "5\r\nhello\r\n0\r\n"+t+"\r\n", "Trailer: X-T")+next)
	}
	// k one-byte chunks with an l-byte extension, which reach the overhead limit (16 KiB beyond
	// 16 bytes per chunk and twice the data) at the 5th of 4000 or the 43rd of 400 (a body Go
	// rejects ends with the line it rejects). The last chunk is not checked: io.EOF replaces
	// the error, even with a 200-byte extension after 42 of 400.
	line := func(l int) string { return "1;" + strings.Repeat("e", l) + "\r\n" }
	exts := []struct {
		name string
		k, l int
		tail string
	}{
		{"4x4000", 4, 4000, "0\r\n\r\n" + next},
		{"5x4000", 4, 4000, line(4000)},
		{"42x400", 42, 400, "0\r\n\r\n" + next},
		{"43x400", 42, 400, line(400)},
		{"42x400 last 100", 42, 400, "0;" + strings.Repeat("e", 100) + "\r\n\r\n" + next},
		{"42x400 last 200", 42, 400, "0;" + strings.Repeat("e", 200) + "\r\n\r\n" + next},
	}
	for _, e := range exts {
		for _, target := range []string{"/openrtb2/auction", "/t/body/none"} {
			c.add(fmt.Sprintf("gen chunked ext %s %s", e.name, target), "main", 0,
				send(chunked(target, "")), sendN(line(e.l)+"X\r\n", e.k), send(e.tail))
		}
	}
	// The body arrives in two parts, split inside a size line, the data, its CRLF, the last
	// chunk or the trailer.
	wire := "5\r\nhello\r\n0\r\nX-T: v\r\n\r\n"
	for split := 1; split < len(wire); split++ {
		c.add(fmt.Sprintf("gen chunked split at %d", split), "main", 0,
			send(chunked("/openrtb2/auction", wire[:split])), pause(100), send(wire[split:]+next))
	}
}
