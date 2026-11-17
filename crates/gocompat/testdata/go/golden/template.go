package main

import (
	"errors"
	"strings"
	"text/template"
	"text/template/parse"

	"github.com/prebid/prebid-server/v4/macros"
)

type tinner struct {
	ID    string
	Level int
}

type tdata struct {
	Name    string
	Count   int
	Price   float64
	OK      bool
	Tags    []string
	Empty   []string
	Attrs   map[string]string
	Nums    map[string]int
	IntKeys map[int]string
	Inner   tinner
	Ptr     *tinner
	NilPtr  *tinner
	Any     any
	Nil     any
	Err     error
	Bidder  bidderName
	S       stringer
	Raw     []byte
	Arr     [2]int
	N       int
	U8      uint8
	hidden  string
}

// newTdata is the struct the Rust tests build as tdata(); fuzz runs use it without Ptr, whose
// address would otherwise print.
func newTdata(withPtr bool) tdata {
	d := tdata{
		Name:    "x",
		Count:   3,
		Price:   1.5,
		OK:      true,
		Tags:    []string{"a", "b", "c"},
		Attrs:   map[string]string{"k": "v", "j": "w"},
		Nums:    map[string]int{"b": 2, "a": 1, "c": 3},
		IntKeys: map[int]string{1: "one"},
		Inner:   tinner{ID: "i", Level: 1},
		Any:     tinner{ID: "any", Level: 5},
		Err:     errors.New("boom"),
		Bidder:  "appnexus",
		S:       stringer{id: 7},
		Raw:     []byte("hi"),
		Arr:     [2]int{4, 5},
		N:       3,
		U8:      2,
		hidden:  "h",
	}
	if withPtr {
		d.Ptr = &tinner{ID: "p", Level: 2}
	}
	return d
}

// templateData is the data catalog the Rust tests rebuild by key.
func templateData() map[string]any {
	return map[string]any{
		"privacy": macros.UserSyncPrivacy{
			GDPR:        "1",
			GDPRConsent: "BOPVK28OVJoTBABABAENBs-AAAAhuAKAANAAoACwAGgAPAAQAAgAEAAoACA",
			USPrivacy:   "1YNN",
			GPP:         "DBACNYA~CPXxRfAPXxRfAAfKABENB-CgAAAAAAAAAAYgAAAAAAAA~1NYN",
			GPPSID:      "2",
		},
		"endpoint":    endpointParams(),
		"endpointPtr": func() *macros.EndpointTemplateParams { p := endpointParams(); return &p }(),
		"nilEndpoint": (*macros.EndpointTemplateParams)(nil),
		"nil":         nil,
		"data":        newTdata(true),
		"string":      "hello",
		"map":         map[string]any{"a": 1, "b": "two", "n": nil, "m": map[string]int{"z": 26}},
		"slice":       []any{"a", 1, nil},
	}
}

func endpointParams() macros.EndpointTemplateParams {
	return macros.EndpointTemplateParams{
		Host:        "ads.example.com",
		PublisherID: "pub-1",
		ZoneID:      "zone-9",
		SourceId:    "src",
		AccountID:   "acc-7",
		Region:      "us-east",
	}
}

type templateCase struct{ name, data, text string }

const syncURL = "https://sync.example.com/getuid?gdpr={{.GDPR}}&gdpr_consent={{.GDPRConsent}}" +
	"&us_privacy={{.USPrivacy}}&gpp={{.GPP}}&gpp_sid={{.GPPSID}}&redirect=http%3A%2F%2Flocalhost" +
	"%3A8000%2Fsetuid%3Fbidder%3Dadnxs%26gdpr%3D{{.GDPR}}%26gdpr_consent%3D{{.GDPRConsent}}%26uid%3D%24UID"

var templateCases = []templateCase{
	// Prebid's own templates.
	{"adnxs_usersync_url", "privacy", syncURL},
	{"adnxs_usersync_url", "privacy", "https://sync.example.com/{{.RedirectURL}}"},
	{"adnxs_usersync_url", "privacy", "{{.}}"},
	{"endpointTemplate", "endpoint", "http://{{.Host}}/bid?pub={{.PublisherID}}&zone={{.ZoneID}}"},
	{"endpointTemplate", "endpoint", "https://{{.Region}}.example.com/{{.AccountID}}/{{.SourceId}}"},
	{"endpointTemplate", "endpoint", "{{.}}"},
	{"endpointTemplate", "endpoint", "https://{{.Nope}}.example.com"},
	{"endpointTemplate", "endpoint", "https://example.com/{{.host}}"},
	{"endpointTemplate", "endpointPtr", "{{.Host}}|{{.}}"},
	{"endpointTemplate", "endpointPtr", "{{.Nope}}"},
	{"endpointTemplate", "nilEndpoint", "{{.Host}}"},
	{"endpointTemplate", "nil", "a{{.Host}}b{{.X.Y}}c"},
	{"endpointTemplate", "endpoint", "{{Malformed}}"},
	{"endpointTemplate", "endpoint", "{{.Host]}}"},
	{"endpointTemplate", "endpoint", "http://{{.Host"},
	{"endpointTemplate", "endpoint", "http://{{.Host}"},
	{"endpointTemplate", "endpoint", "{{"},
	{"endpointTemplate", "endpoint", "}}{{.Host}}}}"},
	{"", "endpoint", "{{.Nope}}"},
	{"", "endpoint", "{{Nope}}"},
	{"a%d", "endpoint", "{{Nope}}"},
	{"a%b", "endpoint", "{{.Nope}}"},
	{"endpointTemplate", "endpoint", "{{.Ho%dst}}"},
	{"endpointTemplate", "endpoint", "line1\nline2 {{.Host}} {{.Nope}}"},
	{"endpointTemplate", "endpoint", "a\nb\n{{.Host ]}}"},
	{"endpointTemplate", "endpoint", "{{.Host\n]}}"},
	{"endpointTemplate", "endpoint", "{{.Host\n"},
	{"endpointTemplate", "endpoint", "{{/* unclosed"},
	{"endpointTemplate", "endpoint", "{{/* c */ .Host}}"},
	{"endpointTemplate", "endpoint", "a {{/* c */}} b {{- /* d */ -}} c"},

	// Parse errors.
	{"t", "data", "{{end}}"},
	{"t", "data", "{{else}}"},
	{"t", "data", "{{if .OK}}"},
	{"t", "data", "{{if .OK}}a{{else}}b{{else}}c{{end}}"},
	{"t", "data", "{{if}}{{end}}"},
	{"t", "data", "{{range}}{{end}}"},
	{"t", "data", "{{$x}}"},
	{"t", "data", "{{.Name | }}"},
	{"t", "data", "{{| .Name}}"},
	{"t", "data", "{{.Name | \"a\"}}"},
	{"t", "data", "{{\"unterminated}}"},
	{"t", "data", "{{`unterminated}}"},
	{"t", "data", "{{'ab'}}"},
	{"t", "data", "{{'a}}"},
	{"t", "data", "{{'\\z'}}"},
	{"t", "data", "{{\"\\z\"}}"},
	{"t", "data", "{{1x}}"},
	{"t", "data", "{{1+2}}"},
	{"t", "data", "{{99999999999999999999}}"},
	{"t", "data", "{{0x}}"},
	{"t", "data", "{{.Name.}}"},
	{"t", "data", "{{(.Name}}"},
	{"t", "data", "{{.Name)}}"},
	{"t", "data", "{{()}}"},
	{"t", "data", "{{.Name :}}"},
	{"t", "data", "{{.Name := 1}}"},
	{"t", "data", "{{$x := 1 := 2}}"},
	{"t", "data", "{{$x, $y := 1}}"},
	{"t", "data", "{{range $i, $e, $f := .Tags}}{{end}}"},
	{"t", "data", "{{range $i, 1}}{{end}}"},
	{"t", "data", "{{define \"a\"}}x{{end}}{{define \"a\"}}y{{end}}"},
	{"t", "data", "{{define \"a\"}}x{{end}}{{define \"a\"}} {{end}}{{template \"a\"}}"},
	{"t", "data", "{{define a}}x{{end}}"},
	{"t", "data", "{{define \"a\" .}}x{{end}}"},
	{"t", "data", "{{define \"a\"}}x"},
	{"t", "data", "{{define \"a\"}}x{{else}}y{{end}}"},
	{"t", "data", "{{template .Name}}"},
	{"t", "data", "{{template \"a\" | }}"},
	{"t", "data", "{{block}}"},
	{"t", "data", "{{block \"b\"}}{{end}}"},
	{"t", "data", "{{block \"b\" .}}x{{else}}{{end}}"},
	{"t", "data", "{{with $x := 1}}{{end}}{{$x}}"},
	{"t", "data", "{{break}}"},
	{"t", "data", "{{continue}}"},
	{"t", "data", "{{range .Tags}}{{break 1}}{{end}}"},
	{"t", "data", "{{if .OK}}{{break}}{{end}}"},
	{"t", "data", "{{.Name 1 | 2}}"},
	{"t", "data", "{{true.X}}"},
	{"t", "data", "{{\"s\".X}}"},
	{"t", "data", "{{nil.X}}"},
	{"t", "data", "{{..}}"},
	{"t", "data", "{{.Name#}}"},
	{"t", "data", "{{\x01}}"},
	{"t", "data", "{{é}}"},
	{"t", "data", "{{$x := .Name}}{{$x.}}"},
	{"t", "data", "{{.Name}}{{else if}}"},
	{"t", "data", "{{if .OK}}a{{else if}}{{end}}"},
	{"t", "data", "{{if .OK}}a{{else with .Name}}b{{end}}"},
	{"t", "data", "{{else if .OK}}"},
	{"t", "data", "{{with .Name}}a{{else if .OK}}b{{end}}"},
	{"t", "data", "{{-}}"},
	{"t", "data", "{{ - }}"},
	{"t", "data", "{{.Name -}"},
	{"t", "data", "{{/* a */ b}}"},
	{"t", "data", "{{ /* a */}}"},
	{"t", "data", "{{1 | 2}}"},
	{"t", "data", "{{.Name | nil}}"},
	{"t", "data", "{{.Name | .}}"},
	{"t", "data", "{{.Name | true}}"},

	// Execution.
	{"t", "data", "{{.Name}}|{{.Count}}|{{.Price}}|{{.OK}}|{{.U8}}|{{.N}}"},
	{"t", "data", "{{.Tags}}|{{.Empty}}|{{.Attrs}}|{{.Nums}}|{{.IntKeys}}"},
	{"t", "data", "{{.Inner}}|{{.Ptr}}|{{.NilPtr}}|{{.Any}}|{{.Nil}}|{{.Err}}"},
	{"t", "data", "{{.Bidder}}|{{.S}}|{{.Raw}}|{{.Arr}}"},
	{"t", "data", "{{.Inner.ID}}|{{.Inner.Level}}|{{.Ptr.ID}}|{{.Any.ID}}"},
	{"t", "data", "{{.Attrs.k}}|{{.Attrs.missing}}|{{.Attrs.missing.deeper}}"},
	{"t", "data", "{{.Err.Error}}|{{.S.String}}"},
	{"t", "data", "{{.Err.Error 1}}"},
	{"t", "data", "{{.Name | .Err.Error}}"},
	{"t", "data", "{{.Name 1}}"},
	{"t", "data", "{{.Name | .Count}}"},
	{"t", "data", "{{.Attrs.k 1}}"},
	{"t", "data", "{{.Inner .Name}}"},
	{"t", "data", "{{.hidden}}"},
	{"t", "data", "{{.Inner.hidden}}"},
	{"t", "data", "{{.Nil.X}}"},
	{"t", "data", "{{.NilPtr.ID}}"},
	{"t", "data", "{{.Ptr.Nope}}"},
	{"t", "data", "{{.Count.X}}"},
	{"t", "data", "{{.Tags.X}}"},
	{"t", "data", "{{.Inner.ID.X}}"},
	{"t", "data", "{{.Bidder.X}}"},
	{"t", "data", "{{.IntKeys.x}}"},
	{"t", "data", "{{.Raw.X}}"},
	{"t", "data", "{{.Arr.X}}"},
	{"t", "data", "{{nil}}"},
	{"t", "data", "{{1 2}}"},
	{"t", "data", "{{\"a\" | .Name}}"},
	{"t", "data", "{{$x := 1}}{{$x 2}}"},
	{"t", "data", "{{(.Name) 2}}"},
	{"t", "data", "{{. 1}}"},
	{"t", "data", "{{true}}|{{false}}|{{\"str\"}}|{{`raw`}}|{{'a'}}|{{'\\n'}}"},
	{"t", "data", "{{1.5}}|{{1e3}}|{{0x10}}|{{-0}}|{{0x1p4}}|{{1_000}}|{{0o17}}|{{0b101}}|{{-7}}|{{.5}}"},
	{"t", "data", "{{18446744073709551615}}"},
	{"t", "data", "{{9223372036854775807}}|{{-9223372036854775808}}"},
	{"t", "data", "{{1e100}}|{{0x1.8p1}}|{{'\\u00e9'}}"},
	{"t", "data", "{{(.Inner).ID}}|{{(.Ptr).Level}}|{{(.Attrs).k}}"},
	{"t", "data", "{{(.Nope).X}}"},
	{"t", "data", "{{(.NilPtr).ID}}"},
	{"t", "data", "{{if .OK}}yes{{else}}no{{end}}"},
	{"t", "data", "{{if .Count}}c{{end}}{{if .Empty}}e{{end}}{{if .Nil}}n{{else if .Price}}p{{end}}"},
	{"t", "data", "{{if .Attrs}}m{{end}}{{if .NilPtr}}p{{else}}np{{end}}{{if .Inner}}s{{end}}{{if .Err}}e{{end}}"},
	{"t", "data", "{{if 0}}a{{else if 0.0}}b{{else if \"\"}}c{{else}}d{{end}}"},
	{"t", "data", "{{if .Nope}}a{{end}}"},
	{"t", "data", "{{with .Inner}}{{.ID}}{{end}}|{{with .Empty}}x{{else}}y{{end}}"},
	{"t", "data", "{{with .Nil}}x{{else with .Name}}{{.}}{{end}}"},
	{"t", "data", "{{with $x := .Inner}}{{$x.ID}}{{.Level}}{{end}}"},
	{"t", "data", "{{range .Tags}}[{{.}}]{{end}}"},
	{"t", "data", "{{range $i, $t := .Tags}}{{$i}}={{$t}};{{end}}"},
	{"t", "data", "{{range $t := .Tags}}{{$t}}{{end}}"},
	{"t", "data", "{{range $k, $v := .Nums}}{{$k}}={{$v}};{{end}}"},
	{"t", "data", "{{range .Attrs}}{{.}}{{end}}"},
	{"t", "data", "{{range .Empty}}x{{else}}empty{{end}}"},
	{"t", "data", "{{range .Nope}}x{{else}}none{{end}}"},
	{"t", "data", "{{range .N}}{{.}}{{end}}|{{range 0}}x{{else}}zero{{end}}|{{range .U8}}{{.}}{{end}}"},
	{"t", "data", "{{range $i, $e := .N}}{{end}}"},
	{"t", "data", "{{range .Name}}{{end}}"},
	{"t", "data", "{{range .Inner}}{{end}}"},
	{"t", "data", "{{range .NilPtr}}{{end}}"},
	{"t", "data", "{{range .Price}}{{end}}"},
	{"t", "data", "{{range .Raw}}{{.}},{{end}}|{{range .Arr}}{{.}}{{end}}"},
	{"t", "data", "{{range .Tags}}{{.}}{{break}}{{end}}|{{range $i, $t := .Tags}}{{if $i}}{{continue}}{{end}}{{$t}}{{end}}"},
	{"t", "data", "{{range .Tags}}{{range $.Tags}}{{break}}{{end}}{{.}}{{end}}"},
	{"t", "data", "{{$x := 0}}{{range $x = .Tags}}{{end}}{{$x}}"},
	{"t", "data", "{{$i := 0}}{{$e := 0}}{{range $i, $e = .Tags}}{{end}}{{$i}}{{$e}}"},
	{"t", "data", "{{range $i, $e := .Tags}}{{$e}}{{end}}{{$i}}"},
	{"t", "data", "{{$x := .Name}}{{$x}}|{{$x := 1}}{{$x = 2}}{{$x}}"},
	{"t", "data", "{{$x := .Inner}}{{$x.Level}}|{{$.Name}}|{{$x.Nope}}"},
	{"t", "data", "{{$x := .Nil}}{{$x.Field}}|{{$x}}"},
	{"t", "data", "{{if $x := .Name}}{{$x}}{{end}}"},
	{"t", "data", "{{define \"a\"}}[{{.}}]{{end}}{{template \"a\" .Name}}{{template \"a\"}}"},
	{"t", "data", "{{template \"missing\"}}"},
	{"t", "data", "{{block \"b\" .Count}}<{{.}}>{{end}}"},
	{"t", "data", "{{define \"e\"}}\n  {{.Nope}}{{end}}{{template \"e\" .}}"},
	{"t", "data", "{{define \"r\"}}{{template \"r\" .}}{{end}}{{template \"r\" .}}"},
	{"t", "data", "{{define \"v\"}}{{$}}{{end}}{{$x := 1}}{{template \"v\" .Count}}"},
	{"t", "data", "{{define \"t\"}}defined{{end}}"},
	{"t", "data", "{{define \"t\"}}defined{{end}}top"},
	{"t", "data", "a  {{- .Name -}}  b|{{.Name -}}\n\n z|{{- /* c */ -}}  y|a {{-3}}"},
	{"t", "data", "a\n\t{{- .Name}}\n{{.Count -}}\n\tb"},
	{"t", "string", "{{.}}|{{.X}}"},
	{"t", "string", "{{range .}}{{end}}"},
	{"t", "map", "{{.a}}|{{.b}}|{{.n}}|{{.m.z}}|{{.missing}}|{{.m.missing}}"},
	{"t", "map", "{{.n.x}}"},
	{"t", "map", "{{range $k, $v := .}}{{$k}}={{$v}} {{end}}"},
	{"t", "map", "{{.}}"},
	{"t", "slice", "{{range .}}[{{.}}]{{end}}|{{.}}"},
	{"t", "slice", "{{.X}}"},
	{"t", "nil", "{{.}}|{{if .}}a{{else}}b{{end}}|{{range .}}x{{else}}y{{end}}|{{with .}}w{{end}}"},
	{"t", "nil", "{{$}}|{{$.X}}|{{.X.Y.Z}}"},

	// Builtins and complex constants parse; this port does not execute them.
	{"t", "data", "{{len .Tags}}"},
	{"t", "data", "{{printf \"%d\" .Count}}"},
	{"t", "data", "{{if eq .Name \"x\"}}y{{end}}"},
	{"t", "data", "{{.Name | urlquery}}"},
	{"t", "data", "{{1+2i}}"},
	{"t", "data", "{{index .Tags 1}}"},
	{"t", "data", "{{(len .Tags).X}}"},
	{"t", "data", "{{and}}{{or}}{{not}}{{html}}{{js}}{{call}}{{slice}}{{print}}{{println}}{{ge}}{{gt}}{{le}}{{lt}}{{ne}}"},
}

// unsupportedNode reports whether a tree uses a builtin function or a complex constant.
func unsupportedNode(n parse.Node) bool {
	switch n := n.(type) {
	case *parse.ListNode:
		if n == nil {
			return false
		}
		for _, c := range n.Nodes {
			if unsupportedNode(c) {
				return true
			}
		}
	case *parse.ActionNode:
		return unsupportedNode(n.Pipe)
	case *parse.IfNode:
		return unsupportedBranch(&n.BranchNode)
	case *parse.RangeNode:
		return unsupportedBranch(&n.BranchNode)
	case *parse.WithNode:
		return unsupportedBranch(&n.BranchNode)
	case *parse.TemplateNode:
		return n.Pipe != nil && unsupportedNode(n.Pipe)
	case *parse.PipeNode:
		if n == nil {
			return false
		}
		for _, c := range n.Cmds {
			for _, a := range c.Args {
				if unsupportedNode(a) {
					return true
				}
			}
		}
	case *parse.ChainNode:
		return unsupportedNode(n.Node)
	case *parse.IdentifierNode:
		return true
	case *parse.NumberNode:
		return n.IsComplex
	}
	return false
}

func unsupportedBranch(b *parse.BranchNode) bool {
	return unsupportedNode(b.Pipe) || unsupportedNode(b.List) || unsupportedNode(b.ElseList)
}

// runTemplate parses and executes one template: "parse: ...", "unsupported", "exec: [name] ..."
// or "ok: " + output.
func runTemplate(name, text string, data any) string {
	t, err := template.New(name).Parse(text)
	if err != nil {
		return "parse: " + err.Error()
	}
	for _, tt := range t.Templates() {
		if tt.Tree != nil && unsupportedNode(tt.Root) {
			return "unsupported"
		}
	}
	var b strings.Builder
	if err := t.Execute(&b, data); err != nil {
		var execErr template.ExecError
		if errors.As(err, &execErr) {
			return "exec: [" + execErr.Name + "] " + err.Error()
		}
		return "error: " + err.Error()
	}
	return "ok: " + b.String()
}

var fuzzPieces = []string{
	"{{", "}}", "{{- ", " -}}", " ", "\n", "text", ".", ".Name", ".Inner.ID", ".Tags", ".Attrs.k",
	".Nope", ".Count", ".N", ".Nil", ".U8", ".Price", ".Err.Error", ".S", ".Arr", ".Raw",
	".hidden", ".Bidder", ".Nums", ".Empty", "$", "$x", "$y", "$x :=", "$x =", "$i, $e :=", ":=",
	"=", "|", "(", ")", ",", "if", "else", "end", "range", "with", "break", "continue",
	"define \"d\"", "block \"b\" .", "template \"d\"", "template \"b\" .", "\"s\"", "`raw`",
	"'a'", "1", "-2", "0x1F", "1.5", "1e3", "nil", "true", "false", "/*c*/", "{{/* c */}}",
	"len", "foo", "]", "#", "\"open", "'", "`", "{{.Name}}", "{{end}}", "{{else}}", "{{if .OK}}",
	"{{range .Tags}}", "{{with .Inner}}", "{{$x := .Count}}", "{{$x}}", "{{.}}", "%", "é",
	"{{range .N}}", "{{.Name | .Nope}}",
}

// genTemplate writes template_cases.txt, template_fuzz.txt (the first fuzz rows in full) and
// the template_fuzz checksum in template_sweeps.txt.
func genTemplate() {
	data := templateData()
	var cases table
	for _, c := range templateCases {
		cases.row(c.name, c.data, c.text, runTemplate(c.name, c.text, data[c.data]))
	}
	cases.write("template_cases.txt")

	fuzzData := newTdata(false)
	rng := splitmix64{5}
	var rows table
	sw := newSweep()
	for i := 0; i < 30000; i++ {
		var b strings.Builder
		n := 1 + rng.next()%10
		for j := uint64(0); j < n; j++ {
			b.WriteString(fuzzPieces[rng.next()%uint64(len(fuzzPieces))])
		}
		text := b.String()
		result := runTemplate("fz", text, fuzzData)
		if i < 3000 {
			rows.row(text, result)
		}
		sw.add(text + "\t" + result)
	}
	rows.write("template_fuzz.txt")
	var sums table
	sums.row("template_fuzz", sw.sum())
	sums.write("template_sweeps.txt")
}
