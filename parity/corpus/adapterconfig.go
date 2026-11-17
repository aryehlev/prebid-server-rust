package corpus

import (
	"bytes"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"path"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/prebid-server/v4/parity"
	"github.com/prebid/prebid-server/v4/parity/schema"
)

// runJSONBidderTest is the adapterstest entry point whose calls define runs.
const runJSONBidderTest = "RunJSONBidderTest"

// Construction kinds recorded on AdapterRun.
const (
	ConstructBuilder = "builder"
	ConstructLiteral = "literal"
	ConstructUnknown = "unknown"
)

// AdapterRun is one adapterstest.RunJSONBidderTest call: the fixture root it
// walks and the Builder configuration of the bidder it runs the fixtures with.
type AdapterRun struct {
	TestFile string `json:"test_file"`
	TestFunc string `json:"test_func"`
	Line     int    `json:"line"`
	Package  string `json:"package"`
	// Root is the repo-relative fixture directory the run walks.
	Root   string `json:"root"`
	Bidder string `json:"bidder"`
	// Construction is "builder", "literal" or "unknown", prefixed with the
	// helper chain ("helper:buildTestBidder/builder").
	Construction  string               `json:"construction"`
	AdapterConfig parity.AdapterConfig `json:"adapter_config"`
	Server        parity.ServerConfig  `json:"server"`
	// Resolved is true when every field the test sets was resolved statically.
	Resolved bool `json:"resolved"`
	// NeedsFake is true when the Go test injects deterministic sources or the
	// adapter has internal randomness (fakes.json).
	NeedsFake   bool     `json:"needs_fake"`
	FakeReasons []string `json:"fake_reasons,omitempty"`
	Problems    []string `json:"problems,omitempty"`
	Notes       []string `json:"notes,omitempty"`
}

// Build returns the adapter build that reproduces the run: the raw Builder
// result with the run's adapter and server configuration.
func (r *AdapterRun) Build() parity.AdapterBuild {
	adapterCfg := r.AdapterConfig
	server := r.Server
	return parity.AdapterBuild{Bidder: r.Bidder, Wrap: parity.WrapRaw, AdapterConfig: &adapterCfg, Server: &server}
}

// ID returns "<test_file>:<test_func>:<line>".
func (r *AdapterRun) ID() string {
	return fmt.Sprintf("%s:%s:%d", r.TestFile, r.TestFunc, r.Line)
}

func (r *AdapterRun) problem(format string, args ...any) {
	r.Problems = append(r.Problems, fmt.Sprintf(format, args...))
}

func (r *AdapterRun) needsFake(reason string) {
	r.NeedsFake = true
	for _, have := range r.FakeReasons {
		if have == reason {
			return
		}
	}
	r.FakeReasons = append(r.FakeReasons, reason)
}

// runExtractor holds the cross-package lookup tables.
type runExtractor struct {
	bidderConsts map[string]string
	bidderNames  map[string]bool
	fakeBidders  map[string]bool
	// builderPackages maps a bidder name to the adapter package directory
	// whose Builder exchange/adapter_builders.go registers for it;
	// builderDirs holds those directories.
	builderPackages map[string]string
	builderDirs     map[string]bool
}

// ExtractAdapterRuns finds every RunJSONBidderTest call under adapters/ and
// statically resolves the Builder(bidder, config.Adapter{...},
// config.Server{...}) configuration each one uses, following local variables,
// package constants, "+" concatenation and test helper functions. Runs whose
// tests inject fakes or whose adapter has internal randomness are flagged
// NeedsFake. Runs are returned in file, then source order.
func ExtractAdapterRuns(repo *Repo) ([]*AdapterRun, error) {
	x, err := newRunExtractor(repo)
	if err != nil {
		return nil, err
	}
	dirs, err := adapterPackageDirs(repo)
	if err != nil {
		return nil, err
	}
	var runs []*AdapterRun
	for _, dir := range dirs {
		pkgRuns, err := x.packageRuns(repo, dir)
		if err != nil {
			return nil, err
		}
		runs = append(runs, pkgRuns...)
	}
	return runs, nil
}

func newRunExtractor(repo *Repo) (*runExtractor, error) {
	consts, err := loadBidderConstants(repo)
	if err != nil {
		return nil, err
	}
	names := make(map[string]bool, len(consts))
	for _, name := range consts {
		names[name] = true
	}
	fakes, err := schema.Fakes()
	if err != nil {
		return nil, fmt.Errorf("corpus: loading fakes registry: %w", err)
	}
	fakeBidders := map[string]bool{}
	for _, seam := range fakes.Adapters {
		fakeBidders[seam.Bidder] = true
	}
	builders, err := loadBuilderPackages(repo, consts)
	if err != nil {
		return nil, err
	}
	dirs := make(map[string]bool, len(builders))
	for _, dir := range builders {
		dirs[dir] = true
	}
	return &runExtractor{bidderConsts: consts, bidderNames: names, fakeBidders: fakeBidders, builderPackages: builders, builderDirs: dirs}, nil
}

// adapterBuildersFile registers every bidder's Builder.
const adapterBuildersFile = RootAdapterBuilders

// loadBuilderPackages maps bidder names to the adapter package directory
// (adapters/<pkg>) of the Builder newAdapterBuilders registers for them
// (openrtb_ext.BidderX: pkg.Builder). Entries whose package is not resolved
// are left out.
func loadBuilderPackages(repo *Repo, consts map[string]string) (map[string]string, error) {
	src, err := repo.ReadFile(adapterBuildersFile)
	if err != nil {
		return nil, err
	}
	parsed, err := parser.ParseFile(token.NewFileSet(), adapterBuildersFile, src, parser.SkipObjectResolution)
	if err != nil {
		return nil, fmt.Errorf("corpus: parsing %s: %w", adapterBuildersFile, err)
	}
	imports := adapterImports(parsed)
	out := map[string]string{}
	ast.Inspect(parsed, func(n ast.Node) bool {
		kv, ok := n.(*ast.KeyValueExpr)
		if !ok {
			return true
		}
		key, keyOK := kv.Key.(*ast.SelectorExpr)
		val, valOK := kv.Value.(*ast.SelectorExpr)
		if !keyOK || !valOK || val.Sel.Name != "Builder" {
			return true
		}
		pkg, pkgOK := val.X.(*ast.Ident)
		bidder, known := consts[key.Sel.Name]
		if pkgOK && known {
			if dir, ok := imports[pkg.Name]; ok {
				out[bidder] = dir
			}
		}
		return true
	})
	if len(out) == 0 {
		return nil, fmt.Errorf("corpus: no Builder registrations found in %s", adapterBuildersFile)
	}
	return out, nil
}

// adapterImports maps the names an adapter package import is referred to by
// (its alias, its directory name, and that name without "_" or "-", the usual
// Go package name) to its directory adapters/<pkg>.
func adapterImports(f *ast.File) map[string]string {
	out := map[string]string{}
	marker := "/" + RootAdapters + "/"
	for _, spec := range f.Imports {
		p, err := strconv.Unquote(spec.Path.Value)
		i := strings.Index(p, marker)
		if err != nil || i < 0 {
			continue
		}
		dir := p[i+1:]
		if spec.Name != nil {
			out[spec.Name.Name] = dir
			continue
		}
		base := path.Base(dir)
		out[base] = dir
		out[strings.NewReplacer("_", "", "-", "").Replace(base)] = dir
	}
	return out
}

// loadBidderConstants maps openrtb_ext BidderX constant names to bidder names.
func loadBidderConstants(repo *Repo) (map[string]string, error) {
	const file = RootOpenRTBExt + "/bidders.go"
	src, err := repo.ReadFile(file)
	if err != nil {
		return nil, err
	}
	parsed, err := parser.ParseFile(token.NewFileSet(), file, src, parser.SkipObjectResolution)
	if err != nil {
		return nil, fmt.Errorf("corpus: parsing %s: %w", file, err)
	}
	out := map[string]string{}
	for _, decl := range parsed.Decls {
		gd, ok := decl.(*ast.GenDecl)
		if !ok || gd.Tok != token.CONST {
			continue
		}
		for _, spec := range gd.Specs {
			vs, ok := spec.(*ast.ValueSpec)
			if !ok || len(vs.Names) != len(vs.Values) {
				continue
			}
			for i, name := range vs.Names {
				lit, ok := vs.Values[i].(*ast.BasicLit)
				if !ok || lit.Kind != token.STRING || !strings.HasPrefix(name.Name, "Bidder") {
					continue
				}
				if v, err := strconv.Unquote(lit.Value); err == nil {
					out[name.Name] = v
				}
			}
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("corpus: no bidder constants found in %s", file)
	}
	return out, nil
}

// adapterPackageDirs lists adapters/<pkg> directories whose tests call
// RunJSONBidderTest.
func adapterPackageDirs(repo *Repo) ([]string, error) {
	entries, err := repo.ReadDir(RootAdapters)
	if err != nil {
		return nil, err
	}
	var dirs []string
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		dir := path.Join(RootAdapters, e.Name())
		uses, err := callsRunJSON(repo, dir)
		if err != nil {
			return nil, err
		}
		if uses {
			dirs = append(dirs, dir)
		}
	}
	sort.Strings(dirs)
	return dirs, nil
}

func callsRunJSON(repo *Repo, dir string) (bool, error) {
	entries, err := repo.ReadDir(dir)
	if err != nil {
		return false, err
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), "_test.go") {
			continue
		}
		src, err := repo.ReadFile(path.Join(dir, e.Name()))
		if err != nil {
			return false, err
		}
		if bytes.Contains(src, []byte(runJSONBidderTest)) {
			return true, nil
		}
	}
	return false, nil
}

// packageRuns extracts the runs of one adapter package.
func (x *runExtractor) packageRuns(repo *Repo, dir string) ([]*AdapterRun, error) {
	pkg, err := parseGoPackage(repo, dir)
	if err != nil {
		return nil, err
	}
	var runs []*AdapterRun
	for _, file := range pkg.sortedFiles() {
		if !strings.HasSuffix(file, "_test.go") {
			continue
		}
		for _, decl := range pkg.files[file].Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok || fn.Body == nil {
				continue
			}
			for _, call := range runCalls(fn.Body) {
				runs = append(runs, x.extractRun(pkg, file, fn, call))
			}
		}
	}
	return runs, nil
}

// runCalls returns the RunJSONBidderTest calls of body in source order.
func runCalls(body *ast.BlockStmt) []*ast.CallExpr {
	var calls []*ast.CallExpr
	ast.Inspect(body, func(n ast.Node) bool {
		if call, ok := n.(*ast.CallExpr); ok && calleeName(call) == runJSONBidderTest {
			calls = append(calls, call)
		}
		return true
	})
	return calls
}

// extractRun resolves one RunJSONBidderTest(t, dir, bidder) call.
func (x *runExtractor) extractRun(pkg *goPackage, file string, fn *ast.FuncDecl, call *ast.CallExpr) *AdapterRun {
	run := &AdapterRun{
		TestFile:     file,
		TestFunc:     fn.Name.Name,
		Line:         pkg.line(call.Pos()),
		Package:      pkg.dir,
		Construction: ConstructUnknown,
	}
	sc := newScope(pkg, fn.Body).at(call.Pos())
	if len(call.Args) != 3 {
		run.problem("RunJSONBidderTest has %d arguments, want 3", len(call.Args))
		return run
	}
	if dir, ok := resolveString(bound{call.Args[1], sc}, 0); ok {
		run.Root = path.Join(pkg.dir, dir)
	} else {
		run.problem("fixture directory not resolved: %s", exprString(pkg.fset, call.Args[1]))
	}
	x.resolveBidder(run, bound{call.Args[2], sc}, "", 0)
	if id, ok := call.Args[2].(*ast.Ident); ok {
		x.detectMutations(run, pkg, fn.Body, id.Name, call.Pos())
	}
	x.finishRun(run)
	return run
}

// finishRun fills in the bidder fallback, fake tagging and resolution status.
func (x *runExtractor) finishRun(run *AdapterRun) {
	if run.Bidder == "" {
		guess := path.Base(run.Package)
		if x.bidderNames[guess] {
			run.Bidder = guess
			run.Notes = append(run.Notes, "bidder name inferred from the package directory")
		} else {
			run.problem("bidder name not resolved")
		}
	}
	if x.fakeBidders[run.Bidder] {
		run.needsFake("adapter reads the clock or a random source (parity/schema/fakes.json)")
	}
	x.checkBuilderPackage(run)
	run.Resolved = len(run.Problems) == 0
}

// checkBuilderPackage flags a run of a registered adapter package whose bidder
// name is registered with another package's Builder (a test that builds its
// own Builder under, say, openrtb_ext.BidderAJA). The parity contract builds
// adapters by bidder name, so such a run's fixtures would execute a different
// adapter than the Go test does.
func (x *runExtractor) checkBuilderPackage(run *AdapterRun) {
	dir, known := x.builderPackages[run.Bidder]
	if !known || dir == run.Package || !x.builderDirs[run.Package] {
		return
	}
	run.problem("bidder %q is registered with the Builder of %s, not of %s; a build by bidder name would run another adapter", run.Bidder, dir, run.Package)
}

// resolveBidder follows the bidder argument to a Builder call, a helper or a
// struct literal. via records the helper chain for Construction.
func (x *runExtractor) resolveBidder(run *AdapterRun, b bound, via string, depth int) {
	if depth > maxResolveDepth || b.expr == nil {
		run.problem("bidder expression too deep to resolve")
		return
	}
	switch e := b.expr.(type) {
	case *ast.Ident:
		next, ok := b.sc.lookup(e.Name)
		if !ok {
			run.problem("bidder variable %q not resolved", e.Name)
			return
		}
		x.resolveBidder(run, next, via, depth+1)
	case *ast.ParenExpr:
		x.resolveBidder(run, bound{e.X, b.sc}, via, depth+1)
	case *ast.TypeAssertExpr:
		x.resolveBidder(run, bound{e.X, b.sc}, via, depth+1)
	case *ast.CallExpr:
		x.resolveCall(run, e, b.sc, via, depth)
	case *ast.UnaryExpr, *ast.CompositeLit:
		x.fromLiteral(run, b, via)
	default:
		run.problem("unsupported bidder expression: %s", exprString(b.sc.pkg.fset, b.expr))
	}
}

func (x *runExtractor) resolveCall(run *AdapterRun, call *ast.CallExpr, sc *scope, via string, depth int) {
	name := calleeName(call)
	if name == "Builder" {
		run.Construction = via + ConstructBuilder
		x.fromBuilder(run, call, sc)
		return
	}
	helper, ok := sc.pkg.funcs[name]
	if _, isIdent := call.Fun.(*ast.Ident); !ok || !isIdent || helper.Body == nil {
		run.problem("unsupported bidder constructor %q", name)
		return
	}
	hsc := newScope(sc.pkg, helper.Body)
	bindParams(hsc, helper, call, sc)
	ret, retPos, ok := firstReturn(helper.Body)
	if !ok {
		run.problem("helper %s has no return value", name)
		return
	}
	if id, isIdent := ret.(*ast.Ident); isIdent {
		x.detectMutations(run, sc.pkg, helper.Body, id.Name, retPos)
	}
	x.resolveBidder(run, bound{ret, hsc.at(retPos)}, via+"helper:"+name+"/", depth+1)
}

// bindParams binds a helper's parameters to the call's arguments.
func bindParams(hsc *scope, helper *ast.FuncDecl, call *ast.CallExpr, caller *scope) {
	i := 0
	for _, field := range helper.Type.Params.List {
		for _, name := range field.Names {
			if i < len(call.Args) {
				hsc.params[name.Name] = bound{expr: call.Args[i], sc: caller}
			}
			i++
		}
	}
}

// firstReturn returns the first returned value of body and its position.
func firstReturn(body *ast.BlockStmt) (ast.Expr, token.Pos, bool) {
	var (
		ret ast.Expr
		pos token.Pos
	)
	ast.Inspect(body, func(n ast.Node) bool {
		if ret != nil {
			return false
		}
		if _, isFunc := n.(*ast.FuncLit); isFunc {
			return false
		}
		if rs, ok := n.(*ast.ReturnStmt); ok && len(rs.Results) > 0 {
			ret, pos = rs.Results[0], rs.Pos()
		}
		return true
	})
	return ret, pos, ret != nil
}

// fromBuilder reads Builder(bidder, config.Adapter{...}, config.Server{...}).
func (x *runExtractor) fromBuilder(run *AdapterRun, call *ast.CallExpr, sc *scope) {
	if len(call.Args) != 3 {
		run.problem("Builder has %d arguments, want 3", len(call.Args))
		return
	}
	if name, ok := x.bidderName(bound{call.Args[0], sc}, 0); ok {
		run.Bidder = name
	} else {
		run.problem("Builder bidder name not resolved: %s", exprString(sc.pkg.fset, call.Args[0]))
	}
	x.adapterConfig(run, bound{call.Args[1], sc})
	x.serverConfig(run, bound{call.Args[2], sc})
}

// bidderName resolves openrtb_ext.BidderX, BidderName("x") or a string.
func (x *runExtractor) bidderName(b bound, depth int) (string, bool) {
	if depth > maxResolveDepth {
		return "", false
	}
	switch e := b.expr.(type) {
	case *ast.SelectorExpr:
		v, ok := x.bidderConsts[e.Sel.Name]
		return v, ok
	case *ast.CallExpr:
		if len(e.Args) == 1 && calleeName(e) == "BidderName" {
			return resolveString(bound{e.Args[0], b.sc}, depth+1)
		}
	case *ast.Ident:
		if v, ok := x.bidderConsts[e.Name]; ok {
			return v, true
		}
		if next, ok := b.sc.lookup(e.Name); ok {
			return x.bidderName(next, depth+1)
		}
	}
	return resolveString(b, depth+1)
}

func (x *runExtractor) adapterConfig(run *AdapterRun, b bound) {
	lit, sc, ok := compositeOf(b, 0)
	if !ok {
		run.problem("config.Adapter argument is not a literal: %s", exprString(b.sc.pkg.fset, b.expr))
		return
	}
	fields := compositeFields(lit)
	str := func(key string, dst *string) {
		if expr, ok := fields[key]; ok {
			if v, ok := resolveString(bound{expr, sc}, 0); ok {
				*dst = v
			} else {
				run.problem("config.Adapter.%s not resolved: %s", key, exprString(sc.pkg.fset, expr))
			}
		}
	}
	str("Endpoint", &run.AdapterConfig.Endpoint)
	str("ExtraAdapterInfo", &run.AdapterConfig.ExtraInfo)
	str("PlatformID", &run.AdapterConfig.PlatformID)
	str("AppSecret", &run.AdapterConfig.AppSecret)
	if expr, ok := fields["XAPI"]; ok {
		run.AdapterConfig.XAPI = x.xapiConfig(run, bound{expr, sc})
	}
	for key := range fields {
		switch key {
		case "Endpoint", "ExtraAdapterInfo", "PlatformID", "AppSecret", "XAPI":
		default:
			run.problem("unknown config.Adapter field %s", key)
		}
	}
}

func (x *runExtractor) xapiConfig(run *AdapterRun, b bound) *parity.XAPIConfig {
	lit, sc, ok := compositeOf(b, 0)
	if !ok {
		run.problem("config.Adapter.XAPI is not a literal")
		return nil
	}
	out := &parity.XAPIConfig{}
	fields := compositeFields(lit)
	for key, dst := range map[string]*string{"Username": &out.Username, "Password": &out.Password, "Tracker": &out.Tracker} {
		if expr, ok := fields[key]; ok {
			if v, ok := resolveString(bound{expr, sc}, 0); ok {
				*dst = v
			} else {
				run.problem("config.AdapterXAPI.%s not resolved", key)
			}
		}
	}
	return out
}

func (x *runExtractor) serverConfig(run *AdapterRun, b bound) {
	lit, sc, ok := compositeOf(b, 0)
	if !ok {
		run.problem("config.Server argument is not a literal: %s", exprString(b.sc.pkg.fset, b.expr))
		return
	}
	fields := compositeFields(lit)
	if expr, ok := fields["ExternalUrl"]; ok {
		if v, ok := resolveString(bound{expr, sc}, 0); ok {
			run.Server.ExternalURL = v
		} else {
			run.problem("config.Server.ExternalUrl not resolved")
		}
	}
	if expr, ok := fields["DataCenter"]; ok {
		if v, ok := resolveString(bound{expr, sc}, 0); ok {
			run.Server.DataCenter = v
		} else {
			run.problem("config.Server.DataCenter not resolved")
		}
	}
	if expr, ok := fields["GvlID"]; ok {
		if v, ok := resolveInt(bound{expr, sc}, 0); ok {
			run.Server.GvlID = v
		} else {
			run.problem("config.Server.GvlID not resolved")
		}
	}
}

// literalEndpointFields are struct fields that hold the endpoint of adapters
// whose tests build the adapter struct directly.
var literalEndpointFields = []string{"endpoint", "Endpoint", "uri", "URI", "endpointURL", "EndpointURL"}

// fromLiteral handles tests that construct the adapter struct directly. The
// Builder is bypassed, so the run always needs fakes for parity.
func (x *runExtractor) fromLiteral(run *AdapterRun, b bound, via string) {
	run.Construction = via + ConstructLiteral
	run.needsFake("the Go test constructs the adapter struct directly instead of calling Builder")
	lit, sc, ok := compositeOf(b, 0)
	if !ok {
		run.problem("adapter literal not resolved")
		return
	}
	fields := compositeFields(lit)
	for _, key := range literalEndpointFields {
		if expr, ok := fields[key]; ok {
			if v, ok := resolveString(bound{expr, sc}, 0); ok {
				run.AdapterConfig.Endpoint = v
				return
			}
			run.problem("adapter literal field %s not resolved", key)
			return
		}
	}
}

// detectMutations flags tests that modify or hand the built bidder to other
// code before the fixtures run (for example replacing its random generator).
func (x *runExtractor) detectMutations(run *AdapterRun, pkg *goPackage, body *ast.BlockStmt, name string, limit token.Pos) {
	derived := map[string]bool{name: true}
	ast.Inspect(body, func(n ast.Node) bool {
		// Only statements before the run (or the helper's return) matter.
		if n == nil || n.Pos() >= limit {
			return false
		}
		switch s := n.(type) {
		case *ast.AssignStmt:
			x.trackDerived(derived, s)
			for _, lhs := range s.Lhs {
				if root, ok := fieldRoot(lhs); ok && derived[root] {
					run.needsFake(fmt.Sprintf("the Go test sets a field of the bidder (%s) before running the fixtures", pkg.where(s.Pos())))
				}
			}
		case *ast.CallExpr:
			callee := calleeName(s)
			if callee == runJSONBidderTest || callee == "Builder" || isAssertion(s) {
				return true
			}
			for _, arg := range s.Args {
				if id, ok := arg.(*ast.Ident); ok && derived[id.Name] {
					run.needsFake(fmt.Sprintf("the Go test passes the bidder to %s before running the fixtures", callee))
				}
			}
		}
		return true
	})
}

// trackDerived marks variables assigned from a type assertion of a tracked one.
func (x *runExtractor) trackDerived(derived map[string]bool, s *ast.AssignStmt) {
	if len(s.Rhs) != 1 {
		return
	}
	ta, ok := s.Rhs[0].(*ast.TypeAssertExpr)
	if !ok {
		return
	}
	src, ok := ta.X.(*ast.Ident)
	if !ok || !derived[src.Name] {
		return
	}
	if id, ok := s.Lhs[0].(*ast.Ident); ok && id.Name != "_" {
		derived[id.Name] = true
	}
}

// readOnlyReceivers are packages and values whose methods only inspect their
// arguments (test assertions and *testing.T).
var readOnlyReceivers = map[string]bool{"assert": true, "require": true, "t": true}

// isAssertion reports whether call is assert.X(...), require.X(...) or t.X(...).
func isAssertion(call *ast.CallExpr) bool {
	sel, ok := call.Fun.(*ast.SelectorExpr)
	if !ok {
		return false
	}
	x, ok := sel.X.(*ast.Ident)
	return ok && readOnlyReceivers[x.Name]
}

// fieldRoot returns the root identifier of a field write target such as x.a.b
// or x.(*T).a; a bare identifier is not a field write.
func fieldRoot(expr ast.Expr) (string, bool) {
	sel, ok := expr.(*ast.SelectorExpr)
	if !ok {
		return "", false
	}
	return selectorRoot(sel.X)
}

// selectorRoot returns the root identifier of x.a.b or x.(*T).a.
func selectorRoot(expr ast.Expr) (string, bool) {
	for {
		switch e := expr.(type) {
		case *ast.SelectorExpr:
			expr = e.X
		case *ast.TypeAssertExpr:
			expr = e.X
		case *ast.ParenExpr:
			expr = e.X
		case *ast.Ident:
			return e.Name, true
		default:
			return "", false
		}
	}
}
