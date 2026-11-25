package main

import (
	"crypto/sha256"
	"encoding/hex"
	"flag"
	"fmt"
	"strconv"
	"strings"
)

// The seeded corpus: seededCount command lines from a splitmix64 seeded with seededSeed, in
// batches of seededBatch. The pools are written into the golden, and tests/flags.rs builds the
// same command lines from them with the same steps:
//
//	n := below(7)                                  // arguments in the command line
//	each argument: k := below(10)
//	  k < 6:  a flag: d := below(8) picks "-" (d < 5), "--" (d < 7) or "---";
//	          the name is names[below(len(names))] when below(4) < 3, else odd_names[...];
//	          below(3) == 0 appends "=" and values[below(len(values))]
//	  k < 9:  values[below(len(values))] on its own
//	  else:   specials[below(len(specials))]
const (
	seededSeed  = 20261125
	seededCount = 50000
	seededBatch = 1000
)

// seededPools are the pieces the command lines are made of.
type seededPools struct {
	Names    []string `json:"names"`
	OddNames []string `json:"odd_names"`
	Values   []string `json:"values"`
	Specials []string `json:"specials"`
}

var pools = seededPools{
	Names:    glogFlagNames,
	OddNames: []string{"h", "help", "x", "nope", "", "-v", "=x", "V", "Logtostderr", "é", "-", "="},
	Values: []string{
		"0", "1", "-1", "+5", "2", "3", "4", "10", "007", "08", "0x10", "0X1f", "0b101", "0o17",
		"1_0", "_1", "0_7", "4294967297", "2147483648", "-2147483649", "9223372036854775807",
		"9223372036854775808", "-9223372036854775809", "99999999999999999999", "", "x", "true",
		"false", "t", "f", "T", "F", "TRUE", "FALSE", "True", "False", "tRUE", "yes", "INFO",
		"info", "WARNING", "warning", "ERROR", "error", "FATAL", "fatal", "ınfo", "Warning",
		"256", "258", "259", "-129", "a=1", "a=1,b=2", "a", "a=", "=1", "a=1=2", ",", "a=1,",
		"gfs*=3", "foo/bar=1", "a=x", "a=4294967296", "a.go:1", "a.go:1,b.go:2", "a.go", "a:1",
		"a.go:-1", "a.go:x", "a.go:1:2", ",a.go:3,", "a.go:+7", "/tmp/pbs", "rel/dir", "é", "-h",
		"-x", "--", "-", "-v=1", "-logtostderr",
	},
	Specials: []string{"--", "-", "-h", "-help", "--help", "-h=1", "arg", "-help=x"},
}

// splitmix64 is the generator both sides share.
type splitmix64 struct{ state uint64 }

func (s *splitmix64) next() uint64 {
	s.state += 0x9E3779B97F4A7C15
	z := s.state
	z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
	z = (z ^ (z >> 27)) * 0x94D049BB133111EB
	return z ^ (z >> 31)
}

func (s *splitmix64) below(n int) int { return int(s.next() % uint64(n)) }

func (s *splitmix64) pick(from []string) string { return from[s.below(len(from))] }

// commandLine is the next seeded command line.
func (s *splitmix64) commandLine() []string {
	args := []string{}
	for n := s.below(7); n > 0; n-- {
		k := s.below(10)
		switch {
		case k < 6:
			dash := "---"
			switch d := s.below(8); {
			case d < 5:
				dash = "-"
			case d < 7:
				dash = "--"
			}
			var name string
			if s.below(4) < 3 {
				name = s.pick(pools.Names)
			} else {
				name = s.pick(pools.OddNames)
			}
			arg := dash + name
			if s.below(3) == 0 {
				arg += "=" + s.pick(pools.Values)
			}
			args = append(args, arg)
		case k < 9:
			args = append(args, s.pick(pools.Values))
		default:
			args = append(args, s.pick(pools.Specials))
		}
	}
	return args
}

type seededDoc struct {
	Header header      `json:"header"`
	Seed   uint64      `json:"seed"`
	Count  int         `json:"count"`
	Batch  int         `json:"batch"`
	Pools  seededPools `json:"pools"`
	// Encoding says how each line of a batch is written before it is hashed.
	Encoding map[string]string `json:"encoding"`
	// Outcomes counts the corpus's command lines by outcome.
	Outcomes map[string]int       `json:"outcomes"`
	Batches  []seededBatchDigests `json:"batches"`
}

type seededBatchDigests struct {
	Inputs     string `json:"inputs_sha256"`
	Results    string `json:"results_sha256"`
	ErrorLines string `json:"error_lines_sha256"`
}

var seededEncoding = map[string]string{
	"inputs":      "the number of arguments, ':', the arguments hex-encoded and joined by ',', '\\n'",
	"results":     "'parsed ', the nine values in names order hex-encoded and joined by ',', ' ', the number of arguments left, ':', those hex-encoded and joined by ',', '\\n'; or 'help\\n'; or 'error\\n'",
	"error_lines": "the error line hex-encoded, '\\n'; '\\n' when the command line did not fail",
}

func encodeList(ss []string) string {
	parts := make([]string, len(ss))
	for i, s := range ss {
		parts[i] = hex.EncodeToString([]byte(s))
	}
	return strconv.Itoa(len(ss)) + ":" + strings.Join(parts, ",")
}

func encodeResult(r result) string {
	switch r.Outcome {
	case "parsed":
		values := make([]string, len(glogFlagNames))
		for i, name := range glogFlagNames {
			values[i] = hex.EncodeToString([]byte(r.Values[name]))
		}
		return "parsed " + strings.Join(values, ",") + " " + encodeList(r.Rest) + "\n"
	case "help":
		return "help\n"
	}
	return "error\n"
}

// errorLine is the line an error writes before the usage listing, or "" for no error.
func errorLine(r result, usage string) string {
	switch r.Outcome {
	case "error":
		line, after, ok := strings.Cut(r.Stderr, "\n")
		if !ok || after != usage {
			fail("an error wrote more than its line and the usage listing")
		}
		return line
	case "help":
		if r.Stderr != usage {
			fail("help wrote more than the usage listing")
		}
	}
	return ""
}

func buildSeeded(h header) seededDoc {
	return seeded(h, glogFlags(true), true)
}

// seeded parses the corpus through the emulation; withChild checks every 50th command line
// against the child's flag.Parse as well.
func seeded(h header, flags []*flag.Flag, withChild bool) seededDoc {
	doc := seededDoc{
		Header: h, Seed: seededSeed, Count: seededCount, Batch: seededBatch, Pools: pools,
		Encoding: seededEncoding, Outcomes: map[string]int{},
	}
	usage := emulate(flags, []string{"-h"}).Stderr
	rng := splitmix64{state: seededSeed}
	inputs, results, lines := sha256.New(), sha256.New(), sha256.New()
	for i := 0; i < seededCount; i++ {
		args := rng.commandLine()
		r := emulate(flags, args)
		if withChild && i%50 == 0 {
			if c, _ := runChild(args); !sameResult(c, r) {
				fail(fmt.Sprintf("the emulation and the child disagree on seeded line %d: %q", i, args))
			}
		}
		doc.Outcomes[r.Outcome]++
		fmt.Fprint(inputs, encodeList(args)+"\n")
		fmt.Fprint(results, encodeResult(r))
		fmt.Fprint(lines, hex.EncodeToString([]byte(errorLine(r, usage)))+"\n")
		if (i+1)%seededBatch == 0 {
			doc.Batches = append(doc.Batches, seededBatchDigests{
				Inputs:     hex.EncodeToString(inputs.Sum(nil)),
				Results:    hex.EncodeToString(results.Sum(nil)),
				ErrorLines: hex.EncodeToString(lines.Sum(nil)),
			})
			inputs.Reset()
			results.Reset()
			lines.Reset()
		}
	}
	return doc
}
