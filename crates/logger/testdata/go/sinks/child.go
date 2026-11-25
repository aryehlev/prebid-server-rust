package main

import (
	"flag"
	"log"
	"os"
	"strings"

	"github.com/golang/glog"
	"github.com/prebid/prebid-server/v4/logger"
)

// childEnv names the scenario a child runs instead of writing the golden.
const childEnv = "PBS_LOGGER_SINKS_CHILD"

// The messages the scenarios log, with the facade call that logs each.
var levelMessages = []struct{ call, message string }{
	{"Debugf", "debugf 1"},
	{"Infof", "infof 2"},
	{"Warnf", "warnf 3"},
	{"Errorf", "errorf 4"},
}

const (
	fatalMessage = "fatalf 5"
	// stdMessage is a line net/http's server writes through the standard logger
	// (net/http/server.go, the TLS handshake error).
	stdMessage = "http: TLS handshake error from 127.0.0.1:1234: EOF"
	// longMessage is the length of a message longer than glog's MaxLogMessageLen, the 15,000
	// bytes a line may take with its header and newline (internal/logsink/logsink.go:29-33).
	longMessage = 20000
	// newlineMessage ends with the newline some of Prebid's calls put at the end of their format
	// (router/router.go:133, for one), after which glog writes no other.
	newlineMessage = "ends with a newline\n"
)

// child parses its arguments as Prebid Server does and runs one scenario.
func child(scenario string) {
	if err := flag.CommandLine.Parse(os.Args[1:]); err != nil {
		os.Exit(3)
	}
	switch scenario {
	case "levels":
		logger.Debugf("debugf %d", 1)
		logger.Infof("infof %d", 2)
		logger.Warnf("warnf %d", 3)
		logger.Errorf("errorf %d", 4)
	case "fatal":
		logger.Fatalf("fatalf %d", 5)
	case "stdlog":
		log.Printf("http: TLS handshake error from %s: %s", "127.0.0.1:1234", "EOF")
		logger.Warnf("warnf %d", 3)
	case "message":
		logger.Infof("%s", strings.Repeat("x", longMessage))
		logger.Infof(newlineMessage)
	default:
		os.Exit(4)
	}
	// Prebid Server never flushes; glog's daemon does every 30 seconds. The child flushes so that
	// the files hold every line when it exits.
	glog.Flush()
}
