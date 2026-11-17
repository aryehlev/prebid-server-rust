package main

import (
	"encoding/hex"
	"net/http"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/util/httputil"
	"github.com/prebid/prebid-server/v4/util/iputil"
)

// headerIPs are the address texts the FindIP cases combine: public and private IPv4 and IPv6
// addresses of the default request_validation networks, IPv4-mapped forms, zones and junk.
var headerIPs = []string{
	"1.1.1.1", "8.8.4.4", "203.0.113.7", "10.0.0.1", "10.255.255.255", "172.16.0.1",
	"172.31.255.254", "172.32.0.1", "192.168.1.1", "169.254.10.10", "127.0.0.1", "100.64.0.1",
	"0.0.0.0", "255.255.255.255", "2001:4860:4860::8888", "2600::1", "::1", "fc00::1",
	"fd00::abcd", "fe80::1", "ff02::1", "2001:db8::1", "::ffff:10.0.0.1", "::ffff:8.8.8.8",
	"::ffff:192.168.0.1", "::", "fe80::1%eth0", "1.1.1.1%eth0", "unknown", "malformed", "",
	"01.1.1.1", "1.1.1", "1.1.1.1:80", "[::1]", "[2600::1]:443", "::ffff:1.1.1.1",
}

// headerPads are the whitespace a value may be wrapped in, including the Unicode spaces
// strings.TrimSpace removes and bytes it keeps.
var headerPads = []string{"", "", "", " ", "  ", "\t", "\n", "\r\n", "\v\f", " ", " ", "　", "\u0085", "​", "\xa0", "\xff"}

var remoteAddrs = []string{
	"", "5.5.5.5:5", "5.5.5.5:80", "10.0.0.5:1234", "127.0.0.1:8000", "[5555::]:5",
	"[::1]:8000", "[fc00::5]:1", "[fe80::1%eth0]:80", "[::ffff:5.5.5.5]:5", "5.5.5.5",
	"malformed", "@", "[5555::]:5]", "::1:80", "[5555::]", "6.6.6.6:", ":80",
	"[2001:db8::5]:443", "[2600::5]:443", " 5.5.5.5:5", "5.5.5.5 :5",
}

func padded(r *splitmix64, s string) string {
	return pick(r, headerPads) + s + pick(r, headerPads)
}

// headerValue draws an optional header value: absent ("" means not added), one address, or
// for X-Forwarded-For a comma-separated list.
func headerValue(r *splitmix64, list bool) string {
	if r.intn(3) == 0 {
		return ""
	}
	if !list {
		return padded(r, pick(r, headerIPs))
	}
	n := 1 + r.intn(4)
	parts := make([]string, n)
	for i := range parts {
		parts[i] = padded(r, pick(r, headerIPs))
	}
	sep := ","
	if r.intn(2) == 0 {
		sep = ", "
	}
	return strings.Join(parts, sep)
}

type findIPCase struct {
	trueClientIP, xForwardedFor, xRealIP, remoteAddr string
	// secondXFF is a second X-Forwarded-For value; Header.Get only reads the first.
	secondXFF string
}

// genHTTPUtil writes the FindIP, ContentEncoding.Normalize and Pixel1x1PNG goldens.
func genHTTPUtil() {
	cases := []findIPCase{
		{},
		{trueClientIP: "1.1.1.1", xForwardedFor: "2.2.2.2, 3.3.3.3", xRealIP: "4.4.4.4", remoteAddr: "5.5.5.5:5"},
		{trueClientIP: "   1.1.1.1 ", xForwardedFor: "2.2.2.2, 3.3.3.3", xRealIP: "4.4.4.4", remoteAddr: "5.5.5.5:5"},
		{xForwardedFor: "   2.2.2.2, 3.3.3.3 ", xRealIP: "4.4.4.4", remoteAddr: "5.5.5.5:5"},
		{xRealIP: "   4.4.4.4 ", remoteAddr: "5.5.5.5:5"},
		{remoteAddr: "5.5.5.5:80"},
		{trueClientIP: "1111::", xForwardedFor: "2222::, 3333::", xRealIP: "4444::", remoteAddr: "[5555::]:5"},
		{remoteAddr: "[5555::]:5"},
		{trueClientIP: "malformed", xForwardedFor: "malformed", xRealIP: "malformed", remoteAddr: "malformed"},
		{trueClientIP: "malformed", xForwardedFor: "malformed, 4.4.4.4, 3333::, malformed", xRealIP: "malformed", remoteAddr: "malformed"},
		{xForwardedFor: "10.0.0.1, 192.168.0.1, 8.8.8.8", remoteAddr: "10.0.0.9:1"},
		{xForwardedFor: ",,8.8.8.8,", remoteAddr: "10.0.0.9:1"},
		{xForwardedFor: "10.0.0.1", secondXFF: "8.8.8.8", remoteAddr: "10.0.0.9:1"},
		{trueClientIP: "::ffff:10.0.0.1", remoteAddr: "10.0.0.9:1"},
		{trueClientIP: " 8.8.8.8 ", remoteAddr: "10.0.0.9:1"},
		{trueClientIP: "\xa08.8.8.8", xForwardedFor: "\xff, 9.9.9.9", remoteAddr: "10.0.0.9:1"},
	}
	r := &splitmix64{state: 0x5EED_0005}
	for range 3000 {
		c := findIPCase{
			trueClientIP:  headerValue(r, false),
			xForwardedFor: headerValue(r, true),
			xRealIP:       headerValue(r, false),
			remoteAddr:    pick(r, remoteAddrs),
		}
		if r.intn(10) == 0 {
			c.secondXFF = headerValue(r, true)
		}
		cases = append(cases, c)
	}

	validators := []struct {
		name string
		v    iputil.IPValidator
	}{
		{"default", defaultValidator()},
		{"empty", iputil.PublicNetworkIPValidator{}},
	}

	// httputil_findip.txt: validator, True-Client-IP, X-Forwarded-For (first and second),
	// X-Real-IP, RemoteAddr, then the IP bytes, IP.String() and version FindIP returns.
	var t table
	for _, c := range cases {
		req, err := http.NewRequest("GET", "http://anyurl.com", nil)
		check(err)
		if c.trueClientIP != "" {
			req.Header.Add("True-Client-IP", c.trueClientIP)
		}
		if c.xForwardedFor != "" {
			req.Header.Add("X-Forwarded-For", c.xForwardedFor)
		}
		if c.secondXFF != "" {
			req.Header.Add("X-Forwarded-For", c.secondXFF)
		}
		if c.xRealIP != "" {
			req.Header.Add("X-Real-IP", c.xRealIP)
		}
		req.RemoteAddr = c.remoteAddr
		for _, v := range validators {
			ip, ver := httputil.FindIP(req, v.v)
			t.row(v.name, c.trueClientIP, c.xForwardedFor, c.secondXFF, c.xRealIP, c.remoteAddr,
				ipHex(ip), ip.String(), strconv.Itoa(int(ver)))
		}
	}
	t.write("httputil_findip.txt")

	// httputil_misc.txt: canonical header keys, ContentEncoding.Normalize and the pixel.
	var misc table
	misc.row("key", "True-Client-IP", http.CanonicalHeaderKey("True-Client-IP"))
	misc.row("key", "X-Forwarded-For", http.CanonicalHeaderKey("X-Forwarded-For"))
	misc.row("key", "X-Real-IP", http.CanonicalHeaderKey("X-Real-IP"))
	for _, s := range []string{"gzip", "GZIP", "GZip", "br", "", "gzip ", "İ", "K", "DEFLATE", "\xff", "gz\u0130p", "GZ\u0130P", "gzi\u0307p"} {
		n := httputil.ContentEncoding(s).Normalize()
		misc.row("normalize", s, string(n), strconv.FormatBool(n == httputil.ContentEncodingGZIP))
	}
	misc.row("pixel", httputil.Pixel1x1PNG.ContentType, hex.EncodeToString(httputil.Pixel1x1PNG.Content))
	misc.write("httputil_misc.txt")
}
