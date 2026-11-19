use pbs_errortypes::GoError;

use super::{
    current, decide, decide_bool, decide_chance, decide_string, error_record_of, error_records_of,
    intn, nil_error_record, with,
};
use crate::{
    golden::{TestResult, check, check_eq, global_lock_blocking},
    ids::DecisionId,
    tracedoc::{
        ErrorRecord, SCOPE_DEBUG, SCOPE_GENERAL, SEVERITY_FATAL, SEVERITY_UNKNOWN, SEVERITY_WARNING,
    },
};

fn record(
    code: isize,
    kind: &str,
    message: &str,
    scope: &str,
    severity: &str,
    fatal: bool,
) -> ErrorRecord {
    ErrorRecord {
        code,
        kind: kind.to_owned(),
        message: message.to_owned(),
        scope: scope.to_owned(),
        severity: severity.to_owned(),
        fatal,
    }
}

// Go: parity/trace/session_test.go TestErrorRecordOf.
#[test]
fn error_records() -> TestResult {
    let cases = [
        (
            GoError::timeout("t"),
            record(1, "Timeout", "t", SCOPE_GENERAL, SEVERITY_FATAL, true),
        ),
        (
            GoError::bad_input("b"),
            record(2, "BadInput", "b", SCOPE_GENERAL, SEVERITY_FATAL, true),
        ),
        (
            GoError::bad_server_response("s"),
            record(
                4,
                "BadServerResponse",
                "s",
                SCOPE_GENERAL,
                SEVERITY_FATAL,
                true,
            ),
        ),
        (
            GoError::warning(10006, "w"),
            record(
                10006,
                "MultiBidWarning",
                "w",
                SCOPE_GENERAL,
                SEVERITY_WARNING,
                false,
            ),
        ),
        (
            GoError::debug_warning(10003, "d"),
            record(
                10003,
                "BidderLevelDebugDisabledWarning",
                "d",
                SCOPE_DEBUG,
                SEVERITY_WARNING,
                false,
            ),
        ),
        (
            GoError::generic("plain"),
            record(
                999,
                "UnknownError",
                "plain",
                SCOPE_GENERAL,
                SEVERITY_UNKNOWN,
                true,
            ),
        ),
        (
            GoError::warning(42, "odd"),
            record(42, "Unknown", "odd", SCOPE_GENERAL, SEVERITY_WARNING, false),
        ),
    ];
    for (err, want) in &cases {
        check_eq(&error_record_of(err), want, err.message())?;
    }
    check_eq(
        &nil_error_record(),
        &record(
            999,
            "UnknownError",
            "",
            SCOPE_GENERAL,
            SEVERITY_UNKNOWN,
            true,
        ),
        "nil error",
    )?;
    let errs: Vec<GoError> = cases.iter().map(|(err, _)| err.clone()).collect();
    let (fatal, warnings) = error_records_of(&errs);
    check_eq(&fatal.len(), &4, "fatal or unknown")?;
    check_eq(&warnings.len(), &3, "warnings")?;
    check_eq(
        &fatal.iter().map(|r| r.code).collect::<Vec<_>>(),
        &vec![1, 2, 4, 999],
        "input order",
    )?;
    let (no_fatal, no_warnings) = error_records_of(&[]);
    check(no_fatal.is_empty() && no_warnings.is_empty(), "empty")
}

// Go: parity/trace/session_test.go TestNilSessionIsNoOp: outside a case every seam is the
// production default (and without the `trace` feature there is never a case).
#[test]
fn no_case_means_production_defaults() -> TestResult {
    let _global = global_lock_blocking();
    check(current().is_none(), "no session")?;
    let mut called = false;
    check_eq(&with(|_| called = true), &None, "with")?;
    check(!called, "with does not run the hook")?;
    check_eq(&decide(DecisionId::FloorsSkip, || 1), &1, "decide")?;
    check_eq(
        &decide_bool(DecisionId::FloorsEnforce, || true),
        &true,
        "decide_bool",
    )?;
    check_eq(
        &decide_chance(DecisionId::FloorsSkip, false, || true),
        &true,
        "decide_chance",
    )?;
    check_eq(
        &decide_string(DecisionId::UsersyncShuffle, || "rng".to_owned()),
        &"rng".to_owned(),
        "decide_string",
    )?;
    check_eq(
        &intn(DecisionId::FloorsModelGroup, 5, |n| n - 2),
        &3,
        "intn passes n",
    )
}

// Without the feature no session can exist: `Session` and `Tracked` are uninhabited, so an
// `Option<Session>` carries nothing, a hook's body is dead code, and `current` and
// `fakes::active` are constants.
#[cfg(not(feature = "trace"))]
#[test]
fn production_build_has_no_recorder() -> TestResult {
    use super::{Session, Tracked};
    use crate::fakes;

    const NO_CASE: bool = current().is_none();
    const NO_FAKES: bool = fakes::active().is_none();
    check(NO_CASE, "current is a constant None")?;
    check(NO_FAKES, "fakes::active is a constant None")?;
    check_eq(&size_of::<Session>(), &0, "Session")?;
    check_eq(&size_of::<Option<Session>>(), &0, "Option<Session>")?;
    check_eq(&size_of::<Tracked>(), &0, "Tracked")?;
    check_eq(&size_of::<Option<Tracked>>(), &0, "Option<Tracked>")
}

#[cfg(feature = "trace")]
mod live {
    use core::time::Duration;
    use std::{collections::BTreeMap, sync::Arc, thread};

    use pbs_errortypes::GoError;
    use serde::{Serialize, Serializer, ser::Error as _};
    use serde_json::value::RawValue;
    use tokio::{
        sync::{Notify, Semaphore, mpsc},
        task::{self, JoinHandle},
        time::{Instant, sleep, timeout},
    };

    use super::super::{Case, Session, Tracked, begin_case, current, decide, intn, with};
    use crate::{
        envelope::{CaptureSpec, Envelope, ExecRequest, FakeValues},
        fakes::{self, DEFAULT_INT63},
        golden::{Regs, RegsRecord, TestResult, check, check_eq, global_lock},
        ids::{DecisionId, StageId, StageStatus},
        tracedoc::{OutboundCall, StageEntry, Trace},
    };

    /// `text` as a raw JSON value.
    fn raw(text: &str) -> Result<Box<RawValue>, String> {
        RawValue::from_string(text.to_owned()).map_err(|e| format!("{text}: {e}"))
    }

    fn env(case_id: &str) -> Envelope {
        Envelope {
            case_id: case_id.to_owned(),
            request: ExecRequest {
                path: "/openrtb2/auction".to_owned(),
                ..ExecRequest::default()
            },
            ..Envelope::default()
        }
    }

    fn stage_entry(trace: &Trace, stage: StageId, key: &str) -> Result<StageEntry, String> {
        trace
            .stages
            .as_ref()
            .and_then(|stages| stages.get(&stage))
            .and_then(|keyed| keyed.as_ref()?.get(key))
            .cloned()
            .ok_or_else(|| format!("no entry {}[{key}]", stage.as_str()))
    }

    fn status(trace: &Trace, stage: StageId) -> Option<StageStatus> {
        trace
            .stage_status
            .as_ref()
            .and_then(|s| s.get(&stage))
            .copied()
    }

    /// Uninstalls the fakes when dropped.
    struct Installed;

    impl Drop for Installed {
        fn drop(&mut self) {
            fakes::uninstall();
        }
    }

    /// Runs `checks` while `case` is published, then ends the case.
    fn end_after<F: FnOnce(&Case) -> TestResult>(case: Case, checks: F) -> TestResult {
        let checked = checks(&case);
        case.end();
        checked
    }

    // Go: TestBeginPublishesAndEndReleases.
    #[tokio::test]
    async fn begin_publishes_and_end_releases() -> TestResult {
        let _global = global_lock().await;
        let _installed = Installed;
        let src = fakes::install();
        let mut e = env("case-1");
        e.fakes = Some(FakeValues {
            int63: 77,
            uuid: "case-seed".to_owned(),
            ..FakeValues::default()
        });
        end_after(begin_case(e).await, |case| {
            check_eq(
                &current().map(|s| s.case_id().to_owned()),
                &Some("case-1".to_owned()),
                "published",
            )?;
            check_eq(&case.session().case_id(), &"case-1", "session")?;
            check_eq(&fakes::int(), &77, "envelope fakes apply for the case")?;
            check_eq(
                &fakes::new_v4().ok(),
                &Some(fakes::sequence_uuid("case-seed", 1)),
                "the stream restarts from the case seed",
            )?;
            check_eq(&with(|s| s.case_id().len()), &Some(6), "with runs the hook")
        })?;
        check(current().is_none(), "unpublished")?;
        check_eq(
            &src.values().int63,
            &DEFAULT_INT63,
            "fakes reset after the case",
        )?;
        check_eq(
            &fakes::new_v4().ok(),
            &Some(fakes::sequence_uuid(fakes::DEFAULT_UUID, 1)),
            "the stream restarts at the end of the case",
        )?;
        let again = timeout(Duration::from_secs(1), begin_case(env("case-2")))
            .await
            .map_err(|elapsed| format!("the exec lock must be free again: {elapsed}"))?;
        check_eq(
            &current().map(|s| s.case_id().to_owned()),
            &Some("case-2".to_owned()),
            "second case",
        )?;
        drop(again);
        check(current().is_none(), "dropping the case ends it")
    }

    /// While `holder` runs a second case waits and the holder stays published; a case queued
    /// behind it is returned, to run once the holder ends.
    async fn hold_then_end(holder: Case) -> Result<JoinHandle<Option<String>>, String> {
        let waited = timeout(Duration::from_millis(20), begin_case(env("waiter")))
            .await
            .is_err();
        let published = current().map(|s| s.case_id().to_owned());
        let next = tokio::spawn(async {
            let case = begin_case(env("next")).await;
            let id = current().map(|s| s.case_id().to_owned());
            drop(case);
            id
        });
        task::yield_now().await;
        holder.end();
        check(waited, "a second case waits for the first")?;
        check_eq(
            &published,
            &Some("holder".to_owned()),
            "the holder stays published",
        )?;
        Ok(next)
    }

    // Go: TestBeginContextGivesUpWhileLocked.
    #[tokio::test]
    async fn begin_waits_for_the_running_case() -> TestResult {
        let _global = global_lock().await;
        let next = hold_then_end(begin_case(env("holder")).await).await?;
        let id = timeout(Duration::from_secs(5), next)
            .await
            .map_err(|elapsed| format!("the waiter runs once the holder ends: {elapsed}"))?
            .map_err(|e| e.to_string())?;
        check_eq(&id, &Some("next".to_owned()), "the waiter's session")
    }

    /// The `json` member of `entry`, as text.
    fn json_text(entry: &StageEntry) -> Option<&str> {
        entry.json.as_deref().map(RawValue::get)
    }

    // Go: TestRecordKeepsLexicalForm and TestRecordOverwriteCountsWrites. The raw form is what
    // `pbs_jsonutil::marshal` writes, which leaves `<`, `>` and `&` as they are where Go's
    // `jsonutil.Marshal` writes `\u003c`, `\u003e` and `\u0026` (json.html-escape).
    #[test]
    fn record_keeps_the_lexical_form() -> TestResult {
        let s = Session::new(env("lex"));
        let adm = BTreeMap::from([("adm".to_owned(), "<b>&".to_owned())]);
        s.record(StageId::RequestValidated, "", &adm);
        s.record_raw(StageId::HttpResponse, "", b"{\"a\":1.0}\n");
        s.record_raw(StageId::AdapterBids, "appnexus#0", &[0xFF]);
        s.record(StageId::BidderRequest, "appnexus", &1_isize);
        s.record(StageId::BidderRequest, "appnexus", &2_isize);
        let snap = s.snapshot();

        let validated = stage_entry(&snap, StageId::RequestValidated, "_")?;
        check_eq(
            &validated.raw.as_deref(),
            &Some(r#"{"adm":"<b>&"}"#),
            "the bytes jsonutil writes",
        )?;
        check_eq(&json_text(&validated), &Some(r#"{"adm":"<b>&"}"#), "json")?;
        check_eq(&validated.writes, &1, "one write")?;
        let response = stage_entry(&snap, StageId::HttpResponse, "_")?;
        check_eq(
            &response.raw.as_deref(),
            &Some("{\"a\":1.0}\n"),
            "raw bytes are exact",
        )?;
        check_eq(
            &json_text(&response),
            &Some("{\"a\":1.0}"),
            "json without the whitespace around it",
        )?;
        let bids = stage_entry(&snap, StageId::AdapterBids, "appnexus#0")?;
        check_eq(&bids.raw, &None, "no raw for invalid UTF-8")?;
        check_eq(&bids.raw_b64.as_deref(), &Some("/w=="), "raw_b64")?;
        check(bids.json.is_none(), "no json")?;
        check_eq(
            &bids.notes,
            &Some(vec!["raw value is not valid JSON".to_owned()]),
            "invalid JSON is noted",
        )?;
        let request = stage_entry(&snap, StageId::BidderRequest, "appnexus")?;
        check_eq(&request.writes, &2, "overwrite counts writes")?;
        check_eq(&json_text(&request), &Some("2"), "last write wins")?;
        check_eq(&request.seq, &5, "seq of the last write")?;
        check_eq(&s.next_seq(), &6, "shared sequence")
    }

    // Go: TestConcurrentRecord.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_recording() -> TestResult {
        let s = Session::new(env("race"));
        let mut tasks = Vec::new();
        for i in 0..100_isize {
            let s = s.clone();
            tasks.push(tokio::spawn(async move {
                let bidder = format!("bidder{i:03}");
                s.record(
                    StageId::BidderRequest,
                    &bidder,
                    &BTreeMap::from([("i".to_owned(), i)]),
                );
                s.errors(StageId::BidderResult, &bidder, &[GoError::generic("e")]);
                s.add_outbound(OutboundCall {
                    seq: s.next_seq(),
                    bidder,
                    ..OutboundCall::default()
                });
                let _decided = s.decide(DecisionId::CategoryDedupe, || i % 2);
                s.note(format!("n{i}"));
            }));
        }
        for task in tasks {
            task.await.map_err(|e| e.to_string())?;
        }
        let snap = s.snapshot();
        let stages = snap.stages.unwrap_or_default();
        let keyed = |stage: StageId| stages.get(&stage).cloned().flatten().unwrap_or_default();
        let requests = keyed(StageId::BidderRequest);
        let results = keyed(StageId::BidderResult);
        check_eq(&requests.len(), &100, "bidder.request entries")?;
        check_eq(&results.len(), &100, "bidder.result entries")?;
        let calls = snap.http_out.unwrap_or_default();
        check_eq(&calls.len(), &100, "http_out")?;
        check_eq(&snap.decisions.map(|d| d.len()), &Some(100), "decisions")?;
        check_eq(&snap.notes.map(|n| n.len()), &Some(100), "notes")?;
        // Every write took the next value of the one session-wide sequence: the entries'
        // and the calls' seqs are exactly 1..=300, none shared.
        let mut seqs: Vec<isize> = requests
            .values()
            .chain(results.values())
            .map(|e| e.seq)
            .chain(calls.iter().map(|c| c.seq))
            .collect();
        seqs.sort_unstable();
        check_eq(&seqs, &(1..=300).collect::<Vec<isize>>(), "one sequence")?;
        check_eq(&s.next_seq(), &301, "the sequence continues")
    }

    // Go: TestCaptureFilter.
    #[test]
    fn capture_filter() -> TestResult {
        let mut e = env("filter");
        e.capture = CaptureSpec {
            stages: Some(raw(r#"["http.in"]"#)?),
            analytics: Some(false),
            ..CaptureSpec::default()
        };
        let s = Session::new(e);
        s.record(StageId::HttpIn, "", &1_isize);
        s.record(StageId::BidderRequest, "a", &1_isize);
        let mut built = false;
        s.record_with(StageId::BidderRequest, "a", || {
            built = true;
            1_isize
        });
        check(!built, "record_with does not build an uncaptured value")?;
        s.add_outbound(OutboundCall::default());
        s.add_analytics("auction", &1_isize);
        check(
            s.wants(StageId::HttpIn) && !s.wants(StageId::HttpOut),
            "wants",
        )?;
        let snap = s.snapshot();
        check(stage_entry(&snap, StageId::HttpIn, "_").is_ok(), "http.in")?;
        check(
            stage_entry(&snap, StageId::BidderRequest, "a").is_err(),
            "filtered",
        )?;
        check_eq(
            &snap.http_out.as_ref().map(Vec::len),
            &Some(0),
            "no http_out",
        )?;
        check_eq(
            &snap.analytics.as_ref().map(Vec::len),
            &Some(0),
            "no analytics",
        )?;
        check_eq(
            &status(&snap, StageId::BidderRequest),
            &Some(StageStatus::NotReached),
            "not reached",
        )
    }

    /// A value whose `Serialize` fails, as a map with keys `serde_json` cannot write does.
    struct Refuses;

    impl Serialize for Refuses {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("refused"))
        }
    }

    // Go: TestAddAnalyticsKeepsTraceEncodable. Go also refuses an object whose raw member does
    // not compact (`"ext":}`); a `RawValue` always holds JSON, so the refusal here is a marshal
    // error, noted the same way.
    #[test]
    fn analytics_keep_the_trace_encodable() -> TestResult {
        let valid = RegsRecord {
            regs: Some(Regs {
                gdpr: 1,
                ext: Some(raw("{ }")?),
            }),
        };
        let recording = Session::new(env("analytics"));
        recording.add_analytics("auction", &valid);
        let recorded = recording.snapshot();
        check_eq(
            &recorded.analytics.as_ref().map(|a| {
                a.iter()
                    .map(|r| r.object.as_deref().map(RawValue::get))
                    .collect::<Vec<_>>()
            }),
            &Some(vec![Some(r#"{"regs":{"gdpr":1,"ext":{}}}"#)]),
            "recorded as jsonutil writes it, the raw member compacted",
        )?;
        check_eq(
            &status(&recorded, StageId::Analytics),
            &Some(StageStatus::Captured),
            "captured",
        )?;

        let refusing = Session::new(env("analytics"));
        refusing.add_analytics("auction", &Refuses);
        refusing.record(StageId::BidderRequest, "a", &Refuses);
        let refused = refusing.snapshot();
        let _encoded = serde_json::to_vec(&refused).map_err(|e| e.to_string())?;
        check_eq(
            &refused.analytics.as_ref().map(Vec::len),
            &Some(0),
            "not recorded",
        )?;
        check_eq(
            &refused.notes,
            &Some(vec!["analytics auction: marshal error: refused".to_owned()]),
            "noted",
        )?;
        check_eq(
            &stage_entry(&refused, StageId::BidderRequest, "a")?.notes,
            &Some(vec!["marshal error: refused".to_owned()]),
            "a stage value that does not marshal is noted on its entry",
        )
    }

    // Go: TestSnapshotStageStatus and TestInvalidCaptureFilterIsNoted.
    #[test]
    fn snapshot_reports_every_stage() -> TestResult {
        let s = Session::new(env("status"));
        s.record(StageId::HttpIn, "", &1_isize);
        s.mark(StageId::MetricsDelta, StageStatus::Captured);
        s.mark(StageId::VideoCache, StageStatus::Unimplemented);
        let snap = s.snapshot();
        for stage in StageId::ALL {
            check(status(&snap, stage).is_some(), stage.as_str())?;
        }
        check_eq(
            &status(&snap, StageId::HttpIn),
            &Some(StageStatus::Captured),
            "recorded",
        )?;
        check_eq(
            &status(&snap, StageId::MetricsDelta),
            &Some(StageStatus::Captured),
            "marked",
        )?;
        check_eq(
            &status(&snap, StageId::VideoCache),
            &Some(StageStatus::Unimplemented),
            "unimplemented",
        )?;
        check_eq(
            &status(&snap, StageId::HttpResponse),
            &Some(StageStatus::NotReached),
            "not reached",
        )?;
        check_eq(&snap.endpoint.as_str(), &"/openrtb2/auction", "endpoint")?;
        check_eq(&snap.implementation.as_str(), &"rust", "impl")?;

        let mut e = env("bad-filter");
        e.capture.stages = Some(raw(r#""some""#)?);
        let unfiltered = Session::new(e);
        check(
            unfiltered.wants(StageId::HttpIn),
            "an invalid filter captures everything",
        )?;
        let notes = unfiltered.snapshot().notes.unwrap_or_default();
        check(
            notes.len() == 1
                && notes
                    .iter()
                    .all(|n| n.starts_with("capture.stages ignored: ")),
            "noted",
        )
    }

    // Go: TestErrorsSplitsWarnings.
    #[test]
    fn errors_split_warnings() -> TestResult {
        let s = Session::new(env("errs"));
        s.errors(
            StageId::RequestValidated,
            "",
            &[GoError::bad_input("b"), GoError::warning(10001, "w")],
        );
        s.errors(StageId::BidderResult, "x", &[]);
        let snap = s.snapshot();
        let validated = stage_entry(&snap, StageId::RequestValidated, "_")?;
        check_eq(
            &validated
                .errors
                .map(|r| r.iter().map(|x| x.code).collect::<Vec<_>>()),
            &Some(vec![2]),
            "errors",
        )?;
        check_eq(
            &validated
                .warnings
                .map(|r| r.iter().map(|x| x.code).collect::<Vec<_>>()),
            &Some(vec![10001]),
            "warnings",
        )?;
        let empty = stage_entry(&snap, StageId::BidderResult, "x")?;
        check_eq(&empty.errors, &Some(Vec::new()), "recorded empty is []")?;
        check_eq(&empty.warnings, &Some(Vec::new()), "warnings []")?;
        check_eq(&empty.seq, &2, "errors take a seq")?;
        let unrecorded =
            serde_json::to_string(&StageEntry::default()).map_err(|e| e.to_string())?;
        check(
            unrecorded.contains(r#""errors":null"#),
            "unrecorded is null",
        )
    }

    fn decisions_env(pairs: &[(&str, &str)]) -> Result<Envelope, String> {
        let mut e = env("decide");
        e.decisions = Some(
            pairs
                .iter()
                .map(|(k, v)| Ok(((*k).to_owned(), raw(v)?)))
                .collect::<Result<_, String>>()?,
        );
        Ok(e)
    }

    fn last(s: &Session) -> Result<(String, String, Vec<String>), String> {
        let decisions = s.snapshot().decisions.unwrap_or_default();
        let d = decisions.last().ok_or("no decision")?;
        Ok((
            d.value.as_deref().map_or_default(RawValue::get).to_owned(),
            d.source.clone(),
            d.notes.clone().unwrap_or_default(),
        ))
    }

    fn rng(value: &str) -> (String, String, Vec<String>) {
        (value.to_owned(), "rng".to_owned(), Vec::new())
    }

    // Go: TestDecisions (the integer, boolean and chance points).
    #[test]
    fn decisions() -> TestResult {
        let s = Session::new(decisions_env(&[
            ("floors.skip", "1"),
            ("floors.data_rate", "2"),
            ("floors.modelgroup", "2"),
            ("floors.enforce", "0"),
            ("category.dedupe", r#""1""#),
        ])?);
        check_eq(
            &s.decide(DecisionId::FloorsSkip, || 0),
            &1,
            "binary override",
        )?;
        check_eq(
            &last(&s)?,
            &("1".to_owned(), "envelope".to_owned(), Vec::new()),
            "recorded",
        )?;
        check_eq(
            &s.decide(DecisionId::FloorsDataRate, || 0),
            &0,
            "not 0 or 1",
        )?;
        check_eq(
            &last(&s)?,
            &(
                "0".to_owned(),
                "rng".to_owned(),
                vec!["envelope override 2 is not 0 or 1".to_owned()],
            ),
            "noted",
        )?;
        check_eq(
            &s.intn(DecisionId::FloorsModelGroup, 3, |_| 0),
            &2,
            "index override",
        )?;
        check_eq(
            &s.intn(DecisionId::FloorsModelGroup, 2, |n| n - 1),
            &1,
            "outside [0,n)",
        )?;
        check_eq(
            &last(&s)?.2,
            &vec!["envelope override 2 outside [0,2)".to_owned()],
            "noted",
        )?;
        check(
            !s.decide_bool(DecisionId::FloorsEnforce, || true),
            "decide_bool honours 0",
        )?;
        check(
            !s.decide_chance(DecisionId::FloorsSkip, false, || false),
            "a fixed outcome ignores the envelope",
        )?;
        check_eq(
            &last(&s)?,
            &(
                "0".to_owned(),
                "rng".to_owned(),
                vec!["envelope override ignored: the configured rate fixes the outcome".to_owned()],
            ),
            "ignored",
        )?;
        check_eq(
            &s.decide(DecisionId::CategoryDedupe, || 1),
            &1,
            "a string is no integer override",
        )?;
        check_eq(&last(&s)?, &rng("1"), "rng")?;
        check_eq(
            &s.decide(DecisionId::PriceGranularityTimesplit, || 0),
            &0,
            "absent",
        )?;
        check_eq(
            &s.last_decision(DecisionId::FloorsModelGroup)
                .map(|v| v.get().to_owned()),
            &Some("1".to_owned()),
            "last decision",
        )?;
        check_eq(
            &s.last_decision(DecisionId::FloorsEnforce)
                .map(|v| v.get().to_owned()),
            &Some("0".to_owned()),
            "decide_bool records 0",
        )
    }

    // Go: TestDecisions (the enum point).
    #[test]
    fn string_decisions() -> TestResult {
        let allowed = Session::new(decisions_env(&[("usersync.shuffle", r#""identity""#)])?);
        check_eq(
            &allowed.decide_string(DecisionId::UsersyncShuffle, || "rng".to_owned()),
            &"identity".to_owned(),
            "enum override",
        )?;
        check_eq(
            &last(&allowed)?,
            &(
                r#""identity""#.to_owned(),
                "envelope".to_owned(),
                Vec::new(),
            ),
            "quoted value",
        )?;

        let s = Session::new(decisions_env(&[("usersync.shuffle", r#""reverse""#)])?);
        check_eq(
            &s.decide_string(DecisionId::UsersyncShuffle, || "rng".to_owned()),
            &"rng".to_owned(),
            "out of domain",
        )?;
        check_eq(
            &last(&s)?,
            &(
                r#""rng""#.to_owned(),
                "rng".to_owned(),
                vec![r#"envelope override "reverse" is not an allowed value"#.to_owned()],
            ),
            "noted",
        )?;
        check(
            s.last_decision(DecisionId::FloorsSkip).is_none(),
            "not reached",
        )
    }

    // The free functions consult the published session.
    #[tokio::test]
    async fn free_functions_use_the_published_session() -> TestResult {
        let _global = global_lock().await;
        let case = begin_case(decisions_env(&[
            ("floors.skip", "1"),
            ("floors.modelgroup", "4"),
        ])?)
        .await;
        check_eq(&decide(DecisionId::FloorsSkip, || 0), &1, "decide")?;
        check_eq(&intn(DecisionId::FloorsModelGroup, 5, |_| 0), &4, "intn")?;
        check_eq(
            &case.session().snapshot().decisions.map(|d| d.len()),
            &Some(2),
            "recorded on the case's session",
        )?;
        drop(case);
        check_eq(&decide(DecisionId::FloorsSkip, || 0), &0, "after the case")
    }

    // Go: parity/trace/settle_test.go TestSettle.
    #[tokio::test]
    async fn settle() -> TestResult {
        let s = Session::new(env("settle"));
        check(s.settle(Duration::from_millis(1)).await, "nothing tracked")?;

        let gate = Arc::new(Semaphore::new(0));
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let done = s.track();
            let gate = Arc::clone(&gate);
            tasks.push(tokio::spawn(async move {
                let _permit = gate.acquire().await;
                drop(done);
            }));
        }
        check(
            !s.settle(Duration::from_millis(20)).await,
            "a task outlives the timeout",
        )?;
        gate.add_permits(3);
        check(
            s.settle(Duration::from_secs(5)).await,
            "settles once the tasks finish",
        )?;
        for task in tasks {
            task.await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // Settle's limit on the paused clock: with nothing tracked it is settled at once even with
    // no time left (Go's select between a closed channel and a fired timer is a race there); a
    // task that outlives the limit makes it report false exactly at the limit; one that
    // finishes in time ends the wait right then; and the future owns what it waits on, so it
    // runs on after the session handle is gone.
    #[tokio::test(start_paused = true)]
    async fn settle_limits() -> TestResult {
        let s = Session::new(env("limits"));
        check(s.settle(Duration::ZERO).await, "nothing tracked, no time")?;

        let done = s.track();
        check(!s.settle(Duration::ZERO).await, "pending, no time")?;
        let gave_up = Instant::now();
        check(!s.settle(Duration::from_millis(250)).await, "outlived")?;
        check_eq(
            &gave_up.elapsed(),
            &Duration::from_millis(250),
            "gave up at the limit",
        )?;

        let start = Instant::now();
        let waiting = tokio::spawn(s.settle(Duration::from_secs(60)));
        drop(s);
        sleep(Duration::from_millis(40)).await;
        drop(done);
        check(
            waiting.await.map_err(|e| e.to_string())?,
            "finished in time",
        )?;
        check_eq(
            &start.elapsed(),
            &Duration::from_millis(40),
            "returned when the task finished",
        )
    }

    // Guards dropped on another thread wake the waiting settle.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settle_wakes_across_threads() -> TestResult {
        let s = Session::new(env("threads"));
        let guards: Vec<Tracked> = (0..16).map(|_| s.track()).collect();
        let waiting = tokio::spawn(s.settle(Duration::from_secs(30)));
        let dropper = thread::spawn(move || {
            for guard in guards {
                thread::yield_now();
                drop(guard);
            }
        });
        check(waiting.await.map_err(|e| e.to_string())?, "settled")?;
        dropper
            .join()
            .map_err(|e| format!("the dropper failed: {e:?}"))?;
        check(s.settle(Duration::ZERO).await, "still settled")
    }

    // Go: TestSettleWaitsForDeferredWork.
    #[tokio::test]
    async fn settle_waits_for_work_after_the_result() -> TestResult {
        let s = Session::new(env("deferred"));
        let (results, mut received) = mpsc::channel(1);
        let recorded = Arc::new(Notify::new());
        let done = s.track();
        let notify = Arc::clone(&recorded);
        let task = tokio::spawn(async move {
            let _done = done;
            let _sent = results.send(1).await;
            sleep(Duration::from_millis(10)).await;
            notify.notify_one();
        });
        check_eq(&received.recv().await, &Some(1), "result first")?;
        check(s.settle(Duration::from_secs(5)).await, "settled")?;
        check(
            timeout(Duration::from_millis(1), recorded.notified())
                .await
                .is_ok(),
            "the deferred work ran before settle returned",
        )?;
        task.await.map_err(|e| e.to_string())
    }
}
