// Package trace records what the Go server observes while one parity case is
// in flight.
//
// Exactly one case runs per process at a time: Begin acquires a process-wide
// exec lock and publishes the Session returned by Current. Product code reaches
// the session through one-line hooks of the form
//
//	if s := trace.Current(); s != nil { s.Record(parity.StageX, key, value) }
//
// so a disabled harness costs one atomic load per hook. Every Session method is
// nil-safe and concurrency-safe (bidder goroutines record concurrently).
//
// Observer-effect rule: hooks must only snapshot values the code has already
// built. Never call RequestWrapper.RebuildRequest, never marshal a live wrapper
// to snapshot it, and never mutate state for tracing purposes.
package trace

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"sync"
	"sync/atomic"
	"unicode/utf8"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/fakes"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// Session collects everything recorded for one parity case.
type Session struct {
	mu          sync.Mutex
	env         *parity.Envelope
	filter      map[parity.StageID]bool
	seq         int
	stages      map[parity.StageID]map[string]*parity.StageEntry
	stageStatus map[parity.StageID]parity.StageStatus
	decisions   []parity.Decision
	httpOut     []parity.OutboundCall
	analytics   []parity.AnalyticsRecord
	notes       []string
	// pending counts the case goroutines registered with Track.
	pending sync.WaitGroup
}

var (
	current  atomic.Pointer[Session]
	execLock = make(chan struct{}, 1)
)

// Current returns the in-flight session, or nil when no parity case is running.
func Current() *Session {
	return current.Load()
}

// Begin starts a parity case, blocking until no other case is in flight. The
// returned end function unpublishes the session and releases the exec lock; it
// is idempotent and must be called exactly once per Begin.
func Begin(env *parity.Envelope) (*Session, func()) {
	s, end, _ := BeginContext(context.Background(), env)
	return s, end
}

// BeginContext is Begin that gives up when ctx is done before the exec lock is
// acquired. On error the returned end function is a no-op.
func BeginContext(ctx context.Context, env *parity.Envelope) (*Session, func(), error) {
	select {
	case execLock <- struct{}{}:
	case <-ctx.Done():
		return nil, func() {}, fmt.Errorf("parity: waiting for the exec lock: %w", ctx.Err())
	}
	s := newSession(env)
	if src := fakes.Active(); src != nil {
		src.Set(s.env.Fakes)
	}
	current.Store(s)
	var once sync.Once
	end := func() {
		once.Do(func() {
			current.Store(nil)
			if src := fakes.Active(); src != nil {
				src.Set(nil)
			}
			<-execLock
		})
	}
	return s, end, nil
}

func newSession(env *parity.Envelope) *Session {
	if env == nil {
		env = &parity.Envelope{}
	}
	s := &Session{
		env:         env,
		stages:      make(map[parity.StageID]map[string]*parity.StageEntry),
		stageStatus: make(map[parity.StageID]parity.StageStatus),
	}
	filter, err := env.Capture.StageFilter()
	if err != nil {
		s.notes = append(s.notes, fmt.Sprintf("capture.stages ignored: %v", err))
	}
	s.filter = filter
	return s
}

// Envelope returns the case envelope (never nil for a non-nil session).
func (s *Session) Envelope() *parity.Envelope {
	if s == nil {
		return nil
	}
	return s.env
}

// CaseID returns the case ID, or "" for a nil session.
func (s *Session) CaseID() string {
	if s == nil {
		return ""
	}
	return s.env.CaseID
}

// NextSeq returns the next value of the session-wide sequence counter shared by
// stage entries and outbound calls. It returns 0 for a nil session.
func (s *Session) NextSeq() int {
	if s == nil {
		return 0
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.nextSeqLocked()
}

func (s *Session) nextSeqLocked() int {
	s.seq++
	return s.seq
}

// Wants reports whether stage is captured under envelope.capture.stages.
func (s *Session) Wants(stage parity.StageID) bool {
	return s != nil && (s.filter == nil || s.filter[stage])
}

// Record marshals v with jsonutil.Marshal (so the raw bytes keep Go's lexical
// form) and stores it under (stage, key). An empty key means parity.UnkeyedStage.
// Recording the same (stage, key) again replaces the value and increments
// writes. Pass already-marshaled JSON to RecordRaw instead.
func (s *Session) Record(stage parity.StageID, key string, v any) {
	if !s.Wants(stage) {
		return
	}
	raw, err := jsonutil.Marshal(v)
	if err != nil {
		s.entryNote(stage, key, fmt.Sprintf("marshal error: %v", err))
		return
	}
	s.RecordRaw(stage, key, raw)
}

// RecordRaw stores exact bytes under (stage, key). Valid JSON also populates the
// semantic json member; anything else is kept only as raw / raw_b64.
func (s *Session) RecordRaw(stage parity.StageID, key string, raw []byte) {
	if !s.Wants(stage) {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	e := s.entryLocked(stage, key)
	e.Writes++
	e.Seq = s.nextSeqLocked()
	e.Raw, e.RawB64, e.JSON = nil, nil, nil
	if utf8.Valid(raw) {
		str := string(raw)
		e.Raw = &str
	} else {
		str := base64.StdEncoding.EncodeToString(raw)
		e.RawB64 = &str
	}
	if json.Valid(raw) {
		e.JSON = append(json.RawMessage(nil), raw...)
	} else {
		e.Notes = append(e.Notes, "raw value is not valid JSON")
	}
}

// Errors attaches errs to (stage, key): warnings (errortypes.IsWarning) go to
// warnings, everything else to errors. The lists become [] even when errs is
// empty, which records "no errors" explicitly.
func (s *Session) Errors(stage parity.StageID, key string, errs []error) {
	if !s.Wants(stage) {
		return
	}
	fatal, warnings := ErrorRecordsOf(errs)
	s.mu.Lock()
	defer s.mu.Unlock()
	e := s.entryLocked(stage, key)
	if e.Seq == 0 {
		e.Seq = s.nextSeqLocked()
	}
	e.Errors = append(nonNilRecords(e.Errors), fatal...)
	e.Warnings = append(nonNilRecords(e.Warnings), warnings...)
}

// Mark sets the status of stage explicitly, for example StageCaptured for
// stages the exec handler fills outside Record.
func (s *Session) Mark(stage parity.StageID, status parity.StageStatus) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.stageStatus[stage] = status
}

// Note appends a free-form note to the trace.
func (s *Session) Note(format string, args ...any) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.notes = append(s.notes, fmt.Sprintf(format, args...))
}

// AddOutbound appends an outbound call recorded by the transport.
func (s *Session) AddOutbound(call parity.OutboundCall) {
	if !s.Wants(parity.StageHTTPOut) {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.httpOut = append(s.httpOut, call)
	s.stageStatus[parity.StageHTTPOut] = parity.StageCaptured
}

// AddAnalytics marshals one analytics object and appends it.
func (s *Session) AddAnalytics(kind string, object any) {
	if !s.Wants(parity.StageAnalytics) || !s.env.Capture.WantsAnalytics() {
		return
	}
	raw, err := jsonutil.Marshal(object)
	if err != nil {
		s.Note("analytics %s: marshal error: %v", kind, err)
		return
	}
	if !json.Valid(raw) {
		// jsonutil.Marshal writes nothing for a json.RawMessage member that
		// does not compact, so an invalid regs.ext yields `"ext":}`.
		// Embedding it would make the whole trace unencodable.
		s.Note("analytics %s: marshaled object is not valid JSON, not recorded: %s", kind, strconv.Quote(string(raw)))
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.analytics = append(s.analytics, parity.AnalyticsRecord{Kind: kind, Object: raw})
	s.stageStatus[parity.StageAnalytics] = parity.StageCaptured
}

// Snapshot returns the trace document recorded so far. The exec handler fills
// Build, HTTPIn, HTTPResponse and MetricsDelta. Every registered stage gets a
// stage_status: captured, an explicit Mark, or not_reached.
func (s *Session) Snapshot() parity.Trace {
	t := parity.Trace{
		Schema:        parity.SchemaTrace,
		SchemaVersion: parity.SchemaVersion,
		Impl:          parity.ImplGo,
		Stages:        map[parity.StageID]map[string]parity.StageEntry{},
		StageStatus:   map[parity.StageID]parity.StageStatus{},
		HTTPOut:       []parity.OutboundCall{},
		Decisions:     []parity.Decision{},
		MetricsDelta:  []parity.MetricDelta{},
		Analytics:     []parity.AnalyticsRecord{},
		Notes:         []string{},
	}
	if s == nil {
		return t
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	t.CaseID = s.env.CaseID
	t.Endpoint = s.env.Request.Path
	for stage, keyed := range s.stages {
		copied := make(map[string]parity.StageEntry, len(keyed))
		for key, entry := range keyed {
			copied[key] = *entry
		}
		t.Stages[stage] = copied
	}
	t.StageStatus = s.statusLocked()
	t.HTTPOut = append(t.HTTPOut, s.httpOut...)
	t.Decisions = append(t.Decisions, s.decisions...)
	t.Analytics = append(t.Analytics, s.analytics...)
	t.Notes = append(t.Notes, s.notes...)
	return t
}

func (s *Session) statusLocked() map[parity.StageID]parity.StageStatus {
	registered := parity.AllStages()
	status := make(map[parity.StageID]parity.StageStatus, len(registered))
	for _, stage := range registered {
		status[stage] = parity.StageNotReached
	}
	unregistered := make([]string, 0)
	note := func(stage parity.StageID) {
		if _, known := status[stage]; !known {
			unregistered = append(unregistered, string(stage))
		}
	}
	for stage := range s.stageStatus {
		note(stage)
	}
	for stage := range s.stages {
		note(stage)
	}
	for stage, st := range s.stageStatus {
		status[stage] = st
	}
	for stage := range s.stages {
		status[stage] = parity.StageCaptured
	}
	sort.Strings(unregistered)
	for _, stage := range unregistered {
		s.notes = appendOnce(s.notes, "unregistered stage recorded: "+stage)
	}
	return status
}

func (s *Session) entryLocked(stage parity.StageID, key string) *parity.StageEntry {
	if key == "" {
		key = parity.UnkeyedStage
	}
	keyed, ok := s.stages[stage]
	if !ok {
		keyed = make(map[string]*parity.StageEntry)
		s.stages[stage] = keyed
	}
	e, ok := keyed[key]
	if !ok {
		e = &parity.StageEntry{}
		keyed[key] = e
	}
	return e
}

func (s *Session) entryNote(stage parity.StageID, key, note string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e := s.entryLocked(stage, key)
	e.Notes = append(e.Notes, note)
}

func nonNilRecords(in []parity.ErrorRecord) []parity.ErrorRecord {
	if in == nil {
		return []parity.ErrorRecord{}
	}
	return in
}

func appendOnce(list []string, item string) []string {
	for _, existing := range list {
		if existing == item {
			return list
		}
	}
	return append(list, item)
}
