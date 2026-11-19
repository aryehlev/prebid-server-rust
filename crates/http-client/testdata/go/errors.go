package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
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
	// DoError is the text of client.Do's error, the *url.Error a caller without ctxhttp sees;
	// Error the text bidder.go sees (ctxhttp's, which is ctx.Err() once the context is done);
	// Stage says whether it came from sending the request or reading the body.
	DoError string `json:"do_error"`
	Error   string `json:"error"`
	Stage   string `json:"stage"`
	// Timeout is errors.Is(Error, context.DeadlineExceeded), the test bidder.go applies to a send
	// error (exchange/bidder.go:619; a body error goes back unclassified, bidder.go:641-646).
	Timeout bool `json:"timeout"`
}

type errorsDoc struct {
	Header header      `json:"header"`
	Cases  []errorCase `json:"cases"`
}

// send runs one case's request against the loopback port with transport, the bidder's way and
// the plain client.Do way.
func send(c errorCase, port string, transport *http.Transport) errorCase {
	url := strings.ReplaceAll(c.URL, "PORT", port)
	ports := map[string]string{port: "PORT"}
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
	client := &http.Client{Transport: transport}
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
	return doc
}
