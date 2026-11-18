package main

import (
	"encoding/json"
	"fmt"
	"log"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// The review suites are the scripts of an adversarial review of the port: sequences that the
// corpus and setter suites do not run (an ext object changed after a clone, the original after
// CloneAndClearImpWrappers, exchange/utils.go's per-bidder request, conversions run twice or back
// to back without a rebuild, SetImp with other wrappers than GetImp's, a nil request, setter
// values that do not encode, rebuilds repeated), and requests whose exts hold several members of
// which one does not decode, members that decode only in part, the members Go's Clone functions
// copy field by field, and edge values of the members decoded on their own.
const (
	// reviewSuite runs every review script; the review documents run it.
	reviewSuite = "review"
	// reviewCorpusSuite is the review scripts that the corpus requests run.
	reviewCorpusSuite = "review corpus"
)

// preload is exchange/utils.go's PreloadExts (every ext but the app's).
var preload = []string{"get:request", "get:user", "get:device", "get:regs", "get:site", "get:dooh", "get:source"}

func init() {
	review := map[string][]string{
		"review clone touch":      append(append(each("get:"), "clone", "touch:all", "rebuild"), each("get:")...),
		"review cloneclear touch": append(append(each("get:"), "cloneclear", "touch:all", "rebuild"), each("get:")...),
		"review cloneclear keep": {`set:imp.tid="k"`, `set:user.consent="k"`, "cloneclearkeep", "lenimp", "rebuild",
			"get:imp", "touch:imp", "rebuild", "cloneclearkeep", "dropimp", "getimp", "rebuild"},
		"review exchange": append(append(append([]string{}, preload...), "exchangecopy",
			`set:request.prebid={"debug":true}`, "down", "rebuild", "get:app"), "touch:all", "rebuild"),
		"review up up":          {"up", "up", "rebuild", "get:user", "get:regs", "get:source"},
		"review up down":        {"up", "down", "rebuild", "touch:all", "rebuild"},
		"review down down":      {"down", "down", "rebuild"},
		"review down up":        {"down", "up", "rebuild", "up", "rebuild"},
		"review imp reorder":    {`set:imp.tid="r"`, "setimprev", "rebuild", "getimp", "get:imp", "setimprev", "rebuild"},
		"review imp drop":       {"getimp", "setimpdrop", "lenimp", "rebuild", "get:imp", "setimpdrop", "rebuild"},
		"review imp nil":        {"getimp", "setimpnil", "lenimp", "rebuild", "getimp", "lenimp", "rebuild"},
		"review touch twice":    {"touch:all", "rebuild", "touch:all", "rebuild", "touchext:all", "rebuild"},
		"review clear up":       {"clear26", "up", "rebuild", "down", "rebuild"},
		"review special nan":    {"special:request.prebid.nan", "rebuild", "get:request", "rebuild"},
		"review special inf":    {`set:user.consent="x"`, "special:imp.prebid.inf", "rebuild", "get:user", "rebuild"},
		"review special badraw": {"special:user.eids.badraw", "rebuild", "get:user", "up", "rebuild"},
		"review special spaced": {"special:source.schain.spacedraw", "rebuild", "get:source", "up", "rebuild",
			"down", "rebuild"},
		// exchange/utils.go's per-bidder request: the copy, the bidder's impressions, applyPrivacy's
		// partial clone and ScrubTID, then removeImpsWithStoredResponses after the wrappers exist,
		// the down conversion and the rebuild (which brings the removed impressions back).
		"review exchange flow": append(append([]string{}, preload...), "exchangecopy", "assignrev",
			`set:request.prebid={"debug":true}`, "partialclone", `setimpraw={"scrubbed":1}`, "rebuild",
			"assignlast", "lenimp", "down", "rebuild", "lenimp", "getimp", "rebuild"),
		"review exchange no wrappers": append(append([]string{}, preload...), "exchangecopy", "assignrev",
			"partialclone", "rebuild", "assignlast", "lenimp", "down", "rebuild", "getimp", "rebuild"),
		// a nil request, after the wrappers and some ext objects exist, and from the start
		"review nil request": {"getimp", "get:user", `set:user.consent="n"`, "nilreq", "lenimp", "get:imp",
			"get:request", "get:device", "get:app", "get:regs", "get:site", "get:dooh", "get:source",
			"touch:all", "set:regs.gdpr=1", "rebuild", "getimp", "rebuildimp", "lenimp", "rebuild"},
		"review nil fresh": {"nilreq", "get:request", "get:user", "get:device", "get:app", "get:regs",
			"get:site", "get:dooh", "get:source", "touch:request", "touch:user", "touch:device", "touch:app",
			"touch:regs", "touch:site", "touch:dooh", "touch:source", "rebuild"},
	}
	names := []string{"review clone touch", "review cloneclear touch", "review cloneclear keep",
		"review exchange", "review up up", "review up down", "review down down", "review down up",
		"review imp reorder", "review imp drop", "review imp nil", "review touch twice", "review clear up",
		"review special nan", "review special inf", "review special badraw", "review special spaced",
		"review exchange flow", "review exchange no wrappers", "review nil request", "review nil fresh"}
	for _, name := range names {
		if _, ok := scripts[name]; ok {
			log.Fatalf("script %s defined twice", name)
		}
		scripts[name] = review[name]
	}
	suites[reviewSuite] = names
	suites[reviewCorpusSuite] = []string{"review clone touch", "review up down", "review exchange flow"}
}

// reviewExts are the review's ext values, by object.
var reviewExts = map[string][]string{
	"user": {
		// several members, one of which does not decode: those after it are not decoded
		`{"consent":1,"prebid":{"buyeruids":{"a":"b"}},"eids":[{"source":"s"}]}`,
		`{"consent":"c","prebid":1,"eids":[{"source":"s"}],"ConsentedProvidersSettings":{"consented_providers":"1~2"}}`,
		`{"consent":"c","prebid":{"buyeruids":{"a":"b"}},"eids":1,"consented_providers_settings":{"consented_providers":[1]}}`,
		`{"consent":"c","eids":[{"source":"s"}],"ConsentedProvidersSettings":1,"consented_providers_settings":{"consented_providers":[1]}}`,
		// members that decode in part
		`{"prebid":{"buyeruids":{"a":"x","b":1,"c":"y"}}}`,
		`{"eids":[{"source":"a"},{"source":1},{"source":"c"}]}`,
		`{"eids":[{"source":"a","uids":[{"id":"1"},{"id":2},{"id":"3"}]}]}`,
		`{"consented_providers_settings":{"consented_providers":[1,"x",3]}}`,
		`{"ConsentedProvidersSettings":{"consented_providers":5}}`,
		// nil members, duplicate and escaped keys
		`{"eids":null}`,
		`{"prebid":null,"eids":null,"consent":null,"ConsentedProvidersSettings":null,"consented_providers_settings":null}`,
		`{"consent":"a","consent":"b"}`,
		`{"consent":1,"consent":"b"}`,
		`{"consent":"esc","prebid":{"buyeruids":{"a":"b"}}}`,
		`{"consent":"a","Consent":"b"}`,
		`{"eids":[{"source":"s","uids":[{"id":"1","atype":1,"ext":{ "b" : 2 , "a" : 1 }}],"ext":{ "z" : [ 1 ] }}],"data":{"k":"v"}}`,
	},
	"request": {
		// fields that Go's ExtRequestPrebid.Clone copies field by field (or leaves out)
		`{"prebid":{"currency":{"usepbsrates":true}}}`,
		`{"prebid":{"currency":{"rates":null,"usepbsrates":null}}}`,
		`{"prebid":{"currency":{"rates":{"USD":{"EUR":0.9}},"usepbsrates":false}}}`,
		`{"prebid":{"targeting":{"pricegranularity":"medium","mediatypepricegranularity":{"banner":"low","video":{"precision":3,"ranges":[{"max":5,"increment":0.5}]}},"alwaysincludedeals":true,"includewinners":true,"includebidderkeys":false,"includeformat":true,"durationrangesec":[1,2],"preferdeals":true,"appendbiddernames":true,"prefix":"p","includebrandcategory":{"primaryadserver":1,"publisher":"x","withcategory":true,"translatecategories":false}}}}`,
		`{"prebid":{"targeting":{"alwaysincludedeals":true}}}`,
		`{"prebid":{"bidderconfig":[{"bidders":["a"],"config":{"ortb2":{"site":{"id":"1"},"user":{ "x" : 1 }}}},{"bidders":null},{"config":{}}]}}`,
		`{"prebid":{"data":{"eidpermissions":[{"source":"s","bidders":["a"]},{"source":"t"}],"bidders":["a"]}}}`,
		`{"prebid":{"data":{"bidders":["a"]}}}`,
		`{"prebid":{"schains":[{"bidders":["a"],"schain":{"complete":1,"nodes":null,"ver":"1"}},{"bidders":["b"],"schain":{"complete":1,"nodes":[{"asi":"x","hp":1}],"ver":"1"}}]}}`,
		`{"prebid":{"experiment":{"adscert":{"enabled":true}}},"schain":{"complete":1,"nodes":null,"ver":"1"}}`,
		`{"prebid":{"experiment":{}},"schain":{"complete":1,"nodes":[{"asi":"a","hp":null}],"ver":"1"}}`,
		`{"prebid":{"cache":{"bids":{"returnCreative":true},"vastxml":{}}}}`,
		`{"prebid":{"cache":{}}}`,
		`{"prebid":{"multibid":[{"bidder":"a","maxbids":2,"targetbiddercodeprefix":"p"},{"bidders":["b","c"]}]}}`,
		`{"prebid":{"alternatebiddercodes":{"enabled":true,"bidders":{"a":{"enabled":true,"allowedbiddercodes":["*"]},"b":{}}}}}`,
		`{"prebid":{"alternatebiddercodes":{"enabled":false}}}`,
		`{"prebid":{"floors":{"enabled":true,"floormin":1.5,"enforcement":{"enforcepbs":true,"enforcerate":50},"data":{"modelgroups":[{"modelweight":1,"schema":{"fields":["mediaType"]},"values":{"banner":1.0}}]},"location":{"url":"u"}}}}`,
		`{"prebid":{"aliases":{"x":"appnexus"},"aliasgvlids":{"x":32},"bidadjustmentfactors":{"a":0.5},"debug":true,"integration":"i","channel":{"name":"app","version":"1"},"server":{"externalurl":"u","gvlid":1,"datacenter":"d"},"storedrequest":{"id":"s"},"sdk":{"renderers":[{"name":"n","version":"1"}]},"macros":{"a":"b"},"nosale":["*"],"returnallbidstatus":true,"trace":"verbose","biddercontrols":{"a":{"prefmtype":"banner"}},"passthrough":{ "x" : 1 },"events":{ "x" : 1 },"bidderparams":{ "a" : { "b" : 1 } },"analytics":{"a":{ "b" : 1 }},"adservertargeting":[{"key":"k","source":"bidrequest","value":"v"}],"supportdeals":true}}`,
		// members that do not decode, or decode in part
		`{"prebid":{"debug":"yes"}}`,
		`{"prebid":{"targeting":{"pricegranularity":"bad"}}}`,
		`{"prebid":{"aliases":{"a":"b","c":1,"d":"e"},"debug":true}}`,
		`{"schain":{"complete":"x"},"prebid":{"debug":true}}`,
		`{"prebid":1,"schain":{"ver":"1"}}`,
		`{"prebid":{"debug":true},"prebid":{"integration":"x"}}`,
	},
	"source": {
		`{"schain":{"complete":1,"nodes":null,"ver":"1"}}`,
		`{"schain":{"complete":1,"nodes":[{"asi":"a","hp":null},{"asi":"b","hp":0}],"ver":"1"}}`,
		`{"schain":{"complete":1,"nodes":[{"asi":"a","sid":1}]}}`,
		`{"schain":{"complete":1,"nodes":[]},"z":{ "a" : 1 }}`,
	},
	"regs": {
		`{"gdpr":-1}`, `{"gdpr":127}`, `{"gdpr":128}`, `{"gdpr":-128}`, `{"gdpr":-129}`, `{"gdpr":1.0}`,
		`{"gdpr":1e0}`, `{"gdpr":-0}`, `{"gdpr":true}`, `{"gdpr":[1]}`, `{"gdpr":{}}`, `{"gdpr":"x","gdpr":1}`,
		`{"gpc":1e2}`, `{"gpc":-0}`, `{"gpc":1.50}`, `{"gpc":[1, 2]}`, `{"gpc":{"a" : 1}}`, `{"gpc":"1"}`,
		`{"gpc":false}`, `{"gpc":" "}`, `{"gpc":12345678901234567890}`, `{"gpc":"a\"b"}`,
		`{"gpc":-12}`, `{"gpc":0.1e1}`, `{"gpc":""}`,
		`{"us_privacy":"1YNN"}`, `{"us_privacy":"1YNN","us_privacy":""}`, `{"us_privacy":"<&>"}`,
		`{"dsa":{"dsarequired":"x"}}`, `{"dsa":{"transparency":[{"domain":"d","dsaparams":[1,"x"]}]}}`,
		`{"dsa":null,"gdpr":null,"us_privacy":null,"gpc":null}`, `{"dsa":{}}`,
		`{"dsa":1,"gdpr":1,"us_privacy":"u","gpc":"1"}`,
		`{"gdpr":"x","us_privacy":"u","gpc":"1"}`,
		`{"gdpr":1,"us_privacy":1,"gpc":"1"}`,
	},
	"site": {
		`{"amp":-1}`, `{"amp":127}`, `{"amp":128}`, `{"amp":1.0}`, `{"amp":true}`, `{"amp":"x","data":{"a":1}}`,
		`{"amp":1,"data":{ "a" : 1 }}`,
	},
	"app": {
		`{"prebid":{"source":1}}`,
		`{"prebid":{"source":"s","version":"v","x":1},"data":{ "a" : 1 }}`,
	},
	"device": {
		`{"prebid":{"interstitial":{"minwidthperc":"x"}},"cdep":"c"}`,
		`{"cdep":"c","prebid":1}`,
		`{"atts":3,"prebid":{"interstitial":null},"ifa_type":"x"}`,
		`{"cdep":1,"prebid":{"interstitial":{"minwidthperc":1}}}`,
	},
	"dooh": {
		`{"a":{ "b" : [ 1 , 2 ] },"c":null}`,
	},
	"imp": {
		`{"prebid":{"bidder":{"appnexus":{"placementId":1},"rubicon":{ "a" : [ 1 ] }}},"appnexus":{"placementId":2}}`,
		`{"prebid":{"is_rewarded_inventory":true}}`,
		`{"prebid":{"storedbidresponse":[{"bidder":"a","id":"1","replaceimpid":true}],"storedauctionresponse":{"id":"x"},"options":{"echovideoattrs":true},"floors":{"floorrule":"r","floorrulevalue":1.5},"passthrough":{ "p" : 1 },"imp":{"a":{ "x" : 1 }}}}`,
		`{"tid":"t","gpid":"g","data":{"pbadslot":"p"},"prebid":1}`,
		`{"data":1,"tid":"t"}`,
		`{"tid":1,"gpid":"g"}`,
		`{"gpid":1,"tid":"t"}`,
		`{"prebid":{"bidder":null}}`,
		`{"prebid":{"bidder":{}}}`,
		`{"prebid":{"is_rewarded_inventory":0}}`,
		`{"prebid":{"bidder":{"a":{"b":1},"c":1}}}`,
		`{"prebid":{"storedrequest":{"id":1}}}`,
		`{"tid":"{{UUID}}","context":{"data":{"k":"v"}},"skadn":{ "v" : 1 }}`,
	},
}

// reviewStructureDocs are review requests built around the request rather than one ext.
var reviewStructureDocs = [][2]string{
	{"review: second imp ext invalid", `{"id":"r","imp":[{"id":"1","ext":{"prebid":{"is_rewarded_inventory":1}}},{"id":"2","ext":{"prebid":1}},{"id":"3","rwdd":1}]}`},
	{"review: eids in both, 2.6 empty", `{"id":"r","imp":[{"id":"1"}],"user":{"eids":[],"ext":{"eids":[{"source":"x"}]}}}`},
	{"review: eids in both, 2.6 null", `{"id":"r","imp":[{"id":"1"}],"user":{"eids":null,"ext":{"eids":[{"source":"x"}]}}}`},
	{"review: eids 2.5 empty", `{"id":"r","imp":[{"id":"1"}],"user":{"ext":{"eids":[]}}}`},
	{"review: eids 2.5 null", `{"id":"r","imp":[{"id":"1"}],"user":{"ext":{"eids":null}}}`},
	{"review: consent in both, 2.6 empty", `{"id":"r","imp":[{"id":"1"}],"user":{"consent":"","ext":{"consent":"x"}}}`},
	{"review: consent 2.5 empty", `{"id":"r","imp":[{"id":"1"}],"user":{"consent":"y","ext":{"consent":""}}}`},
	{"review: gdpr in both", `{"id":"r","imp":[{"id":"1"}],"regs":{"gdpr":0,"ext":{"gdpr":1}}}`},
	{"review: gdpr 2.5 only", `{"id":"r","imp":[{"id":"1"}],"regs":{"ext":{"gdpr":0}}}`},
	{"review: us_privacy in both, 2.6 empty", `{"id":"r","imp":[{"id":"1"}],"regs":{"us_privacy":"","ext":{"us_privacy":"x"}}}`},
	{"review: schain 2.5 and 2.4, 2.6 null", `{"id":"r","imp":[{"id":"1"}],"source":{"schain":null,"ext":{"schain":{"ver":"25"}}},"ext":{"schain":{"ver":"24"}}}`},
	{"review: schain 2.4 empty, no source", `{"id":"r","imp":[{"id":"1"}],"ext":{"schain":{}}}`},
	{"review: schain 2.4 null", `{"id":"r","imp":[{"id":"1"}],"ext":{"schain":null,"prebid":{"debug":true}}}`},
	{"review: rwdd and is_rewarded_inventory 0", `{"id":"r","imp":[{"id":"1","rwdd":1,"ext":{"prebid":{"is_rewarded_inventory":0}}},{"id":"2","rwdd":0,"ext":{"prebid":{"is_rewarded_inventory":0}}},{"id":"3","rwdd":-1}]}`},
	{"review: rwdd into invalid imp ext", `{"id":"r","imp":[{"id":"1","rwdd":1,"ext":{"prebid":{"bidder":1}}}]}`},
	{"review: schain 2.6 into invalid source ext", `{"id":"r","imp":[{"id":"1","rwdd":1}],"source":{"schain":{"ver":"26"},"ext":{"schain":1}},"regs":{"gdpr":1},"user":{"consent":"c"}}`},
	{"review: gdpr 2.6 into invalid regs ext", `{"id":"r","imp":[{"id":"1","rwdd":1}],"regs":{"gdpr":1,"us_privacy":"u","ext":{"gdpr":"x"}},"user":{"consent":"c"}}`},
	{"review: consent 2.6 into invalid user ext", `{"id":"r","imp":[{"id":"1","rwdd":1}],"regs":{"us_privacy":"u"},"user":{"consent":"c","eids":[{"source":"s"}],"ext":{"eids":1}}}`},
	{"review: eids 2.6 into invalid user ext", `{"id":"r","imp":[{"id":"1","rwdd":1}],"user":{"eids":[{"source":"s"}],"ext":{"prebid":1}}}`},
	{"review: many imps", `{"id":"r","imp":[{"id":"1","ext":{"tid":"a","z":{ "y" : 1 }}},{"id":"2"},{"id":"3","ext":{}},{"id":"4","ext":{"prebid":{"bidder":{"a":{}}}}},{"id":"5","rwdd":1,"ext":{"gpid":"g"}}]}`},
	{"review: full", `{"id":"r","imp":[{"id":"1","rwdd":1,"ext":{"prebid":{"bidder":{"a":{ "p" : 1 }}},"tid":"t","gpid":"g","data":{"pbadslot":"s"}}},{"id":"2","ext":{"prebid":{"is_rewarded_inventory":1}}}],` +
		`"ext":{"prebid":{"debug":true,"currency":{"usepbsrates":true},"targeting":{"alwaysincludedeals":true,"mediatypepricegranularity":{"banner":"low"}}},"schain":{"complete":1,"nodes":null,"ver":"1"},"z":[ 1 ]},` +
		`"site":{"ext":{"amp":1,"data":{"a":1}}},"app":{"ext":{"prebid":{"source":"s"}}},"dooh":{"ext":{"z":1}},` +
		`"device":{"ext":{"prebid":{"interstitial":{"minwidthperc":1}},"cdep":"c","atts":1}},` +
		`"user":{"consent":"26","ext":{"consent":"25","prebid":{"buyeruids":{"a":"1"}},"eids":null,"ConsentedProvidersSettings":{"consented_providers":"1~2"},"consented_providers_settings":{"consented_providers":[2]}}},` +
		`"regs":{"gdpr":1,"ext":{"dsa":{"dsarequired":1},"gdpr":0,"us_privacy":"u","gpc":"1"}},` +
		`"source":{"ext":{"schain":{"complete":1,"nodes":null,"ver":"2"}}}}`},
}

// reviewEncodingDocs are review requests whose ext keys and member values hold characters that
// jsonutil.Marshal escapes, and others that it writes as they are. (Bytes that are not valid UTF-8
// are left out: the port holds strings as Rust strings, a documented difference.)
var reviewEncodingDocs = [][2]string{
	{"review: escaped characters", `{"id":"r","imp":[{"id":"1","ext":{"tid":"  <>&\u0000\u001f\u007f","gpid":"\ud800"}}],` +
		`"user":{"ext":{"consent":"é日😀\"\\\/","é":"é"}},"regs":{"ext":{"us_privacy":"\ud83d","gpc":"\udc00x"}}}`},
	{"review: raw characters", "{\"id\":\"r\",\"imp\":[{\"id\":\"1\",\"ext\":{\"tid\":\" é\U0001F600\x7f\"}}]," +
		"\"user\":{\"ext\":{\"consent\":\"日 \",\"日\":\"日\"}}}"},
}

// reviewRows are the review requests (each running the corpus suite and the review suite) and
// the corpus requests (each running the review corpus suite).
func reviewRows(corpusDocs []doc) []row {
	var rows []row
	add := func(label, data string) {
		var req openrtb2.BidRequest
		if err := jsonutil.Unmarshal([]byte(data), &req); err != nil {
			log.Fatalf("%s: %s: %v", label, data, err)
		}
		if !json.Valid([]byte(data)) {
			log.Fatalf("%s: invalid JSON", label)
		}
		d := doc{ref: "'" + data, data: []byte(data), label: label}
		rows = append(rows, row{doc: d, suite: corpusSuite}, row{doc: d, suite: reviewSuite})
	}
	for _, obj := range objects {
		for i, ext := range reviewExts[obj] {
			add(fmt.Sprintf("review %s ext %d", obj, i), withExt(obj, ext))
		}
	}
	for _, d := range reviewStructureDocs {
		add(d[0], d[1])
	}
	for _, d := range reviewEncodingDocs {
		add(d[0], d[1])
	}
	for _, d := range corpusDocs {
		rows = append(rows, row{doc: d, suite: reviewCorpusSuite})
	}
	return rows
}
