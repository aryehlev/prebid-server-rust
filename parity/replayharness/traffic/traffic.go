// Package traffic compacts a ReplayHarness recording (the JSONL file that
// `replayharness record` writes) into the file the repository keeps.
//
// Every line is one exchange: {time, request, response, durationMs}. Compact
// keeps each line's bytes exactly as ReplayHarness wrote them and drops:
//
//   - an exchange whose request repeats an earlier one;
//   - an exchange whose recorded request body is longer than
//     Options.MaxRequestBody. These are size probes: the recorder truncates
//     bodies at 1 MiB, so the biggest ones are no longer what was sent, and
//     the replays and probes that sent them send them again on every run;
//   - an exchange whose response body is at least Options.LargeResponse long
//     and repeats one already kept for the same method, path and status, such
//     as the same static schema file fetched with an unknown query parameter.
//
// It refuses a recording that carries a secret: a credential header, or any
// of the given secret values (the parity token) anywhere in a line.
package traffic

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
)

// maxLine bounds one recorded exchange. The recorder truncates each body to
// 1 MiB, and base64 grows it by a third, so 8 MiB leaves room for both.
const maxLine = 8 << 20

// Errors Compact reports.
var (
	ErrSecret   = errors.New("traffic: the recording holds a secret")
	ErrTooLarge = errors.New("traffic: the compacted recording is too large")
	ErrBadLine  = errors.New("traffic: a line is not a recorded exchange")
)

// sensitiveHeaders are the headers ReplayHarness leaves out of recordings
// (internal/discover/traffic.go, SensitiveHeaders), plus the parity token
// header, which ReplayHarness does not know.
func sensitiveHeaders() []string {
	return []string{
		"Authorization", "Proxy-Authorization", "Cookie", "Set-Cookie",
		"X-Api-Key", "Api-Key", "X-Auth-Token", "X-Csrf-Token", "X-Xsrf-Token",
		"X-Parity-Token",
	}
}

// Options configure Compact.
type Options struct {
	// Secrets are values that must not appear anywhere in the recording.
	Secrets []string
	// MaxBytes caps the compacted output; 0 means no cap.
	MaxBytes int
	// MaxRequestBody drops an exchange whose recorded request body is longer
	// than this many bytes; 0 keeps every body.
	MaxRequestBody int
	// LargeResponse is the response body length from which a response that
	// repeats one already kept for the same method, path and status drops
	// its exchange; 0 keeps every response.
	LargeResponse int
}

// Stats describe one compaction.
type Stats struct {
	Read              int
	Kept              int
	Duplicates        int
	LargeRequests     int
	RepeatedResponses int
	Bytes             int
}

// request is the part of an exchange that identifies it.
type request struct {
	Method       string            `json:"method"`
	Path         string            `json:"path"`
	Query        string            `json:"query"`
	Headers      map[string]string `json:"headers"`
	Body         string            `json:"body"`
	BodyEncoding string            `json:"bodyEncoding"`
}

type response struct {
	Status       int               `json:"status"`
	Headers      map[string]string `json:"headers"`
	Body         string            `json:"body"`
	BodyEncoding string            `json:"bodyEncoding"`
}

type exchange struct {
	Request  *request  `json:"request"`
	Response *response `json:"response"`
}

// compactor holds the state of one Compact call.
type compactor struct {
	opts      Options
	stats     Stats
	requests  map[string]bool
	responses map[string]bool
	kept      bytes.Buffer
}

// Compact reads recordings from in, in order, and writes the exchanges it
// keeps to out, unchanged.
func Compact(in []io.Reader, out io.Writer, opts Options) (Stats, error) {
	c := &compactor{opts: opts, requests: map[string]bool{}, responses: map[string]bool{}}

	for _, r := range in {
		if err := eachLine(r, c.add); err != nil {
			return c.stats, err
		}
	}

	c.stats.Bytes = c.kept.Len()
	if opts.MaxBytes > 0 && c.stats.Bytes > opts.MaxBytes {
		return c.stats, fmt.Errorf("%w: %d bytes, the cap is %d", ErrTooLarge, c.stats.Bytes, opts.MaxBytes)
	}

	if _, err := out.Write(c.kept.Bytes()); err != nil {
		return c.stats, fmt.Errorf("traffic: writing: %w", err)
	}

	return c.stats, nil
}

// add checks one line and keeps it unless a rule drops it.
func (c *compactor) add(n int, line []byte) error {
	c.stats.Read++

	ex, err := parseLine(line, c.opts.Secrets)
	if err != nil {
		return fmt.Errorf("line %d: %w", n, err)
	}

	key, err := requestKey(ex.Request)
	if err != nil {
		return err
	}

	switch {
	case c.requests[key]:
		c.stats.Duplicates++
	case c.opts.MaxRequestBody > 0 && len(ex.Request.Body) > c.opts.MaxRequestBody:
		c.requests[key] = true
		c.stats.LargeRequests++
	case c.repeatsLargeResponse(ex):
		c.requests[key] = true
		c.stats.RepeatedResponses++
	default:
		c.requests[key] = true
		c.stats.Kept++

		c.kept.Write(line)
		c.kept.WriteByte('\n')
	}

	return nil
}

// repeatsLargeResponse reports whether ex has a large response body that an
// exchange already kept for the same method, path and status had, and
// otherwise remembers the body.
func (c *compactor) repeatsLargeResponse(ex exchange) bool {
	res := ex.Response
	if c.opts.LargeResponse <= 0 || res == nil || len(res.Body) < c.opts.LargeResponse {
		return false
	}

	sum := sha256.Sum256([]byte(res.BodyEncoding + "\x00" + res.Body))
	key := fmt.Sprintf("%s %s %d %x", ex.Request.Method, ex.Request.Path, res.Status, sum)

	if c.responses[key] {
		return true
	}

	c.responses[key] = true

	return false
}

// eachLine calls fn with every non-empty line of r and its 1-based number.
func eachLine(r io.Reader, fn func(n int, line []byte) error) error {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), maxLine)

	for n := 1; sc.Scan(); n++ {
		line := bytes.TrimSpace(sc.Bytes())
		if len(line) == 0 {
			continue
		}

		if err := fn(n, line); err != nil {
			return err
		}
	}

	if err := sc.Err(); err != nil {
		return fmt.Errorf("traffic: reading: %w", err)
	}

	return nil
}

// parseLine refuses a line with a secret and returns its exchange.
func parseLine(line []byte, secrets []string) (exchange, error) {
	var ex exchange

	for _, s := range secrets {
		if s != "" && bytes.Contains(line, []byte(s)) {
			return ex, fmt.Errorf("%w: a secret value appears in the line", ErrSecret)
		}
	}

	if err := json.Unmarshal(line, &ex); err != nil {
		return ex, fmt.Errorf("%w: %w", ErrBadLine, err)
	}

	if ex.Request == nil || ex.Request.Method == "" || !strings.HasPrefix(ex.Request.Path, "/") {
		return ex, fmt.Errorf("%w: no request method and path", ErrBadLine)
	}

	return ex, checkHeaders(ex)
}

// checkHeaders refuses an exchange that recorded a credential header.
func checkHeaders(ex exchange) error {
	if name := sensitive(ex.Request.Headers); name != "" {
		return fmt.Errorf("%w: request header %s", ErrSecret, name)
	}

	if ex.Response == nil {
		return nil
	}

	if name := sensitive(ex.Response.Headers); name != "" {
		return fmt.Errorf("%w: response header %s", ErrSecret, name)
	}

	return nil
}

// sensitive returns the first credential header of h, or "".
func sensitive(h map[string]string) string {
	for name := range h {
		for _, s := range sensitiveHeaders() {
			if http.CanonicalHeaderKey(name) == s {
				return s
			}
		}
	}

	return ""
}

// requestKey identifies a request: method, path, query, headers in name
// order, and body.
func requestKey(r *request) (string, error) {
	names := make([]string, 0, len(r.Headers))
	for name := range r.Headers {
		names = append(names, name)
	}

	sort.Strings(names)

	headers := make([][2]string, 0, len(names))
	for _, name := range names {
		headers = append(headers, [2]string{http.CanonicalHeaderKey(name), r.Headers[name]})
	}

	key, err := json.Marshal([]any{r.Method, r.Path, r.Query, headers, r.BodyEncoding, r.Body})
	if err != nil {
		return "", fmt.Errorf("traffic: request key: %w", err)
	}

	return string(key), nil
}
