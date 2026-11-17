// Command structmeta dumps Go type metadata to testdata/go_struct_meta.json.
//
// For every named type in the target packages it records, through reflect, the Go "pkg.Type" name
// (reflect.Type.String(), the spelling jsoniter and encoding/json use in error messages), the kind,
// and for structs every field in declaration order with its Go name, json tag (name, omitempty,
// string, omitzero, "-"), full tag, Go type string, kind and embedded flag. Named non-struct types
// get their underlying type, and every exported typed constant (the adcom1 enums, BidderName, ...)
// gets its value.
//
// Go cannot list a package's types at run time, so the tool runs in two steps, from the directory
// of this module (crates/devtools/go):
//
//	go run ./structmeta -plan DIR      # writes DIR/overlay.json
//	go run -overlay DIR/overlay.json ./structmeta -out FILE -go-commit SHA
//
// The plan step parses each target package and writes one overlay file per package (and one for
// this command) that registers its declarations with package reg; the overlay compiles those files
// into the real packages, so even unexported adapter types are reflected. `pbs-devtools go-dump
// struct-meta` runs both steps, with a go.work.sum that holds the upstream go.sum, because the
// adapter packages bring dependencies this module's own go.sum does not list.
package main

import (
	"bytes"
	"encoding"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/build"
	"go/parser"
	"go/token"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"unicode"

	"github.com/aryehlev/prebid-server-rust/crates/devtools/go/internal/dump"
	"github.com/aryehlev/prebid-server-rust/crates/devtools/go/structmeta/reg"
)

const (
	modulePath  = dump.UpstreamModule
	selfPath    = "github.com/aryehlev/prebid-server-rust/crates/devtools/go/structmeta"
	regPath     = selfPath + "/reg"
	overlayName = "zz_pbs_devtools_structmeta.go"
)

// externalPackages are dependency-module packages; only their exported declarations are reachable.
var externalPackages = []string{
	"github.com/prebid/openrtb/v20/adcom1",
	"github.com/prebid/openrtb/v20/native1",
	"github.com/prebid/openrtb/v20/native1/request",
	"github.com/prebid/openrtb/v20/native1/response",
	"github.com/prebid/openrtb/v20/openrtb2",
	"github.com/prebid/openrtb/v20/openrtb3",
}

// modulePackages are directories of the upstream module (relative to its root). "adapters/..."
// expands to the adapters package and every package below it.
var modulePackages = []string{
	"adapters/...",
	"config",
	"openrtb_ext",
}

// expectedPackages is filled in by the overlay; empty means the tool was run without -overlay.
var expectedPackages []string

func main() {
	if hasFlag("-plan") {
		plan := flag.String("plan", "", "write overlay files and DIR/overlay.json, then exit")
		root := flag.String("plan-root", "", "upstream checkout for -plan (default: the one this program builds against)")
		flag.Parse()
		runPlan(*plan, dump.UpstreamDir(*root))
		return
	}
	opts := dump.ParseFlags()
	runDump(opts)
}

func hasFlag(name string) bool {
	for _, a := range os.Args[1:] {
		if a == name || strings.HasPrefix(a, name+"=") {
			return true
		}
	}
	return false
}

// ---------------------------------------------------------------------------------------------
// Plan

type typeDecl struct {
	name    string
	alias   bool
	generic bool
}

type constDecl struct {
	name  string
	order int
}

type pkgDecls struct {
	path   string
	dir    string
	name   string
	types  []typeDecl
	consts []constDecl
}

// runPlan writes the overlay. root is the upstream module's directory as the go command reports
// it, so the overlay's keys name the files the go command will look for.
func runPlan(outDir, root string) {
	if outDir == "" {
		dump.Fatalf("-plan needs a directory")
	}
	outDir, err := filepath.Abs(outDir)
	if err != nil {
		dump.Fatalf("resolving -plan: %v", err)
	}
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		dump.Fatalf("creating %s: %v", outDir, err)
	}

	var local []pkgDecls
	for _, rel := range expandModulePackages(root) {
		local = append(local, parsePackage(modulePath+"/"+rel, filepath.Join(root, rel)))
	}
	var external []pkgDecls
	dirs := goListDirs(append([]string{selfPath}, externalPackages...))
	for _, path := range externalPackages {
		external = append(external, parsePackage(path, dirs[path]))
	}

	replace := map[string]string{}
	for i, p := range local {
		target := filepath.Join(p.dir, overlayName)
		if _, err := os.Stat(target); err == nil {
			dump.Fatalf("%s already exists; the overlay would shadow it", target)
		}
		src := filepath.Join(outDir, fmt.Sprintf("local_%03d.go", i))
		writeGo(src, localOverlay(p))
		replace[target] = src
	}
	src := filepath.Join(outDir, "self.go")
	writeGo(src, selfOverlay(local, external))
	replace[filepath.Join(dirs[selfPath], overlayName)] = src

	overlay, err := dump.Marshal(map[string]interface{}{"Replace": replace})
	if err != nil {
		dump.Fatalf("encoding overlay: %v", err)
	}
	dump.WriteFile(filepath.Join(outDir, "overlay.json"), overlay)
}

func expandModulePackages(root string) []string {
	var out []string
	for _, p := range modulePackages {
		base, recursive := strings.CutSuffix(p, "/...")
		if !recursive {
			out = append(out, p)
			continue
		}
		err := filepath.WalkDir(filepath.Join(root, base), func(path string, d os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if !d.IsDir() {
				return nil
			}
			if name := d.Name(); name == "testdata" || strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_") {
				return filepath.SkipDir
			}
			pkg, err := build.Default.ImportDir(path, 0)
			if err != nil {
				if _, ok := err.(*build.NoGoError); ok {
					return nil
				}
				return err
			}
			if len(pkg.GoFiles)+len(pkg.CgoFiles) == 0 {
				return nil
			}
			rel, err := filepath.Rel(root, path)
			if err != nil {
				return err
			}
			out = append(out, filepath.ToSlash(rel))
			return nil
		})
		if err != nil {
			dump.Fatalf("walking %s: %v", base, err)
		}
	}
	sort.Strings(out)
	return out
}

// goListDirs locates packages in this program's own build (the current directory's module, or the
// workspace GOWORK names), so that they are the packages the overlay is compiled against.
func goListDirs(paths []string) map[string]string {
	args := append([]string{"list", "-f", "{{.ImportPath}}\t{{.Dir}}"}, paths...)
	cmd := exec.Command("go", args...)
	cmd.Stderr = os.Stderr
	out, err := cmd.Output()
	if err != nil {
		dump.Fatalf("go list: %v", err)
	}
	dirs := map[string]string{}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		path, dir, ok := strings.Cut(line, "\t")
		if !ok {
			dump.Fatalf("unexpected go list line %q", line)
		}
		dirs[path] = dir
	}
	for _, p := range paths {
		if dirs[p] == "" {
			dump.Fatalf("go list did not locate %s", p)
		}
	}
	return dirs
}

func parsePackage(path, dir string) pkgDecls {
	pkg, err := build.Default.ImportDir(dir, 0)
	if err != nil {
		dump.Fatalf("ImportDir(%s): %v", dir, err)
	}
	if len(pkg.IgnoredGoFiles) > 0 {
		for _, f := range pkg.IgnoredGoFiles {
			if !strings.HasSuffix(f, "_test.go") {
				dump.Fatalf("%s: %s is excluded by build constraints; the golden would depend on GOOS/GOARCH", path, f)
			}
		}
	}
	files := append(append([]string(nil), pkg.GoFiles...), pkg.CgoFiles...)
	sort.Strings(files)
	out := pkgDecls{path: path, dir: dir, name: pkg.Name}
	fset := token.NewFileSet()
	order := 0
	for _, name := range files {
		f, err := parser.ParseFile(fset, filepath.Join(dir, name), nil, parser.SkipObjectResolution)
		if err != nil {
			dump.Fatalf("parsing %s/%s: %v", dir, name, err)
		}
		for _, decl := range f.Decls {
			gen, ok := decl.(*ast.GenDecl)
			if !ok {
				continue
			}
			switch gen.Tok {
			case token.TYPE:
				for _, spec := range gen.Specs {
					ts := spec.(*ast.TypeSpec)
					if ts.Name.Name == "_" {
						continue
					}
					out.types = append(out.types, typeDecl{
						name:    ts.Name.Name,
						alias:   ts.Assign.IsValid(),
						generic: ts.TypeParams != nil && len(ts.TypeParams.List) > 0,
					})
				}
			case token.CONST:
				// Implicit repetition: a spec without type and values repeats the previous one.
				var blockType ast.Expr
				for _, spec := range gen.Specs {
					vs := spec.(*ast.ValueSpec)
					if vs.Type != nil || len(vs.Values) > 0 {
						blockType = vs.Type
					}
					for _, n := range vs.Names {
						if blockType == nil || !ast.IsExported(n.Name) {
							continue
						}
						out.consts = append(out.consts, constDecl{name: n.Name, order: order})
						order++
					}
				}
			}
		}
	}
	return out
}

func localOverlay(p pkgDecls) []byte {
	var b bytes.Buffer
	fmt.Fprintf(&b, "// Code generated by structmeta -plan. DO NOT EDIT.\n\npackage %s\n\n", p.name)
	fmt.Fprintf(&b, "import (\n\tzzpbsreflect \"reflect\"\n\n\tzzpbsreg %q\n)\n\n", regPath)
	fmt.Fprintf(&b, "func init() {\n\tzzpbsreg.Package(%q, %q)\n", p.path, p.name)
	for _, t := range p.types {
		if t.generic {
			fmt.Fprintf(&b, "\tzzpbsreg.Skip(%q, %q, %q)\n", p.path, t.name, "generic type")
			continue
		}
		fmt.Fprintf(&b, "\tzzpbsreg.AddType(%q, %q, zzpbsreflect.TypeOf((*%s)(nil)).Elem(), %t)\n", p.path, t.name, t.name, t.alias)
	}
	for _, c := range p.consts {
		fmt.Fprintf(&b, "\tzzpbsreg.AddConst(%q, %q, %d, %s)\n", p.path, c.name, c.order, c.name)
	}
	b.WriteString("}\n")
	return b.Bytes()
}

func selfOverlay(local, external []pkgDecls) []byte {
	var b bytes.Buffer
	b.WriteString("// Code generated by structmeta -plan. DO NOT EDIT.\n\npackage main\n\nimport (\n\t\"reflect\"\n\n")
	for i, p := range external {
		fmt.Fprintf(&b, "\tzzext%d %q\n", i, p.path)
	}
	for _, p := range local {
		fmt.Fprintf(&b, "\t_ %q\n", p.path)
	}
	fmt.Fprintf(&b, "\n\tzzpbsreg %q\n)\n\n", regPath)
	b.WriteString("func init() {\n\texpectedPackages = []string{\n")
	for _, p := range local {
		fmt.Fprintf(&b, "\t\t%q,\n", p.path)
	}
	for _, p := range external {
		fmt.Fprintf(&b, "\t\t%q,\n", p.path)
	}
	b.WriteString("\t}\n")
	for i, p := range external {
		fmt.Fprintf(&b, "\tzzpbsreg.Package(%q, %q)\n", p.path, p.name)
		for _, t := range p.types {
			switch {
			case !ast.IsExported(t.name):
				fmt.Fprintf(&b, "\tzzpbsreg.Skip(%q, %q, %q)\n", p.path, t.name, "unexported type in a dependency module")
			case t.generic:
				fmt.Fprintf(&b, "\tzzpbsreg.Skip(%q, %q, %q)\n", p.path, t.name, "generic type")
			default:
				fmt.Fprintf(&b, "\tzzpbsreg.AddType(%q, %q, reflect.TypeOf((*zzext%d.%s)(nil)).Elem(), %t)\n", p.path, t.name, i, t.name, t.alias)
			}
		}
		for _, c := range p.consts {
			fmt.Fprintf(&b, "\tzzpbsreg.AddConst(%q, %q, %d, zzext%d.%s)\n", p.path, c.name, c.order, i, c.name)
		}
	}
	b.WriteString("}\n")
	return b.Bytes()
}

func writeGo(path string, src []byte) {
	dump.WriteFile(path, src)
}

// ---------------------------------------------------------------------------------------------
// Dump

var (
	jsonMarshaler   = reflect.TypeOf((*json.Marshaler)(nil)).Elem()
	jsonUnmarshaler = reflect.TypeOf((*json.Unmarshaler)(nil)).Elem()
	textMarshaler   = reflect.TypeOf((*encoding.TextMarshaler)(nil)).Elem()
	textUnmarshaler = reflect.TypeOf((*encoding.TextUnmarshaler)(nil)).Elem()
)

type jsonTag struct {
	Raw       string `json:"raw"`
	Name      string `json:"name"`
	OmitEmpty bool   `json:"omitempty"`
	String    bool   `json:"string"`
	OmitZero  bool   `json:"omitzero"`
	Skip      bool   `json:"skip"`
}

type field struct {
	Name     string   `json:"name"`
	Type     string   `json:"type"`
	Kind     string   `json:"kind"`
	Embedded bool     `json:"embedded"`
	Exported bool     `json:"exported"`
	JSON     *jsonTag `json:"json"`
	Tag      string   `json:"tag"`
	// AnonFields describes an unnamed struct reached through the field's type (directly or via
	// pointer, slice, array or map element).
	AnonFields []field `json:"anon_fields,omitempty"`
}

type typeMeta struct {
	Name       string   `json:"name"`
	GoName     string   `json:"go_name"`
	Kind       string   `json:"kind"`
	AliasOf    string   `json:"alias_of,omitempty"`
	Underlying string   `json:"underlying,omitempty"`
	Methods    []string `json:"interface_methods,omitempty"`
	Marshalers []string `json:"marshalers,omitempty"`
	fields     []field
}

type constMeta struct {
	Name  string          `json:"name"`
	Type  string          `json:"type"`
	Kind  string          `json:"kind"`
	Value json.RawMessage `json:"value"`
}

type skipMeta struct {
	Name   string `json:"name"`
	Reason string `json:"reason"`
}

func runDump(opts dump.Options) {
	if len(expectedPackages) == 0 {
		dump.Fatalf("no overlay compiled in: run `structmeta -plan DIR` and then `go run -overlay DIR/overlay.json`")
	}
	ran := map[string]int{}
	names := map[string]string{}
	for _, p := range reg.Packages() {
		ran[p.Path]++
		names[p.Path] = p.Name
	}
	for _, p := range expectedPackages {
		if ran[p] != 1 {
			dump.Fatalf("overlay for %s ran %d times, want 1", p, ran[p])
		}
	}
	if len(ran) != len(expectedPackages) {
		dump.Fatalf("%d packages registered, %d expected", len(ran), len(expectedPackages))
	}

	paths := append([]string(nil), expectedPackages...)
	sort.Strings(paths)
	typesByPkg := map[string][]reg.Type{}
	for _, t := range reg.Types() {
		typesByPkg[t.PkgPath] = append(typesByPkg[t.PkgPath], t)
	}
	constsByPkg := map[string][]reg.Const{}
	for _, c := range reg.Consts() {
		constsByPkg[c.PkgPath] = append(constsByPkg[c.PkgPath], c)
	}
	skipsByPkg := map[string][]reg.Skipped{}
	for _, s := range reg.Skips() {
		skipsByPkg[s.PkgPath] = append(skipsByPkg[s.PkgPath], s)
	}

	var b bytes.Buffer
	b.WriteString("{\n")
	fmt.Fprintf(&b, "  \"generator\": %s,\n", dump.Raw("pbs-devtools go-dump struct-meta (rust/crates/devtools/go/structmeta)"))
	fmt.Fprintf(&b, "  \"go_commit\": %s,\n", dump.Raw(opts.GoCommit))
	fmt.Fprintf(&b, "  \"notes\": %s,\n", dump.Raw([]string{
		"packages sorted by import path; types sorted by name; fields in declaration order; consts in source order.",
		"go_name and every type string are reflect.Type.String() (package name, not path), the spelling jsoniter/encoding/json use in errors.",
		"json is the parsed `json` tag (null when absent); name is empty when the tag gives none or encoding/json would reject it; skip is the `json:\"-\"` case.",
		"marshalers lists json.Marshaler/json.Unmarshaler/encoding.TextMarshaler/encoding.TextUnmarshaler implemented by T or *T.",
		"consts are the exported typed constants (explicitly typed or via implicit repetition in a const block).",
	}))
	b.WriteString("  \"packages\": [")
	for pi, path := range paths {
		if pi > 0 {
			b.WriteString(",")
		}
		types := typesByPkg[path]
		sort.Slice(types, func(i, j int) bool { return types[i].Name < types[j].Name })
		consts := constsByPkg[path]
		sort.Slice(consts, func(i, j int) bool { return consts[i].Order < consts[j].Order })
		skips := skipsByPkg[path]
		sort.Slice(skips, func(i, j int) bool { return skips[i].Name < skips[j].Name })

		fmt.Fprintf(&b, "\n    {\n      \"path\": %s,\n      \"name\": %s,\n", dump.Raw(path), dump.Raw(names[path]))
		b.WriteString("      \"types\": [")
		for ti, t := range types {
			if ti > 0 {
				b.WriteString(",")
			}
			m := describeNamed(t)
			head := dump.Raw(m)
			// Splice the fields in one per line so diffs stay readable.
			fmt.Fprintf(&b, "\n        %s", bytes.TrimSuffix(head, []byte("}")))
			if m.Kind == "struct" && m.AliasOf == "" {
				b.WriteString(",\"fields\":[")
				for fi, f := range m.fields {
					if fi > 0 {
						b.WriteString(",")
					}
					fmt.Fprintf(&b, "\n          %s", dump.Raw(f))
				}
				if len(m.fields) > 0 {
					b.WriteString("\n        ")
				}
				b.WriteString("]")
			}
			b.WriteString("}")
		}
		if len(types) > 0 {
			b.WriteString("\n      ")
		}
		b.WriteString("],\n      \"consts\": [")
		for ci, c := range consts {
			if ci > 0 {
				b.WriteString(",")
			}
			fmt.Fprintf(&b, "\n        %s", dump.Raw(describeConst(c)))
		}
		if len(consts) > 0 {
			b.WriteString("\n      ")
		}
		b.WriteString("],\n      \"skipped\": [")
		for si, s := range skips {
			if si > 0 {
				b.WriteString(",")
			}
			fmt.Fprintf(&b, "\n        %s", dump.Raw(skipMeta{Name: s.Name, Reason: s.Reason}))
		}
		if len(skips) > 0 {
			b.WriteString("\n      ")
		}
		b.WriteString("]\n    }")
	}
	b.WriteString("\n  ]\n}\n")

	var check interface{}
	if err := json.Unmarshal(b.Bytes(), &check); err != nil {
		dump.Fatalf("generated invalid JSON: %v", err)
	}
	dump.WriteFile(opts.Out, b.Bytes())
}

func describeNamed(rt reg.Type) typeMeta {
	t := rt.Type
	m := typeMeta{Name: rt.Name, GoName: t.String(), Kind: t.Kind().String()}
	if rt.Alias {
		m.AliasOf = t.String()
		m.GoName = ""
		return m
	}
	switch t.Kind() {
	case reflect.Struct:
		m.fields = describeFields(t)
	case reflect.Interface:
		for i := 0; i < t.NumMethod(); i++ {
			m.Methods = append(m.Methods, t.Method(i).Name)
		}
	default:
		m.Underlying = underlying(t)
	}
	if t.Kind() != reflect.Interface {
		m.Marshalers = marshalers(t)
	}
	return m
}

func marshalers(t reflect.Type) []string {
	var out []string
	for _, iface := range []struct {
		name string
		t    reflect.Type
	}{
		{"json.Marshaler", jsonMarshaler},
		{"json.Unmarshaler", jsonUnmarshaler},
		{"encoding.TextMarshaler", textMarshaler},
		{"encoding.TextUnmarshaler", textUnmarshaler},
	} {
		switch {
		case t.Implements(iface.t):
			out = append(out, iface.name)
		case reflect.PointerTo(t).Implements(iface.t):
			out = append(out, "*"+iface.name)
		}
	}
	return out
}

func describeFields(t reflect.Type) []field {
	out := make([]field, 0, t.NumField())
	for i := 0; i < t.NumField(); i++ {
		sf := t.Field(i)
		f := field{
			Name:     sf.Name,
			Type:     sf.Type.String(),
			Kind:     sf.Type.Kind().String(),
			Embedded: sf.Anonymous,
			Exported: sf.IsExported(),
			Tag:      string(sf.Tag),
		}
		if raw, ok := sf.Tag.Lookup("json"); ok {
			f.JSON = parseJSONTag(raw)
		}
		if anon := anonStruct(sf.Type); anon != nil {
			f.AnonFields = describeFields(anon)
		}
		out = append(out, f)
	}
	return out
}

// anonStruct returns the unnamed struct type reached through t's pointer/slice/array/map elements.
func anonStruct(t reflect.Type) reflect.Type {
	for {
		if t.Name() != "" {
			return nil
		}
		switch t.Kind() {
		case reflect.Struct:
			return t
		case reflect.Pointer, reflect.Slice, reflect.Array, reflect.Map:
			t = t.Elem()
		default:
			return nil
		}
	}
}

// parseJSONTag follows encoding/json's typeFields/parseTag/isValidTag.
func parseJSONTag(raw string) *jsonTag {
	tag := &jsonTag{Raw: raw}
	if raw == "-" {
		tag.Skip = true
		return tag
	}
	name, opts, _ := strings.Cut(raw, ",")
	if isValidTag(name) {
		tag.Name = name
	}
	for opts != "" {
		var opt string
		opt, opts, _ = strings.Cut(opts, ",")
		switch opt {
		case "omitempty":
			tag.OmitEmpty = true
		case "string":
			tag.String = true
		case "omitzero":
			tag.OmitZero = true
		}
	}
	return tag
}

// isValidTag is encoding/json's isValidTag (encoding/json/encode.go).
func isValidTag(s string) bool {
	if s == "" {
		return false
	}
	for _, c := range s {
		switch {
		case strings.ContainsRune("!#$%&()*+-./:;<=>?@[]^_{|}~ ", c):
			// Backslash and quote chars are reserved, but otherwise any punctuation chars are
			// allowed in a tag name.
		case !unicode.IsLetter(c) && !unicode.IsDigit(c):
			return false
		}
	}
	return true
}

// underlying spells the underlying type of a named non-struct type the way reflect would spell an
// unnamed type with the same structure.
func underlying(t reflect.Type) string {
	switch t.Kind() {
	case reflect.Pointer:
		return "*" + t.Elem().String()
	case reflect.Slice:
		return "[]" + t.Elem().String()
	case reflect.Array:
		return fmt.Sprintf("[%d]%s", t.Len(), t.Elem().String())
	case reflect.Map:
		return fmt.Sprintf("map[%s]%s", t.Key().String(), t.Elem().String())
	case reflect.Chan:
		return t.ChanDir().String() + " " + t.Elem().String()
	case reflect.Func:
		var in, out []string
		for i := 0; i < t.NumIn(); i++ {
			s := t.In(i).String()
			if t.IsVariadic() && i == t.NumIn()-1 {
				s = "..." + t.In(i).Elem().String()
			}
			in = append(in, s)
		}
		for i := 0; i < t.NumOut(); i++ {
			out = append(out, t.Out(i).String())
		}
		s := "func(" + strings.Join(in, ", ") + ")"
		switch len(out) {
		case 0:
		case 1:
			s += " " + out[0]
		default:
			s += " (" + strings.Join(out, ", ") + ")"
		}
		return s
	default:
		return t.Kind().String()
	}
}

// describeConst records the constant's underlying value; a MarshalJSON method on its type is
// deliberately bypassed.
func describeConst(c reg.Const) constMeta {
	v := reflect.ValueOf(c.Value)
	var raw interface{}
	switch v.Kind() {
	case reflect.Bool:
		raw = v.Bool()
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		raw = v.Int()
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr:
		raw = v.Uint()
	case reflect.Float32, reflect.Float64:
		raw = v.Float()
	case reflect.String:
		raw = v.String()
	default:
		dump.Fatalf("constant %s.%s has unsupported kind %s", c.PkgPath, c.Name, v.Kind())
	}
	return constMeta{Name: c.Name, Type: v.Type().String(), Kind: v.Kind().String(), Value: dump.Raw(raw)}
}
