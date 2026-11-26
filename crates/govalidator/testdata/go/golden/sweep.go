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
