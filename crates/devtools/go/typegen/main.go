// Command typegen describes Go packages for `pbs-devtools typegen`, which turns the description
// into Rust types for pbs-gojson.
//
// pbs-devtools runs it from the directory of this module (crates/devtools/go), so the packages
// resolve in this module's build (the upstream checkout and its dependencies):
//
//	go run ./typegen -out FILE IMPORTPATH...
//
// For each package it records, in source order (files sorted by name, declarations in file
// order): every type declaration with its Go doc comment; structs with their fields (name, type,
// tag, embedded flag, doc and trailing comment); named non-struct types with their underlying
// type; aliases with their target; which of json.Marshaler, json.Unmarshaler,
// encoding.TextMarshaler and encoding.TextUnmarshaler the type or its pointer implements; typed
// constants with their exact values; and the package's functions and methods (name, receiver and
// signature), which the Rust side ports by hand.
//
// Packages are type-checked from source with go/types; their imports come from the export data
// that `go list -export` builds, so any package that compiles can be described.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/constant"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strconv"
	"strings"
)

// description is the whole output document.
type description struct {
	Generator string    `json:"generator"`
	Packages  []pkgDesc `json:"packages"`
}

type pkgDesc struct {
	Path string `json:"path"`
	Name string `json:"name"`
	// Module and Version identify the module the package belongs to; Version is empty for the
	// main module.
	Module  string   `json:"module"`
	Version string   `json:"version"`
	Doc     string   `json:"doc"`
	Files   []string `json:"files"`
	Decls   []decl   `json:"decls"`
}

// decl is one top-level declaration.
type decl struct {
	// Kind is struct, named, alias, interface, const, func or method.
	Kind     string `json:"kind"`
	Name     string `json:"name"`
	File     string `json:"file"`
	Line     int    `json:"line"`
	Exported bool   `json:"exported"`
	Doc      string `json:"doc,omitempty"`
	// Comment is the trailing line comment of a single-line spec.
	Comment string `json:"comment,omitempty"`
	// Generic is set for a generic type or function.
	Generic bool `json:"generic,omitempty"`
	// Fields are a struct's fields in declaration order.
	Fields []field `json:"fields,omitempty"`
	// Type is a named type's underlying type, an alias's target or a constant's type.
	Type *typeRef `json:"type,omitempty"`
	// Methods lists an interface's method names.
	Methods []string `json:"methods,omitempty"`
	// JSON lists the JSON-related methods of a named type.
	JSON *jsonMethods `json:"json_methods,omitempty"`
	// Value is a constant's exact value: decimal for integers, Go's shortest float text for
	// floats, the string itself for strings, true or false for booleans.
	Value     string `json:"value,omitempty"`
	ValueKind string `json:"value_kind,omitempty"`
	// Group numbers the const block a constant belongs to (1-based within the package);
	// GroupDoc is that block's doc comment.
	Group    int    `json:"group,omitempty"`
	GroupDoc string `json:"group_doc,omitempty"`
	// Recv and PtrRecv describe a method's receiver.
	Recv    string `json:"recv,omitempty"`
	PtrRecv bool   `json:"ptr_recv,omitempty"`
	// Signature is a function's or method's signature with package-name qualifiers.
	Signature string `json:"signature,omitempty"`
}

type field struct {
	Name string  `json:"name"`
	Type typeRef `json:"type"`
	// Tag is the whole struct tag; JSON is its `json` key (reflect.StructTag.Lookup), null when
	// the tag has none.
	Tag      string  `json:"tag"`
	JSON     *string `json:"json"`
	Embedded bool    `json:"embedded"`
	Exported bool    `json:"exported"`
	Doc      string  `json:"doc,omitempty"`
	Comment  string  `json:"comment,omitempty"`
}

// typeRef describes a type expression.
type typeRef struct {
	// Kind is basic, named, pointer, slice, array, map, interface, struct, func, chan or
	// typeparam.
	Kind string `json:"kind"`
	// Name is a basic type's name or a named type's name.
	Name string `json:"name,omitempty"`
	// Pkg and PkgName are a named type's import path and package name.
	Pkg     string `json:"pkg,omitempty"`
	PkgName string `json:"pkg_name,omitempty"`
	// Args are a generic instantiation's type arguments.
	Args []typeRef `json:"args,omitempty"`
	Elem *typeRef  `json:"elem,omitempty"`
	Key  *typeRef  `json:"key,omitempty"`
	Len  int64     `json:"len,omitempty"`
	// Fields are an unnamed struct type's fields.
	Fields []field `json:"fields,omitempty"`
	// Methods are an interface type's method names (empty for interface{}).
	Methods []string `json:"methods,omitempty"`
	// Reflect is reflect.Type.String() for the type, the spelling Go's JSON errors use.
	Reflect string `json:"reflect"`
}

// jsonMethods records, for each JSON-related method, "value" (T has it) or "pointer" (only *T).
type jsonMethods struct {
	MarshalJSON   string `json:"marshal_json,omitempty"`
	UnmarshalJSON string `json:"unmarshal_json,omitempty"`
	MarshalText   string `json:"marshal_text,omitempty"`
	UnmarshalText string `json:"unmarshal_text,omitempty"`
}

// listed is the part of `go list -json` output the describer uses.
type listed struct {
	ImportPath string
	Name       string
	Dir        string
	Export     string
	GoFiles    []string
	CgoFiles   []string
	ImportMap  map[string]string
	Module     *struct {
		Path    string
		Version string
		Main    bool
	}
	Error *struct {
		Err string
	}
	DepsErrors []*struct {
		Err string
	}
}

func main() {
	out := flag.String("out", "", "file to write the JSON description to")
	flag.Parse()
	if *out == "" || flag.NArg() == 0 {
		fatalf("usage: typegen -out FILE IMPORTPATH...")
	}
	targets := flag.Args()
	pkgs := goList(targets)
	desc := description{Generator: "pbs-devtools typegen (crates/devtools/go/typegen)"}

	for _, path := range targets {
		desc.Packages = append(desc.Packages, describe(pkgs, pkgs[path]))
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(desc); err != nil {
		fatalf("encoding the description: %v", err)
	}
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		fatalf("writing %s: %v", *out, err)
	}
}

func fatalf(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, "typegen: "+format+"\n", args...)
	os.Exit(1)
}

// goCommand is the go command of the toolchain running this program, so that the export data it
// builds matches this program's importer.
func goCommand() string {
	candidate := filepath.Join(runtime.GOROOT(), "bin", "go")
	if _, err := os.Stat(candidate); err == nil {
		return candidate
	}
	return "go"
}

// goList lists the targets and every dependency, building export data for each.
func goList(targets []string) map[string]*listed {
	args := append([]string{"list", "-e", "-json", "-export", "-deps"}, targets...)
	cmd := exec.Command(goCommand(), args...)
	cmd.Stderr = os.Stderr
	out, err := cmd.Output()
	if err != nil {
		fatalf("go list: %v", err)
	}
	pkgs := map[string]*listed{}
	dec := json.NewDecoder(bytes.NewReader(out))
	for {
		var p listed
		if err := dec.Decode(&p); err == io.EOF {
			break
		} else if err != nil {
			fatalf("decoding go list output: %v", err)
		}
		pkgs[p.ImportPath] = &p
	}
	for _, t := range targets {
		p := pkgs[t]
		if p == nil {
			fatalf("go list did not report %s", t)
		}
		if p.Error != nil {
			fatalf("%s: %s", t, p.Error.Err)
		}
		for _, e := range p.DepsErrors {
			fatalf("%s: dependency error: %s", t, e.Err)
		}
	}
	return pkgs
}

func describe(all map[string]*listed, p *listed) pkgDesc {
	fset := token.NewFileSet()
	names := append(append([]string(nil), p.GoFiles...), p.CgoFiles...)
	sort.Strings(names)
	var files []*ast.File
	for _, name := range names {
		f, err := parser.ParseFile(fset, filepath.Join(p.Dir, name), nil, parser.ParseComments)
		if err != nil {
			fatalf("parsing %s/%s: %v", p.Dir, name, err)
		}
		files = append(files, f)
	}
	lookup := func(path string) (io.ReadCloser, error) {
		if mapped, ok := p.ImportMap[path]; ok {
			path = mapped
		}
		dep := all[path]
		if dep == nil || dep.Export == "" {
			return nil, fmt.Errorf("no export data for %s", path)
		}
		return os.Open(dep.Export)
	}
	conf := types.Config{Importer: importer.ForCompiler(fset, "gc", lookup), FakeImportC: true}
	info := &types.Info{Defs: map[*ast.Ident]types.Object{}}
	pkg, err := conf.Check(p.ImportPath, fset, files, info)
	if err != nil {
		fatalf("type-checking %s: %v", p.ImportPath, err)
	}
	out := pkgDesc{Path: p.ImportPath, Name: pkg.Name(), Files: names}
	if p.Module != nil {
		out.Module = p.Module.Path
		if !p.Module.Main {
			out.Version = p.Module.Version
		}
	}
	d := describer{fset: fset, pkg: pkg, info: info}
	group := 0
	for i, f := range files {
		if f.Doc != nil {
			if out.Doc != "" {
				out.Doc += "\n"
			}
			out.Doc += f.Doc.Text()
		}
		for _, gd := range f.Decls {
			switch gd := gd.(type) {
			case *ast.GenDecl:
				switch gd.Tok {
				case token.TYPE:
					for _, spec := range gd.Specs {
						out.Decls = append(out.Decls, d.typeDecl(names[i], gd, spec.(*ast.TypeSpec)))
					}
				case token.CONST:
					group++
					out.Decls = append(out.Decls, d.constDecls(names[i], gd, group)...)
				}
			case *ast.FuncDecl:
				out.Decls = append(out.Decls, d.funcDecl(names[i], gd))
			}
		}
	}
	return out
}

type describer struct {
	fset *token.FileSet
	pkg  *types.Package
	info *types.Info
}

func text(cg *ast.CommentGroup) string {
	if cg == nil {
		return ""
	}
	return cg.Text()
}

// specDoc is a spec's own doc, or the declaration's when it holds this one spec.
func specDoc(gd *ast.GenDecl, own *ast.CommentGroup) string {
	if own != nil {
		return own.Text()
	}
	if len(gd.Specs) == 1 {
		return text(gd.Doc)
	}
	return ""
}

func (d *describer) typeDecl(file string, gd *ast.GenDecl, ts *ast.TypeSpec) decl {
	obj, ok := d.info.Defs[ts.Name].(*types.TypeName)
	if !ok {
		fatalf("%s: no type object for %s", d.pkg.Path(), ts.Name.Name)
	}
	out := decl{
		Name:     ts.Name.Name,
		File:     file,
		Line:     d.fset.Position(ts.Pos()).Line,
		Exported: ts.Name.IsExported(),
		Doc:      specDoc(gd, ts.Doc),
		Comment:  text(ts.Comment),
		Generic:  ts.TypeParams != nil && len(ts.TypeParams.List) > 0,
	}
	if obj.IsAlias() {
		out.Kind = "alias"
		target := d.ref(obj.Type())
		out.Type = &target
		return out
	}
	named, ok := obj.Type().(*types.Named)
	if !ok {
		fatalf("%s.%s is not a named type", d.pkg.Path(), obj.Name())
	}
	out.JSON = jsonMethodsOf(named)
	switch u := named.Underlying().(type) {
	case *types.Struct:
		out.Kind = "struct"
		st, _ := ts.Type.(*ast.StructType)
		out.Fields = d.fields(u, st)
	case *types.Interface:
		out.Kind = "interface"
		for i := 0; i < u.NumMethods(); i++ {
			out.Methods = append(out.Methods, u.Method(i).Name())
		}
	default:
		out.Kind = "named"
		under := d.ref(u)
		out.Type = &under
	}
	return out
}

// fields describes a struct's fields; st (which may be nil) supplies their comments.
func (d *describer) fields(s *types.Struct, st *ast.StructType) []field {
	type comments struct{ doc, comment string }
	var docs []comments
	if st != nil && st.Fields != nil {
		for _, f := range st.Fields.List {
			c := comments{doc: text(f.Doc), comment: text(f.Comment)}
			n := len(f.Names)
			if n == 0 {
				n = 1
			}
			for i := 0; i < n; i++ {
				docs = append(docs, c)
			}
		}
	}
	out := make([]field, 0, s.NumFields())
	for i := 0; i < s.NumFields(); i++ {
		v := s.Field(i)
		f := field{
			Name:     v.Name(),
			Type:     d.ref(v.Type()),
			Tag:      s.Tag(i),
			Embedded: v.Embedded(),
			Exported: v.Exported(),
		}
		if tag, ok := reflect.StructTag(s.Tag(i)).Lookup("json"); ok {
			f.JSON = &tag
		}
		if i < len(docs) {
			f.Doc = docs[i].doc
			f.Comment = docs[i].comment
		}
		out = append(out, f)
	}
	return out
}

func (d *describer) constDecls(file string, gd *ast.GenDecl, group int) []decl {
	var out []decl
	for _, spec := range gd.Specs {
		vs := spec.(*ast.ValueSpec)
		for _, name := range vs.Names {
			if name.Name == "_" {
				continue
			}
			c, ok := d.info.Defs[name].(*types.Const)
			if !ok {
				fatalf("%s: no constant object for %s", d.pkg.Path(), name.Name)
			}
			typ := d.ref(c.Type())
			value, kind := constValue(c.Val())
			out = append(out, decl{
				Kind:      "const",
				Name:      name.Name,
				File:      file,
				Line:      d.fset.Position(name.Pos()).Line,
				Exported:  name.IsExported(),
				Doc:       text(vs.Doc),
				Comment:   text(vs.Comment),
				Type:      &typ,
				Value:     value,
				ValueKind: kind,
				Group:     group,
				GroupDoc:  text(gd.Doc),
			})
		}
	}
	return out
}

func constValue(v constant.Value) (string, string) {
	switch v.Kind() {
	case constant.Bool:
		return strconv.FormatBool(constant.BoolVal(v)), "bool"
	case constant.String:
		return constant.StringVal(v), "string"
	case constant.Int:
		return v.ExactString(), "int"
	case constant.Float:
		f, _ := constant.Float64Val(v)
		return strconv.FormatFloat(f, 'g', -1, 64), "float"
	default:
		return v.ExactString(), "other"
	}
}

func (d *describer) funcDecl(file string, fd *ast.FuncDecl) decl {
	obj, ok := d.info.Defs[fd.Name].(*types.Func)
	if !ok {
		fatalf("%s: no function object for %s", d.pkg.Path(), fd.Name.Name)
	}
	sig := obj.Type().(*types.Signature)
	out := decl{
		Kind:      "func",
		Name:      fd.Name.Name,
		File:      file,
		Line:      d.fset.Position(fd.Pos()).Line,
		Exported:  fd.Name.IsExported(),
		Doc:       text(fd.Doc),
		Generic:   sig.TypeParams().Len() > 0,
		Signature: types.TypeString(sig, d.qualifier),
	}
	if recv := sig.Recv(); recv != nil {
		out.Kind = "method"
		t := recv.Type()
		if p, ok := t.(*types.Pointer); ok {
			out.PtrRecv = true
			t = p.Elem()
		}
		if n, ok := t.(*types.Named); ok {
			out.Recv = n.Obj().Name()
		}
	}
	return out
}

func (d *describer) qualifier(p *types.Package) string {
	return p.Name()
}

// jsonMethodsOf reports which JSON-related methods T (value receiver) or only *T has.
func jsonMethodsOf(t *types.Named) *jsonMethods {
	m := &jsonMethods{}
	set := func(name string) string {
		if has(types.NewMethodSet(t), name) {
			return "value"
		}
		if has(types.NewMethodSet(types.NewPointer(t)), name) {
			return "pointer"
		}
		return ""
	}
	m.MarshalJSON = set("MarshalJSON")
	m.UnmarshalJSON = set("UnmarshalJSON")
	m.MarshalText = set("MarshalText")
	m.UnmarshalText = set("UnmarshalText")
	if *m == (jsonMethods{}) {
		return nil
	}
	return m
}

func has(ms *types.MethodSet, name string) bool {
	for i := 0; i < ms.Len(); i++ {
		if ms.At(i).Obj().Name() == name {
			return true
		}
	}
	return false
}

// ref describes t, resolving aliases.
func (d *describer) ref(t types.Type) typeRef {
	t = types.Unalias(t)
	switch t := t.(type) {
	case *types.Basic:
		name := t.Name()
		switch t.Kind() {
		case types.Byte:
			name = "uint8"
		case types.Rune:
			name = "int32"
		}
		return typeRef{Kind: "basic", Name: name, Reflect: name}
	case *types.Named:
		obj := t.Obj()
		r := typeRef{Kind: "named", Name: obj.Name()}
		if obj.Pkg() != nil {
			r.Pkg = obj.Pkg().Path()
			r.PkgName = obj.Pkg().Name()
		}
		if args := t.TypeArgs(); args != nil {
			for i := 0; i < args.Len(); i++ {
				r.Args = append(r.Args, d.ref(args.At(i)))
			}
		}
		r.Reflect = reflectName(r)
		return r
	case *types.Pointer:
		elem := d.ref(t.Elem())
		return typeRef{Kind: "pointer", Elem: &elem, Reflect: "*" + elem.Reflect}
	case *types.Slice:
		elem := d.ref(t.Elem())
		return typeRef{Kind: "slice", Elem: &elem, Reflect: "[]" + elem.Reflect}
	case *types.Array:
		elem := d.ref(t.Elem())
		return typeRef{Kind: "array", Elem: &elem, Len: t.Len(), Reflect: fmt.Sprintf("[%d]%s", t.Len(), elem.Reflect)}
	case *types.Map:
		key := d.ref(t.Key())
		elem := d.ref(t.Elem())
		return typeRef{Kind: "map", Key: &key, Elem: &elem, Reflect: "map[" + key.Reflect + "]" + elem.Reflect}
	case *types.Interface:
		r := typeRef{Kind: "interface", Reflect: "interface {}"}
		if t.NumMethods() > 0 {
			var parts []string
			for i := 0; i < t.NumMethods(); i++ {
				m := t.Method(i)
				r.Methods = append(r.Methods, m.Name())
				parts = append(parts, m.Name()+strings.TrimPrefix(types.TypeString(m.Type(), d.qualifier), "func"))
			}
			r.Reflect = "interface { " + strings.Join(parts, "; ") + " }"
		}
		return r
	case *types.Struct:
		r := typeRef{Kind: "struct", Fields: d.fields(t, nil)}
		r.Reflect = reflectStruct(r.Fields)
		return r
	case *types.Signature:
		return typeRef{Kind: "func", Reflect: types.TypeString(t, d.qualifier)}
	case *types.Chan:
		elem := d.ref(t.Elem())
		prefix := "chan "
		switch t.Dir() {
		case types.SendOnly:
			prefix = "chan<- "
		case types.RecvOnly:
			prefix = "<-chan "
		}
		return typeRef{Kind: "chan", Elem: &elem, Reflect: prefix + elem.Reflect}
	case *types.TypeParam:
		return typeRef{Kind: "typeparam", Name: t.Obj().Name(), Reflect: t.Obj().Name()}
	default:
		fatalf("unsupported type %T (%s)", t, t)
		return typeRef{}
	}
}

// reflectName is reflect.Type.String() of a named type.
func reflectName(r typeRef) string {
	s := r.Name
	if r.PkgName != "" {
		s = r.PkgName + "." + s
	}
	if len(r.Args) > 0 {
		var args []string
		for _, a := range r.Args {
			args = append(args, a.Reflect)
		}
		s += "[" + strings.Join(args, ",") + "]"
	}
	return s
}

// reflectStruct is reflect.Type.String() of an unnamed struct type.
func reflectStruct(fields []field) string {
	if len(fields) == 0 {
		return "struct {}"
	}
	var parts []string
	for _, f := range fields {
		s := f.Type.Reflect
		if !f.Embedded {
			s = f.Name + " " + s
		}
		if f.Tag != "" {
			s += " " + strconv.Quote(f.Tag)
		}
		parts = append(parts, s)
	}
	return "struct { " + strings.Join(parts, "; ") + " }"
}
