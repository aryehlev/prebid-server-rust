package corpus

import (
	"fmt"
	"net/url"
	"strings"

	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
)

// verdict returns the tag and reason of a request both servers cannot be
// compared on, or two empty strings.
func verdict(e *rhEntry) (string, string) {
	r := e.req

	if reason := requestLineProblem(r); reason != "" {
		return RHRequestLine, reason
	}

	if e.truncated {
		return RHTruncated, fmt.Sprintf("the recorder cut the body at %d bytes, so it is not the body that was sent",
			rhreplay.RecorderBodyLimit)
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
