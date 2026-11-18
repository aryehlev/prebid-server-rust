package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhgate"
)

type gateOptions struct {
	report    string
	reference string
	jsonOut   string
}

func parseGate(args []string, stderr io.Writer) (gateOptions, error) {
	var o gateOptions

	fs := flag.NewFlagSet("gate", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&o.report, "report", "", "the JSON report of replayharness run --target B --baseline A (required)")
	fs.StringVar(&o.reference, "reference", "", "the JSON report of the same probes run on side A alone, to compare the findings")
	fs.StringVar(&o.jsonOut, "json", "", "also write the verdict as JSON to this file")

	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("parsing flags: %w", err)
	}

	switch {
	case fs.NArg() > 0:
		return o, usageError{"gate takes no arguments"}
	case o.report == "":
		return o, usageError{"-report is required"}
	}

	return o, nil
}

// cmdGate judges a ReplayHarness baseline run (package rhgate): it fails
// when side B diverges from side A on a compared probe, and is incomplete
// when the reports cannot show that.
func cmdGate(args []string, stdout, stderr io.Writer) error {
	opts, err := parseGate(args, stderr)
	if err != nil {
		return err
	}

	base, err := readReport(opts.report)
	if err != nil {
		return err
	}

	var ref *rhgate.Report
	if opts.reference != "" {
		if ref, err = readReport(opts.reference); err != nil {
			return err
		}
	}

	v := rhgate.Judge(base, ref)
	if err := v.Write(stdout); err != nil {
		return fmt.Errorf("printing the verdict: %w", err)
	}

	if err := writeVerdict(v, opts.jsonOut); err != nil {
		return err
	}

	switch {
	case v.Failed():
		return errDiverged
	case len(v.Incomplete) > 0:
		return errIncomplete
	}

	return nil
}

// writeVerdict writes the verdict as JSON to file, unless file is empty.
func writeVerdict(v *rhgate.Verdict, file string) error {
	if file == "" {
		return nil
	}

	raw, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return fmt.Errorf("encoding the verdict: %w", err)
	}

	if err := os.WriteFile(filepath.Clean(file), append(raw, '\n'), privatePerm); err != nil {
		return fmt.Errorf("writing %s: %w", file, err)
	}

	return nil
}

func readReport(file string) (*rhgate.Report, error) {
	raw, err := os.ReadFile(filepath.Clean(file))
	if err != nil {
		return nil, fmt.Errorf("reading %s: %w", file, err)
	}

	rep, err := rhgate.Read(bytes.NewReader(raw))
	if err != nil {
		return nil, fmt.Errorf("reading %s: %w", file, err)
	}

	return rep, nil
}

var (
	errDiverged   = errors.New("side B diverges from side A")
	errIncomplete = errors.New("the reports cannot show whether side B matches side A")
)
