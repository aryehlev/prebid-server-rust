package runner

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
)

// Parity routes relative to parity.PathPrefix.
const (
	routeHealth  = "health"
	routeExec    = "exec"
	routeConfig  = "config"
	routeBidders = "bidders"
	routeReset   = "reset"
	routeAdapter = "adapter/"
)

// maxResponseBytes caps a parity response body.
const maxResponseBytes = 256 << 20

// ErrNotInstrumented matches a parity route the server does not serve
// (404 or 501): the implementation is not parity-capable for that call.
var ErrNotInstrumented = errors.New("parity route not served")

// StatusError is a non-200 parity response.
type StatusError struct {
	Route   string
	Status  int
	Message string
}

func (e *StatusError) Error() string {
	return fmt.Sprintf("%s answered %d: %s", e.Route, e.Status, e.Message)
}

// Is matches ErrNotInstrumented for 404 and 501.
func (e *StatusError) Is(target error) bool {
	return target == ErrNotInstrumented && (e.Status == http.StatusNotFound || e.Status == http.StatusNotImplemented)
}

// answeredError is a response that arrived but could not be used (bad JSON,
// wrong document).
type answeredError struct{ err error }

func (e *answeredError) Error() string { return e.err.Error() }
func (e *answeredError) Unwrap() error { return e.err }

// IsNotInstrumented reports whether err means the route is not served.
func IsNotInstrumented(err error) bool { return errors.Is(err, ErrNotInstrumented) }

// isAnswered reports whether the server answered at all (any status or an
// undecodable document), as opposed to a transport failure.
func isAnswered(err error) bool {
	var se *StatusError
	var ae *answeredError
	return errors.As(err, &se) || errors.As(err, &ae)
}

// Client calls one server's /__parity/* routes.
type Client struct {
	base  string
	token string
	http  *http.Client
}

// NewClient returns a client of the server at base (http://127.0.0.1:<port>).
// It never uses a proxy and never decompresses, so documents arrive as sent.
func NewClient(base, token string) *Client {
	tr := &http.Transport{Proxy: nil, DisableCompression: true, MaxIdleConnsPerHost: 4}
	return &Client{base: strings.TrimSuffix(base, "/"), token: token, http: &http.Client{Transport: tr}}
}

// CloseIdle closes idle connections.
func (c *Client) CloseIdle() { c.http.CloseIdleConnections() }

// Health calls GET /__parity/health.
func (c *Client) Health(ctx context.Context) (*parity.Health, error) {
	var h parity.Health
	if _, err := c.call(ctx, http.MethodGet, routeHealth, nil, &h); err != nil {
		return nil, err
	}
	if h.Schema != parity.SchemaHealth {
		return nil, &answeredError{fmt.Errorf("health document schema %q, want %q", h.Schema, parity.SchemaHealth)}
	}
	return &h, nil
}

// Exec posts an envelope and returns the trace and its raw document.
func (c *Client) Exec(ctx context.Context, env *parity.Envelope) (*parity.Trace, []byte, error) {
	var t parity.Trace
	raw, err := c.call(ctx, http.MethodPost, routeExec, env, &t)
	if err != nil {
		return nil, raw, err
	}
	return &t, raw, nil
}

// AdapterRequests calls POST /__parity/adapter/{bidder}/requests.
func (c *Client) AdapterRequests(ctx context.Context, bidder string, in *parity.AdapterRequestsInput) (*parity.AdapterRequestsOutput, []byte, error) {
	var out parity.AdapterRequestsOutput
	raw, err := c.call(ctx, http.MethodPost, routeAdapter+url.PathEscape(bidder)+"/requests", in, &out)
	if err != nil {
		return nil, raw, err
	}
	return &out, raw, nil
}

// AdapterBids calls POST /__parity/adapter/{bidder}/bids.
func (c *Client) AdapterBids(ctx context.Context, bidder string, in *parity.AdapterBidsInput) (*parity.AdapterBidsOutput, []byte, error) {
	var out parity.AdapterBidsOutput
	raw, err := c.call(ctx, http.MethodPost, routeAdapter+url.PathEscape(bidder)+"/bids", in, &out)
	if err != nil {
		return nil, raw, err
	}
	return &out, raw, nil
}

// Config calls GET /__parity/config.
func (c *Client) Config(ctx context.Context) (*parity.ConfigDump, []byte, error) {
	var out parity.ConfigDump
	raw, err := c.call(ctx, http.MethodGet, routeConfig, nil, &out)
	if err != nil {
		return nil, raw, err
	}
	return &out, raw, nil
}

// Bidders calls GET /__parity/bidders.
func (c *Client) Bidders(ctx context.Context) (*parity.BiddersDump, []byte, error) {
	var out parity.BiddersDump
	raw, err := c.call(ctx, http.MethodGet, routeBidders, nil, &out)
	if err != nil {
		return nil, raw, err
	}
	return &out, raw, nil
}

// Reset calls POST /__parity/reset.
func (c *Client) Reset(ctx context.Context) (*parity.ResetResult, error) {
	var out parity.ResetResult
	if _, err := c.call(ctx, http.MethodPost, routeReset, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// call sends one request and decodes a 200 response into out. It returns the
// raw response body in every case where one arrived.
func (c *Client) call(ctx context.Context, method, route string, in, out any) ([]byte, error) {
	var body io.Reader
	if in != nil {
		raw, err := json.Marshal(in)
		if err != nil {
			return nil, fmt.Errorf("runner: encoding %s request: %w", route, err)
		}
		body = bytes.NewReader(raw)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.base+parity.PathPrefix+route, body)
	if err != nil {
		return nil, fmt.Errorf("runner: building %s request: %w", route, err)
	}
	req.Header.Set(parity.TokenHeader, c.token)
	if in != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("runner: %s: %w", route, err)
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, maxResponseBytes))
	if err != nil {
		return nil, fmt.Errorf("runner: reading %s response: %w", route, err)
	}
	if resp.StatusCode != http.StatusOK {
		return raw, &StatusError{Route: parity.PathPrefix + route, Status: resp.StatusCode, Message: errorMessage(raw)}
	}
	if err := json.Unmarshal(raw, out); err != nil {
		return raw, &answeredError{fmt.Errorf("decoding %s response: %w", route, err)}
	}
	return raw, nil
}

// errorMessage extracts {"error": msg}, else a prefix of the body.
func errorMessage(raw []byte) string {
	var doc struct {
		Error string `json:"error"`
	}
	if json.Unmarshal(raw, &doc) == nil && doc.Error != "" {
		return doc.Error
	}
	const max = 200
	s := strings.TrimSpace(string(bytes.ToValidUTF8(raw, nil)))
	if len(s) > max {
		s = s[:max] + "..."
	}
	return s
}
