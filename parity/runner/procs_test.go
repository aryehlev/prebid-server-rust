package runner

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
)

// fakeSpec is the spec of a fake server in its own work directory, whose
// pbs.yaml names an oracle nothing listens on (the start tests make no
// outbound call).
func fakeSpec(t *testing.T, impl Impl) ServerSpec {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, ConfigFileName), []byte("parity:\n  oracle_url: http://127.0.0.1:9\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return ServerSpec{
		Impl: impl.Name, Binary: impl.Binary, WorkDir: dir, LogPath: filepath.Join(dir, "server.log"),
		Token: "token", StartTimeout: 30 * time.Second, ExtraEnv: impl.Env,
	}
}

// holdPort listens on a loopback port until the test ends.
func holdPort(t *testing.T) int {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	return ln.Addr().(*net.TCPAddr).Port
}

// takenFirst is a port reservation whose first taken calls give the main
// port as taken, with freshly reserved admin and Prometheus ports, and whose
// later calls reserve every port afresh. calls counts the calls.
func takenFirst(taken, port int, calls *int) func(int) ([]int, error) {
	return func(n int) ([]int, error) {
		*calls++
		if *calls > taken {
			return freePorts(n)
		}
		rest, err := freePorts(n - 1)
		return append([]int{port}, rest...), err
	}
}

// TestStartServerRetriesATakenPort: a server whose main port another process
// bound after the reservation exits with a bind failure on it, and is started
// again on fresh ports, where it answers health. The other process accepts
// connections on the port and never answers, so the health call in flight
// must end when the server exits, not at the start timeout.
func TestStartServerRetriesATakenPort(t *testing.T) {
	taken := holdPort(t)
	calls := 0
	spec := fakeSpec(t, fakeImpl(t, parity.ImplGo, fakeGo))
	began := time.Now()
	srv, err := startServer(context.Background(), spec, takenFirst(1, taken, &calls))
	if err != nil {
		t.Fatalf("startServer: %v", err)
	}
	if took := time.Since(began); took >= spec.StartTimeout/2 {
		t.Errorf("the start took %s: the first attempt waited for its health call", took)
	}
	defer func() {
		if err := srv.Stop(); err != nil {
			t.Error(err)
		}
	}()
	if calls != 2 {
		t.Errorf("the ports were reserved %d times, want 2", calls)
	}
	if srv.Health == nil || srv.Health.Impl != parity.ImplGo {
		t.Errorf("health = %+v (%s)", srv.Health, srv.NotInstrumented)
	}
	if srv.Ports.Main == taken {
		t.Errorf("the server kept the taken port %d", taken)
	}
}

// TestStartServerNamesEveryAttempt: when every attempt finds a port taken,
// the start gives up after startAttempts and its error names each attempt
// and its bind failure.
func TestStartServerNamesEveryAttempt(t *testing.T) {
	taken := holdPort(t)
	calls := 0
	_, err := startServer(context.Background(), fakeSpec(t, fakeImpl(t, parity.ImplRust, fakeGo)), takenFirst(startAttempts, taken, &calls))
	if err == nil {
		t.Fatal("startServer started a server on a taken port")
	}
	if calls != startAttempts {
		t.Errorf("the ports were reserved %d times, want %d", calls, startAttempts)
	}
	msg := err.Error()
	wants := []string{fmt.Sprintf("runner: rust did not start in %d attempts:\n", startAttempts)}
	for i := 1; i <= startAttempts; i++ {
		wants = append(wants, fmt.Sprintf("attempt %d on ports %d ", i, taken))
	}
	wants = append(wants, fmt.Sprintf("port %d taken before the server bound it", taken))
	for _, want := range wants {
		if !strings.Contains(msg, want) {
			t.Errorf("error lacks %q:\n%s", want, msg)
		}
	}
	if got, want := strings.Count(msg, "127.0.0.1:"+strconv.Itoa(taken)+": bind: "+addrInUse), startAttempts; got != want {
		t.Errorf("error has %d bind failures, want %d:\n%s", got, want, msg)
	}
	if !errors.Is(err, errExited) {
		t.Errorf("error does not wrap errExited: %v", err)
	}
}

// TestStartServerDoesNotRetryOtherFailures: a server that fails for any other
// reason, a bind failure on a port that is not its own included, is started
// once, and the error is that start's.
func TestStartServerDoesNotRetryOtherFailures(t *testing.T) {
	tests := []struct {
		name    string
		impl    func(t *testing.T) Impl
		timeout time.Duration
		want    []string
	}{
		{
			name: "exits without binding",
			impl: func(t *testing.T) Impl { return fakeImpl(t, parity.ImplGo, fakeExit) },
			want: []string{"runner: go exited before answering health (exit status 1)", "fake server: exiting before it listens"},
		},
		{
			name: "bind failure on another port",
			impl: func(t *testing.T) Impl {
				impl := fakeImpl(t, parity.ImplGo, fakeGo)
				impl.Env = append(impl.Env, fakeListenEnv+"=127.0.0.1:"+strconv.Itoa(holdPort(t)))
				return impl
			},
			want: []string{"runner: go exited before answering health (exit status 1)", ": bind: " + addrInUse},
		},
		{
			name: "no health answer",
			impl: func(t *testing.T) Impl {
				impl := fakeImpl(t, parity.ImplGo, fakeGo)
				impl.Env = append(impl.Env, fakeListenEnv+"=127.0.0.1:0")
				return impl
			},
			timeout: time.Second,
			want:    []string{"runner: go did not answer health within 1s"},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			spec := fakeSpec(t, tc.impl(t))
			if tc.timeout > 0 {
				spec.StartTimeout = tc.timeout
			}
			calls := 0
			_, err := startServer(context.Background(), spec, func(n int) ([]int, error) {
				calls++
				return freePorts(n)
			})
			if err == nil {
				t.Fatal("startServer succeeded")
			}
			if calls != 1 {
				t.Errorf("the ports were reserved %d times, want 1", calls)
			}
			for _, want := range tc.want {
				if !strings.Contains(err.Error(), want) {
					t.Errorf("error lacks %q:\n%v", want, err)
				}
			}
			if strings.Contains(err.Error(), "attempt") {
				t.Errorf("a single start's error names attempts:\n%v", err)
			}
		})
	}
}

// TestBindCollision: a log line counts when it says the address is in use
// and names one of the ports as ":PORT" with no digit after it.
func TestBindCollision(t *testing.T) {
	ports := []int{38207, 40001, 40003}
	tests := []struct {
		name string
		log  string
		want int
	}{
		{"go net.Listen", "fake server: listen tcp 127.0.0.1:38207: bind: address already in use", 38207},
		// The Go server built from the pinned upstream, its main port taken.
		{"go server", "E1118 14:09:57.256516   98883 logger.go:22] Error listening for TCP connections on 127.0.0.1:40003: Error listening for TCP connections on 127.0.0.1:40003: listen tcp 127.0.0.1:40003: bind: address already in use for main server", 40003},
		{"among other lines", "starting\nlisten tcp 127.0.0.1:40001: bind: address already in use\nstack", 40001},
		{"rust std", "Error: bind 127.0.0.1:40001: Address already in use (os error 98)", 40001},
		{"ipv6", "listen tcp [::1]:40003: bind: address already in use", 40003},
		{"another port", "listen tcp 127.0.0.1:9: bind: address already in use", 0},
		{"a longer port", "listen tcp 127.0.0.1:382071: bind: address already in use", 0},
		{"no port", "Error: Address already in use (os error 48)", 0},
		{"port without the error", "listen tcp 127.0.0.1:38207: bind: permission denied", 0},
		{"port and error on different lines", "listening on 127.0.0.1:38207\naddress already in use", 0},
		{"empty", "", 0},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			port, ok := bindCollision([]byte(tc.log), ports)
			if port != tc.want || ok != (tc.want != 0) {
				t.Errorf("bindCollision = %d, %v; want %d", port, ok, tc.want)
			}
		})
	}
}

// TestBindCollisionReadsTheListenError: the error net.Listen gives on this
// operating system for a port in use is one bindCollision recognizes. CI runs
// it on Linux, the system whose text the expectation was read from the Go
// source for (addrInUse).
func TestBindCollisionReadsTheListenError(t *testing.T) {
	port := holdPort(t)
	ln, err := net.Listen("tcp", "127.0.0.1:"+strconv.Itoa(port))
	if err == nil {
		ln.Close()
		t.Fatalf("listened twice on port %d", port)
	}
	if got, ok := bindCollision([]byte("fake server: "+err.Error()+"\n"), []int{port}); !ok || got != port {
		t.Errorf("bindCollision(%q) = %d, %v", err, got, ok)
	}
	if want := "listen tcp 127.0.0.1:" + strconv.Itoa(port) + ": bind: " + addrInUse; err.Error() != want {
		t.Errorf("net.Listen error = %q, want %q", err, want)
	}
}

// TestFreePortsSkipsLeasedPorts: a port leased to a running server is not
// reserved again until it is released, even when the kernel picks it.
func TestFreePortsSkipsLeasedPorts(t *testing.T) {
	l := portLeases{leased: map[int]bool{}, listen: listenLoopback}
	first, err := l.reserve(1)
	if err != nil {
		t.Fatal(err)
	}
	leased := first[0]
	// The kernel picks the leased port first, then any.
	picks := 0
	l.listen = func() (net.Listener, error) {
		picks++
		if picks == 1 {
			return net.Listen("tcp", "127.0.0.1:"+strconv.Itoa(leased))
		}
		return listenLoopback()
	}
	got, err := l.reserve(2)
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got[0] == leased || got[1] == leased || got[0] == got[1] {
		t.Errorf("reserve(2) = %v with %d leased", got, leased)
	}
	if picks < 3 {
		t.Errorf("the kernel was asked %d times, want at least 3", picks)
	}
	l.release(leased)
	picks = 0
	again, err := l.reserve(1)
	if err != nil {
		t.Fatal(err)
	}
	if len(again) != 1 || again[0] != leased {
		t.Errorf("after the release reserve(1) = %v, want [%d]", again, leased)
	}
}

// TestFreePortsGivesUpOnLeasedPorts: a kernel that only ever picks leased
// ports ends the reservation with an error instead of a loop.
func TestFreePortsGivesUpOnLeasedPorts(t *testing.T) {
	l := portLeases{leased: map[int]bool{}, listen: listenLoopback}
	first, err := l.reserve(1)
	if err != nil {
		t.Fatal(err)
	}
	l.listen = func() (net.Listener, error) { return net.Listen("tcp", "127.0.0.1:"+strconv.Itoa(first[0])) }
	if got, err := l.reserve(1); err == nil {
		t.Errorf("reserve(1) = %v", got)
	}
}
