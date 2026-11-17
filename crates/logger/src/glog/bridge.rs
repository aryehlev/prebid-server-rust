//! Routes the `log` crate's records through glog, as Go's `glog.CopyStandardLogTo` routes the
//! standard `log` package.

use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use pbs_gocompat::fmt::Value;

use super::{Severity, Site, global};

/// A `log` logger writing each record as a glog line at the record's file and line.
///
/// Errors log at ERROR, warnings at WARNING, and info, debug and trace records at INFO, where
/// Prebid's Go logger writes debug messages.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlogBridge;

static BRIDGE: GlogBridge = GlogBridge;

impl GlogBridge {
    /// The glog severity of a `log` level.
    #[must_use]
    pub const fn severity(level: Level) -> Severity {
        match level {
            Level::Error => Severity::Error,
            Level::Warn => Severity::Warning,
            Level::Info | Level::Debug | Level::Trace => Severity::Info,
        }
    }
}

impl Log for GlogBridge {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    /// Go: `logBridge.Write`: the text at the record's site, `???:1` when it has none.
    fn log(&self, record: &Record<'_>) {
        let site = Site {
            file: record.file().unwrap_or("???"),
            line: record.line().unwrap_or(1),
        };
        let text = record.args().to_string();
        global().logf(
            site,
            Self::severity(record.level()),
            "%s",
            &[Value::String(text)],
        );
    }

    fn flush(&self) {
        global().flush();
    }
}

/// Installs [`GlogBridge`] as the `log` crate's logger, passing records up to `max_level`.
///
/// # Errors
///
/// When a `log` logger is already installed.
pub fn install_log_bridge(max_level: LevelFilter) -> Result<(), SetLoggerError> {
    log::set_logger(&BRIDGE)?;
    log::set_max_level(max_level);
    Ok(())
}
