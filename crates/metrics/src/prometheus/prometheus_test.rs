//! Go: `metrics/prometheus/prometheus_test.go`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    error::Error,
    fs,
};

use ::prometheus::{
    Histogram, HistogramVec, IntCounter, IntCounterVec,
    core::Collector,
    proto::{self, Metric},
};

use super::{
    ACCOUNT_LABEL, ADAPTER_ERROR_LABEL, ADAPTER_LABEL, CACHE_RESULT_LABEL, CONNECTION_ACCEPT_ERROR,
    CONNECTION_CLOSE_ERROR, CONNECTION_ERROR_LABEL, COOKIE_LABEL, FETCHER_RESULT_LABEL,
    HAS_BIDS_LABEL, IS_AUDIO_LABEL, IS_BANNER_LABEL, IS_NATIVE_LABEL, IS_VIDEO_LABEL,
    MARKUP_DELIVERY_ADM, MARKUP_DELIVERY_LABEL, MARKUP_DELIVERY_NURL, Metrics, OPERATION_LABEL,
    OPT_OUT_LABEL, OVERHEAD_TYPE_LABEL, REQUEST_ENDPOINT_LABEL, REQUEST_FAILED,
    REQUEST_REJECT_LABEL, REQUEST_STATUS_LABEL, REQUEST_SUCCESS_LABEL, REQUEST_SUCCESSFUL,
    REQUEST_TYPE_LABEL, SOURCE_LABEL, SOURCE_REQUEST, STAGE_LABEL, STATUS_LABEL,
    STORED_DATA_ERROR_LABEL, STORED_DATA_FETCH_TYPE_LABEL, SUBSYSTEM_LABEL, SUCCESS_LABEL,
    SYNCER_LABEL, VERSION_LABEL, count,
};
use crate::{
    float_from_i64, float_from_u64,
    metrics::{
        AdapterBid, AdapterError, AdapterLabels, CacheResult, CcpaLabels, CookieFlag,
        CookieSyncStatus, EndpointType, FetcherBackendResult, FetcherOperation, FetcherResult,
        GdprLabels, GoDuration, ImpLabels, ImpMediaType, Labels, MetricsEngine as _, ModuleLabels,
        OverheadType, PUBLISHER_UNKNOWN, PrivacyLabels, RequestStatus, RequestType, SetUidStatus,
        StoredDataError, StoredDataFetchType, StoredDataLabels, StoredDataType,
        SyncerCookieSyncStatus, SyncerSetUidStatus, TcfVersionValue,
    },
    options::{DisabledMetric, DisabledMetrics, PrometheusMetrics},
};

type TestResult = Result<(), Box<dyn Error>>;

/// `openrtb_ext.CoreBidderNames()`, written by the Go golden generator.
pub(super) fn core_bidder_names() -> Result<Vec<String>, Box<dyn Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/golden/core_bidder_names.txt"
    );
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Go: `modulesStages`.
fn modules_stages() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([
        (
            "foobar".to_owned(),
            vec!["entry".to_owned(), "raw".to_owned()],
        ),
        (
            "another_module".to_owned(),
            vec!["raw".to_owned(), "auction".to_owned()],
        ),
    ])
}

fn testing_config() -> PrometheusMetrics {
    PrometheusMetrics {
        port: 8080,
        namespace: "prebid".to_owned(),
        subsystem: "server".to_owned(),
        timeout_millis_raw: 0,
    }
}

/// Go: `createMetricsForTesting`.
fn create_metrics_for_testing() -> Result<Metrics, Box<dyn Error>> {
    Ok(Metrics::new(
        &testing_config(),
        DisabledMetrics::NONE,
        &[],
        &modules_stages(),
        &core_bidder_names()?,
    )?)
}

/// A count as the `float64` Go's tests compare it with.
fn value(count: u64) -> f64 {
    float_from_u64(count)
}

/// The value of the label `name` of a gathered series.
fn label<'a>(metric: &'a Metric, name: &str) -> Option<&'a str> {
    metric
        .get_label()
        .iter()
        .find(|pair| pair.name() == name)
        .map(proto::LabelPair::value)
}

/// The series of a family, as Go's tests collect them.
fn series<C: Collector>(family: &C) -> Vec<Metric> {
    family
        .collect()
        .into_iter()
        .flat_map(|mut f| f.take_metric())
        .collect()
}

/// Go: `assertCounterVecValue`'s read — `counterVec.With(labels)`, which creates the series.
fn counter_vec_value(
    counter_vec: Option<&IntCounterVec>,
    labels: &[(&str, &str)],
) -> Result<f64, Box<dyn Error>> {
    let counter_vec = counter_vec.ok_or("the counter vector is nil")?;
    let labels: HashMap<&str, &str> = labels.iter().copied().collect();
    Ok(value(counter_vec.get_metric_with(&labels)?.get()))
}

/// Go: `assertCounterValue` for a counter without labels.
fn counter_value(counter: &IntCounter) -> f64 {
    value(counter.get())
}

/// Go: `getHistogramFromHistogramVec` — the series with `label_key=label_value`, or the zero
/// histogram and `false`.
fn histogram_from_vec(
    histogram: Option<&HistogramVec>,
    label_key: &str,
    label_value: &str,
) -> (proto::Histogram, bool) {
    let mut result = proto::Histogram::default();
    let mut found = false;
    for metric in histogram.map_or_else(Vec::new, series) {
        if label(&metric, label_key) == Some(label_value) {
            result = metric.get_histogram().clone();
            found = true;
        }
    }
    (result, found)
}

/// Go: `getHistogramFromHistogramVecByTwoKeys`.
fn histogram_from_vec_by_two_keys(
    histogram: &HistogramVec,
    (label1_key, label1_value): (&str, &str),
    (label2_key, label2_value): (&str, &str),
) -> proto::Histogram {
    let mut result = proto::Histogram::default();
    for metric in series(histogram) {
        if label(&metric, label1_key) == Some(label1_value)
            && label(&metric, label2_key) == Some(label2_value)
        {
            result = metric.get_histogram().clone();
        }
    }
    result
}

/// Go: `dto.Metric` of a histogram without labels.
fn plain_histogram(histogram: &Histogram) -> proto::Histogram {
    let mut result = proto::Histogram::default();
    result.set_sample_count(histogram.get_sample_count());
    result.set_sample_sum(histogram.get_sample_sum());
    result
}

/// Go: `assertHistogram`.
fn check_histogram(
    name: &str,
    histogram: &proto::Histogram,
    expected_count: u64,
    expected_sum: f64,
) -> TestResult {
    ensure_eq!(histogram.get_sample_count(), expected_count, "{name}:count");
    ensure_eq!(histogram.get_sample_sum(), expected_sum, "{name}:sum");
    Ok(())
}

fn millis(ms: i64) -> GoDuration {
    GoDuration::times(ms, GoDuration::MILLISECOND)
}

fn seconds(s: i64) -> GoDuration {
    GoDuration::times(s, GoDuration::SECOND)
}

fn adapter_labels(adapter: &str) -> AdapterLabels {
    AdapterLabels {
        adapter: adapter.to_owned(),
        ..AdapterLabels::default()
    }
}

/// Go: `TestMetricCountGatekeeping`.
#[test]
fn metric_count_gatekeeping() -> TestResult {
    let m = create_metrics_for_testing()?;
    let mut general_cardinality_count = 0_usize;
    let mut adapter_cardinality_count = 0_usize;
    for family in m.gatherer().gather() {
        for metric in family.get_metric() {
            if label(metric, ADAPTER_LABEL).is_some() {
                adapter_cardinality_count += 1;
            } else {
                general_cardinality_count += 1;
            }
        }
    }
    let adapter_count = core_bidder_names()?.len();
    let per_adapter_cardinality_count = adapter_cardinality_count
        .checked_div(adapter_count)
        .ok_or("no core bidders")?;
    ensure!(
        general_cardinality_count <= 500,
        "General Cardinality {general_cardinality_count}"
    );
    ensure!(
        per_adapter_cardinality_count <= 33,
        "Per-Adapter Cardinality count equals {per_adapter_cardinality_count}"
    );
    Ok(())
}

/// Go: `TestConnectionMetrics`.
#[test]
fn connection_metrics() -> TestResult {
    type Case = (&'static str, fn(&Metrics), [f64; 6], u64);
    // [opened, opened errors, closed, closed errors, dial errors, dial time], dial timer calls.
    let cases: [Case; 6] = [
        (
            "Open Success",
            |m| m.record_connection_accept(true),
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            0,
        ),
        (
            "Open Error",
            |m| m.record_connection_accept(false),
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            0,
        ),
        (
            "Closed Success",
            |m| m.record_connection_close(true),
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            0,
        ),
        (
            "Closed Error",
            |m| m.record_connection_close(false),
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            0,
        ),
        (
            "connection-dial-ended-and-threw-an-error",
            |m| m.record_adapter_connection_dial_error("anyName"),
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            0,
        ),
        (
            "connection-dial-ended-and-was-timed",
            |m| m.record_adapter_connection_dial_time("anyName", seconds(1)),
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            1,
        ),
    ];
    for (description, record, expected, expected_dial_timer_calls) in cases {
        let m = create_metrics_for_testing()?;
        record(&m);
        let [
            opened,
            opened_errors,
            closed,
            closed_errors,
            dial_errors,
            dial_time,
        ] = expected;
        ensure_eq!(
            counter_value(&m.connections_closed),
            closed,
            "{description}"
        );
        ensure_eq!(
            counter_value(&m.connections_opened),
            opened,
            "{description}"
        );
        let actual_accept_errors = counter_vec_value(
            Some(&m.connections_error),
            &[(CONNECTION_ERROR_LABEL, CONNECTION_ACCEPT_ERROR)],
        )?;
        ensure_eq!(actual_accept_errors, opened_errors, "{description}");
        let actual_close_errors = counter_vec_value(
            Some(&m.connections_error),
            &[(CONNECTION_ERROR_LABEL, CONNECTION_CLOSE_ERROR)],
        )?;
        ensure_eq!(actual_close_errors, closed_errors, "{description}");
        let dial = counter_vec_value(
            m.adapter_connection_dial_errors.as_ref(),
            &[(ADAPTER_LABEL, "anyname")],
        )?;
        ensure_eq!(dial, dial_errors, "{description}");
        let (histogram, found) = histogram_from_vec(
            m.adapter_connection_dial_time.as_ref(),
            ADAPTER_LABEL,
            "anyname",
        );
        ensure_eq!(found, expected_dial_timer_calls > 0, "{description}");
        check_histogram(
            description,
            &histogram,
            expected_dial_timer_calls,
            dial_time,
        )?;
    }
    Ok(())
}

/// Go: `TestRequestMetric`.
#[test]
fn request_metric() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_request(&Labels {
        r_type: RequestType::ORTB2_WEB,
        request_status: RequestStatus::BLOCKED_APP,
        request_size: 1024,
        ..Labels::default()
    });
    let requests = counter_vec_value(
        Some(&m.requests),
        &[
            (REQUEST_TYPE_LABEL, "openrtb2-web"),
            (REQUEST_STATUS_LABEL, "blockedapp"),
        ],
    )?;
    ensure_eq!(requests, 1.0, "requests");
    let (histogram, found) = histogram_from_vec(
        Some(&m.requests_size),
        REQUEST_ENDPOINT_LABEL,
        EndpointType::AUCTION.as_str(),
    );
    ensure!(found, "requests_size_auction found");
    check_histogram("requests_size_auction", &histogram, 1, 1024.0)?;
    Ok(())
}

/// Go: `TestDebugRequestMetric`.
#[test]
fn debug_request_metric() -> TestResult {
    let cases = [
        (
            "Debug is enabled and account debug is enabled",
            true,
            false,
            1.0,
            1.0,
        ),
        (
            "Debug and account debug are disabled",
            false,
            true,
            0.0,
            0.0,
        ),
        (
            "Debug is enabled but account debug is disabled",
            true,
            true,
            1.0,
            0.0,
        ),
        (
            "Debug is disabled and account debug is enabled",
            false,
            false,
            0.0,
            0.0,
        ),
    ];
    for (description, debug_enabled, account_debug_disabled, debug_count, account_count) in cases {
        let mut m = create_metrics_for_testing()?;
        m.disabled_flags
            .set(DisabledMetric::AccountDebug, account_debug_disabled);
        m.record_debug_request(debug_enabled, "acct-id");
        let account = counter_vec_value(
            Some(&m.account_debug_requests),
            &[(ACCOUNT_LABEL, "acct-id")],
        )?;
        ensure_eq!(
            account,
            account_count,
            "{description}: account debug requests"
        );
        ensure_eq!(
            counter_value(&m.debug_requests),
            debug_count,
            "{description}: debug requests"
        );
    }
    Ok(())
}

/// Go: `TestBidValidationCreativeSizeMetric` and `TestBidValidationSecureMarkupMetric`.
#[test]
fn bid_validation_metrics() -> TestResult {
    for (account_adapter_details_disabled, expected_account_count) in [(false, 1.0), (true, 0.0)] {
        let mut m = create_metrics_for_testing()?;
        m.disabled_flags.set(
            DisabledMetric::AccountAdapterDetails,
            account_adapter_details_disabled,
        );
        m.record_bid_validation_creative_size_error("AnyName", "acct-id");
        m.record_bid_validation_creative_size_warn("AnyName", "acct-id");
        m.record_bid_validation_secure_markup_error("AnyName", "acct-id");
        m.record_bid_validation_secure_markup_warn("AnyName", "acct-id");

        let account = [(ACCOUNT_LABEL, "acct-id"), (SUCCESS_LABEL, SUCCESS_LABEL)];
        let adapter = [(ADAPTER_LABEL, "anyname"), (SUCCESS_LABEL, SUCCESS_LABEL)];
        for (name, family, labels, expected) in [
            (
                "account size error",
                &m.account_bid_response_validation_size_error,
                account,
                expected_account_count,
            ),
            (
                "adapter size error",
                &m.adapter_bid_response_validation_size_error,
                adapter,
                1.0,
            ),
            (
                "account size warn",
                &m.account_bid_response_validation_size_warn,
                account,
                expected_account_count,
            ),
            (
                "adapter size warn",
                &m.adapter_bid_response_validation_size_warn,
                adapter,
                1.0,
            ),
            (
                "account secure error",
                &m.account_bid_response_secure_markup_error,
                account,
                expected_account_count,
            ),
            (
                "adapter secure error",
                &m.adapter_bid_response_secure_markup_error,
                adapter,
                1.0,
            ),
            (
                "account secure warn",
                &m.account_bid_response_secure_markup_warn,
                account,
                expected_account_count,
            ),
            (
                "adapter secure warn",
                &m.adapter_bid_response_secure_markup_warn,
                adapter,
                1.0,
            ),
        ] {
            let actual = counter_vec_value(Some(family), &labels)?;
            ensure_eq!(
                actual,
                expected,
                "{name} (disabled={account_adapter_details_disabled})"
            );
        }
    }
    Ok(())
}

/// Go: `TestRequestMetricWithoutCookie`.
#[test]
fn request_metric_without_cookie() -> TestResult {
    for (cookie_flag, expected) in [
        (CookieFlag::YES, 0.0),
        (CookieFlag::NO, 1.0),
        (CookieFlag::UNKNOWN, 0.0),
    ] {
        let m = create_metrics_for_testing()?;
        m.record_request(&Labels {
            r_type: RequestType::ORTB2_WEB,
            request_status: RequestStatus::BLOCKED_APP,
            cookie_flag,
            ..Labels::default()
        });
        let actual = counter_vec_value(
            Some(&m.requests_without_cookie),
            &[(REQUEST_TYPE_LABEL, "openrtb2-web")],
        )?;
        ensure_eq!(actual, expected, "{cookie_flag}");
    }
    Ok(())
}

/// Go: `TestAccountMetric`.
#[test]
fn account_metric() -> TestResult {
    for (pub_id, expected) in [("knownPublisher", 1.0), (PUBLISHER_UNKNOWN, 0.0)] {
        let m = create_metrics_for_testing()?;
        m.record_request(&Labels {
            r_type: RequestType::ORTB2_WEB,
            request_status: RequestStatus::BLOCKED_APP,
            pub_id: pub_id.to_owned(),
            ..Labels::default()
        });
        let actual = counter_vec_value(
            Some(&m.account_requests),
            &[(ACCOUNT_LABEL, "knownPublisher")],
        )?;
        ensure_eq!(actual, expected, "{pub_id}");
    }
    Ok(())
}

/// Go: `TestImpressionsMetric`.
#[test]
fn impressions_metric() -> TestResult {
    let cases = [
        ("Banner Only", [true, false, false, false]),
        ("Video Only", [false, true, false, false]),
        ("Audio Only", [false, false, true, false]),
        ("Native Only", [false, false, false, true]),
        ("Multiple Types", [true, false, false, true]),
    ];
    let types = [
        ImpMediaType::BANNER,
        ImpMediaType::VIDEO,
        ImpMediaType::AUDIO,
        ImpMediaType::NATIVE,
    ];
    for (description, flags) in cases {
        let m = create_metrics_for_testing()?;
        let labels = types
            .iter()
            .zip(flags)
            .fold(ImpLabels::NONE, |l, (t, f)| l.with(*t, f));
        m.record_imps(labels);

        let mut counts: BTreeMap<String, f64> = BTreeMap::new();
        for metric in series(&m.impressions) {
            let value = metric.get_counter().get_value();
            for label in metric.get_label() {
                if label.value() == "true" {
                    *counts.entry(label.name().to_owned()).or_default() += value;
                }
            }
        }
        for (label, expected) in [
            IS_BANNER_LABEL,
            IS_VIDEO_LABEL,
            IS_AUDIO_LABEL,
            IS_NATIVE_LABEL,
        ]
        .into_iter()
        .zip(flags)
        {
            let actual = counts.get(label).copied().unwrap_or_default();
            ensure_eq!(
                actual,
                if expected { 1.0 } else { 0.0 },
                "{description}:{label}"
            );
        }
    }
    Ok(())
}

/// Go: `TestRequestTimeMetric`.
#[test]
fn request_time_metric() -> TestResult {
    for (description, request_status, expected_count, expected_sum) in [
        ("Success", RequestStatus::OK, 1, 0.5),
        ("Error", RequestStatus::ERR, 0, 0.0),
    ] {
        let m = create_metrics_for_testing()?;
        let labels = Labels {
            r_type: RequestType::ORTB2_WEB,
            request_status,
            ..Labels::default()
        };
        m.record_request_time(&labels, millis(500));
        let (result, found) =
            histogram_from_vec(Some(&m.requests_timer), REQUEST_TYPE_LABEL, "openrtb2-web");
        ensure!(found, "{description} found");
        check_histogram(description, &result, expected_count, expected_sum)?;
    }
    Ok(())
}

/// Go: `TestRecordOverheadTimeMetric`.
#[test]
fn record_overhead_time_metric() -> TestResult {
    let cases = [
        (
            "record-pre-bidder-overhead-time-1",
            OverheadType::PRE_BIDDER,
            500,
            1,
            0.5,
        ),
        (
            "record-pre-bidder-overhead-time-2",
            OverheadType::PRE_BIDDER,
            400,
            2,
            0.9,
        ),
        (
            "record-auction-response-overhead-time",
            OverheadType::MAKE_AUCTION_RESPONSE,
            500,
            1,
            0.5,
        ),
        (
            "record-make-bidder-requests-overhead-time",
            OverheadType::MAKE_BIDDER_REQUESTS,
            500,
            1,
            0.5,
        ),
    ];
    let m = create_metrics_for_testing()?;
    for (description, overhead_type, time_in_ms, expected_count, expected_sum) in cases {
        m.record_overhead_time(overhead_type, millis(time_in_ms));
        let (result, found) = histogram_from_vec(
            Some(&m.overhead_timer),
            OVERHEAD_TYPE_LABEL,
            overhead_type.as_str(),
        );
        ensure!(found, "{description} found");
        check_histogram(description, &result, expected_count, expected_sum)?;
    }
    Ok(())
}

fn stored_fetch_timer(m: &Metrics, data_type: StoredDataType) -> Option<&HistogramVec> {
    [
        (StoredDataType::ACCOUNT, &m.stored_account_fetch_timer),
        (StoredDataType::AMP, &m.stored_amp_fetch_timer),
        (StoredDataType::CATEGORY, &m.stored_category_fetch_timer),
        (StoredDataType::REQUEST, &m.stored_request_fetch_timer),
        (StoredDataType::VIDEO, &m.stored_video_fetch_timer),
        (StoredDataType::RESPONSE, &m.stored_responses_fetch_timer),
    ]
    .into_iter()
    .find_map(|(t, timer)| (t == data_type).then_some(timer))
}

fn stored_errors(m: &Metrics, data_type: StoredDataType) -> Option<&IntCounterVec> {
    [
        (StoredDataType::ACCOUNT, &m.stored_account_errors),
        (StoredDataType::AMP, &m.stored_amp_errors),
        (StoredDataType::CATEGORY, &m.stored_category_errors),
        (StoredDataType::REQUEST, &m.stored_request_errors),
        (StoredDataType::VIDEO, &m.stored_video_errors),
        (StoredDataType::RESPONSE, &m.stored_responses_errors),
    ]
    .into_iter()
    .find_map(|(t, errors)| (t == data_type).then_some(errors))
}

/// Go: `TestRecordStoredDataFetchTime`.
#[test]
fn record_stored_data_fetch_time() -> TestResult {
    let mut cases: Vec<(StoredDataType, StoredDataFetchType)> = Vec::new();
    for fetch_type in [StoredDataFetchType::ALL, StoredDataFetchType::DELTA] {
        for data_type in [
            StoredDataType::ACCOUNT,
            StoredDataType::AMP,
            StoredDataType::CATEGORY,
            StoredDataType::REQUEST,
            StoredDataType::VIDEO,
        ] {
            cases.push((data_type, fetch_type));
        }
    }
    cases.push((StoredDataType::RESPONSE, StoredDataFetchType::DELTA));
    for (data_type, fetch_type) in cases {
        let m = create_metrics_for_testing()?;
        m.record_stored_data_fetch_time(
            StoredDataLabels {
                data_type,
                data_fetch_type: fetch_type,
                ..StoredDataLabels::default()
            },
            millis(500),
        );
        let (result, found) = histogram_from_vec(
            stored_fetch_timer(&m, data_type),
            STORED_DATA_FETCH_TYPE_LABEL,
            fetch_type.as_str(),
        );
        ensure!(found, "{data_type} {fetch_type} found");
        check_histogram(&format!("{data_type} {fetch_type}"), &result, 1, 0.5)?;
    }
    Ok(())
}

/// Go: `TestRecordStoredDataError`.
#[test]
fn record_stored_data_error() -> TestResult {
    let mut cases: Vec<(StoredDataType, StoredDataError)> = Vec::new();
    for error in [StoredDataError::NETWORK, StoredDataError::UNDEFINED] {
        for data_type in [
            StoredDataType::ACCOUNT,
            StoredDataType::AMP,
            StoredDataType::CATEGORY,
            StoredDataType::REQUEST,
            StoredDataType::VIDEO,
        ] {
            cases.push((data_type, error));
        }
    }
    cases.push((StoredDataType::RESPONSE, StoredDataError::NETWORK));
    for (data_type, error) in cases {
        let m = create_metrics_for_testing()?;
        m.record_stored_data_error(StoredDataLabels {
            data_type,
            error,
            ..StoredDataLabels::default()
        });
        let actual = counter_vec_value(
            stored_errors(&m, data_type),
            &[(STORED_DATA_ERROR_LABEL, error.as_str())],
        )?;
        ensure_eq!(actual, 1.0, "stored_{data_type}_errors {error}");
    }
    Ok(())
}

/// Go: `TestAdapterBidReceivedMetric`.
#[test]
fn adapter_bid_received_metric() -> TestResult {
    for (description, has_adm, expected_adm, expected_nurl) in
        [("AdM", true, 1.0, 0.0), ("Nurl", false, 0.0, 1.0)]
    {
        let m = create_metrics_for_testing()?;
        m.record_adapter_bid_received(&adapter_labels("anyName"), "banner", has_adm);
        let adm = counter_vec_value(
            Some(&m.adapter_bids),
            &[
                (ADAPTER_LABEL, "anyname"),
                (MARKUP_DELIVERY_LABEL, MARKUP_DELIVERY_ADM),
            ],
        )?;
        let nurl = counter_vec_value(
            Some(&m.adapter_bids),
            &[
                (ADAPTER_LABEL, "anyname"),
                (MARKUP_DELIVERY_LABEL, MARKUP_DELIVERY_NURL),
            ],
        )?;
        ensure_eq!(adm, expected_adm, "{description}: adapterBids[adm]");
        ensure_eq!(nurl, expected_nurl, "{description}: adapterBids[nurl]");
    }
    Ok(())
}

/// Go: `TestRecordAdapterPriceMetric`.
#[test]
fn record_adapter_price_metric() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_adapter_price(&adapter_labels("anyName"), 42.0);
    let (result, found) = histogram_from_vec(Some(&m.adapter_prices), ADAPTER_LABEL, "anyname");
    ensure!(found, "adapterPrices found");
    check_histogram("adapterPrices", &result, 1, 42.0)?;
    Ok(())
}

/// Sums the adapter request counters of `anyname` by cookie value and `has_bids`.
fn adapter_request_totals(m: &Metrics) -> [f64; 5] {
    let mut totals = [0.0; 5];
    for metric in series(&m.adapter_requests) {
        if label(&metric, ADAPTER_LABEL) != Some("anyname") {
            continue;
        }
        let value = metric.get_counter().get_value();
        let [total, cookie_no, cookie_yes, cookie_unknown, has_bids] = &mut totals;
        *total += value;
        if label(&metric, HAS_BIDS_LABEL) == Some("true") {
            *has_bids += value;
        }
        match label(&metric, COOKIE_LABEL) {
            Some("no") => *cookie_no += value,
            Some("exists") => *cookie_yes += value,
            Some("unknown") => *cookie_unknown += value,
            _ => {}
        }
    }
    totals
}

/// Go: `TestAdapterRequestMetrics`.
#[test]
fn adapter_request_metrics() -> TestResult {
    let cases = [
        (
            "No Cookie & No Bids",
            CookieFlag::NO,
            AdapterBid::NONE,
            [1.0, 1.0, 0.0, 0.0, 0.0],
        ),
        (
            "Unknown Cookie & No Bids",
            CookieFlag::UNKNOWN,
            AdapterBid::NONE,
            [1.0, 0.0, 0.0, 1.0, 0.0],
        ),
        (
            "Has Cookie & No Bids",
            CookieFlag::YES,
            AdapterBid::NONE,
            [1.0, 0.0, 1.0, 0.0, 0.0],
        ),
        (
            "No Cookie & Bids Present",
            CookieFlag::NO,
            AdapterBid::PRESENT,
            [1.0, 1.0, 0.0, 0.0, 1.0],
        ),
        (
            "Unknown Cookie & Bids Present",
            CookieFlag::UNKNOWN,
            AdapterBid::PRESENT,
            [1.0, 0.0, 0.0, 1.0, 1.0],
        ),
        (
            "Has Cookie & Bids Present",
            CookieFlag::YES,
            AdapterBid::PRESENT,
            [1.0, 0.0, 1.0, 0.0, 1.0],
        ),
    ];
    for (description, cookie_flag, adapter_bids, expected) in cases {
        let m = create_metrics_for_testing()?;
        m.record_adapter_request(&AdapterLabels {
            adapter: "anyName".to_owned(),
            cookie_flag,
            adapter_bids,
            ..AdapterLabels::default()
        });
        ensure_eq!(adapter_request_totals(&m), expected, "{description}");
    }
    Ok(())
}

/// Go: `TestAdapterRequestErrorMetrics`.
#[test]
fn adapter_request_error_metrics() -> TestResult {
    let cases = [
        ("No Errors", BTreeSet::new(), 0.0, 0.0),
        (
            "Bad Input Error",
            BTreeSet::from([AdapterError::BAD_INPUT]),
            1.0,
            1.0,
        ),
        (
            "Other Error",
            BTreeSet::from([AdapterError::BAD_SERVER_RESPONSE]),
            1.0,
            0.0,
        ),
    ];
    for (description, adapter_errors, expected_errors, expected_bad_input) in cases {
        let m = create_metrics_for_testing()?;
        m.record_adapter_request(&AdapterLabels {
            adapter: "anyName".to_owned(),
            adapter_errors,
            cookie_flag: CookieFlag::UNKNOWN,
            adapter_bids: AdapterBid::PRESENT,
            ..AdapterLabels::default()
        });
        let mut errors = 0.0;
        let mut bad_input = 0.0;
        for metric in series(&m.adapter_errors) {
            if label(&metric, ADAPTER_LABEL) != Some("anyname") {
                continue;
            }
            let value = metric.get_counter().get_value();
            errors += value;
            if label(&metric, ADAPTER_ERROR_LABEL) == Some(AdapterError::BAD_INPUT.as_str()) {
                bad_input += value;
            }
        }
        ensure_eq!(errors, expected_errors, "{description}:errors");
        ensure_eq!(
            bad_input,
            expected_bad_input,
            "{description}:badInputErrors"
        );
    }
    Ok(())
}

/// Go: `TestAdapterTimeMetric`.
#[test]
fn adapter_time_metric() -> TestResult {
    let cases = [
        ("Success", BTreeSet::new(), 1, 0.5),
        ("Error", BTreeSet::from([AdapterError::TIMEOUT]), 0, 0.0),
    ];
    for (description, adapter_errors, expected_count, expected_sum) in cases {
        let m = create_metrics_for_testing()?;
        let labels = AdapterLabels {
            adapter_errors,
            ..adapter_labels("anyName")
        };
        m.record_adapter_time(&labels, millis(500));
        let (result, found) =
            histogram_from_vec(Some(&m.adapter_requests_timer), ADAPTER_LABEL, "anyname");
        ensure_eq!(found, expected_count > 0, "{description} found");
        check_histogram(description, &result, expected_count, expected_sum)?;
    }
    Ok(())
}

/// Go: `TestAdapterPanicMetric`.
#[test]
fn adapter_panic_metric() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_adapter_panic(&adapter_labels("anyName"));
    let actual = counter_vec_value(Some(&m.adapter_panics), &[(ADAPTER_LABEL, "anyname")])?;
    ensure_eq!(actual, 1.0, "adapterPanics");
    Ok(())
}

/// Go: `TestStoredReqCacheResultMetric`, `TestStoredImpCacheResultMetric` and
/// `TestAccountCacheResultMetric`.
#[test]
fn cache_result_metrics() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_stored_req_cache_result(CacheResult::HIT, 42);
    m.record_stored_req_cache_result(CacheResult::MISS, 108);
    m.record_stored_imp_cache_result(CacheResult::HIT, 41);
    m.record_stored_imp_cache_result(CacheResult::MISS, 107);
    m.record_account_cache_result(CacheResult::HIT, 37);
    m.record_account_cache_result(CacheResult::MISS, 92);
    for (name, family, hit, miss) in [
        (
            "storedRequestCacheResult",
            &m.stored_request_cache_result,
            42.0,
            108.0,
        ),
        (
            "storedImpressionsCacheResult",
            &m.stored_impressions_cache_result,
            41.0,
            107.0,
        ),
        ("accountCacheResult", &m.account_cache_result, 37.0, 92.0),
    ] {
        let actual_hit = counter_vec_value(Some(family), &[(CACHE_RESULT_LABEL, "hit")])?;
        let actual_miss = counter_vec_value(Some(family), &[(CACHE_RESULT_LABEL, "miss")])?;
        ensure_eq!(actual_hit, hit, "{name}:hit");
        ensure_eq!(actual_miss, miss, "{name}:miss");
    }
    Ok(())
}

/// Go adds `uint64(float64(n))` to a counter's integer: a count above 2^53 is rounded to the
/// nearest `f64`, a tie to the even one, before it is added.
// Go: metrics/prometheus/prometheus.go:974-990 (Add(float64(inc))); client_golang v1.12.1
// prometheus/counter.go:112-116 (Add's integer part) and :136-140 (get)
#[test]
fn cache_counts_round_as_go_converts_them() -> TestResult {
    const TWO_POW_53: u64 = 9_007_199_254_740_992;
    for (n, expected) in [
        (0, 0),
        (1, 1),
        (TWO_POW_53 - 1, TWO_POW_53 - 1),
        (TWO_POW_53, TWO_POW_53),
        (TWO_POW_53 + 1, TWO_POW_53),
        (TWO_POW_53 + 2, TWO_POW_53 + 2),
        (TWO_POW_53 + 3, TWO_POW_53 + 4),
        (123_456_789_012_345_678, 123_456_789_012_345_680),
    ] {
        ensure_eq!(count(usize::try_from(n)?), expected, "count({n})");
    }

    // Go's integer part holds 2^53 + 1, which its scrape reads as 2^53; adding 2^53 + 1 and 1
    // exactly would read 2^53 + 2.
    let m = create_metrics_for_testing()?;
    m.record_account_cache_result(CacheResult::HIT, usize::try_from(TWO_POW_53 + 1)?);
    m.record_account_cache_result(CacheResult::HIT, 1);
    let actual = counter_vec_value(
        Some(&m.account_cache_result),
        &[(CACHE_RESULT_LABEL, "hit")],
    )?;
    ensure_eq!(actual, float_from_u64(TWO_POW_53), "accountCacheResult:hit");
    Ok(())
}

/// Go: `TestV2FetcherMetrics`.
#[test]
fn v2_fetcher_metrics() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_fetcher_result("account", FetcherResult::HIT);
    m.record_fetcher_result("account", FetcherResult::HIT);
    m.record_fetcher_result("account", FetcherResult::MISS);
    m.record_fetcher_result("account", FetcherResult::NEGATIVE);
    m.record_fetcher_backend_fetch(
        "account",
        FetcherOperation::GET,
        FetcherBackendResult::OK,
        millis(1),
    );
    m.record_fetcher_backend_fetch(
        "account",
        FetcherOperation::START,
        FetcherBackendResult::NOT_FOUND,
        millis(1),
    );
    for (result, expected) in [("hit", 2.0), ("miss", 1.0), ("negative", 1.0)] {
        let actual = counter_vec_value(
            Some(&m.fetcher_result),
            &[(SUBSYSTEM_LABEL, "account"), (FETCHER_RESULT_LABEL, result)],
        )?;
        ensure_eq!(actual, expected, "fetcherResult:{result}");
    }
    for (operation, result) in [("get", "ok"), ("start", "notfound")] {
        let actual = counter_vec_value(
            Some(&m.fetcher_backend_fetch),
            &[
                (SUBSYSTEM_LABEL, "account"),
                (OPERATION_LABEL, operation),
                (FETCHER_RESULT_LABEL, result),
            ],
        )?;
        ensure_eq!(actual, 1.0, "fetcherBackendFetch:{result}");
    }
    Ok(())
}

/// Go: `TestCookieSyncMetric`, `TestRecordSyncerRequestMetric`, `TestSetUidMetric` and
/// `TestRecordSyncerSetMetric`.
#[test]
fn cookie_sync_and_set_uid_metrics() -> TestResult {
    for status in [
        CookieSyncStatus::OK,
        CookieSyncStatus::BAD_REQUEST,
        CookieSyncStatus::OPT_OUT,
        CookieSyncStatus::GDPR_HOST_COOKIE_BLOCKED,
    ] {
        let m = create_metrics_for_testing()?;
        m.record_cookie_sync(status);
        let actual = counter_vec_value(Some(&m.cookie_sync), &[(STATUS_LABEL, status.as_str())])?;
        ensure_eq!(actual, 1.0, "cookie_sync_requests:{status}");
    }
    for status in [
        SyncerCookieSyncStatus::OK,
        SyncerCookieSyncStatus::PRIVACY_BLOCKED,
        SyncerCookieSyncStatus::ALREADY_SYNCED,
        SyncerCookieSyncStatus::REJECTED_BY_FILTER,
    ] {
        let m = create_metrics_for_testing()?;
        m.record_syncer_request("anyKey", status);
        let actual = counter_vec_value(
            Some(&m.syncer_requests),
            &[(SYNCER_LABEL, "anyKey"), (STATUS_LABEL, status.as_str())],
        )?;
        ensure_eq!(actual, 1.0, "syncer_requests:{status}");
    }
    for status in [
        SetUidStatus::OK,
        SetUidStatus::BAD_REQUEST,
        SetUidStatus::OPT_OUT,
        SetUidStatus::GDPR_HOST_COOKIE_BLOCKED,
        SetUidStatus::SYNCER_UNKNOWN,
    ] {
        let m = create_metrics_for_testing()?;
        m.record_set_uid(status);
        let actual = counter_vec_value(Some(&m.set_uid), &[(STATUS_LABEL, status.as_str())])?;
        ensure_eq!(actual, 1.0, "setuid_requests:{status}");
    }
    for status in [SyncerSetUidStatus::OK, SyncerSetUidStatus::CLEARED] {
        let m = create_metrics_for_testing()?;
        m.record_syncer_set("anyKey", status);
        let actual = counter_vec_value(
            Some(&m.syncer_sets),
            &[(SYNCER_LABEL, "anyKey"), (STATUS_LABEL, status.as_str())],
        )?;
        ensure_eq!(actual, 1.0, "syncer_sets:{status}");
    }
    Ok(())
}

/// Go: `TestPrebidCacheRequestTimeMetric`.
#[test]
fn prebid_cache_request_time_metric() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_prebid_cache_request_time(true, millis(100));
    m.record_prebid_cache_request_time(false, millis(200));
    let (success, found_success) =
        histogram_from_vec(Some(&m.prebid_cache_write_timer), SUCCESS_LABEL, "true");
    ensure!(found_success, "Success found");
    check_histogram("Success", &success, 1, 0.1)?;
    let (error, found_error) =
        histogram_from_vec(Some(&m.prebid_cache_write_timer), SUCCESS_LABEL, "false");
    ensure!(found_error, "Error found");
    check_histogram("Error", &error, 1, 0.2)?;
    Ok(())
}

/// Go: `TestRecordRequestQueueTimeMetric`.
#[test]
fn record_request_queue_time_metric() -> TestResult {
    let m = create_metrics_for_testing()?;
    for (description, status, success, secs) in [
        ("Success", REQUEST_SUCCESS_LABEL, true, 2),
        ("TimeoutError", REQUEST_REJECT_LABEL, false, 50),
    ] {
        m.record_request_queue_time(success, RequestType::VIDEO, seconds(secs));
        let result = histogram_from_vec_by_two_keys(
            &m.requests_queue_timer,
            (REQUEST_TYPE_LABEL, "video"),
            (REQUEST_STATUS_LABEL, status),
        );
        let sum = float_from_i64(secs);
        check_histogram(description, &result, 1, sum)?;
    }
    Ok(())
}

/// Go: `TestTimeoutNotifications`, `TestRecordLiveGVLFetch` and `TestRecordAdsCertReqMetric`.
#[test]
fn success_labelled_counters() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_timeout_notice(true);
    m.record_timeout_notice(true);
    m.record_timeout_notice(false);
    m.record_live_gvl_fetch(true);
    m.record_live_gvl_fetch(true);
    m.record_live_gvl_fetch(false);
    for (name, family) in [
        ("timeout_notifications", &m.timeout_notifications),
        ("live_gvl_fetch", &m.live_gvl_fetch),
    ] {
        let ok = counter_vec_value(Some(family), &[(SUCCESS_LABEL, REQUEST_SUCCESSFUL)])?;
        let failed = counter_vec_value(Some(family), &[(SUCCESS_LABEL, REQUEST_FAILED)])?;
        ensure_eq!(ok, 2.0, "{name}:ok");
        ensure_eq!(failed, 1.0, "{name}:fail");
    }
    for (request_success, expected_ok, expected_failed) in [(false, 0.0, 1.0), (true, 1.0, 0.0)] {
        let signer = create_metrics_for_testing()?;
        signer.record_ads_cert_req(request_success);
        let ok = counter_vec_value(
            Some(&signer.ads_cert_requests),
            &[(SUCCESS_LABEL, REQUEST_SUCCESSFUL)],
        )?;
        let failed = counter_vec_value(
            Some(&signer.ads_cert_requests),
            &[(SUCCESS_LABEL, REQUEST_FAILED)],
        )?;
        ensure_eq!(
            ok,
            expected_ok,
            "successfully signed requests ({request_success})"
        );
        ensure_eq!(
            failed,
            expected_failed,
            "unsuccessfully signed requests ({request_success})"
        );
    }
    Ok(())
}

/// Go: `TestRecordDNSTime`, `TestRecordTLSHandshakeTime`, `TestRecordBidderServerResponseTime`
/// and `TestRecordAdsCertSignTime`.
#[test]
fn unlabelled_timers() -> TestResult {
    type Case = (
        &'static str,
        fn(&Metrics, GoDuration),
        fn(&Metrics) -> &Histogram,
    );
    let timers: [Case; 4] = [
        (
            "dns_lookup_time",
            |m, d| m.record_dns_time(d),
            |m| &m.dns_lookup_timer,
        ),
        (
            "tls_handshake_time",
            |m, d| m.record_tls_handshake_time(d),
            |m| &m.tls_handshake_timer,
        ),
        (
            "bidder_server_response_time_seconds",
            |m, d| m.record_bidder_server_response_time(d),
            |m| &m.bidder_server_response_timer,
        ),
        (
            "ads_cert_sign_time",
            |m, d| m.record_ads_cert_sign_time(d),
            |m| &m.ads_cert_sign_timer,
        ),
    ];
    let durations = [
        (seconds(5), 5.0),
        (GoDuration::default(), 0.0),
        (millis(5), 0.005),
        (millis(500), 0.5),
        (millis(400), 0.4),
    ];
    for (name, record, timer) in timers {
        for (duration, expected_sum) in durations {
            let m = create_metrics_for_testing()?;
            record(&m, duration);
            let histogram = plain_histogram(timer(&m));
            check_histogram(&format!("{name} {duration:?}"), &histogram, 1, expected_sum)?;
        }
    }
    Ok(())
}

/// Go: `TestRecordAdapterConnections`.
#[test]
fn record_adapter_connections() -> TestResult {
    let cases = [
        (
            "[1] new connection created, has connection wait",
            false,
            seconds(5),
            [0.0, 1.0],
            5.0,
        ),
        (
            "[2] new connection created, has connection wait",
            false,
            seconds(4),
            [0.0, 1.0],
            4.0,
        ),
        (
            "[3] was reused, no connection wait",
            true,
            GoDuration::default(),
            [1.0, 0.0],
            0.0,
        ),
        (
            "[4] was reused, has connection wait",
            true,
            seconds(5),
            [1.0, 0.0],
            5.0,
        ),
    ];
    for (description, conn_was_reused, conn_wait, [reused, created], wait_time) in cases {
        let m = create_metrics_for_testing()?;
        m.record_adapter_connections("Adapter", conn_was_reused, conn_wait);
        let actual_reused = counter_vec_value(
            m.adapter_reused_connections.as_ref(),
            &[(ADAPTER_LABEL, "adapter")],
        )?;
        let actual_created = counter_vec_value(
            m.adapter_created_connections.as_ref(),
            &[(ADAPTER_LABEL, "adapter")],
        )?;
        ensure_eq!(
            actual_reused,
            reused,
            "{description}: adapter_connection_reused"
        );
        ensure_eq!(
            actual_created,
            created,
            "{description}: adapter_connection_created"
        );
        let (histogram, found) = histogram_from_vec(
            m.adapter_connection_wait_time.as_ref(),
            ADAPTER_LABEL,
            "adapter",
        );
        ensure!(found, "{description}: wait found");
        check_histogram(description, &histogram, 1, wait_time)?;
    }
    Ok(())
}

/// Go: `TestDisabledMetrics`.
#[test]
fn disabled_metrics() -> TestResult {
    let disabled = DisabledMetrics::NONE
        .with(DisabledMetric::AdapterBuyerUidScrubbed, true)
        .with(DisabledMetric::AdapterConnectionMetrics, true)
        .with(DisabledMetric::AdapterGdprRequestBlocked, true);
    let m = Metrics::new(
        &testing_config(),
        disabled,
        &[],
        &BTreeMap::new(),
        &core_bidder_names()?,
    )?;
    ensure!(
        m.adapter_reused_connections.is_none(),
        "adapterReusedConnections should be nil"
    );
    ensure!(
        m.adapter_scrubbed_buyer_uids.is_none(),
        "adapterScrubbedBuyerUIDs should be nil"
    );
    ensure!(
        m.adapter_created_connections.is_none(),
        "adapterCreatedConnections should be nil"
    );
    ensure!(
        m.adapter_connection_wait_time.is_none(),
        "adapterConnectionWaitTime should be nil"
    );
    ensure!(
        m.adapter_gdpr_blocked_requests.is_none(),
        "adapterGDPRBlockedRequests should be nil"
    );
    // The dial families are registered only inside the connection metrics block.
    ensure!(
        m.adapter_connection_dial_errors.is_none(),
        "adapterConnectionDialErrors nil"
    );
    ensure!(
        m.adapter_connection_dial_time.is_none(),
        "adapterConnectionDialTime nil"
    );
    // Recording into a family Go leaves nil records nothing.
    m.record_adapter_connection_dial_error("appnexus");
    m.record_adapter_connection_dial_time("appnexus", seconds(1));
    m.record_adapter_connections("appnexus", true, seconds(1));
    let names: Vec<String> = m
        .gatherer()
        .gather()
        .iter()
        .map(|f| f.name().to_owned())
        .collect();
    ensure!(
        !names.iter().any(|n| n.contains("adapter_connection")),
        "{names:?}"
    );
    Ok(())
}

/// Go: `TestRecordRequestPrivacy`.
#[test]
fn record_request_privacy() -> TestResult {
    let m = create_metrics_for_testing()?;
    let ccpa = |enforced, provided| PrivacyLabels {
        ccpa: CcpaLabels { enforced, provided },
        ..PrivacyLabels::default()
    };
    let gdpr = |tcf_version| PrivacyLabels {
        gdpr: GdprLabels {
            enforced: true,
            tcf_version,
        },
        ..PrivacyLabels::default()
    };
    m.record_request_privacy(ccpa(true, true));
    m.record_request_privacy(ccpa(true, false));
    m.record_request_privacy(ccpa(false, true));
    m.record_request_privacy(PrivacyLabels {
        coppa_enforced: true,
        ..PrivacyLabels::default()
    });
    m.record_request_privacy(PrivacyLabels {
        lmt_enforced: true,
        ..PrivacyLabels::default()
    });
    m.record_request_privacy(gdpr(TcfVersionValue::ERR));
    m.record_request_privacy(gdpr(TcfVersionValue::V2));

    let source = (SOURCE_LABEL, SOURCE_REQUEST);
    for (name, family, labels) in [
        (
            "privacy_ccpa:true",
            &m.privacy_ccpa,
            vec![source, (OPT_OUT_LABEL, "true")],
        ),
        (
            "privacy_ccpa:false",
            &m.privacy_ccpa,
            vec![source, (OPT_OUT_LABEL, "false")],
        ),
        ("privacy_coppa", &m.privacy_coppa, vec![source]),
        ("privacy_lmt", &m.privacy_lmt, vec![source]),
        (
            "privacy_tcf:err",
            &m.privacy_tcf,
            vec![source, (VERSION_LABEL, "err")],
        ),
        (
            "privacy_tcf:v2",
            &m.privacy_tcf,
            vec![source, (VERSION_LABEL, "v2")],
        ),
    ] {
        ensure_eq!(counter_vec_value(Some(family), &labels)?, 1.0, "{name}");
    }
    Ok(())
}

/// Go: `TestRecordAdapterBuyerUIDScrubbed` and `TestRecordAdapterGDPRRequestBlocked`.
#[test]
fn record_adapter_privacy_counters() -> TestResult {
    for (disabled, expected) in [(false, 1.0), (true, 0.0)] {
        let mut m = create_metrics_for_testing()?;
        m.disabled_flags
            .set(DisabledMetric::AdapterBuyerUidScrubbed, disabled);
        m.record_adapter_buyer_uid_scrubbed("AnyName");
        let actual = counter_vec_value(
            m.adapter_scrubbed_buyer_uids.as_ref(),
            &[(ADAPTER_LABEL, "anyname")],
        )?;
        ensure_eq!(
            actual,
            expected,
            "adapter_buyeruids_scrubbed (disabled={disabled})"
        );
    }
    let m = create_metrics_for_testing()?;
    m.record_adapter_gdpr_request_blocked("AnyName");
    let actual = counter_vec_value(
        m.adapter_gdpr_blocked_requests.as_ref(),
        &[(ADAPTER_LABEL, "anyname")],
    )?;
    ensure_eq!(actual, 1.0, "adapter_gdpr_requests_blocked");
    Ok(())
}

/// Go: `TestStoredResponsesMetric`.
#[test]
fn stored_responses_metric() -> TestResult {
    let cases = [
        ("acct-id", false, 1.0),
        ("acct-id", true, 0.0),
        (PUBLISHER_UNKNOWN, true, 0.0),
        (PUBLISHER_UNKNOWN, false, 0.0),
    ];
    for (publisher_id, account_stored_responses_disabled, expected_account) in cases {
        let mut m = create_metrics_for_testing()?;
        m.disabled_flags.set(
            DisabledMetric::AccountStoredResponses,
            account_stored_responses_disabled,
        );
        m.record_stored_response(publisher_id);
        let account = counter_vec_value(
            Some(&m.account_stored_responses),
            &[(ACCOUNT_LABEL, "acct-id")],
        )?;
        let description = format!("{publisher_id} disabled={account_stored_responses_disabled}");
        ensure_eq!(
            account,
            expected_account,
            "{description}: account stored responses"
        );
        ensure_eq!(
            counter_value(&m.stored_responses),
            1.0,
            "{description}: stored responses"
        );
    }
    Ok(())
}

/// Go: `TestRecordGvlListRequest`.
#[test]
fn record_gvl_list_request() -> TestResult {
    let m = create_metrics_for_testing()?;
    m.record_gvl_list_request();
    ensure_eq!(
        counter_value(&m.gvl_list_requests),
        1.0,
        "Record instance of fetched GVL list"
    );
    Ok(())
}

/// Go: `TestRecordModuleMetrics`.
#[test]
fn record_module_metrics() -> TestResult {
    let m = create_metrics_for_testing()?;
    for (module, stages) in modules_stages() {
        for stage in stages {
            let labels = ModuleLabels {
                module: module.clone(),
                stage: stage.clone(),
                ..ModuleLabels::default()
            };
            m.record_module_called(&labels, millis(1));
            m.record_module_failed(&labels);
            m.record_module_success_nooped(&labels);
            m.record_module_success_updated(&labels);
            m.record_module_success_rejected(&labels);
            m.record_module_execution_error(&labels);
            m.record_module_timeout(&labels);

            let (result, found) =
                histogram_from_vec(m.modules.duration.get(&module), STAGE_LABEL, &stage);
            ensure!(found, "module_{module}_duration found");
            check_histogram(&format!("module_{module}_duration"), &result, 1, 0.001)?;
            for (name, families) in [
                ("Module calls performed", &m.modules.calls),
                ("Module calls failed", &m.modules.failures),
                ("Module success noop action", &m.modules.success_noops),
                ("Module success update action", &m.modules.success_updates),
                ("Module success reject action", &m.modules.success_rejects),
                ("Module execution error", &m.modules.execution_errors),
                ("Module timeout", &m.modules.timeouts),
            ] {
                let actual = counter_vec_value(families.get(&module), &[(STAGE_LABEL, &stage)])?;
                ensure_eq!(
                    actual,
                    1.0,
                    "{name}: {module} metric recorded during {stage} stage"
                );
            }
        }
    }
    // Go panics on a module that was not registered; the port records nothing.
    m.record_module_failed(&ModuleLabels {
        module: "unknown".to_owned(),
        ..ModuleLabels::default()
    });
    Ok(())
}

/// Go's `MustRegister` panics on an invalid metric name; the port returns the prometheus crate's
/// error for the same family (ledger: `metrics.registration-errors`).
// Go: metrics/prometheus/prometheus.go:190-192 (the first family) and :660-670
// (newCounterWithoutLabels' MustRegister); client_golang v1.12.1 prometheus/desc.go:85-88
#[test]
fn invalid_namespace_is_a_registration_error() -> TestResult {
    let cfg = PrometheusMetrics {
        namespace: "1bad".to_owned(),
        ..testing_config()
    };
    let Err(err) = Metrics::new(&cfg, DisabledMetrics::NONE, &[], &BTreeMap::new(), &[]) else {
        return Err("an invalid namespace must not register".into());
    };
    ensure_eq!(
        err.to_string(),
        "Error: '1bad_server_connections_closed' is not a valid metric name",
        "registration error"
    );
    Ok(())
}
