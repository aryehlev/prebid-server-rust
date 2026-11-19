package main

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
)

// The seeded corpus of tests/base64.rs: 50,000 strings decoded the way go-gdpr decodes a consent
// string, base64.RawURLEncoding.Decode in place (vendorconsent/tcf2/consent.go:27-33 and
// vendorconsent/tcf1/consent.go:16-22), and the way adapters/flipp does,
// RawURLEncoding.DecodeString (adapters/flipp/flipp.go:291), which must agree. Both sides
// rebuild the strings from splitmix64 (tests/base64.rs implements it identically), so
// base64_seeded_goldens.json holds digests only: per batch, one of the inputs (which catches a
// generator that drifted) and one of the results, with counts that show what the batch covers.

const (
	seededSeed  = 0x4236_344c_454e_3031 // "B64LEN01"
	seededCount = 50000
	seededBatch = 1000
)

// seededAlphabet is the alphabet of base64.RawURLEncoding.
const seededAlphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

// seededSpecials are the pieces a noisy string mixes in: the line breaks Go skips, padding the
// encoding does not have, the standard alphabet's digits, separators, control and non-ASCII
// bytes, and one valid two-byte UTF-8 character.
var seededSpecials = []string{
	"\n", "\r", "\r\n", "=", "==", "+", "/", ".", "~", " ", "\t", "\x00", "\x7f", "\x80", "\xff",
	"\xc3\xa9",
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
// 300 (past Go's 8-byte and the crate's 32-byte fast paths), and noise-free, lightly or heavily
// noisy.
func (s *splitmix64) b64Input() []byte {
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
	var out []byte
	for i := 0; i < n; i++ {
		if noise > 0 && s.below(noise) == 0 {
			out = append(out, seededSpecials[s.below(len(seededSpecials))]...)
		} else {
			out = append(out, seededAlphabet[s.below(len(seededAlphabet))])
		}
	}
	return out
}

// seededResult is go-gdpr's in-place decode of input: "=" and the hex of the bytes, or "!" and
// the error text. It panics when DecodeString disagrees.
func seededResult(input []byte) string {
	buf := append([]byte(nil), input...)
	n, err := base64.RawURLEncoding.Decode(buf, buf)
	viaString, errString := base64.RawURLEncoding.DecodeString(string(input))
	if (err == nil) != (errString == nil) || (err != nil && err.Error() != errString.Error()) ||
		(err == nil && !bytes.Equal(buf[:n], viaString)) {
		panic(fmt.Sprintf("%q: Decode in place and DecodeString disagree", input))
	}
	if err != nil {
		return "!" + err.Error()
	}
	return "=" + hex.EncodeToString(buf[:n])
}

// trailingBits reports whether input, which decoded, ends in a digit whose unused low bits are
// not zero (Go's encodings are not strict, so it accepts them).
func trailingBits(input []byte) bool {
	var digits []byte
	for _, c := range input {
		if c != '\r' && c != '\n' {
			digits = append(digits, c)
		}
	}
	unused := map[int]uint{2: 4, 3: 2}[len(digits)%4]
	if unused == 0 {
		return false
	}
	last := bytes.IndexByte([]byte(seededAlphabet), digits[len(digits)-1])
	return last&(1<<unused-1) != 0
}

type seededBatchDigest struct {
	Inputs   string `json:"inputs_sha256"`
	Results  string `json:"results_sha256"`
	Accepted int    `json:"accepted"`
	// Accepted strings that hold a CR or LF, which Go skips.
	AcceptedNewlines int `json:"accepted_newlines"`
	// Accepted strings whose last digit has unused bits set.
	AcceptedTrailingBits int `json:"accepted_trailing_bits"`
}

type seededDoc struct {
	Header  header              `json:"header"`
	Seed    string              `json:"seed"`
	Count   int                 `json:"count"`
	Batch   int                 `json:"batch"`
	Batches []seededBatchDigest `json:"batches"`
}

func buildSeededDoc(hdr header) seededDoc {
	doc := seededDoc{Header: hdr, Seed: fmt.Sprintf("%#x", uint64(seededSeed)), Count: seededCount, Batch: seededBatch}
	gen := splitmix64{state: seededSeed}
	for start := 0; start < seededCount; start += seededBatch {
		inputs := sha256.New()
		results := sha256.New()
		var d seededBatchDigest
		for i := 0; i < seededBatch; i++ {
			input := gen.b64Input()
			result := seededResult(input)
			fmt.Fprintf(inputs, "%s\n", hex.EncodeToString(input))
			fmt.Fprintf(results, "%s\n", result)
			if result[0] == '=' {
				d.Accepted++
				if bytes.ContainsAny(input, "\r\n") {
					d.AcceptedNewlines++
				}
				if trailingBits(input) {
					d.AcceptedTrailingBits++
				}
			}
		}
		d.Inputs = hex.EncodeToString(inputs.Sum(nil))
		d.Results = hex.EncodeToString(results.Sum(nil))
		doc.Batches = append(doc.Batches, d)
	}
	return doc
}

// writeSeeded writes the document with one batch per line, like writeCases.
func writeSeeded(path string, doc seededDoc) {
	f, err := os.Create(path)
	if err != nil {
		fail(err)
	}
	w := bufio.NewWriter(f)
	h, err := json.Marshal(doc.Header)
	if err != nil {
		fail(err)
	}
	fmt.Fprintf(w, "{\"header\":%s,\n\"seed\":%q,\"count\":%d,\"batch\":%d,\n\"batches\":[\n", h, doc.Seed, doc.Count, doc.Batch)
	for i, b := range doc.Batches {
		line, err := json.Marshal(b)
		if err != nil {
			fail(err)
		}
		sep := ","
		if i == len(doc.Batches)-1 {
			sep = ""
		}
		fmt.Fprintf(w, "%s%s\n", line, sep)
	}
	fmt.Fprint(w, "]}\n")
	if err := w.Flush(); err != nil {
		fail(err)
	}
	if err := f.Close(); err != nil {
		fail(err)
	}
	fmt.Fprintf(os.Stderr, "wrote %d batches of %d to %s\n", len(doc.Batches), doc.Batch, path)
}
