package main

import (
	"log"
	"os"
	"path/filepath"
)

// upstreamModule is the module path of the upstream Go server.
const upstreamModule = "github.com/prebid/prebid-server/v4"

// enterUpstream makes every path absolute, then changes into the upstream checkout this module
// builds against (see moduleDir). The corpora are then read by the repository-relative paths the
// goldens record, as when the program ran inside the upstream tree.
func enterUpstream(paths ...*string) {
	dir := moduleDir(upstreamModule)
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
