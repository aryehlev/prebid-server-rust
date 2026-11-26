//! Go: `openrtb_ext/site_test.go`.

use pbs_openrtb_ext::openrtb_ext::ExtSite;

use super::support::{TestResult, ensure_eq, serde_unmarshal, unmarshal_error};

const INVALID_AMP: &str = "request.site.ext.amp must be either 1, 0, or undefined";

/// The documents of `TestInvalidSiteExt`.
const INVALID_SITE_EXTS: [&str; 5] = [
    r#"{"amp":-1}"#,
    r#"{"amp":2}"#,
    r#"{"amp":true}"#,
    r#"{"amp":null}"#,
    r#"{"amp":"1"}"#,
];

/// The documents of `TestValidSiteExt`, with the `AMP` each gives.
const VALID_SITE_EXTS: [(&str, i8); 3] = [
    (r#"{"amp":0}"#, 0),
    (r#"{"amp":1}"#, 1),
    (r#"{"amp":      1   }"#, 1),
];

/// Go: `TestInvalidSiteExt`.
#[test]
fn invalid_site_ext() -> TestResult {
    let mut s = ExtSite::default();
    for doc in INVALID_SITE_EXTS {
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
    for (doc, amp) in VALID_SITE_EXTS {
        ensure_eq(&unmarshal_error(doc, &mut s), &None, doc)?;
        ensure_eq(&s.amp, &amp, doc)?;
    }
    Ok(())
}

/// Go: `TestInvalidSiteExt`, through the serde code: the document is the `ExtSite`, so the text
/// is Go's exactly.
#[test]
fn invalid_site_ext_serde() -> TestResult {
    for doc in INVALID_SITE_EXTS {
        ensure_eq(
            &serde_unmarshal::<ExtSite>(doc).err(),
            &Some(INVALID_AMP.to_owned()),
            doc,
        )?;
    }
    Ok(())
}

/// Go: `TestValidSiteExt`, through the serde code.
#[test]
fn valid_site_ext_serde() -> TestResult {
    for (doc, amp) in VALID_SITE_EXTS {
        ensure_eq(&serde_unmarshal::<ExtSite>(doc)?.amp, &amp, doc)?;
    }
    Ok(())
}
