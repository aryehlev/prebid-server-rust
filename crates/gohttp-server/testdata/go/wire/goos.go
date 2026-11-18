package main

import (
	"bytes"
	"fmt"
	"net"
	"runtime"
	"sort"
)

// Some cases answer differently on each GOOS (corpus.addByGOOS marks them). For each of them the
// wire golden holds one expect block per GOOS of goosValues: the generator observes the block of
// the GOOS it runs on, and derives every other block by running the case against a copy of the
// case's server whose connections report what Go reports on that GOOS (goosListener). It first
// derives the block of its own GOOS the same way and fails unless that gives exactly the bytes
// it observed, which checks both the mechanism and this GOOS's values against real Go.
// TestGOOSExpectations checks the block of the GOOS it runs on against real Go; CI's go-tooling
// job runs it on linux, so the derived linux block is checked there.

// goosValue is what Go reports, on one GOOS, for the inputs on which a case's bytes depend.
type goosValue struct {
	// unnamedUnixPeer is http.Request.RemoteAddr of a request on a Unix socket from a client
	// that did not bind a path (runCase dials with net.Dial("unix", path)). In Go 1.26.2,
	// conn.serve sets c.remoteAddr = c.rwc.RemoteAddr().String() (net/http/server.go:1897-1899)
	// and readRequest copies it into req.RemoteAddr (net/http/server.go:1042). For an accepted
	// Unix connection RemoteAddr is the peer address accept returned (net/fd_unix.go:150,
	// :166-167), made by sockaddrToUnix (net/sockaddr_posix.go:47-50) into
	// &UnixAddr{Name: sa.Name} (net/unixsock_posix.go:52-57), whose String is Name
	// (net/unixsock.go:33-38).
	unnamedUnixPeer string
}

// goosValues are the GOOSes the golden has expect blocks for, each from Go 1.26.2's source.
var goosValues = map[string]goosValue{
	// accept is syscall.Accept (internal/poll/sys_cloexec.go:18-24, internal/poll/hook_unix.go:15),
	// which zeroes the RawSockaddrAny the kernel fills (syscall/syscall_bsd.go:275-278) and
	// converts it with anyToSockaddr: the name is the sun_len-2 bytes of the path up to its first
	// NUL (syscall/syscall_bsd.go:232-253). For a peer that did not bind a path the kernel's path
	// is all NULs, so the name is empty.
	"darwin": {unnamedUnixPeer: ""},
	// accept is syscall.Accept4 (internal/poll/sock_cloexec.go:16-17,
	// internal/poll/hook_cloexec.go:12), which zeroes the RawSockaddrAny the kernel fills
	// (syscall/syscall_linux.go:691-694) and converts it with anyToSockaddr
	// (syscall/syscall_linux.go:701). That rewrites a leading NUL of the path as '@' and ends the
	// name at the next NUL (syscall/syscall_linux.go:643-665). For a peer that did not bind a
	// path Linux returns only the family, so the path is all NULs and the name is "@". (A peer
	// bound to the abstract name "\x00name" is "@name", up to the first NUL of the name.)
	"linux": {unnamedUnixPeer: "@"},
}

// goosNames are the keys of goosValues, sorted: the order of a case's expect blocks.
func goosNames() []string {
	names := make([]string, 0, len(goosValues))
	for goos := range goosValues {
		names = append(names, goos)
	}
	sort.Strings(names)
	return names
}

// goosTarget is the name of the copy of server that answers as Go does on goos.
func goosTarget(server, goos string) string {
	return server + "@" + goos
}

// goosListener's connections report the RemoteAddr Go reports on its GOOS for a client that did
// not bind a path, so the real net/http server serving them writes the bytes it writes there.
type goosListener struct {
	net.Listener
	peer string
}

type goosConn struct {
	net.Conn
	peer net.Addr
}

func (l goosListener) Accept() (net.Conn, error) {
	c, err := l.Listener.Accept()
	if err != nil {
		return c, err
	}
	return goosConn{Conn: c, peer: &net.UnixAddr{Name: l.peer, Net: "unix"}}, nil
}

func (c goosConn) RemoteAddr() net.Addr { return c.peer }

// runByGOOS returns, for each case whose bytes depend on the GOOS (by its index), its result on
// every GOOS of goosValues: observed[i] for this GOOS, derived for the others.
func runByGOOS(targets map[string]target, cases []wireCase, observed []result) (map[int]map[string]result, error) {
	if _, ok := goosValues[runtime.GOOS]; !ok {
		return nil, fmt.Errorf("goos.go has no values for GOOS %s, the GOOS the generator runs on", runtime.GOOS)
	}
	byGOOS := map[int]map[string]result{}
	for i, c := range cases {
		if !c.byGOOS {
			continue
		}
		results := map[string]result{}
		for _, goos := range goosNames() {
			t, ok := targets[goosTarget(c.server, goos)]
			if !ok {
				return nil, fmt.Errorf("case %q: no server %s", c.name, goosTarget(c.server, goos))
			}
			first, second := runCase(t, c), runCase(t, c)
			if !sameResult(first, second) {
				return nil, fmt.Errorf("case %q on %s is not deterministic:\n%q %s\n%q %s", c.name, goos,
					first.response, first.end, second.response, second.end)
			}
			if goos == runtime.GOOS {
				if !sameResult(first, observed[i]) {
					return nil, fmt.Errorf("case %q: the %s values of goos.go do not give what Go does here:\nderived  %q %s\nobserved %q %s",
						c.name, goos, first.response, first.end, observed[i].response, observed[i].end)
				}
				first = observed[i]
			}
			results[goos] = first
		}
		byGOOS[i] = results
	}
	return byGOOS, nil
}

func sameResult(a, b result) bool {
	return bytes.Equal(a.response, b.response) && a.end == b.end
}
