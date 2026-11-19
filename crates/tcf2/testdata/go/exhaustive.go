package main

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
)

// The exhaustive corpus of tests/base64.rs (base64_exhaustive_goldens.json): every string of up
// to exhaustiveMaxLen bytes over exhaustiveSymbols, decoded as seededResult decodes it (go-gdpr's
// in-place RawURLEncoding.Decode, checked against DecodeString). The symbols are one byte of each
// kind the decoder tells apart: a digit whose low bits are zero, one whose low bits are all set
// (so a last digit with unused bits set), the two line breaks it skips, the padding byte the
// encoding does not have, a digit of the standard alphabet only, and a byte past ASCII. Every
// arrangement of them up to two quanta and a lone digit decodes here, which the seeded corpus
// reaches only by chance.
//
// The strings of one length are enumerated as base-7 numbers counting up, the first symbol the
// most significant digit; tests/base64.rs enumerates them the same way, so the file holds one
// record per length: the count, how many decode, and the SHA-256 of the inputs (one hex line per
// string) and of the results (one line per string, as seededResult writes it).

// exhaustiveSymbols are the bytes the strings are made of.
var exhaustiveSymbols = []byte{'A', '_', '\n', '\r', '=', '+', 0xc3}

// exhaustiveMaxLen is the longest string: a full quantum, a lone digit and two line breaks.
const exhaustiveMaxLen = 7

type exhaustiveLength struct {
	Len      int    `json:"len"`
	Count    int    `json:"count"`
	Accepted int    `json:"accepted"`
	Inputs   string `json:"inputs_sha256"`
	Results  string `json:"results_sha256"`
}

func buildExhaustiveCases() []exhaustiveLength {
	var out []exhaustiveLength
	for n := 0; n <= exhaustiveMaxLen; n++ {
		inputs := sha256.New()
		results := sha256.New()
		rec := exhaustiveLength{Len: n}
		digits := make([]int, n)
		input := make([]byte, n)
		for {
			for i, d := range digits {
				input[i] = exhaustiveSymbols[d]
			}
			result := seededResult(input)
			fmt.Fprintf(inputs, "%s\n", hex.EncodeToString(input))
			fmt.Fprintf(results, "%s\n", result)
			rec.Count++
			if result[0] == '=' {
				rec.Accepted++
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
		rec.Inputs = hex.EncodeToString(inputs.Sum(nil))
		rec.Results = hex.EncodeToString(results.Sum(nil))
		out = append(out, rec)
	}
	return out
}
