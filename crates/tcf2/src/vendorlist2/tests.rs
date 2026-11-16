//! Go: `vendorlist2/eager-parsing_test.go` and `vendorlist2/shared_test.go`.

use super::parse_eagerly;
use crate::{
    api::{Vendor, VendorList},
    consentconstants::{Purpose, SpecialFeature},
    test_support::{TestError, TestResult},
};

/// Go: `testDataSpecVersion2`.
const TEST_DATA_SPEC_VERSION_2: &str = r#"
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
"#;

/// Go: `testDataSpecVersion2Empty`.
const TEST_DATA_SPEC_VERSION_2_EMPTY: &str = r#"
{
	"gvlSpecificationVersion": 2,
	"vendorListVersion": 28,
	"tcfPolicyVersion": 2,
	"lastUpdated": "2020-03-05T16:05:29Z",
	"vendors": { }
}
"#;

/// Go: `testDataSpecVersion3`.
const TEST_DATA_SPEC_VERSION_3: &str = r#"
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
"#;

/// Go: `testDataSpecVersion3Empty`.
const TEST_DATA_SPEC_VERSION_3_EMPTY: &str = r#"
{
	"gvlSpecificationVersion": 3,
	"vendorListVersion": 1,
	"tcfPolicyVersion": 4,
	"lastUpdated": "2023-05-18T16:07:14Z",
	"vendors": { }
}
"#;

fn vendor(list: &dyn VendorList, id: u16) -> Result<&dyn Vendor, TestError> {
    list.vendor(id)
        .ok_or_else(|| format!("vendor {id} missing").into())
}

/// The expectations for one vendor: `(purpose, purpose strict)` for purposes 1-6,
/// `(legitimate interest, strict)` for purposes 1-3, special purposes 1-3 and special features
/// 1-3.
type Expected = ([(bool, bool); 6], [(bool, bool); 3], [bool; 3], [bool; 3]);

fn assert_vendor(vendor: &dyn Vendor, expected: &Expected, name: &str) -> TestResult {
    let (purposes, legitimate_interests, special_purposes, special_features) = expected;
    for (id, &(purpose, strict)) in (1..).zip(purposes) {
        check_eq!(vendor.purpose(Purpose(id)), purpose, "{name} purpose {id}");
        check_eq!(
            vendor.purpose_strict(Purpose(id)),
            strict,
            "{name} strict {id}"
        );
    }
    for (id, &(li, strict)) in (1..).zip(legitimate_interests) {
        check_eq!(
            vendor.legitimate_interest(Purpose(id)),
            li,
            "{name} LI {id}"
        );
        check_eq!(
            vendor.legitimate_interest_strict(Purpose(id)),
            strict,
            "{name} LI strict {id}"
        );
    }
    for (id, &special) in (1..).zip(special_purposes) {
        check_eq!(
            vendor.special_purpose(Purpose(id)),
            special,
            "{name} SP {id}"
        );
    }
    for (id, &feature) in (1..).zip(special_features) {
        check_eq!(
            vendor.special_feature(SpecialFeature(id)),
            feature,
            "{name} SF {id}"
        );
    }
    Ok(())
}

/// Go: `AssertVendorListCorrectness`.
fn assert_vendor_list_correctness(list: &dyn VendorList) -> TestResult {
    let (t, f) = (true, false);
    assert_vendor(
        vendor(list, 8)?,
        &(
            [(t, t), (t, f), (t, t), (t, t), (f, f), (f, f)],
            [(f, f), (t, t), (f, f)],
            [t, t, f],
            [t, t, f],
        ),
        "vendor 8",
    )?;
    assert_vendor(
        vendor(list, 80)?,
        &(
            [(t, t), (t, t), (f, f), (t, t), (f, f), (f, f)],
            [(f, f), (t, f), (f, f)],
            [f, f, f],
            [f, f, f],
        ),
        "vendor 80",
    )
}

/// Go: `TestParseEagerlyVendorList`.
#[test]
fn parse_eagerly_vendor_list() -> TestResult {
    for (name, data, spec_version, version) in [
        ("vendor_list_spec_2", TEST_DATA_SPEC_VERSION_2, 2, 28),
        ("vendor_list_spec_3", TEST_DATA_SPEC_VERSION_3, 3, 1),
    ] {
        let list = parse_eagerly(data.as_bytes())?;
        check_eq!(list.spec_version(), spec_version, "{name} spec version");
        check_eq!(list.version(), version, "{name} version");
        check!(list.vendor(8).is_some(), "{name} vendor 8");
        check!(list.vendor(80).is_some(), "{name} vendor 80");
        assert_vendor_list_correctness(&list)?;
    }
    Ok(())
}

/// Go: `TestParseEagerlyEmptyVendorList`.
#[test]
fn parse_eagerly_empty_vendor_list() -> TestResult {
    for (name, data, spec_version, version) in [
        ("vendor_list_spec_2", TEST_DATA_SPEC_VERSION_2_EMPTY, 2, 28),
        ("vendor_list_spec_3", TEST_DATA_SPEC_VERSION_3_EMPTY, 3, 1),
    ] {
        let list = parse_eagerly(data.as_bytes())?;
        check_eq!(list.spec_version(), spec_version, "{name} spec version");
        check_eq!(list.version(), version, "{name} version");
        check!(list.vendor(8).is_none(), "{name} vendor 8");
        check!(list.vendor(80).is_none(), "{name} vendor 80");
    }
    Ok(())
}

/// The error texts prebid-server logs for bad vendor lists (`gdpr/vendorlist-fetching.go`).
#[test]
fn parse_eagerly_errors() -> TestResult {
    for (data, message) in [
        ("", "unexpected end of JSON input"),
        ("{", "unexpected end of JSON input"),
        (
            r#"{"vendorListVersion":1}x"#,
            "invalid character 'x' after top-level value",
        ),
        (
            "{\"vendorListVersion\":1 \x01}",
            r"invalid character '\x01' after object key:value pair",
        ),
        (
            "{}",
            "data.vendorListVersion was 0 or undefined. Versions should start at 1",
        ),
        (
            r#"{"vendorListVersion":"1"}"#,
            "json: cannot unmarshal string into Go struct field vendorListContract.\
             vendorListVersion of type uint16",
        ),
        (
            r#"{"vendorListVersion":1,"vendors":{"1":{"purposes":[256]}}}"#,
            "json: cannot unmarshal number 256 into Go struct field vendorListVendorContract.\
             vendors.purposes of type uint8",
        ),
        (
            r#"{"vendorListVersion":1,"vendors":{"1":{"purposes":"AQI"}}}"#,
            "illegal base64 data at input byte 0",
        ),
        (
            "[]",
            "json: cannot unmarshal array into Go value of type vendorlist2.vendorListContract",
        ),
    ] {
        match parse_eagerly(data.as_bytes()) {
            Ok(_) => return Err(format!("{data:?} parsed").into()),
            Err(err) => check_eq!(err.to_string(), message, "{data:?}"),
        }
    }
    Ok(())
}
