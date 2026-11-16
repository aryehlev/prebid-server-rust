package corpus

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"path"
	"sort"
	"strconv"
	"strings"
)

// maxResolveDepth bounds identifier and helper-call chasing.
const maxResolveDepth = 12

// goPackage is one parsed Go package directory (test and non-test files),
// enough to resolve string constants and helper functions syntactically.
type goPackage struct {
	dir    string
	fset   *token.FileSet
	files  map[string]*ast.File
	values map[string]ast.Expr
	funcs  map[string]*ast.FuncDecl
}

// parseGoPackage parses every .go file of a repo-relative directory.
func parseGoPackage(repo *Repo, dir string) (*goPackage, error) {
	entries, err := repo.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	pkg := &goPackage{
		dir:    dir,
		fset:   token.NewFileSet(),
		files:  map[string]*ast.File{},
		values: map[string]ast.Expr{},
		funcs:  map[string]*ast.FuncDecl{},
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".go") {
			continue
		}
		rel := path.Join(dir, e.Name())
		src, err := repo.ReadFile(rel)
		if err != nil {
			return nil, err
		}
		file, err := parser.ParseFile(pkg.fset, rel, src, parser.SkipObjectResolution)
		if err != nil {
			return nil, fmt.Errorf("corpus: parsing %s: %w", rel, err)
		}
		pkg.files[rel] = file
		pkg.index(file)
	}
	return pkg, nil
}

// index records package-level single-name values and functions.
func (p *goPackage) index(file *ast.File) {
	for _, decl := range file.Decls {
		switch d := decl.(type) {
		case *ast.FuncDecl:
			if d.Recv == nil {
				p.funcs[d.Name.Name] = d
			}
		case *ast.GenDecl:
			if d.Tok != token.CONST && d.Tok != token.VAR {
				continue
			}
			for _, spec := range d.Specs {
				vs, ok := spec.(*ast.ValueSpec)
				if !ok || len(vs.Names) != len(vs.Values) {
					continue
				}
				for i, name := range vs.Names {
					p.values[name.Name] = vs.Values[i]
				}
			}
		}
	}
}

// sortedFiles returns the package's file paths in lexical order.
func (p *goPackage) sortedFiles() []string {
	names := make([]string, 0, len(p.files))
	for name := range p.files {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

// line returns the 1-based line of pos.
func (p *goPackage) line(pos token.Pos) int {
	return p.fset.Position(pos).Line
}

// where returns "file:line" of pos.
func (p *goPackage) where(pos token.Pos) string {
	position := p.fset.Position(pos)
	return fmt.Sprintf("%s:%d", position.Filename, position.Line)
}

// scope resolves identifiers inside one function body. Locals are recorded with
// their positions so lookups see the latest assignment before a use.
type scope struct {
	pkg    *goPackage
	locals map[string][]localValue
	params map[string]bound
	limit  token.Pos
}

type localValue struct {
	pos  token.Pos
	expr ast.Expr
}

// bound is an expression together with the scope it must be resolved in.
type bound struct {
	expr ast.Expr
	sc   *scope
}

// newScope collects the local assignments of body.
func newScope(pkg *goPackage, body *ast.BlockStmt) *scope {
	sc := &scope{pkg: pkg, locals: map[string][]localValue{}, params: map[string]bound{}}
	if body == nil {
		return sc
	}
	ast.Inspect(body, func(n ast.Node) bool {
		switch s := n.(type) {
		case *ast.AssignStmt:
			sc.recordAssign(s)
		case *ast.ValueSpec:
			if len(s.Names) == len(s.Values) {
				for i, name := range s.Names {
					sc.add(name.Name, s.Pos(), s.Values[i])
				}
			}
		}
		return true
	})
	return sc
}

func (sc *scope) recordAssign(s *ast.AssignStmt) {
	switch {
	case len(s.Lhs) == len(s.Rhs):
		for i, lhs := range s.Lhs {
			if id, ok := lhs.(*ast.Ident); ok {
				sc.add(id.Name, s.Pos(), s.Rhs[i])
			}
		}
	case len(s.Rhs) == 1:
		// "bidder, err := Builder(...)": the first result is the value.
		if id, ok := s.Lhs[0].(*ast.Ident); ok {
			sc.add(id.Name, s.Pos(), s.Rhs[0])
		}
	}
}

func (sc *scope) add(name string, pos token.Pos, expr ast.Expr) {
	if name == "_" {
		return
	}
	sc.locals[name] = append(sc.locals[name], localValue{pos: pos, expr: expr})
}

// at returns a copy of sc whose local lookups only see assignments before pos.
func (sc *scope) at(pos token.Pos) *scope {
	cp := *sc
	cp.limit = pos
	return &cp
}

// lookup resolves an identifier to the expression it was bound to.
func (sc *scope) lookup(name string) (bound, bool) {
	if b, ok := sc.params[name]; ok {
		return b, true
	}
	if vals, ok := sc.locals[name]; ok {
		var best *localValue
		for i := range vals {
			if sc.limit == token.NoPos || vals[i].pos < sc.limit {
				best = &vals[i]
			}
		}
		if best != nil {
			return bound{expr: best.expr, sc: sc.at(best.pos)}, true
		}
	}
	if expr, ok := sc.pkg.values[name]; ok {
		return bound{expr: expr, sc: &scope{pkg: sc.pkg, locals: map[string][]localValue{}, params: map[string]bound{}}}, true
	}
	return bound{}, false
}

// resolveString evaluates a string-valued expression built from literals,
// constants, local variables and "+" concatenation.
func resolveString(b bound, depth int) (string, bool) {
	if depth > maxResolveDepth || b.expr == nil {
		return "", false
	}
	switch e := b.expr.(type) {
	case *ast.BasicLit:
		if e.Kind != token.STRING {
			return "", false
		}
		s, err := strconv.Unquote(e.Value)
		return s, err == nil
	case *ast.ParenExpr:
		return resolveString(bound{e.X, b.sc}, depth+1)
	case *ast.BinaryExpr:
		if e.Op != token.ADD {
			return "", false
		}
		left, ok := resolveString(bound{e.X, b.sc}, depth+1)
		if !ok {
			return "", false
		}
		right, ok := resolveString(bound{e.Y, b.sc}, depth+1)
		return left + right, ok
	case *ast.Ident:
		next, ok := b.sc.lookup(e.Name)
		if !ok {
			return "", false
		}
		return resolveString(next, depth+1)
	case *ast.CallExpr:
		// string(x) conversions.
		if id, ok := e.Fun.(*ast.Ident); ok && id.Name == "string" && len(e.Args) == 1 {
			return resolveString(bound{e.Args[0], b.sc}, depth+1)
		}
	}
	return "", false
}

// resolveInt evaluates an integer literal, constant or local.
func resolveInt(b bound, depth int) (int, bool) {
	if depth > maxResolveDepth || b.expr == nil {
		return 0, false
	}
	switch e := b.expr.(type) {
	case *ast.BasicLit:
		if e.Kind != token.INT {
			return 0, false
		}
		n, err := strconv.Atoi(e.Value)
		return n, err == nil
	case *ast.ParenExpr:
		return resolveInt(bound{e.X, b.sc}, depth+1)
	case *ast.Ident:
		next, ok := b.sc.lookup(e.Name)
		if !ok {
			return 0, false
		}
		return resolveInt(next, depth+1)
	}
	return 0, false
}

// calleeName returns the function name of a call ("Builder" for both
// Builder(...) and pkg.Builder(...)).
func calleeName(call *ast.CallExpr) string {
	switch f := call.Fun.(type) {
	case *ast.Ident:
		return f.Name
	case *ast.SelectorExpr:
		return f.Sel.Name
	}
	return ""
}

// compositeFields maps the keys of a keyed composite literal to their values.
func compositeFields(lit *ast.CompositeLit) map[string]ast.Expr {
	out := map[string]ast.Expr{}
	for _, elt := range lit.Elts {
		kv, ok := elt.(*ast.KeyValueExpr)
		if !ok {
			continue
		}
		if key, ok := kv.Key.(*ast.Ident); ok {
			out[key.Name] = kv.Value
		}
	}
	return out
}

// compositeOf unwraps &T{...} and T{...}, following identifiers.
func compositeOf(b bound, depth int) (*ast.CompositeLit, *scope, bool) {
	if depth > maxResolveDepth || b.expr == nil {
		return nil, nil, false
	}
	switch e := b.expr.(type) {
	case *ast.CompositeLit:
		return e, b.sc, true
	case *ast.UnaryExpr:
		if e.Op == token.AND {
			return compositeOf(bound{e.X, b.sc}, depth+1)
		}
	case *ast.ParenExpr:
		return compositeOf(bound{e.X, b.sc}, depth+1)
	case *ast.Ident:
		if next, ok := b.sc.lookup(e.Name); ok {
			return compositeOf(next, depth+1)
		}
	}
	return nil, nil, false
}

// typeName renders the type of a composite literal ("config.Adapter").
func typeName(expr ast.Expr) string {
	switch t := expr.(type) {
	case *ast.Ident:
		return t.Name
	case *ast.SelectorExpr:
		if x, ok := t.X.(*ast.Ident); ok {
			return x.Name + "." + t.Sel.Name
		}
		return t.Sel.Name
	case *ast.StarExpr:
		return typeName(t.X)
	case *ast.MapType:
		return "map[" + typeName(t.Key) + "]" + typeName(t.Value)
	}
	return ""
}

// exprString renders an expression for problem reports.
func exprString(fset *token.FileSet, expr ast.Expr) string {
	if expr == nil {
		return "<nil>"
	}
	pos := fset.Position(expr.Pos())
	return fmt.Sprintf("%T at %s:%d", expr, pos.Filename, pos.Line)
}
