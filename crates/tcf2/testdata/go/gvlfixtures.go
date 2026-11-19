package main

import "encoding/json"

// The GVL fixtures of tests/vendorlist.rs (gvl_fixture_goldens.json): every vendor list the
// go-gdpr and prebid-server tests parse with vendorlist2.ParseEagerly, each recorded by
// newVendorListCase. The IAB-shaped lists (realistic*, many vendors) are in
// vendorlist_goldens.json already, and tests/vendorlist.rs replays them by name; the decoding
// quirks of the rest of that file and of vendorlist_fuzz_goldens.json are replayed separately.

// pbsVendorList and pbsVendor are the vendorList and vendor types of prebid-server's
// gdpr/vendorlist-fetching_test.go:304-316. MarshalVendorList writes them with
// jsonutil.Marshal, whose output for these types (integers, nil and non-nil slices and maps, and
// sorted map keys) is encoding/json's.
type pbsVendorList struct {
	GVLSpecificationVersion uint16                `json:"gvlSpecificationVersion"`
	VendorListVersion       uint16                `json:"vendorListVersion"`
	Vendors                 map[string]*pbsVendor `json:"vendors"`
}

type pbsVendor struct {
	ID               uint16 `json:"id"`
	Purposes         []int  `json:"purposes"`
	LegIntPurposes   []int  `json:"legIntPurposes"`
	FlexiblePurposes []int  `json:"flexiblePurposes"`
	SpecialFeatures  []int  `json:"specialFeatures"`
}

func marshalPBSVendorList(l pbsVendorList) string {
	b, err := json.Marshal(l)
	if err != nil {
		fail(err)
	}
	return string(b)
}

// pbsBuildVendorList34 is gdpr/impl_test.go:447-484.
func pbsBuildVendorList34() pbsVendorList {
	return pbsVendorList{
		VendorListVersion: 2,
		Vendors: map[string]*pbsVendor{
			"2":  {ID: 2, Purposes: []int{1}},
			"6":  {ID: 6, Purposes: []int{1, 2, 4}, LegIntPurposes: []int{7}, SpecialFeatures: []int{1}, FlexiblePurposes: []int{1, 2, 4, 7}},
			"8":  {ID: 8, Purposes: []int{1, 7}, LegIntPurposes: []int{2, 4}},
			"10": {ID: 10, Purposes: []int{2, 4, 7}, SpecialFeatures: []int{1}},
			"20": {ID: 20, Purposes: []int{1}, LegIntPurposes: []int{2, 7}, FlexiblePurposes: []int{2, 7}},
			"32": {ID: 32, Purposes: []int{1, 2, 4, 7}},
		},
	}
}

// pbsFixtures are prebid-server's vendor lists, named after the test that builds them.
func pbsFixtures() [][2]string {
	var out [][2]string
	add := func(name string, l pbsVendorList) {
		out = append(out, [2]string{"prebid-server " + name, marshalPBSVendorList(l)})
	}

	// gdpr/vendorlist-fetching_test.go
	add("vendorList1", pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: 1, Vendors: map[string]*pbsVendor{"12": {ID: 12, Purposes: []int{2}}}})
	add("vendorList2", pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: 2, Vendors: map[string]*pbsVendor{"12": {ID: 12, Purposes: []int{2, 3}}}})
	for v := uint16(1); v <= 3; v++ {
		purposes := []int{1, 2, 3}[:v]
		add("TestFetcherThrottling list "+string(rune('0'+v)), pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: v, Vendors: map[string]*pbsVendor{"12": {ID: 12, Purposes: purposes}}})
	}
	for spec := uint16(1); spec <= 4; spec++ {
		for v := uint16(1); v <= 3; v++ {
			add("TestPreloadCache spec "+string(rune('0'+spec))+" list "+string(rune('0'+v)), pbsVendorList{GVLSpecificationVersion: spec, VendorListVersion: v})
		}
	}
	out = append(out, [2]string{"prebid-server TestMalformedVendorlist", "malformed"})

	// gdpr/impl_test.go
	add("impl_test purpose 1", pbsVendorList{VendorListVersion: 2, Vendors: map[string]*pbsVendor{"2": {ID: 2, Purposes: []int{1}}}})
	add("impl_test purpose 2", pbsVendorList{VendorListVersion: 1, Vendors: map[string]*pbsVendor{"2": {ID: 2, Purposes: []int{2}}}})
	add("impl_test purposes 1 2", pbsVendorList{VendorListVersion: 2, Vendors: map[string]*pbsVendor{"2": {ID: 2, Purposes: []int{1, 2}}}})
	add("impl_test version 153", pbsVendorList{GVLSpecificationVersion: 2, VendorListVersion: 153, Vendors: map[string]*pbsVendor{}})
	add("impl_test buildVendorList34", pbsBuildVendorList34())
	list34 := pbsBuildVendorList34()
	list34.Vendors["8"].Purposes = []int{7}
	add("impl_test buildVendorList34 vendor 8 purpose 7", list34)

	// gdpr/full_enforcement_test.go:987-999
	add("full_enforcement_test makeVendorList", pbsVendorList{VendorListVersion: 2, Vendors: map[string]*pbsVendor{"32": {ID: 32, Purposes: []int{1}, LegIntPurposes: []int{2}, FlexiblePurposes: []int{3}}}})

	// gdpr/impl_test.go TestVendorListSelection, whose spec 2 list has an empty, not nil, slice.
	add("impl_test TestVendorListSelection spec 2", pbsVendorList{GVLSpecificationVersion: 2, VendorListVersion: 2, Vendors: map[string]*pbsVendor{"2": {ID: 2, Purposes: []int{}}}})
	add("impl_test TestVendorListSelection spec 3", pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: 2, Vendors: map[string]*pbsVendor{"2": {ID: 2, Purposes: []int{1}}}})

	// gdpr/gvl_vendor_ids_test.go
	add("gvl_vendor_ids_test three vendors", pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: 1, Vendors: map[string]*pbsVendor{"10": {ID: 10}, "20": {ID: 20}, "100": {ID: 100}}})
	add("gvl_vendor_ids_test no vendors", pbsVendorList{GVLSpecificationVersion: 3, VendorListVersion: 1, Vendors: map[string]*pbsVendor{}})
	add("gvl_vendor_ids_test spec 2", pbsVendorList{GVLSpecificationVersion: 2, VendorListVersion: 1, Vendors: map[string]*pbsVendor{"5": {ID: 5}}})
	return out
}

func buildGVLFixtureCases() []vendorListCase {
	var out []vendorListCase
	add := func(name, doc string) { out = append(out, newVendorListCase(name, []byte(doc))) }

	// go-gdpr's vendorlist2 tests (shared_test.go).
	add("go-gdpr testDataSpecVersion2", testDataSpecVersion2)
	add("go-gdpr testDataSpecVersion2Empty", testDataSpecVersion2Empty)
	add("go-gdpr testDataSpecVersion3", testDataSpecVersion3)
	add("go-gdpr testDataSpecVersion3Empty", testDataSpecVersion3Empty)

	for _, f := range pbsFixtures() {
		add(f[0], f[1])
	}
	return out
}
