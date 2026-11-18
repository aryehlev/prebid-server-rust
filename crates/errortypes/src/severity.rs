//! Error severities and the fatal/warning filters.
//!
//! Go: `errortypes/severity.go`. An error is fatal when it does not implement `errortypes.Coder`
//! or when its severity is `SeverityFatal`; it is a warning only when it implements `Coder` with
//! `SeverityWarning`. A `Coder` reporting `SeverityUnknown` is therefore neither.

use crate::GoError;

/// Go: `errortypes.Severity` (an `int`; the discriminants are the Go values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Go: `errortypes.SeverityUnknown` (0).
    Unknown = 0,
    /// Go: `errortypes.SeverityFatal` (1): a bid processing error which prevents a bid response.
    Fatal = 1,
    /// Go: `errortypes.SeverityWarning` (2): invalid or ambiguous data in the request was ignored.
    Warning = 2,
}

impl Severity {
    /// The Go integer value of the severity.
    #[must_use]
    pub const fn as_int(self) -> i64 {
        self as i64
    }
}

/// Go: `errortypes.isFatal` (unexported). True when `err` is not a `Coder` or its severity is
/// fatal.
#[must_use]
pub fn is_fatal(err: &GoError) -> bool {
    err.coder()
        .is_none_or(|(_, severity)| severity == Severity::Fatal)
}

/// Go: `errortypes.IsWarning`. True only when `err` is a `Coder` with warning severity.
#[must_use]
pub fn is_warning(err: &GoError) -> bool {
    err.coder()
        .is_some_and(|(_, severity)| severity == Severity::Warning)
}

/// Go: `errortypes.ContainsFatalError`.
#[must_use]
pub fn contains_fatal_error(errors: &[GoError]) -> bool {
    errors.iter().any(is_fatal)
}

/// Go: `errortypes.FatalOnly`. Returns a new list holding the fatal errors in their original order.
#[must_use]
pub fn fatal_only(errs: &[GoError]) -> Vec<GoError> {
    errs.iter().filter(|err| is_fatal(err)).cloned().collect()
}

/// Go: `errortypes.WarningOnly`. Returns a new list holding the warnings in their original order.
#[must_use]
pub fn warning_only(errs: &[GoError]) -> Vec<GoError> {
    errs.iter().filter(|err| is_warning(err)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;
    use crate::{ForeignCoder, Scope};

    // Go: errortypes/severity_test.go stubError.
    #[derive(Debug)]
    struct StubError {
        severity: Severity,
    }

    impl fmt::Display for StubError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("anyMessage")
        }
    }

    impl ForeignCoder for StubError {
        fn go_type(&self) -> &'static str {
            "*errortypes.stubError"
        }

        fn code(&self) -> i64 {
            42
        }

        fn severity(&self) -> Severity {
            self.severity
        }

        fn scope(&self) -> Scope {
            Scope::Any
        }
    }

    fn stub(severity: Severity) -> GoError {
        GoError::foreign(StubError { severity })
    }

    // Go: errortypes/severity_test.go TestContainsFatalError.
    #[test]
    fn contains_fatal_error_cases() {
        let fatal = stub(Severity::Fatal);
        let not_fatal = stub(Severity::Warning);
        let unknown = GoError::generic("anyError");
        let cases: [(&str, Vec<GoError>, bool); 5] = [
            ("None", vec![], false),
            ("One - Fatal", vec![fatal.clone()], true),
            ("One - Not Fatal", vec![not_fatal.clone()], false),
            (
                "One - Unknown Severity Same As Fatal",
                vec![unknown.clone()],
                true,
            ),
            ("Mixed", vec![fatal, not_fatal, unknown], true),
        ];
        for (description, errors, expected) in cases {
            assert_eq!(contains_fatal_error(&errors), expected, "{description}");
        }
    }

    // Go: errortypes/severity_test.go TestFatalOnly.
    #[test]
    fn fatal_only_cases() {
        let fatal = stub(Severity::Fatal);
        let not_fatal = stub(Severity::Warning);
        let unknown = GoError::generic("anyError");
        let cases: [(&str, Vec<GoError>, Vec<GoError>); 5] = [
            ("None", vec![], vec![]),
            ("One - Fatal", vec![fatal.clone()], vec![fatal.clone()]),
            ("One - Not Fatal", vec![not_fatal.clone()], vec![]),
            (
                "One - Unknown Severity Same As Fatal",
                vec![unknown.clone()],
                vec![unknown.clone()],
            ),
            (
                "Mixed",
                vec![fatal.clone(), not_fatal, unknown.clone()],
                vec![fatal, unknown],
            ),
        ];
        for (description, errs, expected) in cases {
            assert_eq!(fatal_only(&errs), expected, "{description}");
        }
    }

    // Go: errortypes/severity_test.go TestWarningOnly.
    #[test]
    fn warning_only_cases() {
        let warning = stub(Severity::Warning);
        let not_warning = stub(Severity::Fatal);
        let unknown = GoError::generic("anyError");
        let cases: [(&str, Vec<GoError>, Vec<GoError>); 5] = [
            ("None", vec![], vec![]),
            (
                "One - Warning",
                vec![warning.clone()],
                vec![warning.clone()],
            ),
            ("One - Not Warning", vec![not_warning.clone()], vec![]),
            (
                "One - Unknown Severity Not Warning",
                vec![unknown.clone()],
                vec![],
            ),
            (
                "One - Mixed",
                vec![warning.clone(), not_warning, unknown],
                vec![warning],
            ),
        ];
        for (description, errs, expected) in cases {
            assert_eq!(warning_only(&errs), expected, "{description}");
        }
    }

    #[test]
    fn unknown_severity_coder_is_neither_fatal_nor_warning() {
        let err = stub(Severity::Unknown);
        assert!(!is_fatal(&err), "unknown severity is not fatal");
        assert!(!is_warning(&err), "unknown severity is not a warning");
    }

    #[test]
    fn severity_int_values() {
        assert_eq!(
            [
                Severity::Unknown.as_int(),
                Severity::Fatal.as_int(),
                Severity::Warning.as_int()
            ],
            [0, 1, 2],
            "Go iota values"
        );
    }
}
