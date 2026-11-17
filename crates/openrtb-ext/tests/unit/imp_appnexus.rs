//! Go: `openrtb_ext/imp_appnexus_test.go`.

use pbs_gojson::GoJson;
use pbs_openrtb_ext::openrtb_ext::ExtImpAppnexusKeywords;

use super::support::{TestResult, all_ok, ensure_eq, unmarshal_error};

/// The test's `keywords` struct.
#[derive(Debug, Default, GoJson)]
#[gojson(go_type = "openrtb_ext.keywords")]
struct Keywords {
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
