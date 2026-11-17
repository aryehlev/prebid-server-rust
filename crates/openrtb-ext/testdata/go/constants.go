package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"log"
	"path/filepath"
	"sort"
	"strings"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
)

// untypedConstant is an exported constant declared without a type, with its value.
type untypedConstant struct {
	name  string
	value interface{}
}

// untypedConstants are the exported constants that openrtb_ext declares without a type (outside
// request_wrapper.go and the convert files, whose port is separate). untypedConstantRows checks
// the list against the package's source.
var untypedConstants = []untypedConstant{
	{"StoredRequestAttributes", openrtb_ext.StoredRequestAttributes},
	{"OriginalBidCpmKey", openrtb_ext.OriginalBidCpmKey},
	{"OriginalBidCurKey", openrtb_ext.OriginalBidCurKey},
	{"Passthrough", openrtb_ext.Passthrough},
	{"PrebidExtKey", openrtb_ext.PrebidExtKey},
	{"PrebidExtBidderKey", openrtb_ext.PrebidExtBidderKey},
	{"FetchSuccess", openrtb_ext.FetchSuccess},
	{"FetchTimeout", openrtb_ext.FetchTimeout},
	{"FetchError", openrtb_ext.FetchError},
	{"FetchInprogress", openrtb_ext.FetchInprogress},
	{"FetchNone", openrtb_ext.FetchNone},
	{"NoDataLocation", openrtb_ext.NoDataLocation},
	{"RequestLocation", openrtb_ext.RequestLocation},
	{"FetchLocation", openrtb_ext.FetchLocation},
	{"IsRewardedInventoryKey", openrtb_ext.IsRewardedInventoryKey},
	{"OptionsKey", openrtb_ext.OptionsKey},
	{"DefaultBidLimit", openrtb_ext.DefaultBidLimit},
	{"MaxBidLimit", openrtb_ext.MaxBidLimit},
	{"FirstPartyDataExtKey", openrtb_ext.FirstPartyDataExtKey},
	{"FirstPartyDataContextExtKey", openrtb_ext.FirstPartyDataContextExtKey},
	{"SKAdNExtKey", openrtb_ext.SKAdNExtKey},
	{"GPIDKey", openrtb_ext.GPIDKey},
	{"TIDKey", openrtb_ext.TIDKey},
	{"AuctionEnvironmentKey", openrtb_ext.AuctionEnvironmentKey},
	{"NativeExchangeSpecificLowerBound", openrtb_ext.NativeExchangeSpecificLowerBound},
}

// separatelyPorted are the files of openrtb_ext whose port is not pbs-openrtb-ext's.
var separatelyPorted = map[string]bool{
	"request_wrapper.go": true,
	"convert_up.go":      true,
	"convert_down.go":    true,
}

// untypedConstantRows are the name, Go type (of the constant's default type) and value of each
// untyped constant; it fails unless untypedConstants lists exactly the exported constants that
// the package's source declares without a type.
func untypedConstantRows() [][]string {
	paths, err := filepath.Glob(filepath.Join("openrtb_ext", "*.go"))
	if err != nil {
		log.Fatal(err)
	}
	fset := token.NewFileSet()
	var declared []string
	for _, path := range paths {
		if strings.HasSuffix(path, "_test.go") || separatelyPorted[filepath.Base(path)] {
			continue
		}
		file, err := parser.ParseFile(fset, path, nil, 0)
		if err != nil {
			log.Fatal(err)
		}
		for _, decl := range file.Decls {
			gen, ok := decl.(*ast.GenDecl)
			if !ok || gen.Tok != token.CONST {
				continue
			}
			// A spec without a type or values repeats the previous one (iota groups).
			typed := false
			for _, spec := range gen.Specs {
				vs := spec.(*ast.ValueSpec)
				if vs.Type != nil || len(vs.Values) > 0 {
					typed = vs.Type != nil
				}
				for _, name := range vs.Names {
					if name.IsExported() && !typed {
						declared = append(declared, name.Name)
					}
				}
			}
		}
	}
	var listed []string
	var rows [][]string
	for _, c := range untypedConstants {
		listed = append(listed, c.name)
		rows = append(rows, []string{"openrtb_ext." + c.name, fmt.Sprintf("%T", c.value), fmt.Sprint(c.value)})
	}
	sort.Strings(declared)
	sort.Strings(listed)
	if strings.Join(declared, ",") != strings.Join(listed, ",") {
		log.Fatalf("untyped constants are out of date: the source declares %v, the list has %v", declared, listed)
	}
	return rows
}

func writeConstants(path string, rows [][]string) {
	rw := newRowWriter(path)
	for _, r := range rows {
		rw.row(r...)
	}
	rw.close()
}

func writeFunctions(path string, calls []call) {
	rw := newRowWriter(path)
	for _, c := range calls {
		fields := append([]string{c.name, c.label}, c.args...)
		fields = append(fields, "->")
		rw.row(append(fields, c.results...)...)
	}
	rw.close()
}
