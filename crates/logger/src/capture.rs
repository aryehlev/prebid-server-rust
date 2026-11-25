//! A layer that keeps the lines code logs, for tests: Go's tests swap a mock logger into the
//! logger package (`mockLogger` in `logger/logger_test.go`), Rust's put this layer in a
//! subscriber.

use std::{
    fmt::{self, Write as _},
    sync::{Arc, Mutex, PoisonError},
};

use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    subscriber::with_default,
};
use tracing_subscriber::{
    Layer, Registry,
    layer::{Context, SubscriberExt as _},
};

use crate::Severity;

/// One line a [`Capture`] kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// glog's severity of the line ([`Severity::of`]).
    pub severity: Severity,
    /// The line's target, the module that logged it unless the call named another.
    pub target: String,
    /// The message, formatted.
    pub message: String,
}

/// Keeps every line at INFO and above that reaches it, the levels Prebid Server writes; clones
/// share the lines.
#[derive(Debug, Clone, Default)]
pub struct Capture {
    lines: Arc<Mutex<Vec<Line>>>,
}

impl Capture {
    /// A capture that has kept nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines kept so far, oldest first.
    #[must_use]
    pub fn lines(&self) -> Vec<Line> {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Runs `f` with a subscriber of this layer alone as the thread's default.
    pub fn run<T, F: FnOnce() -> T>(&self, f: F) -> T {
        with_default(Registry::default().with(self.clone()), f)
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        if *metadata.level() > Level::INFO {
            return;
        }
        let mut message = Message::default();
        event.record(&mut message);
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Line {
                severity: Severity::of(metadata),
                target: metadata.target().to_owned(),
                message: message.0,
            });
    }
}

/// Reads an event's message.
#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            value.clone_into(&mut self.0);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0.clear();
            write!(self.0, "{value:?}").unwrap_or_default();
        }
    }
}
