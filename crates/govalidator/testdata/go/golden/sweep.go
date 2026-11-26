package main

import (
	"crypto/sha256"
	"encoding/hex"
	"hash"
	"hash/fnv"
	"strconv"
	"strings"
)

// The randomized differentials draw their inputs from splitmix64, which the Rust tests
// implement identically. A sweep.txt line holds the sweep's name, seed and count, two SHA-256
// digests and the tokens, each quoted in a field of its own. The digests are of the inputs,
// which catches a generator that drifted, and of the inputs with Go's outcomes (outcome, as in
// validator.txt). The Rust tests rebuild the inputs, compute their own outcomes and compare.

// sweepCount is the number of inputs of a sweep.
const sweepCount = 50000

// maxTokens is the most tokens an input is made of.
const maxTokens = 12

type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *splitmix64) below(n int) int {
	return int(s.next() % uint64(n))
}

// input concatenates up to max tokens, each chosen uniformly.
func (s *splitmix64) input(tokens []string, max int) string {
	n := s.below(max + 1)
	var b strings.Builder
	for i := 0; i < n; i++ {
		b.WriteString(tokens[s.below(len(tokens))])
	}
	return b.String()
}

func seedOf(s string) uint64 {
	h := fnv.New64a()
	h.Write([]byte(s))
	return h.Sum64()
}

type digests struct {
	in, out hash.Hash
}

func newDigests() digests {
	return digests{sha256.New(), sha256.New()}
}

func (d digests) line(in, out string) {
	d.in.Write([]byte(in + "\n"))
	d.out.Write([]byte(in + "\t" + out + "\n"))
}

// urlTokens build URL-like inputs: schemes, separators, user info, hosts, IP literals, ports,
// escapes (valid and not), spaces, multibyte text and template actions.
var urlTokens = []string{
	"http", "https", "ftp", "wss", "udp", "mailto", "://", ":", "/", "//", "?", "#", "&", "=", "@",
	"user", "pass", "www.", "example", ".", "com", "co.uk", "-", "_", "~", "%", "%20", "%2F",
	"%zz", "%25", "[", "]", "::1", "1.2.3.4", "255", "256", "0", "8080", "65536", "localhost",
	"a", "Z", "9", " ", "\t", "é", "日本", "xn--", "{{.Host}}", "{", "}", "*", "\\", "..", "\"",
	"<", "|", "^", "`", "+", ";", "!", "$", "(", "'", "fe80::1",
}

// ipTokens build IP-like inputs: decimal and hex fields with and without leading zeros or
// overflow, separators, zones, an IPv4-mapped prefix and characters no address has.
var ipTokens = []string{
	"0", "1", "9", "00", "01", "10", "25", "99", "255", "256", "1000", "a", "f", "F", "g", "ff",
	"ffff", "fffff", "12345", "abcd", "ABCD", ".", ".", ".", ":", ":", "::", "%", "eth0",
	"1.2.3.4", "::ffff:", " ", "x", "[", "]", "/", "-", "é", "٣",
}

// sweep is the sweep.txt line of one sweep: the outcome of sweepCount inputs built from tokens.
func sweep(name string, tokens []string) string {
	seed := seedOf(name)
	rng := splitmix64{seed}
	d := newDigests()
	for i := 0; i < sweepCount; i++ {
		s := rng.input(tokens, maxTokens)
		d.line(strconv.Quote(s), outcome(s))
	}
	fields := []string{name, strconv.FormatUint(seed, 10), strconv.Itoa(sweepCount),
		hex.EncodeToString(d.in.Sum(nil)), hex.EncodeToString(d.out.Sum(nil))}
	for _, t := range tokens {
		fields = append(fields, strconv.Quote(t))
	}
	return strings.Join(fields, "\t")
}

func urlSweep() string { return sweep("url", urlTokens) }

func ipSweep() string { return sweep("ip", ipTokens) }

// partsName names the parts sweep, whose seed is the FNV-64a hash of the name, as a token
// sweep's is of its own.
const partsName = "parts"

// urlParts are the parts a parts-sweep input joins, in this order: a scheme, user info, a host,
// a port, and a path, query or fragment. Each holds edge cases of its part for the branches of
// url.Parse and url.ParseRequestURI (an opaque or rootless rest, an authority, the last '@', a
// bracketed IPv6 host with and without a zone, the port after the last colon, the escapes a
// host, zone, user info, path or fragment may and may not hold, control bytes) and for the
// parts of govalidator's pattern. The token sweeps rarely build such a part whole.
var urlParts = [][]string{
	{
		"", "http://", "https://", "HTTP://", "hTTps://", "ftp://", "ws://", "wss://", "tcp://",
		"udp://", "foo://", "h-t.t+p://", "1http://", "+x://", "http:", "http:/", "http:///",
		"//", "///", "mailto:", "a:", "x.y:",
	},
	{
		"", "u@", "u:p@", "u:p@q@", "u%41@", "u%zz@", "u%4@", "u%@", "u p@", "@", ":@", "::@",
		"ü@", "u:p%3A@", "u!$&'()*+,;=@", "u~-._@", "u[@", "u]@", "u\"@", "u<@",
	},
	{
		"a.com", "localhost", "127.0.0.1", "1.2.3.4", "256.1.1.1", "1.2.3", "", "a.", "[]",
		"[::1]", "[::1%25en0]", "[fe80::1%25en0]", "[fe80::1%en0]", "[1.2.3.4]",
		"[::ffff:1.2.3.4]", "[::ffff:01.2.3.4]", "[::1", "::1]", "a[b]", "[::1]]", "[::1]:80",
		"[%25en0]", "[::1%25]", "[::1%25%20x]", "[::1%25%C3%A9]", "[::1%25ab%41]",
		"[::1%25ab%2F]", "[::1%25a~b]", "[::1%25a%25b]", "[v1.x]", "[1::2::3]", "[0001::]",
		"[00001::]", "[1:2:3:4:5:6:7::]", "[1:2:3:4:5:6:7:8::]", "[::1:2:3:4:5:6:7:8]",
		"[1:2:3:4:5:6:1.2.3.4]", "[1:2:3:4:5:1.2.3.4]", "[%3A%3A1]", "[::%31]", "%5B::1%5D",
		"%41.com", "%C3%A9.com", "%c3%a9.com", "ü.com", "a_b.com", "a-.com", "-a.com", ".a.com",
		"a..com", "a.com.", "a%25.com", "a%2.com", "a b.com", "a!$&'()*+,;=.com", "<a>.com",
		"a\"b.com", "a{b}.com", "xn--bcher-kva.ch", "a:b:c",
	},
	{"", ":", ":80", ":080", ":65535", ":65536", ":99999", ":8a", "::80", ":80:90", ": 80", ":%38"},
	{
		"", "/", "/a", "/a b", "/%zz", "/%41", "/a:b", "/a?b", "/#", "/a#%zz", "/a#%41", "?q",
		"?%zz", "#f", "#%zz", "/ü", "/\\", "/a/b.c", "/.", "?", "/?#", "#", "/%", "/a%2", "/\x7f",
		"/\t",
	},
}

// partsSweep is parts.txt: a line with the parts sweep's name, seed and count and the two
// digests, as a sweep.txt line starts, then a line for each part with its tokens, each quoted
// in a field of its own. Each of the sweepCount inputs joins one token of every part, chosen
// uniformly, in the order of urlParts.
func partsSweep() []string {
	seed := seedOf(partsName)
	rng := splitmix64{seed}
	d := newDigests()
	for i := 0; i < sweepCount; i++ {
		var b strings.Builder
		for _, part := range urlParts {
			b.WriteString(part[rng.below(len(part))])
		}
		s := b.String()
		d.line(strconv.Quote(s), outcome(s))
	}
	lines := []string{strings.Join([]string{partsName, strconv.FormatUint(seed, 10),
		strconv.Itoa(sweepCount), hex.EncodeToString(d.in.Sum(nil)),
		hex.EncodeToString(d.out.Sum(nil))}, "\t")}
	for _, part := range urlParts {
		quoted := make([]string, len(part))
		for i, t := range part {
			quoted[i] = strconv.Quote(t)
		}
		lines = append(lines, strings.Join(quoted, "\t"))
	}
	return lines
}
