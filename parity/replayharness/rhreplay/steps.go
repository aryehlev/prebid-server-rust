package rhreplay

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
)

// HopHeaders are the request headers `replayharness record` leaves out of a
// recording (internal/discover/traffic.go hopHeaders): net/http's transport
// writes them, or they describe one connection.
func HopHeaders() []string {
	return []string{
		"Connection", "Keep-Alive", "Proxy-Connection", "Transfer-Encoding", "Upgrade",
		"Te", "Trailer", "Host", "Content-Length", "Accept-Encoding",
	}
}

// Built is one replay step rebuilt offline.
type Built struct {
	// Replay and Step index the replay and its step.
	Replay int
	Step   int
	// Wire is what the step sends. It is set when NotSent is empty.
	Wire Wire
	// NotSent says why the step cannot be rebuilt as ReplayHarness sends it.
	NotSent string
}

// Answers returns Go's recorded answer to the request a saving step sends.
type Answers func(Wire) (Response, bool)

// BuildAll rebuilds every step of replays, in order, as ReplayHarness's
// client sends it with userAgent. Variables a step saves come from Go's
// recorded answer to that step, which answers looks up.
//
// It follows internal/replay/run.go runOne: a step that cannot be built or
// sent, or that cannot save a value from its answer, ends its replay, so the
// steps after it are never sent. A step that uses the {{unique}} or
// {{timestamp}} builtin sends a new value on every run, and a step that
// needs a value answers holds no answer for is sent live but cannot be
// rebuilt offline; those steps get a NotSent reason and the replay goes on.
func BuildAll(replays []Replay, userAgent string, answers Answers) []Built {
	n := 0
	for _, r := range replays {
		n += len(r.Steps)
	}

	out := make([]Built, 0, n)

	for ri := range replays {
		out = append(out, buildReplay(ri, &replays[ri], userAgent, answers)...)
	}

	return out
}

func buildReplay(ri int, r *Replay, userAgent string, answers Answers) []Built {
	vars := map[string]any{}
	missing := map[string]string{}
	ended := ""
	out := make([]Built, 0, len(r.Steps))

	for si, st := range r.Steps {
		b := Built{Replay: ri, Step: si}
		if ended != "" {
			b.NotSent = ended
			out = append(out, b)

			continue
		}

		if reason := unresolved(st, vars, missing); reason != "" {
			b.NotSent = reason
			out = append(out, b)

			for name := range st.Save {
				missing[name] = fmt.Sprintf("needs {{%s}}, which step %d saves, and that step cannot be rebuilt offline", name, si+1)
			}

			continue
		}

		w, err := buildWire(st, vars, userAgent)
		if err != nil {
			ended = "ReplayHarness ends the replay at step " + strconv.Itoa(si+1) + ", which it cannot send"
			b.NotSent = err.Error()
			out = append(out, b)

			continue
		}

		b.Wire = w
		out = append(out, b)

		if len(st.Save) > 0 {
			ended = save(si, st, w, answers, vars, missing)
		}
	}

	return out
}

// unresolved returns why a step cannot be rebuilt offline, or "".
func unresolved(st Step, vars map[string]any, missing map[string]string) string {
	for _, name := range VarRefs(st) {
		if _, ok := vars[name]; ok {
			continue
		}

		switch name {
		case BuiltinUnique:
			return "uses {{unique}}, which ReplayHarness fills with a new random value on every run"
		case BuiltinTimestamp:
			return "uses {{timestamp}}, which ReplayHarness fills from the wall clock on every run"
		}

		if reason, ok := missing[name]; ok {
			return reason
		}
	}

	return ""
}

func buildWire(st Step, vars map[string]any, userAgent string) (Wire, error) {
	req, err := BuildRequest(st, vars)
	if err != nil {
		return Wire{}, err
	}

	return ToWire(req, userAgent)
}

// save stores the values step si saves from Go's recorded answer to w. It
// returns why the replay ends there, or "".
func save(si int, st Step, w Wire, answers Answers, vars map[string]any, missing map[string]string) string {
	names := make([]string, 0, len(st.Save))
	for name := range st.Save {
		names = append(names, name)
	}

	sort.Strings(names)

	resp, ok := answers(w)
	if !ok {
		for _, name := range names {
			missing[name] = fmt.Sprintf("needs {{%s}}, which step %d saves from Go's answer, "+
				"and the recording of saved values holds no answer to that step", name, si+1)
		}

		return ""
	}

	for _, name := range names {
		v, ok := Extract(resp, st.Save[name])
		if !ok {
			return fmt.Sprintf("ReplayHarness ends the replay after step %d: Go's recorded answer has no %s to save as {{%s}}",
				si+1, st.Save[name], name)
		}

		vars[name] = v
	}

	return ""
}

// AnswerKey identifies a request for matching a saving step with its
// recorded answer: the method, request target, body and headers, without
// the User-Agent (which names the ReplayHarness build) and the HopHeaders.
func AnswerKey(method, path, query string, header map[string]string, body []byte) string {
	skip := map[string]bool{"User-Agent": true}
	for _, h := range HopHeaders() {
		skip[h] = true
	}

	pairs := make([][2]string, 0, len(header))

	for k, v := range header {
		ck := http.CanonicalHeaderKey(k)
		if !skip[ck] {
			pairs = append(pairs, [2]string{ck, v})
		}
	}

	sort.Slice(pairs, func(i, j int) bool { return pairs[i][0] < pairs[j][0] })

	raw, err := json.Marshal([]any{strings.ToUpper(method), path, query, pairs, body})
	if err != nil {
		return ""
	}

	sum := sha256.Sum256(raw)

	return hex.EncodeToString(sum[:])
}

// Key is the AnswerKey of a wire request.
func (w Wire) Key() string {
	flat := make(map[string]string, len(w.Header))
	for k, v := range w.Header {
		flat[k] = strings.Join(v, ", ")
	}

	return AnswerKey(w.Method, w.Path, w.Query, flat, w.Body)
}

// Key is the AnswerKey of a recorded request.
func (r Request) Key() string {
	return AnswerKey(r.Method, r.Path, r.Query, r.Headers, r.BodyBytes())
}
