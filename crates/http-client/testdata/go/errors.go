package main

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"syscall"
	"time"

	"golang.org/x/net/context/ctxhttp"
	"golang.org/x/net/http/httpproxy"
)

// errorCase is one failing request, sent the way bidder.go sends it (exchange/bidder.go:615-647:
// ctxhttp.Do under the request's deadline, then io.ReadAll of the body).
type errorCase struct {
	Name string `json:"name"`
	// Method and URL of the request, PORT standing for the server's port.
	Method string `json:"method"`
	URL    string `json:"url"`
	// DeadlineMs is the context deadline (none when zero).
	DeadlineMs int `json:"deadline_ms"`
	// ClientTimeoutMs is the client's Timeout (none when zero), as scope3's client has one
	// (modules/scope3/rtd/module.go:77-80).
	ClientTimeoutMs int `json:"client_timeout_ms"`
	// DoError is the text of client.Do's error, the *url.Error a caller without ctxhttp sees;
	// Error the text bidder.go sees (ctxhttp's, which is ctx.Err() once the context is done);
	// Stage says whether it came from sending the request or reading the body.
	DoError string `json:"do_error"`
	Error   string `json:"error"`
	Stage   string `json:"stage"`
	// Timeout is errors.Is(Error, context.DeadlineExceeded), the test bidder.go applies to a send
	// error (exchange/bidder.go:619; a body error goes back unclassified, bidder.go:641-646).
	Timeout bool `json:"timeout"`
	// Unreachable is isBidderUnreachableError of Error (exchange/non_bid_reason.go:53-57): a
	// refused connection or a host name that does not exist, seat non-bid code 103.
	Unreachable bool `json:"unreachable"`
}

type errorsDoc struct {
	Header header      `json:"header"`
	Cases  []errorCase `json:"cases"`
}

// send runs one case's request against the loopback port with transport, the bidder's way and
// the plain client.Do way.
func send(c errorCase, port string, transport *http.Transport) errorCase {
	return sendNaming(c, port, nil, transport)
}

// sendNaming is send, with the ports of other names the texts hold written as those names.
func sendNaming(c errorCase, port string, names map[string]string, transport *http.Transport) errorCase {
	url := strings.ReplaceAll(c.URL, "PORT", port)
	ports := map[string]string{port: "PORT"}
	for other, name := range names {
		ports[other] = name
	}
	ctx := context.Background()
	if c.DeadlineMs > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, time.Duration(c.DeadlineMs)*time.Millisecond)
		defer cancel()
	}
	body := func() io.Reader {
		if c.Method == http.MethodPost {
			return strings.NewReader(`{"id":"r"}`)
		}
		return nil
	}
	client := &http.Client{Transport: transport, Timeout: time.Duration(c.ClientTimeoutMs) * time.Millisecond}
	// bidder.go's way.
	req, err := http.NewRequest(c.Method, url, body())
	if err != nil {
		panic(err)
	}
	resp, err := ctxhttp.Do(ctx, client, req)
	c.Stage = "send"
	if err == nil {
		_, err = io.ReadAll(resp.Body)
		resp.Body.Close()
		c.Stage = "body"
	}
	c.Error = withPorts(errText(err), ports)
	c.Timeout = isDeadline(err)
	c.Unreachable = isUnreachable(err)
	// The plain client.Do way, under a fresh deadline of the same length.
	doCtx := context.Background()
	if c.DeadlineMs > 0 {
		var cancel context.CancelFunc
		doCtx, cancel = context.WithTimeout(doCtx, time.Duration(c.DeadlineMs)*time.Millisecond)
		defer cancel()
	}
	req, err = http.NewRequestWithContext(doCtx, c.Method, url, body())
	if err != nil {
		panic(err)
	}
	resp, err = client.Do(req)
	if err == nil {
		_, err = io.ReadAll(resp.Body)
		resp.Body.Close()
	}
	c.DoError = withPorts(errText(err), ports)
	client.CloseIdleConnections()
	return c
}

// isUnreachable is bidder.go's isBidderUnreachableError (exchange/non_bid_reason.go:53-57).
func isUnreachable(err error) bool {
	var dnsErr *net.DNSError
	isNoSuchHost := errors.As(err, &dnsErr) && dnsErr.IsNotFound
	return errors.Is(err, syscall.ECONNREFUSED) || isNoSuchHost
}

// noSuchHost is a dial that fails as Go's dialer does when the system resolver finds no such
// name (net/cgo_unix.go:189-190 and net/dial.go resolveAddrList), so no lookup leaves the
// machine: the host of a .invalid URL never resolves (RFC 6761).
func noSuchHost(_ context.Context, network, addr string) (net.Conn, error) {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		panic(err)
	}
	return nil, &net.OpError{Op: "dial", Net: network, Err: &net.DNSError{Err: "no such host", Name: host, IsNotFound: true}}
}

func errorsGolden() errorsDoc {
	doc := errorsDoc{Header: newHeader("golang.org/x/net")}
	// The server accepts and never answers.
	silent := newRawServer(func(conn *rawConn) { conn.drain() })
	doc.Cases = append(doc.Cases, send(errorCase{Name: "deadline_awaiting_headers", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 200}, silent.port, generalTransport(nil)))
	silent.close()

	// The server sends the head and part of the body, then stalls.
	stalling := newRawServer(func(conn *rawConn) {
		if _, ok := conn.readRequest(); !ok {
			return
		}
		conn.write("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789")
		conn.drain()
	})
	doc.Cases = append(doc.Cases, send(errorCase{Name: "deadline_reading_body", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 200}, stalling.port, generalTransport(nil)))
	stalling.close()

	// The server reads the request and closes the connection without answering.
	closing := newRawServer(func(conn *rawConn) { conn.readRequest() })
	doc.Cases = append(doc.Cases, send(errorCase{Name: "server_closes", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 1000}, closing.port, generalTransport(nil)))
	closing.close()

	// Nothing listens on the port.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	closedPort := strings.TrimPrefix(ln.Addr().String(), "127.0.0.1:")
	if err := ln.Close(); err != nil {
		panic(err)
	}
	doc.Cases = append(doc.Cases, send(errorCase{Name: "connection_refused", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 1000}, closedPort, generalTransport(nil)))

	// HTTP_PROXY under REQUEST_METHOD (a CGI environment): the proxy function refuses it.
	cgi := newRawServer(func(conn *rawConn) { conn.drain() })
	transport := generalTransport(nil)
	proxyFor := (&httpproxy.Config{HTTPProxy: "http://127.0.0.1:9", CGI: true}).ProxyFunc()
	transport.Proxy = func(r *http.Request) (*url.URL, error) { return proxyFor(r.URL) }
	doc.Cases = append(doc.Cases, send(errorCase{Name: "cgi_http_proxy", Method: http.MethodGet,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 1000}, cgi.port, transport))
	cgi.close()

	// Go's Client.Timeout passes while the client awaits the head, or reads the body, with no
	// context deadline; then a context deadline earlier than the client's timeout, which the
	// timeout leaves alone (net/http/client.go setRequestCancel).
	silent = newRawServer(func(conn *rawConn) { conn.drain() })
	doc.Cases = append(doc.Cases, send(errorCase{Name: "client_timeout_awaiting_headers", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", ClientTimeoutMs: 200}, silent.port, generalTransport(nil)))
	doc.Cases = append(doc.Cases, send(errorCase{Name: "deadline_before_client_timeout", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 200, ClientTimeoutMs: 2000}, silent.port, generalTransport(nil)))
	silent.close()
	stalling = newRawServer(func(conn *rawConn) {
		if _, ok := conn.readRequest(); !ok {
			return
		}
		conn.write("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n0123456789")
		conn.drain()
	})
	doc.Cases = append(doc.Cases, send(errorCase{Name: "client_timeout_reading_body", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", ClientTimeoutMs: 200}, stalling.port, generalTransport(nil)))
	stalling.close()

	// The bidder's host name does not exist.
	transport = generalTransport(nil)
	transport.DialContext = noSuchHost
	doc.Cases = append(doc.Cases, send(errorCase{Name: "dns_not_found", Method: http.MethodPost,
		URL: "http://nosuch.invalid:PORT/e", DeadlineMs: 1000}, closedPort, transport))

	// The url.Error's URL: one without a path, as URL.String writes it back; and the hop that
	// failed when a redirect leads to a port nothing listens on (net/http/client.go Client.do).
	doc.Cases = append(doc.Cases, send(errorCase{Name: "empty_path_refused", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT?x=1", DeadlineMs: 1000}, closedPort, generalTransport(nil)))
	redirecting := newRawServer(func(conn *rawConn) {
		for {
			if _, ok := conn.readRequest(); !ok {
				return
			}
			conn.write("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:" + closedPort +
				"/next\r\nContent-Length: 0\r\n\r\n")
		}
	})
	doc.Cases = append(doc.Cases, sendNaming(errorCase{Name: "redirect_then_refused", Method: http.MethodPost,
		URL: "http://127.0.0.1:PORT/e", DeadlineMs: 1000}, redirecting.port, map[string]string{closedPort: "CLOSED"},
		generalTransport(nil)))
	redirecting.close()
	return doc
}
