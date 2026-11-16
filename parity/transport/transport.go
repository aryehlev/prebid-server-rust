// Package transport is the parity RoundTripper. In parity mode every outbound
// client of the Go server is wrapped by it, so no call ever reaches the real
// network:
//
//   - Oracle mode (default): the request is cloned, an https URL is rewritten to
//     http, X-Parity-Meta is added, and the clone is sent through a clone of the
//     client's own production *http.Transport whose Proxy is the oracle. The
//     oracle therefore sees Go's real wire defaults (User-Agent
//     Go-http-client/1.1, automatic Accept-Encoding: gzip, header casing).
//   - Inline mode (envelope.mock.mode = inline): the response comes from the
//     envelope and the call is recorded to the session's http_out with wire=false.
//
// Calls made while no case is in flight (startup GVL preload, the first currency
// fetch, ticker tasks) go to the oracle with an empty case ID. Without an oracle
// URL such calls fail with "parity: network disabled".
package transport

import (
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
)

// ErrNetworkDisabled is returned for calls that have nowhere to go.
var ErrNetworkDisabled = errors.New("parity: network disabled")

// originalDefault is http.DefaultTransport as it was before parity bootstrap
// possibly replaced it, used as the template for clients without a transport.
var originalDefault = http.DefaultTransport

// Options configures one wrapped client.
type Options struct {
	// Kind is the client label used when the call has no bidder in its context.
	Kind string
	// Classify optionally refines Kind per request (used for the shared general
	// client); it returns "" to keep Kind.
	Classify func(*http.Request) string
}

// Transport is the parity RoundTripper.
type Transport struct {
	kind      string
	classify  func(*http.Request) string
	base      *http.Transport
	oracle    *http.Transport
	oracleErr error
}

// Wrap returns base wrapped for parity mode with the given client kind.
func Wrap(base http.RoundTripper, kind string, cfg config.ParityConfig) http.RoundTripper {
	return New(base, Options{Kind: kind}, cfg)
}

// New builds a Transport. base supplies the production settings (dialer, pool,
// timeouts, TLS) that the oracle transport clones; it is never used to reach the
// network itself. An invalid or empty oracle URL is not fatal here: oracle-mode
// calls then fail with a descriptive error instead of escaping to the network.
func New(base http.RoundTripper, opts Options, cfg config.ParityConfig) *Transport {
	t := &Transport{
		kind:     opts.Kind,
		classify: opts.Classify,
		base:     productionTemplate(base),
	}
	if t.kind == "" {
		t.kind = parity.OutboundGeneral
	}
	t.oracle, t.oracleErr = oracleTransport(t.base, cfg.OracleURL)
	return t
}

// Kind returns the client label.
func (t *Transport) Kind() string { return t.kind }

// RoundTrip implements http.RoundTripper.
func (t *Transport) RoundTrip(req *http.Request) (*http.Response, error) {
	sess := trace.Current()
	info := t.describe(req, sess)
	if sess != nil && sess.Envelope().Mock.EffectiveMode() == parity.MockModeInline {
		return t.inline(req, sess, info)
	}
	return t.viaOracle(req, info)
}

// CloseIdleConnections closes idle connections to the oracle.
func (t *Transport) CloseIdleConnections() {
	if t.oracle != nil {
		t.oracle.CloseIdleConnections()
	}
}

type callInfo struct {
	caseID  string
	kind    string
	bidder  string
	origURL string
	seq     int
}

func (t *Transport) describe(req *http.Request, sess *trace.Session) callInfo {
	info := callInfo{caseID: sess.CaseID(), kind: t.kind, origURL: req.URL.String(), seq: sess.NextSeq()}
	if bidder, ok := trace.BidderFrom(req.Context()); ok {
		info.kind, info.bidder = parity.OutboundBidder, bidder
		return info
	}
	if t.classify != nil {
		if refined := t.classify(req); refined != "" {
			info.kind = refined
		}
	}
	return info
}

func (t *Transport) viaOracle(req *http.Request, info callInfo) (*http.Response, error) {
	if t.oracle == nil {
		closeBody(req)
		return nil, t.oracleErr
	}
	meta, err := EncodeMeta(parity.OutboundMeta{
		V:       MetaVersion,
		Case:    info.caseID,
		Impl:    parity.ImplGo,
		Kind:    info.kind,
		Bidder:  info.bidder,
		OrigURL: info.origURL,
		Seq:     info.seq,
	})
	if err != nil {
		closeBody(req)
		return nil, err
	}
	out := req.Clone(req.Context())
	if strings.EqualFold(out.URL.Scheme, "https") {
		out.URL.Scheme = "http"
	}
	out.Header.Set(parity.MetaHeader, meta)
	return t.oracle.RoundTrip(out)
}

// productionTemplate returns a clone of the production transport behind base.
func productionTemplate(base http.RoundTripper) *http.Transport {
	switch b := base.(type) {
	case *Transport:
		return b.base.Clone()
	case *http.Transport:
		return b.Clone()
	}
	if def, ok := originalDefault.(*http.Transport); ok {
		return def.Clone()
	}
	return &http.Transport{}
}

func oracleTransport(template *http.Transport, oracleURL string) (*http.Transport, error) {
	if oracleURL == "" {
		return nil, fmt.Errorf("%w: parity.oracle_url is not configured", ErrNetworkDisabled)
	}
	proxy, err := url.Parse(oracleURL)
	if err != nil {
		return nil, fmt.Errorf("%w: invalid parity.oracle_url: %v", ErrNetworkDisabled, err)
	}
	if proxy.Scheme != "http" || proxy.Host == "" {
		return nil, fmt.Errorf("%w: parity.oracle_url must be an absolute http URL, got %q", ErrNetworkDisabled, oracleURL)
	}
	oracle := template.Clone()
	oracle.Proxy = http.ProxyURL(proxy)
	return oracle, nil
}

func closeBody(req *http.Request) {
	if req.Body != nil {
		req.Body.Close()
	}
}

// PrefixRule maps a URL prefix to an outbound kind.
type PrefixRule struct {
	Prefix string
	Kind   string
}

// PrefixClassifier returns a Classify function that labels a request with the
// kind of the first rule whose prefix its original URL starts with.
func PrefixClassifier(rules []PrefixRule) func(*http.Request) string {
	kept := make([]PrefixRule, 0, len(rules))
	for _, r := range rules {
		if r.Prefix != "" && r.Kind != "" {
			kept = append(kept, r)
		}
	}
	return func(req *http.Request) string {
		u := req.URL.String()
		for _, r := range kept {
			if strings.HasPrefix(u, r.Prefix) {
				return r.Kind
			}
		}
		return ""
	}
}
