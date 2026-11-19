package main

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
)

// splitmix64 is the generator the Rust tests implement identically (tests/seeded/mod.rs), so
// both sides rebuild the same seeded inputs.
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

// pick returns one element of list.
func (s *splitmix64) pick(list []string) string {
	return list[s.below(len(list))]
}

// seededCorpus is one seeded set: count cases in batches of batch, each case an input line and
// the outcome line Go gives it.
type seededCorpus struct {
	seed  uint64
	count int
	batch int
	next  func(*splitmix64) (input, outcome string)
}

// batchDigest is the digest pair of one batch.
type batchDigest struct {
	Inputs   string `json:"inputs_sha256"`
	Outcomes string `json:"outcomes_sha256"`
}

// seededDigests is what a golden records of a seeded set.
type seededDigests struct {
	Seed    string        `json:"seed"`
	Count   int           `json:"count"`
	Batch   int           `json:"batch"`
	Batches []batchDigest `json:"batches"`
}

// digests runs the set and records, per batch, the SHA-256 of the input lines and of the outcome
// lines, each line ending in a newline.
func (c seededCorpus) digests() seededDigests {
	d := seededDigests{Seed: fmt.Sprintf("%#x", c.seed), Count: c.count, Batch: c.batch}
	gen := splitmix64{state: c.seed}
	for start := 0; start < c.count; start += c.batch {
		inputs := sha256.New()
		outcomes := sha256.New()
		for i := 0; i < c.batch; i++ {
			input, outcome := c.next(&gen)
			fmt.Fprintf(inputs, "%s\n", input)
			fmt.Fprintf(outcomes, "%s\n", outcome)
		}
		d.Batches = append(d.Batches, batchDigest{
			Inputs:   hex.EncodeToString(inputs.Sum(nil)),
			Outcomes: hex.EncodeToString(outcomes.Sum(nil)),
		})
	}
	return d
}
