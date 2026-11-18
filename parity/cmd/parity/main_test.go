package main

import (
	"bytes"
	"context"
	"maps"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/gen"
	"github.com/prebid/prebid-server/v4/parity/report"
	"github.com/prebid/prebid-server/v4/parity/runner"
)

func TestRunUsage(t *testing.T) {
	tests := []struct {
		name     string
		args     []string
		wantCode int
		wantErr  string
	}{
		{"no subcommand", nil, exitUsage, "usage: parity"},
		{"unknown subcommand", []string{"bogus"}, exitUsage, "usage: parity"},
		{"help", []string{"report", "-h"}, exitOK, "-baseline"},
		{"report needs a run", []string{"report"}, exitUsage, "--run is required"},
		{"matrix needs a run", []string{"matrix"}, exitUsage, "--run is required"},
		{"minimize needs a run", []string{"minimize"}, exitUsage, "--run is required"},
		{"unknown flag", []string{"run", "--nope"}, exitUsage, "flag provided but not defined"},
		{"unknown implementation", []string{"run", "--impl-a", "java", "--no-build"}, exitUsage, `unknown implementation "java"`},
		{"bad match", []string{"selftest", "--match", "(", "--no-build"}, exitUsage, "--match"},
		{"negative gen", []string{"run", "--gen", "-1", "--no-build"}, exitUsage, "--gen -1"},
		{"missing run dir", []string{"report", "--run", "/nonexistent/parity-run"}, exitUsage, "parity report"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			var stdout, stderr bytes.Buffer
			code := run(context.Background(), tc.args, &stdout, &stderr)
			if code != tc.wantCode {
				t.Errorf("exit code = %d, want %d (stderr %s)", code, tc.wantCode, stderr.String())
			}
			if !strings.Contains(stderr.String(), tc.wantErr) {
				t.Errorf("stderr = %q, want %q", stderr.String(), tc.wantErr)
			}
		})
	}
}

// TestStaleTreeIsRefused: every subcommand that reads the assembled tree
// refuses one that the workspace it names has moved past, here by holding
// another patch, before it reads the tree or a run, and says to assemble the
// tree again.
func TestStaleTreeIsRefused(t *testing.T) {
	const pin = "4e296450de2fcc62741340b2e5f7d1df6c1744a5"
	ws := t.TempDir()
	write := func(rel, data string) {
		t.Helper()
		p := filepath.Join(ws, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("UPSTREAM_SYNC", pin+"\n")
	write("parity/upstream-seams.patch", "diff --git a/main.go b/main.go\n")
	write(corpus.DefaultTree+"/go.mod", "module github.com/prebid/prebid-server/v4\n")
	write(corpus.DefaultTree+"/parity/README.md", "")
	// e69de29b... is the blob ID of an empty patch, which the workspace no
	// longer holds.
	write(corpus.DefaultTree+"/"+corpus.TreeMarker,
		"workspace="+ws+"\nupstream="+pin+"\npatch=e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\nseams=none\n")
	tree := filepath.Join(ws, filepath.FromSlash(corpus.DefaultTree))
	runDir := filepath.Join(ws, "no-run")
	for _, args := range [][]string{
		{"corpus", "--repo", tree},
		{"run", "--no-build", "--repo", tree},
		{"selftest", "--no-build", "--repo", tree},
		{"minimize", "--run", runDir, "--repo", tree},
		{"matrix", "--run", runDir, "--repo", tree},
	} {
		var stdout, stderr bytes.Buffer
		code := run(context.Background(), args, &stdout, &stderr)
		if code != exitUsage || !strings.Contains(stderr.String(), "assemble the tree again with parity/scripts/build-go.sh") {
			t.Errorf("parity %v: exit code %d, want %d and a pointer to build-go.sh; stderr:\n%s", args, code, exitUsage, stderr.String())
		}
	}
}

// TestRunCode: without --baseline, parity run fails on a mismatch, a harness
// error or a failing fixture and is incomplete when it compared nothing; with
// one, the ratchet decides, so the mismatches the baseline already counts
// pass and more harness errors or fewer compared cases fail.
func TestRunCode(t *testing.T) {
	newRun := func(c report.Counts, f *report.FixtureTotals) *report.Run {
		return &report.Run{Summary: report.RunSummary{Mode: report.ModeRun, RunID: "r", ImplA: "go", ImplB: "rust", Counts: c, Fixtures: f}}
	}
	clean := newRun(report.Counts{Total: 3, Passing: 3}, nil)
	mismatch := newRun(report.Counts{Total: 3, Passing: 1, Mismatch: 2}, nil)
	harness := newRun(report.Counts{Total: 3, Passing: 2, Error: 1}, nil)
	fixture := newRun(report.Counts{Total: 3, Passing: 3}, &report.FixtureTotals{Total: 1, Failing: 1})
	nothing := newRun(report.Counts{Total: 3, NotYetInstrumented: 3}, nil)
	nothingButError := newRun(report.Counts{Total: 3, NotYetInstrumented: 2, Error: 1}, nil)

	tests := []struct {
		name     string
		run      *report.Run
		baseline *report.Run
		want     int
		wantOut  string
	}{
		{"clean", clean, nil, exitOK, "run: ok"},
		{"mismatch", mismatch, nil, exitFailure, "run: 2 cases mismatch between go and rust (0 clusters)"},
		{"harness error", harness, nil, exitFailure, "run: 1 cases failed in the harness"},
		{"failing fixture", fixture, nil, exitFailure, "run: 1 adapter fixtures fail"},
		{"nothing compared", nothing, nil, exitIncomplete, "run: no case was compared"},
		{"nothing compared and an error", nothingButError, nil, exitFailure, "run: 1 cases failed in the harness"},
		{"mismatches the baseline counts", mismatch, mismatch, exitOK, "ratchet: ok"},
		{"more errors than the baseline", harness, clean, exitFailure, "ratchet regression: harness errors: 0 -> 1"},
		{"fewer compared than the baseline", nothing, clean, exitFailure, "ratchet regression: compared cases: 3 -> 0"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			path := ""
			if tc.baseline != nil {
				path = filepath.Join(t.TempDir(), "baseline.json")
				if err := report.WriteJSON(path, report.BaselineOf(tc.baseline)); err != nil {
					t.Fatal(err)
				}
			}
			var stdout bytes.Buffer
			code, err := runCode(&stdout, tc.run, path)
			if err != nil {
				t.Fatalf("runCode: %v", err)
			}
			if code != tc.want || !strings.Contains(stdout.String(), tc.wantOut) {
				t.Errorf("exit code %d, output %q; want %d and %q", code, stdout.String(), tc.want, tc.wantOut)
			}
		})
	}
}

// TestCorpusHelpNamesEveryCorpus: the --corpus help lists exactly the corpora
// the repository's cases come from, the runner's introspection cases, the
// generated cases and all.
func TestCorpusHelpNamesEveryCorpus(t *testing.T) {
	repo, err := corpus.FindRepo(".")
	if err != nil {
		t.Fatalf("FindRepo: %v", err)
	}
	c, err := corpus.LoadAll(repo)
	if err != nil {
		t.Fatalf("LoadAll: %v", err)
	}
	want := map[string]bool{runner.CorpusIntrospection: true, gen.CorpusGenerated: true, "all": true}
	for _, cs := range c.Cases {
		want[cs.Corpus] = true
	}
	for _, sub := range []string{"run", "selftest"} {
		var stdout, stderr bytes.Buffer
		if code := run(context.Background(), []string{sub, "-h"}, &stdout, &stderr); code != exitOK {
			t.Fatalf("%s -h: exit code %d", sub, code)
		}
		_, help, ok := strings.Cut(stderr.String(), "comma-separated corpora: ")
		if !ok {
			t.Fatalf("%s -h has no --corpus help:\n%s", sub, stderr.String())
		}
		help, _, _ = strings.Cut(help, "\n")
		help = strings.TrimSuffix(help, ` (default "all")`)
		got := map[string]bool{}
		for _, name := range strings.Split(help, ", ") {
			got[name] = true
		}
		if !reflect.DeepEqual(got, want) {
			t.Errorf("%s -h lists the corpora %q, want %v", sub, help, slices.Sorted(maps.Keys(want)))
		}
	}
}

func TestSplitList(t *testing.T) {
	tests := []struct {
		in   string
		want []string
	}{
		{"", nil},
		{"a", []string{"a"}},
		{" a, ,b ,", []string{"a", "b"}},
	}
	for _, tc := range tests {
		if got := splitList(tc.in); !reflect.DeepEqual(got, tc.want) {
			t.Errorf("splitList(%q) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

func TestWithGenerated(t *testing.T) {
	tests := []struct {
		in   []string
		want []string
	}{
		{nil, nil},
		{[]string{"all"}, []string{"all"}},
		{[]string{"gen"}, []string{"gen"}},
		{[]string{"sample-requests"}, []string{"sample-requests", "gen"}},
	}
	for _, tc := range tests {
		if got := withGenerated(tc.in); !reflect.DeepEqual(got, tc.want) {
			t.Errorf("withGenerated(%v) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

// TestGeneratedCorpus: --gen adds exactly n cases of corpus gen whose IDs
// replay, and keeps the corpus sorted by ID.
func TestGeneratedCorpus(t *testing.T) {
	repo, err := corpus.FindRepo(".")
	if err != nil {
		t.Fatalf("FindRepo: %v", err)
	}
	c, err := generatedCorpus(repo.Root(), 7, 3)
	if err != nil {
		t.Fatalf("generatedCorpus: %v", err)
	}
	var generated []string
	for i, cs := range c.Cases {
		if i > 0 && c.Cases[i-1].ID >= cs.ID {
			t.Fatalf("cases not sorted by unique ID at %d: %q then %q", i, c.Cases[i-1].ID, cs.ID)
		}
		if cs.Corpus == gen.CorpusGenerated {
			generated = append(generated, cs.ID)
		}
	}
	if len(generated) != 3 {
		t.Fatalf("generated cases = %v, want 3", generated)
	}
	for _, id := range generated {
		if _, err := gen.ParseID(id); err != nil {
			t.Errorf("generated ID %q does not parse: %v", id, err)
		}
	}
}

func TestListFlag(t *testing.T) {
	var l listFlag
	for _, v := range []string{"x", "y"} {
		if err := l.Set(v); err != nil {
			t.Fatalf("Set: %v", err)
		}
	}
	if l.String() != "x,y" {
		t.Errorf("listFlag = %q", l.String())
	}
}
