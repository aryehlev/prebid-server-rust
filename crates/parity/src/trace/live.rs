//! The recording session of parity mode (Go: `parity/trace/session.go`, `decide.go`,
//! `settle.go`).

use core::{
    future::Future,
    pin::pin,
    str,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError, RwLock},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pbs_errortypes::GoError;
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::{
    sync::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard, Notify},
    time::timeout,
};

use super::errors::error_records_of;
use crate::{
    contracts::{IMPL_RUST, SCHEMA_TRACE, SCHEMA_VERSION},
    envelope::Envelope,
    fakes,
    ids::{
        DECISION_SOURCE_ENVELOPE, DECISION_SOURCE_RNG, DecisionDomain, DecisionId, StageId,
        StageStatus, UNKEYED_STAGE,
    },
    tracedoc::{AnalyticsRecord, BuildInfo, Decision, OutboundCall, StageEntry, Trace},
};

/// The process-wide exec lock: one case at a time (Go: `trace.execLock`).
static EXEC_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
/// Whether a session is published; the fast path of [`current`].
static PUBLISHED: AtomicBool = AtomicBool::new(false);
/// The published session (Go: `trace.current`).
static CURRENT: RwLock<Option<Session>> = RwLock::new(None);

/// The in-flight session, or `None` when no parity case is running (Go: `trace.Current`).
#[must_use]
#[inline]
pub fn current() -> Option<Session> {
    if !PUBLISHED.load(Ordering::Acquire) {
        return None;
    }
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Publishes `session` (or unpublishes with `None`).
fn publish(session: Option<Session>) {
    let mut slot = CURRENT.write().unwrap_or_else(PoisonError::into_inner);
    PUBLISHED.store(session.is_some(), Ordering::Release);
    *slot = session;
}

/// Starts a parity case, waiting until no other case is in flight (Go: `trace.Begin`).
///
/// The case's session is published until the returned [`Case`] is ended or dropped. When the
/// fakes are installed, the envelope's fake values apply for the case (and the defaults again
/// afterwards). Dropping the future before it completes gives up waiting, like Go's
/// `BeginContext` with a done context.
pub async fn begin_case(env: Envelope) -> Case {
    let lock = EXEC_LOCK.lock().await;
    let session = Session::new(env);
    if let Some(src) = fakes::active() {
        src.set(session.0.env.fakes.as_ref());
    }
    publish(Some(session.clone()));
    Case {
        session,
        _lock: lock,
    }
}

/// One running parity case (Go: the `end` function `trace.Begin` returns). Ending or dropping it
/// unpublishes the session, resets the fakes to their defaults and releases the exec lock.
#[derive(Debug)]
#[must_use = "dropping the case ends it"]
pub struct Case {
    session: Session,
    _lock: AsyncMutexGuard<'static, ()>,
}

impl Case {
    /// The case's session.
    #[must_use]
    pub const fn session(&self) -> &Session {
        &self.session
    }

    /// Ends the case (the same as dropping it).
    pub fn end(self) {
        drop(self);
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        // Go: the end function: unpublish, reset the fakes, then release the exec lock (the
        // guard field drops after this).
        publish(None);
        if let Some(src) = fakes::active() {
            src.set(None);
        }
    }
}

/// Everything recorded for one parity case (Go: `trace.Session`). Cloning is cheap and shares
/// the recording.
#[derive(Clone, Debug)]
pub struct Session(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    env: Envelope,
    /// The captured stages as a bit set, or `None` for every stage.
    filter: Option<u64>,
    state: Mutex<State>,
    /// The tasks registered with `track` that have not finished.
    pending: AtomicUsize,
    settled: Notify,
}

#[derive(Debug, Default)]
struct State {
    seq: isize,
    stages: BTreeMap<StageId, BTreeMap<String, StageEntry>>,
    stage_status: BTreeMap<StageId, StageStatus>,
    decisions: Vec<Decision>,
    http_out: Vec<OutboundCall>,
    analytics: Vec<AnalyticsRecord>,
    notes: Vec<String>,
}

/// Registers one task of the case that can still change observable state after the handler
/// has written its response (Go: the function `Session.Track` returns). Dropping it marks the
/// task finished.
#[derive(Debug)]
#[must_use = "dropping the guard marks the task finished"]
pub struct Tracked(Arc<Inner>);

impl Drop for Tracked {
    fn drop(&mut self) {
        if self.0.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.settled.notify_waiters();
        }
    }
}

impl Session {
    /// A session for `env` that is not published (Go: `newSession`): what [`begin_case`]
    /// publishes, and what tests and tools record into directly. An invalid `capture.stages`
    /// captures every stage and is noted.
    #[must_use]
    pub fn new(env: Envelope) -> Self {
        let mut notes = Vec::new();
        let filter = match env.capture.stage_filter() {
            Ok(None) => None,
            Ok(Some(ids)) => Some(
                StageId::ALL
                    .into_iter()
                    .filter(|stage| ids.contains(stage.as_str()))
                    .fold(0, |set, stage| set | stage.bit()),
            ),
            Err(err) => {
                notes.push(format!("capture.stages ignored: {err}"));
                None
            }
        };
        Self(Arc::new(Inner {
            env,
            filter,
            state: Mutex::new(State {
                notes,
                ..State::default()
            }),
            pending: AtomicUsize::new(0),
            settled: Notify::new(),
        }))
    }

    /// The case envelope (Go: `Session.Envelope`).
    #[must_use]
    pub fn envelope(&self) -> &Envelope {
        &self.0.env
    }

    /// The case ID (Go: `Session.CaseID`).
    #[must_use]
    pub fn case_id(&self) -> &str {
        &self.0.env.case_id
    }

    /// The next value of the session-wide sequence shared by stage entries and outbound calls
    /// (Go: `Session.NextSeq`).
    #[must_use]
    pub fn next_seq(&self) -> isize {
        self.with_state(|rec| {
            rec.seq += 1;
            rec.seq
        })
    }

    /// Whether `stage` is captured under `capture.stages` (Go: `Session.Wants`).
    #[must_use]
    pub fn wants(&self, stage: StageId) -> bool {
        self.0.filter.is_none_or(|set| set & stage.bit() != 0)
    }

    /// Records `value` under (`stage`, `key`) as the bytes `jsonutil.Marshal` writes
    /// ([`pbs_jsonutil::marshal`]), so the raw form keeps the implementation's lexical form (Go:
    /// `Session.Record`). An empty key means [`UNKEYED_STAGE`]. Recording the same (stage, key)
    /// again replaces the value and counts the write. A marshal error is noted on the entry
    /// instead.
    pub fn record<T: Serialize + ?Sized>(&self, stage: StageId, key: &str, value: &T) {
        if !self.wants(stage) {
            return;
        }
        match pbs_jsonutil::marshal(value) {
            Ok(raw) => self.record_raw(stage, key, &raw),
            Err(err) => {
                let note = format!("marshal error: {}", err.message());
                self.with_state(|rec| {
                    entry(&mut rec.stages, stage, key)
                        .notes
                        .get_or_insert_with(Vec::new)
                        .push(note);
                });
            }
        }
    }

    /// [`Session::record`] of the value `build` returns, built only when `stage` is captured.
    pub fn record_with<T: Serialize, F: FnOnce() -> T>(&self, stage: StageId, key: &str, build: F) {
        if self.wants(stage) {
            self.record(stage, key, &build());
        }
    }

    /// Records exact bytes under (`stage`, `key`) (Go: `Session.RecordRaw`). Valid JSON also
    /// fills the entry's `json`; anything else is kept only as `raw` / `raw_b64` and noted.
    pub fn record_raw(&self, stage: StageId, key: &str, raw: &[u8]) {
        if !self.wants(stage) {
            return;
        }
        let (text, b64) = str::from_utf8(raw).map_or_else(
            |_| (None, Some(STANDARD.encode(raw))),
            |text| (Some(text.to_owned()), None),
        );
        let json = text
            .as_ref()
            .and_then(|text| RawValue::from_string(text.clone()).ok());
        self.with_state(|rec| {
            rec.seq += 1;
            let e = entry(&mut rec.stages, stage, key);
            e.writes += 1;
            e.seq = rec.seq;
            e.raw = text;
            e.raw_b64 = b64;
            if json.is_none() {
                e.notes
                    .get_or_insert_with(Vec::new)
                    .push("raw value is not valid JSON".to_owned());
            }
            e.json = json;
        });
    }

    /// Attaches `errs` to (`stage`, `key`): warnings (`errortypes.IsWarning`) to `warnings`,
    /// everything else to `errors` (Go: `Session.Errors`). Both lists become `[]` even when
    /// `errs` is empty, which records "no errors" explicitly.
    pub fn errors(&self, stage: StageId, key: &str, errs: &[GoError]) {
        if !self.wants(stage) {
            return;
        }
        let (fatal, warnings) = error_records_of(errs);
        self.with_state(|rec| {
            let e = entry(&mut rec.stages, stage, key);
            if e.seq == 0 {
                rec.seq += 1;
                e.seq = rec.seq;
            }
            e.errors.get_or_insert_with(Vec::new).extend(fatal);
            e.warnings.get_or_insert_with(Vec::new).extend(warnings);
        });
    }

    /// Sets the status of `stage` explicitly, for stages the exec handler fills outside
    /// [`Session::record`] (Go: `Session.Mark`).
    pub fn mark(&self, stage: StageId, status: StageStatus) {
        let _previous = self.with_state(|rec| rec.stage_status.insert(stage, status));
    }

    /// Appends a free-form note to the trace (Go: `Session.Note`).
    pub fn note<N: Into<String>>(&self, note: N) {
        let note = note.into();
        self.with_state(|rec| rec.notes.push(note));
    }

    /// Appends an outbound call recorded by the transport (Go: `Session.AddOutbound`).
    pub fn add_outbound(&self, call: OutboundCall) {
        if !self.wants(StageId::HttpOut) {
            return;
        }
        self.with_state(|rec| {
            rec.http_out.push(call);
            let _previous = rec
                .stage_status
                .insert(StageId::HttpOut, StageStatus::Captured);
        });
    }

    /// Marshals one analytics object with `jsonutil.Marshal` ([`pbs_jsonutil::marshal_raw`]) and
    /// appends it (Go: `Session.AddAnalytics`). A marshal error is noted instead. (Go also notes
    /// an object `jsonutil.Marshal` writes as invalid JSON, a raw message member that does not
    /// compact leaving `"ext":}`; a `RawValue` always holds JSON, so that cannot happen here.)
    pub fn add_analytics<T: Serialize + ?Sized>(&self, kind: &str, object: &T) {
        if !self.wants(StageId::Analytics) || !self.0.env.capture.wants_analytics() {
            return;
        }
        let object = match pbs_jsonutil::marshal_raw(object) {
            Ok(object) => object,
            Err(err) => {
                self.note(format!(
                    "analytics {kind}: marshal error: {}",
                    err.message()
                ));
                return;
            }
        };
        let record = AnalyticsRecord {
            kind: kind.to_owned(),
            object: Some(object),
        };
        self.with_state(|rec| {
            rec.analytics.push(record);
            let _previous = rec
                .stage_status
                .insert(StageId::Analytics, StageStatus::Captured);
        });
    }

    /// The trace recorded so far (Go: `Session.Snapshot`). The exec handler fills `build`,
    /// `http_in`, `http_response` and `metrics_delta`. Every registered stage has a status:
    /// captured when recorded, else an explicit [`Session::mark`], else not reached.
    #[must_use]
    pub fn snapshot(&self) -> Trace {
        let (stages, explicit, http_out, decisions, analytics, notes) = self.with_state(|rec| {
            (
                rec.stages.clone(),
                rec.stage_status.clone(),
                rec.http_out.clone(),
                rec.decisions.clone(),
                rec.analytics.clone(),
                rec.notes.clone(),
            )
        });
        let mut status: BTreeMap<StageId, StageStatus> = StageId::ALL
            .into_iter()
            .map(|stage| (stage, StageStatus::NotReached))
            .collect();
        status.extend(explicit);
        status.extend(stages.keys().map(|k| (*k, StageStatus::Captured)));
        Trace {
            schema: SCHEMA_TRACE.to_owned(),
            schema_version: SCHEMA_VERSION.to_owned(),
            implementation: IMPL_RUST.to_owned(),
            build: BuildInfo::default(),
            case_id: self.0.env.case_id.clone(),
            endpoint: self.0.env.request.path.clone(),
            http_in: None,
            http_response: None,
            stages: Some(
                stages
                    .into_iter()
                    .map(|(stage, keyed)| (stage, Some(keyed)))
                    .collect(),
            ),
            stage_status: Some(status),
            http_out: Some(http_out),
            decisions: Some(decisions),
            metrics_delta: Some(Vec::new()),
            analytics: Some(analytics),
            notes: Some(notes),
        }
    }

    /// The index-valued decision point (Go: `Session.Intn`): the envelope override for `point`
    /// when it is an integer in `[0, n)`, otherwise `def(n)`. The outcome and its source are
    /// recorded.
    pub fn intn<F: FnOnce(isize) -> isize>(&self, point: DecisionId, n: isize, def: F) -> isize {
        if let Some(v) = self.0.env.decision_int(point) {
            if v >= 0 && v < go_int64(n) {
                self.record_decision(point, v.to_string(), DECISION_SOURCE_ENVELOPE, None);
                return go_int(v);
            }
            let got = def(n);
            self.record_decision(
                point,
                got.to_string(),
                DECISION_SOURCE_RNG,
                Some(format!("envelope override {v} outside [0,{n})")),
            );
            return got;
        }
        let got = def(n);
        self.record_decision(point, got.to_string(), DECISION_SOURCE_RNG, None);
        got
    }

    /// The semantic decision point (Go: `Session.Decide`): the envelope's integer override for
    /// `point` (a binary point accepts only 0 and 1), otherwise `def()`. The outcome and its
    /// source are recorded.
    pub fn decide<F: FnOnce() -> isize>(&self, point: DecisionId, def: F) -> isize {
        if let Some(v) = self.0.env.decision_int(point) {
            if point.domain() != DecisionDomain::Binary || v == 0 || v == 1 {
                self.record_decision(point, v.to_string(), DECISION_SOURCE_ENVELOPE, None);
                return go_int(v);
            }
            let got = def();
            self.record_decision(
                point,
                got.to_string(),
                DECISION_SOURCE_RNG,
                Some(format!("envelope override {v} is not 0 or 1")),
            );
            return got;
        }
        let got = def();
        self.record_decision(point, got.to_string(), DECISION_SOURCE_RNG, None);
        got
    }

    /// [`Session::decide`] for a binary point whose Go site computes a boolean
    /// (Go: `Session.DecideBool`): `draw` is the original random computation and `true` is the
    /// decision value 1.
    pub fn decide_bool<F: FnOnce() -> bool>(&self, point: DecisionId, draw: F) -> bool {
        self.decide(point, || isize::from(draw())) == 1
    }

    /// [`Session::decide_bool`] for a binary point whose Go site draws against a configured rate
    /// (Go: `Session.DecideChance`). `uncertain` reports whether `draw` can return both outcomes
    /// for the configured rates. When it cannot (a rate of 0 or 100 fixes the outcome), the
    /// envelope value is ignored, since Go never draws differently there; the fixed outcome is
    /// recorded with source `rng`, plus a note when the envelope pinned the point.
    pub fn decide_chance<F: FnOnce() -> bool>(
        &self,
        point: DecisionId,
        uncertain: bool,
        draw: F,
    ) -> bool {
        if uncertain {
            return self.decide_bool(point, draw);
        }
        let got = isize::from(draw());
        let note =
            self.0.env.decision(point).map(|_| {
                "envelope override ignored: the configured rate fixes the outcome".to_owned()
            });
        self.record_decision(point, got.to_string(), DECISION_SOURCE_RNG, note);
        got == 1
    }

    /// The string-valued decision point (Go: `Session.DecideString`): the envelope override
    /// when it is one of the point's allowed values, otherwise `def()` (an out-of-domain
    /// override is noted and ignored). Points without allowed values accept any string.
    pub fn decide_string<F: FnOnce() -> String>(&self, point: DecisionId, def: F) -> String {
        if let Some(v) = self.0.env.decision_string(point) {
            let allowed = point.enum_values();
            if allowed.is_empty() || allowed.contains(&v.as_str()) {
                self.record_decision(point, json_string(&v), DECISION_SOURCE_ENVELOPE, None);
                return v;
            }
            let got = def();
            self.record_decision(
                point,
                json_string(&got),
                DECISION_SOURCE_RNG,
                Some(format!(
                    "envelope override {} is not an allowed value",
                    json_string(&v)
                )),
            );
            return got;
        }
        let got = def();
        self.record_decision(point, json_string(&got), DECISION_SOURCE_RNG, None);
        got
    }

    /// The value most recently recorded for `point`, or `None` when the case did not reach it
    /// (Go: `Session.LastDecision`).
    #[must_use]
    pub fn last_decision(&self, point: DecisionId) -> Option<Box<RawValue>> {
        self.with_state(|rec| {
            rec.decisions
                .iter()
                .rev()
                .find(|d| d.point == point)
                .and_then(|d| d.value.clone())
        })
    }

    /// Registers one task of the case that can still change observable state after the
    /// handler has written its response (Go: `Session.Track`). The exchange's bidder tasks are
    /// the example: each one hands its result to the auction and only then records its
    /// adapter-request metric, so the metric can move after the response is complete; the exec
    /// handler calls [`Session::settle`] before its closing metrics snapshot. Register before the
    /// task hands its result to the handler and drop the guard when the task finishes.
    #[must_use = "dropping the guard marks the task finished"]
    pub fn track(&self) -> Tracked {
        let _before = self.0.pending.fetch_add(1, Ordering::AcqRel);
        Tracked(Arc::clone(&self.0))
    }

    /// Waits until every task registered with [`Session::track`] has finished, or until
    /// `limit` elapses, and reports whether they all finished (Go: `Session.Settle`). It needs a
    /// Tokio runtime with the time driver.
    pub fn settle(&self, limit: Duration) -> impl Future<Output = bool> + Send + 'static {
        let inner = Arc::clone(&self.0);
        async move {
            let all_finished = async {
                loop {
                    let mut notified = pin!(inner.settled.notified());
                    let _registered = notified.as_mut().enable();
                    if inner.pending.load(Ordering::Acquire) == 0 {
                        return;
                    }
                    notified.await;
                }
            };
            timeout(limit, all_finished).await.is_ok()
        }
    }

    /// Runs `f` on the recorded state, holding the lock only for the call.
    fn with_state<R, F: FnOnce(&mut State) -> R>(&self, f: F) -> R {
        f(&mut self.0.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Appends one consulted decision (Go: `Session.recordDecision`).
    fn record_decision(
        &self,
        point: DecisionId,
        value: String,
        source: &str,
        note: Option<String>,
    ) {
        let decision = Decision {
            point,
            value: RawValue::from_string(value).ok(),
            source: source.to_owned(),
            notes: note.map(|n| vec![n]),
        };
        self.with_state(|rec| rec.decisions.push(decision));
    }
}

/// The entry for (`stage`, `key`), created empty (Go: `Session.entryLocked`).
fn entry<'a>(
    stages: &'a mut BTreeMap<StageId, BTreeMap<String, StageEntry>>,
    stage: StageId,
    key: &str,
) -> &'a mut StageEntry {
    let key = if key.is_empty() { UNKEYED_STAGE } else { key };
    stages
        .entry(stage)
        .or_default()
        .entry(key.to_owned())
        .or_default()
}

/// A string as a JSON value (Go: `trace.quote`, `json.Marshal` of the string, which writes `<`,
/// `>` and `&` as `\u` escapes).
fn json_string(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "\"\"".to_owned())
}

/// Go's `int64(n)` of an `int`.
fn go_int64(n: isize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Go's `int(v)` of an `int64` (both are 64-bit on every supported target).
fn go_int(v: i64) -> isize {
    isize::try_from(v).unwrap_or(isize::MAX)
}
