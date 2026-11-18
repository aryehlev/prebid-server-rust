package scripts

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// stub stands in for both go and the ReplayHarness CLI. Whatever it is asked,
// it first fills every state directory the scripts made under $TMPDIR the way
// rhtool serve fills its stack directory: the parity token file, state.env,
// the outbound log and a server log. With STUB_HOLD set, it then creates
// $STUB_HOLD.started and waits for $STUB_HOLD, so a test can signal a script
// in the middle of a step, and succeeds. Otherwise, as go it fails, so the
// script's first build fails (build-go.sh runs go version before anything
// else), and as replayharness it prints a version.
const stub = `#!/bin/sh
for d in "$TMPDIR"/replayharness.* "$TMPDIR"/replayharness-replay.*; do
  [ -d "$d" ] || continue
  mkdir -p "$d/stack"
  printf '%s' "$STUB_TOKEN" >"$d/stack/token"
  echo "RH_TOKEN_FILE=$d/stack/token" >"$d/stack/state.env"
  : >"$d/stack/outbound.jsonl"
  : >"$d/stack/server-0.log"
done
if [ -n "$STUB_HOLD" ]; then
  : >"$STUB_HOLD.started"
  i=0
  while [ ! -e "$STUB_HOLD" ] && [ "$i" -lt 600 ]; do
    sleep 0.05
    i=$((i + 1))
  done
  exit 0
fi
if [ "$(basename "$0")" = go ]; then
  exit 1
fi
echo "replayharness 0.0.0-stub"
`

// State directory prefixes under $TMPDIR, and the reports directory the main
// script makes when --out is not given.
const (
	statePrefix       = "replayharness."
	replayStatePrefix = "replayharness-replay."
	outPrefix         = "replayharness-out."
)

// TestScriptsDeleteTheirState: replayharness.sh and replayharness-replay.sh
// delete their state directory, which holds the parity token file and rhtool
// serve's state.env, outbound log and server logs, when they succeed, fail or
// get SIGTERM, unless --keep-state is given. The reports stay outside it, and
// neither script prints the token.
func TestScriptsDeleteTheirState(t *testing.T) {
	bash := requireBash(t)
	tests := []struct {
		name   string
		script string
		args   []string
		out    bool // pass --out
		term   bool // send SIGTERM while the stub holds
		keep   bool // --keep-state is in args
		code   int
	}{
		{"replayharness.sh cli succeeds", "replayharness.sh", []string{"cli"}, true, false, false, 0},
		{"replayharness.sh keeps the reports apart", "replayharness.sh", []string{"cli"}, false, false, false, 0},
		{"replayharness.sh fails a build", "replayharness.sh", []string{"parity"}, true, false, false, 1},
		{"replayharness.sh gets SIGTERM", "replayharness.sh", []string{"cli"}, true, true, false, 130},
		{"replayharness.sh with --keep-state", "replayharness.sh", []string{"parity", "--keep-state"}, true, false, true, 1},
		{"replayharness-replay.sh fails a build", "replayharness-replay.sh", nil, true, false, false, 1},
		{"replayharness-replay.sh gets SIGTERM", "replayharness-replay.sh", nil, true, true, false, 130},
		{"replayharness-replay.sh with --keep-state", "replayharness-replay.sh", []string{"--keep-state"}, true, false, true, 1},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			tmp, bin, hold := t.TempDir(), t.TempDir(), filepath.Join(t.TempDir(), "hold")
			t.Cleanup(func() { _ = os.WriteFile(hold, nil, 0o600) })
			for _, name := range []string{"go", "replayharness"} {
				if err := os.WriteFile(filepath.Join(bin, name), []byte(stub), 0o755); err != nil {
					t.Fatal(err)
				}
			}
			token := newToken(t)
			args := append([]string{tc.script}, tc.args...)
			out := filepath.Join(t.TempDir(), "out")
			if tc.out {
				args = append(args, "--out", out)
			}
			cmd := exec.Command(bash, args...)
			cmd.Env = scriptEnv(tmp, bin, token, tc.term, hold)
			var stdout, stderr bytes.Buffer
			cmd.Stdout, cmd.Stderr = &stdout, &stderr
			cmd.WaitDelay = 10 * time.Second
			if err := cmd.Start(); err != nil {
				t.Fatal(err)
			}
			if tc.term {
				waitFor(t, hold+".started")
				if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(hold, nil, 0o600); err != nil {
					t.Fatal(err)
				}
			}
			code := 0
			if err := cmd.Wait(); err != nil {
				var exit *exec.ExitError
				if !errors.As(err, &exit) {
					t.Fatal(err)
				}
				code = exit.ExitCode()
			}
			output := stdout.String() + stderr.String()
			if code != tc.code {
				t.Errorf("exit code %d, want %d; output:\n%s", code, tc.code, output)
			}
			if strings.Contains(output, token) {
				t.Errorf("the output prints the parity token:\n%s", output)
			}
			checkState(t, tmp, tc.script, tc.keep, output)
			checkReports(t, tmp, out, tc.out, output)
		})
	}
}

// scriptEnv is the environment of a script under test: the stubs first on
// PATH, $TMPDIR a fresh directory, and every proxy variable on a closed
// loopback port.
func scriptEnv(tmp, bin, token string, term bool, hold string) []string {
	var env []string
	for _, kv := range os.Environ() {
		name, _, _ := strings.Cut(kv, "=")
		switch strings.ToUpper(name) {
		case "PATH", "TMPDIR", "REPLAYHARNESS", "REPLAYHARNESS_SRC", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "STUB_HOLD", "STUB_TOKEN":
			continue
		}
		env = append(env, kv)
	}
	env = append(env,
		"PATH="+bin+string(os.PathListSeparator)+os.Getenv("PATH"),
		"TMPDIR="+tmp,
		"REPLAYHARNESS="+filepath.Join(bin, "replayharness"),
		"STUB_TOKEN="+token,
		"HTTP_PROXY=http://127.0.0.1:9", "HTTPS_PROXY=http://127.0.0.1:9", "NO_PROXY=127.0.0.1,localhost",
		"http_proxy=http://127.0.0.1:9", "https_proxy=http://127.0.0.1:9", "no_proxy=127.0.0.1,localhost",
	)
	if term {
		env = append(env, "STUB_HOLD="+hold)
	}
	return env
}

// checkState checks what is left of the script's state directory under tmp:
// nothing, or with --keep-state the directory with the stack files and
// without the builds (bin/, and tree/, the tree build-go.sh assembles).
func checkState(t *testing.T, tmp, script string, keep bool, output string) {
	t.Helper()
	prefix := statePrefix
	if script == "replayharness-replay.sh" {
		prefix = replayStatePrefix
	}
	var states []string
	for _, e := range readDir(t, tmp) {
		if strings.HasPrefix(e, prefix) {
			states = append(states, e)
		}
	}
	if !keep {
		if len(states) != 0 {
			t.Errorf("state directories left in $TMPDIR: %v", states)
		}
		return
	}
	if len(states) != 1 {
		t.Fatalf("state directories in $TMPDIR: %v, want one", states)
	}
	state := filepath.Join(tmp, states[0])
	if !strings.Contains(output, "state kept in "+state) {
		t.Errorf("the output does not name the kept state %s:\n%s", state, output)
	}
	for _, f := range []string{"token", "state.env", "outbound.jsonl", "server-0.log"} {
		if _, err := os.Stat(filepath.Join(state, "stack", f)); err != nil {
			t.Errorf("--keep-state did not keep stack/%s: %v", f, err)
		}
	}
	for _, f := range []string{"bin", "tree"} {
		if _, err := os.Stat(filepath.Join(state, f)); !errors.Is(err, os.ErrNotExist) {
			t.Errorf("--keep-state kept the build %s (%v)", f, err)
		}
	}
}

// checkReports checks that the reports directory outlives the state: --out,
// or else the one new directory the main script made under tmp.
func checkReports(t *testing.T, tmp, out string, given bool, output string) {
	t.Helper()
	if given {
		if _, err := os.Stat(out); err != nil {
			t.Errorf("the reports directory --out %s is gone: %v", out, err)
		}
		return
	}
	var reports []string
	for _, e := range readDir(t, tmp) {
		if strings.HasPrefix(e, outPrefix) {
			reports = append(reports, e)
		}
	}
	if len(reports) != 1 {
		t.Fatalf("reports directories in $TMPDIR: %v, want one", reports)
	}
	if want := "reports in " + filepath.Join(tmp, reports[0]); !strings.Contains(output, want) {
		t.Errorf("the output does not say %q:\n%s", want, output)
	}
}

func readDir(t *testing.T, dir string) []string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		names = append(names, e.Name())
	}
	return names
}

// waitFor waits until the file exists.
func waitFor(t *testing.T, file string) {
	t.Helper()
	deadline := time.Now().Add(30 * time.Second)
	for {
		if _, err := os.Stat(file); err == nil {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s did not appear within 30s", file)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func newToken(t *testing.T) string {
	t.Helper()
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		t.Fatal(err)
	}
	return hex.EncodeToString(b)
}
