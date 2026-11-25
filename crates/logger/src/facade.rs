//! The facade (logger/logger.go, logger/glog.go): the macros Prebid's code logs with, glog's
//! severities and the fatal error `main` ends with.

use std::{
    error::Error,
    fmt,
    process::{ExitCode, Termination},
};

use tracing::{Level, Metadata};

/// The target [`Fatal`] is reported with: `ERROR` at this target is glog's FATAL, which
/// `-stderrthreshold=FATAL` still lets through to stderr.
pub const FATAL_TARGET: &str = "pbs_logger::fatal";

/// `logger.Debugf` (logger/logger.go:6-8): an INFO line, as `GlogLogger.Debugf` calls
/// `glog.InfoDepthf` (logger/glog.go:13-15). It takes `tracing::info!`'s arguments.
#[macro_export]
macro_rules! debugf {
    ($($arg:tt)+) => {
        $crate::__private::info!($($arg)+)
    };
}

/// `logger.Infof` (logger/logger.go:11-13): an INFO line. It takes `tracing::info!`'s arguments.
#[macro_export]
macro_rules! infof {
    ($($arg:tt)+) => {
        $crate::__private::info!($($arg)+)
    };
}

/// `logger.Warnf` (logger/logger.go:16-18): a WARN line, glog's WARNING. It takes
/// `tracing::warn!`'s arguments.
#[macro_export]
macro_rules! warnf {
    ($($arg:tt)+) => {
        $crate::__private::warn!($($arg)+)
    };
}

/// `logger.Errorf` (logger/logger.go:21-23): an ERROR line. It takes `tracing::error!`'s
/// arguments.
#[macro_export]
macro_rules! errorf {
    ($($arg:tt)+) => {
        $crate::__private::error!($($arg)+)
    };
}

/// `logger.Fatalf` (logger/logger.go:26-28) without the exit, which only `main` may take: a
/// [`Fatal`](crate::Fatal) with the message `format!` makes of the arguments, for `main` to
/// return and report.
#[macro_export]
macro_rules! fatalf {
    ($($arg:tt)+) => {
        $crate::Fatal::new(::std::format!($($arg)+))
    };
}

/// glog's severities (`logsink.Severity`), lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// INFO (0).
    Info,
    /// WARNING (1).
    Warning,
    /// ERROR (2).
    Error,
    /// FATAL (3).
    Fatal,
}

impl Severity {
    /// Every severity, lowest first.
    pub const ALL: [Self; 4] = [Self::Info, Self::Warning, Self::Error, Self::Fatal];

    /// The severity of a line: its level, with `ERROR` at [`FATAL_TARGET`] as FATAL and `DEBUG`
    /// and `TRACE`, which glog has not, as INFO.
    #[must_use]
    pub fn of(metadata: &Metadata<'_>) -> Self {
        let level = *metadata.level();
        if level == Level::ERROR {
            if metadata.target() == FATAL_TARGET {
                Self::Fatal
            } else {
                Self::Error
            }
        } else if level == Level::WARN {
            Self::Warning
        } else {
            Self::Info
        }
    }

    /// glog's name: `INFO`, `WARNING`, `ERROR` or `FATAL`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }

    /// glog's number, which `-stderrthreshold`'s value prints as.
    #[must_use]
    pub const fn number(self) -> i8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
            Self::Fatal => 3,
        }
    }

    /// The severity with glog's number `n`, if one has it.
    #[must_use]
    pub const fn from_number(n: i8) -> Option<Self> {
        match n {
            0 => Some(Self::Info),
            1 => Some(Self::Warning),
            2 => Some(Self::Error),
            3 => Some(Self::Fatal),
            _ => None,
        }
    }
}

/// What `logger.Fatalf` ends the process with.
///
/// Go logs the message with a stack and exits 2 on the spot; here the error travels up to `main`,
/// whose [`Termination::report`] logs the message at ERROR with [`FATAL_TARGET`] and returns exit
/// code 2 (ledger: `logging.fatal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fatal {
    message: String,
}

impl Fatal {
    /// A fatal error with `message`.
    #[must_use]
    pub fn new<M: Into<String>>(message: M) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    #[must_use]
    pub const fn message(&self) -> &str {
        self.message.as_str()
    }
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for Fatal {}

impl Termination for Fatal {
    /// Logs the message at ERROR with [`FATAL_TARGET`] and returns exit code 2, glog's.
    fn report(self) -> ExitCode {
        tracing::error!(target: FATAL_TARGET, "{}", self.message);
        ExitCode::from(2)
    }
}
