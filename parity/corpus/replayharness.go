package corpus

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"net/url"
	"path"
	"slices"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/replayharness/rhreplay"
	"gopkg.in/yaml.v3"
)

// CorpusReplayHarness is the corpus of the requests ReplayHarness sends to
// the Go server: every exchange of parity/replayharness/traffic.jsonl and
// every step of the replays in parity/replayharness/replays.
const CorpusReplayHarness = "replayharness"

// RootReplayHarness is the directory the replayharness corpus is read from.
// Every case of the corpus has it as its fixture path, because a case's ID
// is the hash of its request, which several files may hold.
const RootReplayHarness = "parity/replayharness"

// Files of RootReplayHarness.
const (
	rhTrafficFile = "traffic.jsonl"
	rhSavedFile   = "saved.jsonl"
	rhReplayDir   = "replays"
	rhStoredDir   = "stored"
	rhAPIFile     = "api.yaml"
	rhStoredExt   = ".json"
)

// Tags of the replayharness requests that are not compared
// (NotCompared.Tag).
const (
	// RHWallClock: Go's answer carries a cookie whose expiry comes from the
	// wall clock, which no fake or normalization rule covers.
	RHWallClock = "wall_clock"
	// RHMapOrder: Go's answer lists syncers in Go map iteration order.
	RHMapOrder = "map_order"
	// RHExecFraming: the request depends on how net/http frames the body,
	// which /__parity/exec does not reproduce.
	RHExecFraming = "exec_framing"
	// RHRequestLine: net/http rejects the request line before routing.
	RHRequestLine = "request_line"
	// RHTruncated: the recorder cut the body, so it is not what was sent.
	RHTruncated = "truncated_body"
	// RHNotRebuilt: a replay step whose request cannot be rebuilt offline.
	RHNotRebuilt = "not_rebuilt"
	// RHSidePort: an API map endpoint served on the admin or Prometheus
	// listener, which /__parity/exec cannot reach.
	RHSidePort = "side_port"
)

// rhMaxRequestSize is max_request_size in every parity profile: the Go
// default (config/config.go SetDefault "max_request_size").
const rhMaxRequestSize = 262144

// rhDefaultUserAgent is the User-Agent of a ReplayHarness built from a
// source archive, used when traffic.jsonl names no ReplayHarness build.
const rhDefaultUserAgent = "ReplayHarness/dev"

// rhLabelLen caps the path shown in a case ID; the hash after it keeps IDs
// unique.
const rhLabelLen = 40

// rhShownLen caps the request line shown in descriptions.
const rhShownLen = 160

var errReplayHarness = errors.New("corpus: replayharness")

// ReplayHarness is the replayharness corpus.
type ReplayHarness struct {
	// Cases are the requests both servers are compared on, sorted by ID.
	Cases []Case
	// NotCompared are the requests and API map endpoints left out of Cases,
	// each with its reason, sorted by ID. Nothing recorded or replayed is
	// dropped without an entry here.
	NotCompared []NotCompared
	// Errors are the files that could not be read.
	Errors []error
}

// NotCompared is a request of the replayharness corpus that is not a case.
type NotCompared struct {
	// ID is the case ID the request would have.
	ID string `json:"case_id"`
	// Request is the request line, or the API map endpoint, it stands for.
	Request string `json:"request"`
	// Sources are where it was recorded or replayed.
	Sources []string `json:"sources"`
	Tag     string   `json:"tag"`
	Reason  string   `json:"reason"`
}

// ReplayHarnessCases returns the cases of the replayharness corpus and the
// errors met loading it.
func ReplayHarnessCases(repo *Repo) ([]Case, []error) {
	rh := LoadReplayHarness(repo)

	return rh.Cases, rh.Errors
}

// LoadReplayHarness builds the replayharness corpus. A repository without
// parity/replayharness has an empty one.
//
// Each distinct request becomes one case, whatever recorded or replayed it:
// its method, path, query, headers and body bytes are kept exactly, except
// the hop-by-hop headers ReplayHarness's recorder also leaves out and the
// parity token header, and the Content-Length and Accept-Encoding that Go's
// HTTP client writes are put back (clientHeaders). Replay steps are rebuilt
// the way ReplayHarness sends them (package rhreplay), with the User-Agent of
// the build that recorded traffic.jsonl, so a step and its recording are one
// case. A step that sends
// a value an earlier step saved takes the value Go answered in saved.jsonl.
// The envelope leaves the host and remote address to the exec defaults
// (127.0.0.1 and 127.0.0.1:0), pins every decision point, and the case
// carries the stored data of parity/replayharness/stored.
func LoadReplayHarness(repo *Repo) *ReplayHarness {
	in, err := readReplayHarness(repo)
	if err != nil {
		return &ReplayHarness{Errors: []error{err}}
	}

	l := newRHLoader(in)
	l.addReplays()
	l.addTraffic()

	return l.result()
}

// rhInput is what the corpus reads from RootReplayHarness.
type rhInput struct {
	traffic []rhreplay.Exchange
	saved   []rhreplay.Exchange
	replays []rhreplay.Replay
	stored  Stored
	api     rhAPI
}

// rhAPI is the part of api.yaml the corpus uses.
type rhAPI struct {
	Endpoints []rhAPIEndpoint `yaml:"endpoints"`
}

type rhAPIEndpoint struct {
	Method string `yaml:"method"`
	Path   string `yaml:"path"`
	Rules  []any  `yaml:"rules"`
}

// readReplayHarness reads the corpus files through Repo.FS, so every read
// goes through Repo.Resolve, as every other loader's does: nothing outside the
// approved corpus roots is read, whatever symlinks the directory holds. A
// missing directory reads as empty.
func readReplayHarness(repo *Repo) (*rhInput, error) {
	_, err := repo.Resolve(RootReplayHarness)

	switch {
	case errors.Is(err, fs.ErrNotExist):
		return &rhInput{}, nil
	case err != nil:
		return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
	}

	return readRHFiles(repo.FS(RootReplayHarness))
}

func readRHFiles(fsys fs.FS) (*rhInput, error) {
	in := &rhInput{}

	var err error
	if in.traffic, err = readRecording(fsys, rhTrafficFile, true); err != nil {
		return nil, err
	}

	if in.saved, err = readRecording(fsys, rhSavedFile, false); err != nil {
		return nil, err
	}

	if in.replays, err = rhreplay.LoadDir(fsys, rhReplayDir); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return nil, fmt.Errorf("%w: %s: %w", errReplayHarness, rhReplayDir, err)
	}

	if in.stored, err = readRHStored(fsys); err != nil {
		return nil, err
	}

	raw, err := fs.ReadFile(fsys, rhAPIFile)
	switch {
	case errors.Is(err, fs.ErrNotExist):
	case err != nil:
		return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
	default:
		if err := yaml.Unmarshal(raw, &in.api); err != nil {
			return nil, fmt.Errorf("%w: %s: %w", errReplayHarness, rhAPIFile, err)
		}
	}

	return in, nil
}

func readRecording(fsys fs.FS, name string, required bool) ([]rhreplay.Exchange, error) {
	raw, err := fs.ReadFile(fsys, name)
	if errors.Is(err, fs.ErrNotExist) && !required {
		return nil, nil
	}

	if err != nil {
		return nil, fmt.Errorf("%w: %w", errReplayHarness, err)
	}

	exs, err := rhreplay.ReadRecording(bytes.NewReader(raw))
	if err != nil {
		return nil, fmt.Errorf("%w: %s: %w", errReplayHarness, name, err)
	}

	return exs, nil
}

// readRHStored reads stored/<kind>/<id>.json, the stored data the replays
// use besides parity/fixtures/stored (which the runner always serves).
func readRHStored(fsys fs.FS) (Stored, error) {
	var out Stored

	kinds := []struct {
		dir string
		dst *map[string]json.RawMessage
	}{
		{"stored_requests", &out.Requests},
		{"stored_imps", &out.Imps},
		{"stored_responses", &out.Responses},
		{"accounts", &out.Accounts},
	}

	for _, k := range kinds {
		dir := path.Join(rhStoredDir, k.dir)

		entries, err := fs.ReadDir(fsys, dir)
		if errors.Is(err, fs.ErrNotExist) {
			continue
		}

		if err != nil {
			return out, fmt.Errorf("%w: %w", errReplayHarness, err)
		}

		for _, e := range entries {
			if e.IsDir() || path.Ext(e.Name()) != rhStoredExt {
				continue
			}

			raw, err := fs.ReadFile(fsys, path.Join(dir, e.Name()))
			if err != nil {
				return out, fmt.Errorf("%w: %w", errReplayHarness, err)
			}

			if *k.dst == nil {
				*k.dst = map[string]json.RawMessage{}
			}

			(*k.dst)[strings.TrimSuffix(e.Name(), rhStoredExt)] = raw
		}
	}

	return out, nil
}

// rhRequest is a request as Go's router receives it.
type rhRequest struct {
	method  string
	path    string
	query   string
	headers parity.HeaderList
	body    []byte
	// dropped are the hop-by-hop and parity token headers left out.
	dropped []string
	// added are the headers Go's HTTP client writes (clientHeaders).
	added parity.HeaderList
}

// rhStatus is what one source says about Go's status for a request.
type rhStatus struct {
	// spec is a code ("200") or a class ("4xx").
	spec string
	// recorded marks a status traffic.jsonl recorded. The recorder drops
	// Cookie headers, so it may be Go's answer to the request with a cookie.
	recorded bool
}

// rhEntry is one distinct request with everything known about it.
type rhEntry struct {
	key       string
	req       rhRequest
	sources   []string
	statuses  []rhStatus
	truncated bool
}

type rhLoader struct {
	in        *rhInput
	userAgent string
	entries   map[string]*rhEntry
	skipped   map[string]*NotCompared
	sandbox   map[string]bool
	ampImps   map[string][]string
}

func newRHLoader(in *rhInput) *rhLoader {
	l := &rhLoader{
		in:        in,
		userAgent: dominantUserAgent(in.traffic),
		entries:   map[string]*rhEntry{},
		skipped:   map[string]*NotCompared{},
		sandbox:   map[string]bool{},
		ampImps:   map[string][]string{},
	}

	for id, raw := range in.stored.Accounts {
		if cookieDeprecationEnabled(raw) {
			l.sandbox[id] = true
		}
	}

	for id, raw := range in.stored.Requests {
		l.ampImps[id] = impIDs(raw)
	}

	return l
}

// dominantUserAgent is the User-Agent most ReplayHarness requests of the
// recording carry: the build that recorded it.
func dominantUserAgent(traffic []rhreplay.Exchange) string {
	counts := map[string]int{}

	for _, ex := range traffic {
		if ua := ex.Request.Headers["User-Agent"]; strings.HasPrefix(ua, "ReplayHarness/") {
			counts[ua]++
		}
	}

	best := rhDefaultUserAgent
	for _, ua := range sortedKeys(counts) {
		if counts[ua] > counts[best] {
			best = ua
		}
	}

	return best
}

// addReplays adds every replay step, saved values taken from saved.jsonl.
func (l *rhLoader) addReplays() {
	answers := map[string]rhreplay.Response{}
	for _, ex := range l.in.saved {
		answers[ex.Request.Key()] = ex.Response
	}

	lookup := func(w rhreplay.Wire) (rhreplay.Response, bool) {
		resp, ok := answers[w.Key()]

		return resp, ok
	}

	for _, b := range rhreplay.BuildAll(l.in.replays, l.userAgent, lookup) {
		r := &l.in.replays[b.Replay]
		st := r.Steps[b.Step]
		source := fmt.Sprintf("%s/%s: replay %q, step %d", rhReplayDir, r.File, r.Name, b.Step+1)

		if b.NotSent != "" {
			l.skipStep(st, source, b.NotSent)

			continue
		}

		req := requestFromWire(b.Wire)

		var statuses []rhStatus
		for _, spec := range st.Expect.StatusClasses() {
			statuses = append(statuses, rhStatus{spec: spec})
		}

		l.add(req, source, statuses, false)
	}
}

// addTraffic adds every recorded exchange.
func (l *rhLoader) addTraffic() {
	for _, ex := range l.in.traffic {
		req := requestFromRecording(ex.Request)
		source := fmt.Sprintf("%s:%d", rhTrafficFile, ex.Line)
		status := rhStatus{spec: strconv.Itoa(ex.Response.Status), recorded: true}
		l.add(req, source, []rhStatus{status}, len(req.body) >= rhreplay.RecorderBodyLimit)
	}
}

func (l *rhLoader) add(req rhRequest, source string, statuses []rhStatus, truncated bool) {
	key := requestKey(req)

	e, ok := l.entries[key]
	if !ok {
		e = &rhEntry{key: key, req: req}
		l.entries[key] = e
	}

	e.sources = append(e.sources, source)
	e.statuses = append(e.statuses, statuses...)
	e.truncated = e.truncated || truncated
}

// skipStep records a replay step that is not rebuilt. Its ID is the hash of
// the step, so the same step in two replays is one entry.
func (l *rhLoader) skipStep(st rhreplay.Step, source, reason string) {
	raw, err := json.Marshal(st)
	if err != nil {
		raw = []byte(st.Method + " " + st.Path)
	}

	id := RootReplayHarness + "#step:" + hashHex(raw)[:16]

	nc, ok := l.skipped[id]
	if !ok {
		nc = &NotCompared{ID: id, Request: shown(st.Method + " " + st.Path), Tag: RHNotRebuilt, Reason: reason}
		l.skipped[id] = nc
	}

	nc.Sources = append(nc.Sources, source)
}

func requestFromWire(w rhreplay.Wire) rhRequest {
	flat := make(map[string]string, len(w.Header))
	for k, v := range w.Header {
		flat[k] = strings.Join(v, ", ")
	}

	return newRHRequest(w.Method, w.Path, w.Query, flat, w.Body)
}

func requestFromRecording(r rhreplay.Request) rhRequest {
	return newRHRequest(r.Method, r.Path, r.Query, r.Headers, r.BodyBytes())
}

// newRHRequest canonicalizes header names, trims header values as net/http's
// server does, sorts headers and leaves out the hop-by-hop headers and the
// parity token. It then adds the headers Go's HTTP client writes on its own
// (clientHeaders), so the request is the one the Go server received.
func newRHRequest(method, reqPath, query string, header map[string]string, body []byte) rhRequest {
	drop := map[string]bool{http.CanonicalHeaderKey(parity.TokenHeader): true}
	for _, h := range rhreplay.HopHeaders() {
		drop[h] = true
	}

	r := rhRequest{method: method, path: reqPath, query: query, headers: parity.HeaderList{}, body: body}

	for _, k := range sortedKeys(header) {
		ck := http.CanonicalHeaderKey(k)
		if drop[ck] {
			r.dropped = append(r.dropped, ck)

			continue
		}

		r.headers = append(r.headers, [2]string{ck, strings.Trim(header[k], " \t")})
	}

	r.added = clientHeaders(method, r.headers, body)
	r.headers = append(r.headers, r.added...)

	sort.SliceStable(r.headers, func(i, j int) bool { return r.headers[i][0] < r.headers[j][0] })

	return r
}

// clientHeaders are the headers Go's HTTP client writes itself, which the
// recorder leaves out with the hop-by-hop headers (rhreplay.HopHeaders) but
// the Go server received from ReplayHarness, and which a handler or a
// compression layer may read:
//   - Content-Length, whenever net/http sends one: for a body, and for an
//     empty POST, PUT or PATCH (net/http/transfer.go,
//     shouldSendContentLength);
//   - Accept-Encoding: gzip, which the transport of ReplayHarness's client
//     (DisableCompression off) asks for on every request but a HEAD or a
//     Range request (net/http/transport.go). The recorder's own transport
//     does the same on the requests it forwards.
func clientHeaders(method string, headers parity.HeaderList, body []byte) parity.HeaderList {
	var out parity.HeaderList

	if len(body) > 0 || method == http.MethodPost || method == http.MethodPut || method == http.MethodPatch {
		out = append(out, [2]string{"Content-Length", strconv.Itoa(len(body))})
	}

	ranged := slices.ContainsFunc(headers, func(kv [2]string) bool { return kv[0] == "Range" })
	if method != http.MethodHead && !ranged {
		out = append(out, [2]string{"Accept-Encoding", "gzip"})
	}

	return out
}

// requestKey is the hash every source of one request shares.
func requestKey(r rhRequest) string {
	raw, err := json.Marshal([]any{r.method, r.path, r.query, r.headers, base64.StdEncoding.EncodeToString(r.body)})
	if err != nil {
		return ""
	}

	return hashHex(raw)
}

func hashHex(raw []byte) string {
	sum := sha256.Sum256(raw)

	return hex.EncodeToString(sum[:])
}

func (r rhRequest) line() string {
	target := r.path
	if r.query != "" {
		target += "?" + r.query
	}

	return r.method + " " + target
}

func shown(s string) string {
	if len(s) <= rhShownLen {
		return s
	}

	return truncateUTF8(s, rhShownLen) + "..."
}

func truncateUTF8(s string, n int) string {
	if len(s) <= n {
		return s
	}

	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}

	return s[:n]
}

// caseIDFor is RootReplayHarness#<method>:<path, cut>:<hash>.
func caseIDFor(e *rhEntry) string {
	return fmt.Sprintf("%s#%s:%s:%s", RootReplayHarness, strings.ToLower(e.req.method), truncateUTF8(e.req.path, rhLabelLen), e.key[:16])
}

func (l *rhLoader) result() *ReplayHarness {
	out := &ReplayHarness{}

	for _, key := range sortedKeys(l.entries) {
		e := l.entries[key]
		id := caseIDFor(e)

		if tag, reason := l.verdict(e); tag != "" {
			out.NotCompared = append(out.NotCompared, NotCompared{
				ID: id, Request: shown(e.req.line()), Sources: e.sources, Tag: tag, Reason: reason,
			})

			continue
		}

		out.Cases = append(out.Cases, l.newCase(id, e))
	}

	for _, nc := range l.skipped {
		out.NotCompared = append(out.NotCompared, *nc)
	}

	out.NotCompared = append(out.NotCompared, l.sidePorts()...)

	sort.Slice(out.Cases, func(i, j int) bool { return out.Cases[i].ID < out.Cases[j].ID })
	sort.Slice(out.NotCompared, func(i, j int) bool { return out.NotCompared[i].ID < out.NotCompared[j].ID })

	return out
}

func (l *rhLoader) newCase(id string, e *rhEntry) Case {
	req := parity.ExecRequest{Method: e.req.method, Path: e.req.path, Query: e.req.query, Headers: e.req.headers}
	if len(e.req.body) > 0 {
		req.Body = wireBody(e.req.body)
	}

	env := newEnvelope(id, RootReplayHarness, DefaultProfile, req)
	env.Decisions = pinnedDecisions()

	notes := make([]string, 0, len(e.sources)+2)
	for _, s := range e.sources {
		notes = append(notes, "from "+s)
	}

	if len(e.req.dropped) > 0 {
		notes = append(notes, "left out as the recorder does (hop-by-hop) or as secret (the parity token): "+
			strings.Join(e.req.dropped, ", "))
	}

	if len(e.req.added) > 0 {
		added := make([]string, 0, len(e.req.added))
		for _, kv := range e.req.added {
			added = append(added, kv[0]+": "+kv[1])
		}

		notes = append(notes, "added as Go's HTTP client writes them: "+strings.Join(added, ", "))
	}

	if ep, ok := l.route(e.req); ok && sidePortRule(ep) != "" {
		notes = append(notes, "the API map serves "+ep.Path+" on another listener ("+sidePortRule(ep)+"); this case checks the main port's answer")
	}

	return Case{
		ID:          id,
		Kind:        KindEndpoint,
		Corpus:      CorpusReplayHarness,
		FixturePath: RootReplayHarness,
		Description: shown(e.req.line()) + " (" + e.sources[0] + ")",
		Endpoint:    l.endpoint(e.req),
		Envelope:    env,
		Mocks:       MockPlan{ImpIDs: l.mockImpIDs(e.req)},
		Stored:      copyStored(l.in.stored),
		Notes:       notes,
	}
}

// wireBody is the exact body bytes: text when they are UTF-8, else base64.
func wireBody(b []byte) *parity.Body {
	if utf8.Valid(b) {
		return parity.TextBody(string(b))
	}

	s := base64.StdEncoding.EncodeToString(b)

	return &parity.Body{B64: &s}
}

// pinnedDecisions pins every decision point, so neither server draws at
// random: the binary ones and floors.modelgroup to 0, usersync.shuffle to
// identity.
func pinnedDecisions() map[string]json.RawMessage {
	out := map[string]json.RawMessage{}

	for _, point := range parity.AllDecisions() {
		value := json.RawMessage("0")
		if point == parity.DecisionUsersyncShuffle {
			value = json.RawMessage(strconv.Quote(parity.UsersyncShuffleIdentity))
		}

		out[string(point)] = value
	}

	return out
}

// mockImpIDs are the imp IDs of the inbound request, for the oracle's
// bidder responses: those of an auction body, or of the stored request an
// AMP tag_id names.
func (l *rhLoader) mockImpIDs(r rhRequest) []string {
	switch {
	case r.method == http.MethodPost && r.path == EndpointAuction:
		return impIDs(r.body)
	case r.method == http.MethodGet && r.path == EndpointAMP:
		values, err := url.ParseQuery(r.query)
		if err != nil {
			return nil
		}

		return l.ampImps[values.Get(ampTagParam)]
	}

	return nil
}

// endpoint is the API map route of the request followed by its method, such
// as "/info/bidders/{bidderName} GET": the route keeps product requirement
// attribution, and the method keeps these cases apart from the auction
// cases the generator derives from.
func (l *rhLoader) endpoint(r rhRequest) string {
	if ep, ok := l.route(r); ok {
		return ep.Path + " " + r.method
	}

	return truncateUTF8(r.path, rhLabelLen) + " " + r.method
}

// route finds the API map endpoint whose path matches the request path; a
// {param} segment matches one non-empty segment. The method is not matched,
// so a probe with another method still names its route.
func (l *rhLoader) route(r rhRequest) (rhAPIEndpoint, bool) {
	segs := strings.Split(r.path, "/")

	for _, ep := range l.in.api.Endpoints {
		want := strings.Split(ep.Path, "/")
		if len(want) != len(segs) {
			continue
		}

		match := true

		for i, w := range want {
			isParam := strings.HasPrefix(w, "{") && strings.HasSuffix(w, "}")
			if (isParam && segs[i] == "") || (!isParam && w != segs[i]) {
				match = false

				break
			}
		}

		if match {
			return ep, true
		}
	}

	return rhAPIEndpoint{}, false
}

// sidePortRule returns the endpoint's first rule when it names the admin or
// Prometheus listener.
func sidePortRule(ep rhAPIEndpoint) string {
	if len(ep.Rules) == 0 {
		return ""
	}

	first, ok := ep.Rules[0].(string)
	if !ok {
		return ""
	}

	lower := strings.ToLower(first)
	if strings.HasPrefix(lower, "admin port only") || strings.HasPrefix(lower, "prometheus port only") {
		rule, _, _ := strings.Cut(first, ";")

		return rule
	}

	return ""
}

// sidePorts lists the API map endpoints served on the admin or Prometheus
// listener: /__parity/exec dispatches to the main router only.
func (l *rhLoader) sidePorts() []NotCompared {
	var out []NotCompared

	for _, ep := range l.in.api.Endpoints {
		rule := sidePortRule(ep)
		if rule == "" {
			continue
		}

		listener := "admin"
		if strings.HasPrefix(strings.ToLower(rule), "prometheus") {
			listener = "prometheus"
		}

		out = append(out, NotCompared{
			ID:      fmt.Sprintf("%s#%s-port:%s:%s", RootReplayHarness, listener, strings.ToLower(ep.Method), ep.Path),
			Request: ep.Method + " " + ep.Path,
			Sources: []string{rhAPIFile},
			Tag:     RHSidePort,
			Reason: "the API map serves it on the " + listener + " listener (" + rule + "); /__parity/exec dispatches " +
				"to the main router only, so the cases of this path check the main port's answer",
		})
	}

	return out
}
