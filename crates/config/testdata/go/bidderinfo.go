package main

import (
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// bidderInfoFile is one static/bidder-info file as yaml.v3 reads it untyped.
type bidderInfoFile struct {
	File  string `json:"file"`
	Value any    `json:"value"`
}

// bidderInfoDoc is bidder_info.json.
type bidderInfoDoc struct {
	Header header           `json:"header"`
	Notes  []string         `json:"notes"`
	Files  []bidderInfoFile `json:"files"`
}

func bidderInfoGolden() bidderInfoDoc {
	dir := filepath.Join(upstreamDir, "static", "bidder-info")
	entries, err := os.ReadDir(dir)
	must(err)
	doc := bidderInfoDoc{
		Header: newHeader("gopkg.in/yaml.v3"),
		Notes: []string{
			"Written by crates/config/testdata/go (go run .); do not edit.",
			"Every static/bidder-info/*.yaml file of the pinned upstream, unmarshalled by yaml.v3 into interface{}, in the compact form of encode.go.",
		},
	}
	var names []string
	for _, e := range entries {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".yaml") {
			names = append(names, e.Name())
		}
	}
	sort.Strings(names)
	for _, name := range names {
		data, err := os.ReadFile(filepath.Join(dir, name))
		must(err)
		var v any
		must(yaml.Unmarshal(data, &v))
		doc.Files = append(doc.Files, bidderInfoFile{File: name, Value: dyn(v)})
	}
	return doc
}
