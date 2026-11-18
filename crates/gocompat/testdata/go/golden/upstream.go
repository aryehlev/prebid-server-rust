package main

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// errNoUpstreamDir reports a build in which the go command resolves upstreamModule to no
// directory, because the module is not replaced by a checkout.
var errNoUpstreamDir = errors.New("go list -m " + upstreamModule + " gave no directory; is the module replaced by a checkout?")

// enterUpstream returns path made absolute, then changes into the upstream checkout this module
// builds against: the directory the go command resolves upstreamModule to (the go.mod replace
// target, .upstream/ at the repository root, or the replacement in the go.work file GOWORK
// names). The program then reads static/bidder-info by the repository-relative path it used when
// it ran inside the upstream tree.
func enterUpstream(path string) string {
	abs, err := filepath.Abs(path)
	check(err)

	out, err := exec.CommandContext(context.Background(), "go", "list", "-m", "-f", "{{.Dir}}", upstreamModule).Output()
	check(err)

	dir := strings.TrimSpace(string(out))
	if dir == "" {
		check(errNoUpstreamDir)
	}

	check(os.Chdir(dir))

	return abs
}
