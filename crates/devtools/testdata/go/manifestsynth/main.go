// Command manifestsynth writes crates/devtools/testdata/manifest_synth.json, the golden the
// pbs-devtools manifest evaluator is checked against.
//
// It generates a Go adapter package whose tests build bidders in the shapes adapters/*/*_test.go
// use (string and raw literals with every escape form, constants, package and local variables,
// concatenation, integer literal forms, helpers, if-init statements, struct literals) plus shapes
// the evaluator must refuse (state it cannot see, calls Go may skip or repeat). It then runs the
// package with `go test` and records what each adapterstest.RunJSONBidderTest call really
// received. The golden holds the generated sources and those observations; the Rust test parses
// the sources and requires, per case, the exact Go result ("match"), a loud refusal ("loud"), or
// no run at all ("none").
//
// Run from the directory of this module (crates/devtools/testdata/go):
//
//	go run ./manifestsynth
//
// The package is built against the upstream checkout this module builds against (the go.mod
// replace target, .upstream/ at the repository root, or the replacement a GOWORK file names).
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io/fs"
	"math/rand"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

const (
	seed     = 20261117
	pkgName  = "zzsynth"
	tmpName  = "zzsynth_tmp"
	module   = "github.com/prebid/prebid-server/v4"
	fakeName = "adapterstest"
	// genRel is where this program lived inside the upstream module tree when the golden was
	// first generated. The golden's generator label and the fake package's import path are built
	// from it, so the package is compiled in a throwaway module whose path keeps that import path
	// and regenerating the golden stays byte-identical.
	genRel = "rust/crates/devtools/testdata/go/manifestsynth"
	// outRel is the golden, relative to this module's directory.
	outRel = "../manifest_synth.json"
)

// bidderConsts are the openrtb_ext constants the generated tests use, with their Go values.
var bidderConsts = map[string]openrtb_ext.BidderName{
	"Bidder33Across":        openrtb_ext.Bidder33Across,
	"BidderAdf":             openrtb_ext.BidderAdf,
	"BidderAppnexus":        openrtb_ext.BidderAppnexus,
	"BidderAudienceNetwork": openrtb_ext.BidderAudienceNetwork,
	"BidderGamoshi":         openrtb_ext.BidderGamoshi,
	"BidderIx":              openrtb_ext.BidderIx,
	"BidderMicrosoft":       openrtb_ext.BidderMicrosoft,
	"BidderRubicon":         openrtb_ext.BidderRubicon,
	"BidderSmaato":          openrtb_ext.BidderSmaato,
	"BidderYieldlab":        openrtb_ext.BidderYieldlab,
}

type sourceFile struct {
	Name string `json:"name"`
	Src  string `json:"src"`
}

type testCase struct {
	Test   string `json:"test"`
	File   string `json:"file"`
	Expect string `json:"expect"`
	// Blame is what a refusal must name: "<file>:<func>" or, for a file-level refusal, "<file>".
	Blame string `json:"blame"`
	Note  string `json:"note"`
}

type observation struct {
	Test    string            `json:"test"`
	Dir     string            `json:"dir"`
	Root    string            `json:"root"`
	Type    string            `json:"type"`
	Name    *string           `json:"bidder_name"`
	Adapter map[string]string `json:"adapter"`
	Server  map[string]any    `json:"server"`
	Direct  map[string]string `json:"direct"`
}

type golden struct {
	Generator    string            `json:"generator"`
	Notes        []string          `json:"notes"`
	Package      string            `json:"package"`
	Consts       map[string]string `json:"consts"`
	Files        []sourceFile      `json:"files"`
	Cases        []testCase        `json:"cases"`
	Observations []observation     `json:"observations"`
}

func main() {
	out := flag.String("out", filepath.FromSlash(outRel), "golden to write")
	n := flag.Int("n", 240, "number of generated match cases")
	flag.Parse()

	g := newGen(rand.New(rand.NewSource(seed)))
	for i := 0; i < *n; i++ {
		g.matchCase()
	}
	g.loudCases()

	files := g.files()
	obs := run(files)
	consts := map[string]string{}
	for k, v := range bidderConsts {
		consts[k] = string(v)
	}
	gold := golden{
		Generator: "go run ./" + genRel,
		Notes: []string{
			"files: a generated adapter package (adapters/" + pkgName + " for the evaluator); observations: what each RunJSONBidderTest call received when `go test -vet=off` ran it.",
			"observation.test is t.Name() (subtests keep their /suffix); root is path.Join(\"adapters/" + pkgName + "\", dir), what the manifest records as fixture_root.",
			"cases: expect=match requires the evaluator to reproduce the observations of that test exactly with no setup; expect=loud requires an error naming blame or setup statements, never a clean run; expect=none means Go makes no call and the evaluator must report no clean run.",
		},
		Package:      pkgName,
		Consts:       consts,
		Files:        files,
		Cases:        g.cases,
		Observations: obs,
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(gold); err != nil {
		fatalf("encoding golden: %v", err)
	}
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		fatalf("writing %s: %v", *out, err)
	}
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "manifestsynth: "+format+"\n", args...)
	os.Exit(1)
}

// run writes the package into a throwaway module (see genRel), runs its tests and returns what
// the fake RunJSONBidderTest recorded, sorted by test name (stable within a test).
func run(files []sourceFile) []observation {
	dir, err := os.MkdirTemp("", "manifestsynth-")
	if err != nil {
		fatalf("creating a temporary module: %v", err)
	}
	defer os.RemoveAll(dir)

	for _, f := range append(files, moduleFiles()...) {
		p := filepath.Join(dir, filepath.FromSlash(f.Name))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			fatalf("creating %s: %v", filepath.Dir(p), err)
		}
		if err := os.WriteFile(p, []byte(f.Src), 0o644); err != nil {
			fatalf("writing %s: %v", p, err)
		}
	}

	// The module requires only upstream; tidy adds the requirements its packages need, at the
	// versions upstream selects, and drops the go.sum lines they do not.
	tidy := exec.CommandContext(context.Background(), "go", "mod", "tidy")
	tidy.Dir = dir
	tidy.Env = outsideWorkspace()
	commandOutput(tidy)

	rec := filepath.Join(dir, "observations.jsonl")
	cmd := exec.CommandContext(context.Background(), "go", "test", "-vet=off", "-count=1", ".")
	cmd.Dir = dir
	cmd.Env = outsideWorkspace("ZZSYNTH_OUT=" + rec)

	if output, err := cmd.CombinedOutput(); err != nil {
		fatalf("go test failed: %v\n%s", err, output)
	}
	data, err := os.ReadFile(rec)
	if err != nil {
		fatalf("reading %s: %v", rec, err)
	}
	var obs []observation
	dec := json.NewDecoder(bytes.NewReader(data))
	for dec.More() {
		var o observation
		if err := dec.Decode(&o); err != nil {
			fatalf("decoding observation: %v", err)
		}
		o.Root = path.Join("adapters/"+pkgName, o.Dir)
		obs = append(obs, o)
	}
	sort.SliceStable(obs, func(i, j int) bool { return obs[i].Test < obs[j].Test })
	return obs
}

// moduleFiles are the files of the throwaway module besides the generated package: the fake
// adapterstest, and a go.mod and go.sum that build the package against the upstream checkout this
// module builds against (the go.mod replace target or a GOWORK replacement), with upstream's go
// version, under the import path the golden records (see genRel).
func moduleFiles() []sourceFile {
	listed := commandOutput(exec.CommandContext(context.Background(),
		"go", "list", "-m", "-f", "{{.Dir}}\t{{.Version}}\t{{.GoVersion}}", module))

	fields := strings.Split(listed, "\t")
	if len(fields) != 3 || fields[0] == "" {
		fatalf("go list -m %s gave %q; is the module replaced by a checkout?", module, listed)
	}

	upstream, version, goVersion := fields[0], fields[1], fields[2]

	sum, err := fs.ReadFile(os.DirFS(upstream), "go.sum")
	if err != nil {
		fatalf("reading the upstream go.sum: %v", err)
	}

	goMod := "module " + module + "/" + genRel + "/" + tmpName + "\n\ngo " + goVersion + "\n\n" +
		"require " + module + " " + version + "\n\nreplace " + module + " => " + upstream + "\n"

	return []sourceFile{
		{Name: fakeName + "/" + fakeName + ".go", Src: fakeSrc},
		{Name: "go.mod", Src: goMod},
		{Name: "go.sum", Src: string(sum)},
	}
}

// outsideWorkspace is the environment, plus extra, of a go command run in the throwaway module,
// which belongs to no go.work.
func outsideWorkspace(extra ...string) []string {
	return append(append(os.Environ(), "GOWORK=off"), extra...)
}

// commandOutput runs cmd, showing its standard error, and returns its trimmed standard output.
func commandOutput(cmd *exec.Cmd) string {
	cmd.Stderr = os.Stderr

	out, err := cmd.Output()
	if err != nil {
		fatalf("%s: %v", cmd, err)
	}

	return strings.TrimSpace(string(out))
}

// fakeSrc is the adapterstest stand-in: it records the call instead of running fixtures.
const fakeSrc = `package adapterstest

import (
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"testing"

	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/config"
)

func RunJSONBidderTest(t *testing.T, rootDir string, bidder adapters.Bidder) {
	rec := map[string]any{"test": t.Name(), "dir": rootDir, "type": fmt.Sprintf("%T", bidder)}
	v := reflect.ValueOf(bidder)
	for v.Kind() == reflect.Pointer || v.Kind() == reflect.Interface {
		v = v.Elem()
	}
	if f := v.FieldByName("Cfg"); f.IsValid() {
		a := f.Interface().(config.Adapter)
		s := v.FieldByName("Srv").Interface().(config.Server)
		rec["bidder_name"] = v.FieldByName("Name").String()
		rec["adapter"] = map[string]string{"endpoint": a.Endpoint, "extra_info": a.ExtraAdapterInfo,
			"platform_id": a.PlatformID, "app_secret": a.AppSecret, "xapi_username": a.XAPI.Username,
			"xapi_password": a.XAPI.Password, "xapi_tracker": a.XAPI.Tracker}
		rec["server"] = map[string]any{"external_url": s.ExternalUrl, "gvl_id": s.GvlID, "datacenter": s.DataCenter}
	} else {
		rec["direct"] = map[string]string{"Endpoint": v.FieldByName("Endpoint").String(), "Extra": v.FieldByName("Extra").String()}
	}
	f, err := os.OpenFile(os.Getenv("ZZSYNTH_OUT"), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := json.NewEncoder(f).Encode(rec); err != nil {
		t.Fatal(err)
	}
}
`

const adapterSrc = `package zzsynth

import (
	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/adapters"
	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

type adapter struct {
	Name openrtb_ext.BidderName
	Cfg  config.Adapter
	Srv  config.Server
}

func Builder(bidderName openrtb_ext.BidderName, config config.Adapter, server config.Server) (adapters.Bidder, error) {
	return &adapter{Name: bidderName, Cfg: config, Srv: server}, nil
}

func (a *adapter) MakeRequests(*openrtb2.BidRequest, *adapters.ExtraRequestInfo) ([]*adapters.RequestData, []error) {
	return nil, nil
}

func (a *adapter) MakeBids(*openrtb2.BidRequest, *adapters.RequestData, *adapters.ResponseData) (*adapters.BidderResponse, []error) {
	return nil, nil
}

type directAdapter struct {
	Endpoint string
	Extra    string
}

func (a directAdapter) MakeRequests(*openrtb2.BidRequest, *adapters.ExtraRequestInfo) ([]*adapters.RequestData, []error) {
	return nil, nil
}

func (a directAdapter) MakeBids(*openrtb2.BidRequest, *adapters.RequestData, *adapters.ResponseData) (*adapters.BidderResponse, []error) {
	return nil, nil
}

// Used by generated tests.
var errNever error
`

// ---------------------------------------------------------------------------------------------
// Generation

type gen struct {
	r     *rand.Rand
	n     int
	decls strings.Builder
	tests strings.Builder
	extra []sourceFile
	cases []testCase
}

func newGen(r *rand.Rand) *gen {
	return &gen{r: r}
}

func (g *gen) id(prefix string) string {
	g.n++
	return fmt.Sprintf("%s%d", prefix, g.n)
}

func (g *gen) pick(options ...string) string {
	return options[g.r.Intn(len(options))]
}

const testFile = pkgName + "_test.go"

func (g *gen) files() []sourceFile {
	header := "package " + pkgName + "\n\nimport (\n\t\"strings\"\n\t\"testing\"\n\n" +
		"\t\"github.com/prebid/prebid-server/v4/adapters\"\n" +
		"\t\"" + module + "/" + genRel + "/" + tmpName + "/" + fakeName + "\"\n" +
		"\t\"github.com/prebid/prebid-server/v4/config\"\n" +
		"\t\"github.com/prebid/prebid-server/v4/openrtb_ext\"\n)\n\n" +
		"var _ = strings.ToUpper\nvar _ adapters.Bidder\nvar _ = config.Adapter{}\nvar _ = openrtb_ext.BidderIx\n\n"
	files := []sourceFile{
		{Name: pkgName + ".go", Src: adapterSrc},
		{Name: testFile, Src: header + g.decls.String() + g.tests.String()},
	}
	return append(files, g.extra...)
}

// scope is what a generated expression may refer to. Locals are declared on demand into body
// (Go rejects unused locals), so every local is used by the expression that created it.
type scope struct {
	body   *strings.Builder // where new locals go; nil at package level
	locals []string         // string-valued locals declared so far
	params []string         // string-valued helper parameters
}

// local declares a new string local in s.body and returns its name.
func (g *gen) local(depth int, s *scope) string {
	name := g.id("l")
	expr := g.strExpr(depth+1, s)
	switch g.r.Intn(5) {
	case 4:
		// Reassigned from its own value.
		fmt.Fprintf(s.body, "\t%s := %s\n\t%s = %s + %s\n", name, g.literal(), name, name, expr)
	case 0:
		fmt.Fprintf(s.body, "\tvar %s = %s\n", name, expr)
	case 1:
		fmt.Fprintf(s.body, "\tvar %s string = %s\n", name, expr)
	case 2:
		// Declared, then reassigned: the second value is the one in effect.
		fmt.Fprintf(s.body, "\t%s := %s\n\t%s = %s\n", name, g.literal(), name, expr)
	default:
		fmt.Fprintf(s.body, "\t%s := %s\n", name, expr)
	}
	s.locals = append(s.locals, name)
	return name
}

var escapes = []string{`\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\"`, `\x41`, `\x7e`, `\101`, `\177`, `\u00e9`, `\u2028`, `\U0001F600`, `\u0041`}

var runes = []string{"a", "Z", "0", "9", "/", ":", "?", "&", "=", "%", "{", "}", "[", "]", "$", "'", " ", "é", "日", "😀", "<", ">", "#", "-", "_", "."}

func (g *gen) interpreted() string {
	var b strings.Builder
	b.WriteByte('"')
	for i, n := 0, g.r.Intn(8); i < n; i++ {
		if g.r.Intn(3) == 0 {
			b.WriteString(escapes[g.r.Intn(len(escapes))])
		} else {
			b.WriteString(runes[g.r.Intn(len(runes))])
		}
	}
	b.WriteByte('"')
	return b.String()
}

func (g *gen) raw() string {
	var b strings.Builder
	b.WriteByte('`')
	for i, n := 0, g.r.Intn(8); i < n; i++ {
		switch g.r.Intn(6) {
		case 0:
			b.WriteString(g.pick(`\n`, `\`, `"`, `\"`, `\x41`))
		case 1:
			// A raw newline, and a carriage return Go drops from raw strings.
			b.WriteString(g.pick("\n", "\r\n", "\r"))
		default:
			b.WriteString(runes[g.r.Intn(len(runes))])
		}
	}
	b.WriteByte('`')
	return b.String()
}

func (g *gen) literal() string {
	if g.r.Intn(4) == 0 {
		return g.raw()
	}
	return g.interpreted()
}

// constDecl declares a string constant (alone, in a block, typed or not) and returns its name.
func (g *gen) constDecl() string {
	name := g.id("c")
	switch g.r.Intn(4) {
	case 0:
		fmt.Fprintf(&g.decls, "const %s = %s\n\n", name, g.literal())
	case 1:
		fmt.Fprintf(&g.decls, "const %s string = %s + %s\n\n", name, g.literal(), g.literal())
	case 2:
		other := g.id("c")
		fmt.Fprintf(&g.decls, "const (\n\t%s = %s\n\t%s = %s + %s\n)\n\n", other, g.literal(), name, other, g.literal())
	default:
		other := g.id("c")
		fmt.Fprintf(&g.decls, "const %s, %s = %s, %s\n\n", other, name, g.literal(), g.literal())
	}
	return name
}

// varDecl declares a package string variable and returns its name.
func (g *gen) varDecl() string {
	name := g.id("v")
	switch g.r.Intn(3) {
	case 0:
		fmt.Fprintf(&g.decls, "var %s = %s\n\n", name, g.strExpr(1, &scope{}))
	case 1:
		fmt.Fprintf(&g.decls, "var %s string = %s\n\n", name, g.strExpr(1, &scope{}))
	default:
		fmt.Fprintf(&g.decls, "var (\n\t%s = %s\n)\n\n", name, g.strExpr(1, &scope{}))
	}
	return name
}

func (g *gen) strExpr(depth int, s *scope) string {
	if depth >= 4 {
		return g.literal()
	}
	choice := g.r.Intn(10)
	if depth >= 3 {
		choice = g.r.Intn(4)
	}
	switch choice {
	case 0, 1:
		return g.literal()
	case 2:
		return g.constDecl()
	case 3:
		names := append(append([]string(nil), s.locals...), s.params...)
		if len(names) > 0 && g.r.Intn(2) == 0 {
			return names[g.r.Intn(len(names))]
		}
		if s.body != nil {
			return g.local(depth, s)
		}
		return g.interpreted()
	case 4:
		return g.varDecl()
	case 5:
		return "(" + g.strExpr(depth+1, s) + ")"
	default:
		parts := []string{g.strExpr(depth+1, s), g.strExpr(depth+1, s)}
		if g.r.Intn(3) == 0 {
			parts = append(parts, g.strExpr(depth+1, s))
		}
		return strings.Join(parts, " + ")
	}
}

func (g *gen) intLit() string {
	v := g.r.Intn(70000)
	switch g.r.Intn(7) {
	case 0:
		return fmt.Sprintf("0x%X", v)
	case 1:
		return fmt.Sprintf("0o%o", v)
	case 2:
		return fmt.Sprintf("0%o", v+1)
	case 3:
		return fmt.Sprintf("0b%b", v%64)
	case 4:
		s := fmt.Sprint(v + 1000)
		return s[:1] + "_" + s[1:]
	case 5:
		return fmt.Sprintf("0x_%x", v)
	default:
		return fmt.Sprint(v)
	}
}

func (g *gen) intExpr() string {
	switch g.r.Intn(4) {
	case 0:
		name := g.id("i")
		fmt.Fprintf(&g.decls, "const %s = %s\n\n", name, g.intLit())
		return name
	case 1:
		name := g.id("i")
		fmt.Fprintf(&g.decls, "const %s int = %s\n\n", name, g.intLit())
		return name + " + " + g.intLit()
	default:
		return g.intLit()
	}
}

func (g *gen) bidderName() string {
	names := make([]string, 0, len(bidderConsts))
	for k := range bidderConsts {
		names = append(names, k)
	}
	sort.Strings(names)
	return "openrtb_ext." + names[g.r.Intn(len(names))]
}

// dirExpr is a fixture directory: relative, possibly with ./, inner / or a trailing /.
func (g *gen) dirExpr() string {
	base := g.pick("zzsynthtest", "test", "x-test", "a/b_test", "./zzsynthtest", "zzsynthtest/", "./a/./b", "a//b")
	switch g.r.Intn(4) {
	case 0:
		name := g.id("d")
		fmt.Fprintf(&g.decls, "const %s = %q\n\n", name, base)
		return name
	case 1:
		cut := 1 + g.r.Intn(len(base)-1)
		return fmt.Sprintf("%q + %q", base[:cut], base[cut:])
	default:
		return fmt.Sprintf("%q", base)
	}
}

// literalFields renders a composite literal body with the given fields in random layout.
func (g *gen) literalFields(fields [][2]string) string {
	g.r.Shuffle(len(fields), func(i, j int) { fields[i], fields[j] = fields[j], fields[i] })
	if len(fields) == 0 {
		return "{}"
	}
	parts := make([]string, len(fields))
	for i, f := range fields {
		parts[i] = f[0] + ": " + f[1]
	}
	if g.r.Intn(2) == 0 {
		return "{" + strings.Join(parts, ", ") + "}"
	}
	return "{\n\t\t" + strings.Join(parts, ",\n\t\t") + ",\n\t}"
}

func (g *gen) adapterLit(s *scope) string {
	var fields [][2]string
	for _, f := range []string{"Endpoint", "ExtraAdapterInfo", "PlatformID", "AppSecret"} {
		if g.r.Intn(2) == 0 {
			fields = append(fields, [2]string{f, g.strExpr(0, s)})
		}
	}
	if g.r.Intn(4) == 0 {
		var x [][2]string
		for _, f := range []string{"Username", "Password", "Tracker"} {
			if g.r.Intn(2) == 0 {
				x = append(x, [2]string{f, g.strExpr(1, s)})
			}
		}
		fields = append(fields, [2]string{"XAPI", "config.AdapterXAPI" + g.literalFields(x)})
	}
	return "config.Adapter" + g.literalFields(fields)
}

func (g *gen) serverLit(s *scope) string {
	var fields [][2]string
	if g.r.Intn(2) == 0 {
		fields = append(fields, [2]string{"ExternalUrl", g.strExpr(1, s)})
	}
	if g.r.Intn(2) == 0 {
		fields = append(fields, [2]string{"GvlID", g.intExpr()})
	}
	if g.r.Intn(2) == 0 {
		fields = append(fields, [2]string{"DataCenter", g.strExpr(1, s)})
	}
	return "config.Server" + g.literalFields(fields)
}

func (g *gen) addTest(name, body, file, expect, note string) {
	if file == testFile {
		fmt.Fprintf(&g.tests, "func %s(t *testing.T) {\n%s}\n\n", name, body)
	}
	rel := "adapters/" + pkgName + "/" + file
	g.cases = append(g.cases, testCase{Test: name, File: rel, Expect: expect, Blame: rel + ":" + name, Note: note})
}

// blameLast changes what the last case's refusal must name.
func (g *gen) blameLast(blame string) {
	g.cases[len(g.cases)-1].Blame = blame
}

// matchCase generates one test the evaluator must reproduce exactly.
func (g *gen) matchCase() {
	test := g.id("TestMatch")
	var b strings.Builder
	s := &scope{body: &b}
	run := func(bidder string) {
		fmt.Fprintf(&b, "\tadapterstest.RunJSONBidderTest(t, %s, %s)\n", g.dirExpr(), bidder)
	}
	form := g.r.Intn(8)
	note := ""
	switch form {
	case 0:
		note = "Builder, error checked with t.Fatalf"
		fmt.Fprintf(&b, "\tbidder, buildErr := Builder(%s, %s, %s)\n", g.bidderName(), g.adapterLit(s), g.serverLit(s))
		b.WriteString("\tif buildErr != nil {\n\t\tt.Fatalf(\"Builder returned unexpected error %v\", buildErr)\n\t}\n\n")
		run("bidder")
	case 1:
		note = "Builder, error discarded"
		fmt.Fprintf(&b, "\tbidder, _ := Builder(%s, %s, %s)\n", g.bidderName(), g.adapterLit(s), g.serverLit(s))
		run("bidder")
	case 2:
		note = "Builder in an if-init statement"
		fmt.Fprintf(&b, "\tif bidder, err := Builder(%s, %s, %s); err == nil {\n", g.bidderName(), g.adapterLit(s), g.serverLit(s))
		fmt.Fprintf(&b, "\t\tadapterstest.RunJSONBidderTest(t, %s, bidder)\n\t} else {\n\t\tt.Fatalf(\"x %%v\", err)\n\t}\n", g.dirExpr())
	case 3:
		note = "helper function with parameters"
		helper := g.id("newBidder")
		var h strings.Builder
		hs := &scope{body: &h, params: []string{"ep", "extra"}}
		build := fmt.Sprintf("\tbidder, err := Builder(%s, config.Adapter{Endpoint: %s, ExtraAdapterInfo: extra}, config.Server{GvlID: gvl, DataCenter: %s})\n",
			g.bidderName(), g.strExpr(1, hs), g.strExpr(1, hs))
		fmt.Fprintf(&g.decls, "func %s(t *testing.T, ep, extra string, gvl int) adapters.Bidder {\n%s%s", helper, h.String(), build)
		g.decls.WriteString("\tif err != nil {\n\t\tt.Fatalf(\"x %v\", err)\n\t}\n\treturn bidder\n}\n\n")
		if g.r.Intn(2) == 0 {
			fmt.Fprintf(&b, "\tbidder := %s(t, %s, %s, %s)\n", helper, g.strExpr(1, s), g.strExpr(1, s), g.intExpr())
			run("bidder")
		} else {
			run(fmt.Sprintf("%s(t, %s, %s, %s)", helper, g.strExpr(1, s), g.strExpr(1, s), g.intExpr()))
		}
	case 4:
		note = "config literals in locals"
		fmt.Fprintf(&b, "\tcfg := %s\n\tsrv := %s\n", g.adapterLit(s), g.serverLit(s))
		fmt.Fprintf(&b, "\tbidder, err := Builder(%s, cfg, srv)\n\tif err != nil {\n\t\tt.Fatalf(\"x %%v\", err)\n\t}\n", g.bidderName())
		run("bidder")
	case 5:
		note = "config literal in a package variable"
		cfg := g.id("cfg")
		fmt.Fprintf(&g.decls, "var %s = %s\n\n", cfg, g.adapterLit(&scope{}))
		fmt.Fprintf(&b, "\tbidder, _ := Builder(%s, %s, %s)\n", g.bidderName(), cfg, g.serverLit(s))
		run("bidder")
	case 6:
		note = "bidder rebuilt: the second Builder call is the one passed"
		fmt.Fprintf(&b, "\tbidder, err := Builder(%s, %s, config.Server{})\n", g.bidderName(), g.adapterLit(s))
		fmt.Fprintf(&b, "\tbidder, err = Builder(%s, %s, %s)\n\tif err != nil {\n\t\tt.Fatalf(\"x %%v\", err)\n\t}\n", g.bidderName(), g.adapterLit(s), g.serverLit(s))
		run("bidder")
	default:
		note = "struct literal"
		amp := g.pick("&", "")
		fmt.Fprintf(&b, "\tbidder := %sdirectAdapter{Endpoint: %s, Extra: %s}\n", amp, g.strExpr(0, s), g.strExpr(0, s))
		run("bidder")
	}
	g.addTest(test, b.String(), testFile, "match", note)
}

// loudCases are tests the evaluator cannot reproduce from the source alone; it must refuse them.
func (g *gen) loudCases() {
	adapter := "config.Adapter{Endpoint: ep}, config.Server{}"
	build := func(pre string) string {
		return pre + "\tbidder, _ := Builder(openrtb_ext.BidderIx, " + adapter + ")\n" +
			"\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n"
	}
	g.addTest("TestLoudPlusAssign", build("\tep := \"a\"\n\tep += \"b\"\n"), testFile, "loud", "+= changes a local")
	g.addTest("TestLoudCallValue", build("\tep := \"a\"\n\tep = strings.ToUpper(ep)\n"), testFile, "loud", "a value computed by a call")
	g.addTest("TestLoudNestedAssign", build("\tep := \"a\"\n\tif len(ep) > 0 {\n\t\tep = \"b\"\n\t}\n"), testFile, "loud", "assignment inside a nested block")
	g.decls.WriteString("const shadowed = \"package\"\n\n")
	g.addTest("TestLoudLocalConst", "\tconst shadowed = \"local\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{Endpoint: shadowed}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "a local const shadows the package one")
	g.addTest("TestLoudIfOther", "\tep := \"a\"\n\tif x := len(ep); x == 2 {\n\t\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\t\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\t}\n", testFile, "loud", "if whose condition is false at run time")
	g.addTest("TestLoudIfFalse", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\tif errNever != nil {\n\t\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\t}\n", testFile, "loud", "if without init")
	g.addTest("TestLoudIfInitNonNil", "\tep := \"a\"\n\tif bidder, err := Builder(openrtb_ext.BidderIx, "+adapter+"); err != nil {\n\t\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\t}\n", testFile, "loud", "if-init whose then-block runs on error")
	g.addTest("TestLoudReturn", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\tif ep != \"\" {\n\t\treturn\n\t}\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "return before the call")
	g.addTest("TestLoudBareReturn", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\treturn\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "unreachable call after return")
	g.addTest("TestLoudSkip", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\tt.Skip(\"skipped\")\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "t.Skip before the call")
	g.addTest("TestLoudSubtest", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\tt.Run(\"sub\", func(t *testing.T) {\n\t\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\t})\n", testFile, "loud", "call in a subtest closure")
	g.addTest("TestLoudLoop", "\tfor _, ep := range []string{\"a\", \"b\"} {\n\t\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\t\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\t}\n", testFile, "loud", "call in a loop")
	g.addTest("TestLoudTwice", "\tep := \"a\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, "+adapter+")\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n\tadapterstest.RunJSONBidderTest(t, \"other\", bidder)\n", testFile, "loud", "two calls in one test")
	g.decls.WriteString("func runShared(t *testing.T) {\n\tep := \"shared\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, " + adapter + ")\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n}\n\n")
	g.addTest("TestLoudHelperA", "\trunShared(t)\n", testFile, "loud", "the call lives in a helper used by two tests")
	g.blameLast("adapters/" + pkgName + "/" + testFile + ":runShared")
	g.addTest("TestLoudHelperB", "\trunShared(t)\n", testFile, "loud", "the call lives in a helper used by two tests")
	g.blameLast("adapters/" + pkgName + "/" + testFile + ":runShared")
	g.addTest("Testlowercase", build("\tep := \"a\"\n"), testFile, "loud", "go test does not run Testlowercase")
	g.decls.WriteString("var mutated = \"declared\"\n\n")
	g.addTest("TestLoudMutateFirst", "\tmutated = \"changed by an earlier test\"\n", testFile, "none", "assigns the package variable TestLoudMutateSecond reads; no run")
	g.addTest("TestLoudMutateSecond", "\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{Endpoint: mutated}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "reads a package variable another test assigned")
	g.decls.WriteString("var setInInit = \"declared\"\n\nfunc init() {\n\tsetInInit = \"init\"\n}\n\n")
	g.addTest("TestLoudInitVar", "\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{Endpoint: setInInit}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "reads a package variable init() assigns")
	g.addTest("TestLoudInvalidUTF8", build("\tep := \"\\xff\"\n"), testFile, "loud", "a string literal that is not UTF-8")
	g.addTest("TestLoudFieldAssign", "\tcfg := config.Adapter{Endpoint: \"a\"}\n\tcfg.Endpoint = \"b\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, cfg, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "a field assigned after the literal")
	g.addTest("TestLoudAliasReassign", "\td := directAdapter{Endpoint: \"a\"}\n\tp := &d\n\td = directAdapter{Endpoint: \"b\"}\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", p)\n", testFile, "loud", "a pointer sees a later reassignment of its target")
	g.addTest("TestLoudAliasRedeclare", "\td := directAdapter{Endpoint: \"a\"}\n\tp := &d\n\td, n := directAdapter{Endpoint: \"b\"}, 1\n\t_ = n\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", p)\n", testFile, "loud", "a := that reuses the pointer's target")
	g.decls.WriteString("var fieldCfg = config.Adapter{Endpoint: \"declared\"}\n\nfunc init() {\n\tfieldCfg.Endpoint = \"init\"\n}\n\n")
	g.addTest("TestLoudInitField", "\tbidder, _ := Builder(openrtb_ext.BidderIx, fieldCfg, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "reads a package variable whose field init() assigns")
	g.decls.WriteString("var viaPtr = \"declared\"\n\nfunc setPtr(p *string) {\n\t*p = \"set\"\n}\n\nfunc init() {\n\tsetPtr(&viaPtr)\n}\n\n")
	g.addTest("TestLoudInitPointer", "\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{Endpoint: viaPtr}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "reads a package variable init() changes through a pointer")
	g.decls.WriteString("var pkgFlag = \"declared\"\n\n")
	g.addTest("TestLoudAssignPkgVar", "\tpkgFlag = \"test\"\n\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n", testFile, "loud", "assigns a package variable the adapter may read")

	g.extraFile("TestLoudMethod", pkgName+"_method_test.go", "type suite struct{}\n\nfunc (suite) run(t *testing.T) {\n\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n}\n\nfunc TestLoudMethod(t *testing.T) {\n\tsuite{}.run(t)\n}\n", "call in a method")
	g.extraFile("TestLoudFuncVar", pkgName+"_funcvar_test.go", "var runIt = func(t *testing.T) {\n\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n}\n\nfunc TestLoudFuncVar(t *testing.T) {\n\trunIt(t)\n}\n", "call in a package-level function literal")
	g.extraFile("TestLoudIgnored", pkgName+"_ignored_test.go", "func TestLoudIgnored(t *testing.T) {\n\tbidder, _ := Builder(openrtb_ext.BidderIx, config.Adapter{}, config.Server{})\n\tadapterstest.RunJSONBidderTest(t, \"zzsynthtest\", bidder)\n}\n", "file excluded by //go:build ignore")
}

func (g *gen) extraFile(test, name, body, note string) {
	header := ""
	if strings.Contains(name, "ignored") {
		header = "//go:build ignore\n\n"
	}
	src := header + "package " + pkgName + "\n\nimport (\n\t\"testing\"\n\n" +
		"\t\"" + module + "/" + genRel + "/" + tmpName + "/" + fakeName + "\"\n" +
		"\t\"github.com/prebid/prebid-server/v4/config\"\n" +
		"\t\"github.com/prebid/prebid-server/v4/openrtb_ext\"\n)\n\n" + body
	g.extra = append(g.extra, sourceFile{Name: name, Src: src})
	g.addTest(test, "", name, "loud", note)
	g.blameLast("adapters/" + pkgName + "/" + name)
}
