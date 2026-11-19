package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"text/template"

	"github.com/prebid/prebid-server/v4/config"
	"github.com/prebid/prebid-server/v4/macros"
)

// upstreamRoot is the Go server's checkout, relative to this module.
const upstreamRoot = "../../../../.upstream"

// target is what net/http writes for one URL: http.NewRequest, then Request.Write (the request
// line and Host header the Transport writes for a request that does not go through a proxy).
type target struct {
	URI string `json:"uri"`
	// Target is the request-target of the request line and Host the Host header's value; Error
	// is the http.NewRequest or Request.Write error, when there is one.
	Target string `json:"target"`
	Host   string `json:"host"`
	Error  string `json:"error,omitempty"`
}

// endpointTarget is a bidder-info endpoint template rendered with the values config.go
// validates endpoints with (config/bidderinfo.go:478-488).
type endpointTarget struct {
	Bidder   string `json:"bidder"`
	Template string `json:"template"`
	target
}

type targetsDoc struct {
	Header    header           `json:"header"`
	Fixtures  []target         `json:"fixtures"`
	Endpoints []endpointTarget `json:"endpoints"`
}

// writeTarget fills in what net/http writes for t.URI.
func writeTarget(uri string) target {
	t := target{URI: uri}
	req, err := http.NewRequest(http.MethodPost, uri, nil)
	if err != nil {
		t.Error = err.Error()
		return t
	}
	var buf bytes.Buffer
	if err := req.Write(&buf); err != nil {
		t.Error = err.Error()
		return t
	}
	r := bufio.NewReader(&buf)
	line, err := r.ReadString('\n')
	if err != nil {
		panic(err)
	}
	method, rest, _ := strings.Cut(strings.TrimRight(line, "\r\n"), " ")
	if method != http.MethodPost {
		panic("unexpected request line " + line)
	}
	t.Target = strings.TrimSuffix(rest, " HTTP/1.1")
	hostLine, err := r.ReadString('\n')
	if err != nil {
		panic(err)
	}
	t.Host = strings.TrimPrefix(strings.TrimRight(hostLine, "\r\n"), "Host: ")
	return t
}

// fixtureURIs lists every expected request URI of the adapter JSON fixtures, sorted, once each.
func fixtureURIs() []string {
	seen := map[string]bool{}
	root := filepath.Join(upstreamRoot, "adapters")
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".json") || !strings.Contains(filepath.ToSlash(path), "test/") {
			return nil
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		var fixture struct {
			HTTPCalls []struct {
				ExpectedRequest struct {
					URI *string `json:"uri"`
				} `json:"expectedRequest"`
			} `json:"httpCalls"`
		}
		if json.Unmarshal(data, &fixture) != nil {
			return nil
		}
		for _, call := range fixture.HTTPCalls {
			if call.ExpectedRequest.URI != nil {
				seen[*call.ExpectedRequest.URI] = true
			}
		}
		return nil
	})
	if err != nil {
		panic(err)
	}
	uris := make([]string, 0, len(seen))
	for uri := range seen {
		uris = append(uris, uri)
	}
	sort.Strings(uris)
	return uris
}

// testEndpointTemplateParams are config.go's validation values (config/bidderinfo.go:478-488).
var testEndpointTemplateParams = macros.EndpointTemplateParams{
	Host:        "anyHost",
	PublisherID: "anyPublisherID",
	AccountID:   "anyAccountID",
	ZoneID:      "anyZoneID",
	SourceId:    "anySourceID",
	AdUnit:      "anyAdUnit",
	MediaType:   "MediaType",
	Region:      "anyRegion",
	PartnerId:   "anyPartnerId",
}

func targetsGolden() targetsDoc {
	doc := targetsDoc{Header: newHeader()}
	for _, uri := range fixtureURIs() {
		doc.Fixtures = append(doc.Fixtures, writeTarget(uri))
	}
	infos, err := config.LoadBidderInfoFromDisk(filepath.Join(upstreamRoot, "static", "bidder-info"))
	if err != nil {
		panic(err)
	}
	bidders := make([]string, 0, len(infos))
	for name := range infos {
		bidders = append(bidders, name)
	}
	sort.Strings(bidders)
	for _, name := range bidders {
		endpoint := infos[name].Endpoint
		if endpoint == "" {
			continue
		}
		tmpl, err := template.New("endpointTemplate").Parse(endpoint)
		if err != nil {
			panic(err)
		}
		rendered, err := macros.ResolveMacros(tmpl, testEndpointTemplateParams)
		if err != nil {
			panic(err)
		}
		doc.Endpoints = append(doc.Endpoints, endpointTarget{Bidder: name, Template: endpoint, target: writeTarget(rendered)})
	}
	return doc
}
