//! What a fatal log does once it is written and flushed: end the process.
//!
//! Go's glog sends the process `SIGABRT`, which Go's runtime answers by printing every
//! goroutine's stack and exiting with status 2; chasex's logger calls `os.Exit` directly. A
//! library cannot end the process portably for its host, so the ending is a function the host
//! installs: the default aborts the process (it dies of `SIGABRT`, without Go's goroutine dump
//! and with the signal's status rather than 2), and a server installs [`std::process::exit`] to
//! exit with the Go status.

use std::{
    process,
    sync::{PoisonError, RwLock},
};

/// A function that ends the process with an exit status (Go: `os.Exit`).
pub type ExitFn = fn(i32) -> !;

/// Aborts the process, ignoring the status (the default [`ExitFn`]).
fn abort(_status: i32) -> ! {
    process::abort()
}

static EXIT_FN: RwLock<ExitFn> = RwLock::new(abort);

/// Installs the function that ends the process after a fatal log, called with Go's exit status,
/// 2 (glog's fatal logs, and chasex's failure to open a rotated file).
pub fn set_exit_fn(exit_fn: ExitFn) {
    *EXIT_FN.write().unwrap_or_else(PoisonError::into_inner) = exit_fn;
}

/// Ends the process with the installed [`ExitFn`].
pub(crate) fn exit(status: i32) -> ! {
    let exit_fn = *EXIT_FN.read().unwrap_or_else(PoisonError::into_inner);
    exit_fn(status)
}
