package main

import (
	"bytes"
	"context"
	"reflect"
	"strings"
	"testing"
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
