package main

import (
	"encoding/json"
	"fmt"
	"math/rand"
	"strconv"
	"strings"

	"github.com/prebid/go-gdpr/api"
	"github.com/prebid/go-gdpr/consentconstants"
	"github.com/prebid/go-gdpr/vendorlist2"
)

// vendorListCase records vendorlist2.ParseEagerly (gdpr/vendorlist-fetching.go).
type vendorListCase struct {
	Name        string       `json:"name"`
	InputHex    string       `json:"input_hex,omitempty"`
	Input       *string      `json:"input,omitempty"`
	Err         string       `json:"err,omitempty"`
	Panic       string       `json:"panic,omitempty"`
	SpecVersion uint16       `json:"spec_version"`
	Version     uint16       `json:"version"`
	Vendors     []vendorDump `json:"vendors,omitempty"`
}

// vendorDump lists, for one vendor, the ids in [0, 255] each accessor returns true for.
type vendorDump struct {
	ID              int   `json:"id"`
	Purposes        []int `json:"purposes"`
	PurposesStrict  []int `json:"purposes_strict"`
	LI              []int `json:"li"`
	LIStrict        []int `json:"li_strict"`
	SpecialPurposes []int `json:"special_purposes"`
	SpecialFeatures []int `json:"special_features"`
}

func newVendorListCase(name string, input []byte) vendorListCase {
	c := vendorListCase{Name: name}
	c.Input, c.InputHex = encodeInput(input)
	var list api.VendorList
	var err error
	if msg, panicked := catch(func() { list, err = vendorlist2.ParseEagerly(input) }); panicked {
		c.Panic = msg
		return c
	}
	if err != nil {
		c.Err = err.Error()
		return c
	}
	c.SpecVersion = list.SpecVersion()
	c.Version = list.Version()
	for id := 0; id <= 65535; id++ {
		v := list.Vendor(uint16(id))
		if v == nil {
			continue
		}
		d := vendorDump{ID: id}
		for p := 0; p < 256; p++ {
			pid := consentconstants.Purpose(p)
			if v.Purpose(pid) {
				d.Purposes = append(d.Purposes, p)
			}
			if v.PurposeStrict(pid) {
				d.PurposesStrict = append(d.PurposesStrict, p)
			}
			if v.LegitimateInterest(pid) {
				d.LI = append(d.LI, p)
			}
			if v.LegitimateInterestStrict(pid) {
				d.LIStrict = append(d.LIStrict, p)
			}
			if v.SpecialPurpose(pid) {
				d.SpecialPurposes = append(d.SpecialPurposes, p)
			}
			if v.SpecialFeature(consentconstants.SpecialFeature(p)) {
				d.SpecialFeatures = append(d.SpecialFeatures, p)
			}
		}
		c.Vendors = append(c.Vendors, d)
	}
	return c
}

// The vendor lists used by the go-gdpr vendorlist2 tests.
const testDataSpecVersion2 = `
{
	"gvlSpecificationVersion": 2,
	"vendorListVersion": 28,
	"tcfPolicyVersion": 2,
	"lastUpdated": "2020-03-05T16:05:29Z",
	"vendors": {
		"8": {
			"id": 8,
			"name": "Emerse Sverige AB",
			"purposes": [1, 3, 4],
			"legIntPurposes": [2, 7, 8, 9],
			"flexiblePurposes": [2, 9],
			"specialPurposes": [1, 2],
			"features": [1, 2],
			"specialFeatures": [1, 2],
			"policyUrl": "https://www.emerse.com/privacy-policy/"
		},
		"80": {
			"id": 80,
			"name": "Sharethrough, Inc",
			"purposes": [1, 2, 4, 7, 9, 10],
			"legIntPurposes": [],
			"flexiblePurposes": [2, 4, 7, 9, 10],
			"specialPurposes": [],
			"features": [],
			"specialFeatures": [],
			"policyUrl": "https://platform-cdn.sharethrough.com/privacy-policy"
		}
	}
}
`

const testDataSpecVersion2Empty = `
{
	"gvlSpecificationVersion": 2,
	"vendorListVersion": 28,
	"tcfPolicyVersion": 2,
	"lastUpdated": "2020-03-05T16:05:29Z",
	"vendors": { }
}
`

const testDataSpecVersion3 = `
{
	"gvlSpecificationVersion": 3,
	"vendorListVersion": 1,
	"tcfPolicyVersion": 4,
	"lastUpdated": "2023-05-18T16:07:14Z",
	"vendors": {
		"8": {
			"id": 8,
			"name": "Emerse Sverige AB",
			"purposes": [1, 3, 4],
			"legIntPurposes": [2, 7, 8, 9],
			"flexiblePurposes": [2, 9],
			"specialPurposes": [1, 2],
			"features": [1, 2],
			"specialFeatures": [1, 2],
			"dataRetention": {
				"stdRetention": 30,
				"purposes": { "9": 180 },
				"specialPurposes": {}
			},
		   "dataDeclaration" : [ 1, 2, 4, 6 ],
		   "urls": [
			  {
				"langId": "en",
				"privacy": "https://vendorname.com/gdpr.html",
				"legIntClaim": "https://vendorname.com/gdpr.html#li"
			  },
			  {
				"langId": "fr",
				"privacy": "https://vendorname.com/fr/gdpr.html",
				"legIntClaim": "https://vendorname.com/fr/gdpr.html#li"
			  }
		   ]
		},
		"80": {
			"id": 80,
			"name": "Sharethrough, Inc",
			"purposes": [1, 2, 4, 7, 9, 10],
			"legIntPurposes": [],
			"flexiblePurposes": [2, 4, 7, 9, 10],
			"specialPurposes": [],
			"features": [],
			"specialFeatures": [],
			"dataRetention": {
				"stdRetention": 30,
				"purposes": { "9": 180 },
				"specialPurposes": {}
			},
		   "dataDeclaration" : [ 1, 2, 4, 6 ],
		   "urls": [
			  {
				"langId": "en",
				"privacy": "https://vendorname.com/gdpr.html",
				"legIntClaim": "https://vendorname.com/gdpr.html#li"
			  },
			  {
				"langId": "fr",
				"privacy": "https://vendorname.com/fr/gdpr.html",
				"legIntClaim": "https://vendorname.com/fr/gdpr.html#li"
			  }
		   ]
		}
	}
}
`

const testDataSpecVersion3Empty = `
{
	"gvlSpecificationVersion": 3,
	"vendorListVersion": 1,
	"tcfPolicyVersion": 4,
	"lastUpdated": "2023-05-18T16:07:14Z",
	"vendors": { }
}
`

// smallList is mutated byte by byte to reach every scanner error state.
const smallList = `{"gvlSpecificationVersion":2,"vendorListVersion":5,"vendors":{"1":{"id":1,"purposes":[1,2],"legIntPurposes":[3],"flexiblePurposes":[2,3],"specialPurposes":[1],"specialFeatures":[2]}},"x":[true,false,null,-1.5e+3,0,"sé\n\"\\\/\b\f\r\t"]}`

type gvlPurpose struct {
	ID              int      `json:"id"`
	Name            string   `json:"name"`
	Description     string   `json:"description"`
	Illustrations   []string `json:"illustrations"`
	ConsentRequired *bool    `json:"consentable,omitempty"`
}

type gvlVendor struct {
	ID                         int              `json:"id"`
	Name                       string           `json:"name"`
	Purposes                   []int            `json:"purposes"`
	LegIntPurposes             []int            `json:"legIntPurposes"`
	FlexiblePurposes           []int            `json:"flexiblePurposes"`
	SpecialPurposes            []int            `json:"specialPurposes"`
	Features                   []int            `json:"features"`
	SpecialFeatures            []int            `json:"specialFeatures"`
	CookieMaxAgeSeconds        *int64           `json:"cookieMaxAgeSeconds,omitempty"`
	UsesCookies                bool             `json:"usesCookies"`
	CookieRefresh              bool             `json:"cookieRefresh"`
	UsesNonCookieAccess        bool             `json:"usesNonCookieAccess"`
	DeviceStorageDisclosureURL string           `json:"deviceStorageDisclosureUrl,omitempty"`
	DataRetention              map[string]any   `json:"dataRetention,omitempty"`
	URLs                       []map[string]any `json:"urls,omitempty"`
	DataDeclaration            []int            `json:"dataDeclaration,omitempty"`
	DeletedDate                string           `json:"deletedDate,omitempty"`
	Overflow                   map[string]int   `json:"overflow,omitempty"`
}

func randomIDs(r *rand.Rand, max, count int) []int {
	ids := []int{}
	seen := map[int]bool{}
	for i := 0; i < count; i++ {
		id := 1 + r.Intn(max)
		if !seen[id] {
			seen[id] = true
			ids = append(ids, id)
		}
	}
	return ids
}

// realisticGVL builds a document shaped like the IAB vendor-list.json files.
func realisticGVL(r *rand.Rand, spec, version, vendors int) []byte {
	var b strings.Builder
	fmt.Fprintf(&b, "{\n  \"gvlSpecificationVersion\": %d,\n  \"vendorListVersion\": %d,\n  \"tcfPolicyVersion\": %d,\n  \"lastUpdated\": \"2024-05-30T16:05:28Z\",\n", spec, version, spec+2)
	purposes := map[string]gvlPurpose{}
	for i := 1; i <= 11; i++ {
		purposes[strconv.Itoa(i)] = gvlPurpose{ID: i, Name: fmt.Sprintf("Purpose %d – \"quoted\"", i), Description: "Store and/or access <information> & more", Illustrations: []string{"line one\nline two"}}
	}
	writeField(&b, "purposes", purposes)
	writeField(&b, "specialPurposes", map[string]gvlPurpose{"1": {ID: 1, Name: "Security"}, "2": {ID: 2, Name: "Deliver"}, "3": {ID: 3, Name: "Save"}})
	writeField(&b, "features", map[string]gvlPurpose{"1": {ID: 1, Name: "Match"}, "2": {ID: 2, Name: "Link"}, "3": {ID: 3, Name: "Identify"}})
	writeField(&b, "specialFeatures", map[string]gvlPurpose{"1": {ID: 1, Name: "Geo"}, "2": {ID: 2, Name: "Scan"}})
	writeField(&b, "stacks", map[string]any{"1": map[string]any{"id": 1, "purposes": []int{}, "specialFeatures": []int{1, 2}, "name": "Precise geolocation", "description": "x"}})
	if spec >= 3 {
		writeField(&b, "dataCategories", map[string]any{"1": map[string]any{"id": 1, "name": "IP addresses", "description": "Your IP address"}})
	}
	vs := map[string]gvlVendor{}
	for i := 0; i < vendors; i++ {
		id := 1 + r.Intn(1400)
		age := int64(r.Intn(100000000))
		v := gvlVendor{
			ID:                  id,
			Name:                fmt.Sprintf("Vendor é %d", id),
			Purposes:            randomIDs(r, 11, r.Intn(8)),
			LegIntPurposes:      randomIDs(r, 11, r.Intn(5)),
			FlexiblePurposes:    randomIDs(r, 11, r.Intn(4)),
			SpecialPurposes:     randomIDs(r, 3, r.Intn(3)),
			Features:            randomIDs(r, 3, r.Intn(3)),
			SpecialFeatures:     randomIDs(r, 2, r.Intn(3)),
			CookieMaxAgeSeconds: &age,
			UsesCookies:         r.Intn(2) == 0,
			CookieRefresh:       r.Intn(2) == 0,
			UsesNonCookieAccess: r.Intn(2) == 0,
		}
		if r.Intn(3) == 0 {
			v.DeviceStorageDisclosureURL = "https://example.com/deviceStorage.json"
		}
		if spec >= 3 {
			v.DataRetention = map[string]any{"stdRetention": 365, "purposes": map[string]int{"2": 30}, "specialPurposes": map[string]int{}}
			v.URLs = []map[string]any{{"langId": "en", "privacy": "https://example.com/p", "legIntClaim": "https://example.com/li"}}
			v.DataDeclaration = randomIDs(r, 11, 4)
		}
		if r.Intn(10) == 0 {
			v.DeletedDate = "2023-01-01T00:00:00Z"
		}
		if r.Intn(5) == 0 {
			v.Overflow = map[string]int{"httpGetLimit": 32}
		}
		vs[strconv.Itoa(id)] = v
	}
	writeField(&b, "vendors", vs)
	s := strings.TrimSuffix(b.String(), ",\n")
	return []byte(s + "\n}\n")
}

func writeField(b *strings.Builder, name string, v any) {
	data, err := json.MarshalIndent(v, "  ", "  ")
	if err != nil {
		fail(err)
	}
	fmt.Fprintf(b, "  %q: %s,\n", name, data)
}

func buildVendorListCases() []vendorListCase {
	var out []vendorListCase
	add := func(name, doc string) { out = append(out, newVendorListCase(name, []byte(doc))) }

	add("go-gdpr testDataSpecVersion2", testDataSpecVersion2)
	add("go-gdpr testDataSpecVersion2Empty", testDataSpecVersion2Empty)
	add("go-gdpr testDataSpecVersion3", testDataSpecVersion3)
	add("go-gdpr testDataSpecVersion3Empty", testDataSpecVersion3Empty)
	add("small list", smallList)

	r := rand.New(rand.NewSource(3))
	out = append(out, newVendorListCase("realistic v2", realisticGVL(r, 2, 200, 120)))
	out = append(out, newVendorListCase("realistic v3", realisticGVL(r, 3, 70, 150)))
	out = append(out, newVendorListCase("realistic v3 small", realisticGVL(r, 3, 4095, 5)))

	for _, c := range vendorListQuirks {
		add(c[0], c[1])
	}
	for _, c := range escapeCases() {
		add(c[0], c[1])
	}

	deep := func(n int) string { return strings.Repeat("[", n) + strings.Repeat("]", n) }
	add("nesting depth 10000", `{"vendorListVersion":1,"x":`+deep(9999)+`}`)
	add("nesting depth 10001 arrays", `{"vendorListVersion":1,"x":`+deep(10000)+`}`)
	add("nesting depth 10001 objects", `{"vendorListVersion":1,"x":`+strings.Repeat(`{"a":`, 10000)+"1"+strings.Repeat("}", 10000)+"}")
	add("nesting top-level depth 10000", deep(10000))
	add("nesting top-level depth 10001", deep(10001))

	for n := 0; n < len(smallList); n++ {
		add(fmt.Sprintf("small list prefix %d", n), smallList[:n])
	}
	inserts := []string{"x", "}", "]", ",", ":", "\"", "\x01", "\xff", " ", "{", "[", "0", "-", ".", "e", "\\", "n", "t", "f"}
	for p := 0; p <= len(smallList); p++ {
		for i, ins := range inserts {
			if (p+i)%3 != 0 {
				continue
			}
			add(fmt.Sprintf("small list insert %q at %d", ins, p), smallList[:p]+ins+smallList[p:])
		}
	}
	for p := 0; p < len(smallList); p++ {
		add(fmt.Sprintf("small list delete at %d", p), smallList[:p]+smallList[p+1:])
	}
	for b := 0; b < 256; b++ {
		add(fmt.Sprintf("leading byte %d", b), string([]byte{byte(b)})+`{}`)
		add(fmt.Sprintf("byte %d in string", b), `{"name":"`+string([]byte{byte(b)})+`","vendorListVersion":1}`)
		add(fmt.Sprintf("byte %d after value", b), `{"vendorListVersion":1 `+string([]byte{byte(b)})+`}`)
	}
	return out
}

// vendorListQuirks are hand-written documents covering encoding/json decoding rules.
var vendorListQuirks = [][2]string{
	{"empty", ""},
	{"whitespace", " \t\r\n"},
	{"null", "null"},
	{"null padded", " null \n"},
	{"empty object", "{}"},
	{"array", "[]"},
	{"array of objects", `[{"vendorListVersion":1}]`},
	{"array of the members", `[3,7,{"5":{"id":5,"purposes":[1]}}]`},
	{"number", "1"},
	{"negative number", "-1"},
	{"string", `"x"`},
	{"true", "true"},
	{"false", "false"},
	{"two values", `{"vendorListVersion":1} {}`},
	{"trailing garbage", `{"vendorListVersion":1}x`},
	{"trailing space", "{\"vendorListVersion\":1} \n\t\r"},
	{"bom", "\xef\xbb\xbf{\"vendorListVersion\":1}"},
	{"version only", `{"vendorListVersion":1}`},
	{"version zero", `{"vendorListVersion":0,"vendors":{"1":{"id":1}}}`},
	{"version null", `{"vendorListVersion":null}`},
	{"version null after value", `{"vendorListVersion":3,"vendorListVersion":null}`},
	{"version duplicate", `{"vendorListVersion":3,"vendorListVersion":4}`},
	{"version string", `{"vendorListVersion":"28"}`},
	{"version float", `{"vendorListVersion":28.0}`},
	{"version exponent", `{"vendorListVersion":2e1}`},
	{"version negative", `{"vendorListVersion":-1}`},
	{"version negative zero", `{"vendorListVersion":-0}`},
	{"version 65535", `{"vendorListVersion":65535}`},
	{"version 65536", `{"vendorListVersion":65536}`},
	{"version huge", `{"vendorListVersion":123456789012345678901234567890}`},
	{"version bool", `{"vendorListVersion":true}`},
	{"version array", `{"vendorListVersion":[1]}`},
	{"version object", `{"vendorListVersion":{"a":1}}`},
	{"spec string", `{"gvlSpecificationVersion":"3","vendorListVersion":1}`},
	{"spec float then version", `{"gvlSpecificationVersion":3.5,"vendorListVersion":1}`},
	{"spec 65536", `{"gvlSpecificationVersion":65536,"vendorListVersion":9}`},
	{"first error wins", `{"vendorListVersion":"a","gvlSpecificationVersion":true}`},
	{"type error then version zero", `{"gvlSpecificationVersion":"x"}`},
	{"type error then syntax error", `{"vendorListVersion":"a",}`},
	{"case insensitive keys", `{"GVLSPECIFICATIONVERSION":3,"VendorListVersion":7,"VENDORS":{"5":{"ID":5,"PURPOSES":[1],"LegIntPurposes":[2],"flexiblepurposes":[3],"SpecialPurposes":[1],"SPECIALFEATURES":[2]}}}`},
	{"long s key", "{\"vendorListVersion\":7,\"vendors\":{\"5\":{\"id\":5,\"ſpecialFeatures\":[1],\"purpoſeſ\":[4]}}}"},
	{"kelvin key", "{\"vendorListVersion\":7,\"vendors\":{\"5\":{\"id\":5,\"Kurposes\":[1]}}}"},
	{"dotless i key", "{\"vendorListVersion\":7,\"vendors\":{\"5\":{\"ıd\":5}}}"},
	{"escaped key", `{"vendorListVersion":7,"vendors":{"5":{"id":5,"purposes":[1]}}}`},
	{"exact and folded duplicate", `{"vendorListVersion":7,"VENDORLISTVERSION":8}`},
	{"unknown fields", `{"vendorListVersion":7,"purposes":{"1":{"id":1}},"stacks":[1,{"a":[null,true]}],"lastUpdated":"x","tcfPolicyVersion":4}`},
	{"vendors null", `{"vendorListVersion":7,"vendors":null}`},
	{"vendors null then object", `{"vendorListVersion":7,"vendors":null,"vendors":{"2":{"id":2}}}`},
	{"vendors object then null", `{"vendorListVersion":7,"vendors":{"2":{"id":2}},"vendors":null}`},
	{"vendors merged", `{"vendorListVersion":7,"vendors":{"2":{"id":2,"purposes":[1]}},"vendors":{"3":{"id":3,"purposes":[2]}}}`},
	{"vendors array", `{"vendorListVersion":7,"vendors":[{"id":2}]}`},
	{"vendors number", `{"vendorListVersion":7,"vendors":5}`},
	{"vendors string", `{"vendorListVersion":7,"vendors":"x"}`},
	{"vendors bool", `{"vendorListVersion":7,"vendors":false}`},
	{"vendor null", `{"vendorListVersion":7,"vendors":{"2":null}}`},
	{"vendor number", `{"vendorListVersion":7,"vendors":{"2":2}}`},
	{"vendor array", `{"vendorListVersion":7,"vendors":{"2":[1]}}`},
	{"vendor array of the members", `{"vendorListVersion":7,"vendors":{"5":[5,[1],[2],[3],[4],[5]]}}`},
	{"vendor string", `{"vendorListVersion":7,"vendors":{"2":"x"}}`},
	{"vendor empty", `{"vendorListVersion":7,"vendors":{"2":{}}}`},
	{"vendor missing id", `{"vendorListVersion":7,"vendors":{"2":{"purposes":[1,2]}}}`},
	{"vendor id 0", `{"vendorListVersion":7,"vendors":{"0":{"id":0,"purposes":[3]}}}`},
	{"vendor id 65535", `{"vendorListVersion":7,"vendors":{"65535":{"id":65535,"purposes":[3]}}}`},
	{"vendor id 65536", `{"vendorListVersion":7,"vendors":{"1":{"id":65536}}}`},
	{"vendor id string", `{"vendorListVersion":7,"vendors":{"1":{"id":"1"}}}`},
	{"vendor id negative", `{"vendorListVersion":7,"vendors":{"1":{"id":-4}}}`},
	{"vendor id float", `{"vendorListVersion":7,"vendors":{"1":{"id":1.5}}}`},
	{"vendor key differs from id", `{"vendorListVersion":7,"vendors":{"abc":{"id":9,"purposes":[1]}}}`},
	{"vendor duplicate key", `{"vendorListVersion":7,"vendors":{"9":{"id":9,"purposes":[1]},"9":{"id":9,"purposes":[2]}}}`},
	{"vendor duplicate key different id", `{"vendorListVersion":7,"vendors":{"9":{"id":9,"purposes":[1]},"9":{"id":10,"purposes":[2]}}}`},
	{"vendor same id identical", `{"vendorListVersion":7,"vendors":{"9":{"id":9,"purposes":[1]},"x":{"id":9,"purposes":[1]}}}`},
	{"vendor key invalid utf8 collide", "{\"vendorListVersion\":7,\"vendors\":{\"\xff\":{\"id\":1},\"\xfe\":{\"id\":2}}}"},
	{"purposes 255", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[0,255,24,25]}}}`},
	{"purposes 256", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,256,2]}}}`},
	{"purposes negative", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[-1]}}}`},
	{"purposes float", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1.0]}}}`},
	{"purposes exponent", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1e0]}}}`},
	{"purposes string element", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":["1"]}}}`},
	{"purposes bool element", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[true]}}}`},
	{"purposes array element", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[[1]]}}}`},
	{"purposes object element", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[{"a":1}]}}}`},
	{"purposes null element", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[null,3]}}}`},
	{"purposes null", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":null}}}`},
	{"purposes number", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":3}}}`},
	{"purposes object", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":{"1":true}}}}`},
	{"purposes bool", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":true}}}`},
	{"purposes base64 string", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQID","specialFeatures":"Ag=="}}}`},
	{"purposes base64 empty", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":""}}}`},
	{"purposes base64 invalid", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQI"}}}`},
	{"purposes base64 bad char", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"A-I="}}}`},
	{"purposes base64 escaped", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQ=="}}}`},
	{"purposes base64 newline", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQ\n=="}}}`},
	{"base64 error then type error", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"!","legIntPurposes":5}}}`},
	{"duplicate purposes shorter", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3],"purposes":[4]}}}`},
	{"duplicate purposes nulls reuse", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3],"purposes":[4],"purposes":[null,null,null]}}}`},
	{"duplicate purposes nulls past cap", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3,4,5,6,7,8],"purposes":[9],"purposes":[null,null,null,null,null,null,null,null,null,null]}}}`},
	{"duplicate purposes nulls cap 16", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17],"purposes":[],"purposes":[null,null]}}}`},
	{"duplicate purposes nulls after null", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3],"purposes":null,"purposes":[null,null]}}}`},
	{"duplicate purposes nulls after base64", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQIDBAUG","purposes":[9],"purposes":[null,null,null,null,null,null,null]}}}`},
	{"duplicate purposes nulls after type error", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3],"purposes":[300,null],"purposes":[null,null,null]}}}`},
	{"null elements fresh", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[null,null,7,null]}}}`},
	{"flexible only", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"flexiblePurposes":[1,2]}}}`},
	{"vendor fields duplicated across vendors", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1,2,3]},"2":{"id":2,"purposes":[null]}}}`},
	{"strings with escapes", `{"vendorListVersion":7,"x":"\u0000𐀀\ud800xé\/","vendors":{}}`},
	{"invalid escape", `{"vendorListVersion":7,"x":"\x"}`},
	{"invalid unicode escape", `{"vendorListVersion":7,"x":"\u12g4"}`},
	{"single quote escape", `{"vendorListVersion":7,"x":"\'"}`},
	{"control char in string", "{\"vendorListVersion\":7,\"x\":\"\t\"}"},
	{"invalid utf8 in string", "{\"vendorListVersion\":7,\"x\":\"\xff\xfe\"}"},
	{"leading zero", `{"vendorListVersion":07}`},
	{"plus number", `{"vendorListVersion":+7}`},
	{"dot number", `{"vendorListVersion":.7}`},
	{"number trailing dot", `{"vendorListVersion":7.}`},
	{"number trailing e", `{"vendorListVersion":7e}`},
	{"number e sign only", `{"vendorListVersion":7e+}`},
	{"minus only", `{"vendorListVersion":-}`},
	{"huge exponent unknown field", `{"vendorListVersion":7,"x":1e999999}`},
	{"literal typo true", `{"vendorListVersion":7,"x":tru}`},
	{"literal typo false", `{"vendorListVersion":7,"x":fals}`},
	{"literal typo null", `{"vendorListVersion":7,"x":nul}`},
	{"literal wrong", `{"vendorListVersion":7,"x":nil}`},
	{"top literal truncated", `tru`},
	{"top number minus", `-`},
	{"top number dot", `1.`},
	{"top string unterminated", `"abc`},
	{"missing colon", `{"vendorListVersion" 7}`},
	{"missing value", `{"vendorListVersion":}`},
	{"missing comma", `{"vendorListVersion":7 "vendors":{}}`},
	{"trailing comma object", `{"vendorListVersion":7,}`},
	{"trailing comma array", `{"vendorListVersion":7,"x":[1,]}`},
	{"single quoted key", `{'vendorListVersion':7}`},
	{"unquoted key", `{vendorListVersion:7}`},
	{"unclosed object", `{"vendorListVersion":7`},
	{"unclosed array", `{"vendorListVersion":7,"x":[1`},
	{"mismatched close", `{"vendorListVersion":7,"x":[1}`},
	{"comment", `{"vendorListVersion":7 /* c */}`},
	{"nan", `{"vendorListVersion":NaN}`},
	{"many vendors", manyVendors()},
}

// escapeCases cover JSON escapes of code units in keys, map keys and base64 strings. u("0070")
// is the six bytes backslash, 'u', "0070"; the escapes are assembled at run time so that no
// editing tool can turn them into the characters they stand for.
func escapeCases() [][2]string {
	u := func(hex string) string { return `\` + "u" + hex }
	return [][2]string{
		{"escaped field names", `{"` + u("0076") + `endorListVersion":7,"vendors":{"` + u("0035") + `":{"` + u("0069") + `d":5,"` + u("0070") + `urposes":[1],"` + u("017f") + `pecialFeatures":[2],"specialPurpose` + u("0053") + `":[3]}}}`},
		{"escaped strings", `{"vendorListVersion":7,"x":"` + u("0000") + u("d800") + u("dc00") + u("d800") + "x" + u("00e9") + u("dc00") + u("d83d") + u("0041") + u("D83D") + u("DE00") + `\/"}`},
		{"escaped base64", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"` + u("0041") + "Q" + u("003d") + `=","legIntPurposes":"A` + u("000a") + `g=="}}}`},
		{"escaped base64 surrogate", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":"AQ` + u("d800") + `"}}}`},
		{"escaped vendor keys collide", `{"vendorListVersion":7,"vendors":{"` + u("d800") + `":{"id":1},"` + u("dfff") + `":{"id":2},"` + u("fffd") + `":{"id":3,"purposes":[1]}}}`},
		{"escaped key surrogate pair", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purposes":[1],"` + u("d835") + u("dc29") + `d":2}}}`},
		{"escaped vendor key repeated", `{"vendorListVersion":7,"vendors":{"` + u("0039") + `":{"id":9,"purposes":[1]},"9":{"id":9,"purposes":[2]}}}`},
		{"escaped long s key", `{"vendorListVersion":7,"vendors":{"1":{"id":1,"purpo` + u("017F") + `es":[4]}}}`},
		{"escape invalid hex in key", `{"vendorListVersion":7,"` + u("00g0") + `":1}`},
		{"escape truncated", `{"vendorListVersion":7,"x":"` + u("00e") + `"}`},
	}
}

func manyVendors() string {
	var b strings.Builder
	b.WriteString(`{"gvlSpecificationVersion":3,"vendorListVersion":99,"vendors":{`)
	for id := 1; id <= 600; id++ {
		if id > 1 {
			b.WriteByte(',')
		}
		fmt.Fprintf(&b, `"%d":{"id":%d,"purposes":[%d],"legIntPurposes":[%d,%d],"flexiblePurposes":[%d],"specialFeatures":[%d]}`, id, id, id%12, id%7, id%11, id%5, id%3)
	}
	b.WriteString("}}")
	return b.String()
}
