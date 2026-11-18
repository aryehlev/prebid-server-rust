//! Go: `openrtb_ext/site_test.go`.

use pbs_openrtb_ext::openrtb_ext::ExtSite;

use super::support::{TestResult, ensure_eq, unmarshal_error};

const INVALID_AMP: &str = "request.site.ext.amp must be either 1, 0, or undefined";

/// Go: `TestInvalidSiteExt`.
#[test]
fn invalid_site_ext() -> TestResult {
    let mut s = ExtSite::default();
    for doc in [
        r#"{"amp":-1}"#,
        r#"{"amp":2}"#,
        r#"{"amp":true}"#,
        r#"{"amp":null}"#,
        r#"{"amp":"1"}"#,
    ] {
        ensure_eq(
            &unmarshal_error(doc, &mut s),
            &Some(INVALID_AMP.to_owned()),
            doc,
        )?;
    }
    Ok(())
}

/// Go: `TestValidSiteExt`.
#[test]
fn valid_site_ext() -> TestResult {
    let mut s = ExtSite::default();
    for (doc, amp) in [
        (r#"{"amp":0}"#, 0),
        (r#"{"amp":1}"#, 1),
        (r#"{"amp":      1   }"#, 1),
    ] {
        ensure_eq(&unmarshal_error(doc, &mut s), &None, doc)?;
        ensure_eq(&s.amp, &amp, doc)?;
    }
    Ok(())
}
