// Package runner is the parity harness's differential runner (design
// section 8): it starts pairs of servers in parity mode with equivalent
// configuration, sends every selected corpus case to both sides, merges the
// mock oracle's wire captures, diffs what the two sides observed and writes
// the run reports and coverage matrix.
//
// A run groups cases into batches that share one host configuration (base
// profile fixture, case profile and non-conflicting stored data). Every
// batch gets a working directory with pbs.yaml, a static/ symlink and the
// stored-data tree, and N shard pairs of server processes on free loopback
// ports; each shard runs its cases one at a time, the same order on both
// sides, calling /__parity/reset before every case. Servers are restarted
// per batch.
//
// Implementation B (and A) may not be parity-capable yet: a server whose
// /__parity/health answers 404 or 501, or that does not start, turns every
// case of its batch into not-yet-instrumented with the reason, and the run
// still completes with its reports.
//
// The package is harness-only.
package runner

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/diff"
	"github.com/prebid/prebid-server/v4/parity/matrix"
	"github.com/prebid/prebid-server/v4/parity/oracle"
	"github.com/prebid/prebid-server/v4/parity/report"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// Defaults of Options.
const (
	defaultCaseTimeout = 30 * time.Second
	defaultOutDir      = "parity/out"
	defaultProfilesDir = "parity/fixtures/profiles"
	defaultStoredDir   = "parity/fixtures/stored"
	tokenBytes         = 24
)

// Impl is one side of the comparison.
type Impl struct {
	// Name is the implementation name the server reports in traces and
	// X-Parity-Meta (parity.ImplGo or parity.ImplRust).
	Name string
	// Binary is the server executable; empty means the side is not run and
	// every case is not-yet-instrumented.
	Binary string
	// Env are extra KEY=VALUE entries appended to the server's environment
	// (for example RUST_LOG). Being last, they take precedence over the
	// runner's own entries.
	Env []string
}

// Options configure a run.
type Options struct {
	// RepoRoot is the checkout; default: found from the working directory.
	RepoRoot string
	// OutDir is the run directory; default <repo>/parity/out/<RunID>.
	OutDir string
	// RunID names the run; default a UTC timestamp.
	RunID string
	// Mode is report.ModeRun or report.ModeSelfTest.
	Mode string
	// A is the reference implementation, B the one compared with it.
	A, B Impl
	// ProfilesDir holds the profile fixtures; default parity/fixtures/profiles.
	ProfilesDir string
	// Profiles are the base profiles to run every case under; default
	// ["default"].
	Profiles []string
	// StoredDir is copied into every working directory; default
	// parity/fixtures/stored.
	StoredDir string
	// Selection filters the corpus.
	Selection Selection
	// Shards is the number of server pairs per batch; default 1.
	Shards int
	// Shuffle, when not 0, runs each batch's cases in an order drawn from
	// this seed instead of case ID order (shuffleBatches). A case whose
	// result depends on the cases run before it on the same servers then
	// shows up as a difference between runs.
	Shuffle int64
	// Budget stops starting new cases after this long; 0 means no limit.
	Budget time.Duration
	// CaseTimeout bounds one case on one pair; default 30s.
	CaseTimeout time.Duration
	// StartTimeout bounds a server's health wait; default 90s.
	StartTimeout time.Duration
	// FixtureSelfTest compares implementation A's adapter endpoint outputs
	// with the adapter fixtures' expectations.
	FixtureSelfTest bool
	// Corpus is a preloaded corpus; nil loads every corpus of the repo.
	Corpus *corpus.Corpus
	// Logf receives progress messages; nil discards them.
	Logf func(format string, args ...any)
}

// Result is a finished run.
type Result struct {
	Run    *report.Run
	Matrix *matrix.Matrix
}

// Run executes a parity run and writes its reports.
func Run(ctx context.Context, opts Options) (*Result, error) {
	r, err := newRunner(opts)
	if err != nil {
		return nil, err
	}
	defer r.close()
	return r.run(ctx)
}

// runner is the state of one run.
type runner struct {
	opts        Options
	repo        *corpus.Repo
	corpus      *corpus.Corpus
	profiles    Profiles
	layout      report.Layout
	writer      *report.Writer
	engine      *diff.Engine
	oracle      *oracle.Oracle
	stageDomain map[parity.StageID]parity.Domain
	token       string
	started     time.Time
	deadline    time.Time
	// minBudget is the per-case execution budget of minimization.
	minBudget int

	mu      sync.Mutex
	servers []report.ServerRecord
	notes   []string
}

func newRunner(opts Options) (*runner, error) {
	opts, err := withDefaults(opts)
	if err != nil {
		return nil, err
	}
	r := &runner{opts: opts, started: time.Now().UTC()}
	if err := r.load(); err != nil {
		return nil, err
	}
	if err := r.startOracle(); err != nil {
		return nil, err
	}
	return r, nil
}

func withDefaults(opts Options) (Options, error) {
	if opts.RepoRoot == "" {
		repo, err := corpus.FindRepo(".")
		if err != nil {
			return opts, err
		}
		opts.RepoRoot = repo.Root()
	}
	if opts.A.Name == "" || opts.B.Name == "" {
		return opts, errors.New("runner: both implementation names are required")
	}
	if opts.Mode == "" {
		opts.Mode = report.ModeRun
	}
	if opts.RunID == "" {
		opts.RunID = time.Now().UTC().Format("20060102T150405Z")
	}
	if opts.OutDir == "" {
		opts.OutDir = filepath.Join(opts.RepoRoot, filepath.FromSlash(defaultOutDir), opts.RunID)
	}
	if opts.ProfilesDir == "" {
		opts.ProfilesDir = filepath.Join(opts.RepoRoot, filepath.FromSlash(defaultProfilesDir))
	}
	if opts.StoredDir == "" {
		opts.StoredDir = filepath.Join(opts.RepoRoot, filepath.FromSlash(defaultStoredDir))
	}
	if len(opts.Profiles) == 0 {
		opts.Profiles = []string{DefaultProfile}
	}
	if opts.Shards <= 0 {
		opts.Shards = 1
	}
	if opts.CaseTimeout <= 0 {
		opts.CaseTimeout = defaultCaseTimeout
	}
	if opts.Logf == nil {
		opts.Logf = func(string, ...any) {}
	}
	return opts, nil
}

// load opens the repository, corpus, profiles, run directory and differ.
func (r *runner) load() error {
	repo, err := corpus.OpenRepo(r.opts.RepoRoot)
	if err != nil {
		return err
	}
	r.repo = repo
	if r.corpus = r.opts.Corpus; r.corpus == nil {
		if r.corpus, err = corpus.LoadAll(repo); err != nil {
			return err
		}
	}
	if err := corpusErrors(r.corpus.Errors); err != nil {
		return err
	}
	if r.profiles, err = LoadProfiles(r.opts.ProfilesDir, r.repoRel); err != nil {
		return err
	}
	for _, p := range r.opts.Profiles {
		if _, err := r.profiles.Resolve(p); err != nil {
			return err
		}
	}
	if r.layout, err = report.NewLayout(r.opts.OutDir, r.opts.RepoRoot); err != nil {
		return err
	}
	ignoreOutputRoot(r.opts.RepoRoot, r.layout.Dir)
	if r.writer, err = report.NewWriter(r.layout); err != nil {
		return err
	}
	if r.engine, err = diff.New(); err != nil {
		return err
	}
	reg, err := schema.Stages()
	if err != nil {
		return fmt.Errorf("runner: loading stages.json: %w", err)
	}
	r.stageDomain = make(map[parity.StageID]parity.Domain, len(reg.Stages))
	for _, s := range reg.Stages {
		r.stageDomain[s.ID] = s.Domain
	}
	r.token, err = newToken()
	return err
}

// ignoreOutputRoot keeps parity/out out of git status: when path lies inside
// <repo>/parity/out it writes a .gitignore holding "*" there (once). Failures
// only leave the directory visible to git, so they are ignored.
func ignoreOutputRoot(repoRoot, path string) {
	dir := filepath.Join(repoRoot, filepath.FromSlash(defaultOutDir))
	rel, err := filepath.Rel(dir, path)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return
	}
	p := filepath.Join(dir, ".gitignore")
	if _, err := os.Stat(p); errors.Is(err, os.ErrNotExist) {
		if err := os.MkdirAll(dir, 0o755); err == nil {
			_ = os.WriteFile(p, []byte("*\n"), 0o644)
		}
	}
}

func (r *runner) startOracle() error {
	aliases, err := oracle.BidderAliases(r.repo)
	if err != nil {
		return err
	}
	opts := oracle.Options{RepoRoot: r.opts.RepoRoot, Logf: r.opts.Logf}
	if r.corpus.Adapters != nil {
		opts.Templates = oracle.TemplatesFromCorpus(r.corpus.Adapters, aliases)
		opts.Replay = oracle.ReplayFromCorpus(r.corpus.Adapters)
	}
	r.oracle, err = oracle.Start(opts)
	return err
}

func (r *runner) close() {
	if r.oracle != nil {
		if err := r.oracle.Close(); err != nil {
			r.logf("[parity runner] %v", err)
		}
	}
}

func (r *runner) logf(format string, args ...any) { r.opts.Logf(format, args...) }

// repoRel is the recorded form of a path: repo-relative when inside the
// checkout.
func (r *runner) repoRel(p string) string {
	rel, err := filepath.Rel(r.opts.RepoRoot, p)
	if err != nil || strings.HasPrefix(rel, "..") {
		return filepath.ToSlash(p)
	}
	return filepath.ToSlash(rel)
}

// corpusErrors refuses a corpus that did not load cleanly. The loaders skip
// what they cannot read (one bad replay file drops every replayharness
// case), so a run would compare fewer cases than the corpus holds and could
// pass, or end "nothing was compared", without saying why.
func corpusErrors(errs []error) error {
	if len(errs) == 0 {
		return nil
	}
	return fmt.Errorf("runner: the corpus has %d load errors, and the cases they hold would not be compared:\n%w", len(errs), errors.Join(errs...))
}

func newToken() (string, error) {
	b := make([]byte, tokenBytes)
	if _, err := rand.Read(b); err != nil {
		return "", fmt.Errorf("runner: generating the parity token: %w", err)
	}
	return hex.EncodeToString(b), nil
}

// run selects and executes the cases and writes the reports.
func (r *runner) run(ctx context.Context) (*Result, error) {
	if r.opts.Budget > 0 {
		r.deadline = r.started.Add(r.opts.Budget)
	}
	cases, skipped := selectCases(r.corpus, r.opts.Profiles, r.opts.Selection)
	batches := planBatches(cases)
	if r.opts.Selection.wantsCorpus(CorpusIntrospection) {
		addIntrospection(batches, r.profiles)
	}
	shuffleBatches(batches, r.opts.Shuffle)
	for _, s := range skipped {
		if err := r.recordSkipped(s.rc, profileLabel(s.rc.Base, s.rc.Case.Profile), s.reason); err != nil {
			return nil, err
		}
	}
	for _, n := range selectNotCompared(r.corpus, r.opts.Profiles, r.opts.Selection) {
		if err := r.recordNotCompared(n); err != nil {
			return nil, err
		}
	}
	r.logf("[parity runner] %s: %d cases in %d batches, %s vs %s", r.opts.RunID, len(cases), len(batches), r.opts.A.Name, r.opts.B.Name)
	for i, b := range batches {
		if err := r.runBatch(ctx, i, len(batches), b); err != nil {
			return nil, err
		}
	}
	return r.finish()
}

// finish writes the run documents and the coverage matrix.
func (r *runner) finish() (*Result, error) {
	finished := time.Now().UTC()
	r.mu.Lock()
	summary := report.RunSummary{
		RunID: r.opts.RunID, Mode: r.opts.Mode, ImplA: r.opts.A.Name, ImplB: r.opts.B.Name,
		BinaryA: r.repoRel(r.opts.A.Binary), BinaryB: r.repoRel(r.opts.B.Binary),
		StartedAt: r.started.Format(time.RFC3339), FinishedAt: finished.Format(time.RFC3339),
		DurationMS: finished.Sub(r.started).Milliseconds(), Servers: r.servers, Options: r.optionsRecord(), Notes: r.notes,
	}
	r.mu.Unlock()
	run, err := r.writer.Finish(summary)
	if err != nil {
		return nil, err
	}
	m, err := matrix.BuildMatrix(run)
	if err != nil {
		return nil, err
	}
	if err := matrix.WriteRunFiles(m, run.Layout.Dir); err != nil {
		return nil, err
	}
	return &Result{Run: run, Matrix: m}, nil
}

// optionsRecord is the run.json options member.
func (r *runner) optionsRecord() map[string]string {
	o := r.opts
	rec := map[string]string{
		"profiles": strings.Join(o.Profiles, ","), "shards": strconv.Itoa(o.Shards), "shuffle": strconv.FormatInt(o.Shuffle, 10),
		"corpora": strings.Join(o.Selection.Corpora, ","), "kinds": strings.Join(o.Selection.Kinds, ","),
		"limit": strconv.Itoa(o.Selection.Limit), "budget": o.Budget.String(), "case_timeout": o.CaseTimeout.String(),
		"fixture_selftest": strconv.FormatBool(o.FixtureSelfTest),
	}
	if o.Selection.Match != nil {
		rec["match"] = o.Selection.Match.String()
	}
	return rec
}

func (r *runner) note(format string, args ...any) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.notes = append(r.notes, fmt.Sprintf(format, args...))
}

// newSummary is a case summary with the case identity filled in.
func (r *runner) newSummary(rc *runCase, label string) report.CaseSummary {
	s := report.NewCaseSummary(rc.ID)
	cs := rc.Case
	s.FixturePath, s.Endpoint, s.Profile, s.Kind, s.Corpus, s.Bidder = cs.FixturePath, cs.Endpoint, label, string(cs.Kind), cs.Corpus, rc.Bidder
	s.Tags = append(s.Tags, cs.Tags...)
	s.ImplA, s.ImplB = r.opts.A.Name, r.opts.B.Name
	return s
}

func (r *runner) recordSkipped(rc *runCase, label, reason string) error {
	s := r.newSummary(rc, label)
	s.Status, s.Reason = report.CaseSkipped, reason
	return r.writer.AddCase(s, nil)
}

// recordNotCompared reports a request the corpus does not compare: status
// not-compared, the reason "<tag>: <reason>", and notes naming the request
// and where it came from.
func (r *runner) recordNotCompared(n notComparedCase) error {
	nc := n.NC
	s := report.NewCaseSummary(n.ID)
	s.FixturePath, s.Endpoint, s.Profile, s.Kind, s.Corpus = nc.FixturePath, nc.Endpoint, n.Base, string(nc.Kind), nc.Corpus
	s.ImplA, s.ImplB = r.opts.A.Name, r.opts.B.Name
	s.Status, s.Reason = report.CaseNotCompared, nc.Tag+": "+nc.Reason
	s.Notes = append(s.Notes, "request "+nc.Request)
	for _, src := range nc.Sources {
		s.Notes = append(s.Notes, "from "+src)
	}
	return r.writer.AddCase(s, nil)
}

func profileLabel(base string, p corpus.Profile) string {
	if p.IsDefault() {
		return base
	}
	return base + "+" + p.Name()
}

// runBatch prepares the batch's working directory, starts its shard pairs,
// runs the cases and stops the servers.
func (r *runner) runBatch(ctx context.Context, idx, total int, b *batch) error {
	if r.expired(ctx) {
		return r.skipAll(b.Cases, b.Label(), r.stopReason(ctx))
	}
	workdir, err := r.prepareWorkdir(b)
	if err != nil {
		return err
	}
	shards := r.opts.Shards
	if shards > len(b.Cases) {
		shards = len(b.Cases)
	}
	r.logf("[parity runner] batch %d/%d %s: %d cases on %d shards", idx+1, total, b.Name, len(b.Cases), shards)
	pairs := r.startPairs(ctx, b, workdir, shards)
	defer r.stopPairs(pairs)
	errs := make([]error, len(pairs))
	var wg sync.WaitGroup
	for i, p := range pairs {
		wg.Add(1)
		go func(i int, p *pair) {
			defer wg.Done()
			errs[i] = p.runShard(ctx, shardCases(b.Cases, i, len(pairs)))
		}(i, p)
	}
	wg.Wait()
	return errors.Join(errs...)
}

// shardCases are the cases of shard i out of n (round robin, order kept).
func shardCases(cases []*runCase, i, n int) []*runCase {
	var out []*runCase
	for j := i; j < len(cases); j += n {
		out = append(out, cases[j])
	}
	return out
}

// runShard runs cases one at a time on a pair.
func (p *pair) runShard(ctx context.Context, cases []*runCase) error {
	for i, rc := range cases {
		if p.r.expired(ctx) {
			return p.r.skipAll(cases[i:], p.b.Label(), p.r.stopReason(ctx))
		}
		started := time.Now()
		if err := p.record(rc, p.execute(ctx, rc), started); err != nil {
			return err
		}
	}
	return nil
}

func (r *runner) expired(ctx context.Context) bool {
	return ctx.Err() != nil || (!r.deadline.IsZero() && time.Now().After(r.deadline))
}

func (r *runner) stopReason(ctx context.Context) string {
	if err := ctx.Err(); err != nil {
		return "run stopped: " + err.Error()
	}
	return reasonBudget
}

func (r *runner) skipAll(cases []*runCase, label, reason string) error {
	for _, rc := range cases {
		if err := r.recordSkipped(rc, label, reason); err != nil {
			return err
		}
	}
	return nil
}

// prepareWorkdir writes the batch's pbs.yaml and data files.
func (r *runner) prepareWorkdir(b *batch) (string, error) {
	base, err := r.profiles.Resolve(b.Base)
	if err != nil {
		return "", err
	}
	currencyPath := oracle.CurrencyPath
	if len(b.Profile.CurrencyRates) > 0 {
		u, err := r.oracle.RegisterCurrency(b.Name, b.Profile.CurrencyRates, "")
		if err != nil {
			return "", err
		}
		parsed, err := url.Parse(u)
		if err != nil {
			return "", fmt.Errorf("runner: oracle currency URL %q: %w", u, err)
		}
		currencyPath = parsed.Path
	}
	cfg, err := BuildConfig(base, b.Profile, HostConfig{OracleURL: r.oracle.URL(), CurrencyPath: currencyPath, DefaultRequest: len(b.Profile.DefaultRequest) > 0})
	if err != nil {
		return "", err
	}
	stored := r.opts.StoredDir
	if _, err := os.Stat(stored); err != nil {
		stored = ""
	}
	dir := r.layout.Path("work", slug(b.Name))
	w, err := PrepareWorkdir(WorkdirSpec{
		Dir: dir, StaticDir: filepath.Join(r.opts.RepoRoot, StaticDirName), StoredFixtures: stored,
		Config: cfg, Stored: b.Stored, DefaultRequest: b.Profile.DefaultRequest,
	})
	if err != nil {
		return "", err
	}
	for _, n := range w.Notes {
		r.note("batch %s: %s", b.Name, n)
	}
	return dir, nil
}

var slugRE = regexp.MustCompile(`[^A-Za-z0-9._-]+`)

// slug makes a batch name safe as a file name.
func slug(name string) string {
	return strings.Trim(slugRE.ReplaceAllString(name, "_"), "_")
}

// startPairs starts both servers of every shard concurrently.
func (r *runner) startPairs(ctx context.Context, b *batch, workdir string, shards int) []*pair {
	pairs := make([]*pair, shards)
	var wg sync.WaitGroup
	for i := range pairs {
		pairs[i] = &pair{r: r, b: b, shard: i}
		for j, impl := range []Impl{r.opts.A, r.opts.B} {
			wg.Add(1)
			go func(p *pair, j int, impl Impl) {
				defer wg.Done()
				p.sides[j] = r.startSide(ctx, b, workdir, p.shard, []string{sideA, sideB}[j], impl)
			}(pairs[i], j, impl)
		}
	}
	wg.Wait()
	return pairs
}

// startSide starts one server; failures make the side not-yet-instrumented.
func (r *runner) startSide(ctx context.Context, b *batch, workdir string, shard int, name string, impl Impl) *side {
	s := &side{name: name, impl: impl.Name}
	rec := report.ServerRecord{Batch: b.Name, Profile: b.Label(), Shard: shard, Side: name, Impl: impl.Name, Binary: r.repoRel(impl.Binary)}
	defer func() { r.addServer(rec) }()
	if impl.Binary == "" {
		s.nyi = fmt.Sprintf("no %s server binary was given for side %s", impl.Name, name)
		rec.Reason = s.nyi
		return s
	}
	logPath := r.layout.Path("servers", fmt.Sprintf("%s.s%d.%s.%s.log", slug(b.Name), shard, name, impl.Name))
	rec.Log = r.layout.Ref(logPath)
	srv, err := StartServer(ctx, ServerSpec{Impl: impl.Name, Binary: impl.Binary, WorkDir: workdir, LogPath: logPath, Token: r.token, StartTimeout: r.opts.StartTimeout, ExtraEnv: impl.Env})
	if err != nil {
		s.nyi = err.Error()
		rec.Reason = s.nyi
		return s
	}
	s.srv = srv
	if srv.Health == nil {
		s.nyi = srv.NotInstrumented
		rec.Reason = s.nyi
		return s
	}
	rec.Instrumented, rec.Build = true, srv.Health.Build
	if srv.Health.Impl != impl.Name {
		s.nyi = fmt.Sprintf("side %s reports impl %q, want %q", name, srv.Health.Impl, impl.Name)
		rec.Instrumented, rec.Reason = false, s.nyi
	}
	return s
}

func (r *runner) addServer(rec report.ServerRecord) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.servers = append(r.servers, rec)
}

// stopPairs stops every server and drops the oracle captures of calls made
// outside any case (startup GVL and currency fetches).
func (r *runner) stopPairs(pairs []*pair) {
	for _, p := range pairs {
		for _, s := range p.sides {
			if s != nil && s.srv != nil {
				if err := s.srv.Stop(); err != nil {
					r.note("%v", err)
				}
			}
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	for _, impl := range []string{"", r.opts.A.Name, r.opts.B.Name} {
		_, _ = r.oracle.Drain(ctx, "", impl)
	}
}
