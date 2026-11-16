package oracle

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
)

// Wire limits of the raw HTTP/1.1 reader.
const (
	maxLineBytes   = 64 << 10
	maxHeaderLines = 1000
)

// errBadRequest marks malformed requests; the proxy answers them with 400.
var errBadRequest = errors.New("oracle: malformed HTTP request")

// wireRequest is one HTTP/1.x request exactly as it arrived. net/http is not
// used on the server side because it canonicalizes header names and loses
// their order, both of which are compared.
type wireRequest struct {
	method  string
	target  string
	proto   string
	headers parity.HeaderList
	body    []byte
	// chunked is true when the body arrived with chunked transfer coding.
	chunked bool
}

// header returns the first value of name (case-insensitive).
func (r *wireRequest) header(name string) (string, bool) {
	return headerValue(r.headers, name)
}

func headerValue(h parity.HeaderList, name string) (string, bool) {
	for _, kv := range h {
		if strings.EqualFold(kv[0], name) {
			return kv[1], true
		}
	}
	return "", false
}

// headerValues returns every value of name (case-insensitive), in order.
func headerValues(h parity.HeaderList, name string) []string {
	var out []string
	for _, kv := range h {
		if strings.EqualFold(kv[0], name) {
			out = append(out, kv[1])
		}
	}
	return out
}

// hasToken reports whether a comma-separated header contains token.
func hasToken(values []string, token string) bool {
	for _, v := range values {
		for _, part := range strings.Split(v, ",") {
			if strings.EqualFold(strings.TrimSpace(part), token) {
				return true
			}
		}
	}
	return false
}

// keepAlive reports whether the connection may serve another request.
func (r *wireRequest) keepAlive() bool {
	conn := headerValues(r.headers, "Connection")
	if r.proto == "HTTP/1.0" {
		return hasToken(conn, "keep-alive")
	}
	return !hasToken(conn, "close")
}

// readRequest reads one request. It returns io.EOF when the connection closed
// cleanly between requests. interim is written before the body is read when
// the client sent "Expect: 100-continue".
func readRequest(br *bufio.Reader, interim io.Writer, maxBody int64) (*wireRequest, error) {
	line, err := readLine(br)
	if err != nil {
		return nil, err
	}
	if line == "" {
		// RFC 9112 section 2.2: a server should ignore at least one empty
		// line received before the request-line.
		if line, err = readLine(br); err != nil {
			return nil, err
		}
	}
	req, err := parseRequestLine(line)
	if err != nil {
		return nil, err
	}
	if req.headers, err = readHeaders(br); err != nil {
		return nil, err
	}
	if hasToken(headerValues(req.headers, "Expect"), "100-continue") && interim != nil {
		if _, err := io.WriteString(interim, "HTTP/1.1 100 Continue\r\n\r\n"); err != nil {
			return nil, fmt.Errorf("oracle: writing 100 Continue: %w", err)
		}
	}
	if err := readBody(br, req, maxBody); err != nil {
		return nil, err
	}
	return req, nil
}

func parseRequestLine(line string) (*wireRequest, error) {
	parts := strings.Split(line, " ")
	if len(parts) != 3 || parts[0] == "" || parts[1] == "" {
		return nil, fmt.Errorf("%w: request line %q", errBadRequest, line)
	}
	if parts[2] != "HTTP/1.1" && parts[2] != "HTTP/1.0" {
		return nil, fmt.Errorf("%w: unsupported protocol %q", errBadRequest, parts[2])
	}
	return &wireRequest{method: parts[0], target: parts[1], proto: parts[2]}, nil
}

// readLine reads one CRLF (or bare LF) terminated line without the terminator.
func readLine(br *bufio.Reader) (string, error) {
	var buf []byte
	for {
		chunk, isPrefix, err := br.ReadLine()
		if err != nil {
			if errors.Is(err, io.EOF) && len(buf) > 0 {
				return "", fmt.Errorf("%w: truncated line", errBadRequest)
			}
			return "", err
		}
		buf = append(buf, chunk...)
		if len(buf) > maxLineBytes {
			return "", fmt.Errorf("%w: line longer than %d bytes", errBadRequest, maxLineBytes)
		}
		if !isPrefix {
			return string(buf), nil
		}
	}
}

// readHeaders reads header lines up to the empty line, keeping name case,
// order and duplicates. Obsolete line folding is rejected (RFC 9112 5.2).
func readHeaders(br *bufio.Reader) (parity.HeaderList, error) {
	headers := parity.HeaderList{}
	for {
		line, err := readLine(br)
		if err != nil {
			return nil, noEOF(err)
		}
		if line == "" {
			return headers, nil
		}
		if len(headers) >= maxHeaderLines {
			return nil, fmt.Errorf("%w: more than %d header lines", errBadRequest, maxHeaderLines)
		}
		if line[0] == ' ' || line[0] == '\t' {
			return nil, fmt.Errorf("%w: obsolete header line folding", errBadRequest)
		}
		name, value, ok := strings.Cut(line, ":")
		if !ok || name == "" || strings.ContainsAny(name, " \t") {
			return nil, fmt.Errorf("%w: header line %q", errBadRequest, line)
		}
		headers = append(headers, [2]string{name, strings.Trim(value, " \t")})
	}
}

// noEOF turns an EOF inside a request into a malformed-request error.
func noEOF(err error) error {
	if errors.Is(err, io.EOF) {
		return fmt.Errorf("%w: connection closed inside the request", errBadRequest)
	}
	return err
}

// readBody reads the body framed by Transfer-Encoding or Content-Length.
func readBody(br *bufio.Reader, req *wireRequest, maxBody int64) error {
	if te := headerValues(req.headers, "Transfer-Encoding"); len(te) > 0 {
		codings := strings.Split(te[len(te)-1], ",")
		if !strings.EqualFold(strings.TrimSpace(codings[len(codings)-1]), "chunked") {
			return fmt.Errorf("%w: transfer coding %q does not end in chunked", errBadRequest, strings.Join(te, ","))
		}
		body, err := readChunked(br, maxBody)
		if err != nil {
			return err
		}
		req.body, req.chunked = body, true
		return nil
	}
	n, err := contentLength(req.headers)
	if err != nil || n == 0 {
		return err
	}
	if n > maxBody {
		return fmt.Errorf("%w: body of %d bytes exceeds %d", errBadRequest, n, maxBody)
	}
	req.body = make([]byte, n)
	if _, err := io.ReadFull(br, req.body); err != nil {
		return noEOF(fmt.Errorf("oracle: reading body: %w", err))
	}
	return nil
}

func contentLength(h parity.HeaderList) (int64, error) {
	values := headerValues(h, "Content-Length")
	if len(values) == 0 {
		return 0, nil
	}
	n, err := strconv.ParseInt(strings.TrimSpace(values[0]), 10, 64)
	if err != nil || n < 0 {
		return 0, fmt.Errorf("%w: Content-Length %q", errBadRequest, values[0])
	}
	for _, v := range values[1:] {
		if strings.TrimSpace(v) != strings.TrimSpace(values[0]) {
			return 0, fmt.Errorf("%w: conflicting Content-Length values", errBadRequest)
		}
	}
	return n, nil
}

// readChunked de-chunks a body and discards trailers.
func readChunked(br *bufio.Reader, maxBody int64) ([]byte, error) {
	var body bytes.Buffer
	for {
		line, err := readLine(br)
		if err != nil {
			return nil, noEOF(err)
		}
		sizeText, _, _ := strings.Cut(line, ";")
		size, err := strconv.ParseInt(strings.TrimSpace(sizeText), 16, 64)
		if err != nil || size < 0 {
			return nil, fmt.Errorf("%w: chunk size %q", errBadRequest, line)
		}
		if size == 0 {
			return body.Bytes(), skipTrailers(br)
		}
		if int64(body.Len())+size > maxBody {
			return nil, fmt.Errorf("%w: chunked body exceeds %d bytes", errBadRequest, maxBody)
		}
		if _, err := io.CopyN(&body, br, size); err != nil {
			return nil, noEOF(fmt.Errorf("oracle: reading chunk: %w", err))
		}
		if err := expectCRLF(br); err != nil {
			return nil, err
		}
	}
}

// expectCRLF consumes the CRLF that must follow chunk data.
func expectCRLF(br *bufio.Reader) error {
	var crlf [2]byte
	if _, err := io.ReadFull(br, crlf[:]); err != nil || crlf != [2]byte{'\r', '\n'} {
		return fmt.Errorf("%w: chunk data not terminated by CRLF", errBadRequest)
	}
	return nil
}

func skipTrailers(br *bufio.Reader) error {
	for {
		line, err := readLine(br)
		if err != nil {
			return noEOF(err)
		}
		if line == "" {
			return nil
		}
	}
}

// wireResponse is a response the proxy writes.
type wireResponse struct {
	status  int
	headers parity.HeaderList
	body    []byte
}

// framingHeaders are computed by the proxy; plan values for them are dropped.
var framingHeaders = []string{"Content-Length", "Transfer-Encoding", "Connection"}

// bodyless reports whether a status never carries a body.
func bodyless(status int) bool {
	return status < 200 || status == http.StatusNoContent || status == http.StatusNotModified
}

// finalHeaders returns the headers exactly as they go on the wire: the plan
// headers in order without framing headers, then Content-Length (unless the
// status is bodyless) and "Connection: close" when the connection closes.
func (r *wireResponse) finalHeaders(closing bool) parity.HeaderList {
	out := parity.HeaderList{}
	for _, kv := range r.headers {
		drop := false
		for _, f := range framingHeaders {
			if strings.EqualFold(kv[0], f) {
				drop = true
			}
		}
		if !drop {
			out = append(out, kv)
		}
	}
	if !bodyless(r.status) {
		out = append(out, [2]string{"Content-Length", strconv.Itoa(len(r.body))})
	}
	if closing {
		out = append(out, [2]string{"Connection", "close"})
	}
	return out
}

// write sends the response; headers are the finalHeaders result.
func (r *wireResponse) write(w io.Writer, headers parity.HeaderList, method string) error {
	var buf bytes.Buffer
	reason := http.StatusText(r.status)
	if reason == "" {
		reason = "Status " + strconv.Itoa(r.status)
	}
	fmt.Fprintf(&buf, "HTTP/1.1 %d %s\r\n", r.status, reason)
	for _, kv := range headers {
		buf.WriteString(kv[0])
		buf.WriteString(": ")
		buf.WriteString(kv[1])
		buf.WriteString("\r\n")
	}
	buf.WriteString("\r\n")
	if method != http.MethodHead && !bodyless(r.status) {
		buf.Write(r.body)
	}
	if _, err := w.Write(buf.Bytes()); err != nil {
		return fmt.Errorf("oracle: writing response: %w", err)
	}
	return nil
}

// writeMalformed sends a response no HTTP client accepts (fail = "error").
func writeMalformed(w io.Writer) error {
	_, err := io.WriteString(w, "HTTP/1.1 ABC Parity-Error\r\n\r\n")
	return err
}

// resetConn closes conn with an RST instead of a FIN (fail = "reset").
func resetConn(conn net.Conn) {
	if tcp, ok := conn.(*net.TCPConn); ok {
		// SetLinger only fails on a closed connection, which is then reset anyway.
		_ = tcp.SetLinger(0)
	}
	conn.Close()
}

// waitOrClientClose waits for d or until the client closes the connection,
// whichever comes first, and reports whether the client closed. Pipelined
// bytes the client sends meanwhile stay buffered in br.
func waitOrClientClose(conn net.Conn, br *bufio.Reader, d time.Duration, stop <-chan struct{}) bool {
	peeked := make(chan error, 1)
	go func() {
		_, err := br.Peek(1)
		peeked <- err
	}()
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case err := <-peeked:
		if err != nil {
			return true
		}
		// The client pipelined another request; keep waiting the full delay.
		select {
		case <-timer.C:
		case <-stop:
		}
		return false
	case <-timer.C:
	case <-stop:
	}
	// Unblock the peek and restore blocking reads for the next request.
	_ = conn.SetReadDeadline(time.Now())
	err := <-peeked
	_ = conn.SetReadDeadline(time.Time{})
	var ne net.Error
	return err != nil && !(errors.As(err, &ne) && ne.Timeout())
}
