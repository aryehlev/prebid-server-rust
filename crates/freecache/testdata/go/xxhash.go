package main

import (
	"fmt"
	"strconv"

	"github.com/cespare/xxhash/v2"
)

// xxhashSeed seeds the generator of the Sum64 inputs.
const xxhashSeed = 0x7878686173680001

// xxhashLengths are the input lengths: every length up to 160 (all tail paths, one to five
// 32-byte blocks), a spread up to a few KiB, and the key length limits.
func xxhashLengths() []int {
	var lengths []int
	for n := 0; n <= 160; n++ {
		lengths = append(lengths, n)
	}
	for n := 161; n <= 5000; n += 97 {
		lengths = append(lengths, n)
	}
	return append(lengths, 65535, 65536)
}

// writeXxhash records Sum64 of one splitmix64-filled input per length, generated in order
// from a single stream.
func writeXxhash(g *golden) {
	defer g.close()
	rng := splitmix64{state: xxhashSeed}
	for _, n := range xxhashLengths() {
		b := make([]byte, n)
		rng.fill(b)
		g.line("xxhash", strconv.Itoa(n), fmt.Sprintf("%016x", xxhash.Sum64(b)))
	}
}
