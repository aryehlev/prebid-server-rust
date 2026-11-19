package main

import (
	"strconv"

	"gopkg.in/yaml.v3"
)

// yamlOutcome is what yaml.Unmarshal made of one scalar: the decoded value, or the error text.
type yamlOutcome struct {
	Input string `json:"input"`
	Value string `json:"value"`
	Err   string `json:"err"`
}

// yamlBoolSpellings are the YAML 1.1 and 1.2 boolean spellings in Go's cases and in mixed case,
// and the spellings strconv.ParseBool accepts.
var yamlBoolSpellings = []string{
	"y", "Y", "yes", "Yes", "YES", "yEs", "on", "On", "ON", "oN",
	"n", "N", "no", "No", "NO", "nO", "off", "Off", "OFF", "oFf",
	"true", "True", "TRUE", "tRue", "false", "False", "FALSE", "fAlse",
	"1", "0", "t", "f",
}

type yamlDoc struct {
	Header      header        `json:"header"`
	Bools       []yamlOutcome `json:"bools"`
	Underscored []yamlOutcome `json:"underscored"`
}

// yamlGolden decodes as config.processBidderInfos decodes the bidder-info files
// (config/bidderinfo.go:323, yaml.Unmarshal into config.BidderInfo, whose disabled and
// modifyingVastXmlAllowed fields are bools).
func yamlGolden() yamlDoc {
	doc := yamlDoc{Header: newHeader("gopkg.in/yaml.v3")}
	for _, s := range yamlBoolSpellings {
		var target struct {
			A bool `yaml:"a"`
		}
		out := yamlOutcome{Input: s}
		if err := yaml.Unmarshal([]byte("a: "+s+"\n"), &target); err != nil {
			out.Err = err.Error()
		} else {
			out.Value = strconv.FormatBool(target.A)
		}
		doc.Bools = append(doc.Bools, out)
	}
	for _, s := range []string{"1_000", "1_000_000", "-1_0"} {
		var target struct {
			N int64 `yaml:"n"`
		}
		out := yamlOutcome{Input: s}
		if err := yaml.Unmarshal([]byte("n: "+s+"\n"), &target); err != nil {
			out.Err = err.Error()
		} else {
			out.Value = strconv.FormatInt(target.N, 10)
		}
		doc.Underscored = append(doc.Underscored, out)
	}
	return doc
}
