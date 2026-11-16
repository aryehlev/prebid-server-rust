package trace

import (
	"github.com/prebid/prebid-server/v4/errortypes"
	"github.com/prebid/prebid-server/v4/parity"
)

// ErrorRecordOf converts one Go error to its language-neutral record. A nil
// error yields a record with UnknownErrorCode and an empty message.
func ErrorRecordOf(err error) parity.ErrorRecord {
	if err == nil {
		return parity.ErrorRecord{
			Code:     errortypes.UnknownErrorCode,
			Kind:     parity.ErrorKindForCode(errortypes.UnknownErrorCode),
			Scope:    parity.ScopeGeneral,
			Severity: parity.SeverityUnknown,
			Fatal:    true,
		}
	}
	code := errortypes.ReadCode(err)
	return parity.ErrorRecord{
		Code:     code,
		Kind:     parity.ErrorKindForCode(code),
		Message:  err.Error(),
		Scope:    scopeOf(err),
		Severity: severityOf(err),
		Fatal:    len(errortypes.FatalOnly([]error{err})) == 1,
	}
}

// ErrorRecordsOf splits errs into fatal-or-unknown errors and warnings, keeping
// input order. Both results are non-nil.
func ErrorRecordsOf(errs []error) (errorsOut, warnings []parity.ErrorRecord) {
	errorsOut = make([]parity.ErrorRecord, 0, len(errs))
	warnings = make([]parity.ErrorRecord, 0)
	for _, err := range errs {
		rec := ErrorRecordOf(err)
		if errortypes.IsWarning(err) {
			warnings = append(warnings, rec)
			continue
		}
		errorsOut = append(errorsOut, rec)
	}
	return errorsOut, warnings
}

func scopeOf(err error) string {
	if errortypes.ReadScope(err) == errortypes.ScopeDebug {
		return parity.ScopeDebug
	}
	return parity.ScopeGeneral
}

func severityOf(err error) string {
	coder, ok := err.(errortypes.Coder)
	if !ok {
		return parity.SeverityUnknown
	}
	switch coder.Severity() {
	case errortypes.SeverityFatal:
		return parity.SeverityFatal
	case errortypes.SeverityWarning:
		return parity.SeverityWarning
	default:
		return parity.SeverityUnknown
	}
}
