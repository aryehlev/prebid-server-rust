//! Go: `openrtb_ext/alternatebiddercodes_test.go`.

use std::collections::BTreeMap;

use pbs_openrtb_ext::openrtb_ext::{ExtAdapterAlternateBidderCodes, ExtAlternateBidderCodes};

use super::support::{TestResult, all_ok, ensure_eq};

fn adapter(enabled: bool, allowed: Option<&[&str]>) -> ExtAdapterAlternateBidderCodes {
    ExtAdapterAlternateBidderCodes {
        enabled,
        allowed_bidder_codes: allowed.map(|codes| codes.iter().map(|&c| c.to_owned()).collect()),
    }
}

fn bidders<const N: usize>(
    entries: [(&str, ExtAdapterAlternateBidderCodes); N],
) -> BTreeMap<String, ExtAdapterAlternateBidderCodes> {
    entries
        .into_iter()
        .map(|(name, cfg)| (name.to_owned(), cfg))
        .collect()
}

/// A case of `TestAlternateBidderCodes_IsValidBidderCode`: its name, `fields` (`Enabled`,
/// `Bidders`), `args` (`bidder`, `alternateBidder`) and the error wanted.
type ValidCase = (
    &'static str,
    ExtAlternateBidderCodes,
    (&'static str, &'static str),
    Option<&'static str>,
);

const DISABLED: &str =
    r#"alternateBidderCodes disabled for "pubmatic", rejecting bids for "groupm""#;
const NOT_DEFINED: &str =
    r#"alternateBidderCodes not defined for adapter "pubmatic", rejecting bids for "groupm""#;
const INVALID: &str = r#"invalid biddercode "groupm" sent by adapter "pubmatic""#;

fn codes<const N: usize>(
    enabled: bool,
    entries: [(&str, ExtAdapterAlternateBidderCodes); N],
) -> ExtAlternateBidderCodes {
    ExtAlternateBidderCodes {
        enabled,
        bidders: Some(bidders(entries)),
    }
}

/// The cases of `TestAlternateBidderCodes_IsValidBidderCode` without an adapter entry.
fn cases_without_adapters() -> [ValidCase; 5] {
    let disabled = ExtAlternateBidderCodes::default();
    let enabled = ExtAlternateBidderCodes {
        enabled: true,
        bidders: None,
    };
    [
        (
            "alternateBidder is not set/blank (default non-extra bid case)",
            disabled.clone(),
            ("", ""),
            None,
        ),
        (
            "alternateBidder and bidder are same (default non-extra bid case with seat's \
                alternateBidder explicitly set)",
            disabled.clone(),
            ("pubmatic", "pubmatic"),
            None,
        ),
        (
            "alternateBidder and bidder are the same under Unicode case-folding (default \
                non-extra bid case with seat's alternateBidder explicitly set)",
            disabled.clone(),
            ("pubmatic", "pubmatic"),
            None,
        ),
        (
            "account.alternatebiddercodes config not defined (default, reject bid)",
            disabled,
            ("pubmatic", "groupm"),
            Some(DISABLED),
        ),
        (
            "account.alternatebiddercodes config enabled but adapter config not defined",
            enabled,
            ("pubmatic", "groupm"),
            Some(NOT_DEFINED),
        ),
    ]
}

/// The cases of `TestAlternateBidderCodes_IsValidBidderCode` with an adapter entry.
fn cases_with_adapters() -> [ValidCase; 8] {
    [
        (
            "account.alternatebiddercodes config enabled but adapter config is not available",
            codes(true, [("appnexus", adapter(false, None))]),
            ("pubmatic", "groupm"),
            Some(NOT_DEFINED),
        ),
        (
            "account.alternatebiddercodes config enabled but adapter config is disabled",
            codes(true, [("pubmatic", adapter(false, None))]),
            ("pubmatic", "groupm"),
            Some(DISABLED),
        ),
        (
            "account.alternatebiddercodes and adapter config enabled but adapter config does not \
                have allowedBidderCodes defined",
            codes(true, [("pubmatic", adapter(true, None))]),
            ("pubmatic", "groupm"),
            None,
        ),
        (
            "bidder is different in casing than the entry in account.alternatebiddercodes but \
                they match because our case insensitive comparison",
            codes(true, [("pubmatic", adapter(true, None))]),
            ("PUBmatic", "groupm"),
            None,
        ),
        (
            "allowedBidderCodes is *",
            codes(true, [("pubmatic", adapter(true, Some(&["*"])))]),
            ("pubmatic", "groupm"),
            None,
        ),
        (
            "allowedBidderCodes is in the list",
            codes(true, [("pubmatic", adapter(true, Some(&["groupm"])))]),
            ("pubmatic", "groupm"),
            None,
        ),
        (
            "allowedBidderCodes is not in the list",
            codes(true, [("pubmatic", adapter(true, Some(&["xyz"])))]),
            ("pubmatic", "groupm"),
            Some(INVALID),
        ),
        (
            "account.alternatebiddercodes and adapter config enabled but adapter config has \
                allowedBidderCodes list empty",
            codes(true, [("pubmatic", adapter(true, Some(&[])))]),
            ("pubmatic", "groupm"),
            Some(INVALID),
        ),
    ]
}

/// Go: `TestAlternateBidderCodes_IsValidBidderCode`. Go's `(true, nil)` is `Ok(())`, and its
/// `(false, err)` is `Err(err)`.
#[test]
fn is_valid_bidder_code() -> TestResult {
    let cases = cases_without_adapters()
        .into_iter()
        .chain(cases_with_adapters());
    all_ok(
        cases.map(|(name, a, (bidder, alternate_bidder), want_err)| {
            let got = a
                .is_valid_bidder_code(bidder, alternate_bidder)
                .map_err(|e| e.to_string());
            let want = want_err.map_or(Ok(()), |e| Err(e.to_owned()));
            (name.to_owned(), ensure_eq(&got, &want, "result"))
        }),
    )
}

/// Go: `TestIsBidderInAlternateBidderCodes`. Go's `(ExtAdapterAlternateBidderCodes{}, false)` is
/// `None`, and its nil receiver is the caller's `None`.
#[test]
fn is_bidder_in_alternate_bidder_codes() -> TestResult {
    let three = || {
        Some(bidders([
            ("AppNexus", adapter(false, Some(&["adnxsCode"]))),
            ("PubMatic", adapter(false, Some(&["pubCode"]))),
            ("Rubicon", adapter(false, Some(&["rCode"]))),
        ]))
    };
    let cases = [
        (
            "empty bidder",
            "",
            Some(ExtAlternateBidderCodes::default()),
            None,
        ),
        ("nil ExtAlternateBidderCodes", "appnexus", None, None),
        (
            "nil ExtAlternateBidderCodes.Bidder map",
            "appnexus",
            Some(ExtAlternateBidderCodes::default()),
            None,
        ),
        (
            "nil ExtAlternateBidderCodes.Bidder map",
            "appnexus",
            Some(ExtAlternateBidderCodes {
                bidders: None,
                ..ExtAlternateBidderCodes::default()
            }),
            None,
        ),
        (
            "bidder arg identical to entry in Bidders map",
            "appnexus",
            Some(ExtAlternateBidderCodes {
                bidders: Some(bidders([("appnexus", adapter(true, Some(&["abcCode"])))])),
                ..ExtAlternateBidderCodes::default()
            }),
            Some(adapter(true, Some(&["abcCode"]))),
        ),
        (
            "bidder arg matches an entry in Bidders map with case insensitive comparisson",
            "appnexus",
            Some(ExtAlternateBidderCodes {
                bidders: three(),
                ..ExtAlternateBidderCodes::default()
            }),
            Some(adapter(false, Some(&["adnxsCode"]))),
        ),
        (
            "bidder arg doesn't match any entry in map",
            "unknown",
            Some(ExtAlternateBidderCodes {
                bidders: three(),
                ..ExtAlternateBidderCodes::default()
            }),
            None,
        ),
    ];
    all_ok(cases.into_iter().map(|(desc, bidder, codes, expected)| {
        let got = codes
            .as_ref()
            .and_then(|c| c.is_bidder_in_alternate_bidder_codes(bidder))
            .cloned();
        (
            desc.to_owned(),
            ensure_eq(&got, &expected, "adapter config"),
        )
    }))
}
