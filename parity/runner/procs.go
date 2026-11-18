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
	// serverPorts is the number of listeners of one server: main, admin
	// and Prometheus.
	serverPorts = 3
	// startAttempts bounds the starts of one server whose ports another
	// process took before it bound them (StartServer).
	startAttempts = 4
	// leasedRetries bounds the ports one reservation lets go because
	// running servers of this process were given them (portLeases).
	leasedRetries = 64
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

// errExited marks the error of a server that exited before it answered
// health.
var errExited = errors.New("exited before answering health")

// StartServer starts one server and waits until /__parity/health answers or
// the process exits. A server that answers anything other than a valid health
// document is running but not parity-capable (Health nil, NotInstrumented
// set). An error means the process could not be started or never answered.
//
// The server binds the ports freePorts reserved for it, whose listeners
// close before the server starts, so another process can bind one of them
// first. A server that exits before answering health with a bind failure on
// one of its ports in its log (bindCollision) is therefore started again on
// freshly reserved ports, up to startAttempts starts in all; when the last
// start fails too, its error names every attempt. Any other failure is
// returned at once, as the only attempt's error or after the earlier ones.
func StartServer(ctx context.Context, spec ServerSpec) (*Server, error) {
	return startServer(ctx, spec, freePorts)
}

// startServer is StartServer with the port reservation given: reserve(n)
// returns n distinct ports for one attempt.
func startServer(ctx context.Context, spec ServerSpec, reserve func(n int) ([]int, error)) (*Server, error) {
	var tries []error
	for attempt := 1; ; attempt++ {
		ports, err := reserve(serverPorts)
		if err != nil {
			return nil, startError(spec, tries, err, fmt.Errorf("attempt %d: %w", attempt, err))
		}
		s, err := startOnce(ctx, spec, ports)
		if err == nil {
			return s, nil
		}
		try := fmt.Errorf("attempt %d on ports %s: %w", attempt, joinInts(ports), err)
		port, collided := 0, false
		if errors.Is(err, errExited) {
			port, collided = bindCollision(readLog(spec.LogPath), ports)
		}
		if collided {
			try = fmt.Errorf("attempt %d on ports %s, port %d taken before the server bound it: %w", attempt, joinInts(ports), port, err)
		}
		if !collided || attempt == startAttempts || ctx.Err() != nil {
			return nil, startError(spec, tries, err, try)
		}
		tries = append(tries, try)
	}
}

// startError is the error of a start whose last attempt failed with err,
// described as try: err itself when it was the first attempt, else every
// attempt, the earlier ones (tries) first.
func startError(spec ServerSpec, tries []error, err, try error) error {
	if len(tries) == 0 {
		return err
	}
	return fmt.Errorf("runner: %s did not start in %d attempts:\n%w", spec.Impl, len(tries)+1, errors.Join(append(tries, try)...))
}

// startOnce starts one server on ports (main, admin, Prometheus) and waits
// for its health. On failure the server is stopped and its ports released.
func startOnce(ctx context.Context, spec ServerSpec, ports []int) (*Server, error) {
	s := &Server{Spec: spec, Ports: Ports{Main: ports[0], Admin: ports[1], Prometheus: ports[2]}, done: make(chan struct{})}
	if err := s.start(); err != nil {
		releasePorts(ports...)
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
// the start timeout passes. A health call in flight ends when the process
// exits: on a port another process listens on without answering, it would
// otherwise last until the timeout.
func (s *Server) waitHealthy(ctx context.Context) error {
	timeout := s.Spec.StartTimeout
	if timeout <= 0 {
		timeout = defaultStartTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	call, endCalls := context.WithCancel(ctx)
	defer endCalls()
	go func() {
		select {
		case <-s.done:
			endCalls()
		case <-call.Done():
		}
	}()
	tick := time.NewTicker(healthPoll)
	defer tick.Stop()
	for {
		h, err := s.Client.Health(call)
		switch {
		case err == nil:
			s.Health = h
			return nil
		case IsNotInstrumented(err), isAnswered(err):
			s.NotInstrumented = fmt.Sprintf("%s health: %v", s.Spec.Impl, err)
			return nil
		case s.Exited():
			return s.exitedError()
		}
		select {
		case <-s.done:
			return s.exitedError()
		case <-ctx.Done():
			return fmt.Errorf("runner: %s did not answer health within %s: %w", s.Spec.Impl, timeout, err)
		case <-tick.C:
		}
	}
}

// exitedError is the error of a server that exited before it answered
// health.
func (s *Server) exitedError() error {
	return fmt.Errorf("runner: %s %w (%v): %s", s.Spec.Impl, errExited, s.exitErr(), s.LogTail())
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
// log, deletes TempDir and releases the ports. It is safe to call more than
// once.
func (s *Server) Stop() error {
	if s.cancel == nil {
		return nil
	}
	s.cancel()
	<-s.done
	s.cancel = nil
	releasePorts(s.Ports.Main, s.Ports.Admin, s.Ports.Prometheus)
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

// readLog returns the server log at path, nil when it cannot be read.
func readLog(path string) []byte {
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	return raw
}

// addrInUse is the text of EADDRINUSE in Go's errno tables, on Linux
// (src/syscall/zerrors_linux_amd64.go:1453, zerrors_linux_arm64.go:1729 in
// Go 1.26.2) as on macOS (zerrors_darwin_arm64.go:1340).
const addrInUse = "address already in use"

// bindCollision returns the first of ports that a line of log could not
// bind because the port was in use. Go's net.Listen fails with "listen tcp
// 127.0.0.1:PORT: bind: address already in use" (net.OpError.Error over the
// bind os.SyscallError of listenStream: src/net/net.go:501-521,
// src/net/sock_posix.go:167-168 and src/os/error.go:54 in Go 1.26.2), which
// the Go server logs from server.Listen and pbs-gohttp-server's
// Listener::bind_tcp writes alike. Case is ignored, so the Rust standard
// library's "Address already in use (os error 98)" counts too, and a port
// counts only where the line has ":PORT" without a digit after it.
func bindCollision(log []byte, ports []int) (int, bool) {
	for _, line := range strings.Split(string(log), "\n") {
		if !strings.Contains(strings.ToLower(line), addrInUse) {
			continue
		}
		for _, port := range ports {
			if mentionsPort(line, port) {
				return port, true
			}
		}
	}
	return 0, false
}

// mentionsPort reports whether line has ":PORT" without a digit after it.
func mentionsPort(line string, port int) bool {
	needle := ":" + strconv.Itoa(port)
	for rest := line; ; {
		i := strings.Index(rest, needle)
		if i < 0 {
			return false
		}
		rest = rest[i+len(needle):]
		if rest == "" || rest[0] < '0' || rest[0] > '9' {
			return true
		}
	}
}

// joinInts formats ports as "1 2 3".
func joinInts(ports []int) string {
	out := make([]string, len(ports))
	for i, p := range ports {
		out[i] = strconv.Itoa(p)
	}
	return strings.Join(out, " ")
}

// leases are the ports freePorts gave the servers of this process that have
// not stopped. A reserved port is free for the kernel to hand out again from
// the moment its listener closes until its server binds it, which the Go
// server does only after its whole setup. Without the leases, the servers
// that start together (both sides of every shard) could be given the same
// port, and the one that binds it would then answer the other's health
// check.
var leases = portLeases{leased: map[int]bool{}, listen: listenLoopback}

// portLeases hands out loopback ports and remembers them until released.
type portLeases struct {
	mu     sync.Mutex
	leased map[int]bool
	// listen opens a listener on a loopback port the kernel picks.
	listen func() (net.Listener, error)
}

func listenLoopback() (net.Listener, error) { return net.Listen("tcp", "127.0.0.1:0") }

// freePorts reserves n distinct loopback ports for a server and leases them
// until releasePorts. The listeners are closed before the server binds the
// ports: one that another process binds in between shows up as the server's
// bind failure, which StartServer answers with fresh ports.
func freePorts(n int) ([]int, error) { return leases.reserve(n) }

// releasePorts ends the leases of ports.
func releasePorts(ports ...int) { leases.release(ports...) }

// reserve returns n distinct ports, none of them leased, and leases them. It
// holds a listener on each until it returns, so the kernel cannot pick one
// twice; a leased port the kernel picks is let go at once, since its server
// may be about to bind it.
func (l *portLeases) reserve(n int) ([]int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	out := make([]int, 0, n)
	var lns []net.Listener
	defer func() {
		for _, ln := range lns {
			ln.Close()
		}
	}()
	for skipped := 0; len(out) < n; {
		ln, err := l.listen()
		if err != nil {
			return nil, fmt.Errorf("runner: reserving a port: %w", err)
		}
		addr, ok := ln.Addr().(*net.TCPAddr)
		if !ok {
			ln.Close()
			return nil, errors.New("runner: listener has no TCP address")
		}
		if l.leased[addr.Port] {
			ln.Close()
			if skipped++; skipped > leasedRetries {
				return nil, fmt.Errorf("runner: reserving a port: the kernel picked %d ports leased to running servers", skipped)
			}
			continue
		}
		lns = append(lns, ln)
		out = append(out, addr.Port)
	}
	for _, p := range out {
		l.leased[p] = true
	}
	return out, nil
}

// release ends the leases of ports.
func (l *portLeases) release(ports ...int) {
	l.mu.Lock()
	defer l.mu.Unlock()
	for _, p := range ports {
		delete(l.leased, p)
	}
}

// BuildGo builds the Go server binary at out from the repository root, the
// assembled tree (parity/scripts/build-go.sh builds the same way): without a
// go.work, and without VCS stamping, since the tree is a scratch repository
// with nothing to stamp a binary with. An out inside the parity/out of the
// tree's harness root is kept out of git there.
func BuildGo(ctx context.Context, repoRoot, out string) error {
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		return fmt.Errorf("runner: creating %s: %w", filepath.Dir(out), err)
	}
	ignoreOutputRoot(repoRoot, out)
	if _, harness, err := resolveRoots(repoRoot, ""); err == nil {
		ignoreOutputRoot(harness, out)
	}
	cmd := exec.CommandContext(ctx, "go", "build", "-buildvcs=false", "-o", out, ".")
	cmd.Dir = repoRoot
	cmd.Env = append(os.Environ(), "GOWORK=off")
	var stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stderr, &stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("runner: go build: %w: %s", err, strings.TrimSpace(stderr.String()))
	}
	return nil
}
