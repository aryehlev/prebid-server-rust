package main

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// The test vectors of Go's own compress/flate and compress/gzip tests, read from their source
// in GOROOT so that they stay those of the toolchain that runs the generator.

// copyGoTestdata copies Go's compress/flate testdata (the inputs and expected outputs of
// TestWriteBlock, TestWriteBlockDynamic and TestBlockHuff) to ../flate, and the gettysburg and
// issue 6550 inputs to ../corpus.
func copyGoTestdata(goroot, dir string) {
	src := filepath.Join(goroot, "src", "compress", "flate", "testdata")
	entries, err := os.ReadDir(src)
	if err != nil {
		panic(err)
	}
	for _, e := range entries {
		data, err := os.ReadFile(filepath.Join(src, e.Name()))
		if err != nil {
			panic(err)
		}
		writeFile(filepath.Join(dir, "flate", e.Name()), data)
	}
	data, err := os.ReadFile(filepath.Join(goroot, "src", "compress", "testdata", "gettysburg.txt"))
	if err != nil {
		panic(err)
	}
	writeFile(filepath.Join(corpusDir, "gettysburg.txt"), data)
	data, err = os.ReadFile(filepath.Join(goroot, "src", "compress", "gzip", "testdata", "issue6550.gz.base64"))
	if err != nil {
		panic(err)
	}
	gz, err := base64.StdEncoding.DecodeString(string(data))
	if err != nil {
		panic(err)
	}
	writeFile(filepath.Join(corpusDir, "issue6550.gz"), gz)
}

func parseGoTest(goroot, rel string) *ast.File {
	f, err := parser.ParseFile(token.NewFileSet(), filepath.Join(goroot, "src", rel), nil, 0)
	if err != nil {
		panic(err)
	}
	return f
}

// topValue returns the value of the package-level var or const name.
func topValue(f *ast.File, name string) ast.Expr {
	for _, d := range f.Decls {
		if g, ok := d.(*ast.GenDecl); ok {
			if v := specValue(g, name); v != nil {
				return v
			}
		}
	}
	panic("no package-level " + name)
}

func specValue(g *ast.GenDecl, name string) ast.Expr {
	for _, s := range g.Specs {
		if vs, ok := s.(*ast.ValueSpec); ok {
			for i, n := range vs.Names {
				if n.Name == name && i < len(vs.Values) {
					return vs.Values[i]
				}
			}
		}
	}
	return nil
}

// localValue returns the value assigned or declared to name in the body of function fn.
func localValue(f *ast.File, fn, name string) ast.Expr {
	for _, d := range f.Decls {
		fd, ok := d.(*ast.FuncDecl)
		if !ok || fd.Name.Name != fn {
			continue
		}
		for _, stmt := range fd.Body.List {
			switch s := stmt.(type) {
			case *ast.AssignStmt:
				if id, ok := s.Lhs[0].(*ast.Ident); ok && id.Name == name {
					return s.Rhs[0]
				}
			case *ast.DeclStmt:
				if g, ok := s.Decl.(*ast.GenDecl); ok {
					if v := specValue(g, name); v != nil {
						return v
					}
				}
			}
		}
	}
	panic("no " + name + " in " + fn)
}

func stringValue(e ast.Expr) string {
	switch e := e.(type) {
	case *ast.BasicLit:
		if e.Kind == token.STRING {
			s, err := strconv.Unquote(e.Value)
			if err != nil {
				panic(err)
			}
			return s
		}
	case *ast.BinaryExpr:
		if e.Op == token.ADD {
			return stringValue(e.X) + stringValue(e.Y)
		}
	}
	panic(fmt.Sprintf("not a string constant: %T", e))
}

// intValue evaluates an integer literal or one of the named constants.
func intValue(e ast.Expr, consts map[string]uint64) uint64 {
	switch e := e.(type) {
	case *ast.BasicLit:
		switch e.Kind {
		case token.INT:
			n, err := strconv.ParseUint(e.Value, 0, 64)
			if err != nil {
				panic(err)
			}
			return n
		case token.CHAR:
			r, _, tail, err := strconv.UnquoteChar(e.Value[1:len(e.Value)-1], '\'')
			if err != nil || tail != "" {
				panic(fmt.Sprintf("bad character literal %s", e.Value))
			}
			return uint64(r)
		}
	case *ast.Ident:
		if n, ok := consts[e.Name]; ok {
			return n
		}
	}
	panic(fmt.Sprintf("not an integer constant: %T", e))
}

// bytesValue evaluates []byte{...} or []byte("...").
func bytesValue(e ast.Expr, consts map[string]uint64) []byte {
	switch e := e.(type) {
	case *ast.CompositeLit:
		b := []byte{}
		for _, elt := range e.Elts {
			b = append(b, byte(intValue(elt, consts)))
		}
		return b
	case *ast.CallExpr:
		if len(e.Args) == 1 {
			return []byte(stringValue(e.Args[0]))
		}
	}
	panic(fmt.Sprintf("not a byte slice constant: %T", e))
}

// field returns the element of a struct literal by key, or else by position.
func field(e ast.Expr, key string, pos int) ast.Expr {
	lit := e.(*ast.CompositeLit)
	for _, elt := range lit.Elts {
		if kv, ok := elt.(*ast.KeyValueExpr); ok {
			if kv.Key.(*ast.Ident).Name == key {
				return kv.Value
			}
		}
	}
	if pos >= 0 && pos < len(lit.Elts) {
		if _, ok := lit.Elts[pos].(*ast.KeyValueExpr); !ok {
			return lit.Elts[pos]
		}
	}
	return nil
}

// goVector is a stream from a Go test.
type goVector struct {
	name   string
	stream []byte
}

// readerArg returns the argument of the first strings.NewReader call in function fn.
func readerArg(f *ast.File, fn string) ast.Expr {
	var arg ast.Expr
	for _, d := range f.Decls {
		if fd, ok := d.(*ast.FuncDecl); ok && fd.Name.Name == fn {
			ast.Inspect(fd.Body, func(n ast.Node) bool {
				if call, ok := n.(*ast.CallExpr); ok && arg == nil {
					if sel, ok := call.Fun.(*ast.SelectorExpr); ok && sel.Sel.Name == "NewReader" && len(call.Args) == 1 {
						if _, ok := call.Args[0].(*ast.CallExpr); !ok {
							arg = call.Args[0]
						}
					}
				}
				return arg == nil
			})
		}
	}
	if arg == nil {
		panic("no strings.NewReader call in " + fn)
	}
	return arg
}

// goFlateVectors returns the streams of flate's TestStreams, TestReaderTruncated and
// TestNlitOutOfRange, and the truncations of its TestTruncatedStreams.
func goFlateVectors(goroot string) []goVector {
	f := parseGoTest(goroot, "compress/flate/flate_test.go")
	var vs []goVector
	for _, tc := range localValue(f, "TestStreams", "testCases").(*ast.CompositeLit).Elts {
		stream, err := hex.DecodeString(stringValue(field(tc, "stream", 1)))
		if err != nil {
			panic(err)
		}
		vs = append(vs, goVector{"TestStreams/" + stringValue(field(tc, "desc", 0)), stream})
	}
	data := stringValue(localValue(f, "TestTruncatedStreams", "data"))
	for i := 0; i < len(data)-1; i++ {
		vs = append(vs, goVector{fmt.Sprintf("TestTruncatedStreams/%d", i), []byte(data[:i])})
	}
	inflate := parseGoTest(goroot, "compress/flate/inflate_test.go")
	for i, v := range localValue(inflate, "TestReaderTruncated", "vectors").(*ast.CompositeLit).Elts {
		vs = append(vs, goVector{fmt.Sprintf("TestReaderTruncated/%d", i), []byte(stringValue(field(v, "input", 0)))})
	}
	reader := parseGoTest(goroot, "compress/flate/reader_test.go")
	vs = append(vs, goVector{"TestNlitOutOfRange", []byte(stringValue(readerArg(reader, "TestNlitOutOfRange")))})
	return vs
}

// goGzipVectors returns the streams of gzip's gunzipTests and the truncations of its
// TestTruncatedStreams.
func goGzipVectors(goroot string) []goVector {
	f := parseGoTest(goroot, "compress/gzip/gunzip_test.go")
	// The table uses the package's header constants.
	pkg := parseGoTest(goroot, "compress/gzip/gunzip.go")
	consts := map[string]uint64{}
	for _, name := range []string{"gzipID1", "gzipID2", "gzipDeflate"} {
		consts[name] = intValue(topValue(pkg, name), nil)
	}
	var vs []goVector
	for _, tt := range topValue(f, "gunzipTests").(*ast.CompositeLit).Elts {
		vs = append(vs, goVector{"gunzipTests/" + stringValue(field(tt, "desc", 1)), bytesValue(field(tt, "gzip", 3), consts)})
	}
	for _, tc := range localValue(f, "TestTruncatedStreams", "cases").(*ast.CompositeLit).Elts {
		name := stringValue(field(tc, "name", 0))
		data := bytesValue(field(tc, "data", 1), consts)
		for i := 1; i < len(data); i++ {
			vs = append(vs, goVector{fmt.Sprintf("TestTruncatedStreams/%s-%d", name, i), data[:i]})
		}
	}
	return vs
}

// writeBlocks records the tokens of huffman_bit_writer_test.go's writeBlockTests, the input of
// TestWriteBlock and TestWriteBlockDynamic, whose expected outputs are the files in ../flate:
// one line per test with the input file, the expected file patterns (%s is wb or dyn, - for
// none) and the tokens in hexadecimal (runs of a token as token*count).
func writeBlocks(dir, goroot string) {
	f := parseGoTest(goroot, "compress/flate/huffman_bit_writer_test.go")
	consts := map[string]uint64{"ml": intValue(topValue(f, "ml"), nil)}
	g := newGolden(dir, "flate_blocks.txt")
	defer g.close()
	base := func(e ast.Expr) string {
		if e == nil {
			return "-"
		}
		s := stringValue(e)
		if s == "" {
			return "-"
		}
		return strings.TrimPrefix(s, "testdata/")
	}
	for _, tt := range topValue(f, "writeBlockTests").(*ast.CompositeLit).Elts {
		var tokens []string
		for _, t := range field(tt, "tokens", -1).(*ast.CompositeLit).Elts {
			tokens = append(tokens, strconv.FormatUint(intValue(t, consts), 16))
		}
		g.line(base(field(tt, "input", -1)), base(field(tt, "want", -1)), base(field(tt, "wantNoInput", -1)), rle(tokens))
	}
}
