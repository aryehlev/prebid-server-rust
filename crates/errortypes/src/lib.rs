//! Prebid Server error types, codes and severities matching the Go `errortypes` package.
//!
//! The Go package is flat, so this crate re-exports everything at its root. Each private module
//! mirrors one Go file:
//!
//! - `code` (Go `errortypes/code.go`): every `*ErrorCode` / `*WarningCode` constant with Go's
//!   `iota` values, and [`read_code`].
//! - `errortypes` (Go `errortypes/errortypes.go`): [`GoError`] (a Go `error` interface value),
//!   [`ErrorKind`] (its dynamic type) and [`ForeignCoder`] (Coder types defined elsewhere).
//! - `severity` (Go `errortypes/severity.go`): [`Severity`], [`is_fatal`], [`is_warning`],
//!   [`contains_fatal_error`], [`fatal_only`], [`warning_only`].
//! - `scope` (Go `errortypes/scope.go`): [`Scope`], [`read_scope`].
//! - `aggregate` (Go `errortypes/aggregate.go`): [`AggregateError`].
//!
//! # Using it
//!
//! Wherever Go returns or collects `error` values, Rust uses [`GoError`]:
//!
//! ```
//! use pbs_errortypes::{GoError, contains_fatal_error, read_code};
//!
//! // Go: &errortypes.BadInput{Message: "missing imp"}
//! let bad = GoError::bad_input("missing imp");
//! // Go: fmt.Errorf("unexpected %s", "x") -- not a Coder: code 999, fatal.
//! let plain = GoError::generic(format!("unexpected {}", "x"));
//! // Go: &errortypes.Warning{WarningCode: errortypes.MultiBidWarningCode, Message: "w"}
//! let warn = GoError::warning(pbs_errortypes::MULTI_BID_WARNING_CODE, "w");
//!
//! assert_eq!(read_code(&bad), 2);
//! assert_eq!(read_code(&plain), 999);
//! assert_eq!(warn.to_string(), "w"); // Display == Go Error(): the bare message
//! assert!(contains_fatal_error(&[warn, plain]));
//! ```
//!
//! - `Display` is the bare message, identical to the Go `Error()` methods.
//! - Go classifies errors with plain type assertions, never `errors.As`, so an error wrapped with
//!   `fmt.Errorf("...: %w", err)` loses its code: build it with [`GoError::generic`].
//! - Types outside `errortypes` that implement `errortypes.Coder` (today only
//!   `hookexecution.RejectError`) implement [`ForeignCoder`] in their own crate and are wrapped
//!   with [`GoError::foreign`]; [`GoError::downcast_ref`] recovers them (Go: `err.(*T)`).

mod aggregate;
mod code;
mod errortypes;
mod scope;
mod severity;
#[cfg(test)]
pub mod testutil;

pub use aggregate::AggregateError;
pub use code::{
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
    UNKNOWN_ERROR_CODE, UNKNOWN_WARNING_CODE, read_code,
};
pub use errortypes::{ErrorKind, Foreign, ForeignCoder, GoError};
pub use scope::{Scope, read_scope};
pub use severity::{
    Severity, contains_fatal_error, fatal_only, is_fatal, is_warning, warning_only,
};
