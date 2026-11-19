package main

// base64_exhaustive.txt: the exhaustive corpus of tests/base64.rs, every string of up to
// exhaustiveMaxLen symbols over exhaustiveSymbols, decoded by util.NewBitStreamFromBase64. The
// symbols are one of each kind the decoder tells apart: a digit whose low bits are zero, one
// whose low bits are all set (so a last digit with unused bits set), the two line breaks it skips
// (which padLastQuantum counts), the padding byte the encoding does not have, GPP's segment
// separator, and a two-byte character (which padLastQuantum counts twice). Every arrangement of
// them up to two quanta and a lone digit decodes here, which the seeded corpus reaches only by
// chance.
//
// The strings of one length are enumerated as base-7 numbers counting up, the first symbol the
// most significant digit; tests/base64.rs enumerates them the same way, so the file holds digests
// only:
//
//	max \t <longest string, in symbols> \t symbols \t <number of symbols>
//	<symbols> \t <strings> \t <accepted> \t <sha256 of the inputs> \t <sha256 of the results>
//
// The inputs and the results are hashed as base64_seeded.txt hashes them.

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"

	"github.com/prebid/go-gpp/util"
)

// exhaustiveSymbols are the pieces the strings are made of.
var exhaustiveSymbols = []string{"A", "_", "\n", "\r", "=", ".", "é"}

// exhaustiveMaxLen is the longest string, in symbols: a full quantum, a lone digit and two line
// breaks.
const exhaustiveMaxLen = 7

func exhaustiveGolden() []string {
	lines := []string{fmt.Sprintf("max\t%d\tsymbols\t%d", exhaustiveMaxLen, len(exhaustiveSymbols))}
	for n := 0; n <= exhaustiveMaxLen; n++ {
		inputs := sha256.New()
		results := sha256.New()
		count, accepted := 0, 0
		digits := make([]int, n)
		for {
			var b strings.Builder
			for _, d := range digits {
				b.WriteString(exhaustiveSymbols[d])
			}
			input := b.String()
			fmt.Fprintf(inputs, "%s\n", hex.EncodeToString([]byte(input)))
			count++
			if bs, err := util.NewBitStreamFromBase64(input); err != nil {
				fmt.Fprintf(results, "!%s\n", err.Error())
			} else {
				fmt.Fprintf(results, "=%s\n", hex.EncodeToString(rawData(bs)))
				accepted++
			}
			// The next number: the last digit counts up, carrying into the ones before it.
			i := n - 1
			for i >= 0 && digits[i] == len(exhaustiveSymbols)-1 {
				digits[i] = 0
				i--
			}
			if i < 0 {
				break
			}
			digits[i]++
		}
		lines = append(lines, fmt.Sprintf("%d\t%d\t%d\t%x\t%x", n, count, accepted, inputs.Sum(nil), results.Sum(nil)))
	}
	return lines
}
