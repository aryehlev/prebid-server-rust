package main

import (
	"reflect"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/aryehlev/prebid-server-rust/crates/openrtb/testdata/go/p"
)

// containerValues are valuesFor's values, except that a map's are built from its element's
// values (valuesFor gives every map strings), so that a map of ints or of json.RawMessage gets
// samples of its element type and a value of the wrong JSON type for its element.
var containerValues = shapeValues{values: containerJSON, extremes: containerExtremes}

func containerJSON(t reflect.Type) values {
	if t.Kind() != reflect.Map {
		return valuesFor(t)
	}
	e := valuesFor(t.Elem())
	first, last := e.samples[0], e.samples[len(e.samples)-1]
	v := values{
		samples: []string{`{"a":` + first + `}`, `{"b":` + last + `,"a":` + first + `}`},
		zero:    `{}`,
	}
	if len(e.wrong) > 0 {
		v.wrong = append(v.wrong, `{"a":`+e.wrong[0]+`}`)
	}
	v.wrong = append(v.wrong, `[]`, `"x"`)
	return v
}

// containerExtremes are intExtremes, and for a map those of its element as the value of one key.
func containerExtremes(t reflect.Type) []string {
	if t.Kind() != reflect.Map {
		return intExtremes(t)
	}
	var out []string
	for _, e := range intExtremes(t.Elem()) {
		out = append(out, `{"a":`+e+`}`)
	}
	return out
}

// containerEdges are the cases of p.Containers that the shape cases leave out: null as a map value
// and a slice element, merges that name some entries of a map and none of others, merges into
// slices that hold values, and values of the wrong JSON type deep in a container; and merges
// that fail inside a slice of an OpenRTB type, which the shape cases of the ported types do not
// reach (they give a slice field a value of the wrong JSON type only as a whole).
func containerEdges() []shapeCase {
	t := reflect.TypeOf(p.Containers{})
	decode := func(label, doc string) shapeCase {
		return shapeCase{kind: "D", typ: t, label: label, doc: doc}
	}
	merge := func(label, doc, patch string) shapeCase {
		return shapeCase{kind: "M", typ: t, label: label, doc: doc, patch: patch}
	}
	request := func(label, patch string) shapeCase {
		return shapeCase{kind: "M", typ: reflect.TypeOf(openrtb2.BidRequest{}), label: label,
			doc: `{}`, patch: patch}
	}
	return []shapeCase{
		decode("bidder null value", `{"bidder":{"a":null,"b":{ "x" : 1 }}}`),
		decode("responsetimemillis wrong value", `{"responsetimemillis":{"a":"1"}}`),
		decode("sizes null inner", `{"sizes":[null,[1,2]]}`),
		decode("sizes empty inner", `{"sizes":[[],[3]]}`),
		decode("counts null element", `{"counts":[null,1]}`),
		decode("durationrangesec wrong element", `{"durationrangesec":[1,"x"]}`),
		decode("wide negative", `{"wide":{"a":-1}}`),
		merge("bidder named entries", `{"bidder":{"a":{"a":1},"b":[1]}}`,
			`{"bidder":{"a":{"b":2},"c":null}}`),
		merge("bidder null value", `{"bidder":{"a":{"a":1},"b":[1]}}`, `{"bidder":{"a":null}}`),
		merge("responsetimemillis named entries", `{"responsetimemillis":{"a":1,"b":2}}`,
			`{"responsetimemillis":{"b":3,"c":4}}`),
		merge("responsetimemillis wrong value", `{"responsetimemillis":{"a":1}}`,
			`{"responsetimemillis":{"b":"x"}}`),
		merge("durationrangesec shorter", `{"durationrangesec":[1,2,3]}`,
			`{"durationrangesec":[4]}`),
		merge("durationrangesec wrong element", `{"durationrangesec":[1,2,3]}`,
			`{"durationrangesec":[4,"x"]}`),
		merge("sizes inner", `{"sizes":[[1],[2,3]]}`, `{"sizes":[[4],null]}`),
		merge("sizes wrong inner", `{"sizes":[[1]]}`, `{"sizes":[[2],{}]}`),
		merge("counts null element", `{"counts":[1,2]}`, `{"counts":[null,3]}`),
		merge("rawptr into existing", `{"rawptr":{"a":1,"b":2}}`, `{"rawptr":{"b":null,"c":3}}`),
		merge("raws into existing", `{"raws":[{"a":1}]}`, `{"raws":[{"b":2}]}`),
		merge("wide wrong value", `{}`, `{"wide":{"a":-1}}`),
		request("bcat wrong element", `{"bcat":["a",1,"b"]}`),
		request("imp iframebuster wrong element", `{"imp":[{"iframebuster":[2]}]}`),
		request("imp banner format wrong field", `{"imp":[{"banner":{"format":[{"w":"x"}]}}]}`),
		request("imp banner format wrong element", `{"imp":[{"banner":{"format":[1]}}]}`),
	}
}

// writeContainers writes containers.jsonl: the shape cases of p.Containers and its edges (see
// writeShapeCases).
func writeContainers(path string) int {
	cases := shapeCases(reflect.TypeOf(p.Containers{}), containerValues)
	return writeShapeCases(path, append(cases, containerEdges()...))
}
