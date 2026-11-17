// Command freecachegolden writes the golden files that the pbs-freecache Rust tests replay.
//
// It drives the real github.com/coocood/freecache and github.com/cespare/xxhash/v2 at the
// versions the repository go.mod pins (v1.2.1 and v2.3.0). Run it from the repository root:
//
//	go run ./rust/crates/freecache/testdata/go
//
// It writes, next to this directory under ../golden:
//
//   - ringbuf.golden: the RingBuf state after each step of TestRingBuf and a few more calls
//     (ringbuf.go);
//   - xxhash.golden: Sum64 of splitmix64-generated inputs of many lengths (xxhash.go);
//   - scripts.golden: long random operation scripts against freecache on a fake clock. The
//     Rust tests regenerate the same operations from splitmix64 and compare, per batch, digests
//     of the operations, of every result and counter, and of the whole cache state read with
//     reflect (scripts.go);
//   - scenarios.golden: freecache's own tests replayed on a fake clock, with the counters they
//     only log (scenarios.go);
//   - limits.golden: the longest value Set accepts for many cache sizes and key lengths
//     (adversarial.go);
//   - ringfuzz.golden: random RingBuf call sequences, digested call by call (adversarial.go).
//
// scripts.golden also holds the adversarial scripts of adversarial.go: keys that all share one
// segment and slot, a clock that advances on its own as the cache reads it, and iteration
// interleaved with writes.
//
// To debug a mismatch, print one script's per-operation records and compare them with the lines
// the Rust test reports:
//
//	go run ./rust/crates/freecache/testdata/go -trace small_evict -from 12000 -to 13000
package main

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// golden writes lines to one golden file.
type golden struct {
	f *os.File
	w *bufio.Writer
}

func newGolden(dir, name string) *golden {
	f, err := os.Create(filepath.Join(dir, name))
	if err != nil {
		panic(err)
	}
	return &golden{f: f, w: bufio.NewWriter(f)}
}

func (g *golden) line(fields ...string) {
	if _, err := g.w.WriteString(strings.Join(fields, "\t") + "\n"); err != nil {
		panic(err)
	}
}

func (g *golden) close() {
	if err := g.w.Flush(); err != nil {
		panic(err)
	}
	if err := g.f.Close(); err != nil {
		panic(err)
	}
}

// splitmix64 is the generator shared with the Rust tests (src/freecache/golden.rs).
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// below returns next() % n, or 0 when n is 0 (next() is drawn either way).
func (s *splitmix64) below(n uint64) uint64 {
	v := s.next()
	if n == 0 {
		return 0
	}
	return v % n
}

// fill fills b with bytes taken little-endian from successive next() values.
func (s *splitmix64) fill(b []byte) {
	var w uint64
	for i := range b {
		if i%8 == 0 {
			w = s.next()
		}
		b[i] = byte(w)
		w >>= 8
	}
}

// fnv is FNV-1a 64, extended with a word-at-a-time mode for large buffers.
type fnv struct{ h uint64 }

const (
	fnvOffset = 14695981039346656037
	fnvPrime  = 1099511628211
)

func newFnv() *fnv { return &fnv{h: fnvOffset} }

func (f *fnv) bytes(b []byte) {
	for _, c := range b {
		f.h ^= uint64(c)
		f.h *= fnvPrime
	}
}

func (f *fnv) str(s string) { f.bytes([]byte(s)) }

func (f *fnv) i64(v int64) {
	u := uint64(v)
	for i := 0; i < 8; i++ {
		f.h ^= u & 0xFF
		f.h *= fnvPrime
		u >>= 8
	}
}

// words mixes b eight little-endian bytes at a time, then the tail byte by byte.
func (f *fnv) words(b []byte) {
	for len(b) >= 8 {
		w := uint64(b[0]) | uint64(b[1])<<8 | uint64(b[2])<<16 | uint64(b[3])<<24 |
			uint64(b[4])<<32 | uint64(b[5])<<40 | uint64(b[6])<<48 | uint64(b[7])<<56
		f.h ^= w
		f.h *= fnvPrime
		b = b[8:]
	}
	f.bytes(b)
}

func (f *fnv) hex() string { return fmt.Sprintf("%016x", f.h) }

// bytesDigest is the FNV-1a 64 of b, as 16 hex digits.
func bytesDigest(b []byte) string {
	f := newFnv()
	f.bytes(b)
	return f.hex()
}

func main() {
	trace := flag.String("trace", "", "print the per-operation records of this script instead")
	from := flag.Int("from", 0, "first operation to print with -trace")
	to := flag.Int("to", -1, "operation to stop before with -trace (-1: the end)")
	flag.Parse()

	if *trace != "" {
		traceScript(*trace, *from, *to)
		return
	}

	_, file, _, ok := runtime.Caller(0)
	if !ok {
		panic("cannot locate the generator source")
	}
	dir := filepath.Join(filepath.Dir(file), "..", "golden")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		panic(err)
	}

	writeRingBuf(newGolden(dir, "ringbuf.golden"))
	writeXxhash(newGolden(dir, "xxhash.golden"))
	writeScripts(newGolden(dir, "scripts.golden"))
	writeScenarios(newGolden(dir, "scenarios.golden"))
	writeLimits(newGolden(dir, "limits.golden"))
	writeRingFuzz(newGolden(dir, "ringfuzz.golden"))
}
