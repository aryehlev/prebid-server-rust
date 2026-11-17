//! Go: `openrtb_ext/multibid_test.go` (`TestValidateAndBuildExtMultiBid`; `Test_addMultiBid`
//! is a unit test of the private function in `src/multibid.rs`).

use pbs_openrtb_ext::openrtb_ext::{
    ExtMultiBid, ExtRequestPrebid, validate_and_build_ext_multi_bid,
};

use super::support::{TestResult, all_ok, ensure_eq};

/// An entry (Go: `ExtMultiBid{...}`).
fn mb(
    bidder: &str,
    bidders: Option<&[&str]>,
    max_bids: Option<isize>,
    prefix: &str,
) -> ExtMultiBid {
    ExtMultiBid {
        bidder: bidder.to_owned(),
        bidders: bidders.map(|names| names.iter().map(|&n| n.to_owned()).collect()),
        max_bids,
        target_bidder_code_prefix: prefix.to_owned(),
    }
}

/// A request with these `multibid` entries (Go: `[]*ExtMultiBid{...}`).
fn request(entries: Vec<ExtMultiBid>) -> ExtRequestPrebid {
    ExtRequestPrebid {
        multi_bid: Some(entries.into_iter().map(|e| Some(Box::new(e))).collect()),
        ..ExtRequestPrebid::default()
    }
}

/// Go's `fmt.Errorf("...%v", ExtMultiBid{...})` text: `%v` calls `ExtMultiBid.String`.
fn err(prefix: &str, entry: &ExtMultiBid) -> String {
    format!("{prefix}{}", entry.string())
}

/// A case of `TestValidateAndBuildExtMultiBid`: its name, the request, and the validated entries
/// and errors wanted.
type BuildCase = (
    &'static str,
    Option<ExtRequestPrebid>,
    Option<Vec<ExtMultiBid>>,
    Vec<String>,
);

/// The cases of `TestValidateAndBuildExtMultiBid` up to `Only Bidder defined`.
fn bidder_cases() -> Vec<BuildCase> {
    vec![
        ("prebid nil", None, None, vec![]),
        (
            "prebid.MultiBid nil",
            Some(ExtRequestPrebid::default()),
            None,
            vec![],
        ),
        ("prebid.MultiBid empty", Some(request(vec![])), None, vec![]),
        (
            "MaxBids not defined",
            Some(request(vec![mb("pubmatic", None, None, "")])),
            None,
            vec![err(
                "maxBids not defined for ",
                &mb("pubmatic", None, None, ""),
            )],
        ),
        (
            "Bidder or Bidders not defined",
            Some(request(vec![mb("", None, Some(2), "")])),
            None,
            vec![err(
                "bidder(s) not specified for ",
                &mb("", None, Some(2), ""),
            )],
        ),
        (
            "Input bidder is already present in multibid",
            Some(request(vec![
                mb("pubmatic", None, Some(2), "pm"),
                mb("pubmatic", None, Some(3), "pubm"),
            ])),
            Some(vec![mb("pubmatic", None, Some(2), "pm")]),
            vec![err(
                "multiBid already defined for pubmatic, ignoring this instance ",
                &mb("pubmatic", None, Some(3), "pubm"),
            )],
        ),
        (
            "Bidder and Bidders both defined (only Bidder will be used)",
            Some(request(vec![
                mb("pubmatic", None, Some(2), "pm"),
                mb("appnexus", Some(&["rubicon"]), Some(2), "appN"),
            ])),
            Some(vec![
                mb("pubmatic", None, Some(2), "pm"),
                mb("appnexus", None, Some(2), "appN"),
            ]),
            vec![err(
                "ignoring bidders from ",
                &mb("appnexus", Some(&["rubicon"]), Some(2), "appN"),
            )],
        ),
        (
            "Only Bidder defined",
            Some(request(vec![mb("appnexus", None, Some(2), "appN")])),
            Some(vec![mb("appnexus", None, Some(2), "appN")]),
            vec![],
        ),
    ]
}

/// The cases of `TestValidateAndBuildExtMultiBid` from `Only Bidders defined` on.
fn bidders_cases() -> Vec<BuildCase> {
    let pm = &["appnexus", "pubmatic"][..];
    vec![
        (
            "Only Bidders defined",
            Some(request(vec![mb(
                "",
                Some(&["appnexus", "someBidder"]),
                Some(2),
                "",
            )])),
            Some(vec![mb("", Some(&["appnexus", "someBidder"]), Some(2), "")]),
            vec![],
        ),
        (
            "Bidders defined where a bidder is already present in multibid",
            Some(request(vec![
                mb("pubmatic", None, Some(9), "pm"),
                mb("", Some(pm), Some(2), ""),
            ])),
            Some(vec![
                mb("pubmatic", None, Some(9), "pm"),
                mb("", Some(&["appnexus"]), Some(2), ""),
            ]),
            vec![err(
                "multiBid already defined for pubmatic, ignoring this instance ",
                &mb("", Some(pm), Some(2), ""),
            )],
        ),
        (
            "Bidders defined where all the bidders are already present in multibid",
            Some(request(vec![
                mb("pubmatic", None, Some(9), "pm"),
                mb("", Some(&["appnexus"]), Some(3), ""),
                mb("", Some(pm), Some(2), ""),
            ])),
            Some(vec![
                mb("pubmatic", None, Some(9), "pm"),
                mb("", Some(&["appnexus"]), Some(3), ""),
            ]),
            vec![
                err(
                    "multiBid already defined for appnexus, ignoring this instance ",
                    &mb("", Some(pm), Some(2), ""),
                ),
                err(
                    "multiBid already defined for pubmatic, ignoring this instance ",
                    &mb("", Some(pm), Some(2), ""),
                ),
            ],
        ),
        (
            "Bidders defined along with TargetBidderCodePrefix",
            Some(request(vec![mb("", Some(&["appnexus"]), Some(2), "appN")])),
            Some(vec![mb("", Some(&["appnexus"]), Some(2), "")]),
            vec![err(
                "ignoring targetbiddercodeprefix for ",
                &mb("", Some(&["appnexus"]), Some(2), "appN"),
            )],
        ),
        (
            "MaxBids defined below minimum limit",
            Some(request(vec![mb("appnexus", None, Some(0), "appN")])),
            Some(vec![mb("appnexus", None, Some(1), "appN")]),
            vec![err(
                "invalid maxBids value, using minimum 1 limit for ",
                &mb("appnexus", None, Some(0), "appN"),
            )],
        ),
        (
            "MaxBids defined over max limit",
            Some(request(vec![mb("appnexus", None, Some(10), "appN")])),
            Some(vec![mb("appnexus", None, Some(9), "appN")]),
            vec![err(
                "invalid maxBids value, using maximum 9 limit for ",
                &mb("appnexus", None, Some(10), "appN"),
            )],
        ),
    ]
}

/// Go: `TestValidateAndBuildExtMultiBid`.
#[test]
fn validate_and_build() -> TestResult {
    let cases = bidder_cases().into_iter().chain(bidders_cases());
    all_ok(cases.map(|(name, mut prebid, want_validated, want_errs)| {
        let (validated, errs) = validate_and_build_ext_multi_bid(prebid.as_mut());
        let errs: Vec<String> = errs.iter().map(ToString::to_string).collect();
        let result = ensure_eq(&validated, &want_validated, "validated multibids")
            .and_then(|()| ensure_eq(&errs, &want_errs, "errors"));
        (name.to_owned(), result)
    }))
}
