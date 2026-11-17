//! The openrtb module's own tests: the `Marshaling` tables of its `*_test.go` files.
//!
//! Each testdata file decodes with `encoding/json` into its type, encodes back with
//! `json.Marshal`, and the encoding is the same JSON as the file (gomega's `MatchJSON`: both
//! documents decoded into `interface{}` and compared deeply). The files are read from
//! `module.tsv`, where the generator copied them from the module.

use pbs_gojson::stdjson;
use serde_json::Value;

use super::{
    golden::{Runners, runners},
    support::{TestResult, golden_rows, report, show},
};

/// Each Go test entry: its description, its testdata file, and the type it decodes into.
const TESTS: [(&str, &str, &str); 34] = [
    // adcom1/adcom1_test.go
    (
        "request context",
        "adcom1/testdata/request-context.json",
        "adcom1.RequestContext",
    ),
    (
        "item specifications",
        "adcom1/testdata/item-specifications.json",
        "adcom1.ItemSpec",
    ),
    (
        "media response",
        "adcom1/testdata/media-response.json",
        "adcom1.BidMedia",
    ),
    // native1/request/request_test.go
    (
        "Social Context (v1.1)",
        "native1/request/testdata/v1.1/social-context.json",
        "request.Request",
    ),
    (
        "Content Context (v1.1)",
        "native1/request/testdata/v1.1/content-context.json",
        "request.Request",
    ),
    (
        "Social Context (v1.2)",
        "native1/request/testdata/v1.2/social-context.json",
        "request.Request",
    ),
    (
        "Content Context (v1.2)",
        "native1/request/testdata/v1.2/content-context.json",
        "request.Request",
    ),
    (
        "Third-party (v1.2)",
        "native1/request/testdata/v1.2/third-party.json",
        "request.Request",
    ),
    // native1/response/response_test.go
    (
        "Clickout (v1.1)",
        "native1/response/testdata/v1.1/clickout.json",
        "response.Response",
    ),
    (
        "Video (v1.1)",
        "native1/response/testdata/v1.1/video.json",
        "response.Response",
    ),
    (
        "Clickout (v1.2)",
        "native1/response/testdata/v1.2/clickout.json",
        "response.Response",
    ),
    (
        "Video (v1.2)",
        "native1/response/testdata/v1.2/video.json",
        "response.Response",
    ),
    (
        "Third-party (v1.2)",
        "native1/response/testdata/v1.2/third-party.json",
        "response.Response",
    ),
    // openrtb2/bid_request_test.go
    (
        "2.5 Simple Banner",
        "openrtb2/testdata/bid-request/2.5/simple-banner.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.5 Expandable Creative",
        "openrtb2/testdata/bid-request/2.5/expandable-creative.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.5 Mobile",
        "openrtb2/testdata/bid-request/2.5/mobile.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.5 Video",
        "openrtb2/testdata/bid-request/2.5/video.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.5 PMP with Direct Deal",
        "openrtb2/testdata/bid-request/2.5/pmp-with-direct-deal.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.5 Native Ad",
        "openrtb2/testdata/bid-request/2.5/native-ad.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.6 Simple Banner",
        "openrtb2/testdata/bid-request/2.6/simple-banner.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.6 Expandable Creative",
        "openrtb2/testdata/bid-request/2.6/expandable-creative.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.6 Mobile",
        "openrtb2/testdata/bid-request/2.6/mobile.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.6 Video",
        "openrtb2/testdata/bid-request/2.6/video.json",
        "openrtb2.BidRequest",
    ),
    (
        "2.6 PMP with Direct Deal",
        "openrtb2/testdata/bid-request/2.6/pmp-with-direct-deal.json",
        "openrtb2.BidRequest",
    ),
    // openrtb2/bid_response_test.go
    (
        "2.5 Ad Served on Win Notice",
        "openrtb2/testdata/bid-response/2.5/ad-served-on-win-notice.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.5 VAST XML Document Returned Inline",
        "openrtb2/testdata/bid-response/2.5/vast-xml-document-returned-inline.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.5 Direct Deal Ad Served on Win Notice",
        "openrtb2/testdata/bid-response/2.5/direct-deal-ad-served-on-win-notice.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.5 Native Markup Returned Inline",
        "openrtb2/testdata/bid-response/2.5/native-markup-returned-inline.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.6 Ad Served on Win Notice",
        "openrtb2/testdata/bid-response/2.6/ad-served-on-win-notice.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.6 VAST XML Document Returned Inline",
        "openrtb2/testdata/bid-response/2.6/vast-xml-document-returned-inline.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.6 Direct Deal Ad Served on Win Notice",
        "openrtb2/testdata/bid-response/2.6/direct-deal-ad-served-on-win-notice.json",
        "openrtb2.BidResponse",
    ),
    (
        "2.6 Native Markup Returned Inline",
        "openrtb2/testdata/bid-response/2.6/native-markup-returned-inline.json",
        "openrtb2.BidResponse",
    ),
    // openrtb3/openrtb3_test.go
    ("request", "openrtb3/testdata/request.json", "openrtb3.Body"),
    (
        "response",
        "openrtb3/testdata/response.json",
        "openrtb3.Body",
    ),
];

/// The document decoded as Go decodes it into `interface{}`.
fn interface(doc: &[u8]) -> TestResult<Value> {
    let mut value = Value::Null;
    stdjson::unmarshal(doc, &mut value)
        .map_err(|e| String::from_utf8_lossy(&e.go_text()).into_owned())?;
    Ok(value)
}

/// Runs one entry; `Ok(Some(failure))` when the port fails it.
fn entry(
    rows: &[Vec<Vec<u8>>],
    runners: &Runners,
    (name, file, go_type): (&str, &str, &str),
) -> TestResult<Option<String>> {
    let row = rows
        .iter()
        .find(|row| row.get(2).is_some_and(|label| label == file.as_bytes()))
        .ok_or_else(|| format!("{file} is not in module.tsv"))?;
    let [doc_ref, row_type, ..] = row.as_slice() else {
        return Err(format!("bad module.tsv row for {file}"));
    };
    if row_type.as_slice() != go_type.as_bytes() {
        return Err(format!(
            "{file} is decoded as {} in module.tsv",
            show(row_type)
        ));
    }
    let golden = doc_ref
        .strip_prefix(b"'")
        .ok_or_else(|| format!("{file}: not inline in module.tsv"))?;
    let run = runners
        .get(go_type)
        .ok_or_else(|| format!("no Rust type for Go type {go_type}"))?;
    let got = run(golden);
    if !got.std_err.is_empty() {
        return Ok(Some(format!(
            "{name}: json.Unmarshal: {}",
            show(&got.std_err)
        )));
    }
    let [_, _, _, _, marshaled, _] = &got.encoded;
    let actual = match marshaled {
        Ok(bytes) => bytes,
        Err(e) => return Ok(Some(format!("{name}: json.Marshal: {}", show(e)))),
    };
    if interface(actual)? == interface(golden)? {
        Ok(None)
    } else {
        Ok(Some(format!(
            "{name}: {} does not match {file}",
            show(actual)
        )))
    }
}

/// `DescribeTable("Marshaling", ...)` of each package.
#[test]
fn marshaling() -> TestResult {
    let runners = runners()?;
    let rows = golden_rows("module.tsv")?;
    let mut failures = Vec::new();
    for test in TESTS {
        if let Some(failure) = entry(&rows, &runners, test)? {
            failures.push(failure);
        }
    }
    report("marshaling", &failures, 34)
}
