package main

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// enterUpstream returns path made absolute, then changes into the upstream checkout this module
// builds against: the directory the go command resolves upstreamModule to (the go.mod replace
// target, .upstream/ at the repository root, or the replacement in the go.work file GOWORK
// names). The program then reads static/bidder-info by the repository-relative path it used when
// it ran inside the upstream tree.
func enterUpstream(path string) string {
	abs, err := filepath.Abs(path)
	check(err)
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", upstreamModule).Output()
	check(err)
	dir := strings.TrimSpace(string(out))
	if dir == "" {
		check(errors.New("go list -m " + upstreamModule + " gave no directory; is the module replaced by a checkout?"))
	}
	check(os.Chdir(dir))
	return abs
}
