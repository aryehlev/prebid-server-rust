//! Error scopes.
//!
//! Go: `errortypes/scope.go`. Only `errortypes.DebugWarning` (and foreign types that implement
//! `errortypes.Scoped`) report [`Scope::Debug`]; every other error is [`Scope::Any`].

use crate::GoError;

/// Go: `errortypes.Scope` (an `int`; the discriminants are the Go values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Go: `errortypes.ScopeAny` (0).
    Any = 0,
    /// Go: `errortypes.ScopeDebug` (1).
    Debug = 1,
}

impl Scope {
    /// The Go integer value of the scope.
    #[must_use]
    pub const fn as_int(self) -> i64 {
        self as i64
    }
}

/// Go: `errortypes.ReadScope`.
#[must_use]
pub fn read_scope(err: &GoError) -> Scope {
    err.scope()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::INVALID_PRIVACY_CONSENT_WARNING_CODE;

    // Go: errortypes/scope_test.go TestReadScope.
    #[test]
    fn read_scope_cases() {
        let cases = [
            (
                "scope-debug",
                GoError::debug_warning(0, "scope is debug"),
                Scope::Debug,
            ),
            ("scope-any", GoError::warning(0, "scope is any"), Scope::Any),
            (
                "default-error",
                GoError::generic("default error"),
                Scope::Any,
            ),
            (
                "warning-with-code",
                GoError::warning(INVALID_PRIVACY_CONSENT_WARNING_CODE, "w"),
                Scope::Any,
            ),
        ];
        for (name, err, want) in cases {
            assert_eq!(read_scope(&err), want, "{name}");
        }
    }

    #[test]
    fn scope_int_values() {
        assert_eq!(
            [Scope::Any.as_int(), Scope::Debug.as_int()],
            [0, 1],
            "Go iota values"
        );
    }
}
