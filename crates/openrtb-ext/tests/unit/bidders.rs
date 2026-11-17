//! Go: `openrtb_ext/bidders_test.go` and `bidders_validate_test.go`, but the tests that register
//! aliases (package state), which `tests/bidder_names` runs on their own.

use std::collections::{BTreeMap, BTreeSet};

use pbs_bidder_params::gojsonschema::{Schema, decode_json_using_number};
use pbs_openrtb_ext::openrtb_ext::{
    BIDDER_ADKERNEL_ADN, BIDDER_SILVER_PUSH, BIDDER_TRIPLELIFT_NATIVE, BidderName,
    BidderParamValidator as _, StandardBidderParamValidator, core_bidder_names,
    is_bidder_name_reserved, new_bidder_params_validator,
};

use super::support::{TempDir, TestResult, all_ok, ensure_eq, repo_root, temp_path};

const TEST_SCHEMA: &str = r#"{
		"$schema": "http://json-schema.org/draft-04/schema#",
		"title": "Test Params",
		"description": "Test Description",
		"type": "object",
		"properties": {
		  "placementId": {
			"type": "integer",
			"description": "An ID which identifies this placement of the impression."
		  },
		  "optionalText": {
			"type": "string",
			"description": "Optional text for testing."
		  }
		},
		"required": ["placementId"]
	}"#;

/// Go: `TestBidderParamValidatorValidate`.
#[test]
fn bidder_param_validator_validate() -> TestResult {
    let schema = Schema::new(TEST_SCHEMA.as_bytes())?;
    let validator = StandardBidderParamValidator::from_schemas(
        BTreeMap::from([("foo".to_owned(), schema)]),
        BTreeMap::new(),
    );
    let cases = [
        ("Valid", r#"{"placementId":123}"#, ""),
        (
            "Invalid - Wrong Type",
            r#"{"placementId":"stringInsteadOfInt"}"#,
            "placementId: Invalid type. Expected: integer, given: string",
        ),
        (
            "Invalid - Empty Object",
            "{}",
            "(root): placementId is required",
        ),
        (
            "Malformed",
            "malformedJSON",
            "invalid character 'm' looking for beginning of value",
        ),
    ];
    all_ok(cases.into_iter().map(|(description, ext, expected_error)| {
        let got = validator
            .validate("foo", ext.as_bytes())
            .err()
            .map_or_else(String::new, |e| e.to_string());
        (
            description.to_owned(),
            ensure_eq(got.as_str(), expected_error, "error"),
        )
    }))
}

/// Go: `TestBidderParamValidatorSchema`.
#[test]
fn bidder_param_validator_schema() -> TestResult {
    let validator = StandardBidderParamValidator::from_schemas(
        BTreeMap::new(),
        BTreeMap::from([
            ("foo".to_owned(), "foo content".to_owned()),
            ("bar".to_owned(), "bar content".to_owned()),
        ]),
    );
    ensure_eq(validator.schema("bar"), "bar content", "Schema(bar)")
}

/// Go: `TestIsBidderNameReserved`.
#[test]
fn bidder_name_reserved() -> TestResult {
    let cases = [
        ("all", true),
        ("aLl", true),
        ("ALL", true),
        ("context", true),
        ("CONTEXT", true),
        ("conTExt", true),
        ("data", true),
        ("DATA", true),
        ("DaTa", true),
        ("general", true),
        ("gEnErAl", true),
        ("GENERAL", true),
        ("gpid", true),
        ("GPID", true),
        ("GPid", true),
        ("prebid", true),
        ("PREbid", true),
        ("PREBID", true),
        ("skadn", true),
        ("skADN", true),
        ("SKADN", true),
        ("tid", true),
        ("TId", true),
        ("Tid", true),
        ("TiD", true),
        ("notreserved", false),
    ];
    all_ok(cases.into_iter().map(|(bidder, expected)| {
        (
            bidder.to_owned(),
            ensure_eq(&is_bidder_name_reserved(bidder), &expected, "reserved"),
        )
    }))
}

/// Go: `TestNewBidderParamsValidator`, the error cases a real file system can produce (the
/// others need Go's mock file system; `pbs_bidder_params` tests the loader's errors). Go's mock
/// says `file does not exist` where the operating system says `no such file or directory`. The
/// valid case registers an alias, so `tests/bidder_names` runs it.
#[test]
fn new_bidder_params_validator_errors() -> TestResult {
    let missing = temp_path("missing");
    let not_a_bidder = TempDir::new("not-a-bidder", &[("anyBidder.json", "{}")])?;
    let cases = [
        (
            "failed to read directory",
            missing.clone(),
            format!(
                "Failed to read JSON schemas from directory {missing}. open {missing}: no such \
                 file or directory"
            ),
        ),
        (
            "file name does not match the bidder name",
            not_a_bidder.0.clone(),
            format!(
                "File {}/anyBidder.json does not match a valid BidderName.",
                not_a_bidder.0
            ),
        ),
    ];
    all_ok(cases.into_iter().map(|(description, dir, expected_err)| {
        let got = new_bidder_params_validator(&dir)
            .map(|_validator| ())
            .map_err(|e| e.to_string());
        (
            description.to_owned(),
            ensure_eq(&got, &Err(expected_err), "result"),
        )
    }))
}

/// Go: `TestMain` and `TestBidderParamSchemas` (`bidders_validate_test.go`): the validator of
/// `static/bidder-params` has a schema for every core bidder, and each is JSON (gojsonschema's
/// `LoadJSON`).
#[test]
fn bidder_param_schemas() -> TestResult {
    let dir = repo_root().join("static/bidder-params");
    let validator =
        new_bidder_params_validator(&dir.to_string_lossy()).map_err(|e| e.to_string())?;
    all_ok(core_bidder_names().into_iter().map(|bidder| {
        let schema = validator.schema(bidder.as_str());
        let result = if schema.is_empty() {
            Err(format!(
                "No schema exists for bidder {bidder}. Does static/bidder-params/{bidder}.json \
                 exist?"
            ))
        } else {
            decode_json_using_number(schema.as_bytes())
                .map(|_document| ())
                .map_err(|e| {
                    format!(
                        "static/bidder-params/{bidder}.json does not have a valid json-schema. {e}"
                    )
                })
        };
        (bidder.to_string(), result)
    }))
}

/// Go: `TestBidderNamesValid`.
#[test]
fn bidder_names_valid() -> TestResult {
    all_ok(core_bidder_names().into_iter().map(|bidder| {
        let result = ensure_eq(
            &is_bidder_name_reserved(bidder.as_str()),
            &false,
            "conflicts with a reserved name",
        );
        (bidder.to_string(), result)
    }))
}

/// Go: `uniqueForPrefixLength`: whether the first `prefix_length` bytes of each name tell them
/// apart.
fn unique_for_prefix_length(b: &[String], prefix_length: usize) -> bool {
    if prefix_length == 0 {
        return false;
    }
    let mut m = BTreeSet::new();
    for (i, n) in b.iter().enumerate() {
        let ns = n.as_bytes().get(..prefix_length).unwrap_or(n.as_bytes());
        let _new = m.insert(ns);
        if m.len() != i + 1 {
            return false;
        }
    }
    true
}

/// Go: `minUniquePrefixLength`: the fewest leading bytes that tell the names apart, or 0 when
/// some are equal.
fn min_unique_prefix_length(b: &[String]) -> usize {
    let targeting_key_max_length = 20;
    (1..=targeting_key_max_length)
        .find(|&prefix_length| unique_for_prefix_length(b, prefix_length))
        .unwrap_or(0)
}

/// Go: `TestBidderUniquenessGatekeeping`: the core bidders but the duplicate adapters of one
/// bidder are told apart by their first 6 characters.
#[test]
fn bidder_uniqueness_gatekeeping() -> TestResult {
    let excluded = [
        BIDDER_SILVER_PUSH,
        BIDDER_TRIPLELIFT_NATIVE,
        BIDDER_ADKERNEL_ADN,
        BidderName::from_static("freewheel-ssp"),
        BidderName::from_static("yahooAdvertising"),
        BidderName::from_static("boldwin_rapid"),
    ];
    let bidders: Vec<String> = core_bidder_names()
        .into_iter()
        .filter(|bidder| !excluded.contains(bidder))
        .map(|bidder| bidder.as_str().to_owned())
        .collect();
    let current_threshold = 6;
    let measured_threshold = min_unique_prefix_length(&bidders);
    if measured_threshold == 0 {
        return Err("BidderMap contains duplicate bidder name values.".to_owned());
    }
    if measured_threshold > current_threshold {
        return Err(format!(
            "measured threshold {measured_threshold} is over {current_threshold}"
        ));
    }
    Ok(())
}
