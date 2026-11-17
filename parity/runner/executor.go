package runner

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"sort"
	"time"

	"github.com/prebid/prebid-server/v4/parity/diff"
	"github.com/prebid/prebid-server/v4/parity/matrix"
	"github.com/prebid/prebid-server/v4/parity/minimize"
	"github.com/prebid/prebid-server/v4/parity/report"
)

// MinimizeOptions configure minimization of a finished run's clusters.
type MinimizeOptions struct {
	// RunDir is the finished run directory.
	RunDir   string
	RepoRoot string
	// A and B default to the run's implementations; a missing binary must
	// be given.
	A, B Impl
	// ProfilesDir and StoredDir default as in Options.
	ProfilesDir string
	StoredDir   string
	// Clusters are signatures to minimize; empty means every primary
	// cluster. Each is minimized from its representative case.
	Clusters []string
	// Cases are case IDs to minimize against all their primary signatures.
	Cases []string
	// Budget is the executions per case (minimize.DefaultBudget when <= 0).
	Budget       int
	CaseTimeout  time.Duration
	StartTimeout time.Duration
	Logf         func(format string, args ...any)
}

// MinimizeOutcome is the result for one case.
type MinimizeOutcome struct {
	CaseID     string           `json:"case_id"`
	Signatures []string         `json:"signatures"`
	Report     *minimize.Report `json:"report,omitempty"`
	Error      string           `json:"error,omitempty"`
}

// MinimizeRun minimizes clusters of a finished run on live servers, writes
// min.envelope.json, min.case.json and min.report.json into each case
// directory and rebuilds the run documents so they link the repros.
func MinimizeRun(ctx context.Context, opts MinimizeOptions) ([]MinimizeOutcome, *report.Run, error) {
	run, err := report.LoadRun(opts.RunDir)
	if err != nil {
		return nil, nil, err
	}
	targets := minimizeTargets(run, opts)
	if len(targets) == 0 {
		return nil, run, nil
	}
	r, err := newRunner(minimizeRunnerOptions(run, opts))
	if err != nil {
		return nil, nil, err
	}
	defer r.close()
	r.minBudget = opts.Budget
	outcomes := r.minimizeAll(ctx, targets)
	rebuilt, err := report.Rebuild(opts.RunDir)
	if err != nil {
		return outcomes, nil, err
	}
	m, err := matrix.BuildMatrix(rebuilt)
	if err != nil {
		return outcomes, rebuilt, err
	}
	return outcomes, rebuilt, matrix.WriteRunFiles(m, rebuilt.Layout.Dir)
}

// minimizeRunnerOptions builds runner options for re-running a run's cases.
func minimizeRunnerOptions(run *report.Run, opts MinimizeOptions) Options {
	pick := func(given Impl, name, binary string) Impl {
		if given.Name == "" {
			given.Name = name
		}
		if given.Binary == "" && binary != "" {
			given.Binary = binary
			if !filepath.IsAbs(binary) && opts.RepoRoot != "" {
				given.Binary = filepath.Join(opts.RepoRoot, filepath.FromSlash(binary))
			}
		}
		return given
	}
	return Options{
		RepoRoot: opts.RepoRoot, OutDir: run.Layout.Dir, RunID: run.Summary.RunID, Mode: run.Summary.Mode,
		A: pick(opts.A, run.Summary.ImplA, run.Summary.BinaryA), B: pick(opts.B, run.Summary.ImplB, run.Summary.BinaryB),
		ProfilesDir: opts.ProfilesDir, StoredDir: opts.StoredDir,
		CaseTimeout: opts.CaseTimeout, StartTimeout: opts.StartTimeout, Logf: opts.Logf,
	}
}

// minimizeTarget is one case and the signatures it must keep reproducing.
type minimizeTarget struct {
	caseID     string
	signatures []string
}

// minimizeTargets picks representative cases: of the named clusters (else
// every primary cluster) and the named cases.
func minimizeTargets(run *report.Run, opts MinimizeOptions) []minimizeTarget {
	bySig := map[string][]string{}
	for _, c := range run.Clusters {
		if (len(opts.Clusters) == 0 && len(opts.Cases) == 0 && c.Primary()) || containsString(opts.Clusters, c.Signature) {
			bySig[c.Representative] = append(bySig[c.Representative], c.Signature)
		}
	}
	for _, id := range opts.Cases {
		if cs, ok := run.Case(id); ok {
			bySig[id] = append(bySig[id], cs.PrimarySignatures...)
		}
	}
	out := make([]minimizeTarget, 0, len(bySig))
	for _, id := range sortedKeys(bySig) {
		sigs := uniqueSorted(bySig[id])
		out = append(out, minimizeTarget{caseID: id, signatures: sigs})
	}
	return out
}

func uniqueSorted(in []string) []string {
	set := map[string]bool{}
	for _, s := range in {
		set[s] = true
	}
	return sortedKeys(set)
}

// minimizeAll groups the targets by batch and minimizes each group on one
// pair.
func (r *runner) minimizeAll(ctx context.Context, targets []minimizeTarget) []MinimizeOutcome {
	var outcomes []MinimizeOutcome
	groups := map[string][]*runCase{}
	sigs := map[string][]string{}
	for _, t := range targets {
		rc, batchName, err := r.loadSavedCase(t.caseID)
		if err != nil {
			outcomes = append(outcomes, MinimizeOutcome{CaseID: t.caseID, Signatures: t.signatures, Error: err.Error()})
			continue
		}
		groups[batchName] = append(groups[batchName], rc)
		sigs[rc.ID] = t.signatures
	}
	for _, name := range sortedKeys(groups) {
		outcomes = append(outcomes, r.minimizeGroup(ctx, name, groups[name], sigs)...)
	}
	sort.Slice(outcomes, func(i, j int) bool { return outcomes[i].CaseID < outcomes[j].CaseID })
	return outcomes
}

// loadSavedCase reads case.json of a run case.
func (r *runner) loadSavedCase(caseID string) (*runCase, string, error) {
	saved, err := readSavedCase(r.writer.CasePath(caseID, report.FileCase))
	if err != nil {
		return nil, "", err
	}
	if saved.Intro || saved.Case.Envelope == nil {
		return nil, "", fmt.Errorf("runner: %s: only envelope cases can be minimized", caseID)
	}
	return &runCase{ID: saved.RunCaseID, Base: saved.Base, Case: saved.Case, Bidder: caseBidder(saved.Case)}, saved.Batch, nil
}

// minimizeGroup starts one pair for a batch's cases and minimizes each.
func (r *runner) minimizeGroup(ctx context.Context, name string, cases []*runCase, sigs map[string][]string) []MinimizeOutcome {
	b := &batch{Name: "min-" + name, Base: cases[0].Base, Profile: cases[0].Case.Profile}
	for _, rc := range cases {
		addStored(&b.Stored, rc.Case.Stored)
	}
	fail := func(err error) []MinimizeOutcome {
		out := make([]MinimizeOutcome, 0, len(cases))
		for _, rc := range cases {
			out = append(out, MinimizeOutcome{CaseID: rc.ID, Signatures: sigs[rc.ID], Error: err.Error()})
		}
		return out
	}
	workdir, err := r.prepareWorkdir(b)
	if err != nil {
		return fail(err)
	}
	pairs := r.startPairs(ctx, b, workdir, 1)
	defer r.stopPairs(pairs)
	p := pairs[0]
	for _, s := range p.sides {
		if reason := s.notReady(); reason != "" {
			return fail(errors.New(reason))
		}
	}
	out := make([]MinimizeOutcome, 0, len(cases))
	for _, rc := range cases {
		out = append(out, r.minimizeCase(ctx, p, rc, sigs[rc.ID]))
	}
	return out
}

// minimizeCase minimizes one case and writes its repro files.
func (r *runner) minimizeCase(ctx context.Context, p *pair, rc *runCase, sigs []string) MinimizeOutcome {
	o := MinimizeOutcome{CaseID: rc.ID, Signatures: sigs}
	subject := minimize.Subject{Envelope: *rc.Case.Envelope, Mocks: rc.Case.Mocks}
	exec := minimize.ExecutorFunc(func(ctx context.Context, s minimize.Subject) ([]string, error) {
		return p.primarySignatures(ctx, rc, s)
	})
	res, err := minimize.Minimize(ctx, subject, exec, minimize.Options{Target: sigs, Budget: r.minBudget, Logf: r.opts.Logf})
	if err != nil {
		o.Error = err.Error()
		return o
	}
	o.Report = &res.Report
	if err := r.writeMinimized(rc, res); err != nil {
		o.Error = err.Error()
	}
	return o
}

// primarySignatures runs a candidate on the pair and returns the primary
// signatures of its diff. A candidate that cannot be compared is an error.
func (p *pair) primarySignatures(ctx context.Context, rc *runCase, s minimize.Subject) ([]string, error) {
	cp := *rc.Case
	env := s.Envelope
	cp.Envelope, cp.Mocks = &env, s.Mocks
	res := p.execute(ctx, &runCase{ID: rc.ID, Base: rc.Base, Case: &cp, Bidder: rc.Bidder})
	if !res.status.Compared() || res.doc == nil {
		return nil, fmt.Errorf("candidate was not compared (%s): %s", res.status, res.reason)
	}
	return diff.PrimarySignatures(*res.doc), nil
}

// writeMinimized writes min.envelope.json, min.case.json and min.report.json.
func (r *runner) writeMinimized(rc *runCase, res *minimize.Result) error {
	cp := *rc.Case
	env := res.Subject.Envelope
	cp.Envelope, cp.Mocks = &env, res.Subject.Mocks
	docs := []struct {
		name string
		v    any
	}{
		{report.FileMinEnvelope, env},
		{report.FileMinCase, newSavedCase(&runCase{ID: rc.ID, Base: rc.Base, Case: &cp, Bidder: rc.Bidder}, "minimized")},
		{report.FileMinReport, res.Report},
	}
	for _, d := range docs {
		if _, err := r.writer.WriteCaseJSON(rc.ID, d.name, d.v); err != nil {
			return err
		}
	}
	return nil
}
