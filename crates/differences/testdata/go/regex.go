package main

import "regexp"

type regexMatch struct {
	Input string `json:"input"`
	Match bool   `json:"match"`
}

// regexCase is one pattern: regexp.Compile's error text, or the verdict on each input.
type regexCase struct {
	Pattern    string       `json:"pattern"`
	CompileErr string       `json:"compile_err"`
	Matches    []regexMatch `json:"matches"`
}

type regexDoc struct {
	Header header      `json:"header"`
	Cases  []regexCase `json:"cases"`
}

// regexGolden compiles patterns as Prebid's code and the bidder-params schemas do
// (regexp.MustCompile at package level, and gojsonschema's pattern keyword on regexp.Compile).
func regexGolden() regexDoc {
	doc := regexDoc{Header: newHeader()}
	for _, c := range []struct {
		pattern string
		inputs  []string
	}{
		{`^\d+$`, []string{"123", "\u0663\u0664"}},
		{`^\w+$`, []string{"abc_1", "\u00e9", "\u65e5\u672c"}},
		{`^\s$`, []string{" ", "\t", "\u00a0", "\u2003"}},
		{`\bfoo\b`, []string{"a foo b", "\u00e9foo\u00e9"}},
		{`\Qa.b\E`, []string{"a.b", "axb"}},
		{`^(?-u:\d)+$`, []string{"123"}},
	} {
		rc := regexCase{Pattern: c.pattern}
		re, err := regexp.Compile(c.pattern)
		if err != nil {
			rc.CompileErr = err.Error()
		} else {
			for _, input := range c.inputs {
				rc.Matches = append(rc.Matches, regexMatch{Input: input, Match: re.MatchString(input)})
			}
		}
		doc.Cases = append(doc.Cases, rc)
	}
	return doc
}
