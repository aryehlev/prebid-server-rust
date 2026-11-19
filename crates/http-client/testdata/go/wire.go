package main

import (
	"bufio"
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

// rawServer is a loopback TCP listener that serves every connection it accepts with handle, each
// in its own goroutine. It counts the connections it accepted and the most that were open at once.
type rawServer struct {
	ln       net.Listener
	port     string
	mu       sync.Mutex
	accepted int
	open     int
	maxOpen  int
	wg       sync.WaitGroup
}

// newRawServer listens on 127.0.0.1 at a free port and starts serving.
func newRawServer(handle func(c *rawConn)) *rawServer {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	s := &rawServer{ln: ln, port: strconv.Itoa(ln.Addr().(*net.TCPAddr).Port)}
	s.wg.Add(1)
	go func() {
		defer s.wg.Done()
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			s.mu.Lock()
			s.accepted++
			s.open++
			s.maxOpen = max(s.maxOpen, s.open)
			s.mu.Unlock()
			s.wg.Add(1)
			go func() {
				defer s.wg.Done()
				defer func() {
					s.mu.Lock()
					s.open--
					s.mu.Unlock()
				}()
				defer c.Close()
				handle(&rawConn{Conn: c, r: bufio.NewReader(c)})
			}()
		}
	}()
	return s
}

// addr is the listener's address.
func (s *rawServer) addr() string { return "127.0.0.1:" + s.port }

// counts returns the connections accepted so far and the most open at once.
func (s *rawServer) counts() (accepted, maxOpen int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.accepted, s.maxOpen
}

// close stops the listener and waits for every connection handler to return.
func (s *rawServer) close() {
	if err := s.ln.Close(); err != nil {
		panic(err)
	}
	s.wg.Wait()
}

// rawConn is one accepted connection.
type rawConn struct {
	net.Conn
	r *bufio.Reader
}

// request is one request a server read: the request line and header lines as they came over the
// wire, and the body its Content-Length gave.
type request struct {
	head []string
	body []byte
}

// readRequest reads the next request on the connection; ok is false once the client has closed
// it or sent something that is not a request head.
func (c *rawConn) readRequest() (req request, ok bool) {
	length := 0
	for {
		line, err := c.r.ReadString('\n')
		if err != nil {
			return req, false
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		if name, value, found := strings.Cut(line, ":"); found && strings.EqualFold(name, "Content-Length") {
			n, err := strconv.Atoi(strings.TrimSpace(value))
			if err != nil {
				return req, false
			}
			length = n
		}
		req.head = append(req.head, line)
	}
	req.body = make([]byte, length)
	if _, err := io.ReadFull(c.r, req.body); err != nil {
		return req, false
	}
	return req, true
}

// write writes s, ignoring a client that has gone away.
func (c *rawConn) write(s string) {
	_, _ = io.WriteString(c.Conn, s)
}

// drain reads until the client closes the connection.
func (c *rawConn) drain() {
	_, _ = io.Copy(io.Discard, c.r)
}

// withPorts writes every port of ports in s as its placeholder: ports maps a port to the name
// the goldens give it.
func withPorts(s string, ports map[string]string) string {
	for port, name := range ports {
		s = strings.ReplaceAll(s, ":"+port, ":"+name)
	}
	return s
}

// dialPort dials 127.0.0.1 at the port of the address the Transport asks for, whatever host the
// request names, so a request can carry any host while its bytes reach a loopback server.
func dialPort(ctx context.Context, network, addr string) (net.Conn, error) {
	_, port, err := net.SplitHostPort(addr)
	if err != nil {
		return nil, err
	}
	d := &net.Dialer{Timeout: 30 * time.Second, KeepAlive: 15 * time.Second}
	return d.DialContext(ctx, network, net.JoinHostPort("127.0.0.1", port))
}

// dialTo returns a DialContext that dials addr for every connection.
func dialTo(addr string) func(ctx context.Context, network, _ string) (net.Conn, error) {
	return func(ctx context.Context, network, _ string) (net.Conn, error) {
		d := &net.Dialer{Timeout: 30 * time.Second, KeepAlive: 15 * time.Second}
		return d.DialContext(ctx, network, addr)
	}
}

// generalTransport is the Transport of router.go's generalHttpClient (router/router.go:143-158)
// with config.go's http_client defaults (config/config.go:983-990) and roots as its RootCAs.
func generalTransport(roots *x509.CertPool) *http.Transport {
	return &http.Transport{
		Proxy: http.ProxyFromEnvironment,
		DialContext: (&net.Dialer{
			Timeout:   30 * time.Second,
			KeepAlive: 15 * time.Second,
		}).DialContext,
		MaxConnsPerHost:       0,
		MaxIdleConns:          400,
		MaxIdleConnsPerHost:   10,
		IdleConnTimeout:       60 * time.Second,
		TLSClientConfig:       &tls.Config{RootCAs: roots},
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: 1 * time.Second,
	}
}

// errText is err's text, or "" for no error.
func errText(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// isDeadline is bidder.go's timeout test (exchange/bidder.go:619).
func isDeadline(err error) bool {
	return errors.Is(err, context.DeadlineExceeded)
}

// roundTrip sends req with client and reads the whole response body.
func roundTrip(client *http.Client, req *http.Request) (*http.Response, []byte, error) {
	resp, err := client.Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	return resp, body, err
}

// nonNil is values, or an empty list for none, so a golden writes [] rather than null.
func nonNil(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}
