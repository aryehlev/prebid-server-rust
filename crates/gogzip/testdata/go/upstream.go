package main

import (
	"log"
	"os"
	"path/filepath"
	"strings"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// goRoot is the upstream checkout the fixtures are read from, found as the Rust tests find it:
// $PBS_GO_ROOT when it is set (a relative path is taken from the workspace root, four
// directories above this module), else .upstream at the workspace root, with its symbolic links
// resolved. It is never searched for. This module imports no upstream package, so the go command
// cannot name the checkout's directory the way it does for the other generators.
func goRoot() string {
	workspace := filepath.Join(moduleDir(), "..", "..", "..", "..")
	dir, source := filepath.Join(workspace, ".upstream"), "the default"
	fetch := "run scripts/fetch-upstream.sh to check out the commit UPSTREAM_SYNC pins there"
	if env := os.Getenv("PBS_GO_ROOT"); env != "" {
		dir, source = env, "PBS_GO_ROOT"
		if !filepath.IsAbs(dir) {
			dir = filepath.Join(workspace, dir)
		}
		fetch = "run scripts/fetch-upstream.sh --dir " + dir + " to check out the commit UPSTREAM_SYNC pins there"
	}

	resolved, err := filepath.EvalSymlinks(dir)
	if err != nil {
		log.Fatalf("the Go root %s (%s) cannot be used: %v. It is the upstream Go server that the "+
			"goldens are generated from; %s (or set PBS_GO_ROOT to such a checkout)", dir, source, err, fetch)
	}

	gomod, err := os.ReadFile(filepath.Join(resolved, "go.mod"))
	first, _, _ := strings.Cut(string(gomod), "\n")
	if err != nil || strings.TrimSpace(first) != "module "+upstreamModule {
		log.Fatalf("the Go root %s (%s) is not a checkout of %s: its go.mod does not declare the "+
			"module; %s", resolved, source, upstreamModule, fetch)
	}

	return resolved
}
