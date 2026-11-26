package main

// The edge-case goldens of the InfluxDB client: exhaustive and randomized inputs to what the
// client of github.com/influxdata/influxdb1-client 8bf82d3c094d and the reporter of
// github.com/vrischmann/go-metrics-influxdb v0.1.1 build their output with. Fields are
// strconv.Quote'd and tab-separated unless noted.
//
//	keys.txt    client.Point{Measurement: s, Tags: {s: s+"x"}, Fields: {s+"f": int64(1)}}
//	            .MarshalString() for every s of up to four bytes over a, comma, space, equals,
//	            backslash and double quote: the escaping of measurements, tags and field keys
//	paths.txt   path.Join(p, "write") and path.Clean(p) for every p of up to six bytes over
//	            slash, dot and a, the paths Write and Ping post to (unquoted: "" is written -)
//	errorf.txt  the error client.Write returns (fmt.Errorf(string(body))) when a 500 answers it
//	            with each crafted and random body, the writes made by one client over one
//	            connection to a fakeEndpoint
//	urls.txt    url.Parse of crafted metrics.influxdb.host values: "error" and the text the
//	            reporter logs it with, or "ok" and the URL written back

import (
	"fmt"
	"math/rand"
	"net/url"
	"os"
	"path"
	"strconv"
	"strings"

	client "github.com/influxdata/influxdb1-client"
)

// keyAlphabet is what keys.txt strings are made of: the bytes the line protocol escapes, the
// escape character and one that is never escaped.
var keyAlphabet = []string{"a", ",", " ", "=", "\\", "\""}

// allStrings returns every string of up to n units over alphabet, shortest first.
func allStrings(alphabet []string, n int) []string {
	all := []string{""}
	level := []string{""}
	for i := 0; i < n; i++ {
		var next []string
		for _, s := range level {
			for _, c := range alphabet {
				next = append(next, s+c)
			}
		}
		all = append(all, next...)
		level = next
	}
	return all
}

func writeKeys(path string) error {
	var b strings.Builder
	for _, s := range allStrings(keyAlphabet, 4) {
		p := client.Point{
			Measurement: s,
			Tags:        map[string]string{s: s + "x"},
			Fields:      map[string]interface{}{s + "f": int64(1)},
		}
		fmt.Fprintf(&b, "%s\t%s\n", strconv.Quote(s), strconv.Quote(p.MarshalString()))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

func writePaths(file string) error {
	dash := func(s string) string {
		if s == "" {
			return "-"
		}
		return s
	}
	var b strings.Builder
	for _, p := range allStrings([]string{"/", ".", "a"}, 6) {
		fmt.Fprintf(&b, "%s\t%s\t%s\n", dash(p), dash(path.Join(p, "write")), dash(path.Clean(p)))
	}
	return os.WriteFile(file, []byte(b.String()), 0o644)
}

// errorfBodies are crafted error bodies: every verb with and without flags, widths, precisions
// and argument indexes, too large numbers, and a body InfluxDB answers with.
var errorfBodies = []string{
	"", "%", "%%", "%%%", "100%", "50% off", "%\n", "%d", "%s", "%v", "%w", "%T", "%p", "%c", "%U",
	"%q", "%x", "%X", "%e", "%E", "%f", "%F", "%g", "%G", "%b", "%o", "%O", "%t", "%!", "%é", "%😀",
	"%[1]d", "%[0]d", "%[2]s", "%[x]d", "%[1", "%[", "%[]d", "%[1]%", "%[-1]d", "%*d", "%.*d",
	"%[3]*.[2]*[1]f", "%-08.3x", "%+q", "%#v", "% d", "%.", "%.5", "%5", "%-", "%+", "%#", "% ",
	"%0", "%9999999999d", "%.9999999999d", "%1000000d", "%1000001d", "%[99999999999999999999]d",
	"%!d(MISSING)", "%v%v", "a%sb%dc", "%.2f%%", "%-5%", "%5.2%",
	"{\"error\":\"partial write: field type conflict: input field \\\"value\\\" on measurement \\\"cpu\\\" is type float, already exists as type integer dropped=1\"}\n",
}

// errorfTokens are what random errorf bodies are made of.
var errorfTokens = []string{
	"%", "%", "%", "d", "s", "v", "w", "x", "q", "!", "[", "]", "1", "2", "0", "9", "*", ".", "-",
	"+", "#", " ", "é", "a", "\n", "%%", "%[1]", "%[0]", "%[2]d", "%.*d", "%!", "😀",
}

// writeErrorf answers the writes of one client, over one connection, with a 500 status and each
// body in turn, and records the error each write returns.
func writeErrorf(path string) error {
	bodies := append([]string(nil), errorfBodies...)
	r := rand.New(rand.NewSource(20261122))
	for i := 0; i < 1500; i++ {
		var s strings.Builder
		for n := r.Intn(9); n > 0; n-- {
			s.WriteString(errorfTokens[r.Intn(len(errorfTokens))])
		}
		bodies = append(bodies, s.String())
	}
	responses := make([]string, len(bodies))
	for i, body := range bodies {
		responses[i] = answer("500 Internal Server Error", body)
	}
	e, err := newFakeEndpoint(responses...)
	if err != nil {
		return err
	}
	defer e.close()
	u, err := url.Parse("http://" + e.addr())
	if err != nil {
		return err
	}
	cl, err := client.NewClient(client.Config{URL: *u})
	if err != nil {
		return err
	}
	point := client.Point{Measurement: "m", Fields: map[string]interface{}{"f": int64(1)}}
	var b strings.Builder
	for _, body := range bodies {
		_, err := cl.Write(client.BatchPoints{Points: []client.Point{point}, Database: "pbs"})
		if err == nil {
			return fmt.Errorf("body %q: no error", body)
		}
		// The request arrived before its answer.
		<-e.arrived
		fmt.Fprintf(&b, "%s\t%s\n", strconv.Quote(body), strconv.Quote(err.Error()))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}

// hostURLs are crafted metrics.influxdb.host values.
var hostURLs = []string{
	"http://localhost:8086", "https://influx.example.com:8086/base/", "localhost:8086", "localhost",
	"//influx/p", "http:///p", "HTTP://H/P", "mailto:a@b", "http://[::1]:8086/x",
	"http://[fe80::1%25en0]:8086/", "http://ex%61mple.com/", "http://u%20ser:p%40ss@h/",
	"http://h/a b", "http://h/%7e", "http://h?q=%zz", "http://h#%zz", "http://h:99999",
	"http://[::1", "%zz", ":foo", "http://a b", "http://h:port", "http://h:-1", "http://h/%zz",
	"http://%41:8080/", "http://[fe80::1%en0]:8086/", "\x7f", "http://h\n", " http://h",
	"http://[::1]x", "http://h:8086:8086", "http://user@:80", "http://h/\x00", "ht tp://h",
	"1http://h", "http://h/é", "http://é.com/",
}

func writeURLs(path string) error {
	var b strings.Builder
	for _, raw := range hostURLs {
		result, text := "ok", ""
		if u, err := url.Parse(raw); err != nil {
			result, text = "error", err.Error()
		} else {
			text = u.String()
		}
		fmt.Fprintf(&b, "%s\t%s\t%s\n", strconv.Quote(raw), strconv.Quote(result), strconv.Quote(text))
	}
	return os.WriteFile(path, []byte(b.String()), 0o644)
}
