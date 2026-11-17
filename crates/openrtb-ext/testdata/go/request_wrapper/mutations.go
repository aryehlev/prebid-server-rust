package main

import (
	"encoding/json"
	"fmt"
	"log"
	"strings"

	"github.com/prebid/openrtb/v20/openrtb2"

	"github.com/prebid/prebid-server/v4/util/jsonutil"
)

// genericExts are ext values that every object gets: other JSON kinds, members with whitespace,
// duplicate and escaped keys, and strings and numbers that re-encoding would change.
var genericExts = []string{
	`null`,
	`{}`,
	`[]`,
	`1`,
	`"s"`,
	`true`,
	`{ "a" : 1 , "b" : [ 1 , { "c" : null } ] }`,
	`{"a":1,"a":{"b":2}}`,
	`{"a":1,"b\n":2,"é":3}`,
	`{"a":"é😀<>&  é","b":"\/"}`,
	`{"a":1e3,"b":-0.0,"c":12345678901234567890,"d":1.50}`,
	`{"a":{}}`,
}

// members are the members that an object's ext decodes on its own, with values of every kind:
// valid ones, empty ones, null and wrong types.
var members = map[string][][2]string{
	"request": {
		{"prebid", `{"debug":true}`},
		{"prebid", `{}`},
		{"prebid", `null`},
		{"prebid", `1`},
		{"prebid", `{"unknown":1,"debug":false,"integration":"x"}`},
		{"prebid", `{"targeting":{"pricegranularity":"medium","includewinners":true},"channel":{"name":"web","version":"1"}}`},
		{"prebid", `{"bidders":{"a":{"x" : 1}},"data":{"bidders":["a"]},"floors":{"enabled":true,"floormin":0.5}}`},
		{"schain", `{"complete":1,"nodes":[{"asi":"a","sid":"s","hp":1}],"ver":"1.0"}`},
		{"schain", `{}`},
		{"schain", `null`},
		{"schain", `1`},
		{"schain", `{"complete":1,"nodes":null,"ver":"1"}`},
	},
	"user": {
		{"consent", `"c"`},
		{"consent", `""`},
		{"consent", `null`},
		{"consent", `1`},
		{"prebid", `{"buyeruids":{"a":"b"}}`},
		{"prebid", `{}`},
		{"prebid", `null`},
		{"prebid", `[]`},
		{"prebid", `{"buyeruids":{}}`},
		{"prebid", `{"x":1}`},
		{"eids", `[{"source":"s","uids":[{"id":"1","atype":1}]}]`},
		{"eids", `[]`},
		{"eids", `null`},
		{"eids", `{}`},
		{"eids", `[{"source":"s","uids":[]}]`},
		{"eids", `[{"source":"s","uids":null,"ext":{ "z" : 1 }}]`},
		{"ConsentedProvidersSettings", `{"consented_providers":"1~2.3"}`},
		{"ConsentedProvidersSettings", `{}`},
		{"ConsentedProvidersSettings", `null`},
		{"ConsentedProvidersSettings", `1`},
		{"consented_providers_settings", `{"consented_providers":[1,2]}`},
		{"consented_providers_settings", `{"consented_providers":[]}`},
		{"consented_providers_settings", `null`},
		{"consented_providers_settings", `{"consented_providers":"x"}`},
	},
	"device": {
		{"prebid", `{"interstitial":{"minwidthperc":50,"minheightperc":60}}`},
		{"prebid", `{}`},
		{"prebid", `null`},
		{"prebid", `1`},
		{"cdep", `"x"`},
		{"cdep", `""`},
		{"cdep", `null`},
		{"cdep", `1`},
	},
	"app": {
		{"prebid", `{"source":"s","version":"v"}`},
		{"prebid", `{}`},
		{"prebid", `null`},
		{"prebid", `1`},
	},
	"regs": {
		{"dsa", `{"dsarequired":1,"pubrender":2,"datatopub":1,"transparency":[{"domain":"d","dsaparams":[1,2]}]}`},
		{"dsa", `{}`},
		{"dsa", `null`},
		{"dsa", `1`},
		{"gdpr", `1`},
		{"gdpr", `0`},
		{"gdpr", `null`},
		{"gdpr", `"1"`},
		{"gdpr", `1.5`},
		{"gdpr", `300`},
		{"us_privacy", `"1YNN"`},
		{"us_privacy", `""`},
		{"us_privacy", `null`},
		{"us_privacy", `1`},
		{"gpc", `"1"`},
		{"gpc", `1`},
		{"gpc", `null`},
		{"gpc", `true`},
		{"gpc", `1.5`},
		{"gpc", `{}`},
	},
	"site": {
		{"amp", `1`},
		{"amp", `0`},
		{"amp", `null`},
		{"amp", `"1"`},
		{"amp", `2`},
	},
	"dooh": {},
	"source": {
		{"schain", `{"complete":1,"nodes":[{"asi":"a","sid":"s","hp":1,"ext":{ "z" : 1 }}],"ver":"1.0"}`},
		{"schain", `{}`},
		{"schain", `null`},
		{"schain", `1`},
		{"schain", `{"complete":1,"nodes":null,"ver":"1"}`},
	},
	"imp": {
		{"prebid", `{"is_rewarded_inventory":1}`},
		{"prebid", `{"bidder":{"appnexus":{"placementId":1}},"storedrequest":{"id":"s"}}`},
		{"prebid", `{}`},
		{"prebid", `null`},
		{"prebid", `1`},
		{"prebid", `{"is_rewarded_inventory":2,"options":{"echovideoattrs":true}}`},
		{"data", `{"pbadslot":"x","adserver":{"name":"gam","adslot":"/1"}}`},
		{"data", `{}`},
		{"data", `null`},
		{"data", `1`},
		{"tid", `"t"`},
		{"tid", `""`},
		{"tid", `null`},
		{"tid", `1`},
		{"gpid", `"g"`},
		{"gpid", `""`},
		{"gpid", `null`},
		{"gpid", `1`},
	},
}

// withExt is a request whose object obj has the ext ext: the request's own ext, the ext of the
// (only) impression, or the ext of the object.
func withExt(obj, ext string) string {
	switch obj {
	case "request":
		return `{"id":"r","imp":[{"id":"1"}],"ext":` + ext + `}`
	case "imp":
		return `{"id":"r","imp":[{"id":"1","ext":` + ext + `},{"id":"2"}]}`
	}
	return `{"id":"r","imp":[{"id":"1"}],"` + obj + `":{"id":"o","ext":` + ext + `}}`
}

// structureDocs are requests that the impression wrappers, the conversions and the Clear*
// functions treat differently: no impressions, empty ones, ORTB 2.6 fields with and without
// their 2.5 ext locations, and every field the Clear* functions clear.
var structureDocs = [][2]string{
	{"empty", `{}`},
	{"no imp", `{"id":"r"}`},
	{"null imp", `{"id":"r","imp":null}`},
	{"empty imp", `{"id":"r","imp":[]}`},
	{"three imps", `{"id":"r","imp":[{"id":"1","ext":{"tid":"a"}},{"id":"2","rwdd":1},{"id":"3","ext":{"prebid":{"is_rewarded_inventory":1}}}]}`},
	{"objects without ext", `{"id":"r","imp":[{"id":"1"}],"site":{},"app":{},"dooh":{},"device":{},"user":{},"regs":{},"source":{}}`},
	{"objects with null ext", `{"id":"r","imp":[{"id":"1","ext":null}],"ext":null,"site":{"ext":null},"app":{"ext":null},"dooh":{"ext":null},"device":{"ext":null},"user":{"ext":null},"regs":{"ext":null},"source":{"ext":null}}`},
	{"2.6 fields", `{"id":"r","imp":[{"id":"1","rwdd":1}],"source":{"schain":{"complete":1,"nodes":[],"ver":"2"}},"regs":{"gdpr":1,"us_privacy":"1YNN"},"user":{"consent":"c","eids":[{"source":"s"}]}}`},
	{"2.6 fields, empty eids", `{"id":"r","imp":[{"id":"1","rwdd":0}],"source":{"schain":{"complete":0,"nodes":null,"ver":""}},"regs":{"gdpr":0},"user":{"eids":[]}}`},
	{"2.5 fields", `{"id":"r","imp":[{"id":"1","ext":{"prebid":{"is_rewarded_inventory":1}}}],"source":{"ext":{"schain":{"complete":1,"nodes":[],"ver":"2"}}},"regs":{"ext":{"gdpr":1,"us_privacy":"1YNN"}},"user":{"ext":{"consent":"c","eids":[{"source":"s"}]}}}`},
	{"2.4 fields", `{"id":"r","imp":[{"id":"1"}],"ext":{"schain":{"complete":1,"nodes":[],"ver":"2"},"prebid":{"debug":true}}}`},
	{"2.4 fields without source", `{"id":"r","ext":{"schain":{"complete":1,"nodes":[{"asi":"x"}],"ver":"2"}}}`},
	{"2.4, 2.5 and 2.6 fields", `{"id":"r","imp":[{"id":"1","rwdd":2,"ext":{"prebid":{"is_rewarded_inventory":1},"tid":"t"}}],"ext":{"schain":{"ver":"24"}},"source":{"schain":{"ver":"26"},"ext":{"schain":{"ver":"25"}}},"regs":{"gdpr":0,"us_privacy":"26","ext":{"gdpr":1,"us_privacy":"25"}},"user":{"consent":"26","eids":[{"source":"26"}],"ext":{"consent":"25","eids":[{"source":"25"}]}}}`},
	{"2.5 fields without objects", `{"id":"r","regs":{"ext":{"gdpr":1}},"user":{"ext":{"consent":"c"}}}`},
	{"2.5 fields, empty", `{"id":"r","imp":[{"id":"1","ext":{"prebid":{"is_rewarded_inventory":0}}}],"regs":{"ext":{"gdpr":0,"us_privacy":""}},"user":{"ext":{"consent":"","eids":[]}}}`},
	{"clear fields", clearFieldsDoc},
}

// clearFieldsDoc has every field that the Clear* functions clear.
const clearFieldsDoc = `{"id":"r","wlangb":["en"],"cattax":4,"acat":["a"],` +
	`"app":{"cattax":4,"kwarray":["k"],"inventorypartnerdomain":"d","content":{"id":"c","cattax":4,"kwarray":["k"],"langb":"en","network":{"id":"n"},"channel":{"id":"ch"},"producer":{"id":"p","cattax":4}},"publisher":{"id":"pub","cattax":4}},` +
	`"site":{"cattax":4,"kwarray":["k"],"inventorypartnerdomain":"d","content":{"id":"c","cattax":4,"producer":{"id":"p","cattax":4}},"publisher":{"id":"pub","cattax":4}},` +
	`"dooh":{"id":"d"},` +
	`"device":{"ip":"1.2.3.4","langb":"en","sua":{"model":"m"}},` +
	`"regs":{"coppa":1,"gdpr":1,"us_privacy":"u","gpp":"g","gpp_sid":[1,2]},` +
	`"source":{"tid":"t","schain":{"complete":1}},` +
	`"user":{"id":"u","kwarray":["k"],"consent":"c","eids":[{"source":"s","inserter":"i","matcher":"m","mm":2,"uids":[{"id":"x"}]}]},` +
	`"imp":[{"id":"1","rwdd":1,"ssai":3,"qty":{"multiplier":2},"dt":42,"refresh":{"count":1},` +
	`"audio":{"mimes":["a"],"poddur":30,"rqddurs":[15],"podid":"1","podseq":1,"slotinpod":1,"mincpmpersec":1.5,"durfloors":[{"mindur":1,"maxdur":2,"bidfloor":3}]},` +
	`"video":{"mimes":["v"],"maxseq":3,"poddur":30,"podid":"1","podseq":1,"rqddurs":[15],"slotinpod":1,"mincpmpersec":1.5,"plcmt":1,"poddedupe":[1],"durfloors":[{"mindur":1}]},` +
	`"pmp":{"private_auction":1,"deals":[{"id":"d","bidfloor":1,"guar":1,"mincpmpersec":2,"durfloors":[{"mindur":1}]}]}}]}`

// setterValues are the arguments of the set steps of each member: null, empty and set values.
var setterValues = map[string][]string{
	"request.prebid": {`null`, `{}`, `{"debug":true}`, `{"targeting":{"pricegranularity":{"precision":2,"ranges":[{"max":20,"increment":0.1}]}}}`},
	"request.schain": {`null`, `{}`, `{"complete":1,"nodes":[{"asi":"a"}],"ver":"1.0"}`},
	"user.consent":   {`null`, `""`, `"c<&>"`},
	"user.prebid":    {`null`, `{}`, `{"buyeruids":{"b":"2","a":"1"}}`},
	"user.eids":      {`null`, `&nil`, `[]`, `[{"source":"s","uids":[{"id":"1"}]}]`},
	"user.cpsin":     {`null`, `{}`, `{"consented_providers":"1~2"}`},
	"user.cpsout":    {`null`, `{}`, `{"consented_providers":[3,1]}`},
	"device.prebid":  {`null`, `{}`, `{"interstitial":{"minwidthperc":1,"minheightperc":2}}`},
	"device.cdep":    {`""`, `"x"`},
	"app.prebid":     {`null`, `{}`, `{"source":"s"}`},
	"regs.dsa":       {`null`, `{}`, `{"dsarequired":3}`},
	"regs.gdpr":      {`null`, `0`, `1`},
	"regs.gpc":       {`null`, `""`, `"1"`},
	"regs.usprivacy": {`""`, `"1YNN"`},
	"site.amp":       {`null`, `0`, `1`},
	"source.schain":  {`null`, `{}`, `{"complete":0,"nodes":[],"ver":""}`},
	"imp.prebid":     {`null`, `{}`, `{"is_rewarded_inventory":1}`},
	"imp.tid":        {`""`, `"t"`},
}

// setterOrder is the order of setterValues' members.
var setterOrder = []string{"request.prebid", "request.schain", "user.consent", "user.prebid",
	"user.eids", "user.cpsin", "user.cpsout", "device.prebid", "device.cdep", "app.prebid",
	"regs.dsa", "regs.gdpr", "regs.gpc", "regs.usprivacy", "site.amp", "source.schain",
	"imp.prebid", "imp.tid"}

// setterBases are the requests the setter suite runs on: without objects, with objects without
// ext, with exts that hold every member and others, and with null exts (Go's nil maps).
var setterBases = [][2]string{
	{"setter base: no objects", `{"id":"r","imp":[{"id":"1"}]}`},
	{"setter base: objects without ext", `{"id":"r","imp":[{"id":"1"}],"site":{},"app":{},"dooh":{},"device":{},"user":{},"regs":{},"source":{}}`},
	{"setter base: full exts", `{"id":"r","imp":[{"id":"1","ext":{"prebid":{"is_rewarded_inventory":1},"tid":"t","gpid":"g","z": [ 1 ]}}],` +
		`"ext":{"prebid":{"debug":true},"schain":{"ver":"1"},"z": [ 1 ]},` +
		`"site":{"ext":{"amp":1,"z": [ 1 ]}},"app":{"ext":{"prebid":{"source":"s"},"z": [ 1 ]}},"dooh":{"ext":{"z": [ 1 ]}},` +
		`"device":{"ext":{"prebid":{"interstitial":{"minwidthperc":1}},"cdep":"c","z": [ 1 ]}},` +
		`"user":{"ext":{"consent":"c","prebid":{"buyeruids":{"a":"1"}},"eids":[{"source":"s"}],"ConsentedProvidersSettings":{"consented_providers":"1~2"},"consented_providers_settings":{"consented_providers":[2]},"z": [ 1 ]}},` +
		`"regs":{"ext":{"dsa":{"dsarequired":1},"gdpr":1,"us_privacy":"u","gpc":"1","z": [ 1 ]}},` +
		`"source":{"ext":{"schain":{"ver":"1"},"z": [ 1 ]}}}`},
	{"setter base: null exts", `{"id":"r","imp":[{"id":"1","ext":null}],"ext":null,"site":{"ext":null},"app":{"ext":null},"dooh":{"ext":null},"device":{"ext":null},"user":{"ext":null},"regs":{"ext":null},"source":{"ext":null}}`},
}

// setterSuite is the suite of the setter bases: each set step followed by a rebuild and a get of
// the object, and the ext map steps.
const setterSuite = "setters"

func init() {
	var names []string
	add := func(name string, steps ...string) {
		if _, ok := scripts[name]; ok {
			log.Fatalf("script %s defined twice", name)
		}
		scripts[name] = steps
		names = append(names, name)
	}
	for _, member := range setterOrder {
		obj, _, _ := strings.Cut(member, ".")
		for _, value := range setterValues[member] {
			add("set "+member+" "+value, "set:"+member+"="+value, "rebuild", "get:"+obj, "touch:"+obj, "rebuild")
		}
	}
	for _, obj := range objects {
		add("setext "+obj+" null", "setext:"+obj+"=null", "rebuild", "get:"+obj)
		add("setext "+obj+" members", "get:"+obj, `setext:`+obj+`={"consent":"x","prebid":{"z":1},"gdpr":"x","amp":7,"tid":3,"schain":{"ver":"x"},"y": { "z" : [ 1 , 2 ] }}`, "rebuild", "get:"+obj)
		add("setext "+obj+" empty", "setext:"+obj+"={}", "rebuild", "get:"+obj, "touch:"+obj, "rebuild")
		add("setrawext "+obj, "setrawext:"+obj+"=bad=malformed", "rebuild")
		add("setrawext "+obj+" spaced", "setrawext:"+obj+"=k= { \"a\" : [ 1 ] } ", "rebuild")
	}
	for _, member := range setterOrder {
		obj, _, _ := strings.Cut(member, ".")
		values := setterValues[member]
		last := values[len(values)-1]
		add("nil map "+member, "setext:"+obj+"=null", "set:"+member+"="+values[0], "rebuild", "setext:"+obj+"=null", "set:"+member+"="+last, "rebuild")
	}
	add("touchext only", "touchext:all", "rebuild", "get:user", "rebuild")
	add("set twice", `set:user.consent="a"`, "rebuild", `set:user.consent="b"`, "set:regs.gdpr=1", "rebuild", "set:user.consent=null", "rebuild")
	add("convert after set", `set:user.consent="c"`, `set:regs.usprivacy="u"`, `set:user.eids=[{"source":"s"}]`, "set:imp.prebid={\"is_rewarded_inventory\":1}", "rebuild", "up", "rebuild", "down", "rebuild")
	add("clone after set", `set:user.consent="c"`, "touch:all", "clone", "get:user", "get:regs", "get:imp", "rebuild", "cloneclear", "get:imp", "rebuild")
	add("imp wrappers", "getimp", "assignrev", "getimp", "lenimp", "setimp", "getimp", "dropimp", "lenimp", "rebuild", "getimp", "setimp")
	suites[setterSuite] = names
}

// mutations are the rows of the generated requests: each generic ext and each member value of
// every object, and the structure documents, with the corpus suite; and the setter bases with
// the setter suite.
func mutations() []row {
	var rows []row
	add := func(label, data, suite string) {
		var req openrtb2.BidRequest
		if err := jsonutil.Unmarshal([]byte(data), &req); err != nil {
			log.Fatalf("%s: %s: %v", label, data, err)
		}
		if !json.Valid([]byte(data)) {
			log.Fatalf("%s: invalid JSON", label)
		}
		rows = append(rows, row{doc: doc{ref: "'" + data, data: []byte(data), label: label}, suite: suite})
	}
	for _, obj := range objects {
		for i, ext := range genericExts {
			add(fmt.Sprintf("%s ext generic %d", obj, i), withExt(obj, ext), corpusSuite)
		}
		for i, m := range members[obj] {
			ext := `{"` + m[0] + `":` + m[1] + `,"other": [ 1 , "x" ]}`
			add(fmt.Sprintf("%s ext %s %d", obj, m[0], i), withExt(obj, ext), corpusSuite)
		}
	}
	for _, d := range structureDocs {
		add(d[0], d[1], corpusSuite)
	}
	for _, d := range setterBases {
		add(d[0], d[1], corpusSuite)
		add(d[0], d[1], setterSuite)
	}
	return rows
}
