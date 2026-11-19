package main

import (
	"bytes"
	"flag"
	"os"
	"os/exec"
	"regexp"
	"strings"

	"github.com/prebid/prebid-server/v4/logger"
)

// glogChildEnv makes the program log one line and exit instead of writing the goldens; glog
// holds on to the process's stderr, so the line is read from a child process.
const glogChildEnv = "DIFFERENCES_GOLDEN_GLOG_CHILD"

// glogFlags are the flags the Dockerfile's CMD starts Prebid Server with.
var glogFlags = []string{"-v", "1", "-logtostderr"}

// The message both sides log.
const (
	logFormat   = "bidder returned no bids: %s"
	logArgument = "appnexus"
)

// glogChild logs a warning through the logger package, as Prebid's code does.
func glogChild() {
	if err := flag.CommandLine.Parse(glogFlags); err != nil {
		panic(err)
	}
	logger.Warnf(logFormat, logArgument)
}

// glogLine is glog's header: severity, month and day, time, thread id (the pid, padded),
// file:line, then the message.
var glogLine = regexp.MustCompile(`^([IWEF])(\d{4}) (\d{2}:\d{2}:\d{2}\.\d{6}) +(\d+) ([^ ]+:\d+)\] (.*)$`)

type loggingDoc struct {
	Header   header   `json:"header"`
	Flags    []string `json:"flags"`
	Message  string   `json:"message"`
	Severity string   `json:"severity"`
	FileLine string   `json:"file_line"`
	// Line is the line glog wrote, with the date, time and pid replaced by placeholders.
	Line string `json:"line"`
}

func loggingGolden() loggingDoc {
	exe, err := os.Executable()
	if err != nil {
		panic(err)
	}
	cmd := exec.Command(exe)
	cmd.Env = append(os.Environ(), glogChildEnv+"=1")
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		panic(err)
	}
	line, _, _ := strings.Cut(stderr.String(), "\n")
	m := glogLine.FindStringSubmatch(line)
	if m == nil {
		panic("not a glog line: " + line)
	}
	return loggingDoc{
		Header:   newHeader("github.com/golang/glog"),
		Flags:    glogFlags,
		Message:  m[6],
		Severity: m[1],
		FileLine: m[5],
		Line:     m[1] + "MMDD hh:mm:ss.uuuuuu PID " + m[5] + "] " + m[6],
	}
}
