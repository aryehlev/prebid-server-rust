// Command rhtool runs the Go Prebid Server for ReplayHarness and keeps what
// ReplayHarness recorded.
//
//	rhtool serve   -dir DIR [-bin FILE] [-profile NAME] [-servers N] [-spare-ports N]
//	               [-bin-b FILE] [-impl-b go|rust]
//	rhtool traffic -token-file FILE -o FILE [-max-bytes N] RECORDING...
//	rhtool saved   -token-file FILE -o FILE [-repo DIR] RECORDING
//	rhtool cases   [-repo DIR] [-json FILE]
//	rhtool gate    -report FILE [-reference FILE] [-json FILE]
//
// serve boots the Go server the way the parity runner boots it, and runs
// until it is interrupted. traffic compacts recordings into the committed
// parity/replayharness/traffic.jsonl. saved keeps, from a recording made
// with --keep-secrets, Go's answers to the replay steps that save values
// (parity/replayharness/saved.jsonl). cases prints what the replayharness
// parity corpus compares and what it leaves out, with the reasons. gate
// judges a `replayharness run --baseline` report of side B against side A.
// parity/scripts/replayharness-replay.sh and parity/scripts/replayharness.sh
// drive them.
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
	exitOK         = 0
	exitFailure    = 1
	exitUsage      = 2
	exitIncomplete = 3
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

	return exitCode(args[0], dispatch(ctx, args, stdout, stderr), stderr)
}

// dispatch runs the subcommand args[0].
func dispatch(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	switch args[0] {
	case "serve":
		return cmdServe(ctx, args[1:], stdout, stderr)
	case "traffic":
		return cmdTraffic(args[1:], stdout, stderr)
	case "saved":
		return cmdSaved(args[1:], stdout, stderr)
	case "cases":
		return cmdCases(args[1:], stdout, stderr)
	case "gate":
		return cmdGate(args[1:], stdout, stderr)
	}

	return errUnknownCommand
}

// exitCode prints the error of subcommand cmd and maps it to an exit code.
func exitCode(cmd string, err error, stderr io.Writer) int {
	switch {
	case err == nil, errors.Is(err, flag.ErrHelp):
		return exitOK
	case errors.Is(err, errUnknownCommand):
		usage(stderr)

		return exitUsage
	}

	_, _ = fmt.Fprintf(stderr, "rhtool %s: %v\n", cmd, err)

	var usageErr usageError

	switch {
	case errors.As(err, &usageErr):
		return exitUsage
	case errors.Is(err, errIncomplete):
		return exitIncomplete
	}

	return exitFailure
}

func usage(w io.Writer) {
	_, _ = fmt.Fprintln(w, "usage: rhtool serve|traffic|saved|cases|gate [flags]  (rhtool <cmd> -h for flags)")
}

var errUnknownCommand = errors.New("unknown command")

// usageError is a bad command line.
type usageError struct{ msg string }

func (e usageError) Error() string { return e.msg }
