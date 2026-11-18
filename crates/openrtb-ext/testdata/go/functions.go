package main

import (
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/openrtb_ext"
	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// call is one call of a function or method: its arguments as text and its results as text.
type call struct {
	name    string
	label   string
	args    []string
	results []string
}

// nilArg stands for a nil pointer, map or slice argument.
const nilArg = "nil"

// errResults are an error's dynamic type and text, both empty for nil.
func errResults(err error) []string {
	if err == nil {
		return []string{"", ""}
	}
	return []string{fmt.Sprintf("%T", err), err.Error()}
}

// marshal is jsonutil.Marshal's encoding of v, or "!" and its error.
func marshal(v interface{}) string {
	data, err := jsonutil.Marshal(v)
	if err != nil {
		return "!" + err.Error()
	}
	return string(data)
}

// decode decodes a JSON argument with jsonutil.Unmarshal, as prebid-server decodes the values
// these functions get.
func decode(doc string, v interface{}) {
	if err := jsonutil.Unmarshal([]byte(doc), v); err != nil {
		panic(fmt.Sprintf("argument %s: %v", doc, err))
	}
}

// protect returns f's results, or "panic: " and the panic value when f panics.
func protect(f func() []string) (out []string) {
	defer func() {
		if r := recover(); r != nil {
			out = []string{fmt.Sprintf("panic: %v", r)}
		}
	}()
	return f()
}

func functionCases() []call {
	var calls []call
	add := func(name, label string, args []string, f func() []string) {
		calls = append(calls, call{name: name, label: label, args: args, results: protect(f)})
	}

	for _, s := range []string{
		"banner", "video", "audio", "native", "unknown", "", "Banner", "BANNER", " banner",
		"banner ", "x\xff",
	} {
		add("ParseBidType", s, []string{s}, func() []string {
			bt, err := openrtb_ext.ParseBidType(s)
			return append([]string{string(bt)}, errResults(err)...)
		})
	}
	add("BidTypes", "", nil, func() []string {
		var names []string
		for _, bt := range openrtb_ext.BidTypes() {
			names = append(names, string(bt))
		}
		return []string{strings.Join(names, ",")}
	})

	keys := []string{"_pb", "_bidder_key", "", "é_ü"}
	prefixes := []string{"hb", "", "prebid_"}
	lengths := []int{-5, -1, 0, 1, 2, 5, 8, 9, 10, 15, 20, 50}
	for _, key := range keys {
		for _, prefix := range prefixes {
			for _, bidder := range []string{"appnexus", "", "ünï"} {
				for _, n := range lengths {
					args := []string{key, prefix, bidder, strconv.Itoa(n)}
					add("TargetingKey.BidderKey", "", args, func() []string {
						tk := openrtb_ext.TargetingKey(key)
						return []string{tk.BidderKey(prefix, openrtb_ext.BidderName(bidder), n)}
					})
				}
			}
			for _, n := range lengths {
				add("TargetingKey.TruncateKey", "", []string{key, prefix, strconv.Itoa(n)}, func() []string {
					return []string{openrtb_ext.TargetingKey(key).TruncateKey(prefix, n)}
				})
			}
		}
	}

	add("NewPriceGranularityDefault", "", nil, func() []string {
		return []string{marshal(openrtb_ext.NewPriceGranularityDefault())}
	})
	for _, id := range []string{
		"low", "med", "medium", "high", "auto", "dense", "", "unknown", "LOW", "Med", " low",
		"low ", "mediu", "densee",
	} {
		add("NewPriceGranularityFromLegacyID", id, []string{id}, func() []string {
			pg, ok := openrtb_ext.NewPriceGranularityFromLegacyID(id)
			return []string{marshal(pg), strconv.FormatBool(ok)}
		})
	}

	for _, doc := range []string{
		`{}`, `{"bidder":"pubmatic"}`, `{"maxbids":2}`, `{"bidders":["a","b"]}`, `{"bidders":[]}`,
		`{"bidder":"x","bidders":["a"],"maxbids":0,"targetbiddercodeprefix":"pm"}`, `{"maxbids":-5}`,
		`{"bidders":["a b",""]}`, `{"bidder":"%v{}"}`, `{"bidders":[""]}`,
		`{"maxbids":9223372036854775807}`, `{"targetbiddercodeprefix":"é"}`,
	} {
		add("ExtMultiBid.String", "", []string{doc}, func() []string {
			var mb openrtb_ext.ExtMultiBid
			decode(doc, &mb)
			return []string{mb.String()}
		})
	}

	for _, doc := range []string{
		nilArg,
		`{}`,
		`{"multibid":[]}`,
		`{"multibid":[{"bidder":"pubmatic"}]}`,
		`{"multibid":[{"maxbids":2}]}`,
		`{"multibid":[{"bidder":"pubmatic","maxbids":2,"targetbiddercodeprefix":"pm"},` +
			`{"bidder":"pubmatic","maxbids":3,"targetbiddercodeprefix":"pubm"}]}`,
		`{"multibid":[{"bidder":"pubmatic","maxbids":2,"targetbiddercodeprefix":"pm"},` +
			`{"maxbids":2,"bidder":"appnexus","bidders":["rubicon"],"targetbiddercodeprefix":"appN"}]}`,
		`{"multibid":[{"maxbids":2,"bidder":"appnexus","targetbiddercodeprefix":"appN"}]}`,
		`{"multibid":[{"maxbids":2,"bidders":["appnexus","someBidder"]}]}`,
		`{"multibid":[{"bidder":"pubmatic","maxbids":9,"targetbiddercodeprefix":"pm"},` +
			`{"maxbids":2,"bidders":["appnexus","pubmatic"]}]}`,
		`{"multibid":[{"bidder":"pubmatic","maxbids":9,"targetbiddercodeprefix":"pm"},` +
			`{"bidders":["appnexus"],"maxbids":3},{"maxbids":2,"bidders":["appnexus","pubmatic"]}]}`,
		`{"multibid":[{"maxbids":2,"bidders":["appnexus"],"targetbiddercodeprefix":"appN"}]}`,
		`{"multibid":[{"maxbids":0,"bidder":"appnexus","targetbiddercodeprefix":"appN"}]}`,
		`{"multibid":[{"maxbids":10,"bidder":"appnexus","targetbiddercodeprefix":"appN"}]}`,
		`{"multibid":[{"maxbids":-1,"bidder":"a"},{"maxbids":100,"bidder":"b"}]}`,
		`{"multibid":[{"maxbids":2,"bidders":[]}]}`,
		`{"multibid":[{"maxbids":2,"bidders":["a","a"]}]}`,
		`{"multibid":[{"maxbids":2,"bidder":"a","bidders":[]}]}`,
		`{"multibid":[{"maxbids":2,"bidders":["x"],"targetbiddercodeprefix":"p"},{"maxbids":2,"bidder":"x"}]}`,
		`{"multibid":[{"maxbids":-3,"bidders":["a","b"],"targetbiddercodeprefix":"t"},{"maxbids":12,"bidder":"b"}]}`,
		`{"multibid":[{"maxbids":5,"bidders":["a"]},{"maxbids":5,"bidders":["a"],"targetbiddercodeprefix":"t"}]}`,
		`{"multibid":[null]}`,
		`{"multibid":[{"maxbids":2,"bidder":"a"},null]}`,
	} {
		add("ValidateAndBuildExtMultiBid", "", []string{doc}, func() []string {
			var prebid *openrtb_ext.ExtRequestPrebid
			if doc != nilArg {
				prebid = &openrtb_ext.ExtRequestPrebid{}
				decode(doc, prebid)
			}
			validated, errs := openrtb_ext.ValidateAndBuildExtMultiBid(prebid)
			after := nilArg
			if prebid != nil {
				after = marshal(prebid.MultiBid)
			}
			out := []string{marshal(validated), after}
			for _, err := range errs {
				out = append(out, errResults(err)...)
			}
			return out
		})
	}

	for _, ext := range []string{
		nilArg, ``, `{}`,
		`{"appnexus": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}, "tid": "1234"}`,
		`{"prebid": null}`, `{"prebid": {}}`, `{"prebid": {"supportdeals": true}}`,
		`{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"unknown": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"UnKnOwn": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"APpNExUS": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "placementId": 12345}}, "supportdeals": true}, "tid": "1234"}`,
		`{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "prefix": "appnexusPrefix"}, "placementId": 12345}, "rubicon": {"dealTier": {"minDealTier": 8, "prefix": "rubiconPrefix"}, "placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"appnexus": {"placementId": 12345}}}}`,
		`{"prebid": {"bidder": {"appnexus": {"dealTier": "wrong type", "placementId": 12345}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":null}}}}`,
		`{"prebid":{"bidder":null}}`,
		`{"prebid":{"bidder":{"appnexus":null}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"minDealTier":"5"}}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{}}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"prefix":1}}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"minDealTier":1.5}}}}}`,
		`{"prebid":"x"}`, `[]`, `{"prebid":{"bidder":[]}}`, `malformed`, `{"prebid":{"bidder":{"ix":{"dealTier":{"minDealTier":2}}}}`,
		`{"PREBID":{"BIDDER":{"ADKERNELADN":{"DEALTIER":{"MINDEALTIER":3,"PREFIX":"p"}}}}}`,
		`{"prebid":{"bidder":{"İX":{"dealTier":{"minDealTier":4}}}}}`,
		`{"prebid":{"bidder":{"appnexus":{"dealTier":{"minDealTier":5}},"rubicon":{"dealTier":{"minDealTier":6}}},"bidder":{"pubmatic":{"dealTier":{"minDealTier":7}}}}}`,
	} {
		add("ReadDealTiersFromImp", "", []string{ext}, func() []string {
			var imp openrtb2.Imp
			if ext != nilArg {
				imp.Ext = json.RawMessage(ext)
			}
			tiers, err := openrtb_ext.ReadDealTiersFromImp(imp)
			return append([]string{marshal(tiers)}, errResults(err)...)
		})
	}

	for _, ext := range []string{
		nilArg, ``, `{}`, `{"atts":3}`, `{"atts":5}`, `malformed`, `{"atts":"1"}`, `{"atts":0}`,
		`{"atts":-1}`, `{"atts":1.5}`, `{"atts":null}`, `{"atts":true}`, `{"atts":[]}`,
		`{"atts":99999999999999999999}`, `{"atts":2,"atts":3}`, `{"ATTS":1}`, `[{"atts":1}]`,
		`{"a":{"atts":1}}`, `{"atts":1e2}`, `{"atts":01}`, `{"atts":`, `{"atts":2`, `{"atts":{}}`,
		`{"x":"y","atts":1}`, `{"atts":-0}`, `{"atts":"x"}`,
	} {
		add("ParseDeviceExtATTS", "", []string{ext}, func() []string {
			var raw json.RawMessage
			if ext != nilArg {
				raw = json.RawMessage(ext)
			}
			status, err := openrtb_ext.ParseDeviceExtATTS(raw)
			s := nilArg
			if status != nil {
				s = strconv.Itoa(int(*status))
			}
			return append([]string{s}, errResults(err)...)
		})
	}
	for _, n := range []int64{-9223372036854775808, -1, 0, 1, 2, 3, 4, 5, 9223372036854775807} {
		add("IsKnownIOSAppTrackingStatus", "", []string{strconv.FormatInt(n, 10)}, func() []string {
			return []string{strconv.FormatBool(openrtb_ext.IsKnownIOSAppTrackingStatus(n))}
		})
	}

	for _, s := range []string{
		"", "1~1.35.41.101", "2~1.35.41.101~dv.", "1~", "~1", "1~a.b", "1~1..2", "1~ 1", "1~+1",
		"1~-1", "1~01", "x", "1~1.35~", "1~9223372036854775808", "~", "1~1.2.3", "1~1,2",
		"abc~1.2", "1~0x10", "1~1_000", "1~١",
	} {
		add("ParseConsentedProvidersString", "", []string{s}, func() []string {
			return []string{marshal(openrtb_ext.ParseConsentedProvidersString(s))}
		})
	}

	codes := []string{
		`{}`,
		`{"enabled":true}`,
		`{"enabled":true,"bidders":{"appnexus":{}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":false}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["*"]}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["groupm"]}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["xyz"]}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":[]}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["*","x"]}}}`,
		`{"enabled":true,"bidders":{"pubmatic":{"enabled":true,"allowedbiddercodes":["GROUPM","*"]}}}`,
		`{"enabled":false,"bidders":{"pubmatic":{"enabled":true}}}`,
		`{"enabled":true,"bidders":{"PubMatic":{"enabled":true,"allowedbiddercodes":["groupm"]}}}`,
		`{"enabled":true,"bidders":{"sKip":{"enabled":true,"allowedbiddercodes":["groupm"]}}}`,
	}
	pairs := [][2]string{
		{"", ""}, {"pubmatic", "pubmatic"}, {"pubmatic", "PubMatic"}, {"pubmatic", "groupm"},
		{"PUBmatic", "groupm"}, {"pubmatic", ""}, {"", "groupm"}, {"pubmatic", "GROUPM"},
		{"pubmatic", "*"}, {"skip", "sKip"}, {"SKIP", "groupm"}, {"appnexus", "groupm"},
		{"pub\"matic", "groupé"},
	}
	for _, doc := range codes {
		for _, p := range pairs {
			add("ExtAlternateBidderCodes.IsValidBidderCode", "", []string{doc, p[0], p[1]}, func() []string {
				var c openrtb_ext.ExtAlternateBidderCodes
				decode(doc, &c)
				valid, err := c.IsValidBidderCode(p[0], p[1])
				return append([]string{strconv.FormatBool(valid)}, errResults(err)...)
			})
		}
	}
	for _, doc := range append([]string{nilArg}, codes...) {
		for _, bidder := range []string{"", "pubmatic", "PUBMATIC", "appnexus", "skip", "unknown"} {
			add("ExtAlternateBidderCodes.IsBidderInAlternateBidderCodes", "", []string{doc, bidder}, func() []string {
				var c *openrtb_ext.ExtAlternateBidderCodes
				if doc != nilArg {
					c = &openrtb_ext.ExtAlternateBidderCodes{}
					decode(doc, c)
				}
				cfg, found := c.IsBidderInAlternateBidderCodes(bidder)
				return []string{marshal(cfg), strconv.FormatBool(found)}
			})
		}
	}

	for _, doc := range append([]string{nilArg}, cloneDocs...) {
		add("ExtRequestPrebid.Clone", "", []string{doc}, func() []string {
			var prebid *openrtb_ext.ExtRequestPrebid
			if doc != nilArg {
				prebid = &openrtb_ext.ExtRequestPrebid{}
				decode(doc, prebid)
			}
			return []string{marshal(prebid.Clone())}
		})
	}
	for _, doc := range []string{
		nilArg, `{}`, `{"dsarequired":1}`, `{"pubrender":1}`, `{"datatopub":1}`, `{"transparency":[]}`,
		`{"transparency":[{"domain":"domain1"}]}`, `{"transparency":[{"domain":"domain1","dsaparams":[]}]}`,
		`{"dsarequired":1,"pubrender":1,"datatopub":1,"transparency":[{"domain":"domain1","dsaparams":[1,2,3]},{"domain":"domain2","dsaparams":[4,5,6]}]}`,
		`{"dsarequired":0,"pubrender":0,"datatopub":0,"transparency":null}`,
	} {
		add("ExtRegsDSA.Clone", "", []string{doc}, func() []string {
			var dsa *openrtb_ext.ExtRegsDSA
			if doc != nilArg {
				dsa = &openrtb_ext.ExtRegsDSA{}
				decode(doc, dsa)
			}
			return []string{marshal(dsa.Clone())}
		})
	}

	for _, doc := range append([]string{nilArg}, floorsDocs...) {
		add("PriceFloorRules.Get", "", []string{doc}, func() []string {
			var rules *openrtb_ext.PriceFloorRules
			if doc != nilArg {
				rules = &openrtb_ext.PriceFloorRules{}
				decode(doc, rules)
			}
			return []string{
				strconv.FormatBool(rules.GetEnforcePBS()),
				strconv.FormatBool(rules.GetFloorsSkippedFlag()),
				strconv.Itoa(rules.GetEnforceRate()),
				strconv.FormatBool(rules.GetEnforceDealsFlag()),
				strconv.FormatBool(rules.GetEnabled()),
			}
		})
		add("PriceFloorRules.DeepCopy", "", []string{doc}, func() []string {
			var rules *openrtb_ext.PriceFloorRules
			if doc != nilArg {
				rules = &openrtb_ext.PriceFloorRules{}
				decode(doc, rules)
			}
			return []string{marshal(rules.DeepCopy())}
		})
		if doc == nilArg {
			continue
		}
		var rules openrtb_ext.PriceFloorRules
		decode(doc, &rules)
		if rules.Data != nil {
			data := marshal(rules.Data)
			add("PriceFloorData.DeepCopy", "", []string{data}, func() []string {
				var d openrtb_ext.PriceFloorData
				decode(data, &d)
				return []string{marshal(d.DeepCopy())}
			})
			for _, group := range rules.Data.ModelGroups {
				g := marshal(group)
				add("PriceFloorModelGroup.Copy", "", []string{g}, func() []string {
					var mg openrtb_ext.PriceFloorModelGroup
					decode(g, &mg)
					return []string{marshal(mg.Copy())}
				})
			}
		}
		if rules.Enforcement != nil {
			e := marshal(rules.Enforcement)
			add("PriceFloorEnforcement.DeepCopy", "", []string{e}, func() []string {
				var enf openrtb_ext.PriceFloorEnforcement
				decode(e, &enf)
				return []string{marshal(enf.DeepCopy())}
			})
		}
	}
	add("PriceFloorData.DeepCopy", "nil", []string{nilArg}, func() []string {
		var d *openrtb_ext.PriceFloorData
		return []string{marshal(d.DeepCopy())}
	})
	add("PriceFloorEnforcement.DeepCopy", "nil", []string{nilArg}, func() []string {
		var e *openrtb_ext.PriceFloorEnforcement
		return []string{marshal(e.DeepCopy())}
	})

	for _, doc := range []string{`[]`, `null`, `[{"id":"a"},{"id":""},{}]`, `[{"id":"x","ext":{}}]`} {
		add("GetImpIDs", "", []string{doc}, func() []string {
			var imps []openrtb2.Imp
			decode(doc, &imps)
			return []string{marshal(openrtb_ext.GetImpIDs(imps))}
		})
	}

	for _, name := range bidderNameProbes() {
		add("IsBidderNameReserved", "", []string{name}, func() []string {
			return []string{strconv.FormatBool(openrtb_ext.IsBidderNameReserved(name))}
		})
		add("IsPotentialBidder", "", []string{name}, func() []string {
			return []string{strconv.FormatBool(openrtb_ext.IsPotentialBidder(name))}
		})
		add("NormalizeBidderName", "", []string{name}, func() []string {
			n, ok := openrtb_ext.NormalizeBidderName(name)
			return []string{string(n), strconv.FormatBool(ok)}
		})
		add("NormalizeBidderNameOrUnchanged", "", []string{name}, func() []string {
			return []string{string(openrtb_ext.NormalizeBidderNameOrUnchanged(name))}
		})
	}
	add("CoreBidderNames", "", nil, func() []string {
		var names []string
		for _, n := range openrtb_ext.CoreBidderNames() {
			names = append(names, string(n))
		}
		return []string{strings.Join(names, ",")}
	})
	add("BuildBidderStringSlice", "", nil, func() []string {
		return []string{strings.Join(openrtb_ext.BuildBidderStringSlice(), ",")}
	})
	add("BuildBidderMap", "", nil, func() []string {
		var entries []string
		for k, v := range openrtb_ext.BuildBidderMap() {
			entries = append(entries, k+"="+string(v))
		}
		sort.Strings(entries)
		return []string{strings.Join(entries, ",")}
	})
	add("BuildBidderNameHashSet", "", nil, func() []string {
		var names []string
		for k := range openrtb_ext.BuildBidderNameHashSet() {
			names = append(names, k)
		}
		sort.Strings(names)
		return []string{strings.Join(names, ",")}
	})
	add("GetAliasBidderToParent", "", nil, func() []string {
		return []string{marshal(openrtb_ext.GetAliasBidderToParent())}
	})
	return calls
}

// bidderNameProbes are names to test the bidder name functions with: reserved names in other
// cases, Unicode case folding, and core bidders in other cases.
func bidderNameProbes() []string {
	return []string{
		"all", "ALL", "aLl", "context", "Context", "data", "DATA", "general", "GENERAL", "general ",
		"gpid", "GPID", "prebid", "Prebid", "PREBID", "skadn", "SKADN", "sKadn", "tid", "TiD",
		"ae", "AE", "igs", "IGS", "", " ", "appnexus", "APPNEXUS", "AppNexus", "adkerneladn",
		"ADKERNELADN", "adkernelAdn", "İX", "ix", "IX", "33ACROSS", "freewheel-ssp",
		"FREEWHEEL-SSP", "Kargo", "kargo", "unknown", "appnexus ", "ȧppnexus",
		"sovrnxsp", "SOVRNXSP", "é", "É",
	}
}

// cloneDocs are request.ext.prebid documents for ExtRequestPrebid.Clone: the cases of Go's
// TestCloneExtRequestPrebid and every field that Clone copies on its own.
var cloneDocs = []string{
	`{}`,
	`{"aliases":{"alias1":"bidder1"},"bidadjustmentfactors":{"bidder5":1.2},"bidderparams":{},` +
		`"channel":{"name":"ABC","version":"1.0"},"debug":true,"experiment":{"adscert":{"enabled":false}},` +
		`"server":{"externalurl":"http://www.example.com","gvlid":5,"datacenter":"Universe Central"},"supportdeals":true}`,
	`{"bidderconfig":[{"bidders":["Bidder1","bidder2"],"config":{"ortb2":{"site":{"value":"config1"}}}},` +
		`{"bidders":["Bidder5","bidder17"],"config":{"ortb2":{"app":{"value":"config2"}}}},` +
		`{"bidders":["foo"],"config":{"ortb2":{"user":{"value":"config3"}}}},` +
		`{"bidders":["Bidder9"],"config":{"ortb2":{"device":{"value":"config4"}}}},{"bidders":[]},{"config":{}}]}`,
	`{"cache":{"bids":{"returnCreative":true},"vastxml":{"returnCreative":false}}}`,
	`{"currency":{"rates":{"A":{"X":5.4}}}}`,
	`{"currency":{"usepbsrates":true}}`,
	`{"currency":{"rates":null}}`,
	`{"currency":{"rates":{}}}`,
	`{"data":{"eidpermissions":[{"source":"S1","bidders":["A1"]},{"source":"S2","bidders":["A2","A3"]}],"bidders":["A","B","C"]}}`,
	`{"multibid":[{"bidder":"somebidder","maxbids":2,"targetbiddercodeprefix":"SB"},` +
		`{"bidders":["A","B","C"],"maxbids":3},null]}`,
	`{"schains":[{"bidders":["A"],"schain":{"complete":1,"nodes":[{"asi":"x","sid":"1"}],"ver":"1.0"}},` +
		`{"bidders":["B","C"],"schain":{"complete":0,"nodes":null}},{"bidders":null}]}`,
	`{"storedrequest":{"id":"abc123"}}`,
	`{"targeting":{"pricegranularity":{"precision":4,"ranges":[{"min":0,"max":5,"increment":0.1},` +
		`{"min":5,"max":10,"increment":0.5}]},"mediatypepricegranularity":{"banner":"low","video":{"precision":2}},` +
		`"includewinners":true,"includebidderkeys":false,"includebrandcategory":{"primaryadserver":1,` +
		`"publisher":"Bezos","withcategory":true,"translatecategories":true},"durationrangesec":[1,2,3],` +
		`"preferdeals":true,"appendbiddernames":true,"alwaysincludedeals":true,"includeformat":true,"prefix":"pre"}}`,
	`{"targeting":{"mediatypepricegranularity":{}}}`,
	`{"targeting":{}}`,
	`{"nosale":["A","B","C"]}`,
	`{"alternatebiddercodes":{"enabled":true,"bidders":{"X":{"enabled":true,"allowedbiddercodes":["A","B","C"]},` +
		`"Y":{"enabled":false,"allowedbiddercodes":["C","B","G"]}}}}`,
	`{"floors":{"floormin":1,"floormincur":"USD","skiprate":10,"floorendpoint":{"url":"https://x"},` +
		`"data":{"currency":"USD","skiprate":5,"modelgroups":[{"currency":"USD","modelweight":1,"skiprate":5,` +
		`"values":{"*|*":1.5},"schema":{"fields":["mediaType","size"],"delimiter":"|"}}]},` +
		`"enforcement":{"enforcejs":true,"enforcepbs":true,"floordeals":true,"bidadjustment":true,"enforcerate":100},` +
		`"enabled":true,"skipped":false,"floorprovider":"Prebid","fetchstatus":"success","location":"fetch"}}`,
	`{"multibidmap":{"X":{"bidder":"X","maxbids":2}}}`,
	`{"adservertargeting":[{"key":"A","source":"Sauce","value":"Gold"},{"key":"B","source":"Omega","value":"Dust"}]}`,
	`{"bidadjustments":{"mediatype":{"banner":{"bidderA":{"dealId":[{"adjtype":"multiplier","value":1.1}]}}}}}`,
	`{"passthrough":{"a":1},"returnallbidstatus":true,"integration":"x","macros":{"a":"b"},` +
		`"trace":"verbose","analytics":{"a":1},"events":{},"sdk":{"renderers":[{"name":"n","version":"1",` +
		`"data":{}}]},"biddercontrols":{"appnexus":{"prefmtype":"video"}},"aliasgvlids":{"alias2":42},` +
		`"bidderparams":{"x":1},"debug":false,"supportdeals":false}`,
	`{"aliases":{},"aliasgvlids":{},"bidadjustmentfactors":{},"bidderconfig":[],"multibid":[],"schains":[],` +
		`"nosale":[],"adservertargeting":[],"data":{"eidpermissions":[],"bidders":[]},"experiment":{},` +
		`"cache":{},"alternatebiddercodes":{"bidders":{}},"targeting":{"durationrangesec":[],` +
		`"pricegranularity":{"ranges":[]}},"floors":{"data":{"modelgroups":[]}}}`,
	`{"schains":[{"bidders":["A"],"schain":{"nodes":[{"asi":"a","hp":1},{"asi":"b"}]}},null]}`,
}

// floorsDocs are request.ext.prebid.floors documents for the PriceFloorRules methods: the cases
// of Go's floors tests and nil members at every level.
var floorsDocs = []string{
	`{}`,
	`{"enabled":true}`,
	`{"enabled":false}`,
	`{"skipped":true}`,
	`{"skipped":false,"enabled":true}`,
	`{"enforcement":{}}`,
	`{"enforcement":null}`,
	`{"enabled":true,"enforcement":{"enforcepbs":true}}`,
	`{"enabled":true,"enforcement":{"enforcepbs":false}}`,
	`{"enabled":true,"enforcement":{"enforcepbs":true,"enforcerate":100}}`,
	`{"enabled":true,"enforcement":{"enforcepbs":true,"floordeals":true}}`,
	`{"enabled":true,"enforcement":{"enforcepbs":true,"floordeals":false},"skipped":false}`,
	`{"floormin":10,"floormincur":"INR","floorendpoint":{"url":"https://test/floors"},` +
		`"data":{"currency":"INR","modelgroups":[{"currency":"INR","modelweight":1,` +
		`"values":{"banner|300x600|www.website5.com":20,"*|*|*":50},` +
		`"schema":{"fields":["mediaType","size","domain"],"delimiter":"|"}}]}}`,
	`{"floormin":10,"floormincur":"INR","floorendpoint":{"url":"http://prebid.com/floor"},` +
		`"data":{"currency":"INR","floorsschemaversion":2,"modeltimestamp":123,"modelgroups":[{"currency":"INR",` +
		`"modelweight":50,"modelversion":"version 1","schema":{"fields":["a","b","c"],"delimiter":"|"},` +
		`"values":{"*|*|*":20},"default":1}],"floorprovider":"prebid"},"enforcement":{"enforcejs":true,` +
		`"enforcepbs":true,"floordeals":true,"bidadjustment":true,"enforcerate":100},"enabled":true,` +
		`"skipped":false,"floorprovider":"Prebid","fetchstatus":"success","location":"fetch"}`,
	`{"data":{}}`,
	`{"data":{"modelgroups":[]}}`,
	`{"data":{"modelgroups":[{}]}}`,
	`{"data":{"modelgroups":[{"schema":{"fields":[]},"values":{}}]},"usefetchdatarate":5}`,
	`{"data":{"modelgroups":[{"schema":{},"values":null},{"modelweight":0,"default":0.5}],"usefetchdatarate":0}}`,
	`{"floorendpoint":{},"enforcement":{"enforcerate":-1}}`,
}
