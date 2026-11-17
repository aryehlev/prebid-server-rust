//! Prebid Server Go `logger` package with `glog`-compatible output.
//!
//! Go: `logger/` (the [`Logger`] interface, the package functions [`debugf`], [`infof`],
//! [`warnf`], [`errorf`] and [`fatalf`], and the default [`GlogLogger`]), with ports of the two
//! logging libraries Prebid's Go code uses: [`glog`] (`github.com/golang/glog` v1.2.5, which
//! writes every server log line) and [`chasex`] (`github.com/chasex/glog`, the file logger of
//! the filesystem analytics module).
//!
//! Messages are Go format strings over [`Value`] operands, formatted by Go's `fmt` rules, and
//! lines are glog's byte for byte:
//!
//! ```text
//! I1117 10:23:45.123456   12345 logger.go:12] Loaded 348 bidders
//! ```
//!
//! The package functions report the call site Go reports for them, `logger.go` at the line of
//! the Go function's body, because Go's default logger skips exactly one frame: every Go
//! Prebid log line names `logger.go:7` (Debugf), `:12` (Infof), `:17` (Warnf), `:22` (Errorf)
//! or `:27` (Fatalf), and so does this port. Calls made directly on a [`Logger`], and glog's
//! own functions, report the Rust file and line of their caller.
//!
//! Start-up mirrors Go's `main`: parse glog's command-line flags with
//! [`glog::parse_command_line`] (Go: `flag.Parse`) and route the `log` crate through glog with
//! [`glog::install_log_bridge`]. A fatal log ends the process with Go's status 2 through
//! [`std::process::exit`] unless [`exit::set_exit_fn`] installs another way to end it.
//!
//! ```
//! use pbs_logger::{Logger as _, Value, glog};
//!
//! // Go: `prebid-server -logtostderr`.
//! glog::parse_command_line(&["-logtostderr".to_owned()]).map_err(|e| e.to_string())?;
//! let logger = pbs_logger::GlogLogger::new();
//! logger.infof("%d bidders, %s", &[Value::from(348), Value::from("ok")]);
//! pbs_logger::warnf("stored request %s not found", &[Value::from("abc")]);
//! glog::flush();
//! # Ok::<(), String>(())
//! ```

pub mod chasex;
pub mod exit;
mod facade;
pub mod glog;
mod glog_logger;
pub mod goerr;
mod interface;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub mod testutil;

pub use facade::{
    debugf, errorf, fatalf, infof, reset_logger, set_logger, uses_default_logger, warnf,
};
pub use glog_logger::GlogLogger;
pub use interface::Logger;
pub use pbs_gocompat::fmt::Value;
