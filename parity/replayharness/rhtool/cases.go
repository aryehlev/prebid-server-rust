package main

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/corpus"
)

type casesOptions struct {
	repo    string
	jsonOut string
}

func parseCases(args []string, stderr io.Writer) (casesOptions, error) {
	var o casesOptions

	fs := flag.NewFlagSet("cases", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&o.repo, "repo", ".", "a directory inside the Prebid Server checkout")
	fs.StringVar(&o.jsonOut, "json", "", "also write the requests that are not compared, with their reasons, to this JSON file")

	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("parsing flags: %w", err)
	}

	if fs.NArg() > 0 {
		return o, usageError{"cases takes no arguments"}
	}

	return o, nil
}

// casesReport is the -json document of cmdCases.
type casesReport struct {
	Cases       int                  `json:"cases"`
	NotCompared []corpus.NotCompared `json:"not_compared"`
	ByTag       map[string]int       `json:"not_compared_by_tag"`
}

// cmdCases prints what the replayharness corpus compares and what it does
// not, with each reason. It fails when the corpus does not load cleanly.
func cmdCases(args []string, stdout, stderr io.Writer) error {
	opts, err := parseCases(args, stderr)
	if err != nil {
		return err
	}

	repo, err := corpus.FindRepo(opts.repo)
	if err != nil {
		return fmt.Errorf("finding the repository: %w", err)
	}

	rh := corpus.LoadReplayHarness(repo)
	if len(rh.Errors) > 0 {
		return fmt.Errorf("loading the replayharness corpus: %w", errors.Join(rh.Errors...))
	}

	rep := casesReport{Cases: len(rh.Cases), NotCompared: rh.NotCompared, ByTag: map[string]int{}}
	for _, nc := range rh.NotCompared {
		rep.ByTag[nc.Tag]++
	}

	tags := make([]string, 0, len(rep.ByTag))
	for tag, n := range rep.ByTag {
		tags = append(tags, fmt.Sprintf("%s %d", tag, n))
	}

	sort.Strings(tags)

	_, _ = fmt.Fprintf(stdout, "rhtool cases: %d cases for the parity runner; %d requests not compared, each with its reason (%s)\n",
		rep.Cases, len(rh.NotCompared), strings.Join(tags, ", "))

	for _, nc := range rh.NotCompared {
		_, _ = fmt.Fprintf(stdout, "  not compared [%s] %s\n", nc.Tag, nc.Request)
	}

	if opts.jsonOut == "" {
		return nil
	}

	raw, err := json.MarshalIndent(rep, "", "  ")
	if err != nil {
		return fmt.Errorf("encoding the report: %w", err)
	}

	if err := os.WriteFile(filepath.Clean(opts.jsonOut), append(raw, '\n'), privatePerm); err != nil {
		return fmt.Errorf("writing %s: %w", opts.jsonOut, err)
	}

	return nil
}
