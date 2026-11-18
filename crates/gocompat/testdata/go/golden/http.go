package main

import (
	"net/http"
	"strings"
)

func genHTTP() {
	var canon table
	for _, in := range []string{
		// net/textproto canonicalHeaderKeyTests.
		"a-b-c", "a-1-c", "User-Agent", "uSER-aGENT", "user-agent", "USER-AGENT",
		"foo-bar_baz", "foo-bar$baz", "foo-bar~baz", "foo-bar*baz",
		"üser-agenT", "a B", "C Ontent-Transfer-Encoding", "foo bar",
		// Header names used by Prebid adapters and endpoints.
		"x-openrtb-version", "X-OPENRTB-VERSION", "content-type", "TOKEN", "x-forwarded-for",
		"X-Device-User-Agent", "x-device-ip", "sec-gpc", "Sec-Browsing-Topics", "dnt",
		"accept-language", "x-api-key", "ALLOW-ORIGIN", "a--b", "-a", "a-", "", "a:b",
		"x_y", "X-Real-IP", "x.y", "x\tz", "A\x7fB",
	} {
		canon.row(in, http.CanonicalHeaderKey(in))
	}
	canon.write("http_canonical.txt")

	h := http.Header{}
	h.Add("x-openrtb-version", "2.5")
	h.Add("X-OPENRTB-VERSION", "2.6")
	h.Set("content-type", "application/json;charset=utf-8")
	h.Add("accept", " application/json\r\n")
	h["TOKEN"] = []string{"raw"}
	h["x-lower"] = []string{"literal"}
	h["bad key"] = []string{"dropped"}
	h.Add("User-Agent", "ua")
	var all, subset strings.Builder
	if err := h.Write(&all); err != nil {
		panic(err)
	}
	if err := h.WriteSubset(&subset, map[string]bool{"Host": true, "User-Agent": true, "Content-Length": true, "Transfer-Encoding": true, "Trailer": true}); err != nil {
		panic(err)
	}
	var ops table
	ops.row("get canonical", h.Get("X-Openrtb-Version"))
	ops.row("values", strings.Join(h.Values("x-openrtb-version"), ","))
	ops.row("get raw upper key via Get", h.Get("token"))
	ops.row("get lower literal via Get", h.Get("x-lower"))
	ops.row("index lower literal", strings.Join(h["x-lower"], ","))
	ops.row("write", all.String())
	ops.row("write subset", subset.String())
	ops.write("http_header.txt")
}
