package scripts

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"testing"
)

// The scripts in these tests read the pinned upstream checkout from
// $PBS_GO_ROOT, else from .upstream at the repository root, where
// scripts/fetch-upstream.sh checks it out; without it they fail, naming
// fetch-upstream.sh.

// fullIndex matches the blob IDs of a patch's index line.
var fullIndex = regexp.MustCompile(`(?m)^index ([0-9a-f]{12})[0-9a-f]{28}\.\.([0-9a-f]{12})[0-9a-f]{28}`)

// TestSeamsCheck: parity/upstream-seams.patch applies cleanly to the commit
// UPSTREAM_SYNC pins and is in the form seams.sh write gives it (seams.sh
// check). The check refuses a patch that does not apply, and one that
// applies but is not in that form.
func TestSeamsCheck(t *testing.T) {
	bash := requireBash(t)
	patch, err := os.ReadFile(filepath.Join("..", "upstream-seams.patch"))
	if err != nil {
		t.Fatal(err)
	}
	files := bytes.Count(patch, []byte("\ndiff --git "))
	if bytes.HasPrefix(patch, []byte("diff --git ")) {
		files++
	}
	abbreviated := fullIndex.ReplaceAll(patch, []byte("index ${1}..${2}"))
	if bytes.Equal(abbreviated, patch) {
		t.Fatal("the patch has no full index line to abbreviate")
	}
	tests := []struct {
		name  string
		patch []byte // nil: the committed patch
		code  int
		want  string
	}{
		{"the committed patch", nil, 0, "applies cleanly to the pinned commit (" + strconv.Itoa(files) + " files)"},
		{"a stale context line", staleContext(t, patch), 1, "does not apply to"},
		{"abbreviated blob IDs", abbreviated, 1, "applies, but is not in the form seams.sh write gives it"},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			args := []string{"seams.sh", "check"}
			if tc.patch != nil {
				file := filepath.Join(t.TempDir(), "seams.patch")
				if err := os.WriteFile(file, tc.patch, 0o600); err != nil {
					t.Fatal(err)
				}
				args = append(args, "--patch", file)
			}
			out, code := runScript(t, bash, args...)
			if code != tc.code || !strings.Contains(out, tc.want) {
				t.Errorf("%v: exit code %d, want %d with %q; output:\n%s", args, code, tc.code, tc.want, out)
			}
		})
	}
}

// TestBuildGoGuardsTheTree: build-go.sh assembles a tree that holds the
// seams and the harness and names this repository in its .parity-tree. It
// assembles anew over a tree it assembled, but refuses one that holds seam
// edits the patch lacks, a directory it did not assemble, and a place inside
// the repository outside parity/out, and changes none of them.
func TestBuildGoGuardsTheTree(t *testing.T) {
	bash := requireBash(t)
	ws, err := filepath.EvalSymlinks(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	if ws, err = filepath.Abs(ws); err != nil {
		t.Fatal(err)
	}
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	tree := filepath.Join(base, "tree")
	assemble := func(dir string) (string, int) {
		t.Helper()
		return runScript(t, bash, "build-go.sh", "--no-build", "--tree", dir)
	}

	out, code := assemble(tree)
	if code != 0 {
		t.Fatalf("build-go.sh --no-build: exit code %d; output:\n%s", code, out)
	}
	if lines := strings.Split(strings.TrimSpace(out), "\n"); lines[len(lines)-1] != tree {
		t.Errorf("the last line of output is not the tree %s:\n%s", tree, out)
	}
	marker := readFile(t, filepath.Join(tree, ".parity-tree"))
	for _, want := range []string{"workspace=" + ws + "\n", "upstream=" + pin(t, ws) + "\n"} {
		if !strings.Contains(marker, want) {
			t.Errorf(".parity-tree does not hold %q:\n%s", want, marker)
		}
	}
	if strings.Contains(marker, "seams=none") {
		t.Errorf(".parity-tree records no seams:\n%s", marker)
	}
	for _, p := range []string{"parity/README.md", "parity/corpus/repo.go", "config/parity.go", "go.mod"} {
		if _, err := os.Stat(filepath.Join(tree, filepath.FromSlash(p))); err != nil {
			t.Errorf("the tree has no %s: %v", p, err)
		}
	}
	for _, p := range []string{"parity/scripts", "parity/upstream-seams.patch", "parity/out"} {
		if _, err := os.Stat(filepath.Join(tree, filepath.FromSlash(p))); !errors.Is(err, os.ErrNotExist) {
			t.Errorf("the tree has %s (%v)", p, err)
		}
	}

	if out, code := assemble(tree); code != 0 {
		t.Fatalf("build-go.sh over the tree it assembled: exit code %d; output:\n%s", code, out)
	}

	mainGo := filepath.Join(tree, "main.go")
	edited := readFile(t, mainGo) + "\n// a seam edit\n"
	if err := os.WriteFile(mainGo, []byte(edited), 0o600); err != nil {
		t.Fatal(err)
	}
	out, code = assemble(tree)
	if code == 0 || !strings.Contains(out, "holds seam edits that the patch lacks") {
		t.Errorf("build-go.sh over a tree with a seam edit: exit code %d; output:\n%s", code, out)
	}
	if readFile(t, mainGo) != edited {
		t.Error("build-go.sh changed the tree that holds a seam edit")
	}

	other := filepath.Join(base, "other")
	if err := os.MkdirAll(other, 0o700); err != nil {
		t.Fatal(err)
	}
	keep := filepath.Join(other, "keep")
	if err := os.WriteFile(keep, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	out, code = assemble(other)
	if code == 0 || !strings.Contains(out, "is neither empty nor a tree this script assembled") {
		t.Errorf("build-go.sh over a directory it did not assemble: exit code %d; output:\n%s", code, out)
	}
	if _, err := os.Stat(keep); err != nil {
		t.Errorf("build-go.sh deleted %s: %v", keep, err)
	}

	inside := filepath.Join(ws, "parity", "fixtures", "tree-test")
	out, code = assemble(inside)
	if code == 0 || !strings.Contains(out, "is inside the repository but not under parity/out/") {
		t.Errorf("build-go.sh inside the repository: exit code %d; output:\n%s", code, out)
	}
	if _, err := os.Stat(inside); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("build-go.sh created %s (%v)", inside, err)
	}
}

// TestBuildGoBorrowsTheGoRootsObjects: the tree sees every object of the Go
// root, even one no ref reaches, such as a blob of the previous pin after
// fetch-upstream.sh moved a shallow checkout to a new one: git apply --3way
// needs those blobs to refresh the seams (parity/README.md, "When the pin
// moves"). A clone of a shallow Go root would copy only what its HEAD
// reaches. The Go root is a shallow checkout fetch-upstream.sh makes from the
// one the other tests use, and build-go.sh leaves it as it was. Once that Go
// root is gone, build-go.sh refuses to replace the tree, whose seam edits git
// can no longer tell apart.
func TestBuildGoBorrowsTheGoRootsObjects(t *testing.T) {
	bash := requireBash(t)
	ws, err := filepath.Abs(filepath.Join("..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	reference := os.Getenv("PBS_GO_ROOT")
	switch {
	case reference == "":
		reference = filepath.Join(ws, ".upstream")
	case !filepath.IsAbs(reference):
		reference = filepath.Join(ws, reference)
	}
	base, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	goRoot := filepath.Join(base, "upstream")
	out, code := runScriptEnv(t, bash, []string{"PBS_UPSTREAM_REFERENCE=" + reference},
		filepath.Join(ws, "scripts", "fetch-upstream.sh"), "--dir", goRoot)
	if code != 0 {
		t.Fatalf("fetch-upstream.sh --dir %s: exit code %d; output:\n%s", goRoot, code, out)
	}
	if shallow := git(t, goRoot, nil, "rev-parse", "--is-shallow-repository"); shallow != "true" {
		t.Fatalf("fetch-upstream.sh made a checkout that is not shallow (%s)", shallow)
	}
	blob := git(t, goRoot, strings.NewReader("a blob no ref of the Go root reaches\n"), "hash-object", "-w", "--stdin")

	tree := filepath.Join(base, "tree")
	out, code = runScriptEnv(t, bash, []string{"PBS_GO_ROOT=" + goRoot}, "build-go.sh", "--no-build", "--tree", tree)
	if code != 0 {
		t.Fatalf("build-go.sh --no-build with the shallow Go root: exit code %d; output:\n%s", code, out)
	}
	if err := exec.Command("git", "-C", tree, "cat-file", "-e", blob).Run(); err != nil {
		t.Errorf("the tree does not see the blob %s, which no ref of the Go root reaches: %v", blob, err)
	}
	if head := git(t, goRoot, nil, "rev-parse", "HEAD"); head != pin(t, ws) {
		t.Errorf("the Go root moved to %s", head)
	}
	if status := git(t, goRoot, nil, "status", "--porcelain"); status != "" {
		t.Errorf("build-go.sh changed the Go root:\n%s", status)
	}

	if err := os.RemoveAll(goRoot); err != nil {
		t.Fatal(err)
	}
	out, code = runScriptEnv(t, bash, []string{"PBS_GO_ROOT=" + reference}, "build-go.sh", "--no-build", "--tree", tree)
	if code == 0 || !strings.Contains(out, "borrows objects its Go root no longer has") {
		t.Errorf("build-go.sh over a tree whose Go root is gone: exit code %d; output:\n%s", code, out)
	}
	if _, err := os.Stat(filepath.Join(tree, "main.go")); err != nil {
		t.Errorf("build-go.sh emptied the tree whose Go root is gone: %v", err)
	}
}

// git runs git in dir with stdin and returns its trimmed output, failing the
// test when git fails.
func git(t *testing.T, dir string, stdin *strings.Reader, args ...string) string {
	t.Helper()
	cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
	if stdin != nil {
		cmd.Stdin = stdin
	}
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("git %v in %s: %v\n%s", args, dir, err, stderr.String())
	}
	return strings.TrimSpace(string(out))
}

// staleContext returns the patch with its first context line changed, so it
// no longer applies.
func staleContext(t *testing.T, patch []byte) []byte {
	t.Helper()
	lines := strings.SplitAfter(string(patch), "\n")
	inHunk := false
	for i, line := range lines {
		switch {
		case strings.HasPrefix(line, "diff --git "):
			inHunk = false
		case strings.HasPrefix(line, "@@ "):
			inHunk = true
		case inHunk && strings.HasPrefix(line, " ") && strings.TrimSpace(line) != "":
			lines[i] = " // a line upstream does not have\n"
			return []byte(strings.Join(lines, ""))
		}
	}
	t.Fatal("the patch has no context line")
	return nil
}

// pin returns the commit UPSTREAM_SYNC pins.
func pin(t *testing.T, ws string) string {
	t.Helper()
	for _, line := range strings.Split(readFile(t, filepath.Join(ws, "UPSTREAM_SYNC")), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "#") {
			return line
		}
	}
	t.Fatal("UPSTREAM_SYNC pins no commit")
	return ""
}

// runScript runs a script of this directory with bash, $TMPDIR a fresh
// directory that it must leave empty, and every proxy variable on a closed
// loopback port. It returns the script's output and exit code.
func runScript(t *testing.T, bash string, args ...string) (string, int) {
	t.Helper()
	return runScriptEnv(t, bash, nil, args...)
}

// runScriptEnv is runScript with the NAME=VALUE pairs of extra set in the
// script's environment, in place of any value inherited.
func runScriptEnv(t *testing.T, bash string, extra []string, args ...string) (string, int) {
	t.Helper()
	tmp := t.TempDir()
	drop := map[string]bool{"TMPDIR": true, "HTTP_PROXY": true, "HTTPS_PROXY": true, "NO_PROXY": true}
	for _, kv := range extra {
		name, _, _ := strings.Cut(kv, "=")
		drop[strings.ToUpper(name)] = true
	}
	var env []string
	for _, kv := range os.Environ() {
		name, _, _ := strings.Cut(kv, "=")
		if drop[strings.ToUpper(name)] {
			continue
		}
		env = append(env, kv)
	}
	env = append(env, extra...)
	env = append(env, "TMPDIR="+tmp,
		"HTTP_PROXY=http://127.0.0.1:9", "HTTPS_PROXY=http://127.0.0.1:9", "NO_PROXY=127.0.0.1,localhost",
		"http_proxy=http://127.0.0.1:9", "https_proxy=http://127.0.0.1:9", "no_proxy=127.0.0.1,localhost",
	)
	cmd := exec.Command(bash, args...)
	cmd.Env = env
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
	code := 0
	if err := cmd.Run(); err != nil {
		var exit *exec.ExitError
		if !errors.As(err, &exit) {
			t.Fatal(err)
		}
		code = exit.ExitCode()
	}
	if left := readDir(t, tmp); len(left) != 0 {
		t.Errorf("%v left %v in $TMPDIR", args, left)
	}
	return out.String(), code
}

// requireBash returns bash, or skips the test where the scripts cannot run.
func requireBash(t *testing.T) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the scripts need a POSIX shell")
	}
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash on PATH")
	}
	return bash
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}
