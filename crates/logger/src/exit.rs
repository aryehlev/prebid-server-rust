//! What a fatal log does once it is written and flushed: end the process.
//!
//! Go's glog sends the process `SIGABRT`, which Go's runtime answers by printing every
//! goroutine's stack and exiting with status 2; chasex's logger calls `os.Exit` directly. The
//! default [`ExitFn`] is [`std::process::exit`], so a Rust process ends with Go's status 2
//! without any set-up (without Go's goroutine dump: the fatal line already carries a Rust
//! backtrace). Tests and embedders that must not end the process install their own.

use std::{
    process,
    sync::{PoisonError, RwLock},
};

/// A function that ends the process with an exit status (Go: `os.Exit`).
pub type ExitFn = fn(i32) -> !;

static EXIT_FN: RwLock<ExitFn> = RwLock::new(process::exit);

/// Installs the function that ends the process after a fatal log, called with Go's exit status,
/// 2 (glog's fatal logs, and chasex's failure to open a rotated file). The default is
/// [`std::process::exit`].
pub fn set_exit_fn(exit_fn: ExitFn) {
    *EXIT_FN.write().unwrap_or_else(PoisonError::into_inner) = exit_fn;
}

/// Ends the process with the installed [`ExitFn`].
pub(crate) fn exit(status: i32) -> ! {
    let exit_fn = *EXIT_FN.read().unwrap_or_else(PoisonError::into_inner);
    exit_fn(status)
}
