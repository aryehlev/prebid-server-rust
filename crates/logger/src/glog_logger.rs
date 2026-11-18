//! Go: `logger/glog.go`.

use std::sync::Arc;

use pbs_gocompat::fmt::Value;

use crate::{
    glog::{self, Glog, Severity, Site},
    interface::Logger,
};

/// Go: `logger.GlogLogger`: a [`Logger`] over glog, with Debugf at glog's INFO severity.
///
/// Go's `depth` counts the frames glog skips to find the call site it prints; the default, 1,
/// names the caller of the logger's method. Rust can only see that immediate caller, so every
/// depth reports it.
#[derive(Clone, Debug)]
pub struct GlogLogger {
    depth: i32,
    glog: Arc<Glog>,
}

impl GlogLogger {
    /// Go: `logger.NewGlogLogger()`: depth 1, writing through the process-wide glog.
    #[must_use]
    pub fn new() -> Self {
        Self::with_depth(1)
    }

    /// Go: `&GlogLogger{depth: depth}`.
    #[must_use]
    pub fn with_depth(depth: i32) -> Self {
        Self {
            depth,
            glog: Arc::clone(glog::global()),
        }
    }

    /// A logger writing through `glog` instead of the process-wide one.
    #[must_use]
    pub const fn with_glog(glog: Arc<Glog>) -> Self {
        Self { depth: 1, glog }
    }

    /// Go: the `depth` field.
    #[must_use]
    pub const fn depth(&self) -> i32 {
        self.depth
    }

    /// Logs at `site`, ending the process for [`Severity::Fatal`] (Go: `glog.XDepthf`).
    pub(crate) fn log_at(&self, site: Site<'_>, severity: Severity, msg: &str, args: &[Value]) {
        self.glog.logf(site, severity, msg, args);
    }
}

impl Default for GlogLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl Logger for GlogLogger {
    #[track_caller]
    fn debugf(&self, msg: &str, args: &[Value]) {
        self.log_at(Site::caller(), Severity::Info, msg, args);
    }

    #[track_caller]
    fn infof(&self, msg: &str, args: &[Value]) {
        self.log_at(Site::caller(), Severity::Info, msg, args);
    }

    #[track_caller]
    fn warnf(&self, msg: &str, args: &[Value]) {
        self.log_at(Site::caller(), Severity::Warning, msg, args);
    }

    #[track_caller]
    fn errorf(&self, msg: &str, args: &[Value]) {
        self.log_at(Site::caller(), Severity::Error, msg, args);
    }

    #[track_caller]
    fn fatalf(&self, msg: &str, args: &[Value]) {
        self.glog.fatalf(Site::caller(), msg, args);
    }
}
