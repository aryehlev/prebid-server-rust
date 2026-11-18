package rhreplay

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"regexp"
	"strconv"
	"strings"
)

// maxLine bounds one recorded exchange: the recorder cuts each body at
// 1 MiB, and base64 grows it by a third.
const maxLine = 8 << 20

// RecorderBodyLimit is where `replayharness record` cuts a body; a recorded
// body this long may not be what was sent.
const RecorderBodyLimit = 1 << 20

// Exchange is one line of a recording (internal/discover/traffic.go
// Exchange). Line is its 1-based line number in the file.
type Exchange struct {
	Request  Request  `json:"request"`
	Response Response `json:"response"`
	Line     int      `json:"-"`
}

// Response is a recorded answer (internal/model/model.go RecordedResponse).
type Response struct {
	Status       int               `json:"status"`
	Headers      map[string]string `json:"headers,omitempty"`
	Body         string            `json:"body,omitempty"`
	BodyEncoding string            `json:"bodyEncoding,omitempty"`
}

// BodyBytes returns the decoded body.
func (r Response) BodyBytes() []byte {
	return decodeBody(r.Body, r.BodyEncoding)
}

// ReadRecording reads a JSONL recording; blank lines are skipped.
func ReadRecording(r io.Reader) ([]Exchange, error) {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64<<10), maxLine)

	var out []Exchange

	for n := 1; sc.Scan(); n++ {
		line := bytes.TrimSpace(sc.Bytes())
		if len(line) == 0 {
			continue
		}

		var ex Exchange
		if err := json.Unmarshal(line, &ex); err != nil {
			return nil, fmt.Errorf("%w: line %d: %w", ErrBadRecording, n, err)
		}

		if ex.Request.Method == "" || ex.Request.Path == "" {
			return nil, fmt.Errorf("%w: line %d has no request method or path", ErrBadRecording, n)
		}

		ex.Line = n
		out = append(out, ex)
	}

	if err := sc.Err(); err != nil {
		return nil, fmt.Errorf("%w: %w", ErrBadRecording, err)
	}

	return out, nil
}

// Extract reads a saved value from an answer the way a step's save does
// (internal/replay/run.go extract): "status", "header:Name" (case-insensitive
// name, the values joined with ", ") or a JSON path into the body.
func Extract(resp Response, src string) (any, bool) {
	switch {
	case src == "status":
		return float64(resp.Status), true
	case strings.HasPrefix(strings.ToLower(src), "header:"):
		v := headerOf(resp.Headers, strings.TrimSpace(src[len("header:"):]))

		return v, v != ""
	}

	var body any
	if json.Unmarshal(resp.BodyBytes(), &body) != nil {
		return nil, false
	}

	return lookup(body, src)
}

// headerOf returns the value of the header name, matched case-insensitively.
// Recorded headers are canonical, so at most one key matches.
func headerOf(h map[string]string, name string) string {
	for k, v := range h {
		if strings.EqualFold(k, name) {
			return v
		}
	}

	return ""
}

var segRe = regexp.MustCompile(`\.([^.\[\]]+)|\[(\d+)\]|\["([^"]*)"\]`)

// lookup evaluates the small JSON path language of saves and expectations:
// $, .key, [0], ["key"] and a final .length.
func lookup(v any, jsonPath string) (any, bool) {
	jsonPath = strings.TrimSpace(jsonPath)
	if !strings.HasPrefix(jsonPath, "$") {
		jsonPath = "$." + jsonPath
	}

	rest := jsonPath[1:]
	cur := v

	for rest != "" {
		m := segRe.FindStringSubmatch(rest)
		if m == nil || !strings.HasPrefix(rest, m[0]) {
			return nil, false
		}

		rest = rest[len(m[0]):]

		next, ok := segment(cur, m, rest == "")
		if !ok {
			return nil, false
		}

		cur = next
	}

	return cur, true
}

// segment applies one path segment, a match of segRe, to cur. last reports
// whether the segment ends the path, where .length counts what cur holds
// unless cur has a "length" member.
func segment(cur any, m []string, last bool) (any, bool) {
	bracketKey := strings.HasPrefix(m[0], `["`)
	if m[2] != "" && !bracketKey {
		return index(cur, m[2])
	}

	key := m[1]
	if bracketKey {
		key = m[3]
	}

	if obj, ok := cur.(map[string]any); ok {
		if val, has := obj[key]; has {
			return val, true
		}
	}

	if key == "length" && last && !bracketKey {
		return length(cur)
	}

	return nil, false
}

// index takes element digits of an array. Digits too large for an int are
// past the end, as in ReplayHarness, which uses the maximum Atoi returns.
func index(cur any, digits string) (any, bool) {
	i, err := strconv.Atoi(digits)
	if err != nil {
		return nil, false
	}

	a, ok := cur.([]any)
	if !ok || i >= len(a) {
		return nil, false
	}

	return a[i], true
}

func length(v any) (any, bool) {
	switch t := v.(type) {
	case []any:
		return float64(len(t)), true
	case string:
		return float64(len([]rune(t))), true
	case map[string]any:
		return float64(len(t)), true
	}

	return nil, false
}
