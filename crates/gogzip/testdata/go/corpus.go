package main

import (
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// The corpus is named by input specs that the Rust tests parse and regenerate byte for byte
// (src/compress/testutil.rs mirrors every generator here):
//
//	lit:HEX            the bytes HEX
//	zeros:N            N zero bytes
//	rand:N:SEED        N bytes of splitmix64 output
//	repeat:P:N:SEED    rand:P:SEED repeated to N bytes
//	low:K:N:SEED       N bytes, each next() % K
//	text:N:SEED        N bytes of words from vocabulary
//	json:N:SEED        N bytes of OpenRTB-like JSON lines
//	mixed:N:SEED       N bytes of zero runs, random bytes, back references and text
//	abc:N              byte(i % 128) for i < N (Go's TestBestSpeed input)
//	seq:N              byte(i) for i < N (Go's TestReaderEarlyEOF input)
//	square:N           byte(i * i) for i < N (Go's TestDeflate largeDataChunk)
//	file:NAME[:N]      ../corpus/NAME, or its first N bytes
//	A+B+...            the concatenation of the specs A, B, ...

// splitmix64 is the generator behind every random input.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// vocabulary is the word list of text inputs, most frequent first.
var vocabulary = strings.Fields("the bid request response imp banner video native site app " +
	"device user geo ext price floor currency USD seat bidder adapter timeout cache targeting " +
	"hb_pb hb_bidder deal id width height prebid server auction quick brown fox jumps over " +
	"lazy dog privacy consent gdpr ccpa gpp schain supply chain")

// domains are the site domains of json inputs.
var domains = []string{"example", "prebid", "news", "sports", "weather", "shop"}

func genRand(n int, seed uint64) []byte {
	r := &splitmix64{seed}
	b := make([]byte, n)
	for i := 0; i < n; i += 8 {
		v := r.next()
		for j := 0; j < 8 && i+j < n; j++ {
			b[i+j] = byte(v >> (8 * j))
		}
	}
	return b
}

func genText(n int, seed uint64) []byte {
	r := &splitmix64{seed}
	v := uint64(len(vocabulary))
	b := make([]byte, 0, n+32)
	for len(b) < n {
		x := r.next() % v
		y := r.next() % v
		b = append(b, vocabulary[x*y/v]...)
		switch r.next() % 16 {
		case 0:
			b = append(b, ".\n"...)
		case 1:
			b = append(b, ", "...)
		default:
			b = append(b, ' ')
		}
	}
	return b[:n]
}

func genJSON(n int, seed uint64) []byte {
	r := &splitmix64{seed}
	b := make([]byte, 0, n+512)
	for i := 0; len(b) < n; i++ {
		id := r.next()
		w := 100 * (1 + r.next()%8)
		h := 50 * (1 + r.next()%12)
		floor := r.next() % 1000
		dom := domains[r.next()%uint64(len(domains))]
		page := r.next() % 100000
		tmax := 100 + r.next()%900
		b = fmt.Appendf(b, `{"id":"%016x","imp":[{"id":"%d","banner":{"w":%d,"h":%d},"bidfloor":%d.%02d}],`+
			`"site":{"domain":"%s.com","page":"https://%s.com/p/%d"},"tmax":%d}`+"\n",
			id, i, w, h, floor/100, floor%100, dom, dom, page, tmax)
	}
	return b[:n]
}

func genMixed(n int, seed uint64) []byte {
	r := &splitmix64{seed}
	b := make([]byte, 0, n+4096)
	for len(b) < n {
		kind := r.next() % 5
		length := 1 + int(r.next()%3000)
		switch kind {
		case 0:
			b = append(b, make([]byte, length)...)
		case 1:
			for j := 0; j < length; j++ {
				b = append(b, byte(r.next()))
			}
		case 2:
			// A back reference, possibly beyond the 32 KiB window.
			if len(b) == 0 {
				continue
			}
			d := 1 + int(r.next()%uint64(min(len(b), 40000)))
			start := len(b) - d
			for j := 0; j < length; j++ {
				b = append(b, b[start+j])
			}
		case 3:
			b = append(b, genText(length, r.next())...)
		default:
			c := byte(r.next())
			for j := 0; j < length; j++ {
				b = append(b, c)
			}
		}
	}
	return b[:n]
}

func atoi(s string) int {
	n, err := strconv.Atoi(s)
	if err != nil {
		panic(err)
	}
	return n
}

func atou(s string) uint64 {
	n, err := strconv.ParseUint(s, 10, 64)
	if err != nil {
		panic(err)
	}
	return n
}

// corpusDir is ../corpus, set by main.
var corpusDir string

// genInput returns the bytes of an input spec.
func genInput(spec string) []byte {
	if strings.Contains(spec, "+") {
		var b []byte
		for _, part := range strings.Split(spec, "+") {
			b = append(b, genInput(part)...)
		}
		return b
	}
	f := strings.Split(spec, ":")
	switch f[0] {
	case "lit":
		b, err := hex.DecodeString(f[1])
		if err != nil {
			panic(err)
		}
		return b
	case "zeros":
		return make([]byte, atoi(f[1]))
	case "rand":
		return genRand(atoi(f[1]), atou(f[2]))
	case "repeat":
		p := genRand(atoi(f[1]), atou(f[3]))
		n := atoi(f[2])
		b := make([]byte, n)
		for i := range b {
			b[i] = p[i%len(p)]
		}
		return b
	case "low":
		k := atou(f[1])
		r := &splitmix64{atou(f[3])}
		b := make([]byte, atoi(f[2]))
		for i := range b {
			b[i] = byte(r.next() % k)
		}
		return b
	case "text":
		return genText(atoi(f[1]), atou(f[2]))
	case "json":
		return genJSON(atoi(f[1]), atou(f[2]))
	case "mixed":
		return genMixed(atoi(f[1]), atou(f[2]))
	case "abc":
		b := make([]byte, atoi(f[1]))
		for i := range b {
			b[i] = byte(i % 128)
		}
		return b
	case "seq":
		b := make([]byte, atoi(f[1]))
		for i := range b {
			b[i] = byte(i)
		}
		return b
	case "square":
		b := make([]byte, atoi(f[1]))
		for i := range b {
			b[i] = byte(i * i)
		}
		return b
	case "file":
		b, err := os.ReadFile(filepath.Join(corpusDir, f[1]))
		if err != nil {
			panic(err)
		}
		if len(f) > 2 {
			b = b[:atoi(f[2])]
		}
		return b
	}
	panic("unknown input spec " + spec)
}
