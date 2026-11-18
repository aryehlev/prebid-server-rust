package main

import (
	"context"
	"log"
	"os/exec"
	"path/filepath"
	"strings"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// upstreamDir is the upstream checkout this module builds against: the directory the go command
// resolves upstreamModule to (the go.mod replace target, .upstream/ at the repository root, or
// the replacement in the go.work file GOWORK names), with its symbolic links resolved, since
// filepath.WalkDir does not follow a link at its root (.upstream may link to a checkout kept
// elsewhere).
func upstreamDir() string {
	out, err := exec.CommandContext(context.Background(), "go", "list", "-m", "-f", "{{.Dir}}", upstreamModule).Output()
	if err != nil {
		log.Fatalf("go list -m %s: %v", upstreamModule, err)
	}

	dir := strings.TrimSpace(string(out))
	if dir == "" {
		log.Fatalf("go list -m %s gave no directory; is the module replaced by a checkout?", upstreamModule)
	}

	resolved, err := filepath.EvalSymlinks(dir)
	if err != nil {
		log.Fatalf("the upstream checkout: %v", err)
	}

	return resolved
}
