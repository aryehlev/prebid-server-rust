//! Go: `metrics/metrics_test.go`, plus the label value lists Go's enumerations return.

use std::{error::Error, time::Duration};

use super::{
    AdapterBid, AdapterError, CacheResult, CookieFlag, CookieSyncStatus, DemandSource,
    EndpointType, FetcherBackendResult, FetcherOperation, FetcherResult, GoDuration, ImpLabels,
    ImpMediaType, OverheadType, RequestStatus, RequestType, SetUidStatus, StoredDataError,
    StoredDataFetchType, StoredDataType, SyncerCookieSyncStatus, SyncerSetUidStatus,
    TcfVersionValue, adapter_bids, adapter_errors, cache_results, cookie_sync_statuses,
    cookie_types, demand_types, endpoint_types, fetcher_backend_results, fetcher_results,
    get_endpoint_from_request_type, imp_types, overhead_types, request_statuses, request_types,
    set_uid_statuses, stored_data_errors, stored_data_fetch_types, stored_data_types,
    syncer_request_statuses, syncer_set_uid_statuses, tcf_version_to_value, tcf_versions,
};

type TestResult = Result<(), Box<dyn Error>>;

fn strings<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn ensure(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

/// Go: `TestV2FetcherMetricEnumerationsReturnAllValues`.
#[test]
fn v2_fetcher_metric_enumerations_return_all_values() -> TestResult {
    ensure_eq!(
        fetcher_results(),
        [
            FetcherResult::HIT,
            FetcherResult::MISS,
            FetcherResult::NEGATIVE
        ],
        "FetcherResults"
    );
    ensure_eq!(
        fetcher_backend_results(),
        [
            FetcherBackendResult::OK,
            FetcherBackendResult::NOT_FOUND,
            FetcherBackendResult::ERROR
        ],
        "FetcherBackendResults"
    );
    Ok(())
}

/// Go: `TestGetEndpointFromRequestType`.
#[test]
fn get_endpoint_from_request_type_maps_every_request_type() -> TestResult {
    let cases = [
        (
            "request-type-openrtb2-web",
            RequestType::ORTB2_WEB,
            EndpointType::AUCTION,
        ),
        (
            "request-type-openrtb2-app",
            RequestType::ORTB2_APP,
            EndpointType::AUCTION,
        ),
        (
            "request-type-openrtb2-dooh",
            RequestType::ORTB2_DOOH,
            EndpointType::AUCTION,
        ),
        ("request-type-amp", RequestType::AMP, EndpointType::AMP),
        (
            "request-type-video",
            RequestType::VIDEO,
            EndpointType::VIDEO,
        ),
        // Go's switch has no default: any other value is the zero EndpointType.
        (
            "request-type-zero-value",
            RequestType::default(),
            EndpointType::default(),
        ),
    ];
    for (name, request_type, expected) in cases {
        ensure_eq!(
            get_endpoint_from_request_type(request_type),
            expected,
            "{name}"
        );
    }
    Ok(())
}

type EnumerationCase<'a> = (&'a str, Vec<String>, &'a [&'a str]);

fn check_enumerations(cases: Vec<EnumerationCase<'_>>) -> TestResult {
    for (name, actual, expected) in cases {
        ensure_eq!(actual, expected, "{name}");
    }
    Ok(())
}

/// The values and order of the Go request and adapter enumeration functions
/// (`metrics/metrics.go`).
#[test]
fn request_enumerations_match_go() -> TestResult {
    check_enumerations(vec![
        (
            "DemandTypes",
            strings(demand_types()),
            &["web", "app", "dooh", "unknown"],
        ),
        (
            "ImpTypes",
            strings(imp_types()),
            &["banner", "video", "audio", "native"],
        ),
        (
            "EndpointTypes",
            strings(endpoint_types()),
            &["auction", "video", "amp"],
        ),
        (
            "RequestTypes",
            strings(request_types()),
            &[
                "openrtb2-web",
                "openrtb2-app",
                "openrtb2-dooh",
                "amp",
                "video",
            ],
        ),
        (
            "CookieTypes",
            strings(cookie_types()),
            &["exists", "no", "unknown"],
        ),
        (
            "RequestStatuses",
            strings(request_statuses()),
            &[
                "ok",
                "badinput",
                "err",
                "networkerr",
                "blockedapp",
                "queuetimeout",
                "acctconfigerr",
            ],
        ),
        ("AdapterBids", strings(adapter_bids()), &["bid", "nobid"]),
        (
            "AdapterErrors",
            strings(adapter_errors()),
            &[
                "badinput",
                "badserverresponse",
                "timeout",
                "failedtorequestbid",
                "validation",
                "tmaxtimeout",
                "unknown_error",
            ],
        ),
        ("CacheResults", strings(cache_results()), &["hit", "miss"]),
        (
            "FetcherResults",
            strings(fetcher_results()),
            &["hit", "miss", "negative"],
        ),
        (
            "FetcherBackendResults",
            strings(fetcher_backend_results()),
            &["ok", "notfound", "error"],
        ),
        ("TCFVersions", strings(tcf_versions()), &["err", "v2"]),
    ])
}

/// The values and order of the Go sync, overhead and stored-data enumeration functions
/// (`metrics/metrics.go`).
#[test]
fn sync_and_stored_data_enumerations_match_go() -> TestResult {
    check_enumerations(vec![
        (
            "CookieSyncStatuses",
            strings(cookie_sync_statuses()),
            &[
                "ok",
                "bad_request",
                "opt_out",
                "gdpr_blocked_host_cookie",
                "acct_blocked",
                "acct_config_malformed",
                "acct_invalid",
            ],
        ),
        (
            "SyncerRequestStatuses",
            strings(syncer_request_statuses()),
            &[
                "ok",
                "privacy_blocked",
                "already_synced",
                "rejected_by_filter",
            ],
        ),
        (
            "SetUidStatuses",
            strings(set_uid_statuses()),
            &[
                "ok",
                "bad_request",
                "opt_out",
                "gdpr_blocked_host_cookie",
                "acct_blocked",
                "acct_config_malformed",
                "acct_invalid",
                "syncer_unknown",
            ],
        ),
        (
            "SyncerSetUidStatuses",
            strings(syncer_set_uid_statuses()),
            &["ok", "cleared"],
        ),
        (
            "OverheadTypes",
            strings(overhead_types()),
            &[
                "pre-bidder",
                "make-auction-response",
                "make-bidder-requests",
            ],
        ),
        (
            "StoredDataTypes",
            strings(stored_data_types()),
            &["account", "amp", "category", "request", "video", "response"],
        ),
        (
            "StoredDataFetchTypes",
            strings(stored_data_fetch_types()),
            &["all", "delta"],
        ),
        (
            "StoredDataErrors",
            strings(stored_data_errors()),
            &["network", "undefined"],
        ),
    ])
}

/// The constants Go defines without an enumeration function, and the Go zero values.
#[test]
fn label_constants_match_go() -> TestResult {
    let cases = [
        (FetcherOperation::GET.as_str(), "get"),
        (FetcherOperation::START.as_str(), "start"),
        (
            FetcherOperation::BACKGROUND_REFRESH.as_str(),
            "background_refresh",
        ),
        (DemandSource::default().as_str(), ""),
        (RequestType::default().as_str(), ""),
        (CookieFlag::default().as_str(), ""),
        (RequestStatus::default().as_str(), ""),
        (AdapterBid::default().as_str(), ""),
        (AdapterError::default().as_str(), ""),
        (CacheResult::default().as_str(), ""),
        (TcfVersionValue::default().as_str(), ""),
        (CookieSyncStatus::default().as_str(), ""),
        (SyncerCookieSyncStatus::default().as_str(), ""),
        (SetUidStatus::default().as_str(), ""),
        (SyncerSetUidStatus::default().as_str(), ""),
        (OverheadType::default().as_str(), ""),
        (StoredDataType::default().as_str(), ""),
        (StoredDataFetchType::default().as_str(), ""),
        (StoredDataError::default().as_str(), ""),
        (ImpMediaType::default().as_str(), ""),
    ];
    for (actual, expected) in cases {
        ensure_eq!(actual, expected, "label constant {expected:?}");
    }
    ensure_eq!(super::PUBLISHER_UNKNOWN, "unknown", "PublisherUnknown");
    ensure_eq!(
        CookieFlag::YES.to_string(),
        "exists",
        "Display writes the Go value"
    );
    Ok(())
}

/// Go: `TCFVersionToValue`.
#[test]
fn tcf_version_to_value_matches_go() -> TestResult {
    ensure_eq!(tcf_version_to_value(2), TcfVersionValue::V2, "version 2");
    for version in [0, 1, 3, -2] {
        ensure_eq!(
            tcf_version_to_value(version),
            TcfVersionValue::ERR,
            "version {version}"
        );
    }
    Ok(())
}

/// `Duration.Seconds()`, including negative and extreme durations.
#[test]
fn go_duration_seconds_matches_go() -> TestResult {
    let cases = [
        (0, 0.0),
        (1, 1e-9),
        (500_000_000, 0.5),
        (1_000_000_001, 1.000_000_001),
        (-5_000_000_000, -5.0),
        (-1, -1e-9),
        (i64::MAX, 9_223_372_036.854_776),
        (i64::MIN, -9_223_372_036.854_776),
    ];
    for (nanos, expected) in cases {
        let actual = GoDuration::from_nanos(nanos).seconds();
        ensure(
            actual.to_bits() == f64::to_bits(expected),
            &format!("{nanos}ns: {actual}"),
        )?;
    }
    ensure(
        GoDuration::times(10, GoDuration::MILLISECOND).as_nanos() == 10_000_000,
        "times",
    )?;
    ensure(
        GoDuration::times(i64::MAX, GoDuration::SECOND).as_nanos() < 0,
        "times wraps",
    )?;
    ensure(
        GoDuration::from(Duration::from_millis(1500)) == GoDuration::from_nanos(1_500_000_000),
        "from std",
    )?;
    ensure(
        GoDuration::from(Duration::MAX) == GoDuration::from_nanos(i64::MAX),
        "saturates",
    )
}

#[test]
fn imp_labels_hold_the_four_flags() -> TestResult {
    let labels = ImpLabels::NONE
        .with(ImpMediaType::BANNER, true)
        .with(ImpMediaType::NATIVE, true);
    ensure!(labels.banner_imps(), "banner");
    ensure!(!labels.video_imps(), "video");
    ensure!(!labels.audio_imps(), "audio");
    ensure!(labels.native_imps(), "native");
    let mut cleared = labels;
    cleared.set(ImpMediaType::BANNER, false);
    ensure!(!cleared.banner_imps(), "cleared banner");
    ensure!(!labels.has(ImpMediaType::default()), "zero media type");
    ensure_eq!(ImpLabels::default(), ImpLabels::NONE, "zero value");
    Ok(())
}
