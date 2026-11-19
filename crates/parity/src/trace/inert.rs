//! The production build of the trace seams: no session can exist, so every hook is dead code.

use core::{convert::Infallible, future::Ready, time::Duration};

use pbs_errortypes::GoError;
use serde::Serialize;
use serde_json::value::RawValue;

use crate::{
    envelope::Envelope,
    ids::{DecisionId, StageId, StageStatus},
    tracedoc::{OutboundCall, Trace},
};

/// No value: a production build never has a session.
#[derive(Debug)]
struct Never(Infallible);

/// Always `None`: a production build never runs a parity case (Go: `trace.Current` without a
/// case in flight).
#[must_use]
#[inline]
pub const fn current() -> Option<Session> {
    None
}

/// The recording session of a parity case (Go: `trace.Session`).
///
/// A production build (without the `trace` feature) cannot construct one, so code that holds a
/// session is unreachable and compiled out; the methods mirror the parity build's.
#[derive(Debug)]
pub struct Session(Never);

/// The guard of a tracked task (Go: the function `Session.Track` returns); never constructed in
/// a production build.
#[derive(Debug)]
#[must_use = "dropping the guard marks the task finished"]
pub struct Tracked(Never);

impl Session {
    /// The case envelope (Go: `Session.Envelope`).
    #[must_use]
    pub const fn envelope(&self) -> &Envelope {
        match self.0.0 {}
    }

    /// The case ID (Go: `Session.CaseID`).
    #[must_use]
    pub const fn case_id(&self) -> &str {
        match self.0.0 {}
    }

    /// The next value of the session-wide sequence (Go: `Session.NextSeq`).
    #[must_use]
    pub const fn next_seq(&self) -> isize {
        match self.0.0 {}
    }

    /// Whether `stage` is captured (Go: `Session.Wants`).
    #[must_use]
    pub const fn wants(&self, _stage: StageId) -> bool {
        match self.0.0 {}
    }

    /// Records `value` under (`stage`, `key`) (Go: `Session.Record`).
    pub const fn record<T: Serialize + ?Sized>(&self, _stage: StageId, _key: &str, _value: &T) {
        match self.0.0 {}
    }

    /// Records the value `build` returns under (`stage`, `key`).
    pub const fn record_with<T: Serialize, F: FnOnce() -> T>(
        &self,
        _stage: StageId,
        _key: &str,
        _build: F,
    ) {
        match self.0.0 {}
    }

    /// Records exact bytes under (`stage`, `key`) (Go: `Session.RecordRaw`).
    pub const fn record_raw(&self, _stage: StageId, _key: &str, _raw: &[u8]) {
        match self.0.0 {}
    }

    /// Attaches errors to (`stage`, `key`) (Go: `Session.Errors`).
    pub const fn errors(&self, _stage: StageId, _key: &str, _errs: &[GoError]) {
        match self.0.0 {}
    }

    /// Sets the status of `stage` (Go: `Session.Mark`).
    pub const fn mark(&self, _stage: StageId, _status: StageStatus) {
        match self.0.0 {}
    }

    /// Appends a note (Go: `Session.Note`).
    pub const fn note<N: Into<String>>(&self, _note: N) {
        match self.0.0 {}
    }

    /// Appends an outbound call (Go: `Session.AddOutbound`).
    pub const fn add_outbound(&self, _call: OutboundCall) {
        match self.0.0 {}
    }

    /// Appends an analytics object (Go: `Session.AddAnalytics`).
    pub const fn add_analytics<T: Serialize + ?Sized>(&self, _kind: &str, _object: &T) {
        match self.0.0 {}
    }

    /// The trace recorded so far (Go: `Session.Snapshot`).
    #[must_use]
    pub const fn snapshot(&self) -> Trace {
        match self.0.0 {}
    }

    /// The index-valued decision point (Go: `Session.Intn`).
    pub const fn intn<F: FnOnce(isize) -> isize>(
        &self,
        _point: DecisionId,
        _n: isize,
        _def: F,
    ) -> isize {
        match self.0.0 {}
    }

    /// The semantic decision point (Go: `Session.Decide`).
    pub const fn decide<F: FnOnce() -> isize>(&self, _point: DecisionId, _def: F) -> isize {
        match self.0.0 {}
    }

    /// The boolean decision point (Go: `Session.DecideBool`).
    pub const fn decide_bool<F: FnOnce() -> bool>(&self, _point: DecisionId, _draw: F) -> bool {
        match self.0.0 {}
    }

    /// The rate-drawn decision point (Go: `Session.DecideChance`).
    pub const fn decide_chance<F: FnOnce() -> bool>(
        &self,
        _point: DecisionId,
        _uncertain: bool,
        _draw: F,
    ) -> bool {
        match self.0.0 {}
    }

    /// The string-valued decision point (Go: `Session.DecideString`).
    pub const fn decide_string<F: FnOnce() -> String>(
        &self,
        _point: DecisionId,
        _def: F,
    ) -> String {
        match self.0.0 {}
    }

    /// The value most recently recorded for `point` (Go: `Session.LastDecision`).
    #[must_use]
    pub const fn last_decision(&self, _point: DecisionId) -> Option<Box<RawValue>> {
        match self.0.0 {}
    }

    /// Registers one task of the case (Go: `Session.Track`).
    #[must_use = "dropping the guard marks the task finished"]
    pub const fn track(&self) -> Tracked {
        match self.0.0 {}
    }

    /// Waits for the tracked tasks (Go: `Session.Settle`).
    pub const fn settle(&self, _limit: Duration) -> Ready<bool> {
        match self.0.0 {}
    }
}
