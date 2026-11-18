package runner

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
)

// Process defaults.
const (
	defaultStartTimeout = 90 * time.Second
	stopGrace           = 10 * time.Second
	healthPoll          = 100 * time.Millisecond
	logTailBytes        = 2048
)

// ServerSpec describes one server process.
type ServerSpec struct {
	// Impl is the implementation name the server reports (parity.ImplGo or
	// parity.ImplRust); it keys the oracle captures of its calls.
	Impl   string
	Binary string
	// WorkDir holds pbs.yaml, static/ and stored/.
	WorkDir string
	// LogPath receives stdout and stderr.
	LogPath string
	// Token is parity.token (sent as PBS_PARITY_TOKEN).
	Token string
	// StartTimeout bounds the health wait; <= 0 means 90s.
	StartTimeout time.Duration
	// ExtraEnv are additional KEY=VALUE entries.
	ExtraEnv []string
}

// TempDir is the server's TMPDIR: LogPath with the extension ".tmp". The
// server starts with it empty, and Stop deletes it. Go's glog writes a file
// per log level to the temporary directory, every INFO line included,
// whatever the server log gets; with the inherited TMPDIR those files
// stayed behind after every run, outside its directory.
func (s ServerSpec) TempDir() string {
	return strings.TrimSuffix(s.LogPath, filepath.Ext(s.LogPath)) + ".tmp"
}

// Ports are the listeners of one server process.
type Ports struct {
	Main       int `json:"main"`
	Admin      int `json:"admin"`
	Prometheus int `json:"prometheus"`
}

// Server is a running server process.
type Server struct {
	Spec   ServerSpec
	Ports  Ports
	Client *Client
	// Health is the health document; nil when the server is not
	// parity-capable.
	Health *parity.Health
	// NotInstrumented explains why Health is nil.
	NotInstrumented string

	cmd    *exec.Cmd
	cancel context.CancelFunc
	done   chan struct{}
	log    *os.File

	mu      sync.Mutex
	waitErr error
}

// StartServer starts one server and waits until /__parity/health answers or
// the process exits. A server that answers anything other than a valid health
// document is running but not parity-capable (Health nil, NotInstrumented
// set). An error means the process could not be started or never answered.
func StartServer(ctx context.Context, spec ServerSpec) (*Server, error) {
	ports, err := freePorts(3)
	if err != nil {
		return nil, err
	}
	s := &Server{Spec: spec, Ports: Ports{Main: ports[0], Admin: ports[1], Prometheus: ports[2]}, done: make(chan struct{})}
	if err := s.start(); err != nil {
		return nil, err
	}
	s.Client = NewClient(fmt.Sprintf("http://127.0.0.1:%d", s.Ports.Main), spec.Token)
	if err := s.waitHealthy(ctx); err != nil {
		stopErr := s.Stop()
		return nil, errors.Join(err, stopErr)
	}
	return s, nil
}

func (s *Server) start() error {
	if err := os.MkdirAll(filepath.Dir(s.Spec.LogPath), 0o755); err != nil {
		return fmt.Errorf("runner: creating log dir: %w", err)
	}
	tmp := s.Spec.TempDir()
	if err := os.RemoveAll(tmp); err != nil {
		return fmt.Errorf("runner: clearing the server temporary directory: %w", err)
	}
	if err := os.Mkdir(tmp, 0o700); err != nil {
		return fmt.Errorf("runner: creating the server temporary directory: %w", err)
	}
	log, err := os.Create(s.Spec.LogPath)
	if err != nil {
		return errors.Join(fmt.Errorf("runner: creating server log: %w", err), os.RemoveAll(tmp))
	}
	procCtx, cancel := context.WithCancel(context.Background())
	cmd := exec.CommandContext(procCtx, s.Spec.Binary)
	cmd.Dir = s.Spec.WorkDir
	cmd.Env = s.environ()
	cmd.Stdout, cmd.Stderr = log, log
	cmd.Cancel = func() error { return cmd.Process.Signal(os.Interrupt) }
	cmd.WaitDelay = stopGrace
	if err := cmd.Start(); err != nil {
		cancel()
		log.Close()
		return errors.Join(fmt.Errorf("runner: starting %s (%s): %w", s.Spec.Impl, s.Spec.Binary, err), os.RemoveAll(tmp))
	}
	s.cmd, s.cancel, s.log = cmd, cancel, log
	go func() {
		err := cmd.Wait()
		s.mu.Lock()
		s.waitErr = err
		s.mu.Unlock()
		close(s.done)
	}()
	return nil
}

// environ is the inherited environment without PBS_* (so the runner's own
// shell cannot leak config into a server), plus the ports, the token, TZ=UTC
// (huaweiads formats local time), TMPDIR (ServerSpec.TempDir) and, for
// non-Go servers, the config-file shim of risk R10.
func (s *Server) environ() []string {
	var env []string
	for _, kv := range os.Environ() {
		if strings.HasPrefix(kv, "PBS_") || strings.HasPrefix(kv, "TZ=") || strings.HasPrefix(kv, "TMPDIR=") {
			continue
		}
		env = append(env, kv)
	}
	env = append(env,
		"TZ=UTC",
		"TMPDIR="+s.Spec.TempDir(),
		"PBS_PORT="+strconv.Itoa(s.Ports.Main),
		"PBS_ADMIN_PORT="+strconv.Itoa(s.Ports.Admin),
		"PBS_METRICS_PROMETHEUS_PORT="+strconv.Itoa(s.Ports.Prometheus),
		"PBS_PARITY_TOKEN="+s.Spec.Token,
	)
	if s.Spec.Impl != parity.ImplGo {
		env = append(env,
			"PBS_CONFIG_FILE="+filepath.Join(s.Spec.WorkDir, ConfigFileName),
			"PBS_STATIC_DIR="+filepath.Join(s.Spec.WorkDir, StaticDirName),
		)
	}
	return append(env, s.Spec.ExtraEnv...)
}

// waitHealthy polls /__parity/health until it answers, the process exits or
// the start timeout passes.
func (s *Server) waitHealthy(ctx context.Context) error {
	timeout := s.Spec.StartTimeout
	if timeout <= 0 {
		timeout = defaultStartTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	tick := time.NewTicker(healthPoll)
	defer tick.Stop()
	for {
		h, err := s.Client.Health(ctx)
		switch {
		case err == nil:
			s.Health = h
			return nil
		case IsNotInstrumented(err), isAnswered(err):
			s.NotInstrumented = fmt.Sprintf("%s health: %v", s.Spec.Impl, err)
			return nil
		}
		select {
		case <-s.done:
			return fmt.Errorf("runner: %s exited before answering health (%v): %s", s.Spec.Impl, s.exitErr(), s.LogTail())
		case <-ctx.Done():
			return fmt.Errorf("runner: %s did not answer health within %s: %w", s.Spec.Impl, timeout, err)
		case <-tick.C:
		}
	}
}

// Exited reports whether the process has exited.
func (s *Server) Exited() bool {
	select {
	case <-s.done:
		return true
	default:
		return false
	}
}

func (s *Server) exitErr() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.waitErr
}

// Stop interrupts the process, kills it after the grace period, closes the
// log and deletes TempDir. It is safe to call more than once.
func (s *Server) Stop() error {
	if s.cancel == nil {
		return nil
	}
	s.cancel()
	<-s.done
	s.cancel = nil
	if s.Client != nil {
		s.Client.CloseIdle()
	}
	var errs []error
	if err := s.log.Close(); err != nil {
		errs = append(errs, fmt.Errorf("runner: closing %s log: %w", s.Spec.Impl, err))
	}
	if err := os.RemoveAll(s.Spec.TempDir()); err != nil {
		errs = append(errs, fmt.Errorf("runner: deleting the %s temporary directory: %w", s.Spec.Impl, err))
	}
	return errors.Join(errs...)
}

// LogTail returns the end of the server log.
func (s *Server) LogTail() string {
	raw, err := os.ReadFile(s.Spec.LogPath)
	if err != nil {
		return ""
	}
	if len(raw) > logTailBytes {
		raw = raw[len(raw)-logTailBytes:]
	}
	return strings.TrimSpace(string(bytes.ToValidUTF8(raw, nil)))
}

// freePorts reserves n distinct loopback ports. The listeners are closed
// before the server binds them; a lost race shows up as a start failure.
func freePorts(n int) ([]int, error) {
	out := make([]int, 0, n)
	var lns []net.Listener
	defer func() {
		for _, ln := range lns {
			ln.Close()
		}
	}()
	for i := 0; i < n; i++ {
		ln, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			return nil, fmt.Errorf("runner: reserving a port: %w", err)
		}
		lns = append(lns, ln)
		addr, ok := ln.Addr().(*net.TCPAddr)
		if !ok {
			return nil, errors.New("runner: listener has no TCP address")
		}
		out = append(out, addr.Port)
	}
	return out, nil
}

// BuildGo builds the Go server binary at out from the repository root.
func BuildGo(ctx context.Context, repoRoot, out string) error {
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		return fmt.Errorf("runner: creating %s: %w", filepath.Dir(out), err)
	}
	ignoreOutputRoot(repoRoot, out)
	cmd := exec.CommandContext(ctx, "go", "build", "-o", out, ".")
	cmd.Dir = repoRoot
	var stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stderr, &stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("runner: go build: %w: %s", err, strings.TrimSpace(stderr.String()))
	}
	return nil
}
