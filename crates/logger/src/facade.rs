//! Go: `logger/logger.go`: the package-level logger and its functions.

use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use pbs_gocompat::fmt::Value;

use crate::{
    glog::{Severity, Site},
    glog_logger::GlogLogger,
    interface::Logger,
};

/// Go: the package variable `logger` when it is not the default `NewGlogLogger()`.
static INSTALLED: RwLock<Option<Arc<dyn Logger>>> = RwLock::new(None);

/// Go: `var logger Logger = NewGlogLogger()`.
static DEFAULT: LazyLock<GlogLogger> = LazyLock::new(GlogLogger::new);

/// Replaces the package logger (Go tests assign the package variable `logger`).
pub fn set_logger(logger: Arc<dyn Logger>) {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = Some(logger);
}

/// Restores the default glog logger (Go: `logger = NewGlogLogger()`).
pub fn reset_logger() {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether the package logger is the default glog logger.
#[must_use]
pub fn uses_default_logger() -> bool {
    INSTALLED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .is_none()
}

/// The installed logger, if it is not the default.
fn installed() -> Option<Arc<dyn Logger>> {
    INSTALLED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone)
}

/// The site Go's default logger reports for the package function whose body is at `line` of
/// `logger/logger.go`.
const fn go_site(line: u32) -> Site<'static> {
    Site {
        file: "logger.go",
        line,
    }
}

/// Go: `logger.Debugf`, logged at glog's INFO severity.
pub fn debugf(msg: &str, args: &[Value]) {
    match installed() {
        Some(logger) => logger.debugf(msg, args),
        None => DEFAULT.log_at(go_site(7), Severity::Info, msg, args),
    }
}

/// Go: `logger.Infof`.
pub fn infof(msg: &str, args: &[Value]) {
    match installed() {
        Some(logger) => logger.infof(msg, args),
        None => DEFAULT.log_at(go_site(12), Severity::Info, msg, args),
    }
}

/// Go: `logger.Warnf`.
pub fn warnf(msg: &str, args: &[Value]) {
    match installed() {
        Some(logger) => logger.warnf(msg, args),
        None => DEFAULT.log_at(go_site(17), Severity::Warning, msg, args),
    }
}

/// Go: `logger.Errorf`.
pub fn errorf(msg: &str, args: &[Value]) {
    match installed() {
        Some(logger) => logger.errorf(msg, args),
        None => DEFAULT.log_at(go_site(22), Severity::Error, msg, args),
    }
}

/// Go: `logger.Fatalf`: logs with a stack trace, flushes and ends the process through the
/// [`ExitFn`](crate::exit::ExitFn); returns only when a replacement logger returns.
pub fn fatalf(msg: &str, args: &[Value]) {
    match installed() {
        Some(logger) => logger.fatalf(msg, args),
        None => DEFAULT.log_at(go_site(27), Severity::Fatal, msg, args),
    }
}
