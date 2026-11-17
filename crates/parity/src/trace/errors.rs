//! Language-neutral error records (Go: `parity/trace/errors.go`).

use pbs_errortypes::{
    GoError, Scope, Severity, UNKNOWN_ERROR_CODE, is_fatal, is_warning, read_code,
};

use crate::{
    errorcodes::error_kind_for_code,
    tracedoc::{
        ErrorRecord, SCOPE_DEBUG, SCOPE_GENERAL, SEVERITY_FATAL, SEVERITY_UNKNOWN, SEVERITY_WARNING,
    },
};

/// The record of one Go error (Go: `trace.ErrorRecordOf`).
#[must_use]
pub fn error_record_of(err: &GoError) -> ErrorRecord {
    let code = read_code(err);
    ErrorRecord {
        code: code_int(code),
        kind: error_kind_for_code(code).to_owned(),
        message: err.to_string(),
        scope: if err.scope() == Scope::Debug {
            SCOPE_DEBUG
        } else {
            SCOPE_GENERAL
        }
        .to_owned(),
        severity: match err.coder() {
            None | Some((_, Severity::Unknown)) => SEVERITY_UNKNOWN,
            Some((_, Severity::Fatal)) => SEVERITY_FATAL,
            Some((_, Severity::Warning)) => SEVERITY_WARNING,
        }
        .to_owned(),
        // Go: len(errortypes.FatalOnly([]error{err})) == 1.
        fatal: is_fatal(err),
    }
}

/// The record Go writes for a nil `error` (Go: `trace.ErrorRecordOf(nil)`): unknown code, empty
/// message, fatal. Rust error lists cannot hold a nil error; this is for code that models one.
#[must_use]
pub fn nil_error_record() -> ErrorRecord {
    ErrorRecord {
        code: code_int(UNKNOWN_ERROR_CODE),
        kind: error_kind_for_code(UNKNOWN_ERROR_CODE).to_owned(),
        message: String::new(),
        scope: SCOPE_GENERAL.to_owned(),
        severity: SEVERITY_UNKNOWN.to_owned(),
        fatal: true,
    }
}

/// Splits `errs` into fatal-or-unknown errors and warnings (`errortypes.IsWarning`), keeping
/// input order (Go: `trace.ErrorRecordsOf`).
#[must_use]
pub fn error_records_of(errs: &[GoError]) -> (Vec<ErrorRecord>, Vec<ErrorRecord>) {
    let mut errors = Vec::with_capacity(errs.len());
    let mut warnings = Vec::new();
    for err in errs {
        let rec = error_record_of(err);
        if is_warning(err) {
            warnings.push(rec);
        } else {
            errors.push(rec);
        }
    }
    (errors, warnings)
}

/// A Go `int` code (64-bit, like `isize` on every supported target).
fn code_int(code: i64) -> isize {
    isize::try_from(code).unwrap_or(isize::MAX)
}
