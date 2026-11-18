// Command biddernames dumps the openrtb_ext bidder name tables to testdata/go_bidder_names.json.
//
// It records every BidderName constant declared in openrtb_ext (parsed from the source, since Go
// cannot enumerate constants at run time), CoreBidderNames() before and after the bidder-info
// aliases are registered, the reserved-name predicates, and the NormalizeBidderName lookup table.
//
// Run it through `pbs-devtools go-dump bidder-names`.
package main

import (
	"go/ast"
	"go/build"
	"go/parser"
	"go/token"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	jsoniter "github.com/json-iterator/go"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"

	"github.com/aryehlev/prebid-server-rust/crates/devtools/go/internal/dump"
)

func init() {
	// Same as main.go, so the process behaves like the server binary.
	jsoniter.RegisterExtension(&jsonutil.RawMessageExtension{})
}

type constant struct {
	Name              string `json:"name"`
	Value             string `json:"value"`
	File              string `json:"file"`
	InCoreBidderNames bool   `json:"in_core_bidder_names"`
	IsReserved        bool   `json:"is_bidder_name_reserved"`
	IsPotentialBidder bool   `json:"is_potential_bidder"`
}

type probe struct {
	Input             string `json:"input"`
	IsReserved        bool   `json:"is_bidder_name_reserved"`
	IsPotentialBidder bool   `json:"is_potential_bidder"`
	Normalized        string `json:"normalized"`
	NormalizedFound   bool   `json:"normalized_found"`
}

type output struct {
	Generator            string            `json:"generator"`
	GoCommit             string            `json:"go_commit"`
	Notes                []string          `json:"notes"`
	Constants            []constant        `json:"constants"`
	CoreBidderNames      []string          `json:"core_bidder_names"`
	NormalizeStatic      map[string]string `json:"normalize_static"`
	Aliases              map[string]string `json:"aliases"`
	AliasNamesAppended   []string          `json:"alias_names_appended_sorted"`
	NormalizeWithAliases map[string]string `json:"normalize_with_aliases"`
	Probes               []probe           `json:"probes"`
}

func main() {
	opts := dump.ParseFlags()
	dump.ClearPBSEnv()

	consts := parseBidderNameConstants(filepath.Join(opts.Root, "openrtb_ext"), opts.Root)

	core := names(openrtb_ext.CoreBidderNames())
	inCore := make(map[string]bool, len(core))
	for _, n := range core {
		inCore[n] = true
	}
	for i := range consts {
		c := &consts[i]
		c.InCoreBidderNames = inCore[c.Value]
		c.IsReserved = openrtb_ext.IsBidderNameReserved(c.Value)
		c.IsPotentialBidder = openrtb_ext.IsPotentialBidder(c.Value)
	}

	normalizeStatic := normalizeTable(core)

	// Loading the bidder info registers every aliasOf file through SetAliasBidderName, which
	// appends to CoreBidderNames in map iteration order and extends the lookup table.
	infoPath := filepath.Join(opts.Root, "static", "bidder-info")
	if _, err := config.LoadBidderInfoFromDisk(infoPath); err != nil {
		dump.Fatalf("LoadBidderInfoFromDisk(%s): %v", infoPath, err)
	}
	after := names(openrtb_ext.CoreBidderNames())
	if len(after) < len(core) {
		dump.Fatalf("CoreBidderNames shrank after loading bidder info")
	}
	for i, n := range core {
		if after[i] != n {
			dump.Fatalf("CoreBidderNames prefix changed after loading bidder info at %d: %s != %s", i, after[i], n)
		}
	}
	appended := append([]string(nil), after[len(core):]...)
	sort.Strings(appended)

	aliases := make(map[string]string)
	for alias, parent := range openrtb_ext.GetAliasBidderToParent() {
		aliases[string(alias)] = string(parent)
	}

	out := output{
		Generator: "pbs-devtools go-dump bidder-names (crates/devtools/go/biddernames)",
		GoCommit:  opts.GoCommit,
		Notes: []string{
			"constants: every BidderName const in openrtb_ext, in source order.",
			"core_bidder_names: openrtb_ext.CoreBidderNames() before any alias is registered, in Go order.",
			"alias_names_appended_sorted: names config.LoadBidderInfoFromDisk appends to CoreBidderNames; Go appends them in random map order, sorted here.",
			"normalize_*: NormalizeBidderName(name) == table[strings.ToLower(name)]; a missing key means (\"\", false).",
		},
		Constants:            consts,
		CoreBidderNames:      core,
		NormalizeStatic:      normalizeStatic,
		Aliases:              aliases,
		AliasNamesAppended:   appended,
		NormalizeWithAliases: normalizeTable(after),
		Probes:               probes(consts),
	}
	dump.WriteJSON(opts.Out, out)
}

func names(in []openrtb_ext.BidderName) []string {
	out := make([]string, len(in))
	for i, n := range in {
		out[i] = string(n)
	}
	return out
}

// normalizeTable reproduces the unexported bidderNameLookup through the public API.
func normalizeTable(all []string) map[string]string {
	table := make(map[string]string, len(all))
	for _, n := range all {
		lower := strings.ToLower(n)
		got, ok := openrtb_ext.NormalizeBidderName(lower)
		if !ok {
			dump.Fatalf("NormalizeBidderName(%q) not found", lower)
		}
		table[lower] = string(got)
	}
	return table
}

func probes(consts []constant) []probe {
	seen := map[string]bool{}
	var inputs []string
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			inputs = append(inputs, s)
		}
	}
	for _, c := range consts {
		if c.Value != "" && (c.IsReserved || !c.IsPotentialBidder) {
			add(c.Value)
			add(strings.ToUpper(c.Value))
			add(strings.ToUpper(c.Value[:1]) + c.Value[1:])
		}
	}
	for _, s := range []string{"AppNexus", "APPNEXUS", "appnexus", "MSFT", "", "unknownbidder", "Prebid", "PREBID"} {
		add(s)
	}
	out := make([]probe, 0, len(inputs))
	for _, in := range inputs {
		n, ok := openrtb_ext.NormalizeBidderName(in)
		out = append(out, probe{
			Input:             in,
			IsReserved:        openrtb_ext.IsBidderNameReserved(in),
			IsPotentialBidder: openrtb_ext.IsPotentialBidder(in),
			Normalized:        string(n),
			NormalizedFound:   ok,
		})
	}
	return out
}

// parseBidderNameConstants returns every constant of type BidderName in the package directory, in
// source order (files sorted by name, declarations in order).
func parseBidderNameConstants(dir, root string) []constant {
	pkg, err := build.Default.ImportDir(dir, 0)
	if err != nil {
		dump.Fatalf("ImportDir(%s): %v", dir, err)
	}
	files := append(append([]string(nil), pkg.GoFiles...), pkg.CgoFiles...)
	sort.Strings(files)
	fset := token.NewFileSet()
	var out []constant
	for _, name := range files {
		path := filepath.Join(dir, name)
		f, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			dump.Fatalf("parsing %s: %v", path, err)
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			dump.Fatalf("relativising %s: %v", path, err)
		}
		for _, decl := range f.Decls {
			gen, ok := decl.(*ast.GenDecl)
			if !ok || gen.Tok != token.CONST {
				continue
			}
			var blockType ast.Expr
			for _, spec := range gen.Specs {
				vs, ok := spec.(*ast.ValueSpec)
				if !ok {
					continue
				}
				if vs.Type != nil || len(vs.Values) > 0 {
					blockType = vs.Type
				}
				ident, ok := blockType.(*ast.Ident)
				if !ok || ident.Name != "BidderName" {
					continue
				}
				if len(vs.Values) != len(vs.Names) {
					dump.Fatalf("%s: BidderName constant %s without an explicit value", rel, vs.Names[0].Name)
				}
				for i, n := range vs.Names {
					lit, ok := vs.Values[i].(*ast.BasicLit)
					if !ok || lit.Kind != token.STRING {
						dump.Fatalf("%s: BidderName constant %s is not a string literal", rel, n.Name)
					}
					value, err := strconv.Unquote(lit.Value)
					if err != nil {
						dump.Fatalf("%s: unquoting %s: %v", rel, lit.Value, err)
					}
					out = append(out, constant{
						Name:  n.Name,
						Value: value,
						File:  filepath.ToSlash(rel),
					})
				}
			}
		}
	}
	if len(out) == 0 {
		dump.Fatalf("no BidderName constants found in %s", dir)
	}
	return out
}
