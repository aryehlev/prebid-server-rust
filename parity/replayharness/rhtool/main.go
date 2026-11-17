// Command rhtool runs the Go Prebid Server for ReplayHarness and keeps what
// ReplayHarness recorded.
//
//	rhtool serve   -dir DIR [-bin FILE] [-profile NAME] [-servers N] [-spare-ports N]
//	rhtool traffic -token-file FILE -o FILE [-max-bytes N] RECORDING...
//
// serve boots the Go server the way the parity runner boots it, and runs
// until it is interrupted. traffic compacts recordings into the committed
// parity/replayharness/traffic.jsonl. parity/scripts/replayharness-replay.sh
// drives both.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"
)

// Exit codes.
const (
	exitOK      = 0
	exitFailure = 1
	exitUsage   = 2
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := run(ctx, os.Args[1:], os.Stdout, os.Stderr)

	stop()
	os.Exit(code)
}

// run dispatches a subcommand and returns the exit code.
func run(ctx context.Context, args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		usage(stderr)

		return exitUsage
	}

	var err error

	switch args[0] {
	case "serve":
		err = cmdServe(ctx, args[1:], stdout, stderr)
	case "traffic":
		err = cmdTraffic(args[1:], stdout, stderr)
	default:
		usage(stderr)

		return exitUsage
	}

	var usageErr usageError

	switch {
	case err == nil, errors.Is(err, flag.ErrHelp):
		return exitOK
	case errors.As(err, &usageErr):
		_, _ = fmt.Fprintf(stderr, "rhtool %s: %v\n", args[0], err)

		return exitUsage
	default:
		_, _ = fmt.Fprintf(stderr, "rhtool %s: %v\n", args[0], err)

		return exitFailure
	}
}

func usage(w io.Writer) {
	_, _ = fmt.Fprintln(w, "usage: rhtool serve|traffic [flags]  (rhtool <cmd> -h for flags)")
}

// usageError is a bad command line.
type usageError struct{ msg string }

func (e usageError) Error() string { return e.msg }
