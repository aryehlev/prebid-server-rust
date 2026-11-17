package main

import (
	"bytes"
	"context"
	"reflect"
	"strings"
	"testing"

	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/gen"
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
