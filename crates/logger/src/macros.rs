//! Call-site shorthands for the package functions: Go's variadic `logger.Infof(msg, args...)`
//! as `infof!(msg, args...)`, each argument converted with [`Value::from`](crate::Value), so a
//! port reads like the Go line it replaces. The lines are the package functions' own, Go's
//! `logger.go:N` site included.
//!
//! ```no_run
//! let bidders = 348;
//! let err = "connection refused";
//! pbs_logger::infof!("Loaded %d bidders", bidders);
//! pbs_logger::errorf!("Failed to fetch rates: %v", err);
//! pbs_logger::warnf!("no arguments");
//! ```

/// Go: `logger.Debugf(msg, args...)`: [`debugf`](crate::debugf()) with each argument converted by
/// [`Value::from`](crate::Value).
#[macro_export]
macro_rules! debugf {
    ($msg:expr $(, $arg:expr)* $(,)?) => {
        $crate::debugf($msg, &[$($crate::Value::from($arg)),*])
    };
}

/// Go: `logger.Infof(msg, args...)`: [`infof`](crate::infof()) with each argument converted by
/// [`Value::from`](crate::Value).
#[macro_export]
macro_rules! infof {
    ($msg:expr $(, $arg:expr)* $(,)?) => {
        $crate::infof($msg, &[$($crate::Value::from($arg)),*])
    };
}

/// Go: `logger.Warnf(msg, args...)`: [`warnf`](crate::warnf()) with each argument converted by
/// [`Value::from`](crate::Value).
#[macro_export]
macro_rules! warnf {
    ($msg:expr $(, $arg:expr)* $(,)?) => {
        $crate::warnf($msg, &[$($crate::Value::from($arg)),*])
    };
}

/// Go: `logger.Errorf(msg, args...)`: [`errorf`](crate::errorf()) with each argument converted by
/// [`Value::from`](crate::Value).
#[macro_export]
macro_rules! errorf {
    ($msg:expr $(, $arg:expr)* $(,)?) => {
        $crate::errorf($msg, &[$($crate::Value::from($arg)),*])
    };
}

/// Go: `logger.Fatalf(msg, args...)`: [`fatalf`](crate::fatalf()) with each argument converted by
/// [`Value::from`](crate::Value).
#[macro_export]
macro_rules! fatalf {
    ($msg:expr $(, $arg:expr)* $(,)?) => {
        $crate::fatalf($msg, &[$($crate::Value::from($arg)),*])
    };
}
