package runner

import (
	"context"
	"fmt"
	"sort"
	"sync"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/diff"
	"github.com/prebid/prebid-server/v4/parity/oracle"
	"github.com/prebid/prebid-server/v4/parity/report"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// Sides of a pair.
const (
	sideA = "a"
	sideB = "b"
)

// drainTimeout bounds the wait for a case's in-flight oracle calls.
const drainTimeout = 10 * time.Second

// pair is one (A, B) shard of a batch.
type pair struct {
	r     *runner
	b     *batch
	shard int
	sides [2]*side
}

// side is one server of a pair. srv is nil when the implementation could not
// be started; nyi then explains why every case is not-yet-instrumented.
type side struct {
	name string
	impl string
	srv  *Server
	nyi  string
}

// notReady returns the reason the side cannot run any case, or "".
func (s *side) notReady() string {
	switch {
	case s.nyi != "":
		return s.nyi
	case s.srv == nil:
		return s.impl + " server is not running"
	case s.srv.Exited():
		return s.impl + " server exited: " + s.srv.LogTail()
	}
	return ""
}

// caseResult is the outcome of executing one case on a pair.
type caseResult struct {
	status  report.CaseStatus
	reason  string
	doc     *parity.DiffDocument
	stagesA map[parity.StageID]parity.StageStatus
	stagesB map[parity.StageID]parity.StageStatus
	notes   []string
	// files are written into the case directory when the case did not pass.
	files   []caseFile
	fixture *report.FixtureResult
}

// caseFile is one case artifact; role names the Artifacts member it fills.
type caseFile struct {
	name string
	role string
	v    any
	raw  []byte
}

// Artifact roles.
const (
	roleEnvelope = "envelope"
	roleTraceA   = "trace_a"
	roleTraceB   = "trace_b"
	roleExtra    = "extra"
)

func notInstrumented(reason string) caseResult {
	return caseResult{status: report.CaseNotYetInstrumented, reason: reason}
}

func failed(format string, args ...any) caseResult {
	return caseResult{status: report.CaseError, reason: fmt.Sprintf(format, args...)}
}

// sideFailure turns a failed parity call into the case outcome: a route the
// server does not serve is not-yet-instrumented, anything else an error.
func sideFailure(s *side, what string, err error) caseResult {
	if IsNotInstrumented(err) {
		return notInstrumented(fmt.Sprintf("%s (%s) %s: %v", s.impl, s.name, what, err))
	}
	return failed("%s (%s) %s: %v", s.impl, s.name, what, err)
}

// execute runs one case on the pair and compares the two sides.
func (p *pair) execute(ctx context.Context, rc *runCase) caseResult {
	for _, s := range p.sides {
		if reason := s.notReady(); reason != "" {
			return notInstrumented(reason)
		}
	}
	ctx, cancel := context.WithTimeout(ctx, p.r.opts.CaseTimeout)
	defer cancel()
	switch {
	case rc.Intro:
		return p.execIntrospection(ctx, rc)
	case rc.Case.Kind == corpus.KindAdapter:
		return p.execAdapter(ctx, rc)
	case rc.Case.Envelope != nil:
		return p.execEnvelope(ctx, rc)
	}
	return failed("case kind %s has neither an envelope nor adapter inputs", rc.Case.Kind)
}

// envelopeSupport checks the health document against what an envelope needs.
func envelopeSupport(s *side, env *parity.Envelope) string {
	h := s.srv.Health
	switch {
	case len(h.Endpoints) > 0 && !containsString(h.Endpoints, env.Request.Path):
		return fmt.Sprintf("%s (%s) exec cannot dispatch to %s", s.impl, s.name, env.Request.Path)
	case len(h.MockModes) > 0 && !containsString(h.MockModes, env.Mock.EffectiveMode()):
		return fmt.Sprintf("%s (%s) does not support mock mode %s", s.impl, s.name, env.Mock.EffectiveMode())
	case len(h.MockLevels) > 0 && !containsString(h.MockLevels, env.Mock.EffectiveLevel()):
		return fmt.Sprintf("%s (%s) does not support mock level %s", s.impl, s.name, env.Mock.EffectiveLevel())
	}
	return ""
}

// sideTrace is what one side produced for an envelope.
type sideTrace struct {
	trace *parity.Trace
	raw   []byte
	caps  []oracle.Capture
	err   error
}

// execEnvelope posts the envelope to both servers (concurrently when they
// are different implementations, one after the other otherwise, because the
// oracle separates captures by implementation name), merges the oracle's wire
// captures into http_out and diffs the traces.
func (p *pair) execEnvelope(ctx context.Context, rc *runCase) caseResult {
	env := rc.Case.Envelope
	for _, s := range p.sides {
		if reason := envelopeSupport(s, env); reason != "" {
			return notInstrumented(reason)
		}
	}
	res := caseResult{}
	res.notes = append(res.notes, p.reset(ctx)...)
	oracleMode := env.Mock.EffectiveMode() == parity.MockModeOracle
	if oracleMode {
		if err := p.r.oracle.Register(oracle.PlanFromCase(rc.Case)); err != nil {
			return failed("registering the oracle plan: %v", err)
		}
		defer p.r.oracle.Unregister(rc.ID)
	}
	traces := p.runBoth(ctx, func(ctx context.Context, s *side) sideTrace {
		t, raw, err := s.srv.Client.Exec(ctx, env)
		out := sideTrace{trace: t, raw: raw, err: err}
		if oracleMode {
			out.caps = p.drain(ctx, rc.ID, s.impl)
		}
		return out
	})
	res.files = []caseFile{{name: report.FileEnvelope, role: roleEnvelope, v: env}}
	for i, t := range traces {
		if t.err == nil {
			continue
		}
		out := sideFailure(p.sides[i], "exec", t.err)
		out.files, out.notes = res.files, res.notes
		for j, u := range traces {
			if u.raw != nil {
				out.files = append(out.files, caseFile{name: traceFileName(p.sides[j]), role: traceRole(j), raw: u.raw})
			}
		}
		return out
	}
	return p.compareTraces(rc, traces, res)
}

// compareTraces merges captures, validates the traces and diffs them.
func (p *pair) compareTraces(rc *runCase, traces [2]sideTrace, res caseResult) caseResult {
	env := rc.Case.Envelope
	for i, t := range traces {
		res.notes = append(res.notes, validationNotes(p.sides[i], t.raw)...)
		mergeCaptures(t.trace, env, t.caps)
	}
	for i, t := range traces {
		res.files = append(res.files, caseFile{name: traceFileName(p.sides[i]), role: traceRole(i), v: t.trace})
	}
	dc := diff.Case{CaseID: rc.ID, FixturePath: rc.Case.FixturePath, Endpoint: rc.Case.Endpoint, Profile: p.b.Label(), Envelope: env, KnownInputs: knownInputs(rc.Case)}
	result, err := p.r.engine.CompareTraces(dc, traces[0].trace, traces[1].trace)
	if err != nil {
		out := failed("diff: %v", err)
		out.files, out.notes = res.files, res.notes
		return out
	}
	res.notes = append(res.notes, result.Notes...)
	res.stagesA, res.stagesB = traces[0].trace.StageStatus, traces[1].trace.StageStatus
	return withDiff(res, result.Doc)
}

// withDiff sets the compared status from a diff document.
func withDiff(res caseResult, doc parity.DiffDocument) caseResult {
	res.doc = &doc
	res.status = report.CasePassing
	if len(doc.Mismatches) > 0 {
		res.status = report.CaseMismatch
	}
	return res
}

// runBoth runs fn on both sides: concurrently when the implementations
// differ, sequentially (A then B) when they share an implementation name.
func (p *pair) runBoth(ctx context.Context, fn func(context.Context, *side) sideTrace) [2]sideTrace {
	var out [2]sideTrace
	if p.sides[0].impl == p.sides[1].impl {
		out[0] = fn(ctx, p.sides[0])
		out[1] = fn(ctx, p.sides[1])
		return out
	}
	var wg sync.WaitGroup
	for i := range p.sides {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			out[i] = fn(ctx, p.sides[i])
		}(i)
	}
	wg.Wait()
	return out
}

// reset calls /__parity/reset on both servers; failures become notes.
func (p *pair) reset(ctx context.Context) []string {
	var notes []string
	for _, s := range p.sides {
		if _, err := s.srv.Client.Reset(ctx); err != nil {
			notes = append(notes, fmt.Sprintf("%s (%s) reset: %v", s.impl, s.name, err))
		}
	}
	return notes
}

// drain collects the oracle captures of one side's calls for a case.
func (p *pair) drain(ctx context.Context, caseID, impl string) []oracle.Capture {
	ctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), drainTimeout)
	defer cancel()
	caps, err := p.r.oracle.Drain(ctx, caseID, impl)
	if err != nil {
		p.r.logf("[parity runner] %s: draining %s captures: %v", caseID, impl, err)
	}
	return caps
}

// mergeCaptures appends the oracle's wire captures to http_out (the trace's
// own records are the inline-mode ones) and marks the stage captured.
func mergeCaptures(t *parity.Trace, env *parity.Envelope, caps []oracle.Capture) {
	if len(caps) == 0 {
		return
	}
	if filter, err := env.Capture.StageFilter(); err == nil && filter != nil && !filter[parity.StageHTTPOut] {
		return
	}
	t.HTTPOut = append(t.HTTPOut, oracle.OutboundCalls(caps)...)
	if t.StageStatus == nil {
		t.StageStatus = map[parity.StageID]parity.StageStatus{}
	}
	t.StageStatus[parity.StageHTTPOut] = parity.StageCaptured
}

// validationNotes validates a raw server document against the schemas.
func validationNotes(s *side, raw []byte) []string {
	if len(raw) == 0 {
		return nil
	}
	if err := schema.Validate(raw); err != nil {
		return []string{fmt.Sprintf("%s (%s) document does not validate: %v", s.impl, s.name, err)}
	}
	return nil
}

func traceFileName(s *side) string { return s.name + "." + s.impl + ".trace.json" }

func traceRole(i int) string {
	if i == 0 {
		return roleTraceA
	}
	return roleTraceB
}

// mergeDocs concatenates the diff documents of one case's comparisons (in
// stage order). The first primary record stays the case's first divergence:
// primary records of later documents become consequential of it, except wire
// records, which are always primary.
func mergeDocs(head parity.DiffDocument, docs []parity.DiffDocument) parity.DiffDocument {
	out := head
	out.Mismatches = []parity.MismatchRecord{}
	var root *string
	for _, d := range docs {
		first := root
		for _, m := range d.Mismatches {
			if first != nil && m.Primary && m.Kind != parity.KindWire {
				m.Primary, m.ConsequentialOf = false, first
			}
			if root == nil && m.Primary {
				sig := m.Signature
				root = &sig
			}
			out.Mismatches = append(out.Mismatches, m)
		}
	}
	return out
}

// docHead is a diff document with the case identity and no mismatches.
func docHead(rc *runCase, profile string) parity.DiffDocument {
	return parity.DiffDocument{
		Schema: parity.SchemaDiff, SchemaVersion: parity.SchemaVersion, CaseID: rc.ID,
		FixturePath: rc.Case.FixturePath, Endpoint: rc.Case.Endpoint, Profile: profile,
		Mismatches: []parity.MismatchRecord{},
	}
}

// exercised returns the domains of the stages a side captured.
func (r *runner) exercised(stages map[parity.StageID]parity.StageStatus) []parity.Domain {
	set := map[parity.Domain]bool{}
	for stage, st := range stages {
		if d, ok := r.stageDomain[stage]; ok && st == parity.StageCaptured {
			set[d] = true
		}
	}
	return report.OrderedDomains(set)
}

// record writes a case's artifacts and summary.
func (p *pair) record(rc *runCase, res caseResult, started time.Time) error {
	r := p.r
	s := r.newSummary(rc, p.b.Label())
	s.Status, s.Reason, s.StagesA, s.StagesB = res.status, res.reason, res.stagesA, res.stagesB
	s.ExercisedDomains = r.exercised(res.stagesA)
	s.Notes = append(s.Notes, res.notes...)
	s.DurationMS = time.Since(started).Milliseconds()
	if res.status == report.CaseMismatch || res.status == report.CaseError {
		if err := r.writeCaseFiles(rc, p.b, &s, res.files); err != nil {
			return err
		}
	}
	if res.fixture != nil {
		r.writer.AddFixtures([]report.FixtureResult{*res.fixture})
	}
	return r.writer.AddCase(s, res.doc)
}

// writeCaseFiles writes the saved case and the per-side documents.
func (r *runner) writeCaseFiles(rc *runCase, b *batch, s *report.CaseSummary, files []caseFile) error {
	ref, err := r.writer.WriteCaseJSON(rc.ID, report.FileCase, newSavedCase(rc, b.Name))
	if err != nil {
		return err
	}
	s.Artifacts.Case = ref
	for _, f := range files {
		var ref string
		var err error
		if f.raw != nil {
			ref, err = r.writer.WriteCaseFile(rc.ID, f.name, f.raw)
		} else {
			ref, err = r.writer.WriteCaseJSON(rc.ID, f.name, f.v)
		}
		if err != nil {
			return err
		}
		switch f.role {
		case roleEnvelope:
			s.Artifacts.Envelope = ref
		case roleTraceA:
			s.Artifacts.TraceA = ref
		case roleTraceB:
			s.Artifacts.TraceB = ref
		default:
			s.Artifacts.Extra = append(s.Artifacts.Extra, ref)
		}
	}
	sort.Strings(s.Artifacts.Extra)
	return nil
}
