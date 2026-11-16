// Package dump holds the plumbing shared by the pbs-devtools Go dump programs: flag parsing, a
// clean environment, and deterministic JSON output.
//
// The programs are run by `pbs-devtools go-dump` from the repository root. Each one writes a single
// JSON golden into rust/testdata/ that Rust ports replay.
package dump

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Options are the flags every dump program accepts.
type Options struct {
	// Out is the file the JSON golden is written to.
	Out string
	// GoCommit is recorded in the golden so readers know which Go tree produced it.
	GoCommit string
	// Root is the repository root (the directory holding go.mod).
	Root string
}

// ParseFlags parses the shared flags plus any flags the caller registered on flag.CommandLine.
func ParseFlags() Options {
	var opts Options
	flag.StringVar(&opts.Out, "out", "", "file to write the JSON golden to")
	flag.StringVar(&opts.GoCommit, "go-commit", "", "Go commit recorded in the golden")
	flag.StringVar(&opts.Root, "root", ".", "repository root")
	flag.Parse()
	// glog (behind the logger package) writes log files to $TMPDIR by default; keep its output on
	// stderr, which pbs-devtools shows only when a dump fails.
	if flag.Lookup("logtostderr") != nil {
		if err := flag.Set("logtostderr", "true"); err != nil {
			Fatalf("setting -logtostderr: %v", err)
		}
	}
	if opts.Out == "" {
		Fatalf("missing -out")
	}
	if opts.GoCommit == "" {
		Fatalf("missing -go-commit")
	}
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		Fatalf("resolving -root: %v", err)
	}
	if _, err := os.Stat(filepath.Join(root, "go.mod")); err != nil {
		Fatalf("-root %s has no go.mod: %v", root, err)
	}
	opts.Root = root
	return opts
}

// ClearPBSEnv removes every PBS_* variable so viper's AutomaticEnv cannot leak the caller's
// environment into a golden.
func ClearPBSEnv() {
	for _, kv := range os.Environ() {
		name, _, _ := strings.Cut(kv, "=")
		if strings.HasPrefix(name, "PBS_") {
			if err := os.Unsetenv(name); err != nil {
				Fatalf("unsetting %s: %v", name, err)
			}
		}
	}
}

// Marshal encodes v as indented JSON with HTML escaping off and a trailing newline. Map keys are
// sorted by encoding/json, so the output is deterministic.
func Marshal(v interface{}) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// Raw encodes v with encoding/json (HTML escaping off) for embedding in a larger document.
func Raw(v interface{}) json.RawMessage {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		Fatalf("encoding %T: %v", v, err)
	}
	return json.RawMessage(bytes.TrimRight(buf.Bytes(), "\n"))
}

// WriteJSON writes v to path using Marshal.
func WriteJSON(path string, v interface{}) {
	data, err := Marshal(v)
	if err != nil {
		Fatalf("encoding %s: %v", path, err)
	}
	WriteFile(path, data)
}

// WriteFile writes data to path, creating parent directories.
func WriteFile(path string, data []byte) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		Fatalf("creating directory for %s: %v", path, err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		Fatalf("writing %s: %v", path, err)
	}
}

// Fatalf prints the message to stderr and exits with status 1.
func Fatalf(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, "go-dump: "+format+"\n", args...)
	os.Exit(1)
}
