package main

import (
	"bytes"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/prebid/prebid-server/v4/parity/replayharness/traffic"
)

const (
	// defaultMaxBytes keeps the committed recording under 5 MB.
	defaultMaxBytes = 5_000_000
	// defaultMaxRequestBody drops size probes: every body above 64 KiB in a
	// recording comes from a replay or probe that sends it again on each run.
	defaultMaxRequestBody = 64 << 10
	// defaultLargeResponse keeps a large response body once per method, path
	// and status.
	defaultLargeResponse = 64 << 10
)

type trafficOptions struct {
	tokenFile      string
	out            string
	maxBytes       int
	maxRequestBody int
	largeResponse  int
	inputs         []string
}

func parseTraffic(args []string, stderr io.Writer) (trafficOptions, error) {
	var o trafficOptions

	fs := flag.NewFlagSet("traffic", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&o.tokenFile, "token-file", "", "file holding the parity token, which must not appear in the recording (required)")
	fs.StringVar(&o.out, "o", "", "the compacted recording to write (required)")
	fs.IntVar(&o.maxBytes, "max-bytes", defaultMaxBytes, "fail when the compacted recording is larger")
	fs.IntVar(&o.maxRequestBody, "max-request-body", defaultMaxRequestBody,
		"drop exchanges whose recorded request body is longer (0 keeps all)")
	fs.IntVar(&o.largeResponse, "large-response", defaultLargeResponse,
		"keep a response body this long or longer once per method, path and status (0 keeps all)")

	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("parsing flags: %w", err)
	}

	o.inputs = fs.Args()

	switch {
	case o.tokenFile == "", o.out == "":
		return o, usageError{"-token-file and -o are required"}
	case len(o.inputs) == 0:
		return o, usageError{"name at least one recording"}
	}

	return o, nil
}

// cmdTraffic compacts recordings into one file: the first exchange of each
// distinct request, unchanged, without size probes, repeated large
// responses or secrets.
func cmdTraffic(args []string, stdout, stderr io.Writer) error {
	opts, err := parseTraffic(args, stderr)
	if err != nil {
		return err
	}

	secret, err := readToken(opts.tokenFile)
	if err != nil {
		return err
	}

	readers := make([]io.Reader, 0, len(opts.inputs))

	for _, in := range opts.inputs {
		raw, err := os.ReadFile(filepath.Clean(in))
		if err != nil {
			return fmt.Errorf("reading %s: %w", in, err)
		}

		readers = append(readers, bytes.NewReader(raw))
	}

	var out bytes.Buffer

	stats, err := traffic.Compact(readers, &out, traffic.Options{
		Secrets:        []string{secret},
		MaxBytes:       opts.maxBytes,
		MaxRequestBody: opts.maxRequestBody,
		LargeResponse:  opts.largeResponse,
	})
	if err != nil {
		return fmt.Errorf("compacting: %w", err)
	}

	if err := os.WriteFile(filepath.Clean(opts.out), out.Bytes(), privatePerm); err != nil {
		return fmt.Errorf("writing %s: %w", opts.out, err)
	}

	_, _ = fmt.Fprintf(stdout,
		"rhtool traffic: %d exchanges read, %d kept; dropped %d duplicates, %d large request bodies, "+
			"%d repeated large responses; %d bytes in %s\n",
		stats.Read, stats.Kept, stats.Duplicates, stats.LargeRequests, stats.RepeatedResponses, stats.Bytes, opts.out)

	return nil
}

var errEmptyToken = errors.New("the token file is empty")
