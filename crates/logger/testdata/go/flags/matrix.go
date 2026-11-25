package main

import (
	"encoding/hex"
	"flag"
	"strings"
	"unicode/utf8"
)

// matrixCases are the command lines of flags.json: every flag with good, bad and edge values in
// each syntax the flag package reads, help, undefined and malformed flags, the ends of the flag
// list, repeats, and the orders in which a value set before a failing argument decides.
var matrixCases = [][]string{
	{},
	{"-v", "1", "-logtostderr"}, // the image's CMD (Dockerfile:50)
	{"--stderrthreshold=WARNING", "-v=2"},
	{"--alsologtostderr"},
	{"-logtostderr=false", "-alsologtostderr=1", "-stderrthreshold", "info"},
	{"-stderrthreshold=fatal"},
	{"-stderrthreshold=3"},
	{"-stderrthreshold=256"},
	{"-stderrthreshold=259"},
	{"-stderrthreshold=-1"},
	{"-stderrthreshold=4"},
	{"-stderrthreshold=x"},
	{"-stderrthreshold="},
	{"-stderrthreshold=Warning"},
	{"-stderrthreshold=ınfo"},
	{"-stderrthreshold=+2"},
	{"-stderrthreshold=0x2"},
	{"-stderrthreshold=99999999999999999999"},
	{"-stderrthreshold"},
	{"-v=x"},
	{"-v="},
	{"-v=-3"},
	{"-v=4294967297"},
	{"-v=99999999999999999999"},
	{"-v", "+5"},
	{"-v=0x10"},
	{"-v=1_0"},
	{"-v"},
	{"-v", "-logtostderr"},
	{"--v=3"},
	{"-vmodule=recordio=2,foo/bar/baz=1,gfs*=3"},
	{"-vmodule=a=1,,b=2,"},
	{"-vmodule=a"},
	{"-vmodule=a=b"},
	{"-vmodule==1"},
	{"-vmodule=a=1=2"},
	{"-vmodule=a=4294967296"},
	{"-vmodule=a=99999999999999999999"},
	{"-vmodule="},
	{"-vmodule=a="},
	{"-log_backtrace_at=recordio.go:234,sstable.go:456"},
	{"-log_backtrace_at=x:1"},
	{"-log_backtrace_at=x.go"},
	{"-log_backtrace_at=x.go:-1"},
	{"-log_backtrace_at=x.go:y"},
	{"-log_backtrace_at=a.go:1:2"},
	{"-log_backtrace_at=,a.go:3,"},
	{"-log_backtrace_at=a.go:+7"},
	{"-log_backtrace_at="},
	{"-log_dir=/var/log/pbs", "-log_link", "/tmp/links"},
	{"-log_dir"},
	{"-log_dir="},
	{"-log_dir", "-h"},
	{"-logbuflevel=-1"},
	{"-logbuflevel=0x7f"},
	{"-logbuflevel=1_0"},
	{"-logbuflevel=_1"},
	{"-logbuflevel=0_7"},
	{"-logbuflevel=08"},
	{"-logbuflevel=0b101"},
	{"-logbuflevel=0o17"},
	{"-logbuflevel=0x"},
	{"-logbuflevel=x"},
	{"-logbuflevel=9223372036854775807"},
	{"-logbuflevel=9223372036854775808"},
	{"-logbuflevel=-9223372036854775808"},
	{"-logbuflevel=-9223372036854775809"},
	{"-logbuflevel=99999999999999999999x"},
	{"-logbuflevel", "0b101"},
	{"-logtostderr=x"},
	{"-logtostderr=T"},
	{"-logtostderr=FALSE"},
	{"-logtostderr=tRUE"},
	{"-logtostderr="},
	{"-logtostderr", "true"},
	{"-logtostderr", "-logtostderr=x"},
	{"-alsologtostderr", "-alsologtostderr=2"},
	{"-logbuflevel=5", "-logbuflevel=x"},
	{"-v=7", "-v=x"},
	{"-stderrthreshold=0", "-stderrthreshold=9"},
	{"-vmodule=a=1", "-vmodule=b"},
	{"-log_backtrace_at=a.go:1", "-log_backtrace_at=b"},
	{"-h"},
	{"-help"},
	{"--help"},
	{"--h"},
	{"-help=1"},
	{"-h=x"},
	{"-nope"},
	{"-x"},
	{"-v=1", "-nope"},
	{"---v"},
	{"---"},
	{"-=x"},
	{"--=x"},
	{"-="},
	{"-"},
	{"--"},
	{"--", "-v=1"},
	{"arg", "-v=1"},
	{"-v=1", "arg", "-logtostderr"},
	{"-v=1", "-v=2", "-logtostderr", "-logtostderr=0"},
	{"-logtostderr", "-stderrthreshold=WARNING", "-stderrthreshold=ERROR"},
	// flag.Parse sets each value as it reads it, so a bad value before -h or an undefined flag
	// decides the outcome.
	{"-logtostderr=x", "-h"},
	{"-v=x", "-nope"},
	{"-h", "-v=x"},
	{"-nope", "-h"},
	{"-logtostderr", "-h"},
	{"-v", "-h"},
	{"-logtostderr=x", "-v"},
	{"-stderrthreshold=9", "---v"},
	{"-v=1", "-log_dir"},
	{"-", "-h"},
	{"x", "-h"},
	{"--", "-h"},
	{"-logtostderr", "true", "-v=x"},
	// Values the error line quotes: strconv.Quote escapes control characters its own way.
	{"-v=\x01"},
	{"-logtostderr=\x7f"},
	{"-v=\t"},
	{"-stderrthreshold=é"},
	{"-é"},
	{"-log_dir=é"},
	{"-v=\"q\""},
}

// bytesCases hold arguments that are not UTF-8.
var bytesCases = [][]string{
	{"-log_dir=\xff"},
	{"x\xff"},
	{"-v=\xff"},
}

// httptestCases hold -httptest.serve, which net/http/httptest's init registers when an argument
// starts with -httptest.serve= or --httptest.serve= (net/http/httptest/server.go:89-93): the
// flag then takes a value as glog's string flags do, the listing names it, and only an httptest
// server, which no code of Prebid Server starts, reads it. Without such an argument it is not
// defined.
var httptestCases = [][]string{
	{"-httptest.serve=127.0.0.1:8000"},
	{"--httptest.serve=127.0.0.1:8000", "-v", "1", "-logtostderr"},
	{"-httptest.serve", "127.0.0.1:8000", "-httptest.serve="},
	{"-httptest.serve", "127.0.0.1:8000"},
	{"-h", "-httptest.serve="},
	{"-nope", "-httptest.serve=127.0.0.1:8000"},
	{"arg", "-httptest.serve=127.0.0.1:8000"},
}

type matrixDoc struct {
	Header header `json:"header"`
	// Usage is what flag.Parse writes after -h, and after the error line of a failure.
	Usage string      `json:"usage"`
	Rows  []matrixRow `json:"rows"`
	// BytesRows are command lines with arguments that are not UTF-8, hex-encoded.
	BytesRows []matrixRow `json:"bytes_rows"`
	// HTTPTestRows are the command lines of httptestCases, parsed by the child alone.
	HTTPTestRows []matrixRow `json:"httptest_rows"`
}

type matrixRow struct {
	Args []string `json:"args"`
	// Outcome is "parsed", "help" or "error".
	Outcome string `json:"outcome"`
	// Exit is the exit code of a help (0) or an error (2); Prebid Server runs on after parsing.
	Exit *int `json:"exit,omitempty"`
	// ErrorLine is the line flag.Parse writes before the usage listing on an error.
	ErrorLine string `json:"error_line,omitempty"`
	// Values maps every flag to its String() once parsed.
	Values map[string]string `json:"values,omitempty"`
	// Rest is flag.Args() once parsed.
	Rest []string `json:"rest,omitempty"`
	// Usage is the listing written after -h or the error line when it is not the document's,
	// as when it names -httptest.serve.
	Usage string `json:"usage,omitempty"`
}

func buildMatrix(h header) matrixDoc {
	flags := glogFlags(true)
	help, _ := runChild([]string{"-h"})
	doc := matrixDoc{Header: h, Usage: help.Stderr}
	if !strings.HasPrefix(doc.Usage, "Usage of "+program+":\n") {
		fail("unexpected usage listing: " + doc.Usage)
	}
	for _, args := range matrixCases {
		for _, arg := range args {
			if !utf8.ValidString(arg) {
				fail("a matrix argument is not UTF-8")
			}
		}
		doc.Rows = append(doc.Rows, row(flags, args, doc.Usage, false))
	}
	for _, args := range bytesCases {
		doc.BytesRows = append(doc.BytesRows, row(flags, args, doc.Usage, true))
	}
	for _, args := range httptestCases {
		doc.HTTPTestRows = append(doc.HTTPTestRows, httptestRow(args, doc.Usage))
	}
	return doc
}

// httptestRow runs args in the child alone, as the copy of glog's flags has no -httptest.serve,
// checks what flag.Parse wrote as row does, and records the listing when it is not usage.
func httptestRow(args []string, usage string) matrixRow {
	r, exit := runChild(args)
	out := matrixRow{Args: args, Outcome: r.Outcome}
	listing := ""
	switch r.Outcome {
	case "help":
		listing = r.Stderr
		out.Exit = &exit
	case "error":
		line, after, ok := strings.Cut(r.Stderr, "\n")
		if !ok {
			fail("an error wrote no line")
		}
		listing = after
		out.Exit = &exit
		out.ErrorLine = line
	case "parsed":
		if r.Stderr != "" {
			fail("a parse wrote to stderr")
		}
		out.Values = r.Values
		out.Rest = r.Rest
	}
	if listing != usage && r.Outcome != "parsed" {
		if !strings.HasPrefix(listing, "Usage of "+program+":\n") {
			fail("unexpected usage listing: " + listing)
		}
		out.Usage = listing
	}
	return out
}

// row runs args in the child and through the emulation, checks that they agree and that
// flag.Parse wrote what the row's fields say, and records the child's result.
func row(flags []*flag.Flag, args []string, usage string, asHex bool) matrixRow {
	r, exit := runChild(args)
	if !sameResult(r, emulate(flags, args)) {
		fail("the emulation and the child disagree on " + strings.Join(args, " "))
	}
	out := matrixRow{Args: args, Outcome: r.Outcome}
	switch r.Outcome {
	case "help":
		if r.Stderr != usage {
			fail("help wrote more than the usage listing")
		}
		out.Exit = &exit
	case "error":
		line, after, ok := strings.Cut(r.Stderr, "\n")
		if !ok || after != usage {
			fail("an error wrote more than its line and the usage listing")
		}
		out.Exit = &exit
		out.ErrorLine = line
	case "parsed":
		if r.Stderr != "" {
			fail("a parse wrote to stderr")
		}
		out.Values = r.Values
		out.Rest = r.Rest
	}
	if asHex {
		out.Args = hexAll(args)
		out.Rest = hexAll(out.Rest)
		if out.Values != nil {
			values := map[string]string{}
			for name, value := range out.Values {
				values[name] = hex.EncodeToString([]byte(value))
			}
			out.Values = values
		}
	}
	return out
}

func hexAll(ss []string) []string {
	if ss == nil {
		return nil
	}
	out := []string{}
	for _, s := range ss {
		out = append(out, hex.EncodeToString([]byte(s)))
	}
	return out
}
