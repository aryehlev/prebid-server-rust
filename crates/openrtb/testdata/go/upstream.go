package main

import (
	"context"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

const (
	// upstreamModule is the module path of the upstream Go server.
	upstreamModule = "github.com/prebid/prebid-server/v4"
	// openrtbModule is the module path of the OpenRTB types the upstream server uses.
	openrtbModule = "github.com/prebid/openrtb/v20"
)

// moduleDirs are the directories, by module path, of openrtbModule and upstreamModule in this
// module's build: the module cache for the openrtb module, the upstream checkout (the go.mod
// replace target, .upstream/ at the repository root, or the replacement in the go.work file
// GOWORK names) for the upstream module. It runs the go command in the current directory, so it
// is called before enterUpstream.
func moduleDirs() map[string]string {
	cmd := exec.CommandContext(context.Background(), "go", "list", "-m", "-f", "{{.Path}}\t{{.Dir}}", openrtbModule, upstreamModule)

	out, err := cmd.Output()
	if err != nil {
		log.Fatalf("go list -m %s %s: %v", openrtbModule, upstreamModule, err)
	}

	dirs := map[string]string{}

	for line := range strings.SplitSeq(strings.TrimSpace(string(out)), "\n") {
		path, dir, _ := strings.Cut(line, "\t")
		if dir == "" {
			log.Fatalf("go list -m %s gave no directory", path)
		}

		dirs[path] = dir
	}

	return dirs
}

// enterUpstream makes every path absolute, then changes into dir, the upstream checkout this
// module builds against (see moduleDirs). The corpora are then read by the repository-relative
// paths the goldens record, as when the program ran inside the upstream tree.
func enterUpstream(dir string, paths ...*string) {
	for _, p := range paths {
		abs, err := filepath.Abs(*p)
		if err != nil {
			log.Fatal(err)
		}

		*p = abs
	}

	if err := os.Chdir(dir); err != nil {
		log.Fatal(err)
	}
}
