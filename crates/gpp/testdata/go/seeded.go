package main

// base64_seeded.txt: the seeded corpus of tests/base64.rs, 50,000 strings decoded by
// util.NewBitStreamFromBase64, which pads an incomplete last quantum with "A" (counting every
// byte, line breaks included) and then calls base64.RawURLEncoding.Decode. Both sides rebuild the
// strings from splitmix64 (tests/base64.rs implements it identically), so the file holds digests
// only:
//
//	seed \t <seed> \t count \t <strings> \t batch \t <strings per batch>
//	<batch> \t <sha256 of the inputs> \t <sha256 of the results> \t <accepted> \t <accepted with CR or LF>
//
// The inputs are hashed as one hex line per string, the results as one line per string: "=" and
// the hex of the decoded bytes, or "!" and the error text. Every string is valid UTF-8, since
// the Rust function takes a &str.

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"

	"github.com/prebid/go-gpp/util"
)

const (
	seededSeed  = 0x4750_5042_3634_3031 // "GPPB6401"
	seededCount = 50000
	seededBatch = 1000
)

// seededAlphabet is the alphabet of base64.RawURLEncoding.
const seededAlphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

// seededSpecials are the pieces a noisy string mixes in: the line breaks Go skips, padding the
// encoding does not have, the standard alphabet's digits, GPP's separators, control bytes and
// one two-byte UTF-8 character.
var seededSpecials = []string{
	"\n", "\r", "\r\n", "=", "==", "+", "/", ".", "~", " ", "\t", "\x00", "\x7f", "é",
}

// splitmix64 is the generator shared with the Rust test.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

// below returns a number in [0, n).
func (s *splitmix64) below(n int) int {
	return int(s.next() % uint64(n))
}

// b64Input returns the next string of the corpus: mostly up to 40 pieces, some tiny, some up to
// 300, and noise-free, lightly or heavily noisy.
func (s *splitmix64) b64Input() string {
	var n int
	switch s.below(8) {
	case 0:
		n = s.below(301)
	case 1:
		n = s.below(5)
	default:
		n = s.below(41)
	}
	noise := [...]int{0, 0, 64, 16, 4}[s.below(5)]
	var b strings.Builder
	for i := 0; i < n; i++ {
		if noise > 0 && s.below(noise) == 0 {
			b.WriteString(seededSpecials[s.below(len(seededSpecials))])
		} else {
			b.WriteByte(seededAlphabet[s.below(len(seededAlphabet))])
		}
	}
	return b.String()
}

func seededGolden() []string {
	lines := []string{fmt.Sprintf("seed\t%#x\tcount\t%d\tbatch\t%d", uint64(seededSeed), seededCount, seededBatch)}
	gen := splitmix64{state: seededSeed}
	for batch := 0; batch*seededBatch < seededCount; batch++ {
		inputs := sha256.New()
		results := sha256.New()
		accepted, newlines := 0, 0
		for i := 0; i < seededBatch; i++ {
			input := gen.b64Input()
			fmt.Fprintf(inputs, "%s\n", hex.EncodeToString([]byte(input)))
			bs, err := util.NewBitStreamFromBase64(input)
			if err != nil {
				fmt.Fprintf(results, "!%s\n", err.Error())
				continue
			}
			fmt.Fprintf(results, "=%s\n", hex.EncodeToString(rawData(bs)))
			accepted++
			if strings.ContainsAny(input, "\r\n") {
				newlines++
			}
		}
		lines = append(lines, fmt.Sprintf("%d\t%x\t%x\t%d\t%d", batch, inputs.Sum(nil), results.Sum(nil), accepted, newlines))
	}
	return lines
}
