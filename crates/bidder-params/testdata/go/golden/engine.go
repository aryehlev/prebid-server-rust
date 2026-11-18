package main

import (
	"bufio"
	"encoding/hex"
	"fmt"
	"log"
	"os"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/xeipuuv/gojsonschema"
)

// engineCase is a hand-written schema with documents to validate against it. A body is wrapped
// in an object once per draft prefix (no $schema, draft-04, draft-06, draft-07).
type engineCase struct {
	name string
	// body is the schema without its braces; schema is a complete schema (used when set).
	body   string
	schema string
	docs   []string
}

var draftPrefixes = []struct {
	name   string
	prefix string
}{
	{"hybrid", ""},
	{"draft4", `"$schema":"http://json-schema.org/draft-04/schema#",`},
	{"draft6", `"$schema":"http://json-schema.org/draft-06/schema#",`},
	{"draft7", `"$schema":"http://json-schema.org/draft-07/schema#",`},
}

var anyValues = []string{"null", "true", "false", "0", "1", "1.5", "-2", `""`, `"s"`, "[]", "[1]", "{}", `{"a":1}`}

var engineCases = []engineCase{
	{name: "type integer", body: `"type":"integer"`, docs: []string{"1", "1.0", "1e2", "1E2", "1.5", "-0", "0.1e1", "1E-2", `"1"`, "null", "true", "[]", "{}", "1e400", "12345678901234567890123", "1e999999999", "-1e-999999999"}},
	{name: "type number", body: `"type":"number"`, docs: []string{"1", "1.5", "-0.0", "1e400", "1e999999999", `"1"`, "null", "[]"}},
	{name: "type string", body: `"type":"string"`, docs: anyValues},
	{name: "type boolean", body: `"type":"boolean"`, docs: anyValues},
	{name: "type null", body: `"type":"null"`, docs: anyValues},
	{name: "type array", body: `"type":"array"`, docs: anyValues},
	{name: "type object", body: `"type":"object"`, docs: anyValues},
	{name: "type list", body: `"type":["string","null"]`, docs: anyValues},
	{name: "type integer or string", body: `"type":["integer","string"]`, docs: []string{"1", "1.5", `"x"`, "true"}},
	{name: "type number and integer", body: `"type":["number","integer"]`, docs: []string{"1", "1.5", "true"}},
	{name: "required", body: `"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a","b"]`, docs: []string{"{}", `{"a":"x"}`, `{"a":1,"b":"x"}`, `{"a":"x","b":1}`, `{"a":"x","b":1,"c":2}`, `{"a":"x","a":2,"b":1}`, `{"b":1.5,"a":null}`, "[]"}},
	{name: "required special names", body: `"required":["<x>","{{.field}}","a.b","",  "é"]`, docs: []string{"{}", `{"<x>":1}`}},
	{name: "property names with dots", body: `"properties":{"a.b":{"type":"string"},"":{"type":"string"},"<&>":{"type":"string"}}`, docs: []string{`{"a.b":1}`, `{"":1}`, `{"<&>":1}`}},
	{name: "additionalProperties false", body: `"properties":{"a":{}},"additionalProperties":false`, docs: []string{`{"a":1}`, `{"b":1}`, `{"b":1,"c":2}`, `{"a":1,"b":2}`, "1"}},
	{name: "additionalProperties true", body: `"properties":{"a":{}},"additionalProperties":true`, docs: []string{`{"b":1}`}},
	{name: "additionalProperties schema", body: `"properties":{"a":{}},"additionalProperties":{"type":"string"}`, docs: []string{`{"b":1}`, `{"b":"x"}`, `{"a":1,"b":true,"c":"x"}`}},
	{name: "patternProperties", body: `"patternProperties":{"^x-":{"type":"integer"},"[0-9]$":{"type":"string"}},"additionalProperties":false`, docs: []string{`{"x-a":1}`, `{"x-a":"s"}`, `{"x-1":1}`, `{"b":1}`, `{"a1":"s","x-b":true}`, `{"é1":"s"}`}},
	{name: "patternProperties with properties", body: `"properties":{"x-a":{"type":"string"}},"patternProperties":{"^x-":{"minLength":2}},"additionalProperties":false`, docs: []string{`{"x-a":"s"}`, `{"x-a":1}`, `{"x-b":"s"}`}},
	{name: "dependencies", body: `"dependencies":{"a":["b","c"],"d":{"required":["e"]},"f":[]}`, docs: []string{`{"a":1}`, `{"a":1,"b":1}`, `{"a":1,"b":1,"c":1}`, `{"d":1}`, `{"d":1,"e":2}`, `{"f":1}`, "1", `"x"`, "[]", "1e999999999"}},
	{name: "dependencies boolean schema", body: `"dependencies":{"a":false,"b":true}`, docs: []string{`{"a":1}`, `{"b":1}`}},
	{name: "propertyNames", body: `"propertyNames":{"maxLength":2,"pattern":"^[a-z]+$"}`, docs: []string{`{"ab":1}`, `{"abc":1}`, `{"A":1}`, `{"Abc":1,"b":2}`, "[]"}},
	{name: "propertyNames false", body: `"propertyNames":false`, docs: []string{"{}", `{"a":1}`}},
	{name: "min and max properties", body: `"minProperties":1,"maxProperties":2`, docs: []string{"{}", `{"a":1}`, `{"a":1,"b":2,"c":3}`, "[]"}},
	{name: "items", body: `"items":{"type":"integer"},"minItems":1,"maxItems":3`, docs: []string{"[]", "[1]", `[1,"a",3]`, "[1,2,3,4]", `["a","b"]`, "{}"}},
	{name: "items tuple", body: `"items":[{"type":"string"},{"type":"integer"}],"additionalItems":false`, docs: []string{"[]", `["a"]`, `["a",1]`, `["a",1,2]`, `[1,"a"]`}},
	{name: "items tuple additional schema", body: `"items":[{"type":"string"}],"additionalItems":{"type":"boolean"}`, docs: []string{`["a",true,1]`, `["a",true]`}},
	{name: "items tuple without additionalItems", body: `"items":[{"type":"string"}]`, docs: []string{`["a",1]`}},
	{name: "items boolean", body: `"items":false`, docs: []string{"[]", "[1]"}},
	{name: "uniqueItems", body: `"uniqueItems":true`, docs: []string{"[1,1]", "[1,1.0]", `[1,"1"]`, `[{"a":1,"b":2},{"b":2,"a":1}]`, "[1,2,1,2]", "[[1],[1]]", "[null,null]", "[1e400]", "[1]", `["<","<"]`}},
	{name: "contains", body: `"contains":{"type":"integer","minimum":5}`, docs: []string{"[]", "[1,2]", "[1,6]", `["a"]`, "{}"}},
	{name: "enum", body: `"enum":[1,"a",{"b":"<&>"},[1,2],null,true,1.5]`, docs: []string{"1", "1.0", "1e0", `"a"`, `{"b":"<&>"}`, "[1,2]", "[2,1]", "null", "true", "false", "1.50", "2", "1e400", `"<"`}},
	{name: "enum strings", body: `"enum":["a<b","é"," ","line\nbreak"]`, docs: []string{`"x"`, `"é"`}},
	{name: "enum floats", body: `"enum":[0.1,1e21,1e-7,123456789,1e20,-0]`, docs: []string{"0.10", "1e21", "0.0000001", "5", "0"}},
	{name: "const", body: `"const":{"a":[1,"x"]}`, docs: []string{`{"a":[1,"x"]}`, `{"a":[1.0,"x"]}`, `{"a":[1]}`, "1"}},
	{name: "const null", body: `"const":null`, docs: []string{"null", "0"}},
	{name: "string length", body: `"minLength":2,"maxLength":3`, docs: []string{`"a"`, `"ab"`, `"abcd"`, `"éé"`, `"日本語"`, `"日本語x"`, `"😀😀"`, `"😀"`, `"\ud800"`, "1"}},
	{name: "pattern charset", body: `"pattern":"^[a-z0-9_-]+$"`, docs: []string{`"abc"`, `"ABC"`, `"a b"`, `""`, "1"}},
	{name: "pattern unanchored digits", body: `"pattern":"\\d{3}"`, docs: []string{`"x123y"`, `"x12"`, `"١٢٣"`}},
	{name: "pattern word", body: `"pattern":"^\\w+$"`, docs: []string{`"abc_1"`, `"é"`}},
	{name: "pattern case insensitive", body: `"pattern":"(?i)^abc$"`, docs: []string{`"ABC"`, `"abd"`, `"aBc"`}},
	{name: "pattern unicode class", body: `"pattern":"^\\p{L}+$"`, docs: []string{`"日本"`, `"a1"`, `"É"`}},
	{name: "pattern dot", body: `"pattern":"^a.c$"`, docs: []string{`"a\nc"`, `"abc"`, `"aéc"`}},
	{name: "pattern space", body: `"pattern":"^\\s*$"`, docs: []string{`" "`, `" \t"`, `"\u000b"`}},
	{name: "pattern posix", body: `"pattern":"^[[:alpha:]]+$"`, docs: []string{`"abc"`, `"é"`}},
	{name: "pattern quoted", body: `"pattern":"\\Qa.b\\E"`, docs: []string{`"a.b"`, `"axb"`}},
	{name: "pattern hex", body: `"pattern":"^\\x41\\x{e9}$"`, docs: []string{`"Aé"`, `"A"`}},
	{name: "pattern repeat", body: `"pattern":"^a{2,}$"`, docs: []string{`"a"`, `"aaa"`}},
	{name: "pattern ip", body: `"pattern":"^(?:[0-9]{1,3}\\.){3}[0-9]{1,3}$"`, docs: []string{`"1.2.3.4"`, `"1.2.3"`}},
	{name: "pattern word boundary", body: `"pattern":"\\bfoo\\b"`, docs: []string{`"a foo b"`, `"éfooé"`, `"afoo"`}},
	{name: "pattern alternation", body: `"pattern":"^(banner|video|native)$"`, docs: []string{`"video"`, `"audio"`}},
	{name: "pattern literal brace", body: `"pattern":"^a{,2}$"`, docs: []string{`"a{,2}"`, `"aa"`}},
	{name: "pattern url", body: `"pattern":"^https?://[^\\s/$.?#].[^\\s]*$"`, docs: []string{`"https://example.com/x"`, `"ftp://x"`}},
	{name: "pattern escapes", body: `"pattern":"^[\\w.%+-]+@[\\w.-]+\\.[a-zA-Z]{2,}$"`, docs: []string{`"a.b@c.de"`, `"a@b"`}},
	{name: "format hostname", body: `"format":"hostname"`, docs: []string{`"example.com"`, `"-bad.com"`, `"a..b"`, `"a_b.com"`, `"localhost"`, `"` + strings.Repeat("a", 64) + `.com"`, `"` + strings.Repeat("a.", 128) + `a"`, "1", `""`}},
	{name: "format uuid", body: `"format":"uuid"`, docs: []string{`"123e4567-e89b-12d3-a456-426614174000"`, `"123E4567-E89B-12D3-A456-426614174000"`, `"123e4567e89b12d3a456426614174000"`, "1"}},
	{name: "format regex", body: `"format":"regex"`, docs: []string{`"a+"`, `"("`, `""`, "1"}},
	{name: "format json-pointer", body: `"format":"json-pointer"`, docs: []string{`"/a/b"`, `"a"`, `""`, `"/a~0b~1"`, `"/a~2"`}},
	{name: "format relative-json-pointer", body: `"format":"relative-json-pointer"`, docs: []string{`"0/a"`, `"1#"`, `"a"`, `"01"`}},
	{name: "format unknown", body: `"format":"custom-unknown"`, docs: []string{`"x"`, "1"}},
	{name: "format on number", body: `"format":"hostname","type":"integer"`, docs: []string{"1"}},
	{name: "minimum and maximum", body: `"minimum":1,"maximum":10`, docs: []string{"0", "1", "10", "11", "0.5", "10.000001", `"x"`, "1e999999999"}},
	{name: "multipleOf fraction", body: `"multipleOf":0.01`, docs: []string{"0.1", "0.123", "1e-2", "3", "1e400", "1e999999999"}},
	{name: "multipleOf integer", body: `"multipleOf":3`, docs: []string{"9", "10", "-6", "0", "4.5"}},
	{name: "limit formatting large", body: `"minimum":1000000`, docs: []string{"5"}},
	{name: "limit formatting six digits", body: `"minimum":123456`, docs: []string{"5"}},
	{name: "limit formatting seven digits", body: `"minimum":1234567`, docs: []string{"5"}},
	{name: "limit formatting small", body: `"maximum":0.00001`, docs: []string{"1"}},
	{name: "limit formatting 1e-4", body: `"maximum":0.0001`, docs: []string{"1"}},
	{name: "limit formatting nine digits", body: `"minimum":123456789`, docs: []string{"1"}},
	{name: "limit formatting fraction", body: `"maximum":1.5`, docs: []string{"2"}},
	{name: "limit formatting negative", body: `"minimum":-2.5`, docs: []string{"-3"}},
	{name: "limit formatting huge", body: `"maximum":123456789012345678901234567890`, docs: []string{"1e40"}},
	{name: "limit formatting tenth", body: `"minimum":0.1`, docs: []string{"0"}},
	{name: "limit formatting 1e-7", body: `"maximum":1e-7`, docs: []string{"1"}},
	{name: "limit formatting exponent", body: `"maximum":2.5e3`, docs: []string{"3000"}},
	{name: "limit formatting 17 digits", body: `"maximum":0.12345678901234567`, docs: []string{"1"}},
	{name: "limit formatting zero", body: `"maximum":0`, docs: []string{"1"}},
	{name: "limit formatting negative zero", body: `"maximum":-0.0`, docs: []string{"1"}},
	{name: "multipleOf formatting", body: `"multipleOf":0.25`, docs: []string{"0.3"}},
	{name: "exclusive minimum boolean", body: `"minimum":5,"exclusiveMinimum":true`, docs: []string{"5", "6"}},
	{name: "exclusive maximum boolean false", body: `"maximum":5,"exclusiveMaximum":false`, docs: []string{"5", "6"}},
	{name: "exclusive minimum number", body: `"exclusiveMinimum":5`, docs: []string{"5", "6"}},
	{name: "exclusive maximum number", body: `"exclusiveMaximum":5,"maximum":4`, docs: []string{"5", "4.5", "4"}},
	{name: "exclusive without limit", body: `"exclusiveMaximum":true`, docs: []string{"1"}},
	{name: "anyOf", body: `"anyOf":[{"type":"string","minLength":5},{"type":"integer","minimum":10}]`, docs: []string{`"abc"`, "3", "true", `"abcdef"`, "12"}},
	{name: "oneOf required", body: `"oneOf":[{"required":["a"]},{"required":["b"]}]`, docs: []string{"{}", `{"a":1}`, `{"a":1,"b":1}`}},
	{name: "oneOf scoring", body: `"oneOf":[{"properties":{"a":{"type":"string"}},"required":["a","b"]},{"properties":{"a":{"type":"integer"}},"required":["a"]}]`, docs: []string{`{"a":"x"}`, `{"a":true}`, `{"a":1.5,"b":1}`, `{"a":1}`}},
	{name: "oneOf all valid", body: `"oneOf":[{},{"type":"integer"},true]`, docs: []string{"1", `"x"`}},
	{name: "allOf", body: `"allOf":[{"required":["a"]},{"properties":{"a":{"type":"string"}}}]`, docs: []string{"{}", `{"a":1}`, `{"a":"x"}`}},
	{name: "not", body: `"not":{"type":"string"}`, docs: []string{`"x"`, "1"}},
	{name: "anyOf in property", body: `"properties":{"id":{"anyOf":[{"type":"string","pattern":"^[0-9]+$"},{"type":"integer"}]}}`, docs: []string{`{"id":"12"}`, `{"id":"ab"}`, `{"id":1.5}`, `{"id":1}`}},
	{name: "nested anyOf scoring", body: `"anyOf":[{"properties":{"a":{"properties":{"b":{"type":"string"}},"required":["b"]}},"required":["a"]},{"required":["c","d"]}]`, docs: []string{`{"a":{}}`, `{"a":{"b":1}}`, `{"c":1}`, "{}"}},
	{name: "if then else", body: `"if":{"required":["a"]},"then":{"required":["b"]},"else":{"required":["c"]}`, docs: []string{"{}", `{"a":1}`, `{"a":1,"b":1}`, `{"c":1}`}},
	{name: "if then", body: `"if":{"properties":{"a":{"const":1}}},"then":{"required":["b"]}`, docs: []string{`{"a":1}`, `{"a":2}`}},
	{name: "definitions", body: `"definitions":{"x":{"type":"string"},"y":true},"type":"object"`, docs: []string{"{}", "1"}},
	{name: "nested contexts", body: `"properties":{"a":{"items":{"properties":{"b":{"type":"string"}}}}}`, docs: []string{`{"a":[{"b":1},{"b":"x"},{"b":true}]}`}},
	{name: "boolean subschemas", body: `"properties":{"a":true,"b":false}`, docs: []string{`{"a":1}`, `{"b":1}`}},
	{name: "realistic bidder", body: `"type":"object","properties":{"placementId":{"type":"integer","minimum":1},"invCode":{"type":"string"},"member":{"type":"string"},"keywords":{"type":"array","items":{"type":"object","properties":{"key":{"type":"string"},"value":{"type":"array","minItems":1,"items":{"type":"string"}}},"required":["key"]}}},"oneOf":[{"required":["placementId"]},{"required":["invCode","member"]}],"not":{"required":["placementId","invCode"]}`, docs: []string{"{}", `{"placementId":0}`, `{"placementId":5}`, `{"invCode":"x"}`, `{"invCode":"x","member":"y"}`, `{"placementId":1,"invCode":"x","member":"y"}`, `{"placementId":1,"keywords":[{"value":[]}]}`}},
	// Load errors.
	{name: "invalid type name", body: `"type":"strin"`},
	{name: "duplicated type", body: `"type":["string","string"]`},
	{name: "type number", body: `"type":1`},
	{name: "type list with number", body: `"type":[1]`},
	{name: "type list with boolean", body: `"type":[true]`},
	{name: "properties array", body: `"properties":[]`},
	{name: "property not a schema", body: `"properties":{"a":1}`},
	{name: "property boolean", body: `"properties":{"a":true}`, docs: []string{`{"a":1}`}},
	{name: "additionalProperties number", body: `"additionalProperties":1`},
	{name: "patternProperties invalid regex", body: `"patternProperties":{"(":{}}`},
	{name: "patternProperties array", body: `"patternProperties":[]`},
	{name: "propertyNames number", body: `"propertyNames":1`},
	{name: "dependency string", body: `"dependencies":{"a":"b"}`},
	{name: "dependency number list", body: `"dependencies":{"a":[1]}`},
	{name: "dependencies array", body: `"dependencies":[]`},
	{name: "items number", body: `"items":1`},
	{name: "items list with number", body: `"items":[1]`},
	{name: "additionalItems string", body: `"additionalItems":"x"`},
	{name: "multipleOf zero", body: `"multipleOf":0`},
	{name: "multipleOf negative", body: `"multipleOf":-1`},
	{name: "multipleOf string", body: `"multipleOf":"x"`},
	{name: "minimum string", body: `"minimum":"x"`},
	{name: "maximum boolean", body: `"maximum":true`},
	{name: "exclusiveMinimum string", body: `"minimum":1,"exclusiveMinimum":"x"`},
	{name: "minLength negative", body: `"minLength":-1`},
	{name: "minLength fraction", body: `"minLength":1.5`},
	{name: "minLength string", body: `"minLength":"1"`},
	{name: "maxLength exponent", body: `"maxLength":1e2`},
	{name: "maxLength decimal integer", body: `"maxLength":2.0`},
	{name: "minLength above maxLength", body: `"minLength":3,"maxLength":2`},
	{name: "pattern invalid", body: `"pattern":"("`},
	{name: "pattern invalid repeat", body: `"pattern":"a**"`},
	{name: "pattern lookahead", body: `"pattern":"(?=a)"`},
	{name: "pattern backreference", body: `"pattern":"(a)\\1"`},
	{name: "pattern big repeat", body: `"pattern":"a{1001}"`},
	{name: "pattern number", body: `"pattern":1`},
	{name: "format number", body: `"format":1`},
	{name: "minProperties above maxProperties", body: `"minProperties":3,"maxProperties":2`},
	{name: "maxProperties negative", body: `"maxProperties":-1`},
	{name: "minItems string", body: `"minItems":"1"`},
	{name: "maxItems negative", body: `"maxItems":-2`},
	{name: "required string", body: `"required":"a"`},
	{name: "required number", body: `"required":[1]`},
	{name: "required duplicated", body: `"required":["a","a"]`},
	{name: "uniqueItems number", body: `"uniqueItems":1`},
	{name: "contains number", body: `"contains":1`},
	{name: "enum number", body: `"enum":1`},
	{name: "enum duplicated numbers", body: `"enum":[1,1.0]`},
	{name: "enum duplicated objects", body: `"enum":[{"a":1,"b":2},{"b":2,"a":1}]`},
	{name: "enum overflow", body: `"enum":[1e400]`},
	{name: "const overflow", body: `"const":1e400`},
	{name: "oneOf object", body: `"oneOf":{}`},
	{name: "anyOf number", body: `"anyOf":1`},
	{name: "allOf number", body: `"allOf":1`},
	{name: "oneOf number item", body: `"oneOf":[1]`},
	{name: "not number", body: `"not":1`},
	{name: "if number", body: `"if":1`},
	{name: "then string", body: `"then":"x"`},
	{name: "id number", body: `"id":1`},
	{name: "id invalid url", body: `"id":"::"`, docs: []string{"1"}},
	{name: "id with space", body: `"id":"http://a b/"`, docs: []string{"1"}},
	{name: "id valid", body: `"id":"http://example.com/s.json#"`, docs: []string{"1"}},
	{name: "nested id invalid escape", body: `"properties":{"a":{"id":"x%zz","$id":"y%zz"}}`, docs: []string{"1"}},
	{name: "ref number", body: `"$ref":1`},
	{name: "ref invalid url", body: `"$ref":"::"`},
	{name: "dollar id number", body: `"$id":1`},
	{name: "definitions number", body: `"definitions":1`},
	{name: "definitions boolean", body: `"definitions":true`},
	{name: "definition number", body: `"definitions":{"a":1}`},
	{name: "title number", body: `"title":1`},
	{name: "description object", body: `"description":{}`},
	{name: "two load errors", body: `"properties":{"a":{"type":"x"},"b":{"type":"y"}}`},
	// Whole schemas.
	{name: "schema true", schema: `true`, docs: []string{"1", "{}"}},
	{name: "schema false", schema: `false`, docs: []string{"1", "{}"}},
	{name: "schema array", schema: `[]`},
	{name: "schema string", schema: `"x"`},
	{name: "schema invalid json", schema: `{`},
	{name: "schema empty", schema: ``},
	{name: "schema trailing", schema: `{"type":"string"} x`, docs: []string{"1"}},
	{name: "schema url without fragment", schema: `{"$schema":"http://json-schema.org/draft-04/schema","exclusiveMinimum":1}`},
	{name: "schema url https", schema: `{"$schema":"https://json-schema.org/draft-04/schema#","exclusiveMinimum":1}`},
	{name: "schema url upper case scheme", schema: `{"$schema":"HTTP://json-schema.org/draft-04/schema#","exclusiveMinimum":1}`},
	{name: "schema url latest", schema: `{"$schema":"http://json-schema.org/schema#","exclusiveMinimum":true}`},
	{name: "schema url 2020-12", schema: `{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"a":{"exclusiveMinimum":5}},"propertyNames":{"maxLength":1},"if":{"required":["a"]},"then":{"required":["b"]}}`, docs: []string{`{"a":5}`, `{"abc":1}`, `{"a":6}`}},
	{name: "schema url draft6 boolean schemas", schema: `{"$schema":"http://json-schema.org/draft-06/schema#","properties":{"a":false}}`, docs: []string{`{"a":1}`}},
	{name: "schema url draft4 boolean schemas", schema: `{"$schema":"http://json-schema.org/draft-04/schema#","properties":{"a":false}}`},
	{name: "schema url number", schema: `{"$schema":1}`},
	{name: "schema url invalid", schema: `{"$schema":"::"}`},
	{name: "schema url bad escape", schema: `{"$schema":"http://x/%zz"}`},
	{name: "schema url with space", schema: `{"$schema":" http://json-schema.org/draft-04/schema#","exclusiveMinimum":1}`},
	// Document decoding.
	{name: "decoding", schema: `{}`, docs: []string{"", " ", "{", "[1,", `{"a" 1}`, "tru", "nul", "fals", "01", "-", "1.", "1e", "1e+", ".5", "+1", "nan", `"\x"`, `"\u12"`, `"\u12g4"`, "\"a\x01\"", "\"a\tb\"", "{} x", "{}{}", "[1,]", `{"a":1,}`, `{"a":1 "b":2}`, `{1:2}`, `"\ud800"`, `"😀"`, `"\udc00\ud800"`, "\xff", "\"\xff\"", "\"\xed\xa0\x80\"", "\"\xc0\xaf\"", "  \n\t{} ", "\x00", "[" + strings.Repeat("[", 9999) + strings.Repeat("]", 10000), strings.Repeat("[", 10001), "{\"a\":" + strings.Repeat("{\"a\":", 10000) + "1" + strings.Repeat("}", 10001), "]", "}", ",", ":", "\"abc", "[1 2]", "true false", "nulll", "-01", "0x1", "1e5000000000000000000000"}},
	{name: "decoded strings", schema: `{"maxLength":1}`, docs: []string{"\"\xff\xfe\"", "\"\xff\"", `"𐀀"`, `"\ud800x"`, `"\uDFFF"`, `"é"`}},
	{name: "decoded replacement", schema: `{"enum":["�"]}`, docs: []string{"\"\xff\"", `"\ud800"`, `"x"`}},
}

// detail is one ResultError.
type detail struct {
	Type        string `json:"type"`
	Field       string `json:"field"`
	Context     string `json:"context"`
	Description string `json:"description"`
}

// engineLine is one line of engine_cases.jsonl.
type engineLine struct {
	Name      string  `json:"name"`
	Schema    *string `json:"schema,omitempty"`
	SchemaHex string  `json:"schema_hex,omitempty"`
	Doc       *string `json:"doc,omitempty"`
	DocHex    string  `json:"doc_hex,omitempty"`
	// Kind is load_error, load_panic, ok, invalid, decode or panic.
	Kind string `json:"kind"`
	// Outputs are the distinct load errors, decode errors or joined error strings.
	Outputs []string `json:"outputs,omitempty"`
	// Details are the errors of one validation, sorted.
	Details []detail `json:"details,omitempty"`
}

func writeEngineCases(path string) {
	file, err := os.Create(path)
	if err != nil {
		log.Fatal(err)
	}
	w := bufio.NewWriter(file)
	for _, c := range engineCases {
		if c.body == "" {
			writeEngineSchema(w, c.name, c.schema, c.docs)
			continue
		}
		for _, d := range draftPrefixes {
			writeEngineSchema(w, c.name+" ("+d.name+")", "{"+d.prefix+c.body+"}", c.docs)
		}
	}
	if err := w.Flush(); err != nil {
		log.Fatal(err)
	}
	if err := file.Close(); err != nil {
		log.Fatal(err)
	}
}

func textFields(text string) (*string, string) {
	if utf8.ValidString(text) {
		return &text, ""
	}
	return nil, hex.EncodeToString([]byte(text))
}

func loadOnce(schema string) (s *gojsonschema.Schema, err error, panicked bool) {
	defer func() {
		if r := recover(); r != nil {
			s, err, panicked = nil, fmt.Errorf("%v", r), true
		}
	}()
	s, err = gojsonschema.NewSchema(gojsonschema.NewStringLoader(schema))
	return s, err, false
}

func writeEngineSchema(w *bufio.Writer, name, schema string, docs []string) {
	line := engineLine{Name: name}
	line.Schema, line.SchemaHex = textFields(schema)
	var loaded []*gojsonschema.Schema
	loadErrors := map[string]bool{}
	panicked := false
	for i := 0; i < copies; i++ {
		s, err, p := loadOnce(schema)
		panicked = panicked || p
		if err != nil {
			loadErrors[err.Error()] = true
			continue
		}
		loaded = append(loaded, s)
	}
	if len(loadErrors) > 0 {
		if len(loaded) > 0 {
			log.Fatalf("%s: loads both fail and succeed", name)
		}
		line.Kind = "load_error"
		if panicked {
			line.Kind = "load_panic"
		}
		for text := range loadErrors {
			line.Outputs = append(line.Outputs, text)
		}
		sort.Strings(line.Outputs)
		writeLine(w, line)
		return
	}
	if len(docs) == 0 {
		line.Kind = "loaded"
		writeLine(w, line)
		return
	}
	for _, doc := range docs {
		docLine := line
		docLine.Doc, docLine.DocHex = textFields(doc)
		s := sample(loaded, []byte(doc))
		docLine.Kind = s.kind
		docLine.Outputs = s.outputs
		for _, e := range s.first.errors {
			docLine.Details = append(docLine.Details, detail{
				Type:        e.Type(),
				Field:       e.Field(),
				Context:     e.Context().String(),
				Description: e.Description(),
			})
		}
		sort.Slice(docLine.Details, func(i, j int) bool {
			a, b := docLine.Details[i], docLine.Details[j]
			return a.Type+"\x00"+a.Context+"\x00"+a.Description < b.Type+"\x00"+b.Context+"\x00"+b.Description
		})
		writeLine(w, docLine)
	}
}

func writeLine(w *bufio.Writer, line interface{}) {
	data, err := marshalLine(line)
	if err != nil {
		log.Fatal(err)
	}
	if _, err := w.Write(data); err != nil {
		log.Fatal(err)
	}
}
