package main

import (
	"fmt"
	"strings"
)

// step is one thing the client does: send data (repeat times), or pause.
type step struct {
	data   string
	repeat int
	pause  int
}

// wireCase is one connection: the steps the client takes, the server it talks to, and how
// many milliseconds of silence (after the last step) end the read.
type wireCase struct {
	name   string
	server string
	wait   int
	steps  []step
}

const defaultWait = 300

type corpus struct {
	cases []wireCase
	names map[string]bool
}

func (c *corpus) add(name, server string, wait int, steps ...step) {
	name = strings.NewReplacer("\r", `\r`, "\n", `\n`).Replace(name)
	if c.names[name] {
		panic("duplicate case " + name)
	}
	c.names[name] = true
	if wait == 0 {
		wait = defaultWait
	}
	c.cases = append(c.cases, wireCase{name: name, server: server, wait: wait, steps: steps})
}

// one adds a case that sends one request to the main server.
func (c *corpus) one(name, request string) {
	c.add(name, "main", 0, send(request))
}

func send(data string) step { return step{data: data, repeat: 1} }

func sendN(data string, n int) step { return step{data: data, repeat: n} }

func pause(ms int) step { return step{pause: ms} }

// raw joins a request line, header lines and a body.
func raw(line string, headers []string, body string) string {
	var b strings.Builder
	b.WriteString(line + "\r\n")
	for _, h := range headers {
		b.WriteString(h + "\r\n")
	}
	b.WriteString("\r\n")
	b.WriteString(body)
	return b.String()
}

// req is an HTTP/1.1 request with a Host header.
func req(method, target string, headers ...string) string {
	return raw(method+" "+target+" HTTP/1.1", append([]string{"Host: example.com"}, headers...), "")
}

// post is an HTTP/1.1 request with a Content-Length body.
func post(target, body string, headers ...string) string {
	hs := append([]string{"Host: example.com", fmt.Sprintf("Content-Length: %d", len(body))}, headers...)
	return raw("POST "+target+" HTTP/1.1", hs, body)
}

// chunked is an HTTP/1.1 POST with a chunked body given in wire form.
func chunked(target, wireBody string, headers ...string) string {
	hs := append([]string{"Host: example.com", "Transfer-Encoding: chunked"}, headers...)
	return raw("POST "+target+" HTTP/1.1", hs, wireBody)
}

// framings are the protocol framings each behaviour route is requested with.
var framings = []struct {
	name   string
	method string
	proto  string
	extra  []string
}{
	{"get11", "GET", "HTTP/1.1", []string{"Host: example.com"}},
	{"head11", "HEAD", "HTTP/1.1", []string{"Host: example.com"}},
	{"get10", "GET", "HTTP/1.0", nil},
	{"get10ka", "GET", "HTTP/1.0", []string{"Connection: keep-alive"}},
	{"get11close", "GET", "HTTP/1.1", []string{"Host: example.com", "Connection: close"}},
}

func (c *corpus) variants(prefix, target string) {
	for _, v := range framings {
		c.one(prefix+" "+v.name, raw(v.method+" "+target+" "+v.proto, v.extra, ""))
	}
}

func allCases() []wireCase {
	c := &corpus{names: map[string]bool{}}
	routeMatrix(c)
	staticMatrix(c)
	parseErrors(c)
	framing(c)
	headersAndStatus(c)
	helpers(c)
	bodies(c)
	connections(c)
	corsMatrix(c)
	admin(c)
	short(c)
	unix(c)
	plain(c)
	return c.cases
}

// routeMatrix requests every Prebid route (and near misses) with several methods.
func routeMatrix(c *corpus) {
	paths := []string{
		"/", "/status", "/status/", "/STATUS", "//status", "/status?x=1", "/status/?x=1",
		"/./status", "/a/../status", "/version", "/version/", "/Version",
		"/info/bidders", "/info/bidders/", "/info/bidders/appnexus", "/info/bidders/AppNexus",
		"/info/bidders/unknown", "/info/bidders/appnexus/", "/info/bidders/app%20nexus",
		"/info/bidders/a%2Fb", "/info/bidders/all", "/INFO/BIDDERS/appnexus", "/info//bidders",
		"/bidders/params", "/openrtb2/auction", "/openrtb2/auction/", "/openrtb2/Auction",
		"/openrtb2/amp?tag_id=1&debug=1", "/openrtb2/amp/", "/OPENRTB2/AMP?x=y",
		"/openrtb2/video", "/cookie_sync", "/setuid?bidder=a&uid=b", "/setuid", "/getuids",
		"/optout", "/event?t=win&b=1", "/vtrack", "/static/", "/static", "/static/a.txt",
		"/static/sub", "/static/sub/", "/static/nested", "/static/missing", "/nope", "/info",
		"/t/req/x?y=z", "/t/req/caf%C3%A9?q=%20", "/t/req/ü", "/t/req/a%2fb/../c",
		"/t/req/x#frag", "/t/req/", "/t/req", "/t/remote/", "http://other.example/status",
		"*",
	}
	for _, p := range paths {
		for _, m := range []string{"GET", "HEAD", "POST", "OPTIONS", "DELETE", "get", "PURGE"} {
			c.one("route "+m+" "+p, req(m, p))
		}
	}
}

// staticMatrix covers ServeFiles and ServeFile: listings, redirects, conditionals, ranges.
func staticMatrix(c *corpus) {
	for _, p := range []string{
		"/static/index.html", "/static/sub/index.html", "/static/big.txt", "/static/data.json",
		"/static/noext", "/static/spa%20ce.txt", "/static/spa ce.txt", "/static/../a.txt",
		"/static/%2e%2e/a.txt", "/static//a.txt", "/static/sub/b.css", "/static/nested/",
		"/static/nested/c.js", "/static/empty/", "/static/empty", "/static/a.txt/",
		"/static/sub/..", "/static/%2e%2e", "/static/a.txt?x=1", "/static/sub?x=1",
		"/static/nested/d.json", "/static/%00", "/static/a%5Cb",
	} {
		c.one("static GET "+p, req("GET", p))
		c.one("static HEAD "+p, req("HEAD", p))
	}
	conds := []struct{ name, header string }{
		{"ims equal", "If-Modified-Since: Thu, 02 Jan 2020 03:04:05 GMT"},
		{"ims older", "If-Modified-Since: Wed, 01 Jan 2020 00:00:00 GMT"},
		{"ims newer", "If-Modified-Since: Fri, 03 Jan 2020 00:00:00 GMT"},
		{"ims rfc850", "If-Modified-Since: Thursday, 02-Jan-20 03:04:05 GMT"},
		{"ims ansic", "If-Modified-Since: Thu Jan  2 03:04:05 2020"},
		{"ims garbage", "If-Modified-Since: garbage"},
		{"ius older", "If-Unmodified-Since: Wed, 01 Jan 2020 00:00:00 GMT"},
		{"ius newer", "If-Unmodified-Since: Fri, 03 Jan 2020 00:00:00 GMT"},
		{"inm star", "If-None-Match: *"},
		{"inm etag", `If-None-Match: "x"`},
		{"im etag", `If-Match: "x"`},
		{"im star", "If-Match: *"},
		{"range first", "Range: bytes=0-4"},
		{"range suffix", "Range: bytes=-5"},
		{"range open", "Range: bytes=10-"},
		{"range multi", "Range: bytes=0-0,2-3"},
		{"range overlap", "Range: bytes=0-5,3-8"},
		{"range beyond", "Range: bytes=9999-"},
		{"range bad", "Range: bytes=abc"},
		{"range reversed", "Range: bytes=5-1"},
		{"range unit", "Range: items=0-1"},
		{"range zero suffix", "Range: bytes=-0"},
		{"range huge end", "Range: bytes=2-99999"},
		{"range many", "Range: bytes=0-0,1-1,2-2,3-3"},
		{"if-range date", "Range: bytes=0-4\r\nIf-Range: Thu, 02 Jan 2020 03:04:05 GMT"},
		{"if-range old", "Range: bytes=0-4\r\nIf-Range: Wed, 01 Jan 2020 00:00:00 GMT"},
		{"if-range etag", "Range: bytes=0-4\r\nIf-Range: \"x\""},
	}
	for _, p := range []string{"/static/a.txt", "/static/big.txt", "/static/sub/", "/", "/static/noext"} {
		for _, cond := range conds {
			c.one("cond "+cond.name+" "+p, req("GET", p, cond.header))
		}
	}
	c.one("cond head range", req("HEAD", "/static/big.txt", "Range: bytes=0-4"))
	c.one("cond post ims", raw("POST /static/a.txt HTTP/1.1", []string{"Host: x", "If-Modified-Since: Thu, 02 Jan 2020 03:04:05 GMT"}, ""))
}

func parseErrors(c *corpus) {
	cases := []struct{ name, request string }{
		{"no host 1.1", "GET /status HTTP/1.1\r\n\r\n"},
		{"no host 1.0", "GET /status HTTP/1.0\r\n\r\n"},
		{"two hosts", "GET /status HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"},
		{"bad host", "GET /status HTTP/1.1\r\nHost: a b\r\n\r\n"},
		{"host port", "GET /t/req/x HTTP/1.1\r\nHost: example.com:8000\r\n\r\n"},
		{"host ipv6", "GET /t/req/x HTTP/1.1\r\nHost: [::1]:80\r\n\r\n"},
		{"host empty", "GET /t/req/x HTTP/1.1\r\nHost:\r\n\r\n"},
		{"host utf8", "GET /t/req/x HTTP/1.1\r\nHost: café.example\r\n\r\n"},
		{"request line short", "GET /status\r\n\r\n"},
		{"request line two spaces", "GET  /status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"request line trailing space", "GET /status HTTP/1.1 \r\nHost: x\r\n\r\n"},
		{"request line tab", "GET\t/status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"lowercase proto", "GET /status http/1.1\r\nHost: x\r\n\r\n"},
		{"proto 1.2", "GET /t/req/x HTTP/1.2\r\nHost: x\r\n\r\n"},
		{"proto 1.9 no host", "GET /t/req/x HTTP/1.9\r\n\r\n"},
		{"proto 2.0", "GET /status HTTP/2.0\r\nHost: x\r\n\r\n"},
		{"proto 0.9", "GET /status HTTP/0.9\r\nHost: x\r\n\r\n"},
		{"proto 1.10", "GET /status HTTP/1.10\r\nHost: x\r\n\r\n"},
		{"proto 10.0", "GET /status HTTP/10.0\r\nHost: x\r\n\r\n"},
		{"proto 3.0", "GET /status HTTP/3.0\r\nHost: x\r\n\r\n"},
		{"proto bare", "GET /status HTTP\r\nHost: x\r\n\r\n"},
		{"pri preface", "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"},
		{"pri with header", "PRI * HTTP/2.0\r\nHost: x\r\n\r\n"},
		{"bad method", "G(T /status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"empty method", " /status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"connect authority", "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"},
		{"connect path", "CONNECT /status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"bad escape", "GET /status%zz HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"relative uri", "GET status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"asterisk get", "GET * HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"options asterisk", "OPTIONS * HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"options asterisk body", "OPTIONS * HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello"},
		{"options asterisk 1.0", "OPTIONS * HTTP/1.0\r\n\r\n"},
		{"options asterisk origin", "OPTIONS * HTTP/1.1\r\nHost: x\r\nOrigin: https://a.example\r\nAccess-Control-Request-Method: POST\r\n\r\n"},
		{"absolute uri", "GET http://other.example/t/req/x?a=b HTTP/1.1\r\nHost: example.com\r\n\r\n"},
		{"absolute uri https port", "GET https://other.example:8443/t/req/y HTTP/1.1\r\nHost: example.com\r\n\r\n"},
		{"absolute uri no path", "GET http://other.example HTTP/1.1\r\nHost: example.com\r\n\r\n"},
		{"absolute uri userinfo", "GET http://u:p@other.example/t/req/z HTTP/1.1\r\nHost: example.com\r\n\r\n"},
		{"header no colon", "GET /status HTTP/1.1\r\nHost: x\r\nBadHeader\r\n\r\n"},
		{"header space before colon", "GET /status HTTP/1.1\r\nHost: x\r\nX-A : b\r\n\r\n"},
		{"header leading space", "GET /status HTTP/1.1\r\n Host: x\r\n\r\n"},
		{"header obs fold", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-Fold: a\r\n b\r\n\tc\r\n\r\n"},
		{"header bad name", "GET /status HTTP/1.1\r\nHost: x\r\nX(A): b\r\n\r\n"},
		{"header empty name", "GET /status HTTP/1.1\r\nHost: x\r\n: b\r\n\r\n"},
		{"header ctl value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a\x01b\r\n\r\n"},
		{"header del value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a\x7fb\r\n\r\n"},
		{"header tab value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a\tb\r\n\r\n"},
		{"header cr value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: a\rb\r\n\r\n"},
		{"header utf8 value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: café\r\n\r\n"},
		{"header latin1 value", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A: caf\xe9\r\n\r\n"},
		{"header canonical", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nx-lower-key: 1\r\nX-UPPER-KEY: 2\r\nx_under_score: 3\r\ncontent-type: text/plain\r\nX-A: 1\r\nx-a: 2\r\nuser-agent: ua\r\n\r\n"},
		{"header space name", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX A: 1\r\n\r\n"},
		{"header value spaces", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nX-A:    padded   \r\nX-B:\r\nX-C: \t\r\n\r\n"},
		{"header many", "GET /t/req/x HTTP/1.1\r\nHost: x\r\n" + strings.Repeat("X-Many: v\r\n", 200) + "\r\n"},
		{"bare lf", "GET /t/req/x HTTP/1.1\nHost: x\nX-A: b\n\n"},
		{"bare lf mixed", "GET /t/req/x HTTP/1.1\r\nHost: x\nX-A: b\r\n\n"},
		{"nul in uri", "GET /sta\x00tus HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"space in uri", "GET /a b HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"del in uri", "GET /t/req/a\x7fb HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"utf8 in uri", "GET /t/req/\xc3\xa9 HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"latin1 in uri", "GET /t/req/caf\xe9 HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"latin1 escaped in uri", "GET /t/req/caf%E9 HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"long uri", "GET /t/req/" + strings.Repeat("a", 3000) + " HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"leading crlf", "\r\nGET /status HTTP/1.1\r\nHost: x\r\n\r\n"},
		{"only crlf", "\r\n"},
		{"garbage", "hello\r\n\r\n"},
		{"te gzip", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip\r\n\r\n"},
		{"te chunked gzip", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, gzip\r\n\r\n"},
		{"te gzip chunked", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n"},
		{"te identity", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: identity\r\n\r\n"},
		{"te two headers", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"},
		{"te uppercase", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: Chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n"},
		{"te empty", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nTransfer-Encoding:\r\n\r\n"},
		{"te 1.0 ignored", "POST /openrtb2/auction HTTP/1.0\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\nabc"},
		{"te on get", "GET /t/req/x HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"},
		{"cl and te", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 50\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n"},
		{"cl two equal", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nhello"},
		{"cl two differ", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello!"},
		{"cl two spaced", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nContent-Length:  5 \r\n\r\nhello"},
		{"cl comma", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 5, 5\r\n\r\nhello"},
		{"cl plus", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: +5\r\n\r\nhello"},
		{"cl negative", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: -1\r\n\r\n"},
		{"cl huge", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999999999999999\r\n\r\n"},
		{"cl hex", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 0x5\r\n\r\nhello"},
		{"cl empty", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length:\r\n\r\n"},
		{"cl leading zeros", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 005\r\n\r\nhello"},
		{"cl on get", "GET /status HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello"},
		{"cl zero post", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"},
		{"cl 1.0 body", "POST /openrtb2/auction HTTP/1.0\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect other", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: foo\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect empty", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect:\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 read", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 mixed case", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-Continue\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 noread", "POST /t/body/none HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 zero length", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 0\r\n\r\n"},
		{"expect 100 http10", "POST /openrtb2/auction HTTP/1.0\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 chunked", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n"},
		{"expect 100 get", "GET /status HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\n\r\n"},
		{"expect 100 big body unread", "POST /t/body/big HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello"},
		{"expect 100 then pipeline", "POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhelloGET /status HTTP/1.1\r\nHost: x\r\n\r\n"},
	}
	for _, tc := range cases {
		c.one("parse "+tc.name, tc.request)
	}
	big := "GET /status HTTP/1.1\r\nHost: x\r\nX-Big: "
	c.add("parse header too large", "main", 0, send(big), sendN("a", 1100000), send("\r\n\r\n"))
	c.add("parse header just fits", "main", 0, send(big), sendN("a", 1040000), send("\r\n\r\n"))
	c.add("parse header many lines too large", "main", 0, send("GET /status HTTP/1.1\r\nHost: x\r\n"), sendN("X-Line: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n", 11000), send("\r\n"))
	c.add("parse request line too large", "main", 0, send("GET /"), sendN("a", 1100000), send(" HTTP/1.1\r\nHost: x\r\n\r\n"))
	c.add("expect 100 pause", "main", 0, send("POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n"), pause(150), send("hello"))
	c.add("split headers", "main", 0, send("GET /t/req/x HTTP/1.1\r\nHo"), pause(50), send("st: x\r\nX-A: 1\r\n"), pause(50), send("\r\n"))
}

// framing covers Content-Length versus chunked versus close-delimited replies.
func framing(c *corpus) {
	for _, n := range []string{"0", "1", "2047", "2048", "2049", "4096", "5000"} {
		c.variants("write "+n, "/t/write/"+n)
	}
	for _, w := range []string{"2/1000", "3/1000", "1/2048", "2/1024", "10/10", "0/5", "5/0", "1/4096"} {
		c.variants("writes "+w, "/t/writes/"+w)
	}
	for _, m := range []string{"early", "mid", "end", "header", "html", "unknown"} {
		c.variants("flush "+m, "/t/flush/"+m)
	}
	for _, dc := range []string{"5/5", "5/10", "100/10", "60/70", "0/0", "0/1", "3000/3000", "3000/2500", "2049/2049"} {
		c.variants("cl "+dc, "/t/cl/"+dc)
	}
}

func headersAndStatus(c *corpus) {
	codes := []string{
		"100", "101", "102", "103", "199", "200", "201", "204", "205", "206", "299", "301",
		"304", "307", "400", "404", "418", "499", "500", "503", "599", "600", "999", "99",
		"1000", "0",
	}
	for _, code := range codes {
		for _, body := range []string{"y", "n"} {
			c.one("status "+code+" "+body+" get11", req("GET", "/t/status/"+code+"/"+body))
			c.one("status "+code+" "+body+" head11", req("HEAD", "/t/status/"+code+"/"+body))
			c.one("status "+code+" "+body+" get10", "GET /t/status/"+code+"/"+body+" HTTP/1.0\r\n\r\n")
		}
	}
	c.variants("twice", "/t/twice")
	modes := []string{
		"noncanon", "newline", "badkey", "multi", "empty", "trailer", "trailerbig",
		"trailerprefix", "trailerhead", "date", "emptyctype", "ce", "space", "afterwrite",
		"connclose", "connka", "connupgrade", "teidentity", "techunked", "tegzip", "clte",
		"clbad", "clneg", "clzero", "location", "ctype", "sniffhtml", "sniffbin", "sniffpng",
		"sniffjson", "sniffutf16", "none",
	}
	for _, m := range modes {
		c.variants("header "+m, "/t/header/"+m)
	}
	for _, m := range []string{"before", "small", "big", "flush", "header"} {
		c.one("panic "+m, req("GET", "/t/panic/"+m))
		c.one("panic "+m+" pipelined", req("GET", "/t/panic/"+m)+req("GET", "/status"))
	}
}

// helpers covers http.Error, NotFound, Redirect and StripPrefix.
func helpers(c *corpus) {
	for _, m := range []string{"plain", "headers", "notfound", "multiline"} {
		c.variants("error "+m, "/t/error/"+m)
	}
	targets := []string{
		"/abs", "/abs?q=1", "rel", "../up", "./same", "", "http://example.org/x",
		"//proto-relative/x", "/caf%C3%A9", "/café", "/a<b>&\"'", "?only=query", "/x/../y",
		"mailto:a@b", "/abs#frag", "dir/",
	}
	for _, code := range []string{"301", "302", "303", "307", "308", "200"} {
		for _, t := range targets {
			q := "/t/redirect/" + code + "?to=" + queryEscape(t)
			c.one("redirect "+code+" "+t+" get", req("GET", q))
		}
	}
	for _, t := range []string{"/abs", "rel", "http://example.org/x"} {
		q := "/t/redirect/302?to=" + queryEscape(t)
		c.one("redirect head "+t, req("HEAD", q))
		c.one("redirect post "+t, post(q, ""))
		c.one("redirect ct "+t, req("GET", q+"&ct=text/plain"))
	}
	c.one("redirect from sub path", req("GET", "/t/redirect/302?to=../x&y=1"))
	for _, p := range []string{"/t/strip/x", "/t/strip/", "/t/strip/a%2Fb", "/t/strip/caf%C3%A9?q", "/t/strip//x"} {
		c.one("strip "+p, req("GET", p))
	}
}

func queryEscape(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		ch := s[i]
		if ch >= 'a' && ch <= 'z' || ch >= 'A' && ch <= 'Z' || ch >= '0' && ch <= '9' || ch == '-' || ch == '_' || ch == '.' {
			b.WriteByte(ch)
		} else {
			fmt.Fprintf(&b, "%%%02X", ch)
		}
	}
	return b.String()
}

// bodies covers request body framing and the handler's use of it.
func bodies(c *corpus) {
	c.one("echo cl", post("/openrtb2/auction", `{"id":"1"}`))
	c.one("echo cl binary", post("/openrtb2/auction", "\x00\xff\xfe"))
	c.one("echo video limit", post("/openrtb2/video", "0123456789abcdef"))
	c.one("echo video limit pipelined", post("/openrtb2/video", "0123456789abcdef")+req("GET", "/status"))
	c.one("echo chunked", chunked("/openrtb2/auction", "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"))
	c.one("echo chunked ext", chunked("/openrtb2/auction", "5;ext=1;b=\"q\"\r\nhello\r\n0;x\r\n\r\n"))
	c.one("echo chunked trailer", chunked("/openrtb2/auction", "5\r\nhello\r\n0\r\nX-T: tv\r\nX-T: tv2\r\n\r\n", "Trailer: X-T"))
	c.one("echo chunked undeclared trailer", chunked("/openrtb2/auction", "5\r\nhello\r\n0\r\nX-U: uv\r\n\r\n"))
	c.one("echo chunked declared missing", chunked("/openrtb2/auction", "5\r\nhello\r\n0\r\n\r\n", "Trailer: X-T, X-U"))
	c.one("echo chunked forbidden trailer", chunked("/openrtb2/auction", "0\r\n\r\n", "Trailer: Content-Length"))
	c.one("echo chunked trailer host", chunked("/openrtb2/auction", "0\r\n\r\n", "Trailer: Host"))
	c.one("echo chunked bad trailer", chunked("/openrtb2/auction", "0\r\nBad Trailer\r\n\r\n"))
	c.one("echo chunked bad size", chunked("/openrtb2/auction", "zz\r\nhello\r\n0\r\n\r\n"))
	c.one("echo chunked missing crlf", chunked("/openrtb2/auction", "5\r\nhelloXX0\r\n\r\n"))
	c.one("echo chunked bare lf", chunked("/openrtb2/auction", "5\nhello\n0\n\n"))
	c.one("echo chunked lf after data", chunked("/openrtb2/auction", "5\r\nhello\n0\r\n\r\n"))
	c.one("echo chunked overflow", chunked("/openrtb2/auction", "ffffffffffffffffff\r\nhello\r\n0\r\n\r\n"))
	c.one("echo chunked leading zeros", chunked("/openrtb2/auction", "0005\r\nhello\r\n0\r\n\r\n"))
	c.one("echo chunked upper hex", chunked("/openrtb2/auction", "A\r\n0123456789\r\n0\r\n\r\n"))
	c.one("echo chunked space size", chunked("/openrtb2/auction", "5 \r\nhello\r\n0\r\n\r\n"))
	c.one("echo chunked empty size", chunked("/openrtb2/auction", "\r\nhello\r\n0\r\n\r\n"))
	c.one("echo chunked pipelined", chunked("/openrtb2/auction", "3\r\nabc\r\n0\r\n\r\n")+req("GET", "/version"))
	c.one("echo chunked big", chunked("/openrtb2/auction", "1000\r\n"+strings.Repeat("z", 4096)+"\r\n0\r\n\r\n"))
	c.one("body none small", post("/t/body/none", "0123456789")+req("GET", "/status"))
	c.one("body some", post("/t/body/some", strings.Repeat("s", 20))+req("GET", "/status"))
	c.one("body some short", post("/t/body/some", "short"))
	c.one("body all", post("/t/body/all", strings.Repeat("a", 20)))
	c.one("body max", post("/t/body/max", strings.Repeat("m", 20))+req("GET", "/status"))
	c.one("body max chunked", chunked("/t/body/max", "14\r\n"+strings.Repeat("m", 20)+"\r\n0\r\n\r\n"))
	c.one("body max exact", post("/t/body/max", strings.Repeat("m", 16))+req("GET", "/status"))
	c.one("body maxok", post("/t/body/maxok", strings.Repeat("m", 20)))
	c.one("body close", post("/t/body/close", "abc"))
	c.one("body big unread", post("/t/body/big", strings.Repeat("u", 100))+req("GET", "/status"))
	c.one("body chunked unread", chunked("/t/body/none", "5\r\nhello\r\n0\r\n\r\n")+req("GET", "/status"))
	c.one("body chunked bad unread", chunked("/t/body/none", "zz\r\n")+req("GET", "/status"))
	c.one("body 1.0 keepalive", raw("POST /t/body/none HTTP/1.0", []string{"Connection: keep-alive", "Content-Length: 3"}, "abc")+raw("GET /status HTTP/1.0", []string{"Connection: keep-alive"}, ""))
	c.add("body none huge", "main", 0, send("POST /t/body/none HTTP/1.1\r\nHost: x\r\nContent-Length: 300000\r\n\r\n"), sendN("h", 300000))
	c.add("body none 200k", "main", 0, send("POST /t/body/none HTTP/1.1\r\nHost: x\r\nContent-Length: 200000\r\n\r\n"), sendN("h", 200000), send(req("GET", "/status")))
	c.add("body chunked huge unread", "main", 0, send("POST /t/body/none HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n"), sendN("1000\r\n"+strings.Repeat("c", 4096)+"\r\n", 70), send("0\r\n\r\n"))
	c.add("echo large", "main", 0, send("POST /openrtb2/auction HTTP/1.1\r\nHost: x\r\nContent-Length: 20000\r\n\r\n"), sendN("l", 20000))
	c.one("form post", post("/t/form?a=1&b=2", "b=3&c=%20x+y", "Content-Type: application/x-www-form-urlencoded"))
	c.one("form post charset", post("/t/form", "z=1&a=2", "Content-Type: application/x-www-form-urlencoded; charset=utf-8"))
	c.one("form post bad", post("/t/form?q=1", "a=%zz", "Content-Type: application/x-www-form-urlencoded"))
	c.one("form post semicolon", post("/t/form", "a=1;b=2", "Content-Type: application/x-www-form-urlencoded"))
	c.one("form post other type", post("/t/form?q=1", "a=1", "Content-Type: text/plain"))
	c.one("form post no type", post("/t/form?q=1", "a=1"))
	c.one("form post bad type", post("/t/form", "a=1", "Content-Type: ;;"))
	c.one("form get", req("GET", "/t/form?x=1&x=2&y=&z"))
	c.one("form get bad", req("GET", "/t/form?a=%zz&b=1"))
	c.one("form get semicolon", req("GET", "/t/form?a=1;b=2&c=3"))
	for _, ck := range []string{
		"", "Cookie: uids=abc", "Cookie: a=1; uids=\"quoted\"; b=2", "Cookie: uids=", "Cookie: uids=a b",
		"Cookie: other=1", "Cookie: uids=1\r\nCookie: uids=2", "Cookie: ;;uids=x;;", "Cookie: uids=a\"b",
		"Cookie: UIDS=upper", "Cookie: uids=%7B%7D",
	} {
		hs := []string{}
		if ck != "" {
			hs = append(hs, ck)
		}
		c.one("cookie "+ck, req("GET", "/t/cookie", hs...))
	}
}

func connections(c *corpus) {
	status := req("GET", "/status")
	version := req("GET", "/version")
	c.one("pipeline two", status+version)
	c.one("pipeline three close second", status+req("GET", "/version", "Connection: close")+status)
	c.one("pipeline after post", post("/openrtb2/auction", "abc")+status)
	c.one("pipeline after error", "GET /status HTTP/1.1\r\n\r\n"+status)
	c.one("pipeline after head", req("HEAD", "/t/write/10")+status)
	c.one("pipeline head chunked", req("HEAD", "/t/write/5000")+status)
	c.one("pipeline after 404", req("GET", "/nope")+status)
	c.one("pipeline after redirect", req("GET", "/status/")+status)
	c.one("pipeline after static", req("GET", "/static/big.txt")+status)
	c.one("pipeline after 405", req("DELETE", "/status")+status)
	c.one("pipeline 10", strings.Repeat(status, 10))
	c.one("pipeline 1.0 keepalive", raw("GET /status HTTP/1.0", []string{"Connection: keep-alive"}, "")+raw("GET /version HTTP/1.0", []string{"Connection: keep-alive"}, ""))
	c.one("pipeline 1.0 no keepalive", raw("GET /version HTTP/1.0", nil, "")+raw("GET /status HTTP/1.0", nil, ""))
	c.one("pipeline 1.0 keepalive chunk", raw("GET /t/write/5000 HTTP/1.0", []string{"Connection: keep-alive"}, "")+status)
	c.one("pipeline 1.0 keepalive cl", raw("GET /t/cl/3/3 HTTP/1.0", []string{"Connection: keep-alive"}, "")+status)
	c.one("pipeline 1.0 keepalive head", raw("HEAD /t/write/5000 HTTP/1.0", []string{"Connection: keep-alive"}, "")+status)
	c.one("pipeline 1.0 keepalive 304", raw("GET /t/status/304/n HTTP/1.0", []string{"Connection: keep-alive"}, "")+status)
	c.one("pipeline 1.1 then 1.0", status+raw("GET /version HTTP/1.0", nil, ""))
	for _, h := range []string{"Connection: close", "Connection: Close", "Connection: keep-alive, close", "Connection: foo", "Connection: keep-alive", "Connection: upgrade\r\nUpgrade: h2c"} {
		c.one("conn 1.1 "+h, req("GET", "/version", h)+status)
		c.one("conn 1.0 "+h, raw("GET /version HTTP/1.0", []string{h}, "")+status)
	}
	c.add("conn idle open", "main", 0, send(status))
	c.add("conn nothing sent", "main", 0)
	c.add("conn partial request", "main", 0, send("GET /status HTTP/1.1\r\nHost: x\r\n"))
	c.add("conn two writes", "main", 0, send(status), pause(50), send(version))
}

func corsMatrix(c *corpus) {
	origins := []struct{ name, header string }{
		{"none", ""},
		{"one", "Origin: https://a.example"},
		{"null", "Origin: null"},
		{"two", "Origin: https://a.example\r\nOrigin: https://b.example"},
		{"empty", "Origin:"},
	}
	methods := []string{"POST", "GET", "DELETE", "post", ""}
	reqHeaders := []string{"", "content-type", "Content-Type, X-Requested-With", "x-custom", "accept,origin", "Content-Type , Accept", "Accept,", "content-type\r\nAccess-Control-Request-Headers: accept"}
	for _, o := range origins {
		for _, m := range methods {
			for _, h := range reqHeaders {
				hs := []string{}
				if o.header != "" {
					hs = append(hs, o.header)
				}
				hs = append(hs, "Access-Control-Request-Method: "+m)
				if h != "" {
					hs = append(hs, "Access-Control-Request-Headers: "+h)
				}
				c.one(fmt.Sprintf("cors preflight %s %q %q", o.name, m, h), req("OPTIONS", "/openrtb2/auction", hs...))
			}
		}
	}
	for _, p := range []string{"/status", "/nope", "/static/a.txt", "/openrtb2/auction/"} {
		c.one("cors preflight path "+p, req("OPTIONS", p, "Origin: https://a.example", "Access-Control-Request-Method: GET"))
	}
	c.one("cors preflight private network", req("OPTIONS", "/openrtb2/auction", "Origin: https://a.example", "Access-Control-Request-Method: POST", "Access-Control-Request-Private-Network: true"))
	c.one("cors preflight 1.0", raw("OPTIONS /openrtb2/auction HTTP/1.0", []string{"Origin: https://a.example", "Access-Control-Request-Method: POST"}, ""))
	c.one("cors preflight body", raw("OPTIONS /openrtb2/auction HTTP/1.1", []string{"Host: x", "Origin: https://a.example", "Access-Control-Request-Method: POST", "Content-Length: 3"}, "abc")+req("GET", "/status"))
	for _, o := range origins {
		hs := []string{}
		if o.header != "" {
			hs = append(hs, o.header)
		}
		c.one("cors actual get status "+o.name, req("GET", "/status", hs...))
		c.one("cors actual get version "+o.name, req("GET", "/version", hs...))
		c.one("cors actual post "+o.name, post("/openrtb2/auction", "{}", hs...))
		c.one("cors actual options "+o.name, req("OPTIONS", "/status", hs...))
		c.one("cors actual 404 "+o.name, req("GET", "/nope", hs...))
		c.one("cors actual static "+o.name, req("GET", "/static/a.txt", hs...))
		c.one("cors actual redirect "+o.name, req("GET", "/status/", hs...))
		c.one("cors actual head "+o.name, req("HEAD", "/version", hs...))
		c.one("cors actual vary "+o.name, req("GET", "/t/header/multi", hs...))
	}
}

func admin(c *corpus) {
	for _, p := range []string{
		"/version", "/version/", "/Version", "/debug/pprof/", "/debug/pprof", "/debug/pprof/heap",
		"/debug/pprof/cmdline", "/debug/pprof/cmdline/", "/debug/pprof/profile?seconds=1",
		"/debug/pprof/symbol", "/debug/pprof/trace", "/currency/rates", "/currency/rates?x=1",
		"/currency/rates/", "/currency", "/", "/nope", "//version", "/debug/pprof/../../version",
		"/debug/%70prof/", "/debug/pprof/a%2Fb", "/./version", "/version?x=1#y", "*",
		"http://other.example/version", "/debug//pprof/", "/currency/../version?q=1",
	} {
		for _, m := range []string{"GET", "HEAD", "POST", "OPTIONS"} {
			c.add("admin "+m+" "+p, "admin", 0, send(req(m, p)))
		}
	}
	c.add("admin connect authority", "admin", 0, send("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"))
	c.add("admin connect path", "admin", 0, send("CONNECT /version HTTP/1.1\r\nHost: x\r\n\r\n"))
	c.add("admin host port", "admin", 0, send(raw("GET /version HTTP/1.1", []string{"Host: example.com:6060"}, "")))
	c.add("admin no host", "admin", 0, send("GET /version HTTP/1.1\r\n\r\n"))
	c.add("admin 1.0", "admin", 0, send("GET /version HTTP/1.0\r\n\r\n"))
	c.add("admin pipeline", "admin", 0, send(req("GET", "/version")+req("GET", "/debug/pprof")+req("GET", "/nope")))
	c.add("admin post body", "admin", 0, send(post("/currency/rates", "abc")))
	c.add("admin asterisk 1.0", "admin", 0, send("GET * HTTP/1.0\r\n\r\n"))
}

// short runs against the server with a 300ms read timeout and a 1s write timeout.
func short(c *corpus) {
	c.add("short stall headers", "short", 1000, send("GET /status HTTP/1.1\r\nHost: x\r\n"))
	c.add("short stall nothing", "short", 1000)
	c.add("short idle close", "short", 1000, send(req("GET", "/status")))
	c.add("short slow body", "short", 1500, send("POST /t/slowbody HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nhello"))
	c.add("short slow chunked body", "short", 1500, send("POST /t/slowbody HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n"))
	c.add("short paused body", "short", 1000, send("POST /t/slowbody HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\n"), pause(100), send("hello"))
	c.add("short sleep under", "short", 1000, send(req("GET", "/t/sleep/100")))
	c.add("short sleep over", "short", 2500, send(req("GET", "/t/sleep/1300")))
	c.add("short keepalive then idle", "short", 1000, send(req("GET", "/status")), pause(100), send(req("GET", "/status")))
}

// plain runs against the main handler on a listener without connection metrics, whose
// connections the server can CloseWrite.
func plain(c *corpus) {
	big := "GET /status HTTP/1.1\r\nHost: x\r\nX-Big: "
	c.add("plain header too large", "plain", 0, send(big), sendN("a", 1100000), send("\r\n\r\n"))
	c.add("plain body max", "plain", 0, send(post("/t/body/max", strings.Repeat("m", 20))+req("GET", "/status")))
	c.add("plain body none huge", "plain", 0, send("POST /t/body/none HTTP/1.1\r\nHost: x\r\nContent-Length: 300000\r\n\r\n"), sendN("h", 300000))
	c.add("plain expect 100 noread", "plain", 0, send("POST /t/body/none HTTP/1.1\r\nHost: x\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhello"))
	c.add("plain status", "plain", 0, send(req("GET", "/status")))
	c.add("plain close", "plain", 0, send(req("GET", "/status", "Connection: close")))
	c.add("plain bad request", "plain", 0, send("GET /status HTTP/1.1\r\n\r\n"))
	c.add("plain panic", "plain", 0, send(req("GET", "/t/panic/flush")))
}

// unix runs against the main handler over Prebid's Unix socket listener.
func unix(c *corpus) {
	c.add("unix status", "unix", 0, send(req("GET", "/status")))
	c.add("unix remote", "unix", 0, send(req("GET", "/t/remote")))
	c.add("unix echo", "unix", 0, send(post("/openrtb2/auction", "abc")))
	c.add("unix pipeline", "unix", 0, send(req("GET", "/version")+req("GET", "/t/write/3000")+req("GET", "/static/a.txt")))
	c.add("unix bad request", "unix", 0, send("GET /status HTTP/1.1\r\n\r\n"))
}
