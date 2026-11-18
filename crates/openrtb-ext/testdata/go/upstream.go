package main

import (
	"context"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// enterUpstream makes every path absolute, then changes into the upstream checkout this module
// builds against: the directory the go command resolves upstreamModule to (the go.mod replace
// target, .upstream/ at the workspace root, or the replacement in the go.work file GOWORK
// names). The corpora and sources are then read by the repository-relative paths the goldens
// record, as when the program ran inside the upstream tree.
func enterUpstream(paths ...*string) {
	for _, p := range paths {
		abs, err := filepath.Abs(*p)
		if err != nil {
			log.Fatal(err)
		}

		*p = abs
	}

	out, err := exec.CommandContext(context.Background(), "go", "list", "-m", "-f", "{{.Dir}}", upstreamModule).Output()
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
}
