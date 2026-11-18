//! The testify semantics of [`MetricsEngineMock`].

use std::error::Error;

use super::{Expectation, MetricsCall, MetricsEngineMock};
use crate::metrics::{CacheResult, FetcherResult, GoDuration, MetricsEngine as _};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn met_expectations_pass() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::call(MetricsCall::ConnectionAccept(true)).once());
    mock.on(Expectation::any_args("RecordAdapterThrottled"));
    mock.record_connection_accept(true);
    mock.record_adapter_throttled("a");
    mock.record_adapter_throttled("b");
    mock.assert_expectations()?;
    ensure_eq!(
        mock.number_of_calls("RecordAdapterThrottled"),
        2,
        "two throttled calls"
    );
    ensure!(
        mock.was_called(&MetricsCall::AdapterThrottled("b".to_owned())),
        "called with b"
    );
    ensure!(
        !mock.was_called(&MetricsCall::AdapterThrottled("c".to_owned())),
        "not called with c"
    );
    ensure_eq!(
        mock.calls(),
        vec![
            MetricsCall::ConnectionAccept(true),
            MetricsCall::AdapterThrottled("a".to_owned()),
            MetricsCall::AdapterThrottled("b".to_owned()),
        ],
        "calls in order"
    );
    Ok(())
}

#[test]
fn unmet_expectation_fails() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::call(MetricsCall::TMaxTimeout));
    mock.on(Expectation::call(MetricsCall::GvlListRequest).maybe());
    let err = mock
        .assert_expectations()
        .err()
        .ok_or("an unmet expectation must fail")?;
    ensure_eq!(
        err,
        "FAIL:\tTMaxTimeout",
        "only the required expectation fails"
    );
    Ok(())
}

#[test]
fn unexpected_and_excess_calls_fail() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::call(MetricsCall::LiveGvlFetch(true)).once());
    mock.record_live_gvl_fetch(true);
    mock.record_live_gvl_fetch(true);
    mock.record_live_gvl_fetch(false);
    let err = mock
        .assert_expectations()
        .err()
        .ok_or("excess calls must fail")?;
    let lines: Vec<&str> = err.lines().collect();
    ensure_eq!(
        lines,
        [
            "mock: The method has been called over the expected number of times: \
             LiveGvlFetch(true)",
            "mock: I don't know what to return because the method call was unexpected: \
             LiveGvlFetch(false)",
        ],
        "failures"
    );
    Ok(())
}

#[test]
fn times_requires_every_call() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::call(MetricsCall::StoredReqCacheResult(CacheResult::HIT, 1)).times(2));
    mock.record_stored_req_cache_result(CacheResult::HIT, 1);
    let err = mock
        .assert_expectations()
        .err()
        .ok_or("a missing call must fail")?;
    ensure!(err.ends_with("expected 1 more call(s)"), "{err}");
    mock.record_stored_req_cache_result(CacheResult::HIT, 1);
    mock.assert_expectations()?;
    Ok(())
}

#[test]
fn matched_by_selects_calls() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::matched_by(
        "RecordFetcherResult",
        |call| matches!(call, MetricsCall::FetcherResult(subsystem, _) if subsystem == "account"),
    ));
    mock.record_fetcher_result("account", FetcherResult::MISS);
    mock.assert_expectations()?;
    mock.record_fetcher_result("request", FetcherResult::MISS);
    ensure!(
        mock.assert_expectations().is_err(),
        "a call the matcher rejects is unexpected"
    );
    Ok(())
}

#[test]
fn dial_error_is_recorded_without_its_adapter() -> TestResult {
    let mock = MetricsEngineMock::new();
    mock.on(Expectation::call(MetricsCall::AdapterConnectionDialError));
    mock.record_adapter_connection_dial_error("appnexus");
    mock.record_adapter_connection_dial_time("appnexus", GoDuration::SECOND);
    ensure_eq!(
        mock.calls().first(),
        Some(&MetricsCall::AdapterConnectionDialError),
        "the Go mock drops the adapter name"
    );
    ensure_eq!(
        MetricsCall::AdapterConnectionDialTime(String::new(), GoDuration::SECOND).method_name(),
        "RecordAdapterConnectionDialTime",
        "Go method name"
    );
    Ok(())
}
