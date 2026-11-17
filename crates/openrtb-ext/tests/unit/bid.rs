//! Go: `openrtb_ext/bid_test.go`.

use pbs_openrtb_ext::openrtb_ext::{
    BID_TYPE_AUDIO, BID_TYPE_BANNER, BID_TYPE_NATIVE, BID_TYPE_VIDEO, BIDDER_APPNEXUS, PB_KEY,
    TargetingKey, parse_bid_type,
};

use super::support::{TestResult, all_ok, ensure_eq};

/// Go: `TestBidderKey`.
#[test]
fn bidder_key() -> TestResult {
    ensure_eq(
        PB_KEY.bidder_key("hb", &BIDDER_APPNEXUS, 50).as_str(),
        "hb_pb_appnexus",
        "Bad resolved targeting key",
    )
}

/// Go: `TestTruncatedKey`.
#[test]
fn truncated_key() -> TestResult {
    ensure_eq(
        PB_KEY.bidder_key("hb", &BIDDER_APPNEXUS, 8).as_str(),
        "hb_pb_ap",
        "Bad truncated targeting key",
    )
}

/// Go: `TestTruncateKey`.
#[test]
fn truncate_key() -> TestResult {
    let cases = [
        (
            "Targeting key is smaller than max length, expect targeting key to stay the same",
            15,
            "_bidder_key",
            "hb_bidder_key",
        ),
        (
            "Targeting key is larger than max length, expect targeting key to be truncated",
            9,
            "_bidder_key",
            "hb_bidder",
        ),
        (
            "Max length isn't greater than zero, expect targeting key to not be truncated",
            0,
            "_bidder_key",
            "hb_bidder_key",
        ),
    ];
    all_ok(
        cases
            .into_iter()
            .map(|(description, max_length, key, expected)| {
                let truncated = TargetingKey::from(key).truncate_key("hb", max_length);
                (
                    description.to_owned(),
                    ensure_eq(
                        truncated.as_str(),
                        expected,
                        "The Targeting Key is incorrect",
                    ),
                )
            }),
    )
}

/// Go: `TestBidParsing` (and `assertBidParse`).
#[test]
fn bid_parsing() -> TestResult {
    for (s, bid_type) in [
        ("banner", BID_TYPE_BANNER),
        ("video", BID_TYPE_VIDEO),
        ("audio", BID_TYPE_AUDIO),
        ("native", BID_TYPE_NATIVE),
    ] {
        ensure_eq(
            &parse_bid_type(s).map_err(|e| e.to_string()),
            &Ok(bid_type),
            s,
        )?;
    }
    // Go returns "" with its error; here the error is the whole result.
    parse_bid_type("unknown").map_or(Ok(()), |parsed| {
        Err(format!(
            "ParseBidType did not return the expected error, got {parsed:?}"
        ))
    })
}
