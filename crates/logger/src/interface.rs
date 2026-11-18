//! Go: `logger/interface.go`.

use pbs_gocompat::fmt::Value;

/// Go: `logger.Logger`: leveled logging with Go format strings.
///
/// Each method formats `msg` with `args` as Go's `fmt.Sprintf` does. The methods are
/// `#[track_caller]`, so an implementation can report where it was called from, as
/// [`GlogLogger`](crate::GlogLogger) does.
pub trait Logger: Send + Sync {
    /// Go: `Debugf`.
    #[track_caller]
    fn debugf(&self, msg: &str, args: &[Value]);

    /// Go: `Infof`.
    #[track_caller]
    fn infof(&self, msg: &str, args: &[Value]);

    /// Go: `Warnf`.
    #[track_caller]
    fn warnf(&self, msg: &str, args: &[Value]);

    /// Go: `Errorf`.
    #[track_caller]
    fn errorf(&self, msg: &str, args: &[Value]);

    /// Go: `Fatalf`: logs, then, in the default logger, ends the process (see
    /// [`set_exit_fn`](crate::exit::set_exit_fn)); a test logger may return.
    #[track_caller]
    fn fatalf(&self, msg: &str, args: &[Value]);
}
