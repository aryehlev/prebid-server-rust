package corpus

import (
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
)

// ErrOutsideCorpus is returned for paths that escape the approved corpus roots.
var ErrOutsideCorpus = errors.New("corpus: path is outside the approved corpus roots")

// ApprovedRoots returns the approved corpus roots in a stable order.
func ApprovedRoots() []string {
	roots := []string{
		RootAdapters, RootSampleRequests, RootEndpointsOpenRTB2, RootExchangeTest,
		RootCacheTest, RootCustomCacheKey, RootEventsCacheTest, RootImpCustomCacheKey,
		RootFPDTests, RootHookExecTest, RootBidderParams, RootBidderInfo, RootRouterParamsTests, RootOpenRTBExt,
	}
	sort.Strings(roots)
	return roots
}

// Repo is a Prebid Server checkout the corpus is read from.
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

// FindRepo walks up from start until it finds the Prebid Server checkout.
func FindRepo(start string) (*Repo, error) {
	dir, err := filepath.Abs(start)
	if err != nil {
		return nil, fmt.Errorf("corpus: resolving %q: %w", start, err)
	}
	for {
		if repo, err := OpenRepo(dir); err == nil {
			return repo, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return nil, fmt.Errorf("corpus: no Prebid Server checkout above %q", start)
		}
		dir = parent
	}
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
