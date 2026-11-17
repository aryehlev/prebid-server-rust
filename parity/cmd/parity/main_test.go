package main

import (
	"bytes"
	"context"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/gen"
	"github.com/prebid/prebid-server/v4/parity/report"
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
