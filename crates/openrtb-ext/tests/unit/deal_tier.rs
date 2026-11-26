//! Go: `openrtb_ext/deal_tier_test.go`.

use std::collections::BTreeMap;

use pbs_errortypes::ErrorKind;
use pbs_gojson::RawJson;
use pbs_openrtb::openrtb2;
use pbs_openrtb_ext::openrtb_ext::{
    BIDDER_APPNEXUS, BIDDER_RUBICON, BidderName, DealTier, DealTierBidderMap,
    read_deal_tiers_from_imp,
};

use super::support::{TestResult, all_ok, ensure_eq};

fn tier(prefix: &str, min_deal_tier: isize) -> DealTier {
    DealTier {
        prefix: prefix.to_owned(),
        min_deal_tier,
    }
}

fn tiers<const N: usize>(entries: [(BidderName, DealTier); N]) -> DealTierBidderMap {
    DealTierBidderMap(Some(BTreeMap::from(entries)))
}

/// Go's `DealTierBidderMap{}`.
fn none() -> DealTierBidderMap {
    tiers([])
}

fn imp(ext: Option<&str>) -> openrtb2::Imp {
    openrtb2::Imp {
        ext: ext.map_or_else(RawJson::nil, |ext| RawJson::from_slice(ext.as_bytes())),
        ..openrtb2::Imp::default()
    }
}

/// A case of `TestReadDealTiersFromImp`: its description, `imp.ext` (`None` for nil) and the
/// result wanted. Go returns a nil map with its error; here the error is the whole result.
type DealTierCase = (
    &'static str,
    Option<&'static str>,
    Result<DealTierBidderMap, ErrorKind>,
);

/// The cases of `TestReadDealTiersFromImp` without `imp.ext.prebid.bidder`.
fn cases_without_bidders() -> Vec<DealTierCase> {
    vec![
        ("nil", None, Ok(none())),
        ("none", Some(""), Ok(none())),
        ("empty_object", Some("{}"), Ok(none())),
        (
            "imp.ext_no_prebid_but_with_other_params",
            Some(concat!(
                r#"{"appnexus": {"dealTier": {"minDealTier": 5, "prefix": "anyPrefix"}, "#,
                r#""placementId": 12345}, "tid": "1234"}"#,
            )),
            Ok(none()),
        ),
        (
            "imp.ext.prebid_nil",
            Some(r#"{"prebid": null}"#),
            Ok(none()),
        ),
        (
            "imp.ext.prebid_empty",
            Some(r#"{"prebid": {}}"#),
            Ok(none()),
        ),
        (
            "imp.ext.prebid_no bidder but with other params",
            Some(r#"{"prebid": {"supportdeals": true}}"#),
            Ok(none()),
        ),
    ]
}

/// The cases of `TestReadDealTiersFromImp` with `imp.ext.prebid.bidder`.
fn cases_with_bidders() -> Vec<DealTierCase> {
    vec![
        (
            "imp.ext.prebid.bidder_one",
            Some(concat!(
                r#"{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "anyPrefix"}, "placementId": 12345}}}}"#,
            )),
            Ok(tiers([(BIDDER_APPNEXUS, tier("anyPrefix", 5))])),
        ),
        (
            "imp.ext.prebid.bidder_one_but_not_found_in_the_adapter_bidder_list",
            Some(concat!(
                r#"{"prebid": {"bidder": {"unknown": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "anyPrefix"}, "placementId": 12345}}}}"#,
            )),
            Ok(tiers([(BidderName::from("unknown"), tier("anyPrefix", 5))])),
        ),
        (
            "imp.ext.prebid.bidder_one_but_not_found_in_the_adapter_bidder_list_with_\
                case_insensitive",
            Some(concat!(
                r#"{"prebid": {"bidder": {"UnKnOwn": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "anyPrefix"}, "placementId": 12345}}}}"#,
            )),
            Ok(tiers([(BidderName::from("UnKnOwn"), tier("anyPrefix", 5))])),
        ),
        (
            "imp.ext.prebid.bidder_one_but_case_is_different_from_the_adapter_bidder_list",
            Some(concat!(
                r#"{"prebid": {"bidder": {"APpNExUS": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "anyPrefix"}, "placementId": 12345}}}}"#,
            )),
            Ok(tiers([(BIDDER_APPNEXUS, tier("anyPrefix", 5))])),
        ),
        (
            "imp.ext.prebid.bidder_one_with_other_params",
            Some(concat!(
                r#"{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "anyPrefix"}, "placementId": 12345}}, "supportdeals": true}, "#,
                r#""tid": "1234"}"#,
            )),
            Ok(tiers([(BIDDER_APPNEXUS, tier("anyPrefix", 5))])),
        ),
        (
            "imp.ext.prebid.bidder_multiple",
            Some(concat!(
                r#"{"prebid": {"bidder": {"appnexus": {"dealTier": {"minDealTier": 5, "#,
                r#""prefix": "appnexusPrefix"}, "placementId": 12345}, "rubicon": "#,
                r#"{"dealTier": {"minDealTier": 8, "prefix": "rubiconPrefix"}, "#,
                r#""placementId": 12345}}}}"#,
            )),
            Ok(tiers([
                (BIDDER_APPNEXUS, tier("appnexusPrefix", 5)),
                (BIDDER_RUBICON, tier("rubiconPrefix", 8)),
            ])),
        ),
        (
            "imp.ext.prebid.bidder_one_without_deal_tier",
            Some(r#"{"prebid": {"bidder": {"appnexus": {"placementId": 12345}}}}"#),
            Ok(none()),
        ),
        (
            "imp.ext.prebid.bidder_error",
            Some(concat!(
                r#"{"prebid": {"bidder": {"appnexus": {"dealTier": "wrong type", "#,
                r#""placementId": 12345}}}}"#,
            )),
            Err(ErrorKind::FailedToUnmarshal),
        ),
    ]
}

/// Go: `TestReadDealTiersFromImp`.
#[test]
fn read_deal_tiers() -> TestResult {
    let cases = cases_without_bidders()
        .into_iter()
        .chain(cases_with_bidders());
    all_ok(cases.map(|(description, ext, expected)| {
        let got = read_deal_tiers_from_imp(&imp(ext)).map_err(|e| e.kind().clone());
        (description.to_owned(), ensure_eq(&got, &expected, "result"))
    }))?;

    // imp.ext.prebid.bidder_dedupe: two spellings of one bidder give one entry.
    let ext = concat!(
        r#"{"prebid": {"bidder": {"APPNEXUS": {"dealTier": {"minDealTier": 100}},"#,
        r#""APpNExUS": {"dealTier": {"minDealTier": 5}}}}}"#,
    );
    let result = read_deal_tiers_from_imp(&imp(Some(ext))).map_err(|e| e.to_string())?;
    let entries = result.0.unwrap_or_default();
    ensure_eq(&entries.len(), &1, "bidder_dedupe entries")?;
    ensure_eq(
        &entries.contains_key(&BidderName::from("appnexus")),
        &true,
        "bidder_dedupe appnexus",
    )
}

/// The `imp.ext` documents `serde_json` reads otherwise than jsoniter, with go1.26.2's results
/// (run by hand at the pinned upstream): `null` gives Go an empty map, and the port an error
/// (`json.null-document`); a repeated `prebid` gives Go both members' tiers, decoded one into
/// the other, and the port an error (`json.duplicate-keys`); keys in another case give Go the
/// tier and the port none (`json.key-case`); and `prebid` given as an array fails in Go
/// (`cannot unmarshal Prebid: expect { or n, but found [`) and reads as no tiers here
/// (`json.struct-from-array`). The exchange drops an imp's tiers when the read fails
/// (exchange/exchange.go:673-676).
// Go: openrtb_ext/deal_tier.go:36
#[test]
fn read_deal_tiers_where_serde_differs() -> TestResult {
    let cases: [DealTierCase; 4] = [
        (
            "null document",
            Some("null"),
            Err(ErrorKind::FailedToUnmarshal),
        ),
        (
            "repeated prebid",
            Some(concat!(
                r#"{"prebid":{"bidder":{"appnexus":"#,
                r#"{"dealTier":{"minDealTier":5,"prefix":"a"}}}},"#,
                r#""prebid":{"bidder":{"rubicon":"#,
                r#"{"dealTier":{"minDealTier":8,"prefix":"r"}}}}}"#,
            )),
            Err(ErrorKind::FailedToUnmarshal),
        ),
        (
            "keys in another case",
            Some(concat!(
                r#"{"Prebid":{"Bidder":{"appnexus":"#,
                r#"{"DealTier":{"MinDealTier":5,"Prefix":"a"}}}}}"#,
            )),
            Ok(none()),
        ),
        ("prebid array", Some(r#"{"prebid":[]}"#), Ok(none())),
    ];
    all_ok(cases.into_iter().map(|(description, ext, expected)| {
        let got = read_deal_tiers_from_imp(&imp(ext)).map_err(|e| e.kind().clone());
        (description.to_owned(), ensure_eq(&got, &expected, "result"))
    }))
}
