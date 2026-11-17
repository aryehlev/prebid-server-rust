// Command errortypes writes the golden files replayed by the pbs-errortypes Rust tests.
//
// Run from the directory of this module (crates/errortypes/testdata/go):
//
//	go run ./errortypes errortypes
package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/prebid/prebid-server/v4/errortypes"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: errortypes <output dir>")
		os.Exit(2)
	}
	dir := os.Args[1]
	if err := os.WriteFile(filepath.Join(dir, "golden.txt"), []byte(kinds()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if err := os.WriteFile(filepath.Join(dir, "codes.txt"), []byte(codes()), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// kinds prints, for every error type, the values the Rust GoError must reproduce:
// %T, ReadCode, Severity (or -1 when not a Coder), ReadScope, fatal, IsWarning, Error().
func kinds() string {
	errs := []error{
		&errortypes.Timeout{Message: "m"},
		&errortypes.TmaxTimeout{Message: "m"},
		&errortypes.BadInput{Message: "m"},
		&errortypes.BlockedApp{Message: "m"},
		&errortypes.AccountDisabled{Message: "m"},
		&errortypes.AcctRequired{Message: "m"},
		&errortypes.BadServerResponse{Message: "m"},
		&errortypes.FailedToRequestBids{Message: "m"},
		&errortypes.BidderTemporarilyDisabled{Message: "m"},
		&errortypes.BidderThrottled{Message: "m"},
		&errortypes.MalformedAcct{Message: "m"},
		&errortypes.Warning{Message: "m", WarningCode: errortypes.AlternateBidderCodeWarningCode},
		&errortypes.FailedToUnmarshal{Message: "m"},
		&errortypes.FailedToMarshal{Message: "m"},
		&errortypes.DebugWarning{Message: "m", WarningCode: errortypes.BidderLevelDebugDisabledWarningCode},
		&errortypes.InvalidImpFirstPartyData{Message: "m"},
		errors.New("m"),
	}
	var b strings.Builder
	for _, err := range errs {
		severity := -1
		if c, ok := err.(errortypes.Coder); ok {
			severity = int(c.Severity())
		}
		fatal := 0
		if errortypes.ContainsFatalError([]error{err}) {
			fatal = 1
		}
		warning := 0
		if errortypes.IsWarning(err) {
			warning = 1
		}
		fmt.Fprintf(&b, "%T\t%d\t%d\t%d\t%d\t%d\t%s\n", err, errortypes.ReadCode(err), severity,
			int(errortypes.ReadScope(err)), fatal, warning, err.Error())
	}
	return b.String()
}

// codes prints every code constant in declaration order.
func codes() string {
	values := []int{
		errortypes.UnknownErrorCode,
		errortypes.TimeoutErrorCode,
		errortypes.BadInputErrorCode,
		errortypes.BlockedAppErrorCode,
		errortypes.BadServerResponseErrorCode,
		errortypes.FailedToRequestBidsErrorCode,
		errortypes.BidderTemporarilyDisabledErrorCode,
		errortypes.AccountDisabledErrorCode,
		errortypes.AcctRequiredErrorCode,
		errortypes.NoConversionRateErrorCode,
		errortypes.MalformedAcctErrorCode,
		errortypes.ModuleRejectionErrorCode,
		errortypes.TmaxTimeoutErrorCode,
		errortypes.FailedToMarshalErrorCode,
		errortypes.FailedToUnmarshalErrorCode,
		errortypes.InvalidImpFirstPartyDataErrorCode,
		errortypes.BidderTemporarilyThrottledErrorCode,
		errortypes.UnknownWarningCode,
		errortypes.InvalidPrivacyConsentWarningCode,
		errortypes.AccountLevelDebugDisabledWarningCode,
		errortypes.BidderLevelDebugDisabledWarningCode,
		errortypes.DisabledCurrencyConversionWarningCode,
		errortypes.AlternateBidderCodeWarningCode,
		errortypes.MultiBidWarningCode,
		errortypes.AdServerTargetingWarningCode,
		errortypes.BidAdjustmentWarningCode,
		errortypes.FloorBidRejectionWarningCode,
		errortypes.InvalidBidResponseDSAWarningCode,
		errortypes.SecCookieDeprecationLenWarningCode,
		errortypes.SecBrowsingTopicsWarningCode,
		errortypes.InvalidUserEIDsWarningCode,
		errortypes.InvalidUserUIDsWarningCode,
		errortypes.TooLongTargetingPrefixWarningCode,
		errortypes.TooShortTargetingPrefixWarningCode,
		errortypes.BidderBlockedByPrivacySettings,
	}
	var b strings.Builder
	for _, v := range values {
		fmt.Fprintf(&b, "%d\n", v)
	}
	return b.String()
}
