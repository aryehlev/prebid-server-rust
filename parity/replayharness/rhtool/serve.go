package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"net"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/corpus"
	"github.com/prebid/prebid-server/v4/parity/oracle"
	"github.com/prebid/prebid-server/v4/parity/runner"
)

// Repository paths serve reads.
const (
	profilesDir    = "parity/fixtures/profiles"
	storedFixtures = "parity/fixtures/stored"
	rhStoredDir    = "parity/replayharness/stored"
	staticDir      = "static"
)

// Files serve writes into -dir.
const (
	stateFile    = "state.env"
	tokenFile    = "token"
	outboundFile = "outbound.jsonl"
	workDirName  = "work"
	binaryName   = "pbs-go"
)

// Timing of the outbound log.
const (
	drainEvery   = time.Second
	drainTimeout = 2 * time.Second
	stopTimeout  = 15 * time.Second
	tokenBytes   = 32
	maxServers   = 2
	stateDirPerm = 0o750
	privatePerm  = 0o600
)

var errNotParity = errors.New("the server did not answer /__parity/health as a parity server")

type serveOptions struct {
	repo        string
	bin         string
	binB        string
	implB       string
	dir         string
	profile     string
	caseProfile string
	servers     int
}

func parseServe(args []string, stderr io.Writer) (serveOptions, error) {
	var o serveOptions

	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	fs.SetOutput(stderr)
	fs.StringVar(&o.repo, "repo", ".", "the assembled tree or the workspace, or a directory inside either")
	fs.StringVar(&o.bin, "bin", "", "the Go server binary (default: build it into -dir)")
	fs.StringVar(&o.binB, "bin-b", "", "with -servers 2: the binary of the second server, side B (default: the -bin server)")
	fs.StringVar(&o.implB, "impl-b", parity.ImplGo, "with -servers 2: the implementation side B runs: go or rust")
	fs.StringVar(&o.dir, "dir", "", "directory for the state, token, logs, work directory and outbound log (required)")
	fs.StringVar(&o.profile, "profile", runner.DefaultProfile, "parity base profile (parity/fixtures/profiles)")
	fs.StringVar(&o.caseProfile, "case-profile", "", "a case profile applied over the base profile, as parity/replayharness/profiles/NAME/profile.json (default: none)")
	fs.IntVar(&o.servers, "servers", 1, "servers to start: 1, or 2 for a ReplayHarness --baseline pair (side A, then side B)")

	if err := fs.Parse(args); err != nil {
		return o, fmt.Errorf("parsing flags: %w", err)
	}

	if fs.NArg() > 0 {
		return o, usageError{"serve takes no arguments"}
	}

	return o, o.check()
}

func (o serveOptions) check() error {
	switch {
	case o.dir == "":
		return usageError{"-dir is required"}
	case o.servers < 1 || o.servers > maxServers:
		return usageError{"-servers must be 1 or 2"}
	}

	return o.checkSideB()
}

// checkSideB checks -bin-b and -impl-b.
func (o serveOptions) checkSideB() error {
	switch {
	case o.implB != parity.ImplGo && o.implB != parity.ImplRust:
		return usageError{"-impl-b must be go or rust"}
	case (o.binB != "" || o.implB != parity.ImplGo) && o.servers != maxServers:
		return usageError{"-bin-b and -impl-b need -servers 2"}
	case o.implB == parity.ImplRust && o.binB == "":
		return usageError{"-impl-b rust needs -bin-b"}
	}

	return nil
}

// cmdServe starts the oracle and the servers, writes the state file and logs
// every outbound call until ctx ends.
func cmdServe(ctx context.Context, args []string, stdout, stderr io.Writer) error {
	opts, err := parseServe(args, stderr)
	if err != nil {
		return err
	}

	st, err := newStack(opts)
	if err != nil {
		return err
	}

	err = st.start(ctx, opts)
	if err == nil {
		_, _ = fmt.Fprintf(stdout, "rhtool serve: %s; state in %s\n", st.describe(), filepath.Join(opts.dir, stateFile))
		st.watch(ctx)
	}

	return errors.Join(err, st.stop(context.WithoutCancel(ctx), stderr))
}

// stack is the oracle, the servers and the outbound log of one serve run.
type stack struct {
	root     *os.Root
	repo     string
	oracle   *oracle.Oracle
	servers  []*runner.Server
	outbound *os.File
	counts   map[string]int
}

func newStack(opts serveOptions) (*stack, error) {
	repo, err := corpus.FindRepo(opts.repo)
	if err != nil {
		return nil, fmt.Errorf("finding the repository: %w", err)
	}

	if err := os.MkdirAll(opts.dir, stateDirPerm); err != nil {
		return nil, fmt.Errorf("creating -dir: %w", err)
	}

	root, err := os.OpenRoot(opts.dir)
	if err != nil {
		return nil, fmt.Errorf("opening -dir: %w", err)
	}

	return &stack{root: root, repo: repo.Root(), counts: map[string]int{}}, nil
}

func (s *stack) start(ctx context.Context, opts serveOptions) error {
	bin, err := s.binary(ctx, opts)
	if err != nil {
		return err
	}

	workdir, err := s.prepare(opts)
	if err != nil {
		return err
	}

	token, err := s.newToken()
	if err != nil {
		return err
	}

	if err := s.startServers(ctx, opts, bin, workdir, token); err != nil {
		return err
	}

	return s.writeState(workdir)
}

// startServers starts side A from bin and, with -servers 2, side B from
// -bin-b (default bin) as -impl-b.
func (s *stack) startServers(ctx context.Context, opts serveOptions, bin, workdir, token string) error {
	binB := bin

	if opts.binB != "" {
		abs, err := filepath.Abs(opts.binB)
		if err != nil {
			return fmt.Errorf("resolving -bin-b: %w", err)
		}

		binB = abs
	}

	for i := range opts.servers {
		impl, binary := parity.ImplGo, bin
		if i == 1 {
			impl, binary = opts.implB, binB
		}

		srv, err := runner.StartServer(ctx, runner.ServerSpec{
			Impl: impl, Binary: binary, WorkDir: workdir,
			LogPath: filepath.Join(opts.dir, "server-"+strconv.Itoa(i)+".log"),
			Token:   token, ExtraEnv: proxyGuard(s.oracle.URL()),
		})
		if err != nil {
			return fmt.Errorf("starting server %d: %w", i, err)
		}

		s.servers = append(s.servers, srv)
		if srv.Health == nil {
			return fmt.Errorf("server %d: %w: %s", i, errNotParity, srv.NotInstrumented)
		}
	}

	return nil
}

// binary returns -bin, or builds the server from the repository into -dir.
func (s *stack) binary(ctx context.Context, opts serveOptions) (string, error) {
	if opts.bin != "" {
		bin, err := filepath.Abs(opts.bin)
		if err != nil {
			return "", fmt.Errorf("resolving -bin: %w", err)
		}

		return bin, nil
	}

	out := filepath.Join(s.root.Name(), binaryName)
	if err := runner.BuildGo(ctx, s.repo, out); err != nil {
		return "", fmt.Errorf("building the Go server: %w", err)
	}

	return out, nil
}

// readCaseProfile reads -case-profile; "" is no case profile.
func readCaseProfile(name string) (corpus.Profile, error) {
	if name == "" {
		return corpus.Profile{}, nil
	}

	raw, err := os.ReadFile(name)
	if err != nil {
		return corpus.Profile{}, fmt.Errorf("reading -case-profile: %w", err)
	}

	p, err := corpus.ReadReplayHarnessProfile(raw)
	if err != nil {
		return corpus.Profile{}, fmt.Errorf("-case-profile %s: %w", name, err)
	}

	return p, nil
}

// prepare starts the oracle and writes the work directory, as the parity
// runner does for one batch (parity/runner/run.go, prepareWorkdir).
func (s *stack) prepare(opts serveOptions) (string, error) {
	repo, err := corpus.OpenRepo(s.repo)
	if err != nil {
		return "", fmt.Errorf("opening the repository: %w", err)
	}

	profiles, err := runner.LoadProfiles(filepath.Join(s.repo, profilesDir), filepath.ToSlash)
	if err != nil {
		return "", fmt.Errorf("loading the profiles: %w", err)
	}

	base, err := profiles.Resolve(opts.profile)
	if err != nil {
		return "", fmt.Errorf("resolving profile %s: %w", opts.profile, err)
	}

	caseProfile, err := readCaseProfile(opts.caseProfile)
	if err != nil {
		return "", err
	}

	if err := s.startOracle(repo); err != nil {
		return "", err
	}

	cfg, err := runner.BuildConfig(base, caseProfile, runner.HostConfig{OracleURL: s.oracle.URL(), CurrencyPath: oracle.CurrencyPath})
	if err != nil {
		return "", fmt.Errorf("building pbs.yaml: %w", err)
	}

	stored, err := loadStored(filepath.Join(s.repo, rhStoredDir))
	if err != nil {
		return "", err
	}

	dir := filepath.Join(s.root.Name(), workDirName)

	w, err := runner.PrepareWorkdir(runner.WorkdirSpec{
		Dir: dir, StaticDir: filepath.Join(s.repo, staticDir), StoredFixtures: filepath.Join(s.repo, storedFixtures),
		Config: cfg, Stored: stored,
	})
	if err != nil {
		return "", fmt.Errorf("preparing the work directory: %w", err)
	}

	if len(w.Notes) > 0 {
		return "", fmt.Errorf("%w: %s", errBadStored, strings.Join(w.Notes, "; "))
	}

	return dir, nil
}

// startOracle starts the mock oracle with the adapter fixtures' templates and
// replay index, as the runner does (parity/runner/run.go, startOracle).
func (s *stack) startOracle(repo *corpus.Repo) error {
	adapters, err := corpus.LoadAdapterCorpus(repo)
	if err != nil {
		return fmt.Errorf("loading the adapter corpus: %w", err)
	}

	aliases, err := oracle.BidderAliases(repo)
	if err != nil {
		return fmt.Errorf("loading the bidder aliases: %w", err)
	}

	s.oracle, err = oracle.Start(oracle.Options{
		RepoRoot:  s.repo,
		Templates: oracle.TemplatesFromCorpus(adapters, aliases),
		Replay:    oracle.ReplayFromCorpus(adapters),
	})
	if err != nil {
		return fmt.Errorf("starting the oracle: %w", err)
	}

	s.outbound, err = s.root.OpenFile(outboundFile, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, privatePerm)
	if err != nil {
		return fmt.Errorf("creating the outbound log: %w", err)
	}

	return nil
}

// newToken writes a fresh parity token to -dir/token, readable only by the
// owner, so the caller can check that no recording holds it.
func (s *stack) newToken() (string, error) {
	b := make([]byte, tokenBytes)
	if _, err := rand.Read(b); err != nil {
		return "", fmt.Errorf("generating the parity token: %w", err)
	}

	token := hex.EncodeToString(b)
	if err := s.root.WriteFile(tokenFile, []byte(token), privatePerm); err != nil {
		return "", fmt.Errorf("writing the token file: %w", err)
	}

	return token, nil
}

// proxyGuard points every proxy variable at the oracle. The parity transport
// already sends each wrapped client's calls there; a client it does not wrap
// and that uses http.ProxyFromEnvironment then reaches the oracle too, which
// logs it as unwrapped, instead of the network.
func proxyGuard(oracleURL string) []string {
	env := make([]string, 0, 6)
	for _, name := range []string{"HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"} {
		env = append(env, name+"="+oracleURL)
	}

	return append(env, "NO_PROXY=127.0.0.1,localhost", "no_proxy=127.0.0.1,localhost")
}

var errBadStored = errors.New("parity/replayharness/stored holds data the runner cannot write")

// writeState writes state.env: KEY=VALUE lines for the script, written to a
// temporary name first so a reader never sees half of it.
func (s *stack) writeState(workdir string) error {
	lines := []string{
		"RH_TARGET=" + baseURL(s.servers[0].Ports.Main),
		"RH_ADMIN=" + baseURL(s.servers[0].Ports.Admin),
		"RH_PROMETHEUS=" + baseURL(s.servers[0].Ports.Prometheus),
		"RH_ORACLE=" + s.oracle.URL(),
		"RH_WORKDIR=" + workdir,
		"RH_TOKEN_FILE=" + filepath.Join(s.root.Name(), tokenFile),
		"RH_OUTBOUND=" + filepath.Join(s.root.Name(), outboundFile),
		"RH_MAIN_PORTS=" + s.mainPorts(),
	}
	if len(s.servers) > 1 {
		// Side A is the first server, the Go reference; side B the second.
		lines = append(lines, "RH_BASELINE="+baseURL(s.servers[1].Ports.Main),
			"RH_SIDE_A="+baseURL(s.servers[0].Ports.Main), "RH_SIDE_B="+baseURL(s.servers[1].Ports.Main),
			"RH_IMPL_B="+s.servers[1].Spec.Impl)
	}

	tmp := stateFile + ".tmp"
	if err := s.root.WriteFile(tmp, []byte(strings.Join(lines, "\n")+"\n"), privatePerm); err != nil {
		return fmt.Errorf("writing the state file: %w", err)
	}

	if err := s.root.Rename(tmp, stateFile); err != nil {
		return fmt.Errorf("writing the state file: %w", err)
	}

	return nil
}

func (s *stack) mainPorts() string {
	ports := make([]int, 0, len(s.servers))
	for _, srv := range s.servers {
		ports = append(ports, srv.Ports.Main)
	}

	return joinPorts(ports)
}

func (s *stack) describe() string {
	parts := make([]string, 0, len(s.servers)+1)
	for i, srv := range s.servers {
		parts = append(parts, fmt.Sprintf("%s server %d on %s (admin %d, prometheus %d)", srv.Spec.Impl, i, baseURL(srv.Ports.Main), srv.Ports.Admin, srv.Ports.Prometheus))
	}

	return strings.Join(append(parts, "oracle on "+s.oracle.URL()), ", ")
}

func baseURL(port int) string {
	return "http://" + net.JoinHostPort("127.0.0.1", strconv.Itoa(port))
}

func joinPorts(ports []int) string {
	out := make([]string, 0, len(ports))
	for _, p := range ports {
		out = append(out, strconv.Itoa(p))
	}

	return strings.Join(out, " ")
}

// watch appends the oracle's captures to the outbound log every second until
// ctx ends.
func (s *stack) watch(ctx context.Context) {
	tick := time.NewTicker(drainEvery)
	defer tick.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
			s.drain(ctx)
		}
	}
}

// drain logs the captures of the calls made outside any parity case: those
// with the meta of a side's implementation, and those without meta (a client
// the parity transport does not wrap).
func (s *stack) drain(ctx context.Context) {
	impls := []string{parity.ImplGo, ""}
	for _, srv := range s.servers {
		if !slices.Contains(impls, srv.Spec.Impl) {
			impls = append(impls, srv.Spec.Impl)
		}
	}

	for _, impl := range impls {
		dctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), drainTimeout)
		caps, _ := s.oracle.Drain(dctx, "", impl)

		cancel()

		for _, c := range caps {
			s.log(c)
		}
	}
}

// outboundRecord is one line of the outbound log.
type outboundRecord struct {
	Kind       string `json:"kind"`
	Bidder     string `json:"bidder,omitempty"`
	Method     string `json:"method"`
	URL        string `json:"url"`
	OrigURL    string `json:"orig_url,omitempty"`
	Status     int    `json:"status,omitempty"`
	Error      string `json:"error,omitempty"`
	Mock       string `json:"mock,omitempty"`
	Unexpected bool   `json:"unexpected,omitempty"`
	Wrapped    bool   `json:"wrapped"`
}

func (s *stack) log(c oracle.Capture) {
	rec := outboundRecord{
		Kind: c.Call.Kind, Bidder: c.Call.Bidder, Method: c.Call.Request.Method, URL: c.Call.Request.URL,
		OrigURL: c.Call.OrigURL, Unexpected: c.Unexpected, Wrapped: c.Meta != nil,
	}
	if c.Call.Response != nil {
		rec.Status = c.Call.Response.Status
	}

	if c.Call.Error != nil {
		rec.Error = *c.Call.Error
	}

	if c.Call.Mock != nil {
		rec.Mock = path.Join(c.Call.Mock.Source, c.Call.Mock.ID)
	}

	s.counts["calls"]++
	s.counts["kind "+rec.Kind]++

	if !rec.Wrapped {
		s.counts["unwrapped"]++
	}

	line, err := json.Marshal(rec)
	if err != nil {
		return
	}

	_, _ = s.outbound.Write(append(line, '\n'))
}

// stop stops the servers, logs the last captures, closes the oracle and
// prints the outbound counts.
func (s *stack) stop(ctx context.Context, stderr io.Writer) error {
	var errs []error

	for _, srv := range s.servers {
		errs = append(errs, srv.Stop())
	}

	if s.oracle != nil {
		dctx, cancel := context.WithTimeout(ctx, stopTimeout)
		s.drain(dctx)
		cancel()

		errs = append(errs, s.oracle.Close())
	}

	if s.outbound != nil {
		errs = append(errs, s.outbound.Close())
	}

	errs = append(errs, s.root.Close())

	_, _ = fmt.Fprintf(stderr, "rhtool serve: outbound calls %s\n", s.summary())

	return errors.Join(errs...)
}

func (s *stack) summary() string {
	b, err := json.Marshal(s.counts)
	if err != nil {
		return err.Error()
	}

	return string(b)
}

// loadStored reads <kind>/<id>.json files under dir into corpus.Stored. A
// missing dir is empty.
func loadStored(dir string) (corpus.Stored, error) {
	out := corpus.Stored{}
	fsys := os.DirFS(dir)
	kinds := []struct {
		name string
		dst  *map[string]json.RawMessage
	}{{"stored_requests", &out.Requests}, {"stored_imps", &out.Imps}, {"stored_responses", &out.Responses}, {"accounts", &out.Accounts}}

	for _, k := range kinds {
		entries, err := fs.ReadDir(fsys, k.name)
		if errors.Is(err, fs.ErrNotExist) {
			continue
		}

		if err != nil {
			return out, fmt.Errorf("reading %s: %w", path.Join(dir, k.name), err)
		}

		*k.dst = map[string]json.RawMessage{}

		for _, e := range entries {
			if e.IsDir() || filepath.Ext(e.Name()) != ".json" {
				continue
			}

			raw, err := fs.ReadFile(fsys, path.Join(k.name, e.Name()))
			if err != nil {
				return out, fmt.Errorf("reading stored data: %w", err)
			}

			(*k.dst)[strings.TrimSuffix(e.Name(), ".json")] = raw
		}
	}

	return out, nil
}
