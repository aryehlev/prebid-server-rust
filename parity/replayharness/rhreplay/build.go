package rhreplay

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

// Builtin variables every replay starts with (internal/replay/run.go
// BuiltinVars): a random ID and the wall clock in milliseconds. A step that
// uses one sends a different request on every run.
const (
	BuiltinUnique    = "unique"
	BuiltinTimestamp = "timestamp"
)

var varRe = regexp.MustCompile(`\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}`)

// Request is a request as ReplayHarness records and sends it
// (internal/model/model.go Request).
type Request struct {
	Method       string            `json:"method"`
	Path         string            `json:"path"`
	Query        string            `json:"query,omitempty"`
	Headers      map[string]string `json:"headers,omitempty"`
	Body         string            `json:"body,omitempty"`
	BodyEncoding string            `json:"bodyEncoding,omitempty"`
}

// BodyBytes returns the decoded body. A base64 body that does not decode is
// taken as text, like ReplayHarness does.
func (r Request) BodyBytes() []byte {
	return decodeBody(r.Body, r.BodyEncoding)
}

func decodeBody(body, encoding string) []byte {
	if encoding == "base64" {
		if b, err := base64.StdEncoding.DecodeString(body); err == nil {
			return b
		}
	}

	return []byte(body)
}

// VarRefs returns the variable names a step refers to, sorted and without
// repeats: in its path, query values, header values and body (keys of a json
// body are not filled in, so they are not looked at).
func VarRefs(st Step) []string {
	seen := map[string]bool{}
	add := func(s string) {
		for _, m := range varRe.FindAllStringSubmatch(s, -1) {
			seen[m[1]] = true
		}
	}

	add(st.Path)

	for _, v := range st.Query {
		items, ok := v.([]any)
		if !ok {
			items = []any{v}
		}

		for _, item := range items {
			add(Scalar(item))
		}
	}

	for _, v := range st.Headers {
		add(v)
	}

	if st.Body != nil {
		add(*st.Body)
	}

	walkStrings(st.JSON, add)

	out := make([]string, 0, len(seen))
	for name := range seen {
		out = append(out, name)
	}

	sort.Strings(out)

	return out
}

func walkStrings(v any, visit func(string)) {
	switch t := v.(type) {
	case string:
		visit(t)
	case map[string]any:
		for _, val := range t {
			walkStrings(val, visit)
		}
	case []any:
		for _, val := range t {
			walkStrings(val, visit)
		}
	}
}

// BuildRequest turns a step into the request ReplayHarness builds for it,
// filling in saved variables (internal/replay/run.go BuildRequest).
func BuildRequest(st Step, vars map[string]any) (Request, error) {
	req := Request{Method: st.Method}

	p, q, err := buildTarget(st, vars)
	if err != nil {
		return req, err
	}

	req.Path, req.Query = p, q

	if req.Headers, err = buildHeaders(st, vars); err != nil {
		return req, err
	}

	if err := setBody(&req, st, vars); err != nil {
		return req, err
	}

	if len(req.Headers) == 0 {
		req.Headers = nil
	}

	return req, nil
}

// buildTarget fills in the path and the query of a step.
func buildTarget(st Step, vars map[string]any) (string, string, error) {
	raw := st.Path
	// A path that starts with a saved path (a Location header) uses it as is.
	if m := varRe.FindStringSubmatchIndex(raw); m != nil && m[0] == 0 {
		if v, ok := vars[raw[m[2]:m[3]]]; ok && strings.HasPrefix(Scalar(v), "/") {
			raw = Scalar(v) + raw[m[1]:]
		}
	}

	rawPath, rawQuery, _ := strings.Cut(raw, "?")

	p, err := substitute(rawPath, vars, url.PathEscape)
	if err != nil {
		return "", "", err
	}

	if !strings.HasPrefix(p, "/") {
		return "", "", fmt.Errorf("%w: path %q does not start with / after filling in variables", ErrNotSendable, p)
	}

	q, err := buildQuery(st, rawQuery, vars)
	if err != nil {
		return "", "", err
	}

	return p, q, nil
}

// buildHeaders fills in the header values of a step. The map is never nil,
// so setBody can add a Content-Type.
func buildHeaders(st Step, vars map[string]any) (map[string]string, error) {
	h := make(map[string]string, len(st.Headers))

	for k, v := range st.Headers {
		s, err := substitute(v, vars, nil)
		if err != nil {
			return nil, err
		}

		h[k] = s
	}

	return h, nil
}

func buildQuery(st Step, rawQuery string, vars map[string]any) (string, error) {
	q, err := substitute(rawQuery, vars, url.QueryEscape)
	if err != nil {
		return "", err
	}

	if len(st.Query) == 0 {
		return q, nil
	}

	vals := url.Values{}

	for k, v := range st.Query {
		items, ok := v.([]any)
		if !ok {
			items = []any{v}
		}

		for _, item := range items {
			s, err := substitute(Scalar(item), vars, nil)
			if err != nil {
				return "", err
			}

			vals.Add(k, s)
		}
	}

	if q != "" {
		q += "&"
	}

	return q + vals.Encode(), nil
}

func setBody(req *Request, st Step, vars map[string]any) error {
	switch {
	case st.JSON != nil:
		v, err := substituteValue(st.JSON, vars)
		if err != nil {
			return err
		}

		b, err := json.Marshal(v)
		if err != nil {
			return fmt.Errorf("%w: encoding the json body: %w", ErrNotSendable, err)
		}

		req.Body = string(b)
		if !hasHeader(req.Headers, "Content-Type") {
			req.Headers["Content-Type"] = "application/json"
		}
	case st.Body != nil:
		b, err := substitute(*st.Body, vars, nil)
		if err != nil {
			return err
		}

		req.Body = b

		t := strings.TrimSpace(b)
		if !hasHeader(req.Headers, "Content-Type") && (strings.HasPrefix(t, "{") || strings.HasPrefix(t, "[")) {
			req.Headers["Content-Type"] = "application/json"
		}
	}

	return nil
}

func hasHeader(h map[string]string, name string) bool {
	for k := range h {
		if strings.EqualFold(k, name) {
			return true
		}
	}

	return false
}

// substitute replaces {{name}} in s; escape, when set, encodes each value.
func substitute(s string, vars map[string]any, escape func(string) string) (string, error) {
	var missing []string

	out := varRe.ReplaceAllStringFunc(s, func(m string) string {
		name := varRe.FindStringSubmatch(m)[1]

		v, ok := vars[name]
		if !ok {
			missing = append(missing, name)

			return m
		}

		str := Scalar(v)
		if escape != nil {
			str = escape(str)
		}

		return str
	})

	if len(missing) > 0 {
		return "", fmt.Errorf("%w: unknown variable {{%s}}", ErrNotSendable, missing[0])
	}

	return out, nil
}

// substituteValue replaces variables inside a JSON value; a string that is
// exactly "{{name}}" takes the saved value with its type.
func substituteValue(v any, vars map[string]any) (any, error) {
	switch t := v.(type) {
	case string:
		if m := varRe.FindStringSubmatch(t); m != nil && m[0] == strings.TrimSpace(t) {
			if val, ok := vars[m[1]]; ok {
				return val, nil
			}
		}

		return substitute(t, vars, nil)
	case map[string]any:
		return substituteMap(t, vars)
	case []any:
		return substituteList(t, vars)
	}

	return v, nil
}

func substituteMap(m, vars map[string]any) (map[string]any, error) {
	out := make(map[string]any, len(m))

	for k, val := range m {
		nv, err := substituteValue(val, vars)
		if err != nil {
			return nil, err
		}

		out[k] = nv
	}

	return out, nil
}

func substituteList(list []any, vars map[string]any) ([]any, error) {
	out := make([]any, len(list))

	for i, val := range list {
		nv, err := substituteValue(val, vars)
		if err != nil {
			return nil, err
		}

		out[i] = nv
	}

	return out, nil
}

// Scalar formats a value the way ReplayHarness writes it into a request
// (internal/replay/run.go scalar).
func Scalar(v any) string {
	switch t := v.(type) {
	case nil:
		return ""
	case string:
		return t
	case float64:
		return strconv.FormatFloat(t, 'f', -1, 64)
	case int:
		return strconv.Itoa(t)
	case int64:
		return strconv.FormatInt(t, 10)
	case bool:
		return strconv.FormatBool(t)
	case []any:
		parts := make([]string, len(t))
		for i, item := range t {
			parts[i] = Scalar(item)
		}

		return strings.Join(parts, " or ")
	}

	b, err := json.Marshal(v)
	if err != nil {
		return ""
	}

	return string(b)
}

// Wire is a request as the server receives it from ReplayHarness's client.
type Wire struct {
	Method string
	// Path and Query are the request target: the URL's escaped path and raw
	// query, as net/http writes them.
	Path  string
	Query string
	// Header holds the headers in canonical form.
	Header http.Header
	Body   []byte
}

// ToWire sends req the way internal/runner/runner.go Client.Do does: the
// URL is the target plus path and query, the User-Agent is set first and
// each request header after it with http.Header.Set. It fails where
// ReplayHarness's client fails, with ErrNotSendable.
func ToWire(req Request, userAgent string) (Wire, error) {
	target := "http://127.0.0.1" + req.Path
	if req.Query != "" {
		target += "?" + req.Query
	}

	u, err := url.Parse(target)
	if err != nil {
		return Wire{}, fmt.Errorf("%w: %w", ErrNotSendable, err)
	}

	if !validMethod(req.Method) {
		return Wire{}, fmt.Errorf("%w: invalid method %q", ErrNotSendable, req.Method)
	}

	h := http.Header{}
	h.Set("User-Agent", userAgent)

	names := make([]string, 0, len(req.Headers))
	for k := range req.Headers {
		names = append(names, k)
	}

	sort.Strings(names)

	set := map[string]string{}

	for _, k := range names {
		canon := http.CanonicalHeaderKey(k)
		if prev, ok := set[canon]; ok {
			return Wire{}, fmt.Errorf("%w: headers %q and %q name one header, and Go's map order picks one at random", ErrNotSendable, prev, k)
		}

		if !validHeaderName(k) || strings.ContainsAny(req.Headers[k], "\r\n\x00") {
			return Wire{}, fmt.Errorf("%w: header %q is not valid on the wire", ErrNotSendable, k)
		}

		set[canon] = k
		h.Set(k, req.Headers[k])
	}

	return Wire{Method: req.Method, Path: u.EscapedPath(), Query: u.RawQuery, Header: h, Body: req.BodyBytes()}, nil
}

func validMethod(m string) bool {
	return m != "" && validHeaderName(m)
}

// validHeaderName reports whether s is an RFC 9110 token, which net/http
// requires of methods and header names.
func validHeaderName(s string) bool {
	if s == "" {
		return false
	}

	for i := range len(s) {
		c := s[i]
		ok := c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || strings.IndexByte("!#$%&'*+-.^_`|~", c) >= 0

		if !ok {
			return false
		}
	}

	return true
}
