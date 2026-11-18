//! The Go error value and its kinds.
//!
//! Go: `errortypes/errortypes.go`. Go code passes errors around as the `error` interface and
//! classifies them with type assertions (`err.(errortypes.Coder)`, `err.(*errortypes.Warning)`).
//! [`GoError`] is the Rust stand-in for such an interface value: an [`ErrorKind`] naming the Go
//! dynamic type plus the message returned by `Error()`.
//!
//! - Every type in `errortypes/errortypes.go` has a kind with the same name.
//! - [`ErrorKind::Generic`] stands for any Go error that does not implement `errortypes.Coder`:
//!   `errors.New`, `fmt.Errorf` (with or without `%w`; Go uses plain type assertions, never
//!   `errors.As`, so wrapping never exposes a code), `strconv.NumError`, JSON errors, and so on.
//!   Such errors read as [`crate::UNKNOWN_ERROR_CODE`] and are fatal.
//! - [`ErrorKind::Foreign`] holds a Go type defined outside `errortypes` that implements `Coder`
//!   (for example `hookexecution.RejectError`). The value is kept so the owning crate can recover
//!   it with [`GoError::downcast_ref`].

use std::{any::Any, error::Error as StdError, fmt, sync::Arc};

use crate::{
    ACCOUNT_DISABLED_ERROR_CODE, ACCT_REQUIRED_ERROR_CODE, BAD_INPUT_ERROR_CODE,
    BAD_SERVER_RESPONSE_ERROR_CODE, BIDDER_TEMPORARILY_DISABLED_ERROR_CODE,
    BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE, BLOCKED_APP_ERROR_CODE, FAILED_TO_MARSHAL_ERROR_CODE,
    FAILED_TO_REQUEST_BIDS_ERROR_CODE, FAILED_TO_UNMARSHAL_ERROR_CODE,
    INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE, MALFORMED_ACCT_ERROR_CODE, Scope, Severity,
    TIMEOUT_ERROR_CODE, TMAX_TIMEOUT_ERROR_CODE,
};

/// A Go error type defined outside the `errortypes` package that implements `errortypes.Coder`.
///
/// `Display` must produce exactly the Go `Error()` text.
pub trait ForeignCoder: fmt::Display + fmt::Debug + Send + Sync + Any {
    /// The Go dynamic type as `%T` prints it, e.g. `*hookexecution.RejectError`.
    fn go_type(&self) -> &'static str;
    /// Go: `Coder.Code()`.
    fn code(&self) -> i64;
    /// Go: `Coder.Severity()`.
    fn severity(&self) -> Severity;
    /// Go: `Scoped.Scope()`. Types that do not implement `Scoped` return [`Scope::Any`].
    fn scope(&self) -> Scope;
}

/// A shared [`ForeignCoder`] value inside [`ErrorKind::Foreign`].
///
/// Two values are equal when they are the same allocation or when their Go type, code,
/// severity, scope and message all match (the analogue of testify's `assert.Equal`).
#[derive(Debug, Clone)]
pub struct Foreign(Arc<dyn ForeignCoder>);

impl Foreign {
    /// The wrapped value.
    #[must_use]
    pub fn get(&self) -> &dyn ForeignCoder {
        &*self.0
    }
}

impl PartialEq for Foreign {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.go_type() == other.0.go_type()
                && self.0.code() == other.0.code()
                && self.0.severity() == other.0.severity()
                && self.0.scope() == other.0.scope()
                && self.0.to_string() == other.0.to_string())
    }
}

impl Eq for Foreign {}

/// The Go dynamic type of a [`GoError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Go: `*errortypes.Timeout` (code 1, fatal).
    Timeout,
    /// Go: `*errortypes.TmaxTimeout` (code 12, fatal).
    TmaxTimeout,
    /// Go: `*errortypes.BadInput` (code 2, fatal).
    BadInput,
    /// Go: `*errortypes.BlockedApp` (code 3, fatal).
    BlockedApp,
    /// Go: `*errortypes.AccountDisabled` (code 7, fatal).
    AccountDisabled,
    /// Go: `*errortypes.AcctRequired` (code 8, fatal).
    AcctRequired,
    /// Go: `*errortypes.BadServerResponse` (code 4, fatal).
    BadServerResponse,
    /// Go: `*errortypes.FailedToRequestBids` (code 5, fatal).
    FailedToRequestBids,
    /// Go: `*errortypes.BidderTemporarilyDisabled` (code 6, warning).
    BidderTemporarilyDisabled,
    /// Go: `*errortypes.BidderThrottled` (code 16, warning).
    BidderThrottled,
    /// Go: `*errortypes.MalformedAcct` (code 10, fatal).
    MalformedAcct,
    /// Go: `*errortypes.Warning` (code `WarningCode`, warning).
    Warning {
        /// Go: `Warning.WarningCode`.
        code: i64,
    },
    /// Go: `*errortypes.FailedToUnmarshal` (code 14, fatal).
    FailedToUnmarshal,
    /// Go: `*errortypes.FailedToMarshal` (code 13, fatal).
    FailedToMarshal,
    /// Go: `*errortypes.DebugWarning` (code `WarningCode`, warning, debug scope).
    DebugWarning {
        /// Go: `DebugWarning.WarningCode`.
        code: i64,
    },
    /// Go: `*errortypes.InvalidImpFirstPartyData` (code 15, fatal).
    InvalidImpFirstPartyData,
    /// Any Go error that does not implement `errortypes.Coder` (`errors.New`, `fmt.Errorf`, ...).
    /// Reads as code 999, is fatal, and has scope any.
    Generic,
    /// A Go type outside `errortypes` that implements `errortypes.Coder`.
    Foreign(Foreign),
}

impl ErrorKind {
    /// Go: the `Code()` and `Severity()` methods, or `None` when the type is not a `Coder`.
    #[must_use]
    pub fn coder(&self) -> Option<(i64, Severity)> {
        let fatal = |code| Some((code, Severity::Fatal));
        match self {
            Self::Timeout => fatal(TIMEOUT_ERROR_CODE),
            Self::TmaxTimeout => fatal(TMAX_TIMEOUT_ERROR_CODE),
            Self::BadInput => fatal(BAD_INPUT_ERROR_CODE),
            Self::BlockedApp => fatal(BLOCKED_APP_ERROR_CODE),
            Self::AccountDisabled => fatal(ACCOUNT_DISABLED_ERROR_CODE),
            Self::AcctRequired => fatal(ACCT_REQUIRED_ERROR_CODE),
            Self::BadServerResponse => fatal(BAD_SERVER_RESPONSE_ERROR_CODE),
            Self::FailedToRequestBids => fatal(FAILED_TO_REQUEST_BIDS_ERROR_CODE),
            Self::BidderTemporarilyDisabled => {
                Some((BIDDER_TEMPORARILY_DISABLED_ERROR_CODE, Severity::Warning))
            }
            Self::BidderThrottled => {
                Some((BIDDER_TEMPORARILY_THROTTLED_ERROR_CODE, Severity::Warning))
            }
            Self::MalformedAcct => fatal(MALFORMED_ACCT_ERROR_CODE),
            Self::Warning { code } | Self::DebugWarning { code } => {
                Some((*code, Severity::Warning))
            }
            Self::FailedToUnmarshal => fatal(FAILED_TO_UNMARSHAL_ERROR_CODE),
            Self::FailedToMarshal => fatal(FAILED_TO_MARSHAL_ERROR_CODE),
            Self::InvalidImpFirstPartyData => fatal(INVALID_IMP_FIRST_PARTY_DATA_ERROR_CODE),
            Self::Generic => None,
            Self::Foreign(foreign) => Some((foreign.0.code(), foreign.0.severity())),
        }
    }

    /// Go: `errortypes.ReadScope` applied to a value of this type.
    #[must_use]
    pub fn scope(&self) -> Scope {
        match self {
            Self::DebugWarning { .. } => Scope::Debug,
            Self::Foreign(foreign) => foreign.0.scope(),
            Self::Timeout
            | Self::TmaxTimeout
            | Self::BadInput
            | Self::BlockedApp
            | Self::AccountDisabled
            | Self::AcctRequired
            | Self::BadServerResponse
            | Self::FailedToRequestBids
            | Self::BidderTemporarilyDisabled
            | Self::BidderThrottled
            | Self::MalformedAcct
            | Self::Warning { .. }
            | Self::FailedToUnmarshal
            | Self::FailedToMarshal
            | Self::InvalidImpFirstPartyData
            | Self::Generic => Scope::Any,
        }
    }

    /// The Go dynamic type as `%T` prints it. [`ErrorKind::Generic`] reports
    /// `*errors.errorString`, the type of `errors.New` and of `fmt.Errorf` without `%w`.
    #[must_use]
    pub fn go_type(&self) -> &'static str {
        match self {
            Self::Timeout => "*errortypes.Timeout",
            Self::TmaxTimeout => "*errortypes.TmaxTimeout",
            Self::BadInput => "*errortypes.BadInput",
            Self::BlockedApp => "*errortypes.BlockedApp",
            Self::AccountDisabled => "*errortypes.AccountDisabled",
            Self::AcctRequired => "*errortypes.AcctRequired",
            Self::BadServerResponse => "*errortypes.BadServerResponse",
            Self::FailedToRequestBids => "*errortypes.FailedToRequestBids",
            Self::BidderTemporarilyDisabled => "*errortypes.BidderTemporarilyDisabled",
            Self::BidderThrottled => "*errortypes.BidderThrottled",
            Self::MalformedAcct => "*errortypes.MalformedAcct",
            Self::Warning { .. } => "*errortypes.Warning",
            Self::FailedToUnmarshal => "*errortypes.FailedToUnmarshal",
            Self::FailedToMarshal => "*errortypes.FailedToMarshal",
            Self::DebugWarning { .. } => "*errortypes.DebugWarning",
            Self::InvalidImpFirstPartyData => "*errortypes.InvalidImpFirstPartyData",
            Self::Generic => "*errors.errorString",
            Self::Foreign(foreign) => foreign.0.go_type(),
        }
    }
}

/// A Go `error` value: its dynamic type ([`ErrorKind`]) and its `Error()` text.
///
/// `Display` prints the message only, exactly like Go's `Error()` methods in `errortypes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoError {
    kind: ErrorKind,
    message: String,
}

impl GoError {
    /// Builds an error of `kind` whose `Error()` text is `message`.
    ///
    /// For [`ErrorKind::Foreign`] prefer [`GoError::foreign`], which takes the message from the
    /// value's `Display`.
    #[must_use]
    pub fn new<M: Into<String>>(kind: ErrorKind, message: M) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Go: `&errortypes.Timeout{Message: message}`.
    #[must_use]
    pub fn timeout<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    /// Go: `&errortypes.TmaxTimeout{Message: message}`.
    #[must_use]
    pub fn tmax_timeout<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::TmaxTimeout, message)
    }

    /// Go: `&errortypes.BadInput{Message: message}`.
    #[must_use]
    pub fn bad_input<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::BadInput, message)
    }

    /// Go: `&errortypes.BlockedApp{Message: message}`.
    #[must_use]
    pub fn blocked_app<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::BlockedApp, message)
    }

    /// Go: `&errortypes.AccountDisabled{Message: message}`.
    #[must_use]
    pub fn account_disabled<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::AccountDisabled, message)
    }

    /// Go: `&errortypes.AcctRequired{Message: message}`.
    #[must_use]
    pub fn acct_required<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::AcctRequired, message)
    }

    /// Go: `&errortypes.BadServerResponse{Message: message}`.
    #[must_use]
    pub fn bad_server_response<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::BadServerResponse, message)
    }

    /// Go: `&errortypes.FailedToRequestBids{Message: message}`.
    #[must_use]
    pub fn failed_to_request_bids<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::FailedToRequestBids, message)
    }

    /// Go: `&errortypes.BidderTemporarilyDisabled{Message: message}`.
    #[must_use]
    pub fn bidder_temporarily_disabled<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::BidderTemporarilyDisabled, message)
    }

    /// Go: `&errortypes.BidderThrottled{Message: message}`.
    #[must_use]
    pub fn bidder_throttled<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::BidderThrottled, message)
    }

    /// Go: `&errortypes.MalformedAcct{Message: message}`.
    #[must_use]
    pub fn malformed_acct<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::MalformedAcct, message)
    }

    /// Go: `&errortypes.Warning{Message: message, WarningCode: code}`.
    #[must_use]
    pub fn warning<M: Into<String>>(code: i64, message: M) -> Self {
        Self::new(ErrorKind::Warning { code }, message)
    }

    /// Go: `&errortypes.FailedToUnmarshal{Message: message}`.
    #[must_use]
    pub fn failed_to_unmarshal<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::FailedToUnmarshal, message)
    }

    /// Go: `&errortypes.FailedToMarshal{Message: message}`.
    #[must_use]
    pub fn failed_to_marshal<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::FailedToMarshal, message)
    }

    /// Go: `&errortypes.DebugWarning{Message: message, WarningCode: code}`.
    #[must_use]
    pub fn debug_warning<M: Into<String>>(code: i64, message: M) -> Self {
        Self::new(ErrorKind::DebugWarning { code }, message)
    }

    /// Go: `&errortypes.InvalidImpFirstPartyData{Message: message}`.
    #[must_use]
    pub fn invalid_imp_first_party_data<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::InvalidImpFirstPartyData, message)
    }

    /// Go: `errors.New(message)`, `fmt.Errorf(...)`, or any other error that is not a `Coder`.
    #[must_use]
    pub fn generic<M: Into<String>>(message: M) -> Self {
        Self::new(ErrorKind::Generic, message)
    }

    /// Wraps a Go `Coder` type defined outside `errortypes`. The message is `value.to_string()`.
    #[must_use]
    pub fn foreign<T: ForeignCoder>(value: T) -> Self {
        let message = value.to_string();
        Self {
            kind: ErrorKind::Foreign(Foreign(Arc::new(value))),
            message,
        }
    }

    /// The Go dynamic type.
    #[must_use]
    pub const fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Go: `Error()`.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Consumes the error and returns its `Error()` text.
    #[must_use]
    pub fn into_message(self) -> String {
        self.message
    }

    /// Go: `err.(errortypes.Coder)` followed by `Code()` and `Severity()`; `None` when the
    /// type assertion fails.
    #[must_use]
    pub fn coder(&self) -> Option<(i64, Severity)> {
        self.kind.coder()
    }

    /// Go: `errortypes.ReadCode(err)`.
    #[must_use]
    pub fn code(&self) -> i64 {
        crate::read_code(self)
    }

    /// Go: `errortypes.ReadScope(err)`.
    #[must_use]
    pub fn scope(&self) -> Scope {
        self.kind.scope()
    }

    /// Go: `errortypes.isFatal(err)`.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        crate::is_fatal(self)
    }

    /// Go: `errortypes.IsWarning(err)`.
    #[must_use]
    pub fn is_warning(&self) -> bool {
        crate::is_warning(self)
    }

    /// Go: `err.(T)` for a foreign `Coder` type `T` wrapped with [`GoError::foreign`].
    #[must_use]
    pub fn downcast_ref<T: ForeignCoder>(&self) -> Option<&T> {
        match &self.kind {
            ErrorKind::Foreign(foreign) => {
                let any: &dyn Any = &*foreign.0;
                any.downcast_ref::<T>()
            }
            ErrorKind::Timeout
            | ErrorKind::TmaxTimeout
            | ErrorKind::BadInput
            | ErrorKind::BlockedApp
            | ErrorKind::AccountDisabled
            | ErrorKind::AcctRequired
            | ErrorKind::BadServerResponse
            | ErrorKind::FailedToRequestBids
            | ErrorKind::BidderTemporarilyDisabled
            | ErrorKind::BidderThrottled
            | ErrorKind::MalformedAcct
            | ErrorKind::Warning { .. }
            | ErrorKind::FailedToUnmarshal
            | ErrorKind::FailedToMarshal
            | ErrorKind::DebugWarning { .. }
            | ErrorKind::InvalidImpFirstPartyData
            | ErrorKind::Generic => None,
        }
    }
}

impl fmt::Display for GoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for GoError {}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;
    use crate::{UNKNOWN_ERROR_CODE, contains_fatal_error, testutil::ensure_eq};

    #[derive(Debug, PartialEq, Eq)]
    struct RejectLike {
        nbr: i64,
    }

    impl fmt::Display for RejectLike {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "rejected with code {}", self.nbr)
        }
    }

    impl ForeignCoder for RejectLike {
        fn go_type(&self) -> &'static str {
            "hookexecution.RejectError"
        }

        fn code(&self) -> i64 {
            crate::MODULE_REJECTION_ERROR_CODE
        }

        fn severity(&self) -> Severity {
            Severity::Warning
        }

        fn scope(&self) -> Scope {
            Scope::Any
        }
    }

    // Each Go type: (constructor result, code, severity, scope, %T). The expected values come
    // from testdata/go/errortypes/golden.txt, produced by the real Go package.
    fn every_kind() -> Vec<GoError> {
        vec![
            GoError::timeout("m"),
            GoError::tmax_timeout("m"),
            GoError::bad_input("m"),
            GoError::blocked_app("m"),
            GoError::account_disabled("m"),
            GoError::acct_required("m"),
            GoError::bad_server_response("m"),
            GoError::failed_to_request_bids("m"),
            GoError::bidder_temporarily_disabled("m"),
            GoError::bidder_throttled("m"),
            GoError::malformed_acct("m"),
            GoError::warning(10005, "m"),
            GoError::failed_to_unmarshal("m"),
            GoError::failed_to_marshal("m"),
            GoError::debug_warning(10003, "m"),
            GoError::invalid_imp_first_party_data("m"),
            GoError::generic("m"),
        ]
    }

    #[test]
    fn kinds_match_go_golden() -> Result<(), String> {
        let golden = include_str!("../testdata/go/errortypes/golden.txt");
        let mut lines = golden.lines();
        for err in every_kind() {
            let line = lines.next().ok_or("golden file too short")?;
            let got = format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                err.kind().go_type(),
                err.code(),
                err.coder().map_or(-1, |(_, severity)| severity.as_int()),
                err.scope().as_int(),
                i64::from(err.is_fatal()),
                i64::from(err.is_warning()),
                err
            );
            ensure_eq(got.as_str(), line, "golden line")?;
        }
        ensure_eq(&lines.next(), &None, "golden file has extra lines")
    }

    #[test]
    fn display_is_message_only() {
        let err = GoError::bad_input("Unexpected status code: 400.");
        assert_eq!(err.to_string(), "Unexpected status code: 400.", "display");
        assert_eq!(err.message(), "Unexpected status code: 400.", "message");
        assert_eq!(
            err.into_message(),
            "Unexpected status code: 400.",
            "into_message"
        );
    }

    #[test]
    fn foreign_coder_round_trip() {
        let err = GoError::foreign(RejectLike { nbr: 7 });
        assert_eq!(err.code(), crate::MODULE_REJECTION_ERROR_CODE, "code");
        assert!(err.is_warning(), "warning severity");
        assert!(!contains_fatal_error(slice::from_ref(&err)), "not fatal");
        assert_eq!(
            err.message(),
            "rejected with code 7",
            "message from Display"
        );
        assert_eq!(err.kind().go_type(), "hookexecution.RejectError", "go type");
        assert_eq!(
            err.downcast_ref::<RejectLike>(),
            Some(&RejectLike { nbr: 7 }),
            "downcast"
        );
        assert_eq!(
            err,
            GoError::foreign(RejectLike { nbr: 7 }),
            "structural equality"
        );
        assert_ne!(
            err,
            GoError::foreign(RejectLike { nbr: 8 }),
            "different values"
        );
        assert_eq!(
            GoError::bad_input("x").downcast_ref::<RejectLike>(),
            None,
            "not foreign"
        );
    }

    #[test]
    fn generic_is_not_a_coder() {
        let err = GoError::generic("x");
        assert_eq!(err.coder(), None, "no coder");
        assert_eq!(err.code(), UNKNOWN_ERROR_CODE, "code 999");
        assert!(err.is_fatal(), "fatal");
    }

    #[test]
    fn equality_compares_kind_and_message() {
        assert_eq!(GoError::warning(1, "a"), GoError::warning(1, "a"), "same");
        assert_ne!(
            GoError::warning(1, "a"),
            GoError::warning(2, "a"),
            "code differs"
        );
        assert_ne!(
            GoError::warning(1, "a"),
            GoError::debug_warning(1, "a"),
            "type differs"
        );
        assert_ne!(
            GoError::bad_input("a"),
            GoError::bad_input("b"),
            "message differs"
        );
    }
}
