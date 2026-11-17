package server

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// Defaults for envelope members a runner may omit. They are constants, not the
// parity caller's own address, so both implementations dispatch identical
// requests.
const (
	// DefaultRemoteAddr is the RemoteAddr of a dispatched request without
	// request.remote_addr.
	DefaultRemoteAddr = "127.0.0.1:0"
	// DefaultHost is the Host of a dispatched request with neither request.host
	// nor a Host header.
	DefaultHost = "127.0.0.1"
)

// validateEnvelope checks what decoding alone cannot: the document header, the
// required members and the enum-valued members of the envelope schema.
func validateEnvelope(env *parity.Envelope) error {
	if err := checkHeader(env.Schema, env.SchemaVersion, parity.SchemaEnvelope); err != nil {
		return err
	}
	if env.CaseID == "" {
		return badDocument("case_id is required")
	}
	if err := validateExecRequest(env.Request); err != nil {
		return err
	}
	if err := validateMock(env.Mock); err != nil {
		return err
	}
	if err := validateDecisions(env.Decisions); err != nil {
		return err
	}
	return validateCapture(env.Capture)
}

func validateExecRequest(in parity.ExecRequest) error {
	if !isToken(in.Method) {
		return badDocument("request.method %q is not an HTTP method token", in.Method)
	}
	if !strings.HasPrefix(in.Path, "/") {
		return badDocument("request.path %q must start with /", in.Path)
	}
	for i, kv := range in.Headers {
		if !isToken(kv[0]) {
			return badDocument("request.headers[%d]: invalid header name %q", i, kv[0])
		}
		if strings.ContainsAny(kv[1], "\r\n\x00") {
			return badDocument("request.headers[%d]: header %s has a value with CR, LF or NUL", i, kv[0])
		}
	}
	return nil
}

func validateMock(m parity.MockSpec) error {
	switch m.Mode {
	case "", parity.MockModeOracle, parity.MockModeInline:
	default:
		return badDocument("mock.mode %q is not oracle or inline", m.Mode)
	}
	switch m.Level {
	case "", parity.MockLevelHTTP, parity.MockLevelBidderResp:
	default:
		return badDocument("mock.level %q is not http or bidder_response", m.Level)
	}
	if m.Inline == nil {
		return nil
	}
	for kind := range m.Inline.Kinds {
		if !knownOutboundKind(kind) {
			return badDocument("mock.inline.kinds: unknown outbound kind %q", kind)
		}
	}
	for i, rule := range m.Inline.Rules {
		if rule.Kind != "" && !knownOutboundKind(rule.Kind) {
			return badDocument("mock.inline.rules[%d]: unknown outbound kind %q", i, rule.Kind)
		}
	}
	return nil
}

func knownOutboundKind(kind string) bool {
	for _, known := range parity.AllOutboundKinds() {
		if kind == known {
			return true
		}
	}
	return false
}

// validateDecisions rejects unknown decision points. Out-of-domain values are
// left to the decision sites, which ignore them and add a note (README).
func validateDecisions(decisions map[string]json.RawMessage) error {
	for point := range decisions {
		if !knownDecision(parity.DecisionID(point)) {
			return badDocument("decisions: unknown decision point %q", point)
		}
	}
	return nil
}

func knownDecision(point parity.DecisionID) bool {
	for _, known := range parity.AllDecisions() {
		if point == known {
			return true
		}
	}
	return false
}

func validateCapture(c parity.CaptureSpec) error {
	filter, err := c.StageFilter()
	if err != nil {
		return badDocument("%v", err)
	}
	known := make(map[parity.StageID]bool, len(parity.AllStages()))
	for _, stage := range parity.AllStages() {
		known[stage] = true
	}
	for stage := range filter {
		if !known[stage] {
			return badDocument("capture.stages: unknown stage %q", stage)
		}
	}
	return nil
}

// buildRequest rebuilds the inbound request the way net/http's server would
// have parsed it from the wire: the request URI is parsed with
// url.ParseRequestURI, Host headers move to Request.Host, a chunked
// Transfer-Encoding moves to Request.TransferEncoding, and the body is the
// envelope's wire bytes. It returns the request and those bytes.
func buildRequest(ctx context.Context, in parity.ExecRequest) (*http.Request, []byte, error) {
	wire, err := in.Body.WireBytes()
	if err != nil {
		return nil, nil, badDocument("request.body: %v", err)
	}
	requestURI := in.Path
	if in.Query != "" {
		requestURI += "?" + in.Query
	}
	u, err := url.ParseRequestURI(requestURI)
	if err != nil {
		return nil, nil, badDocument("request path and query: %v", err)
	}
	req, err := http.NewRequestWithContext(ctx, in.Method, "/", http.NoBody)
	if err != nil {
		return nil, nil, badDocument("request.method: %v", err)
	}
	req.URL, req.RequestURI = u, requestURI
	req.RemoteAddr = in.RemoteAddr
	if req.RemoteAddr == "" {
		req.RemoteAddr = DefaultRemoteAddr
	}
	req.Header, req.Host = splitHost(in.Headers)
	if in.Host != "" {
		req.Host = in.Host
	}
	setBody(req, wire)
	if err := applyFraming(req, len(wire)); err != nil {
		return nil, nil, err
	}
	return req, wire, nil
}

// splitHost converts the envelope headers to an http.Header and takes the
// first Host header out of it, as net/http does. It returns DefaultHost when
// there is no Host header.
func splitHost(list parity.HeaderList) (http.Header, string) {
	header := make(http.Header, len(list))
	host := ""
	for _, kv := range list {
		if http.CanonicalHeaderKey(kv[0]) == "Host" {
			if host == "" {
				host = kv[1]
			}
			continue
		}
		header.Add(kv[0], kv[1])
	}
	if host == "" {
		host = DefaultHost
	}
	return header, host
}

// setBody gives the request its body as net/http's server does: http.NoBody
// when there is none, and otherwise a reader that ends like the server's
// body of a request with a Content-Length (wireBody).
func setBody(req *http.Request, wire []byte) {
	req.GetBody = nil
	req.ContentLength = int64(len(wire))
	if len(wire) == 0 {
		req.Body = http.NoBody
		return
	}
	req.Body = &wireBody{rest: wire}
}

// wireBody reads like the body net/http's server hands a handler for a
// request with a Content-Length (net/http transfer.go, body.readLocked over
// an io.LimitedReader): the read that returns the last bytes also returns
// io.EOF, where a bytes.Reader returns it only on the next read, and a read
// after Close fails with http.ErrBodyReadAfterClose. A handler that reads
// through an io.LimitedReader and then reads one more byte to see whether
// the body ended (the max_request_size check of endpoints/openrtb2/auction.go)
// takes the branch it takes on the main port.
type wireBody struct {
	rest   []byte
	closed bool
}

func (b *wireBody) Read(p []byte) (int, error) {
	if b.closed {
		return 0, http.ErrBodyReadAfterClose
	}
	if len(b.rest) == 0 {
		return 0, io.EOF
	}
	n := copy(p, b.rest)
	b.rest = b.rest[n:]
	if len(b.rest) == 0 {
		return n, io.EOF
	}
	return n, nil
}

func (b *wireBody) Close() error {
	b.closed = true
	return nil
}

// applyFraming applies the framing headers like net/http's server: a chunked
// Transfer-Encoding is removed from the header (with any Content-Length) and
// recorded on the request; otherwise a Content-Length header must match the
// body, because the envelope cannot describe a body the server would truncate.
func applyFraming(req *http.Request, bodyLen int) error {
	if te := req.Header.Values("Transfer-Encoding"); len(te) > 0 {
		if len(te) != 1 || !strings.EqualFold(te[0], "chunked") {
			return badDocument("request.headers: only Transfer-Encoding: chunked is supported, got %q", te)
		}
		req.Header.Del("Transfer-Encoding")
		req.Header.Del("Content-Length")
		req.TransferEncoding = []string{"chunked"}
		req.ContentLength = -1
		return nil
	}
	for _, cl := range req.Header.Values("Content-Length") {
		n, err := strconv.ParseInt(strings.TrimSpace(cl), 10, 64)
		if err != nil || n != int64(bodyLen) {
			return badDocument("request.headers: Content-Length %q does not match the %d-byte body", cl, bodyLen)
		}
	}
	return nil
}

// isToken reports whether s is a non-empty RFC 9110 token (method or header name).
func isToken(s string) bool {
	if s == "" {
		return false
	}
	for i := 0; i < len(s); i++ {
		if !isTokenByte(s[i]) {
			return false
		}
	}
	return true
}

func isTokenByte(c byte) bool {
	switch {
	case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z', c >= '0' && c <= '9':
		return true
	}
	return strings.IndexByte("!#$%&'*+-.^_`|~", c) >= 0
}
