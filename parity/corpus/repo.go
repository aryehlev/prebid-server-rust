package corpus

import (
	"crypto/sha1"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
)

// modulePrefix identifies the Prebid Server Go module in go.mod.
const modulePrefix = "module github.com/prebid/prebid-server/"

// Approved corpus roots, relative to the repository root. Loaders only read
// files below these directories (Phase-1 threat T-01-01): every path goes
// through Repo.Resolve, which rejects absolute paths, ".." escapes and symlinks
// that leave the approved root.
const (
	RootAdapters          = "adapters"
	RootSampleRequests    = "endpoints/openrtb2/sample-requests"
	RootEndpointsOpenRTB2 = "endpoints/openrtb2"
	RootExchangeTest      = "exchange/exchangetest"
	RootCacheTest         = "exchange/cachetest"
	RootCustomCacheKey    = "exchange/customcachekeytest"
	RootEventsCacheTest   = "exchange/eventscachetest"
	RootImpCustomCacheKey = "exchange/impcustomcachekeytest"
	RootFPDTests          = "firstpartydata/tests"
	RootHookExecTest      = "hooks/hookexecution/test"
	RootBidderParams      = "static/bidder-params"
	RootBidderInfo        = "static/bidder-info"
	RootRouterParamsTests = "router/bidder_params_tests"
	RootOpenRTBExt        = "openrtb_ext"
	// RootAdapterBuilders is a single file: the bidder to Builder registry.
	RootAdapterBuilders = "exchange/adapter_builders.go"
	// RootReplayHarness (replayharness.go) is approved too.
)

// ErrOutsideCorpus is returned for paths that escape the approved corpus roots.
var ErrOutsideCorpus = errors.New("corpus: path is outside the approved corpus roots")

// ApprovedRoots returns the approved corpus roots in a stable order.
func ApprovedRoots() []string {
	roots := []string{
		RootAdapters, RootSampleRequests, RootEndpointsOpenRTB2, RootExchangeTest,
		RootCacheTest, RootCustomCacheKey, RootEventsCacheTest, RootImpCustomCacheKey,
		RootFPDTests, RootHookExecTest, RootBidderParams, RootBidderInfo, RootRouterParamsTests, RootOpenRTBExt,
		RootAdapterBuilders, RootReplayHarness,
	}
	sort.Strings(roots)
	return roots
}

// The workspace and the assembled tree. The harness lives in the parity/
// directory of the Rust workspace (the repository that holds the Rust port),
// which never holds upstream Go, so this code compiles only inside a tree that
// parity/scripts/build-go.sh assembles: the upstream commit UPSTREAM_SYNC pins,
// the seams patch and a copy of the workspace's parity/. The corpus is read
// from that tree; reports go to the workspace (Repo.HarnessRoot).
const (
	// TreeMarker is the file build-go.sh writes at the top of the tree it
	// assembles: "workspace=", "upstream=", "patch=" and "seams=" lines naming
	// the workspace, the upstream commit, the blob ID of the patch it applied
	// and that of the seams the tree holds.
	TreeMarker = ".parity-tree"
	// DefaultTree is where build-go.sh assembles the tree, relative to the
	// workspace.
	DefaultTree = "parity/out/tree"
	// workspacePin and workspaceSeams are the files that make a directory the
	// workspace: the pinned upstream commit and the seams patch.
	workspacePin   = "UPSTREAM_SYNC"
	workspaceSeams = "parity/upstream-seams.patch"
	// buildScript is what assembles the tree, relative to the workspace.
	buildScript = "parity/scripts/build-go.sh"
)

// IsWorkspace reports whether dir is the Rust workspace of the harness: it
// holds the pinned upstream commit and the seams patch.
func IsWorkspace(dir string) bool {
	for _, name := range []string{workspacePin, workspaceSeams} {
		fi, err := os.Stat(filepath.Join(dir, filepath.FromSlash(name)))
		if err != nil || !fi.Mode().IsRegular() {
			return false
		}
	}
	return true
}

// Repo is a Prebid Server checkout the corpus is read from: the assembled tree.
type Repo struct {
	root string
}

// OpenRepo returns the repository rooted at root. It checks that root holds the
// Prebid Server go.mod so that a misconfigured path fails early.
func OpenRepo(root string) (*Repo, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, fmt.Errorf("corpus: resolving repository root %q: %w", root, err)
	}
	resolved, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return nil, fmt.Errorf("corpus: resolving repository root %q: %w", root, err)
	}
	mod, err := os.ReadFile(filepath.Join(resolved, "go.mod"))
	if err != nil {
		return nil, fmt.Errorf("corpus: %q is not a Prebid Server checkout: %w", root, err)
	}
	if !strings.Contains(string(mod), modulePrefix) {
		return nil, fmt.Errorf("corpus: %q is not a Prebid Server checkout (go.mod has no %q)", root, modulePrefix)
	}
	return &Repo{root: resolved}, nil
}

// FindRepo walks up from start until it finds the tree to read the corpus
// from: a Prebid Server checkout that holds parity/ (the assembled tree), or
// the workspace, whose tree is DefaultTree. A checkout without parity/, such
// as the workspace's .upstream, is passed over.
func FindRepo(start string) (*Repo, error) {
	dir, err := filepath.Abs(start)
	if err != nil {
		return nil, fmt.Errorf("corpus: resolving %q: %w", start, err)
	}
	for {
		if fi, err := os.Stat(filepath.Join(dir, "parity")); err == nil && fi.IsDir() {
			if repo, err := OpenRepo(dir); err == nil {
				return repo, nil
			}
		}
		if IsWorkspace(dir) {
			tree := filepath.Join(dir, filepath.FromSlash(DefaultTree))
			repo, err := OpenRepo(tree)
			if err != nil {
				return nil, fmt.Errorf("corpus: %s is the workspace, but its assembled tree cannot be used (run %s): %w", dir, buildScript, err)
			}
			return repo, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return nil, fmt.Errorf("corpus: no assembled tree or workspace above %q (run %s in the workspace)", start, buildScript)
		}
		dir = parent
	}
}

// HarnessRoot returns the directory whose parity/ holds the harness sources
// and receives the reports (parity/out): the workspace TreeMarker names, for
// an assembled tree, or else the checkout itself. In the workspace the Rust
// build is under target/.
//
// An assembled tree must still be the one build-go.sh would assemble from the
// workspace now, at the commit its UPSTREAM_SYNC pins and with its
// parity/upstream-seams.patch: once either moved, the corpus, the profiles and
// every Go build of the tree would be another upstream commit or other seams
// than the workspace names, so HarnessRoot fails and says to assemble the tree
// again.
func (r *Repo) HarnessRoot() (string, error) {
	marker := filepath.Join(r.root, TreeMarker)
	raw, err := os.ReadFile(marker)
	if errors.Is(err, fs.ErrNotExist) {
		return r.root, nil
	}
	if err != nil {
		return "", fmt.Errorf("corpus: reading %s: %w", marker, err)
	}
	fields := map[string]string{}
	for _, line := range strings.Split(string(raw), "\n") {
		if key, value, ok := strings.Cut(line, "="); ok {
			if _, seen := fields[key]; !seen {
				fields[key] = value
			}
		}
	}
	ws, ok := fields["workspace"]
	if !ok {
		return "", fmt.Errorf("corpus: %s names no workspace; assemble the tree again with %s", marker, buildScript)
	}
	if !filepath.IsAbs(ws) || !IsWorkspace(ws) {
		return "", fmt.Errorf("corpus: %s names %q, which is not the workspace (any more); assemble the tree again with %s", marker, ws, buildScript)
	}
	pinFile := filepath.Join(ws, workspacePin)
	pin, err := readPin(pinFile)
	if err != nil {
		return "", err
	}
	if fields["upstream"] != pin {
		return "", fmt.Errorf("corpus: %s was assembled at upstream commit %q, but %s pins %s; assemble the tree again with %s", marker, fields["upstream"], pinFile, pin, buildScript)
	}
	seamsFile := filepath.Join(ws, filepath.FromSlash(workspaceSeams))
	seams, err := os.ReadFile(seamsFile)
	if err != nil {
		return "", fmt.Errorf("corpus: reading %s: %w", seamsFile, err)
	}
	if want := blobID(seams); fields["patch"] != want {
		return "", fmt.Errorf("corpus: %s was assembled with the patch blob %q, but %s is blob %s; assemble the tree again with %s", marker, fields["patch"], seamsFile, want, buildScript)
	}
	return ws, nil
}

// readPin returns the commit an UPSTREAM_SYNC file pins: its one line that is
// neither blank nor a comment, trimmed, as build-go.sh reads it.
func readPin(path string) (string, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("corpus: reading %s: %w", path, err)
	}
	var pins []string
	for _, line := range strings.Split(string(raw), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "#") {
			pins = append(pins, line)
		}
	}
	if len(pins) != 1 {
		return "", fmt.Errorf("corpus: %s must hold exactly one non-comment line, the pinned commit", path)
	}
	return pins[0], nil
}

// blobID returns the ID git gives a blob of raw (git hash-object
// --no-filters), which build-go.sh records for the patch it applies.
func blobID(raw []byte) string {
	h := sha1.New()
	fmt.Fprintf(h, "blob %d\x00", len(raw))
	h.Write(raw)
	return hex.EncodeToString(h.Sum(nil))
}

// Root returns the absolute repository root.
func (r *Repo) Root() string { return r.root }

// Resolve maps a repo-relative slash path to an absolute path, rejecting
// anything outside the approved corpus roots.
func (r *Repo) Resolve(rel string) (string, error) {
	clean, err := cleanRel(rel)
	if err != nil {
		return "", err
	}
	if !underApprovedRoot(clean) {
		return "", fmt.Errorf("%w: %q", ErrOutsideCorpus, rel)
	}
	abs := filepath.Join(r.root, filepath.FromSlash(clean))
	resolved, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return "", fmt.Errorf("corpus: resolving %q: %w", rel, err)
	}
	back, err := filepath.Rel(r.root, resolved)
	if err != nil || !underApprovedRoot(filepath.ToSlash(back)) {
		return "", fmt.Errorf("%w: %q resolves to %q", ErrOutsideCorpus, rel, resolved)
	}
	return resolved, nil
}

// ReadFile reads a repo-relative corpus file.
func (r *Repo) ReadFile(rel string) ([]byte, error) {
	abs, err := r.Resolve(rel)
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(abs)
	if err != nil {
		return nil, fmt.Errorf("corpus: reading %s: %w", rel, err)
	}
	return data, nil
}

// Walk visits every regular file below the repo-relative directory rel in
// lexical order, passing repo-relative slash paths. Symlinks are not followed.
func (r *Repo) Walk(rel string, visit func(relPath string, d fs.DirEntry) error) error {
	abs, err := r.Resolve(rel)
	if err != nil {
		return err
	}
	return filepath.WalkDir(abs, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return fmt.Errorf("corpus: walking %s: %w", rel, err)
		}
		if d.IsDir() || !d.Type().IsRegular() {
			return nil
		}
		back, err := filepath.Rel(r.root, p)
		if err != nil {
			return fmt.Errorf("corpus: relativizing %s: %w", p, err)
		}
		return visit(filepath.ToSlash(back), d)
	})
}

// ReadDir lists a repo-relative corpus directory.
func (r *Repo) ReadDir(rel string) ([]fs.DirEntry, error) {
	abs, err := r.Resolve(rel)
	if err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(abs)
	if err != nil {
		return nil, fmt.Errorf("corpus: listing %s: %w", rel, err)
	}
	return entries, nil
}

// FS returns the repo-relative corpus directory rel as an fs.FS for code that
// reads through one. Every name it opens, reads or lists goes through Resolve,
// so it reads nothing outside the approved corpus roots, whatever symlinks
// the directory holds.
func (r *Repo) FS(rel string) fs.FS { return repoFS{repo: r, dir: rel} }

// repoFS is Repo.FS.
type repoFS struct {
	repo *Repo
	dir  string
}

// rel maps an fs.FS name to a repo-relative path.
func (f repoFS) rel(op, name string) (string, error) {
	if !fs.ValidPath(name) {
		return "", &fs.PathError{Op: op, Path: name, Err: fs.ErrInvalid}
	}
	return path.Join(f.dir, name), nil
}

func (f repoFS) Open(name string) (fs.File, error) {
	rel, err := f.rel("open", name)
	if err != nil {
		return nil, err
	}
	abs, err := f.repo.Resolve(rel)
	if err != nil {
		return nil, &fs.PathError{Op: "open", Path: name, Err: err}
	}
	return os.Open(abs)
}

func (f repoFS) ReadFile(name string) ([]byte, error) {
	rel, err := f.rel("readfile", name)
	if err != nil {
		return nil, err
	}
	return f.repo.ReadFile(rel)
}

func (f repoFS) ReadDir(name string) ([]fs.DirEntry, error) {
	rel, err := f.rel("readdir", name)
	if err != nil {
		return nil, err
	}
	return f.repo.ReadDir(rel)
}

func cleanRel(rel string) (string, error) {
	if rel == "" {
		return "", fmt.Errorf("%w: empty path", ErrOutsideCorpus)
	}
	slashed := filepath.ToSlash(rel)
	if path.IsAbs(slashed) || filepath.IsAbs(rel) || filepath.VolumeName(rel) != "" {
		return "", fmt.Errorf("%w: absolute path %q", ErrOutsideCorpus, rel)
	}
	for _, part := range strings.Split(slashed, "/") {
		if part == ".." {
			return "", fmt.Errorf("%w: %q contains \"..\"", ErrOutsideCorpus, rel)
		}
	}
	clean := path.Clean(slashed)
	if clean == "." {
		return "", fmt.Errorf("%w: %q names the repository root", ErrOutsideCorpus, rel)
	}
	return clean, nil
}

func underApprovedRoot(clean string) bool {
	for _, root := range ApprovedRoots() {
		if clean == root || strings.HasPrefix(clean, root+"/") {
			return true
		}
	}
	return false
}
