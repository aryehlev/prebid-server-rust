// Command parity drives the Go-vs-Rust live differential parity harness.
//
//	parity corpus    [--json]                       corpus statistics
//	parity run       [flags] [--gen N --seed S]     run A against B and write parity/out/<run>
//	parity selftest  [flags]                        A/A self-test plus the adapter fixture self-test
//	parity minimize  --run <dir> [--cluster sig]... minimize clusters on live servers
//	parity report    --run <dir> [--baseline f]     rebuild reports, check the CI ratchet
//	parity matrix    --run <dir> [--out dir]        write the coverage matrix snapshot
//
// Exit codes: 0 success; 1 parity failure (self-test failed, ratchet
// regressed); 2 usage or harness error; 3 self-test incomplete (nothing could
// be compared because the server is not parity-capable yet).
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/gen"
	"github.com/prebid/prebid-server/v4/parity/matrix"
	"github.com/prebid/prebid-server/v4/parity/report"
	"github.com/prebid/prebid-server/v4/parity/runner"
)

// Exit codes.
const (
	exitOK         = 0
	exitFailure    = 1
	exitUsage      = 2
	exitIncomplete = 3
)

// corpusNames are the values --corpus takes: every corpus LoadAll reads, the
// runner's introspection cases, the generated cases of --gen, and all.
var corpusNames = []string{
	corpus.CorpusAdapters, corpus.CorpusSampleRequests, corpus.CorpusExchangeTest, corpus.CorpusBidderParams,
	corpus.CorpusReplayHarness, runner.CorpusIntrospection, gen.CorpusGenerated, "all",
}

// Default locations, relative to the harness root: the workspace the tree
// was assembled from (corpus.Repo.HarnessRoot), which holds the harness in
// parity/ and the Rust build in target/.
const (
	defaultGoBinary   = "parity/out/bin/pbs-go"
	defaultRustBinary = "target/release/pbs-server"
	defaultBaseline   = "parity/baseline.json"
	defaultMatrixDir  = "parity/matrix"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	code := run(ctx, os.Args[1:], os.Stdout, os.Stderr)
	stop()
	os.Exit(code)
}

// run dispatches a subcommand and returns the exit code.
func run(ctx context.Context, args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		usage(stderr)
		return exitUsage
	}
	cmds := map[string]func(context.Context, []string, io.Writer, io.Writer) (int, error){
		"corpus": cmdCorpus, "run": cmdRun, "selftest": cmdSelfTest,
		"minimize": cmdMinimize, "report": cmdReport, "matrix": cmdMatrix,
	}
	cmd, ok := cmds[args[0]]
	if !ok {
		usage(stderr)
		return exitUsage
	}
	code, err := cmd(ctx, args[1:], stdout, stderr)
	switch {
	case errors.Is(err, flag.ErrHelp):
		return exitOK
	case err != nil:
		fmt.Fprintf(stderr, "parity %s: %v\n", args[0], err)
		return exitUsage
	}
	return code
}

func usage(w io.Writer) {
	fmt.Fprintln(w, "usage: parity corpus|run|selftest|minimize|report|matrix [flags]  (parity <cmd> -h for flags)")
}

// common are the flags every subcommand shares.
type common struct {
	repo    string
	verbose bool
}

func (c *common) register(fs *flag.FlagSet) {
	fs.StringVar(&c.repo, "repo", "", "the assembled tree, or a directory inside it or the workspace (default: found from the working directory)")
	fs.BoolVar(&c.verbose, "v", false, "log progress to stderr")
}

// repoRoot resolves --repo to the assembled tree.
func (c *common) repoRoot() (string, error) {
	repo, err := c.find()
	if err != nil {
		return "", err
	}
	return repo.Root(), nil
}

// roots resolves --repo to the assembled tree and its harness root.
func (c *common) roots() (tree, harness string, err error) {
	repo, err := c.find()
	if err != nil {
		return "", "", err
	}
	if harness, err = repo.HarnessRoot(); err != nil {
		return "", "", err
	}
	return repo.Root(), harness, nil
}

func (c *common) find() (*corpus.Repo, error) {
	start := c.repo
	if start == "" {
		start = "."
	}
	return corpus.FindRepo(start)
}

func (c *common) logf(stderr io.Writer) func(string, ...any) {
	if !c.verbose {
		return nil
	}
	return func(format string, args ...any) { fmt.Fprintf(stderr, format+"\n", args...) }
}

func cmdCorpus(_ context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("corpus", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var c common
	c.register(fs)
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	root, err := c.repoRoot()
	if err != nil {
		return exitUsage, err
	}
	repo, err := corpus.OpenRepo(root)
	if err != nil {
		return exitUsage, err
	}
	all, err := corpus.LoadAll(repo)
	if err != nil {
		return exitUsage, err
	}
	for _, e := range all.Errors {
		fmt.Fprintln(stderr, e)
	}
	return exitOK, writeJSON(stdout, all.Stats())
}

// runFlags are the flags of run and selftest.
type runFlags struct {
	common
	out, runID, implA, binA, implB, binB string
	profiles, profilesDir, storedDir     string
	corpora, kinds, match                string
	limit, shards, gen                   int
	seed                                 uint64
	shuffle                              int64
	budget, caseTimeout, startTimeout    time.Duration
	fixtures, noBuild                    bool
	baseline                             string
}

func (f *runFlags) register(fs *flag.FlagSet, withB bool) {
	f.common.register(fs)
	fs.StringVar(&f.out, "out", "", "run directory (default parity/out/<run-id>)")
	fs.StringVar(&f.runID, "run-id", "", "run name (default a UTC timestamp)")
	fs.StringVar(&f.implA, "impl-a", parity.ImplGo, "implementation A (the reference): go or rust")
	fs.StringVar(&f.binA, "bin-a", "", "server binary of A (default: build Go into "+defaultGoBinary+")")
	if withB {
		fs.StringVar(&f.implB, "impl-b", parity.ImplRust, "implementation B: go or rust")
		fs.StringVar(&f.binB, "bin-b", "", "server binary of B (default: the Go build for go, "+defaultRustBinary+" for rust when present)")
	}
	fs.StringVar(&f.profiles, "profiles", runner.DefaultProfile, "comma-separated base profiles (parity/fixtures/profiles)")
	fs.StringVar(&f.profilesDir, "profiles-dir", "", "profile fixtures directory")
	fs.StringVar(&f.storedDir, "stored-dir", "", "stored data fixtures directory")
	fs.StringVar(&f.corpora, "corpus", "all", "comma-separated corpora: "+strings.Join(corpusNames, ", "))
	fs.IntVar(&f.gen, "gen", 0, "generated cases (corpus gen, parity/gen) to add to the corpus")
	fs.Uint64Var(&f.seed, "seed", 1, "seed the --gen cases are drawn with")
	fs.StringVar(&f.kinds, "kinds", "", "comma-separated case kinds (adapter, lifted_auction, endpoint, exchange, bidder_params, introspection)")
	fs.StringVar(&f.match, "match", "", "regular expression on case IDs")
	fs.IntVar(&f.limit, "limit", 0, "maximum cases per base profile (0: no limit)")
	fs.IntVar(&f.shards, "shards", 1, "server pairs per batch")
	fs.Int64Var(&f.shuffle, "shuffle", 0, "run each batch's cases in an order drawn from this seed (0: case ID order)")
	fs.DurationVar(&f.budget, "budget", 0, "stop starting cases after this long (0: no limit)")
	fs.DurationVar(&f.caseTimeout, "case-timeout", 30*time.Second, "timeout of one case")
	fs.DurationVar(&f.startTimeout, "start-timeout", 90*time.Second, "timeout of a server start")
	fs.BoolVar(&f.noBuild, "no-build", false, "do not build the Go server when --bin-a is empty")
	if withB {
		fs.BoolVar(&f.fixtures, "fixtures", false, "also run the adapter fixture self-test of A")
		fs.StringVar(&f.baseline, "baseline", "", "check the CI ratchet against this baseline after the run")
	}
}

// options resolves the flags into runner options, building Go when needed.
func (f *runFlags) options(ctx context.Context, stderr io.Writer) (runner.Options, error) {
	root, harness, err := f.roots()
	if err != nil {
		return runner.Options{}, err
	}
	opts := runner.Options{
		RepoRoot: root, HarnessRoot: harness, OutDir: f.out, RunID: f.runID, ProfilesDir: f.profilesDir, StoredDir: f.storedDir,
		Profiles: splitList(f.profiles), Shards: f.shards, Shuffle: f.shuffle, Budget: f.budget, CaseTimeout: f.caseTimeout,
		StartTimeout: f.startTimeout, FixtureSelfTest: f.fixtures, Logf: f.logf(stderr),
		Selection: runner.Selection{Corpora: splitList(f.corpora), Kinds: splitList(f.kinds), Limit: f.limit},
	}
	if f.match != "" {
		if opts.Selection.Match, err = regexp.Compile(f.match); err != nil {
			return opts, fmt.Errorf("--match: %w", err)
		}
	}
	if f.gen < 0 {
		return opts, fmt.Errorf("--gen %d: must not be negative", f.gen)
	}
	if f.gen > 0 {
		if opts.Corpus, err = generatedCorpus(root, f.seed, f.gen); err != nil {
			return opts, err
		}
		opts.Selection.Corpora = withGenerated(opts.Selection.Corpora)
	}
	if opts.A, err = f.impl(ctx, root, harness, f.implA, f.binA); err != nil {
		return opts, err
	}
	if f.implB != "" {
		if opts.B, err = f.impl(ctx, root, harness, f.implB, f.binB); err != nil {
			return opts, err
		}
	}
	return opts, nil
}

// generatedCorpus loads every corpus of the repository at root and adds n
// cases drawn by parity/gen with seed, keeping Cases sorted by ID. Each added
// case's ID regenerates it exactly (gen.Generator.Regenerate).
func generatedCorpus(root string, seed uint64, n int) (*corpus.Corpus, error) {
	repo, err := corpus.OpenRepo(root)
	if err != nil {
		return nil, err
	}
	c, err := corpus.LoadAll(repo)
	if err != nil {
		return nil, err
	}
	g, err := gen.New(repo, c, gen.Options{})
	if err != nil {
		return nil, err
	}
	generated := make([]corpus.Case, 0, n)
	for i := 0; i < n; i++ {
		cs, err := g.Case(seed, uint64(i))
		if err != nil {
			return nil, fmt.Errorf("--gen: case %d of seed %d: %w", i, seed, err)
		}
		generated = append(generated, cs)
	}
	c.Cases = append(c.Cases, generated...)
	sort.SliceStable(c.Cases, func(i, j int) bool { return c.Cases[i].ID < c.Cases[j].ID })
	return c, nil
}

// withGenerated keeps the generated cases in a corpus selection that names
// corpora but neither all nor gen, so --gen is never silently filtered out.
func withGenerated(corpora []string) []string {
	if len(corpora) == 0 || slices.Contains(corpora, "all") || slices.Contains(corpora, gen.CorpusGenerated) {
		return corpora
	}
	return append(corpora, gen.CorpusGenerated)
}

// impl resolves one implementation's binary: given, the Go build of the tree
// at root, or the default Rust release binary of the harness root when it
// exists (else none: not-yet-instrumented).
func (f *runFlags) impl(ctx context.Context, root, harness, name, bin string) (runner.Impl, error) {
	if name != parity.ImplGo && name != parity.ImplRust {
		return runner.Impl{}, fmt.Errorf("unknown implementation %q (want go or rust)", name)
	}
	if bin != "" {
		abs, err := filepath.Abs(bin)
		return runner.Impl{Name: name, Binary: abs}, err
	}
	if name == parity.ImplRust {
		def := filepath.Join(harness, filepath.FromSlash(defaultRustBinary))
		if _, err := os.Stat(def); err == nil {
			return runner.Impl{Name: name, Binary: def}, nil
		}
		return runner.Impl{Name: name}, nil
	}
	out := filepath.Join(harness, filepath.FromSlash(defaultGoBinary))
	if !f.noBuild {
		if err := runner.BuildGo(ctx, root, out); err != nil {
			return runner.Impl{}, err
		}
	}
	return runner.Impl{Name: name, Binary: out}, nil
}

func cmdRun(ctx context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var f runFlags
	f.register(fs, true)
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	opts, err := f.options(ctx, stderr)
	if err != nil {
		return exitUsage, err
	}
	res, err := runner.Run(ctx, opts)
	if err != nil {
		return exitUsage, err
	}
	printRun(stdout, res.Run)
	return runCode(stdout, res.Run, f.baseline)
}

// runCode is the exit code of parity run. With a baseline the ratchet
// decides, so a run whose mismatches the baseline already counts passes, and
// one with more mismatches or harness errors, fewer compared cases or a new
// cluster fails. Without one the run is judged as a self-test is: a
// mismatch, a harness error or a failing fixture fails it, and a run that
// compared nothing is incomplete.
func runCode(stdout io.Writer, run *report.Run, baseline string) (int, error) {
	if baseline != "" {
		return ratchet(stdout, run, baseline, false)
	}
	return verdictCode(stdout, "run", runner.Judge(run)), nil
}

// verdictCode prints a verdict's reasons, each after prefix, and returns its
// exit code: exitIncomplete when nothing was compared and nothing else is
// wrong, exitFailure for anything else that is wrong.
func verdictCode(stdout io.Writer, prefix string, v runner.Verdict) int {
	for _, r := range v.Reasons {
		fmt.Fprintf(stdout, "%s: %s\n", prefix, r)
	}
	switch {
	case v.OK:
		fmt.Fprintf(stdout, "%s: ok\n", prefix)
		return exitOK
	case v.Incomplete && len(v.Reasons) == 1:
		return exitIncomplete
	}
	return exitFailure
}

func cmdSelfTest(ctx context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("selftest", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var f runFlags
	f.register(fs, false)
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	opts, err := f.options(ctx, stderr)
	if err != nil {
		return exitUsage, err
	}
	res, verdict, err := runner.SelfTest(ctx, opts)
	if err != nil {
		return exitUsage, err
	}
	printRun(stdout, res.Run)
	return verdictCode(stdout, "selftest", verdict), nil
}

func cmdMinimize(ctx context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("minimize", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var c common
	c.register(fs)
	runDir := fs.String("run", "", "finished run directory (required)")
	binA := fs.String("bin-a", "", "server binary of A (default: the run's)")
	binB := fs.String("bin-b", "", "server binary of B (default: the run's)")
	budget := fs.Int("budget", 200, "executions per case")
	var clusters, cases listFlag
	fs.Var(&clusters, "cluster", "cluster signature to minimize (repeatable; default every primary cluster)")
	fs.Var(&cases, "case", "case ID to minimize (repeatable)")
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	if *runDir == "" {
		return exitUsage, errors.New("--run is required")
	}
	root, err := c.repoRoot()
	if err != nil {
		return exitUsage, err
	}
	opts := runner.MinimizeOptions{
		RunDir: *runDir, RepoRoot: root, A: runner.Impl{Binary: absOrEmpty(*binA)}, B: runner.Impl{Binary: absOrEmpty(*binB)},
		Clusters: clusters, Cases: cases, Budget: *budget, Logf: c.logf(stderr),
	}
	outcomes, _, err := runner.MinimizeRun(ctx, opts)
	if err != nil {
		return exitUsage, err
	}
	return exitOK, writeJSON(stdout, outcomes)
}

func cmdReport(_ context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("report", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var c common
	c.register(fs)
	runDir := fs.String("run", "", "finished run directory (required)")
	baseline := fs.String("baseline", "", "baseline to ratchet against (e.g. "+defaultBaseline+")")
	update := fs.Bool("update-baseline", false, "write the run's totals to --baseline instead of checking")
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	if *runDir == "" {
		return exitUsage, errors.New("--run is required")
	}
	run, err := report.Rebuild(*runDir)
	if err != nil {
		return exitUsage, err
	}
	m, err := matrix.BuildMatrix(run)
	if err != nil {
		return exitUsage, err
	}
	if err := matrix.WriteRunFiles(m, run.Layout.Dir); err != nil {
		return exitUsage, err
	}
	printRun(stdout, run)
	if *baseline == "" {
		return exitOK, nil
	}
	return ratchet(stdout, run, *baseline, *update)
}

func cmdMatrix(_ context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("matrix", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var c common
	c.register(fs)
	runDir := fs.String("run", "", "finished run directory (required)")
	out := fs.String("out", "", "snapshot directory (default "+defaultMatrixDir+")")
	if err := fs.Parse(args); err != nil {
		return exitUsage, err
	}
	if *runDir == "" {
		return exitUsage, errors.New("--run is required")
	}
	dir := *out
	if dir == "" {
		_, harness, err := c.roots()
		if err != nil {
			return exitUsage, err
		}
		dir = filepath.Join(harness, filepath.FromSlash(defaultMatrixDir))
	}
	m, err := matrix.BuildFromRun(*runDir)
	if err != nil {
		return exitUsage, err
	}
	if err := matrix.WriteSnapshot(m, dir); err != nil {
		return exitUsage, err
	}
	fmt.Fprintf(stdout, "wrote %s and %s\n", filepath.Join(dir, matrix.FileStatus), filepath.Join(dir, matrix.FileReadme))
	return exitOK, nil
}

// ratchet checks (or with update, writes) the baseline.
func ratchet(stdout io.Writer, run *report.Run, path string, update bool) (int, error) {
	cur := report.BaselineOf(run)
	if update {
		if err := report.WriteJSON(path, cur); err != nil {
			return exitUsage, err
		}
		fmt.Fprintf(stdout, "baseline: wrote %s\n", path)
		return exitOK, nil
	}
	base, err := report.LoadBaseline(path)
	if err != nil {
		return exitUsage, err
	}
	r := report.CheckRatchet(base, cur)
	for _, s := range r.Regressions {
		fmt.Fprintln(stdout, "ratchet regression:", s)
	}
	for _, s := range r.Improvements {
		fmt.Fprintln(stdout, "ratchet improvement:", s)
	}
	if !r.OK {
		return exitFailure, nil
	}
	fmt.Fprintln(stdout, "ratchet: ok")
	return exitOK, nil
}

// printRun prints the run directory and totals.
func printRun(w io.Writer, run *report.Run) {
	c := run.Summary.Counts
	fmt.Fprintf(w, "run %s (%s vs %s): %d cases, %d passing, %d mismatch, %d not-yet-instrumented, %d error, %d skipped, %d not-compared; %d clusters (%d primary)\n",
		run.Summary.RunID, run.Summary.ImplA, run.Summary.ImplB, c.Total, c.Passing, c.Mismatch, c.NotYetInstrumented, c.Error, c.Skipped,
		c.NotCompared, run.Summary.Clusters, run.Summary.PrimaryClusters)
	if f := run.Summary.Fixtures; f != nil {
		fmt.Fprintf(w, "fixtures: %d passing, %d failing, %d excused, %d not run\n", f.Passing, f.Failing, f.Excused, f.NotRun)
	}
	fmt.Fprintf(w, "reports: %s\n", run.Layout.Dir)
}

func writeJSON(w io.Writer, v any) error {
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		return fmt.Errorf("encoding output: %w", err)
	}
	return nil
}

// splitList splits a comma-separated flag; empty items are dropped.
func splitList(s string) []string {
	var out []string
	for _, p := range strings.Split(s, ",") {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
}

func absOrEmpty(p string) string {
	if p == "" {
		return ""
	}
	if abs, err := filepath.Abs(p); err == nil {
		return abs
	}
	return p
}

// listFlag is a repeatable string flag.
type listFlag []string

func (l *listFlag) String() string { return strings.Join(*l, ",") }

func (l *listFlag) Set(v string) error {
	*l = append(*l, v)
	return nil
}
