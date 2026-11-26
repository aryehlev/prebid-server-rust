//! Go: `openrtb_ext/imp_appnexus_test.go`.

use pbs_gojson::GoJson;
use pbs_jsonutil::{null_default, unmarshal_valid};
use pbs_openrtb_ext::openrtb_ext::ExtImpAppnexusKeywords;
use serde::Deserialize;

use super::support::{TestResult, all_ok, ensure_eq, unmarshal_error};

/// The test's `keywords` struct, with the serde code typegen writes for it.
#[derive(Debug, Default, GoJson, Deserialize)]
#[serde(default)]
#[gojson(go_type = "openrtb_ext.keywords")]
struct Keywords {
    #[serde(deserialize_with = "null_default")]
    #[gojson(go = "Keywords", json = "keywords")]
    keywords: ExtImpAppnexusKeywords,
}

/// Go: `TestKeywordsUnmarshalJSON`.
#[test]
fn keywords_unmarshal_json() -> TestResult {
    let valid = [
        (
            r#"{"keywords" : { "pets": ["dog"] }}"#,
            "pets=dog",
            "dynamic json object",
        ),
        (
            r#"{"keywords" : { "foo":[] }}"#,
            "foo",
            "dynamic json object with empty value array",
        ),
        (
            r#"{"keywords" : [{"key": "foo", "value": ["bar","baz"]},{"key": "valueless"}]}"#,
            "foo=bar,foo=baz,valueless",
            "array of objects",
        ),
        (
            r#"{"keywords" : "foo=bar,foo=baz,valueless"}"#,
            "foo=bar,foo=baz,valueless",
            "string keywords",
        ),
        (r#"{"keywords" : ""}"#, "", "empty string"),
        (r#"{"keywords" : {}}"#, "", "empty keywords object"),
        (r#"{"keywords" : [{}]}"#, "", "empty keywords object array"),
        (r#"{"keywords": []}"#, "", "empty keywords array"),
    ];
    all_ok(valid.into_iter().map(|(input, expected, desc)| {
        let mut keywords = Keywords::default();
        let result = ensure_eq(&unmarshal_error(input, &mut keywords), &None, "error")
            .and_then(|()| ensure_eq(keywords.keywords.as_str(), expected, "keywords"));
        (desc.to_owned(), result)
    }))?;
    let invalid = [
        (r#"{"keywords": [{]}"#, "invalid keywords array"),
        (r#"{"keywords" : {"}}"#, "invalid keywords object"),
    ];
    all_ok(invalid.into_iter().map(|(input, desc)| {
        let mut keywords = Keywords::default();
        let result = if unmarshal_error(input, &mut keywords).is_some() {
            Ok(())
        } else {
            Err("no error".to_owned())
        };
        (desc.to_owned(), result)
    }))
}

/// Go: `TestKeywordsUnmarshalJSON`, through the serde impl (`jsonutil.UnmarshalValid` is
/// `pbs_jsonutil::unmarshal_valid`).
#[test]
fn keywords_unmarshal_json_serde() -> TestResult {
    let valid = [
        (
            r#"{"keywords" : { "pets": ["dog"] }}"#,
            "pets=dog",
            "dynamic json object",
        ),
        (
            r#"{"keywords" : { "foo":[] }}"#,
            "foo",
            "dynamic json object with empty value array",
        ),
        (
            r#"{"keywords" : [{"key": "foo", "value": ["bar","baz"]},{"key": "valueless"}]}"#,
            "foo=bar,foo=baz,valueless",
            "array of objects",
        ),
        (
            r#"{"keywords" : "foo=bar,foo=baz,valueless"}"#,
            "foo=bar,foo=baz,valueless",
            "string keywords",
        ),
        (r#"{"keywords" : ""}"#, "", "empty string"),
        (r#"{"keywords" : {}}"#, "", "empty keywords object"),
        (r#"{"keywords" : [{}]}"#, "", "empty keywords object array"),
        (r#"{"keywords": []}"#, "", "empty keywords array"),
    ];
    all_ok(valid.into_iter().map(|(input, expected, desc)| {
        let result = unmarshal_valid::<Keywords>(input.as_bytes())
            .map_err(|e| e.to_string())
            .and_then(|keywords| ensure_eq(keywords.keywords.as_str(), expected, "keywords"));
        (desc.to_owned(), result)
    }))?;
    let invalid = [
        (r#"{"keywords": [{]}"#, "invalid keywords array"),
        (r#"{"keywords" : {"}}"#, "invalid keywords object"),
    ];
    all_ok(invalid.into_iter().map(|(input, desc)| {
        let result = match unmarshal_valid::<Keywords>(input.as_bytes()) {
            Ok(_) => Err("no error".to_owned()),
            Err(_) => Ok(()),
        };
        (desc.to_owned(), result)
    }))
}
