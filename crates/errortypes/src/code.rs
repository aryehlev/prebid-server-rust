//! Numeric error and warning codes.
//!
//! Go: `errortypes/code.go`. The values reproduce Go's `iota` arithmetic exactly: the first
//! constant of each block (`UnknownErrorCode = 999`, `UnknownWarningCode = 10999`) consumes
//! `iota == 0`, so the well-known codes start at 1 and 10001 respectively.

use crate::GoError;

/// Go: `errortypes.UnknownErrorCode`. Returned by [`read_code`] for errors that do not implement
/// `errortypes.Coder` (for example `errors.New` and `fmt.Errorf` values).
pub const UNKNOWN_ERROR_CODE: i64 = 999;
/// Go: `errortypes.TimeoutErrorCode`.
pub const TIMEOUT_ERROR_CODE: i64 = 1;
/// Go: `errortypes.BadInputErrorCode`.
pub const BAD_INPUT_ERROR_CODE: i64 = 2;
/// Go: `errortypes.BlockedAppErrorCode`.
pub const BLOCKED_APP_ERROR_CODE: i64 = 3;
/// Go: `errortypes.BadServerResponseErrorCode`.
pub const BAD_SERVER_RESPONSE_ERROR_CODE: i64 = 4;
/// Go: `errortypes.FailedToRequestBidsErrorCode`.
pub const FAILED_TO_REQUEST_BIDS_ERROR_CODE: i64 = 5;
/// Go: `errortypes.BidderTemporarilyDisabledErrorCode`.
pub const BIDDER_TEMPORARILY_DISABLED_ERROR_CODE: i64 = 6;
/// Go: `errortypes.AccountDisabledErrorCode`.
pub const ACCOUNT_DISABLED_ERROR_CODE: i64 = 7;
/// Go: `errortypes.AcctRequiredErrorCode`.
pub const ACCT_REQUIRED_ERROR_CODE: i64 = 8;
/// Go: `errortypes.NoConversionRateErrorCode`. No Go type returns it; kept for completeness.
pub const NO_CONVERSION_RATE_ERROR_CODE: i64 = 9;
/// Go: `errortypes.MalformedAcctErrorCode`.
pub const MALFORMED_ACCT_ERROR_CODE: i64 = 10;
/// Go: `errortypes.ModuleRejectionErrorCode` (returned by `hookexecution.RejectError`).
pub const MODULE_REJECTION_ERROR_CODE: i64 = 11;
/// Go: `errortypes.TmaxTimeoutErrorCode`.
pub const TMAX_TIMEOUT_ERROR_CODE: i64 = 12;
/// Go: `errortypes.FailedToMarshalErrorCode`.
pub const FAILED_TO_MARSHAL_ERROR_CODE: i64 = 13;
/// Go: `errortypes.FailedToUnmarshalErrorCode`.
pub const FAILED_TO_UNMARSHAL_ERROR_CODE: i64 = 14;
/// Go: `errortypes.InvalidImpFirstPartyDataErrorCode`.
pub const INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE: i64 = 15;
/// Go: `errortypes.BidderTemporarilyThrottledErrorCode` (returned by `errortypes.BidderThrottled`).
pub const BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE: i64 = 16;

/// Go: `errortypes.UnknownWarningCode`.
pub const UNKNOWN_WARNING_CODE: i64 = 10999;
/// Go: `errortypes.InvalidPrivacyConsentWarningCode`.
pub const INVALID_PRIVACY_CONSENT_WARNING_CODE: i64 = 10001;
/// Go: `errortypes.AccountLevelDebugDisabledWarningCode`.
pub const ACCOUNT_LEVEL_DEBUG_DISABLED_WARNING_CODE: i64 = 10002;
/// Go: `errortypes.BidderLevelDebugDisabledWarningCode`.
pub const BIDDER_LEVEL_DEBUG_DISABLED_WARNING_CODE: i64 = 10003;
/// Go: `errortypes.DisabledCurrencyConversionWarningCode`.
pub const DISABLED_CURRENCY_CONVERSION_WARNING_CODE: i64 = 10004;
/// Go: `errortypes.AlternateBidderCodeWarningCode`.
pub const ALTERNATE_BIDDER_CODE_WARNING_CODE: i64 = 10005;
/// Go: `errortypes.MultiBidWarningCode`.
pub const MULTI_BID_WARNING_CODE: i64 = 10006;
/// Go: `errortypes.AdServerTargetingWarningCode`.
pub const AD_SERVER_TARGETING_WARNING_CODE: i64 = 10007;
/// Go: `errortypes.BidAdjustmentWarningCode`.
pub const BID_ADJUSTMENT_WARNING_CODE: i64 = 10008;
/// Go: `errortypes.FloorBidRejectionWarningCode`.
pub const FLOOR_BID_REJECTION_WARNING_CODE: i64 = 10009;
/// Go: `errortypes.InvalidBidResponseDSAWarningCode`.
pub const INVALID_BID_RESPONSE_DSA_WARNING_CODE: i64 = 10010;
/// Go: `errortypes.SecCookieDeprecationLenWarningCode`.
pub const SEC_COOKIE_DEPRECATION_LEN_WARNING_CODE: i64 = 10011;
/// Go: `errortypes.SecBrowsingTopicsWarningCode`.
pub const SEC_BROWSING_TOPICS_WARNING_CODE: i64 = 10012;
/// Go: `errortypes.InvalidUserEIDsWarningCode`.
pub const INVALID_USER_EIDS_WARNING_CODE: i64 = 10013;
/// Go: `errortypes.InvalidUserUIDsWarningCode`.
pub const INVALID_USER_UIDS_WARNING_CODE: i64 = 10014;
/// Go: `errortypes.TooLongTargetingPrefixWarningCode`.
pub const TOO_LONG_TARGETING_PREFIX_WARNING_CODE: i64 = 10015;
/// Go: `errortypes.TooShortTargetingPrefixWarningCode`.
pub const TOO_SHORT_TARGETING_PREFIX_WARNING_CODE: i64 = 10016;
/// Go: `errortypes.BidderBlockedByPrivacySettings` (the Go name has no `WarningCode` suffix).
pub const BIDDER_BLOCKED_BY_PRIVACY_SETTINGS: i64 = 10017;

/// Go: `errortypes.ReadCode`. Returns the error or warning code, or [`UNKNOWN_ERROR_CODE`] when
/// the error does not implement `errortypes.Coder`.
#[must_use]
pub fn read_code(err: &GoError) -> i64 {
    err.coder().map_or(UNKNOWN_ERROR_CODE, |(code, _)| code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    // Go: errortypes/code_test.go TestReadCodeWithCodeDefined.
    #[test]
    fn read_code_with_code_defined() {
        let err = GoError::timeout("code is defined");
        assert_eq!(read_code(&err), TIMEOUT_ERROR_CODE, "timeout code");
    }

    // Go: errortypes/code_test.go TestReadCodeWithCodeNotDefined.
    #[test]
    fn read_code_with_code_not_defined() {
        let err = GoError::generic("missing error code");
        assert_eq!(read_code(&err), UNKNOWN_ERROR_CODE, "generic code");
    }

    // The iota arithmetic of errortypes/code.go. testdata/go/errortypes/codes.txt is printed by
    // compiling the real Go package, in declaration order.
    #[test]
    fn code_table_matches_go_iota() -> Result<(), String> {
        let all = [
            UNKNOWN_ERROR_CODE,
            TIMEOUT_ERROR_CODE,
            BAD_INPUT_ERROR_CODE,
            BLOCKED_APP_ERROR_CODE,
            BAD_SERVER_RESPONSE_ERROR_CODE,
            FAILED_TO_REQUEST_BIDS_ERROR_CODE,
            BIDDER_TEMPORARILY_DISABLED_ERROR_CODE,
            ACCOUNT_DISABLED_ERROR_CODE,
            ACCT_REQUIRED_ERROR_CODE,
            NO_CONVERSION_RATE_ERROR_CODE,
            MALFORMED_ACCT_ERROR_CODE,
            MODULE_REJECTION_ERROR_CODE,
            TMAX_TIMEOUT_ERROR_CODE,
            FAILED_TO_MARSHAL_ERROR_CODE,
            FAILED_TO_UNMARSHAL_ERROR_CODE,
            INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE,
            BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE,
            UNKNOWN_WARNING_CODE,
            INVALID_PRIVACY_CONSENT_WARNING_CODE,
            ACCOUNT_LEVEL_DEBUG_DISABLED_WARNING_CODE,
            BIDDER_LEVEL_DEBUG_DISABLED_WARNING_CODE,
            DISABLED_CURRENCY_CONVERSION_WARNING_CODE,
            ALTERNATE_BIDDER_CODE_WARNING_CODE,
            MULTI_BID_WARNING_CODE,
            AD_SERVER_TARGETING_WARNING_CODE,
            BID_ADJUSTMENT_WARNING_CODE,
            FLOOR_BID_REJECTION_WARNING_CODE,
            INVALID_BID_RESPONSE_DSA_WARNING_CODE,
            SEC_COOKIE_DEPRECATION_LEN_WARNING_CODE,
            SEC_BROWSING_TOPICS_WARNING_CODE,
            INVALID_USER_EIDS_WARNING_CODE,
            INVALID_USER_UIDS_WARNING_CODE,
            TOO_LONG_TARGETING_PREFIX_WARNING_CODE,
            TOO_SHORT_TARGETING_PREFIX_WARNING_CODE,
            BIDDER_BLOCKED_BY_PRIVACY_SETTINGS,
        ];
        let golden = include_str!("../testdata/go/errortypes/codes.txt")
            .lines()
            .map(str::parse::<i64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        ensure_eq(all.as_slice(), golden.as_slice(), "codes match Go")
    }
}
