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
