// Command golden writes the goldens of the Rust pbs-govalidator crate from
// github.com/asaskevich/govalidator at the version Prebid Server requires.
//
// Run it from the directory of this module (crates/govalidator/testdata/go):
//
//	go run ./golden ../golden
//
// It reads the upstream checkout at $PBS_GO_ROOT (default: .upstream at the repository root),
// govalidator's own tests and the net/url, net and net/netip tests of the Go toolchain that runs
// it. The outputs:
//
//   - validator.txt: IsURL, IsRequestURL, IsRequestURI, IsIP and net.ParseIP of every input of
//     the corpus (see corpus.go), one quoted input and its outcome per line;
//   - pattern.txt: the pattern IsURL matches with (govalidator.URL), quoted;
//   - sweep.txt: the randomized differentials (see sweep.go);
//   - parts.txt: the parts sweep, inputs that join edge cases of each part of a URL (see
//     sweep.go).
package main

import (
	"encoding/hex"
	"log"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/asaskevich/govalidator"
)

func main() {
	if len(os.Args) != 2 {
		log.Fatal("usage: golden <output directory>")
	}
	out := os.Args[1]
	upstream := os.Getenv("PBS_GO_ROOT")
	if upstream == "" {
		upstream = "../../../../.upstream"
	}
	upstream, err := filepath.Abs(upstream)
	if err != nil {
		log.Fatal(err)
	}
	if err := os.MkdirAll(out, 0o755); err != nil {
		log.Fatal(err)
	}

	var lines []string
	for _, s := range corpus(upstream) {
		lines = append(lines, strconv.Quote(s)+"\t"+outcome(s))
	}
	writeLines(filepath.Join(out, "validator.txt"), lines)
	writeLines(filepath.Join(out, "pattern.txt"), []string{strconv.Quote(govalidator.URL)})
	writeLines(filepath.Join(out, "sweep.txt"), []string{urlSweep(), ipSweep()})
	writeLines(filepath.Join(out, "parts.txt"), partsSweep())
}

// outcome is what the Rust crate ports for one input: IsURL, IsRequestURL, IsRequestURI, IsIP,
// and net.ParseIP as its 16 bytes in hex or nil.
func outcome(s string) string {
	ip := "nil"
	if parsed := net.ParseIP(s); parsed != nil {
		ip = hex.EncodeToString(parsed)
	}
	return strings.Join([]string{
		strconv.FormatBool(govalidator.IsURL(s)),
		strconv.FormatBool(govalidator.IsRequestURL(s)),
		strconv.FormatBool(govalidator.IsRequestURI(s)),
		strconv.FormatBool(govalidator.IsIP(s)),
		ip,
	}, "\t")
}

func writeLines(path string, lines []string) {
	if err := os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
		log.Fatal(err)
	}
}

// goCmd runs the go command in this module and returns its trimmed output.
func goCmd(args ...string) string {
	b, err := exec.Command("go", args...).Output()
	if err != nil {
		log.Fatalf("go %s: %v", strings.Join(args, " "), err)
	}
	return strings.TrimSpace(string(b))
}
