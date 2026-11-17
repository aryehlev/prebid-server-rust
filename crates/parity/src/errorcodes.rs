//! The canonical names of the Go `errortypes` codes (Go: `parity/errorcodes.go`,
//! `error_codes.json`), used by [`ErrorRecord`](crate::tracedoc::ErrorRecord).

use pbs_errortypes::{
    ACCOUNT_DISABLED_ERROR_CODE, ACCOUNT_LEVEL_DEBUG_DISABLED_WARNING_CODE,
    ACCT_REQUIRED_ERROR_CODE, AD_SERVER_TARGETING_WARNING_CODE, ALTERNATE_BIDDER_CODE_WARNING_CODE,
    BAD_INPUT_ERROR_CODE, BAD_SERVER_RESPONSE_ERROR_CODE, BID_ADJUSTMENT_WARNING_CODE,
    BIDDER_BLOCKED_BY_PRIVACY_SETTINGS, BIDDER_LEVEL_DEBUG_DISABLED_WARNING_CODE,
    BIDDER_TEMPORARILY_DISABLED_ERROR_CODE, BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE,
    BLOCKED_APP_ERROR_CODE, DISABLED_CURRENCY_CONVERSION_WARNING_CODE,
    FAILED_TO_MARSHAL_ERROR_CODE, FAILED_TO_REQUEST_BIDS_ERROR_CODE,
    FAILED_TO_UNMARSHAL_ERROR_CODE, FLOOR_BID_REJECTION_WARNING_CODE,
    INVALID_BID_RESPONSE_DSA_WARNING_CODE, INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE,
    INVALID_PRIVACY_CONSENT_WARNING_CODE, INVALID_USER_EIDS_WARNING_CODE,
    INVALID_USER_UIDS_WARNING_CODE, MALFORMED_ACCT_ERROR_CODE, MODULE_REJECTION_ERROR_CODE,
    MULTI_BID_WARNING_CODE, NO_CONVERSION_RATE_ERROR_CODE, SEC_BROWSING_TOPICS_WARNING_CODE,
    SEC_COOKIE_DEPRECATION_LEN_WARNING_CODE, TIMEOUT_ERROR_CODE, TMAX_TIMEOUT_ERROR_CODE,
    TOO_LONG_TARGETING_PREFIX_WARNING_CODE, TOO_SHORT_TARGETING_PREFIX_WARNING_CODE,
    UNKNOWN_ERROR_CODE, UNKNOWN_WARNING_CODE,
};

use crate::tracedoc::ERROR_KIND_UNKNOWN;

/// Every registered code and its canonical kind, in code order (Go: `parity.errorKinds`).
const ERROR_KINDS: [(i64, &str); 35] = [
    (TIMEOUT_ERROR_CODE, "Timeout"),
    (BAD_INPUT_ERROR_CODE, "BadInput"),
    (BLOCKED_APP_ERROR_CODE, "BlockedApp"),
    (BAD_SERVER_RESPONSE_ERROR_CODE, "BadServerResponse"),
    (FAILED_TO_REQUEST_BIDS_ERROR_CODE, "FailedToRequestBids"),
    (
        BIDDER_TEMPORARILY_DISABLED_ERROR_CODE,
        "BidderTemporarilyDisabled",
    ),
    (ACCOUNT_DISABLED_ERROR_CODE, "AccountDisabled"),
    (ACCT_REQUIRED_ERROR_CODE, "AcctRequired"),
    (NO_CONVERSION_RATE_ERROR_CODE, "NoConversionRate"),
    (MALFORMED_ACCT_ERROR_CODE, "MalformedAcct"),
    (MODULE_REJECTION_ERROR_CODE, "ModuleRejection"),
    (TMAX_TIMEOUT_ERROR_CODE, "TmaxTimeout"),
    (FAILED_TO_MARSHAL_ERROR_CODE, "FailedToMarshal"),
    (FAILED_TO_UNMARSHAL_ERROR_CODE, "FailedToUnmarshal"),
    (
        INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE,
        "InvalidImpFirstPartyData",
    ),
    (
        BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE,
        "BidderTemporarilyThrottled",
    ),
    (UNKNOWN_ERROR_CODE, "UnknownError"),
    (
        INVALID_PRIVACY_CONSENT_WARNING_CODE,
        "InvalidPrivacyConsentWarning",
    ),
    (
        ACCOUNT_LEVEL_DEBUG_DISABLED_WARNING_CODE,
        "AccountLevelDebugDisabledWarning",
    ),
    (
        BIDDER_LEVEL_DEBUG_DISABLED_WARNING_CODE,
        "BidderLevelDebugDisabledWarning",
    ),
    (
        DISABLED_CURRENCY_CONVERSION_WARNING_CODE,
        "DisabledCurrencyConversionWarning",
    ),
    (
        ALTERNATE_BIDDER_CODE_WARNING_CODE,
        "AlternateBidderCodeWarning",
    ),
    (MULTI_BID_WARNING_CODE, "MultiBidWarning"),
    (AD_SERVER_TARGETING_WARNING_CODE, "AdServerTargetingWarning"),
    (BID_ADJUSTMENT_WARNING_CODE, "BidAdjustmentWarning"),
    (FLOOR_BID_REJECTION_WARNING_CODE, "FloorBidRejectionWarning"),
    (
        INVALID_BID_RESPONSE_DSA_WARNING_CODE,
        "InvalidBidResponseDSAWarning",
    ),
    (
        SEC_COOKIE_DEPRECATION_LEN_WARNING_CODE,
        "SecCookieDeprecationLenWarning",
    ),
    (SEC_BROWSING_TOPICS_WARNING_CODE, "SecBrowsingTopicsWarning"),
    (INVALID_USER_EIDS_WARNING_CODE, "InvalidUserEIDsWarning"),
    (INVALID_USER_UIDS_WARNING_CODE, "InvalidUserUIDsWarning"),
    (
        TOO_LONG_TARGETING_PREFIX_WARNING_CODE,
        "TooLongTargetingPrefixWarning",
    ),
    (
        TOO_SHORT_TARGETING_PREFIX_WARNING_CODE,
        "TooShortTargetingPrefixWarning",
    ),
    (
        BIDDER_BLOCKED_BY_PRIVACY_SETTINGS,
        "BidderBlockedByPrivacySettings",
    ),
    (UNKNOWN_WARNING_CODE, "UnknownWarning"),
];

/// The canonical name of a Go `errortypes` code, or [`ERROR_KIND_UNKNOWN`] for codes outside the
/// registry (Go: `parity.ErrorKindForCode`).
#[must_use]
pub fn error_kind_for_code(code: i64) -> &'static str {
    ERROR_KINDS
        .iter()
        .find(|(c, _)| *c == code)
        .map_or(ERROR_KIND_UNKNOWN, |(_, kind)| kind)
}

/// The code-to-kind registry in code order (Go: `parity.ErrorCodes`).
#[must_use]
pub const fn error_codes() -> &'static [(i64, &'static str)] {
    &ERROR_KINDS
}

#[cfg(test)]
mod tests {
    use super::{error_codes, error_kind_for_code};
    use crate::golden::{TestResult, check, check_eq, contract_json};

    // Go: parity/errorcodes_test.go: the registry matches error_codes.json.
    #[test]
    fn registry_matches_error_codes_json() -> TestResult {
        let doc = contract_json("error_codes.json")?;
        let mut want = Vec::new();
        for list in ["errors", "warnings"] {
            for entry in doc.array(list)? {
                want.push((entry.int("code")?, entry.string("kind")?));
            }
        }
        let got: Vec<(i64, String)> = error_codes()
            .iter()
            .map(|(code, kind)| (*code, (*kind).to_owned()))
            .collect();
        check_eq(&got, &want, "error codes and kinds in file order")?;
        check(
            got.windows(2)
                .all(|w| w.first().map(|a| a.0) < w.get(1).map(|b| b.0)),
            "codes ascend",
        )
    }

    #[test]
    fn kind_for_code() -> TestResult {
        check_eq(&error_kind_for_code(2), &"BadInput", "2")?;
        check_eq(&error_kind_for_code(999), &"UnknownError", "999")?;
        check_eq(
            &error_kind_for_code(10017),
            &"BidderBlockedByPrivacySettings",
            "10017",
        )?;
        check_eq(&error_kind_for_code(0), &"Unknown", "0")?;
        check_eq(&error_kind_for_code(17), &"Unknown", "17")?;
        check_eq(&error_kind_for_code(-1), &"Unknown", "-1")
    }
}
