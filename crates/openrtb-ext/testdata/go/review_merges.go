package main

// review_merges.jsonl (the format of shapes.jsonl, merge cases only) holds the adversarial
// review's jsonutil.MergeClone cases, aimed at what the shape cases do not reach: they merge a
// type's edge values over its first sample only, and give the structs inside maps no fields.
//
//   - The values the UnmarshalJSON methods tell apart (review.go's grids) merged over a value
//     that already holds something, at the top and through the pointer, value and map fields
//     that hold the types. A pointer is cloned and the clone handed to the method, which writes
//     the width before it checks the height (ExtDeviceInt), replaces the value only when it reads
//     the patch (PriceGranularity), sets amp only when it is 0 or 1 (ExtSite) and leaves the
//     keywords on a value it ignores (ExtImpAppnexusKeywords); a map entry is decoded into a new
//     value (stringOrStringArray).
//   - The openrtb_ext types an account holds (config.Account's alternatebiddercodes,
//     bidadjustments and preferredmediatype), with fields in their map entries: Prebid merges an
//     account's JSON over the account defaults with MergeClone (account/fetcher.go:141-147), so
//     an entry the account names replaces the default's entry whole.

import (
	"fmt"
	"log"
	"reflect"
)

// alternateBidderCodesPatches are merged over an ExtAlternateBidderCodes with two bidders.
var alternateBidderCodesPatches = []string{
	`{"bidders":{"pubmatic":{"enabled":false}}}`,
	`{"bidders":{"pubmatic":{"allowedbiddercodes":["y"]}}}`,
	`{"bidders":{"pubmatic":{}}}`,
	`{"bidders":{"pubmatic":null}}`,
	`{"bidders":{"rubicon":{"enabled":true,"allowedbiddercodes":["z"]}}}`,
	`{"bidders":{}}`,
	`{"bidders":null}`,
	`{"enabled":false}`,
	`{"enabled":null,"bidders":{"pubmatic":{"enabled":null}}}`,
	`{"bidders":{"pubmatic":{"allowedbiddercodes":null}}}`,
	`{"bidders":{"pubmatic":{"allowedbiddercodes":[]}}}`,
	`{"bidders":{"pubmatic":{"allowedbiddercodes":[null]}}}`,
	`{"bidders":{"pubmatic":{"enabled":"true"}}}`,
	`{"bidders":{"pubmatic":{"allowedbiddercodes":"x"}}}`,
	`{"bidders":{"pubmatic":[]}}`,
	`{"bidders":{"PubMatic":{"enabled":false}}}`,
	`{"bidders":{"pubmatic":{"ENABLED":false}}}`,
	`{"bidders":{"pubmatic":{"enabled":false},"pubmatic":{"allowedbiddercodes":["w"]}}}`,
	`{"bidders":{"appnexus":{"enabled":false},"pubmatic":{"allowedbiddercodes":["w"]}}}`,
	`{"bidders":[]}`,
	`{"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["groupm","*"]}}}`,
	`{}`,
	`null`,
}

// bidAdjustmentsPatches are merged over an ExtRequestPrebidBidAdjustments with banner and
// video-instream adjustments.
var bidAdjustmentsPatches = []string{
	`{"mediatype":{"banner":{"appnexus":{"deal2":[{"adjtype":"multiplier","value":2}]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":[]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":null}}}}`,
	`{"mediatype":{"banner":{"appnexus":null}}}`,
	`{"mediatype":{"banner":null}}`,
	`{"mediatype":{"banner":{}}}`,
	`{"mediatype":{"native":{"pubmatic":{"*":[{"adjtype":"cpm","value":1}]}}}}`,
	`{"mediatype":{"banner":{"rubicon":{"*":[{"value":3}]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":[{"adjtype":"multiplier","value":"x"}]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":[null]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":{}}}}}`,
	`{"mediatype":null}`,
	`{"mediatype":{}}`,
	`{"mediatype":{"*":{"*":{"*":[{"adjtype":"multiplier","value":1.5}]}}}}`,
	`{"mediatype":{"video-instream":{"*":{"*":[{"adjtype":"multiplier","value":0.9}],"*":[{"adjtype":"cpm","value":0.1}]}}}}`,
	`{"MediaType":{"banner":{"appnexus":{"*":[]}}}}`,
	`{"mediatype":{"banner":{"APPNEXUS":{"*":[]}}}}`,
	`{"mediatype":{"banner":{"appnexus":{"*":[{"adjtype":"cpm","value":0.5,"currency":"usd","zz":1}]}}}}`,
	`{"mediatype":{"audio":{"appnexus":{"*":[{"adjtype":"static","value":-1}]}},"banner":{"appnexus":{"deal1":[]}}}}`,
	`{"mediatype":[]}`,
	`{}`,
	`null`,
	`[]`,
}

// preferredMediaTypePatches are merged over a PreferredMediaType of two bidders.
var preferredMediaTypePatches = []string{
	`{"rubicon":"native"}`, `{"pubmatic":"video"}`, `{"appnexus":null}`, `{}`, `null`,
	`{"appnexus":1}`, `[]`, `{"appnexus":"banner","appnexus":"audio"}`, `{"AppNexus":"video"}`,
	`{"appnexus":""}`, `{"appnexus":"video","pubmatic":true}`,
}

// reviewMergeCases are review_merges.jsonl's cases.
func reviewMergeCases(types map[string]reflect.Type) []shapeCase {
	var out []shapeCase
	typ := func(name string) reflect.Type {
		t, ok := types[name]
		if !ok {
			log.Fatalf("review merges: no ported type %s", name)
		}
		return t
	}
	// merges adds each patch, put at the "@" of each wrapper, merged into the value target
	// decodes to as name.
	merges := func(group, name, target string, wrappers, patches []string) {
		t := typ(name)
		for wi, w := range wrappers {
			for pi, p := range patches {
				out = append(out, shapeCase{
					kind:  "M",
					typ:   t,
					label: fmt.Sprintf("%s %d.%d", group, wi, pi),
					doc:   target,
					patch: nest(w, p),
				})
			}
		}
	}

	interstitials := interstitialValues()
	var sampled []string
	for i, v := range interstitials {
		if i < 45 || i%9 == 0 {
			sampled = append(sampled, v)
		}
	}
	const percents = `{"minwidthperc":50,"minheightperc":60}`
	merges("interstitial", "openrtb_ext.ExtDeviceInt", percents, []string{`@`}, interstitials)
	merges("nested interstitial", "openrtb_ext.ExtDevicePrebid", `{"interstitial":`+percents+`}`,
		[]string{`{"interstitial":@}`}, interstitials)
	merges("new interstitial", "openrtb_ext.ExtDevicePrebid", `{}`,
		[]string{`{"interstitial":@}`}, sampled)
	merges("device interstitial", "openrtb_ext.ExtDevice",
		`{"atts":1,"prebid":{"interstitial":`+percents+`}}`,
		[]string{`{"prebid":{"interstitial":@}}`, `{"atts":2,"prebid":{"interstitial":@},"x":1}`},
		sampled)

	granularities := priceGranularityValues()
	const custom = `{"precision":3,"ranges":[{"max":5,"increment":0.5}]}`
	merges("price granularity", "openrtb_ext.PriceGranularity", custom, []string{`@`}, granularities)
	merges("targeting price granularity", "openrtb_ext.ExtRequestTargeting",
		`{"pricegranularity":`+custom+`,"includewinners":true}`,
		[]string{`{"pricegranularity":@}`, `{"mediatypepricegranularity":{"banner":@}}`},
		granularities)
	merges("media type price granularity", "openrtb_ext.MediaTypePriceGranularity",
		`{"banner":"high","video":{"precision":1}}`, []string{`{"video":@}`}, granularities)

	var amps []string
	for _, form := range ampForms {
		for _, v := range ampValues {
			amps = append(amps, nest(form, v))
		}
	}
	merges("amp", "openrtb_ext.ExtSite", `{"amp":1}`, []string{`@`}, amps)

	keywords := keywordValues(&splitMix{state: 0x6d65726765})
	merges("keywords", "openrtb_ext.ExtImpAppnexusKeywords", `"x=y"`, []string{`@`}, keywords)
	merges("appnexus keywords", "openrtb_ext.ExtImpAppnexus",
		`{"placementId":1,"keywords":"x=y","member":"m"}`, []string{`{"keywords":@}`}, keywords)

	goldbach := typ("openrtb_ext.ImpExtGoldbach")
	targeting, ok := goldbach.FieldByName("CustomTargeting")
	if !ok {
		log.Fatal("review merges: ImpExtGoldbach has no CustomTargeting")
	}
	merges("custom targeting", "openrtb_ext.ImpExtGoldbach",
		`{"publisherId":"p","customTargeting":{"k":["a"],"j":"b"}}`,
		[]string{
			`{"customTargeting":{"k":@}}`, `{"customTargeting":{"j":@,"n":["x"]}}`,
			`{"slotId":"s","customTargeting":{"k":@}}`,
		},
		stringOrArrayValues(targeting.Type.Elem()))

	merges("alternate bidder codes", "openrtb_ext.ExtAlternateBidderCodes",
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["groupm","*"]},`+
			`"appnexus":{"enabled":true,"allowedbiddercodes":["x"]}}}`,
		[]string{`@`}, alternateBidderCodesPatches)
	merges("bid adjustments", "openrtb_ext.ExtRequestPrebidBidAdjustments",
		`{"mediatype":{"banner":{"appnexus":{"*":[{"adjtype":"multiplier","value":1.1}],`+
			`"deal1":[{"adjtype":"cpm","value":0.5,"currency":"USD"}]},`+
			`"rubicon":{"*":[{"adjtype":"static","value":2,"currency":"EUR"}]}},`+
			`"video-instream":{"*":{"*":[{"adjtype":"multiplier","value":0.9}]}}}}`,
		[]string{`@`}, bidAdjustmentsPatches)
	merges("preferred media type", "openrtb_ext.PreferredMediaType",
		`{"appnexus":"banner","rubicon":"video"}`, []string{`@`}, preferredMediaTypePatches)
	return out
}
