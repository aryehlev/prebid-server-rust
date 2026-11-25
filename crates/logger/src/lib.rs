//! Prebid Server's logger (`logger/`): the facade Prebid's code logs through and the command line
//! that says where the lines go, over `tracing`.
//!
//! Go: `logger.Debugf`, `Infof`, `Warnf`, `Errorf` and `Fatalf` (logger/logger.go) call glog
//! v1.2.5 through `GlogLogger` with depth 1 (logger/glog.go), and `main.go:28`'s `flag.Parse`
//! reads glog's nine flags, and net/http/httptest's `-httptest.serve` when an argument starts with
//! `-httptest.serve=` (not a flag here: `logging.httptest-flag`). Here [`debugf!`], [`infof!`],
//! [`warnf!`] and [`errorf!`] are `tracing` events (`debugf!` at INFO, as `Debugf` calls
//! `glog.InfoDepthf`); [`fatalf!`] builds a [`Fatal`] that `main` returns and reports, which logs
//! it at ERROR and gives exit code 2; [`init_from_glog_flags`] reads glog's flags in Go's syntax
//! with getopts and installs a `tracing-subscriber` that writes the lines where glog would; and
//! [`Capture`] keeps the lines a test logs, as Go's tests swap in a mock logger. What differs from
//! Go is in `crates/logger/differences.toml`.

mod capture;
mod facade;
mod flags;
mod sinks;

pub use capture::{Capture, Line};
pub use facade::{FATAL_TARGET, Fatal, Severity};
pub use flags::{CommandLine, Exit, GlogFlags, ModulePattern, TraceLocation, parse_command_line};
pub use sinks::{LogFileError, Logging, init_from_glog_flags, subscriber};

/// What the facade macros expand to; not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use tracing::{error, info, warn};
}
