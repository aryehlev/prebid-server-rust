package corpus

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

// Routes whose answers can depend on the wall clock or on map order.
const (
	rhSetUIDPath     = "/setuid"
	rhCookieSyncPath = "/cookie_sync"
	// rhDeprecationCookie is the cookie /cookie_sync sets for an account
	// with privacysandbox.cookiedeprecation on (endpoints/cookie_sync.go).
	rhDeprecationCookie = "receive-cookie-deprecation"
)

// Why requests are not compared.
const (
	reasonUIDsCookie = "Go may answer it with the uids Set-Cookie, whose Expires and per-uid expires come from the " +
		"wall clock (usersync/cookie.go WriteCookie and Sync); no fake or normalization rule covers them"
	reasonUIDsOptOut  = " (traffic.jsonl recorded 401, but the recorder dropped the opt-out Cookie behind it)"
	reasonDeprecation = "the account turns on privacysandbox.cookiedeprecation, so Go sets " + rhDeprecationCookie +
		" with an Expires from the wall clock (endpoints/cookie_sync.go setCookieDeprecationHeader); " +
		"no fake or normalization rule covers it"
	reasonSyncerOrder = "it lists no bidders (or turns coopSync on), so Go evaluates the syncers in the order " +
		"NewChooser got them ranging over a map (usersync/chooser.go); usersync.shuffle=identity keeps that order, " +
		"which differs between two processes"
	reasonExecFraming = "the body is max_request_size + 1 bytes: net/http hands the last byte over with io.EOF, so the " +
		"auction's size check passes and the body is parsed cut (endpoints/openrtb2/auction.go), while " +
		"/__parity/exec reads the body from a bytes.Reader, which returns io.EOF only on the next read; " +
		"the case would compare the harness, not the server"
)

// verdict returns the tag and reason of a request both servers cannot be
// compared on, or two empty strings.
func (l *rhLoader) verdict(e *rhEntry) (string, string) {
	r := e.req

	if reason := requestLineProblem(r); reason != "" {
		return RHRequestLine, reason
	}

	if e.truncated {
		return RHTruncated, fmt.Sprintf("the recorder cut the body at %d bytes, so it is not the body that was sent",
			rhreplay.RecorderBodyLimit)
	}

	return l.routeVerdict(e)
}

// routeVerdict classifies a request by the route it reaches.
func (l *rhLoader) routeVerdict(e *rhEntry) (string, string) {
	r := e.req

	switch {
	case r.method == http.MethodPost && r.path == EndpointAuction && len(r.body) == rhMaxRequestSize+1:
		return RHExecFraming, reasonExecFraming
	case r.method == http.MethodGet && r.path == rhSetUIDPath:
		if write, optOut := mayWriteUIDs(e.statuses); write {
			reason := reasonUIDsCookie
			if optOut {
				reason += reasonUIDsOptOut
			}

			return RHWallClock, reason
		}
	case r.method == http.MethodPost && r.path == rhCookieSyncPath:
		return l.cookieSyncVerdict(e)
	}

	return "", ""
}

// requestLineProblem returns why /__parity/exec cannot carry the request as
// Go's server receives it, or "".
func requestLineProblem(r rhRequest) string {
	if !httpToken(r.method) {
		return fmt.Sprintf("the method %q is not an HTTP token", r.method)
	}

	if !strings.HasPrefix(r.path, "/") {
		return "the path does not start with /"
	}

	target := r.path
	if r.query != "" {
		target += "?" + r.query
	}

	if reason := targetProblem(target); reason != "" {
		return reason
	}

	for _, kv := range r.headers {
		if !httpToken(kv[0]) || strings.ContainsAny(kv[1], "\r\n\x00") {
			return fmt.Sprintf("the header %q cannot be sent", kv[0])
		}
	}

	return ""
}

// targetProblem returns why the request target cannot be carried, or "".
func targetProblem(target string) string {
	for i := range len(target) {
		if c := target[i]; c <= ' ' || c == 0x7f {
			return "net/http answers 400 before routing to a request line whose target has a space or a control " +
				"byte, and /__parity/exec would route it"
		}
	}

	if _, err := url.ParseRequestURI(target); err != nil {
		return "the request target does not parse: " + err.Error()
	}

	return ""
}

// httpToken reports whether s is a non-empty RFC 9110 token.
func httpToken(s string) bool {
	if s == "" {
		return false
	}

	for i := range len(s) {
		if !tokenByte(s[i]) {
			return false
		}
	}

	return true
}

func tokenByte(c byte) bool {
	return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || strings.IndexByte("!#$%&'*+-.^_`|~", c) >= 0
}

// mayWriteUIDs reports whether Go may answer a GET /setuid with the uids
// cookie, and whether only a recorded 401 says otherwise. A 200 writes it.
// Another status from a replay expectation, or recorded in traffic.jsonl,
// settles that it does not, except a recorded 401: the only 401 is the
// opted-out uids cookie, which the recorder drops, so the request as
// recorded reaches the sync path.
func mayWriteUIDs(statuses []rhStatus) (bool, bool) {
	settled, optOut := false, false

	for _, s := range statuses {
		switch {
		case s.spec == "200" || s.spec == "2xx":
			return true, false
		case s.recorded && s.spec == "401":
			optOut = true
		case errorStatus(s.spec):
			settled = true
		}
	}

	return !settled, optOut && !settled
}

// errorStatus reports whether a status code or class is 3xx, 4xx or 5xx.
func errorStatus(spec string) bool {
	if len(spec) == 3 && spec[1:] == "xx" {
		return spec[0] >= '3' && spec[0] <= '5'
	}

	n, err := strconv.Atoi(spec)

	return err == nil && n >= 300
}

// cookieSyncVerdict classifies a POST /cookie_sync.
func (l *rhLoader) cookieSyncVerdict(e *rhEntry) (string, string) {
	var body map[string]any
	if json.Unmarshal(e.req.body, &body) != nil {
		// Go answers 400 before it reads the account or chooses syncers.
		return "", ""
	}

	if l.deprecationAccount(body) && !hasCookie(e.req, rhDeprecationCookie) {
		return RHWallClock, reasonDeprecation
	}

	if !biddersListed(body) && !requestRejected(e.statuses) {
		return RHMapOrder, reasonSyncerOrder
	}

	return "", ""
}

// deprecationAccount reports whether the body names an account that turns
// cookie deprecation on. Go matches JSON keys without regard to case.
func (l *rhLoader) deprecationAccount(body map[string]any) bool {
	for k, v := range body {
		if id, ok := v.(string); ok && strings.EqualFold(k, "account") && l.sandbox[id] {
			return true
		}
	}

	return false
}

// biddersListed reports whether every bidders member is a non-empty list
// and coopSync is not true: then the syncers Go evaluates, and their order,
// follow the request.
func biddersListed(body map[string]any) bool {
	listed := false

	for k, v := range body {
		switch {
		case strings.EqualFold(k, "bidders"):
			list, ok := v.([]any)
			if !ok || len(list) == 0 {
				return false
			}

			listed = true
		case strings.EqualFold(k, "coopSync") && v == true:
			return false
		}
	}

	return listed
}

// requestRejected reports whether a source shows Go rejects the request
// with a 4xx before it chooses syncers. A recorded 401 does not: the
// recorder dropped the opt-out cookie behind it.
func requestRejected(statuses []rhStatus) bool {
	for _, s := range statuses {
		if s.recorded && s.spec == "401" {
			continue
		}

		if s.spec == "4xx" {
			return true
		}

		if n, err := strconv.Atoi(s.spec); err == nil && n >= 400 && n < 500 {
			return true
		}
	}

	return false
}

// hasCookie reports whether a Cookie header of the request sets name.
func hasCookie(r rhRequest, name string) bool {
	for _, kv := range r.headers {
		if kv[0] != "Cookie" {
			continue
		}

		for part := range strings.SplitSeq(kv[1], ";") {
			if strings.HasPrefix(strings.TrimSpace(part), name+"=") {
				return true
			}
		}
	}

	return false
}

// cookieDeprecationEnabled reports whether an account turns on
// privacy.privacysandbox.cookiedeprecation.enabled.
func cookieDeprecationEnabled(raw json.RawMessage) bool {
	var v any
	if json.Unmarshal(raw, &v) != nil {
		return false
	}

	for _, name := range []string{"privacy", "privacysandbox", "cookiedeprecation", "enabled"} {
		v = member(v, name)
	}

	return v == true
}

// member returns the member of an object whose key matches name without
// regard to case, as Go's JSON decoding does, or nil.
func member(v any, name string) any {
	obj, ok := v.(map[string]any)
	if !ok {
		return nil
	}

	for k, val := range obj {
		if strings.EqualFold(k, name) {
			return val
		}
	}

	return nil
}
