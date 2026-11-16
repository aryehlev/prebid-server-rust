package transport

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"regexp"
	"sort"
	"syscall"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/trace"
)

// Built-in inline defaults when the envelope has no matching mock.
const (
	defaultBidderStatus = http.StatusNoContent
	defaultCacheStatus  = http.StatusServiceUnavailable
)

// inlineAnswer is the resolved mock for one call. A nil response means the
// built-in "network disabled" failure.
type inlineAnswer struct {
	response *parity.InlineResponse
	ref      parity.MockRef
	notes    []string
}

func (t *Transport) inline(req *http.Request, sess *trace.Session, info callInfo) (*http.Response, error) {
	reqBody, readErr := drainBody(req)
	call := parity.OutboundCall{
		Seq:     info.seq,
		Impl:    parity.ImplGo,
		Kind:    info.kind,
		Bidder:  info.bidder,
		OrigURL: info.origURL,
		Request: parity.OutboundRequest{
			Method: req.Method,
			URL:    info.origURL,
			Proto:  req.Proto,
			Header: HeaderListOf(req.Header),
			Body:   parity.NewBody(reqBody, req.Header.Get("Content-Encoding")),
		},
	}
	if readErr != nil {
		call.Notes = append(call.Notes, "reading request body: "+readErr.Error())
	}
	answer := resolveInline(sess.Envelope().Mock.Inline, info)
	call.Mock = &answer.ref
	call.Notes = append(call.Notes, answer.notes...)

	resp, outResp, failClass, err := respond(req, answer.response)
	call.Response = outResp
	if failClass != "" {
		call.Error = &failClass
	}
	sess.AddOutbound(call)
	return resp, err
}

func resolveInline(mocks *parity.InlineMocks, info callInfo) inlineAnswer {
	if mocks == nil {
		mocks = &parity.InlineMocks{}
	}
	var notes []string
	for i := range mocks.Rules {
		rule := &mocks.Rules[i]
		matched, err := ruleMatches(rule, info)
		if err != nil {
			notes = append(notes, fmt.Sprintf("rules[%d]: %v", i, err))
			continue
		}
		if matched {
			id := rule.ID
			if id == "" {
				id = fmt.Sprintf("rules[%d]", i)
			}
			return inlineAnswer{response: &rule.Response, ref: parity.MockRef{Source: parity.MockSourceRule, ID: id}, notes: notes}
		}
	}
	answer := resolveByKind(mocks, info)
	answer.notes = append(notes, answer.notes...)
	return answer
}

func resolveByKind(mocks *parity.InlineMocks, info callInfo) inlineAnswer {
	inline := func(r parity.InlineResponse, id string) inlineAnswer {
		return inlineAnswer{response: &r, ref: parity.MockRef{Source: parity.MockSourceInline, ID: id}}
	}
	builtin := func(status int, id string) inlineAnswer {
		return inlineAnswer{response: &parity.InlineResponse{Status: status}, ref: parity.MockRef{Source: parity.MockSourceDefault, ID: id}}
	}
	switch info.kind {
	case parity.OutboundBidder:
		if r, ok := mocks.Bidders[info.bidder]; ok {
			return inline(r, "bidders."+info.bidder)
		}
		if mocks.DefaultBidder != nil {
			return inline(*mocks.DefaultBidder, "default_bidder")
		}
		return builtin(defaultBidderStatus, parity.OutboundBidder)
	case parity.OutboundCache:
		if mocks.Cache != nil {
			return inline(*mocks.Cache, "cache")
		}
		return builtin(defaultCacheStatus, parity.OutboundCache)
	}
	if r, ok := mocks.Kinds[info.kind]; ok {
		return inline(r, "kinds."+info.kind)
	}
	return inlineAnswer{ref: parity.MockRef{Source: parity.MockSourceDefault, ID: info.kind}}
}

func ruleMatches(rule *parity.InlineRule, info callInfo) (bool, error) {
	if rule.Kind != "" && rule.Kind != info.kind {
		return false, nil
	}
	if rule.Bidder != "" && rule.Bidder != info.bidder {
		return false, nil
	}
	if rule.URIRegex == "" {
		return true, nil
	}
	re, err := regexp.Compile(rule.URIRegex)
	if err != nil {
		return false, fmt.Errorf("invalid uri_regex: %w", err)
	}
	return re.MatchString(info.origURL), nil
}

// respond turns a resolved mock into the client response. It honours delay_ms
// and fail against the request context, so client deadlines behave as they
// would against a slow server.
func respond(req *http.Request, mock *parity.InlineResponse) (*http.Response, *parity.OutboundResponse, string, error) {
	if mock == nil {
		return nil, nil, parity.OutboundErrNetDisabled, ErrNetworkDisabled
	}
	if err := wait(req.Context(), time.Duration(mock.DelayMS)*time.Millisecond); err != nil {
		return nil, nil, contextClass(err), err
	}
	if mock.Fail != nil {
		class, err := failure(req.Context(), *mock.Fail)
		return nil, nil, class, err
	}
	body, err := mock.Body.WireBytes()
	if err != nil {
		return nil, nil, parity.OutboundErrOther, fmt.Errorf("parity: inline mock body: %w", err)
	}
	status := mock.Status
	if status == 0 {
		status = http.StatusOK
	}
	header := HTTPHeaderOf(mock.Headers)
	resp := &http.Response{
		Status:        fmt.Sprintf("%d %s", status, http.StatusText(status)),
		StatusCode:    status,
		Proto:         "HTTP/1.1",
		ProtoMajor:    1,
		ProtoMinor:    1,
		Header:        header,
		Body:          io.NopCloser(bytes.NewReader(body)),
		ContentLength: int64(len(body)),
		Request:       req,
	}
	recorded := &parity.OutboundResponse{
		Status:  status,
		Headers: append(parity.HeaderList{}, mock.Headers...),
		Body:    parity.NewBody(body, header.Get("Content-Encoding")),
		DelayMS: mock.DelayMS,
	}
	return resp, recorded, "", nil
}

func wait(ctx context.Context, d time.Duration) error {
	if d <= 0 {
		return nil
	}
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-timer.C:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func contextClass(err error) string {
	if errors.Is(err, context.DeadlineExceeded) {
		return parity.OutboundErrTimeout
	}
	return parity.OutboundErrOther
}

// timeoutError is a net.Error reporting a timeout, used for fail=timeout when
// the request has no deadline to wait for.
type timeoutError struct{}

func (timeoutError) Error() string   { return "parity: inline mock timeout" }
func (timeoutError) Timeout() bool   { return true }
func (timeoutError) Temporary() bool { return true }

func failure(ctx context.Context, kind string) (string, error) {
	switch kind {
	case parity.OutboundErrTimeout:
		if _, hasDeadline := ctx.Deadline(); hasDeadline {
			<-ctx.Done()
			return contextClass(ctx.Err()), ctx.Err()
		}
		return parity.OutboundErrTimeout, timeoutError{}
	case parity.OutboundErrRefused:
		return parity.OutboundErrRefused, &net.OpError{Op: "dial", Net: "tcp", Err: os.NewSyscallError("connect", syscall.ECONNREFUSED)}
	case parity.OutboundErrReset:
		return parity.OutboundErrReset, &net.OpError{Op: "read", Net: "tcp", Err: os.NewSyscallError("read", syscall.ECONNRESET)}
	default:
		return parity.OutboundErrOther, errors.New("parity: inline mock failure")
	}
}

func drainBody(req *http.Request) ([]byte, error) {
	if req.Body == nil || req.Body == http.NoBody {
		return nil, nil
	}
	defer req.Body.Close()
	return io.ReadAll(req.Body)
}

// HeaderListOf flattens an http.Header into a HeaderList sorted by canonical
// name, keeping value order within a name. http.Header has already lost the
// original order and case, which is why inline captures are wire=false.
func HeaderListOf(h http.Header) parity.HeaderList {
	names := make([]string, 0, len(h))
	for name := range h {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make(parity.HeaderList, 0, len(h))
	for _, name := range names {
		for _, v := range h[name] {
			out = append(out, [2]string{name, v})
		}
	}
	return out
}

// HTTPHeaderOf converts a HeaderList to an http.Header, canonicalizing names and
// keeping value order.
func HTTPHeaderOf(list parity.HeaderList) http.Header {
	h := make(http.Header, len(list))
	for _, kv := range list {
		h.Add(kv[0], kv[1])
	}
	return h
}
