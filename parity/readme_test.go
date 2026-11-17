package parity

import (
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
)

// codeSpan matches a Markdown code span.
var codeSpan = regexp.MustCompile("`([^`]+)`")

// TestContentsNamesEveryPath: the Contents table of README.md names every
// directory of parity/ down to two levels and every file at its top level, so
// a reader finds each part of the harness there, and every path the table
// names exists.
func TestContentsNamesEveryPath(t *testing.T) {
	var named []string
	for _, p := range contentsPaths(t) {
		for _, alt := range alternatives(p) {
			if !exists(t, alt) {
				t.Errorf("the Contents table names %s, which does not exist", alt)
			}
			named = append(named, alt)
		}
	}
	for _, want := range harnessPaths(t) {
		if !slices.ContainsFunc(named, func(p string) bool { return covers(p, want) }) {
			t.Errorf("the Contents table of README.md does not name %s", want)
		}
	}
}

// contentsPaths returns the code spans of the first column of the Contents
// table: a directory with a trailing slash, a file, or a glob pattern.
func contentsPaths(t *testing.T) []string {
	t.Helper()
	raw, err := os.ReadFile("README.md")
	if err != nil {
		t.Fatal(err)
	}
	_, section, ok := strings.Cut(string(raw), "\n## Contents\n")
	if !ok {
		t.Fatal("README.md has no Contents section")
	}
	section, _, _ = strings.Cut(section, "\n## ")
	var paths []string
	for _, line := range strings.Split(section, "\n") {
		cells := strings.Split(line, "|")
		if len(cells) < 3 || strings.TrimSpace(cells[0]) != "" {
			continue
		}
		for _, m := range codeSpan.FindAllStringSubmatch(cells[1], -1) {
			paths = append(paths, m[1])
		}
	}
	if len(paths) == 0 {
		t.Fatal("the Contents table names no path")
	}
	return paths
}

// alternatives expands one {a,b,...} alternation in p.
func alternatives(p string) []string {
	head, rest, ok := strings.Cut(p, "{")
	if !ok {
		return []string{p}
	}
	list, tail, ok := strings.Cut(rest, "}")
	if !ok {
		return []string{p}
	}
	var out []string
	for _, alt := range strings.Split(list, ",") {
		out = append(out, head+alt+tail)
	}
	return out
}

// exists reports whether p, relative to parity/, names a directory (p ends
// with a slash) or matches at least one file.
func exists(t *testing.T, p string) bool {
	t.Helper()
	if dir, ok := strings.CutSuffix(p, "/"); ok {
		fi, err := os.Stat(filepath.FromSlash(dir))
		return err == nil && fi.IsDir()
	}
	matches, err := filepath.Glob(filepath.FromSlash(p))
	if err != nil {
		t.Errorf("the Contents table names %s, which is not a valid pattern: %v", p, err)
	}
	return len(matches) > 0
}

// covers reports whether the named path p accounts for want: a directory is
// covered by itself or by a path inside it, a file by a path or pattern that
// matches it.
func covers(p, want string) bool {
	if strings.HasSuffix(want, "/") {
		return strings.HasPrefix(p, want)
	}
	ok, err := path.Match(p, want)
	return err == nil && ok
}

// harnessPaths lists the directories of parity/ down to two levels, each with
// a trailing slash, and the files at its top level except README.md and the
// tests. Hidden directories are left out, and so is out/, where a run writes
// its reports by default.
func harnessPaths(t *testing.T) []string {
	t.Helper()
	var out []string
	err := filepath.WalkDir(".", func(p string, d fs.DirEntry, err error) error {
		if err != nil || p == "." {
			return err
		}
		rel := filepath.ToSlash(p)
		depth := strings.Count(rel, "/") + 1
		switch {
		case d.IsDir() && (strings.HasPrefix(d.Name(), ".") || rel == "out"):
			return fs.SkipDir
		case d.IsDir():
			out = append(out, rel+"/")
			if depth == 2 {
				return fs.SkipDir
			}
		case depth == 1 && d.Name() != "README.md" && !strings.HasSuffix(d.Name(), "_test.go"):
			out = append(out, rel)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return out
}
