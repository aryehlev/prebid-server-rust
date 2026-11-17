package main

import (
	"log"
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
// names). The program then reads the upstream files by the repository-relative paths it used
// when it ran inside the upstream tree.
func enterUpstream(path string) string {
	abs, err := filepath.Abs(path)
	if err != nil {
		log.Fatal(err)
	}
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Dir}}", upstreamModule).Output()
	if err != nil {
		log.Fatalf("go list -m %s: %v", upstreamModule, err)
	}
	dir := strings.TrimSpace(string(out))
	if dir == "" {
		log.Fatalf("go list -m %s gave no directory; is the module replaced by a checkout?", upstreamModule)
	}
	if err := os.Chdir(dir); err != nil {
		log.Fatal(err)
	}
	return abs
}
