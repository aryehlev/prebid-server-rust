package main

import (
	"bufio"
	"bytes"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net"
	"strconv"
	"strings"
	"sync"
	"time"
)

// slot is a listener that lives for the whole run (so HTTP_PROXY can name it) and serves the
// current case's Server scripts, one per accepted connection.
type slot struct {
	name string
	ln   net.Listener
	addr string
	env  *runEnv

	mu    sync.Mutex
	srv   *Server
	next  int
	conns []*capture
	wg    sync.WaitGroup
	// marker is closed when the accept loop reaches the connection from markerAddr.
	marker     chan struct{}
	markerAddr string
}

// capture records what one server connection consumed.
type capture struct {
	mu   sync.Mutex
	data []byte
	raw  net.Conn
	stop chan struct{}
	once sync.Once
}

func (c *capture) record(b []byte) {
	c.mu.Lock()
	c.data = append(c.data, b...)
	c.mu.Unlock()
}

func (c *capture) bytes() []byte {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([]byte{}, c.data...)
}

// abort ends the connection's script: sleeps return and blocked reads fail.
func (c *capture) abort() {
	c.once.Do(func() {
		close(c.stop)
		_ = c.raw.SetDeadline(time.Now())
	})
}

func newSlot(name string, env *runEnv) (*slot, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	s := &slot{name: name, ln: ln, addr: ln.Addr().String(), env: env}
	go s.acceptLoop()
	return s, nil
}

func (s *slot) acceptLoop() {
	for {
		conn, err := s.ln.Accept()
		if err != nil {
			return
		}
		s.mu.Lock()
		if s.marker != nil && conn.RemoteAddr().String() == s.markerAddr {
			close(s.marker)
			s.marker = nil
			s.mu.Unlock()
			_ = conn.Close()
			continue
		}
		var ops []Op
		scripted := s.srv != nil && s.next < len(s.srv.Conns)
		if scripted {
			ops = s.srv.Conns[s.next]
		}
		s.next++
		c := &capture{raw: conn, stop: make(chan struct{})}
		s.conns = append(s.conns, c)
		s.wg.Add(1)
		s.mu.Unlock()
		go func() {
			defer s.wg.Done()
			if !scripted {
				// A connection the script did not expect: hang up at once.
				_ = conn.Close()
				return
			}
			serve(conn, ops, c, s.env)
		}()
	}
}

// start installs a case's server (nil: none) and forgets earlier connections.
func (s *slot) start(srv *Server) {
	s.mu.Lock()
	s.srv = srv
	s.next = 0
	s.conns = nil
	s.mu.Unlock()
}

// settle returns once the accept loop has taken every connection the client opened: it queues
// a marker connection behind them (the accept queue is FIFO) and waits for the loop to reach
// it. The lock is held while dialing so the loop cannot take the marker before it is known.
func (s *slot) settle(timeout time.Duration) {
	s.mu.Lock()
	conn, err := net.DialTimeout("tcp", s.addr, timeout)
	if err != nil {
		s.mu.Unlock()
		return
	}
	defer func() { _ = conn.Close() }()
	marker := make(chan struct{})
	s.marker, s.markerAddr = marker, conn.LocalAddr().String()
	s.mu.Unlock()
	select {
	case <-marker:
	case <-time.After(timeout):
		s.mu.Lock()
		s.marker = nil
		s.mu.Unlock()
	}
}

// finish waits for the case's connections, aborting them after the grace period, and returns
// what each consumed.
func (s *slot) finish(grace time.Duration) []Bytes {
	s.settle(grace)
	done := make(chan struct{})
	go func() {
		s.wg.Wait()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(grace):
		s.mu.Lock()
		for _, c := range s.conns {
			c.abort()
		}
		s.mu.Unlock()
		<-done
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]Bytes, 0, len(s.conns))
	for _, c := range s.conns {
		out = append(out, Bytes(s.env.normBytes(c.bytes())))
	}
	s.srv = nil
	return out
}

// bufConn reads through the script's buffered reader, so a TLS server started on it sees
// bytes that were already buffered.
type bufConn struct {
	net.Conn
	r *bufio.Reader
}

func (b *bufConn) Read(p []byte) (int, error) { return b.r.Read(p) }

// stream is a server connection as the script sees it: plain, or TLS after a tls op.
type stream struct {
	raw  net.Conn
	r    *bufio.Reader
	w    io.Writer
	tls  *tls.Conn
	c    *capture
	head []byte
}

func serve(conn net.Conn, ops []Op, c *capture, env *runEnv) {
	st := &stream{raw: conn, r: bufio.NewReader(conn), w: conn, c: c}
	defer func() {
		if st.tls != nil {
			_ = st.tls.Close()
		}
		_ = conn.Close()
	}()
	for _, op := range ops {
		if !st.run(op, env) {
			return
		}
	}
	// The script is over: record whatever else the client sends until it hangs up.
	st.drain(true)
}

// run executes one op and reports whether the script continues.
func (st *stream) run(op Op, env *runEnv) bool {
	switch op.Op {
	case "req":
		if err := st.readHead(); err != nil {
			return false
		}
		return st.readBody() == nil
	case "head":
		return st.readHead() == nil
	case "body":
		return st.readBody() == nil
	case "write":
		_, err := st.w.Write(env.subst(op.Data))
		return err == nil
	case "sleep":
		select {
		case <-time.After(time.Duration(op.Ms) * time.Millisecond):
			return true
		case <-st.c.stop:
			return false
		}
	case "close":
		if st.tls != nil {
			_ = st.tls.Close()
		}
		_ = st.raw.Close()
		return false
	case "reset":
		if tc, ok := st.raw.(*net.TCPConn); ok {
			_ = tc.SetLinger(0)
		}
		_ = st.raw.Close()
		return false
	case "drain":
		st.drain(true)
		return false
	case "skip":
		_ = st.raw.SetReadDeadline(time.Now().Add(time.Duration(op.Ms) * time.Millisecond))
		st.drain(false)
		_ = st.raw.SetReadDeadline(time.Time{})
		return true
	case "tls":
		cert, err := env.pki.serverCert(string(op.Data))
		if err != nil {
			return false
		}
		tc := tls.Server(&bufConn{Conn: st.raw, r: st.r}, &tls.Config{Certificates: []tls.Certificate{cert}})
		if err := tc.Handshake(); err != nil {
			return false
		}
		st.tls = tc
		st.r = bufio.NewReader(tc)
		st.w = tc
		return true
	}
	return false
}

// drain reads until EOF or an error, recording the bytes when asked.
func (st *stream) drain(record bool) {
	buf := make([]byte, 4096)
	for {
		n, err := st.r.Read(buf)
		if record && n > 0 {
			st.c.record(buf[:n])
		}
		if err != nil {
			return
		}
	}
}

func (st *stream) readLine() ([]byte, error) {
	line, err := st.r.ReadBytes('\n')
	st.c.record(line)
	return line, err
}

// readHead reads a request head up to its blank line.
func (st *stream) readHead() error {
	var head []byte
	for {
		line, err := st.readLine()
		head = append(head, line...)
		if err != nil {
			return err
		}
		if string(line) == "\r\n" || string(line) == "\n" {
			st.head = head
			return nil
		}
	}
}

// readBody reads the body the last head announced (chunked, Content-Length or none).
func (st *stream) readBody() error {
	chunked, length := false, int64(0)
	for _, line := range strings.Split(string(st.head), "\n") {
		k, v, ok := strings.Cut(strings.TrimRight(line, "\r"), ":")
		if !ok {
			continue
		}
		v = strings.TrimSpace(v)
		switch strings.ToLower(k) {
		case "transfer-encoding":
			chunked = strings.Contains(strings.ToLower(v), "chunked")
		case "content-length":
			if n, err := strconv.ParseInt(v, 10, 64); err == nil {
				length = n
			}
		}
	}
	if chunked {
		return st.readChunked()
	}
	return st.readN(length)
}

func (st *stream) readN(n int64) error {
	b := make([]byte, n)
	got, err := io.ReadFull(st.r, b)
	st.c.record(b[:got])
	return err
}

func (st *stream) readChunked() error {
	for {
		line, err := st.readLine()
		if err != nil {
			return err
		}
		size := strings.TrimSpace(string(line))
		if i := strings.IndexByte(size, ';'); i >= 0 {
			size = size[:i]
		}
		n, err := strconv.ParseInt(size, 16, 64)
		if err != nil {
			return fmt.Errorf("bad chunk size %q", size)
		}
		if n == 0 {
			for {
				line, err := st.readLine()
				if err != nil {
					return err
				}
				if bytes.Equal(line, []byte("\r\n")) || bytes.Equal(line, []byte("\n")) {
					return nil
				}
			}
		}
		if err := st.readN(n + 2); err != nil {
			return err
		}
	}
}

// refusedAddr returns a loopback address nothing listens on.
func refusedAddr() (string, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return "", err
	}
	addr := ln.Addr().String()
	if err := ln.Close(); err != nil {
		return "", err
	}
	return addr, nil
}

var errUnknownCert = errors.New("unknown certificate")
