package corpus

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"strconv"
)

// Go test files that define the stored data the endpoint JSON tests serve.
const (
	endpointTestUtilsFile = RootEndpointsOpenRTB2 + "/test_utils.go"
	videoAuctionTestFile  = RootEndpointsOpenRTB2 + "/video_auction_test.go"
)

// endpointStored is the stored data the endpoints/openrtb2 JSON tests serve,
// read from the Go test sources so the corpus never forks them.
type endpointStored struct {
	// Auction is what mockStoredReqFetcher and the buildTestEndpoint
	// mockAccountFetcher serve to /openrtb2/auction and /openrtb2/amp tests.
	Auction Stored
	// Video is what the video tests' mockVideoStoredReqFetcher and account
	// fetcher serve.
	Video Stored
}

// loadEndpointStored extracts the map[string]json.RawMessage literals of the
// endpoint tests with go/ast.
func loadEndpointStored(repo *Repo) (endpointStored, error) {
	var out endpointStored
	utils, err := parseGoFile(repo, endpointTestUtilsFile)
	if err != nil {
		return out, err
	}
	if out.Auction.Requests, err = rawMessageVar(utils, "testStoredRequestData"); err != nil {
		return out, err
	}
	if out.Auction.Imps, err = rawMessageVar(utils, "testStoredImpData"); err != nil {
		return out, err
	}
	if out.Auction.Accounts, err = accountFetcherData(utils, "buildTestEndpoint"); err != nil {
		return out, err
	}
	video, err := parseGoFile(repo, videoAuctionTestFile)
	if err != nil {
		return out, err
	}
	if out.Video.Requests, err = rawMessageVar(video, "testVideoStoredRequestData"); err != nil {
		return out, err
	}
	if out.Video.Imps, err = rawMessageVar(video, "testVideoStoredImpData"); err != nil {
		return out, err
	}
	if out.Video.Accounts, err = rawMessageVar(video, "mockVideoAccountData"); err != nil {
		return out, err
	}
	return out, nil
}

func parseGoFile(repo *Repo, rel string) (*ast.File, error) {
	src, err := repo.ReadFile(rel)
	if err != nil {
		return nil, err
	}
	file, err := parser.ParseFile(token.NewFileSet(), rel, src, parser.SkipObjectResolution)
	if err != nil {
		return nil, fmt.Errorf("corpus: parsing %s: %w", rel, err)
	}
	return file, nil
}

// rawMessageVar decodes the package-level "var name = map[string]json.RawMessage{...}".
func rawMessageVar(file *ast.File, name string) (map[string]json.RawMessage, error) {
	for _, decl := range file.Decls {
		gd, ok := decl.(*ast.GenDecl)
		if !ok || gd.Tok != token.VAR {
			continue
		}
		for _, spec := range gd.Specs {
			vs, ok := spec.(*ast.ValueSpec)
			if !ok || len(vs.Names) != 1 || vs.Names[0].Name != name || len(vs.Values) != 1 {
				continue
			}
			lit, ok := vs.Values[0].(*ast.CompositeLit)
			if !ok {
				return nil, fmt.Errorf("corpus: %s is not a composite literal", name)
			}
			return rawMessageMap(lit)
		}
	}
	return nil, fmt.Errorf("corpus: variable %s not found", name)
}

// accountFetcherData finds the mockAccountFetcher{data: ...} literal in fn.
func accountFetcherData(file *ast.File, fn string) (map[string]json.RawMessage, error) {
	var (
		found map[string]json.RawMessage
		ferr  error
	)
	for _, decl := range file.Decls {
		fd, ok := decl.(*ast.FuncDecl)
		if !ok || fd.Name.Name != fn || fd.Body == nil {
			continue
		}
		ast.Inspect(fd.Body, func(n ast.Node) bool {
			lit, ok := n.(*ast.CompositeLit)
			if !ok || found != nil || ferr != nil || typeName(lit.Type) != "mockAccountFetcher" {
				return found == nil && ferr == nil
			}
			data, ok := compositeFields(lit)["data"].(*ast.CompositeLit)
			if !ok {
				ferr = fmt.Errorf("corpus: %s: mockAccountFetcher.data is not a literal", fn)
				return false
			}
			found, ferr = rawMessageMap(data)
			return false
		})
	}
	if found == nil && ferr == nil {
		ferr = fmt.Errorf("corpus: mockAccountFetcher literal not found in %s", fn)
	}
	return found, ferr
}

// rawMessageMap decodes {"id": json.RawMessage(`...`), ...}.
func rawMessageMap(lit *ast.CompositeLit) (map[string]json.RawMessage, error) {
	out := make(map[string]json.RawMessage, len(lit.Elts))
	for _, elt := range lit.Elts {
		kv, ok := elt.(*ast.KeyValueExpr)
		if !ok {
			return nil, fmt.Errorf("corpus: unkeyed element in stored data literal")
		}
		key, err := stringLiteral(kv.Key)
		if err != nil {
			return nil, err
		}
		call, ok := kv.Value.(*ast.CallExpr)
		if !ok || len(call.Args) != 1 || calleeName(call) != "RawMessage" {
			return nil, fmt.Errorf("corpus: stored data %q is not json.RawMessage(literal)", key)
		}
		value, err := stringLiteral(call.Args[0])
		if err != nil {
			return nil, fmt.Errorf("corpus: stored data %q: %w", key, err)
		}
		out[key] = json.RawMessage(value)
	}
	return out, nil
}

func stringLiteral(expr ast.Expr) (string, error) {
	lit, ok := expr.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING {
		return "", fmt.Errorf("corpus: expected a string literal")
	}
	s, err := strconv.Unquote(lit.Value)
	if err != nil {
		return "", fmt.Errorf("corpus: unquoting %s: %w", lit.Value, err)
	}
	return s, nil
}

// copyStored returns a deep copy of s (the maps, not the bytes, are copied).
func copyStored(s Stored) Stored {
	cp := func(m map[string]json.RawMessage) map[string]json.RawMessage {
		if m == nil {
			return nil
		}
		out := make(map[string]json.RawMessage, len(m))
		for k, v := range m {
			out[k] = v
		}
		return out
	}
	return Stored{Requests: cp(s.Requests), Imps: cp(s.Imps), Responses: cp(s.Responses), Accounts: cp(s.Accounts)}
}
