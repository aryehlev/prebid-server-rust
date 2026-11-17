// Package minimize reduces a failing parity case to a small one that still
// reproduces the same primary mismatch signatures (design section 8.6).
//
// The reduction is hierarchical delta debugging: each round runs ddmin over
// the semantic edits (drop a bidder, a header, a query parameter, a mock, a
// decision), then over the deletions at every depth of the request body, top
// down, then over the value edits (replace a subtree with the minimal value of
// its type, shorten a string). An edit set is kept only while every target
// signature is still primary in the candidate's diff. Rounds repeat until one
// applies nothing, which makes the result 1-minimal with respect to the edit
// model: no single remaining edit can be applied without losing a target
// signature. Every distinct candidate is executed at most once (results are
// cached by content), the number of executions is capped by the budget, and
// the whole process is deterministic for a deterministic executor.
package minimize

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"

	"github.com/prebid/prebid-server/v4/parity"
)

// DefaultBudget is the default number of executions per minimization.
const DefaultBudget = 200

// SchemaReport names the min.report.json document.
const SchemaReport = "pbs-parity-min-report/1"

// ErrNotReproduced is returned when the original subject does not reproduce
// the target signatures.
var ErrNotReproduced = errors.New("minimize: the original case does not reproduce the target signatures")

// errBudget stops ddmin when the budget is spent.
var errBudget = errors.New("minimize: execution budget exhausted")

// Executor runs a candidate on both implementations and returns the primary
// signatures of its diff (diff.PrimarySignatures).
type Executor interface {
	Execute(ctx context.Context, s Subject) ([]string, error)
}

// ExecutorFunc adapts a function to Executor.
type ExecutorFunc func(ctx context.Context, s Subject) ([]string, error)

// Execute calls f.
func (f ExecutorFunc) Execute(ctx context.Context, s Subject) ([]string, error) { return f(ctx, s) }

// Options configure Minimize.
type Options struct {
	// Target are the signatures that must stay primary. Empty means the
	// primary signatures of the original subject.
	Target []string
	// Budget caps executions (cache hits are free); <= 0 means DefaultBudget.
	Budget int
	// Logf receives progress messages; nil discards them.
	Logf func(format string, args ...any)
}

// Report is min.report.json.
type Report struct {
	Schema        string   `json:"schema"`
	SchemaVersion string   `json:"schema_version"`
	CaseID        string   `json:"case_id"`
	Target        []string `json:"target"`
	Budget        int      `json:"budget"`
	Executions    int      `json:"executions"`
	CacheHits     int      `json:"cache_hits"`
	// ExecErrors counts candidates whose execution failed; they count as
	// not reproducing.
	ExecErrors      int    `json:"exec_errors"`
	BudgetExhausted bool   `json:"budget_exhausted"`
	OneMinimal      bool   `json:"one_minimal"`
	Rounds          int    `json:"rounds"`
	Applied         []Step `json:"applied"`
	Before          Size   `json:"before"`
	After           Size   `json:"after"`
}

// Step is one accepted ddmin pass: the edits it applied.
type Step struct {
	Round int      `json:"round"`
	Phase string   `json:"phase"`
	Edits []string `json:"edits"`
}

// Result is a minimized subject and its report.
type Result struct {
	Subject Subject
	Report  Report
}

// Minimize reduces s while the target signatures stay primary.
func Minimize(ctx context.Context, s Subject, exec Executor, opts Options) (*Result, error) {
	if exec == nil {
		return nil, errors.New("minimize: no executor")
	}
	m := &minimizer{exec: exec, budget: opts.Budget, cache: map[string]bool{}, logf: opts.Logf}
	if m.budget <= 0 {
		m.budget = DefaultBudget
	}
	if m.logf == nil {
		m.logf = func(string, ...any) {}
	}
	if err := m.init(ctx, s, opts.Target); err != nil {
		return nil, err
	}
	m.report.Before = SizeOf(s)
	current, err := m.rounds(ctx, s)
	if err != nil {
		return nil, err
	}
	m.report.After = SizeOf(current)
	return &Result{Subject: current, Report: m.report}, nil
}

// minimizer carries the state of one minimization.
type minimizer struct {
	exec   Executor
	budget int
	target []string
	cache  map[string]bool
	logf   func(format string, args ...any)
	report Report
}

// init executes the original subject and fixes the target.
func (m *minimizer) init(ctx context.Context, s Subject, target []string) error {
	m.report = Report{Schema: SchemaReport, SchemaVersion: parity.SchemaVersion, CaseID: s.Envelope.CaseID, Budget: m.budget, Applied: []Step{}}
	sigs, err := m.exec.Execute(ctx, s)
	m.report.Executions++
	if err != nil {
		return fmt.Errorf("minimize: executing the original case: %w", err)
	}
	m.target = append([]string(nil), target...)
	if len(m.target) == 0 {
		m.target = append([]string(nil), sigs...)
	}
	sort.Strings(m.target)
	m.report.Target = m.target
	if len(m.target) == 0 || !containsAll(sigs, m.target) {
		return ErrNotReproduced
	}
	key, err := subjectKey(s)
	if err != nil {
		return err
	}
	m.cache[key] = true
	return nil
}

// rounds runs reduction rounds until one applies nothing or the budget ends.
func (m *minimizer) rounds(ctx context.Context, s Subject) (Subject, error) {
	current := s
	for round := 1; ; round++ {
		m.report.Rounds = round
		next, progressed, err := m.round(ctx, current, round)
		if errors.Is(err, errBudget) {
			m.report.BudgetExhausted = true
			return next, nil
		}
		if err != nil {
			return Subject{}, err
		}
		current = next
		if !progressed {
			m.report.OneMinimal = true
			return current, nil
		}
	}
}

// round runs every phase once. It returns the subject reached so far even
// when the budget ends mid-round.
func (m *minimizer) round(ctx context.Context, s Subject, round int) (Subject, bool, error) {
	progressed := false
	current := s
	pass := func(phase, depth int, name string) error {
		b, err := newBase(current)
		if err != nil {
			return err
		}
		edits := b.edits(phase, depth)
		if len(edits) == 0 {
			return nil
		}
		next, applied, err := m.reduce(ctx, b, edits)
		if len(applied) > 0 {
			current, progressed = next, true
			m.report.Applied = append(m.report.Applied, Step{Round: round, Phase: name, Edits: applied})
			m.logf("[parity minimize] %s round %d %s: applied %d of %d edits", s.Envelope.CaseID, round, name, len(applied), len(edits))
		}
		return err
	}
	if err := pass(phaseGroups, 0, "groups"); err != nil {
		return current, progressed, err
	}
	for depth := 1; ; depth++ {
		b, err := newBase(current)
		if err != nil {
			return current, progressed, err
		}
		if depth > b.maxDepth() {
			break
		}
		if err := pass(phaseDelete, depth, fmt.Sprintf("delete depth %d", depth)); err != nil {
			return current, progressed, err
		}
	}
	err := pass(phaseValues, 0, "values")
	return current, progressed, err
}

// reduce runs ddmin over edits and returns the reduced subject and the
// descriptions of the applied edits. On errBudget it returns the best subject
// found before the budget ended.
func (m *minimizer) reduce(ctx context.Context, b *base, edits []Edit) (Subject, []string, error) {
	all := make([]int, len(edits))
	for i := range all {
		all[i] = i
	}
	test := func(kept []int) (bool, error) {
		cand, err := b.apply(pick(edits, complement(all, kept)))
		if err != nil {
			return false, err
		}
		return m.reproduces(ctx, cand)
	}
	kept, err := ddmin(all, test)
	applied := pick(edits, complement(all, kept))
	if len(applied) == 0 {
		return b.subj, nil, err
	}
	out, applyErr := b.apply(applied)
	if applyErr != nil {
		return b.subj, nil, applyErr
	}
	names := make([]string, len(applied))
	for i, e := range applied {
		names[i] = e.String()
	}
	return out, names, err
}

// reproduces executes a candidate (once per distinct content) and reports
// whether every target signature is primary in its diff.
func (m *minimizer) reproduces(ctx context.Context, s Subject) (bool, error) {
	key, err := subjectKey(s)
	if err != nil {
		return false, err
	}
	if ok, cached := m.cache[key]; cached {
		m.report.CacheHits++
		return ok, nil
	}
	if err := ctx.Err(); err != nil {
		return false, fmt.Errorf("minimize: %w", err)
	}
	if m.report.Executions >= m.budget {
		return false, errBudget
	}
	m.report.Executions++
	sigs, err := m.exec.Execute(ctx, s)
	if err != nil {
		if ctxErr := ctx.Err(); ctxErr != nil {
			return false, fmt.Errorf("minimize: %w", ctxErr)
		}
		m.report.ExecErrors++
		m.logf("[parity minimize] candidate failed: %v", err)
		m.cache[key] = false
		return false, nil
	}
	ok := containsAll(sigs, m.target)
	m.cache[key] = ok
	return ok, nil
}

// ddmin returns a 1-minimal subset of c (items kept) for which test holds,
// assuming test(c) holds (Zeller and Hildebrandt's ddmin). It returns the
// smallest subset found so far together with the error that stopped it.
func ddmin(c []int, test func([]int) (bool, error)) ([]int, error) {
	if ok, err := test(nil); err != nil || ok {
		if ok {
			return nil, nil
		}
		return c, err
	}
	n := 2
	for len(c) >= 2 {
		next, granularity, reduced, err := ddminStep(c, n, test)
		if err != nil {
			return c, err
		}
		if reduced {
			c, n = next, granularity
			continue
		}
		if n >= len(c) {
			break
		}
		n = minInt(2*n, len(c))
	}
	return c, nil
}

// ddminStep tries each of the n chunks of c, then each complement (when
// n > 2; with two chunks the complements are the chunks themselves).
func ddminStep(c []int, n int, test func([]int) (bool, error)) ([]int, int, bool, error) {
	chunks := split(c, n)
	for _, ch := range chunks {
		ok, err := test(ch)
		if err != nil {
			return nil, 0, false, err
		}
		if ok {
			return ch, 2, true, nil
		}
	}
	if n == 2 {
		return nil, 0, false, nil
	}
	for _, ch := range chunks {
		comp := complement(c, ch)
		ok, err := test(comp)
		if err != nil {
			return nil, 0, false, err
		}
		if ok {
			return comp, maxInt(n-1, 2), true, nil
		}
	}
	return nil, 0, false, nil
}

// split cuts c into n chunks of near-equal size, in order.
func split(c []int, n int) [][]int {
	out := make([][]int, 0, n)
	start := 0
	for i := 0; i < n; i++ {
		end := start + (len(c)-start)/(n-i)
		out = append(out, c[start:end])
		start = end
	}
	return out
}

// complement returns the members of all that are not in sub, in order.
func complement(all, sub []int) []int {
	drop := make(map[int]bool, len(sub))
	for _, i := range sub {
		drop[i] = true
	}
	out := make([]int, 0, len(all)-len(sub))
	for _, i := range all {
		if !drop[i] {
			out = append(out, i)
		}
	}
	return out
}

func pick(edits []Edit, idx []int) []Edit {
	out := make([]Edit, 0, len(idx))
	for _, i := range idx {
		out = append(out, edits[i])
	}
	return out
}

// subjectKey identifies a candidate by content.
func subjectKey(s Subject) (string, error) {
	raw, err := json.Marshal(s)
	if err != nil {
		return "", fmt.Errorf("minimize: encoding candidate: %w", err)
	}
	sum := sha256.Sum256(raw)
	return hex.EncodeToString(sum[:]), nil
}

func containsAll(have, want []string) bool {
	set := make(map[string]bool, len(have))
	for _, s := range have {
		set[s] = true
	}
	for _, s := range want {
		if !set[s] {
			return false
		}
	}
	return true
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func maxInt(a, b int) int {
	if a > b {
		return a
	}
	return b
}
