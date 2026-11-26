package main

import (
	"bytes"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"text/template"
	"unicode/utf8"

	"gopkg.in/yaml.v3"
)

// corpus is every input of validator.txt, sorted and without duplicates. The Rust functions
// take &str, so only valid UTF-8 is kept. The inputs:
//
//   - the URL-like strings of static/bidder-info, and every endpoint resolved as Prebid Server
//     resolves it before it validates it (bidderInfoStrings);
//   - the URL-like and IP-like string literals of the upstream config, usersync, floors and
//     macros packages and their tests;
//   - the request URIs and device IPs of the adapters' JSON test fixtures;
//   - the inputs of govalidator's tests of IsURL, IsRequestURL, IsRequestURI and IsIP;
//   - the string literals of Go's net/url, net and net/netip tests;
//   - edge cases at IsURL's limits and of IP syntax.
func corpus(upstream string) []string {
	found := map[string]bool{}
	add := func(values []string) {
		for _, s := range values {
			if utf8.ValidString(s) {
				found[s] = true
			}
		}
	}
	add(bidderInfoStrings(upstream))
	for _, dir := range []string{"config", "usersync", "floors", "macros"} {
		add(goLiterals(filepath.Join(upstream, dir), urlOrIPLike))
	}
	add(fixtureStrings(filepath.Join(upstream, "adapters")))
	add(govalidatorTestInputs())
	goroot := filepath.Join(goCmd("env", "GOROOT"), "src")
	for _, file := range []string{"net/url/url_test.go", "net/ip_test.go", "net/netip/netip_test.go"} {
		add(fileLiterals(filepath.Join(goroot, file), func(s string) bool { return len(s) <= 256 }))
	}
	add(edgeCases())
	out := make([]string, 0, len(found))
	for s := range found {
		out = append(out, s)
	}
	sort.Strings(out)
	return out
}

// bidderInfoStrings are the strings of static/bidder-info that hold "://" or a template action,
// and each endpoint resolved as config/bidderinfo.go's validateAdapterEndpoint resolves it
// before it calls IsURL and IsRequestURL.
func bidderInfoStrings(upstream string) []string {
	files, err := filepath.Glob(filepath.Join(upstream, "static", "bidder-info", "*.yaml"))
	if err != nil {
		log.Fatal(err)
	}
	if len(files) == 0 {
		log.Fatalf("no bidder-info files under %s", upstream)
	}
	sort.Strings(files)
	params := endpointParams(upstream)
	var out []string
	for _, file := range files {
		src, err := os.ReadFile(file)
		if err != nil {
			log.Fatal(err)
		}
		var doc any
		if err := yaml.Unmarshal(src, &doc); err != nil {
			log.Fatalf("%s: %v", file, err)
		}
		walk(doc, "", func(key, value string) {
			if strings.Contains(value, "://") || strings.Contains(value, "{{") {
				out = append(out, value)
			}
			if key == "endpoint" {
				if resolved, ok := resolveEndpoint(value, params); ok {
					out = append(out, resolved)
				}
			}
		})
	}
	return out
}

// walk calls f with every string of a decoded YAML or JSON document and the key that holds it.
func walk(v any, key string, f func(key, value string)) {
	switch v := v.(type) {
	case string:
		f(key, v)
	case map[string]any:
		keys := make([]string, 0, len(v))
		for k := range v {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			walk(v[k], k, f)
		}
	case []any:
		for _, e := range v {
			walk(e, key, f)
		}
	}
}

// endpointParams is config/bidderinfo.go's testEndpointTemplateParams, the macro values that
// validateAdapterEndpoint resolves endpoints with, as a value of a struct type with the fields
// of macros.EndpointTemplateParams. Both come from the upstream source, so a template resolves
// as it does in Prebid Server: an unknown field fails and an unset one is empty.
func endpointParams(upstream string) any {
	names := structFields(filepath.Join(upstream, "macros", "macros.go"), "EndpointTemplateParams")
	fields := make([]reflect.StructField, 0, len(names))
	for _, name := range names {
		fields = append(fields, reflect.StructField{Name: name, Type: reflect.TypeOf("")})
	}
	v := reflect.New(reflect.StructOf(fields)).Elem()
	values := compositeStrings(filepath.Join(upstream, "config", "bidderinfo.go"), "testEndpointTemplateParams")
	for name, value := range values {
		field := v.FieldByName(name)
		if !field.IsValid() {
			log.Fatalf("testEndpointTemplateParams sets %s, which EndpointTemplateParams does not have", name)
		}
		field.SetString(value)
	}
	return v.Interface()
}

// resolveEndpoint parses an endpoint as validateAdapterEndpoint does and executes it as
// macros.ResolveMacros does; false when either fails.
func resolveEndpoint(endpoint string, params any) (string, bool) {
	t, err := template.New("endpointTemplate").Parse(endpoint)
	if err != nil {
		return "", false
	}
	var b bytes.Buffer
	if err := t.Execute(&b, params); err != nil {
		return "", false
	}
	return b.String(), true
}

func parseFile(file string) *ast.File {
	f, err := parser.ParseFile(token.NewFileSet(), file, nil, 0)
	if err != nil {
		log.Fatal(err)
	}
	return f
}

// structFields are the field names of the struct type `name` declared in a Go file, all of
// which must be strings.
func structFields(file, name string) []string {
	var names []string
	found := false
	ast.Inspect(parseFile(file), func(n ast.Node) bool {
		spec, ok := n.(*ast.TypeSpec)
		if !ok || spec.Name.Name != name {
			return true
		}
		st, ok := spec.Type.(*ast.StructType)
		if !ok {
			log.Fatalf("%s: %s is not a struct", file, name)
		}
		found = true
		for _, field := range st.Fields.List {
			if id, ok := field.Type.(*ast.Ident); !ok || id.Name != "string" {
				log.Fatalf("%s: a field of %s is not a string", file, name)
			}
			for _, id := range field.Names {
				names = append(names, id.Name)
			}
		}
		return false
	})
	if !found || len(names) == 0 {
		log.Fatalf("%s: no struct %s", file, name)
	}
	return names
}

// compositeStrings are the string fields that the composite literal of `var name = T{...}` in
// a Go file sets.
func compositeStrings(file, name string) map[string]string {
	values := map[string]string{}
	ast.Inspect(parseFile(file), func(n ast.Node) bool {
		spec, ok := n.(*ast.ValueSpec)
		if !ok {
			return true
		}
		for i, id := range spec.Names {
			if id.Name != name || i >= len(spec.Values) {
				continue
			}
			lit, ok := spec.Values[i].(*ast.CompositeLit)
			if !ok {
				log.Fatalf("%s: %s is not a composite literal", file, name)
			}
			for _, e := range lit.Elts {
				kv, ok := e.(*ast.KeyValueExpr)
				if !ok {
					log.Fatalf("%s: %s has an element without a key", file, name)
				}
				key, ok := kv.Key.(*ast.Ident)
				val, isLit := kv.Value.(*ast.BasicLit)
				if !ok || !isLit || val.Kind != token.STRING {
					log.Fatalf("%s: %s sets a field to something other than a string", file, name)
				}
				s, err := strconv.Unquote(val.Value)
				if err != nil {
					log.Fatal(err)
				}
				values[key.Name] = s
			}
		}
		return false
	})
	if len(values) == 0 {
		log.Fatalf("%s: no values in %s", file, name)
	}
	return values
}

// fileLiterals are the string literals of a Go file that keep holds for.
func fileLiterals(file string, keep func(string) bool) []string {
	var out []string
	ast.Inspect(parseFile(file), func(n ast.Node) bool {
		if lit, ok := n.(*ast.BasicLit); ok && lit.Kind == token.STRING {
			if s, err := strconv.Unquote(lit.Value); err == nil && keep(s) {
				out = append(out, s)
			}
		}
		return true
	})
	return out
}

// goLiterals are the string literals of the Go files under dir that keep holds for.
func goLiterals(dir string, keep func(string) bool) []string {
	var out []string
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() && strings.HasSuffix(path, ".go") {
			out = append(out, fileLiterals(path, keep)...)
		}
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	if len(out) == 0 {
		log.Fatalf("no literals under %s", dir)
	}
	return out
}

// urlOrIPLike holds for a literal that holds "://", or that is made of hex digits, dots,
// colons and percent signs with at least one dot or colon, as IP addresses are.
func urlOrIPLike(s string) bool {
	if strings.Contains(s, "://") {
		return true
	}
	if len(s) > 64 || !strings.ContainsAny(s, ".:") {
		return false
	}
	for _, c := range s {
		if !strings.ContainsRune("0123456789abcdefABCDEF.:%", c) {
			return false
		}
	}
	return true
}

// fixtureStrings are the "uri", "ip" and "ipv6" strings of the adapters' JSON test fixtures:
// the URLs adapters send requests to and the device addresses they forward.
func fixtureStrings(dir string) []string {
	keys := map[string]bool{"uri": true, "ip": true, "ipv6": true}
	var out []string
	err := filepath.WalkDir(dir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".json") {
			return nil
		}
		src, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		var doc any
		if json.Unmarshal(src, &doc) != nil {
			return nil
		}
		walk(doc, "", func(key, value string) {
			if keys[key] {
				out = append(out, value)
			}
		})
		return nil
	})
	if err != nil {
		log.Fatal(err)
	}
	if len(out) == 0 {
		log.Fatalf("no fixture strings under %s", dir)
	}
	return out
}

// govalidatorTestInputs are the string literals of govalidator's tests of the functions the
// crate ports.
func govalidatorTestInputs() []string {
	dir := goCmd("list", "-m", "-f", "{{.Dir}}", "github.com/asaskevich/govalidator")
	tests := map[string]bool{"TestIsURL": true, "TestIsRequestURL": true, "TestIsRequestURI": true, "TestIsIP": true}
	var out []string
	for _, decl := range parseFile(filepath.Join(dir, "validator_test.go")).Decls {
		fn, ok := decl.(*ast.FuncDecl)
		if !ok || !tests[fn.Name.Name] {
			continue
		}
		delete(tests, fn.Name.Name)
		ast.Inspect(fn, func(n ast.Node) bool {
			if lit, ok := n.(*ast.BasicLit); ok && lit.Kind == token.STRING {
				if s, err := strconv.Unquote(lit.Value); err == nil {
					out = append(out, s)
				}
			}
			return true
		})
	}
	if len(tests) != 0 {
		log.Fatalf("validator_test.go has no %v", tests)
	}
	return out
}

// edgeCases are inputs at IsURL's limits (maxURLRuneCount is 2083 runes and minURLRuneCount 3
// bytes, so lengths around both in ASCII and in two-byte runes), around its other checks (a
// leading dot, a colon without "://", the host and path url.Parse finds) and of IP syntax.
func edgeCases() []string {
	out := []string{
		"", "a", "ab", "abc", "abcd", "a.b", "a.bc", "a.b.c", ".ab", ".abc", ".a.com", "x.co",
		"日本", "日本.jp", "é.fr", ":", "::", ":80", "a:", "a:80", "a.com:80", "a.com:65535",
		"a.com:65536", "a.com:999999", "a.com:", "a.com:x", "http:", "http:/", "http://",
		"http:///", "http:///a", "http://a", "http://a.", "http://.a", "http://..", "http://a..b",
		"http://-a.com", "http://a-.com", "http://a_b.com", "http://[::1]", "http://[::1]:80/",
		"http://[fe80::1%25en0]/", "http://[1.2.3.4]/", "http://1.2.3.4/", "http://1.2.3.256/",
		"http://0.0.0.0/", "http://224.0.0.1/", "http://user:pass@a.com/", "http://user@a.com/",
		"http://@a.com/", "http://a.com/a b", "http://a b.com/", "http://a.com/%zz",
		"http://a.com/%20", "http://a.com?x=y", "http://a.com#f", "//a.com/x", "/a/b", "/a.b",
		"a/b", "a.b/c", "*", "mailto:a@b.com", "ftp://a.com/", "tcp://a.com:1/", "wss://a.com/",
		"gopher://a.com/", "HTTP://A.COM/", "http://a.com\n", " http://a.com",
		"http://{{.Host}}/", "http://anyHost/", "https://www.example.com/?q=日本",
		"0.0.0.0", "255.255.255.255", "256.0.0.0", "01.2.3.4", "1.2.3", "1.2.3.4.5", "1..2.3",
		".1.2.3", "1.2.3.", "::", "::1", "1::", ":::", "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9",
		"1:2:3:4:5:6:7::", "::1:2:3:4:5:6:7", "1::2::3", "12345::", "::ffff:1.2.3.4",
		"::1.2.3.4", "1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4", "::ffff:1.2.3", "fe80::1%eth0",
		"fe80::1%", "%eth0", "1.2.3.4%eth0", "[::1]", "0x1.2.3.4", " 1.2.3.4", "1.2.3.4 ",
		"١.٢.٣.٤", "::FFFF:1.2.3.4", "0:0:0:0:0:0:0:0", "0000:0000:0000:0000:0000:0000:0000:0000",
		"00000::", "1:2:3:4:5:6:7:", ":1:2:3:4:5:6:7",
		// URL's \s, \S and \d are ASCII classes in Go: Unicode spaces and digits are not them.
		"http://a.com/\u00a0", "http://a.com/x\u3000y", "http://a.com/\u0085", "http://a\u00a0b.com/",
		"http://us\u2003er@a.com/", "http://us\u2003er:p\u00a0w@a.com/", "http://a.com:8\u0663/",
		"http://a.com:\u0663/", "\u2028a.com", "http://\u1680a.com/", "a.com/\u00a0",
		"http://a.com/\u200b", "http://\uff11.\uff12.\uff13.\uff14/", "http://\u0661\u0662.com/",
	}
	host := "http://example.com/"
	for _, n := range []int{2081, 2082, 2083, 2084} {
		out = append(out, host+strings.Repeat("a", n-len(host)), host+strings.Repeat("é", n-len(host)))
	}
	return out
}
