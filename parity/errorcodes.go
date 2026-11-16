package parity

// errorKinds maps Go errortypes numeric codes to their canonical, language-neutral
// names. It mirrors parity/schema/error_codes.json; a test keeps both in sync with
// the errortypes constants.
var errorKinds = map[int]string{
	1:     "Timeout",
	2:     "BadInput",
	3:     "BlockedApp",
	4:     "BadServerResponse",
	5:     "FailedToRequestBids",
	6:     "BidderTemporarilyDisabled",
	7:     "AccountDisabled",
	8:     "AcctRequired",
	9:     "NoConversionRate",
	10:    "MalformedAcct",
	11:    "ModuleRejection",
	12:    "TmaxTimeout",
	13:    "FailedToMarshal",
	14:    "FailedToUnmarshal",
	15:    "InvalidImpFirstPartyData",
	16:    "BidderTemporarilyThrottled",
	999:   "UnknownError",
	10001: "InvalidPrivacyConsentWarning",
	10002: "AccountLevelDebugDisabledWarning",
	10003: "BidderLevelDebugDisabledWarning",
	10004: "DisabledCurrencyConversionWarning",
	10005: "AlternateBidderCodeWarning",
	10006: "MultiBidWarning",
	10007: "AdServerTargetingWarning",
	10008: "BidAdjustmentWarning",
	10009: "FloorBidRejectionWarning",
	10010: "InvalidBidResponseDSAWarning",
	10011: "SecCookieDeprecationLenWarning",
	10012: "SecBrowsingTopicsWarning",
	10013: "InvalidUserEIDsWarning",
	10014: "InvalidUserUIDsWarning",
	10015: "TooLongTargetingPrefixWarning",
	10016: "TooShortTargetingPrefixWarning",
	10017: "BidderBlockedByPrivacySettings",
	10999: "UnknownWarning",
}

// ErrorKindForCode returns the canonical name of a Go errortypes code, or
// ErrorKindUnknown for codes outside the registry.
func ErrorKindForCode(code int) string {
	if kind, ok := errorKinds[code]; ok {
		return kind
	}
	return ErrorKindUnknown
}

// ErrorCodes returns a copy of the code-to-kind registry.
func ErrorCodes() map[int]string {
	out := make(map[int]string, len(errorKinds))
	for code, kind := range errorKinds {
		out[code] = kind
	}
	return out
}
