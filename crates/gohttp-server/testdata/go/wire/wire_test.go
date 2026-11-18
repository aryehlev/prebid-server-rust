package main

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
)

// goldenExpect reads the expect blocks of the wire golden: by case name and then by GOOS, what
// Go answers. It also returns the GOOS the generator ran on.
func goldenExpect(text string) (map[string]map[string]result, string, error) {
	expect := map[string]map[string]result{}
	var generatedOn, name, goos string
	for no, line := range strings.Split(text, "\n") {
		key, rest, _ := strings.Cut(line, " ")
		switch key {
		case "goos":
			generatedOn = rest
		case "case":
			name, goos = rest, ""
		case "expect":
			if expect[name] == nil {
				expect[name] = map[string]result{}
			}
			if _, dup := expect[name][rest]; dup {
				return nil, "", fmt.Errorf("wire.golden:%d: case %q has two expect blocks for %s", no+1, name, rest)
			}
			goos = rest
			expect[name][goos] = result{}
		case "response":
			if goos == "" {
				continue
			}
			body, err := strconv.Unquote(rest)
			if err != nil {
				return nil, "", fmt.Errorf("wire.golden:%d: %w", no+1, err)
			}
			r := expect[name][goos]
			r.response = []byte(body)
			expect[name][goos] = r
		case "end":
			if goos == "" {
				continue
			}
			r := expect[name][goos]
			r.end = rest
			expect[name][goos] = r
		}
	}
	return expect, generatedOn, nil
}

// TestGOOSExpectations runs every case whose bytes depend on the GOOS against the real servers
// and checks that Go answers exactly what the wire golden's expect block for the GOOS the test
// runs on says. The generator observed the block of the GOOS it ran on and derived the others
// from Go's source (goos.go), so on linux (CI's go-tooling job runs this test there) this checks
// the derived linux blocks against real Go, and on darwin the darwin ones. It fails on a GOOS the
// golden has no expect block for.
func TestGOOSExpectations(t *testing.T) {
	text, err := os.ReadFile(defaultOut("wire.golden"))
	if err != nil {
		t.Fatal(err)
	}
	expect, generatedOn, err := goldenExpect(string(text))
	if err != nil {
		t.Fatal(err)
	}
	var cases []wireCase
	for _, c := range allCases() {
		if c.byGOOS {
			cases = append(cases, c)
		}
	}
	if len(cases) == 0 {
		t.Fatal("no case of the corpus depends on the GOOS")
	}
	if len(expect) != len(cases) {
		t.Fatalf("wire.golden has expect blocks for %d cases, the corpus marks %d: regenerate it", len(expect), len(cases))
	}

	dir, err := os.MkdirTemp("", "gohttp-wire")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(dir)
	static := filepath.Join(dir, "static")
	if err := writeStatic(static); err != nil {
		t.Fatal(err)
	}
	targets, stop, err := startServers(static, filepath.Join(dir, "prebid.sock"))
	if err != nil {
		t.Fatal(err)
	}
	defer stop()

	for _, c := range cases {
		blocks, ok := expect[c.name]
		if !ok {
			t.Errorf("case %q: wire.golden has no expect blocks for it: regenerate it", c.name)
			continue
		}
		want, ok := blocks[runtime.GOOS]
		if !ok {
			t.Errorf("case %q: wire.golden (generated on %s) has no expect block for GOOS %s", c.name, generatedOn, runtime.GOOS)
			continue
		}
		got := runCase(targets[c.server], c)
		if !sameResult(got, want) {
			t.Errorf("case %q on %s:\ngot  %q %s\nwant %q %s", c.name, runtime.GOOS, got.response, got.end, want.response, want.end)
		}
	}
}
