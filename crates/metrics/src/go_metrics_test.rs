//! Go: `metrics/go_metrics_test.go`.

use std::collections::{BTreeMap, BTreeSet};

use super::{AdapterMetrics, MarkupDeliveryMetrics, Metrics, ModuleMetrics, new_metrics};
use crate::{
    gometrics::{Metric, Registry},
    metrics::{
        AdapterBid, AdapterError, AdapterLabels, CookieSyncStatus, EndpointType, GoDuration,
        MetricsEngine as _, ModuleLabels, OverheadType, PUBLISHER_UNKNOWN, PrivacyLabels,
        RequestStatus, RequestType, SetUidStatus, StoredDataError, StoredDataFetchType,
        StoredDataLabels, StoredDataType, SyncerCookieSyncStatus, SyncerSetUidStatus,
        TcfVersionValue, adapter_errors,
    },
    options::{DisabledMetric, DisabledMetrics},
    testutil::TestResult,
};

/// Go: `NewMetrics(registry, exchanges, disabled, syncerKeys, moduleStageNames)` with the test's
/// literal arguments.
fn new_test_metrics(
    registry: &Registry,
    exchanges: &[&str],
    disabled: DisabledMetrics,
    syncer_keys: &[&str],
    modules: &[(&str, &[&str])],
) -> Metrics {
    let exchanges: Vec<String> = exchanges.iter().map(|&e| e.to_owned()).collect();
    let syncer_keys: Vec<String> = syncer_keys.iter().map(|&k| k.to_owned()).collect();
    let module_stage_names: BTreeMap<String, Vec<String>> = modules
        .iter()
        .map(|&(module, stages)| {
            let stages = stages.iter().map(|&s| s.to_owned()).collect();
            (module.to_owned(), stages)
        })
        .collect();
    new_metrics(
        registry.clone(),
        &exchanges,
        disabled,
        &syncer_keys,
        &module_stage_names,
    )
}

/// Go: `config.DisabledMetrics{<flag>: true}`.
fn disabled(flags: &[DisabledMetric]) -> DisabledMetrics {
    flags
        .iter()
        .fold(DisabledMetrics::NONE, |d, &flag| d.with(flag, true))
}

/// The metrics of adapter `name` (Go: `m.AdapterMetrics[name]`).
fn adapter<'a>(m: &'a Metrics, name: &str) -> Result<&'a AdapterMetrics, String> {
    m.adapters
        .get(name)
        .ok_or_else(|| format!("no adapter metrics for {name}"))
}

const MODULE_STAGES: [(&str, &[&str]); 2] = [
    ("foobar", &["entry", "raw"]),
    ("another_module", &["raw", "auction"]),
];

/// Go: `ensureContains`: `name` is registered, and holds `metric`.
fn ensure_contains(registry: &Registry, name: &str, metric: Option<Metric>) -> TestResult {
    let Some(in_registry) = registry.get(name) else {
        return Err(format!("No metric in registry at {name}.").into());
    };
    ensure!(
        metric.is_some_and(|m| m == in_registry),
        "Bad value stored at metric {name}."
    );
    Ok(())
}

/// Go: `ensureContainsAdapterMetrics`.
fn ensure_contains_adapter_metrics(
    registry: &Registry,
    name: &str,
    am: &AdapterMetrics,
) -> TestResult {
    let error_meter = |err: AdapterError| am.error_meters.get(&err).map(Option::<Metric>::from);
    let expected: Vec<(&str, Option<Metric>)> = vec![
        ("no_cookie_requests", (&am.no_cookie_meter).into()),
        ("requests.gotbids", (&am.got_bids_meter).into()),
        ("requests.nobid", (&am.no_bid_meter).into()),
        (
            "requests.badinput",
            error_meter(AdapterError::BAD_INPUT).flatten(),
        ),
        (
            "requests.badserverresponse",
            error_meter(AdapterError::BAD_SERVER_RESPONSE).flatten(),
        ),
        (
            "requests.timeout",
            error_meter(AdapterError::TIMEOUT).flatten(),
        ),
        (
            "requests.unknown_error",
            error_meter(AdapterError::UNKNOWN).flatten(),
        ),
        ("request_time", (&am.request_timer).into()),
        ("prices", (&am.price_histogram).into()),
        ("connections_created", (&am.conn_created).into()),
        ("connections_reused", (&am.conn_reused).into()),
        ("connection_wait_time", (&am.conn_wait_time).into()),
        ("connection_dial_err", (&am.conn_dial_errors).into()),
        ("connection_dial_time", (&am.conn_dial_time).into()),
        (
            "response.validation.size.err",
            (&am.bid_validation_creative_size_error_meter).into(),
        ),
        (
            "response.validation.size.warn",
            (&am.bid_validation_creative_size_warn_meter).into(),
        ),
        (
            "response.validation.secure.err",
            (&am.bid_validation_secure_markup_error_meter).into(),
        ),
        (
            "response.validation.secure.warn",
            (&am.bid_validation_secure_markup_warn_meter).into(),
        ),
    ];
    for (suffix, metric) in expected {
        ensure_contains(registry, &format!("{name}.{suffix}"), metric)?;
    }
    ensure_contains_bid_type_metrics(registry, name, &am.markup_metrics)
}

/// Go: `ensureContainsBidTypeMetrics`.
fn ensure_contains_bid_type_metrics(
    registry: &Registry,
    prefix: &str,
    mdm: &BTreeMap<String, MarkupDeliveryMetrics>,
) -> TestResult {
    for bid_type in ["banner", "video", "audio", "native"] {
        let metrics = mdm
            .get(bid_type)
            .ok_or_else(|| format!("no {bid_type} markup metrics"))?;
        ensure_contains(
            registry,
            &format!("{prefix}.{bid_type}.adm_bids_received"),
            (&metrics.adm_meter).into(),
        )?;
        ensure_contains(
            registry,
            &format!("{prefix}.{bid_type}.nurl_bids_received"),
            (&metrics.nurl_meter).into(),
        )?;
    }
    Ok(())
}

/// Go: `ensureContainsModuleMetrics`.
fn ensure_contains_module_metrics(
    registry: &Registry,
    name: &str,
    mm: &ModuleMetrics,
) -> TestResult {
    let expected: [(&str, Option<Metric>); 8] = [
        ("duration", (&mm.duration_timer).into()),
        ("call", (&mm.call_counter).into()),
        ("failure", (&mm.failure_counter).into()),
        ("success.noop", (&mm.success_noop_counter).into()),
        ("success.update", (&mm.success_update_counter).into()),
        ("success.reject", (&mm.success_reject_counter).into()),
        ("execution_error", (&mm.execution_error_counter).into()),
        ("timeout", (&mm.timeout_counter).into()),
    ];
    for (suffix, metric) in expected {
        ensure_contains(registry, &format!("{name}.{suffix}"), metric)?;
    }
    Ok(())
}

/// The scalar metrics `TestNewMetrics` checks.
fn new_metrics_scalars(m: &Metrics) -> Vec<(&'static str, Option<Metric>)> {
    vec![
        ("app_requests", (&m.app_request_meter).into()),
        ("debug_requests", (&m.debug_request_meter).into()),
        ("no_cookie_requests", (&m.no_cookie_meter).into()),
        ("request_time", (&m.request_timer).into()),
        ("cookie_sync_requests", (&m.cookie_sync_meter).into()),
        ("setuid_requests", (&m.set_uid_meter).into()),
        ("stored_responses", (&m.stored_responses_meter).into()),
        ("gvl_requests", (&m.gvl_list_requests_meter).into()),
        ("live_gvl_fetch.ok", (&m.live_gvl_fetch_success).into()),
        ("live_gvl_fetch.failed", (&m.live_gvl_fetch_failure).into()),
        (
            "prebid_cache_request_time.ok",
            (&m.prebid_cache_request_timer_success).into(),
        ),
        (
            "prebid_cache_request_time.err",
            (&m.prebid_cache_request_timer_error).into(),
        ),
        (
            "timeout_notification.ok",
            (&m.timeout_notification_success).into(),
        ),
        (
            "timeout_notification.failed",
            (&m.timeout_notification_failure).into(),
        ),
        (
            "privacy.request.ccpa.specified",
            (&m.privacy_ccpa_request).into(),
        ),
        (
            "privacy.request.ccpa.opt-out",
            (&m.privacy_ccpa_request_opt_out).into(),
        ),
        ("privacy.request.coppa", (&m.privacy_coppa_request).into()),
        ("privacy.request.lmt", (&m.privacy_lmt_request).into()),
        (
            "ads_cert_requests.ok",
            (&m.ads_cert_requests_success).into(),
        ),
        (
            "ads_cert_requests.failed",
            (&m.ads_cert_requests_failure).into(),
        ),
        (
            "bidder_server_response_time_seconds",
            (&m.bidder_server_response_timer).into(),
        ),
        ("tmax_timeout", (&m.tmax_timeout_counter).into()),
    ]
}

/// The keyed metrics `TestNewMetrics` checks: by status, request type, endpoint and so on.
fn new_metrics_keyed(m: &Metrics) -> Vec<(String, Option<Metric>)> {
    let mut expected = Vec::new();
    for (name, status) in [
        ("ok", CookieSyncStatus::OK),
        ("bad_request", CookieSyncStatus::BAD_REQUEST),
        ("opt_out", CookieSyncStatus::OPT_OUT),
        (
            "gdpr_blocked_host_cookie",
            CookieSyncStatus::GDPR_HOST_COOKIE_BLOCKED,
        ),
    ] {
        let meter = m.cookie_sync_status_meter.get(&status);
        expected.push((
            format!("cookie_sync_requests.{name}"),
            meter.and_then(Option::<Metric>::from),
        ));
    }
    for (name, status) in [
        ("ok", SetUidStatus::OK),
        ("bad_request", SetUidStatus::BAD_REQUEST),
        ("opt_out", SetUidStatus::OPT_OUT),
        (
            "gdpr_blocked_host_cookie",
            SetUidStatus::GDPR_HOST_COOKIE_BLOCKED,
        ),
        ("syncer_unknown", SetUidStatus::SYNCER_UNKNOWN),
    ] {
        let meter = m.set_uid_status_meter.get(&status);
        expected.push((
            format!("setuid_requests.{name}"),
            meter.and_then(Option::<Metric>::from),
        ));
    }
    for r_type in [
        RequestType::ORTB2_WEB,
        RequestType::ORTB2_APP,
        RequestType::AMP,
        RequestType::VIDEO,
    ] {
        for status in [
            RequestStatus::OK,
            RequestStatus::BAD_INPUT,
            RequestStatus::ERR,
            RequestStatus::NETWORK_ERR,
        ] {
            let meter = m
                .request_statuses
                .get(&r_type)
                .and_then(|statuses| statuses.get(&status));
            expected.push((
                format!("requests.{status}.{r_type}"),
                meter.and_then(Option::<Metric>::from),
            ));
        }
    }
    for endpoint in [EndpointType::AUCTION, EndpointType::VIDEO] {
        let histogram = m.request_size_by_endpoint.get(&endpoint);
        expected.push((
            format!("requests.size.{endpoint}"),
            histogram.and_then(Option::<Metric>::from),
        ));
    }
    new_metrics_keyed_rest(m, &mut expected);
    expected
}

/// The rest of [`new_metrics_keyed`].
fn new_metrics_keyed_rest(m: &Metrics, expected: &mut Vec<(String, Option<Metric>)>) {
    let video = m.requests_queue_timer.get(&RequestType::VIDEO);
    for (name, accepted) in [("rejected", false), ("accepted", true)] {
        let timer = video.and_then(|timers| timers.get(&accepted));
        expected.push((
            format!("queued_requests.video.{name}"),
            timer.and_then(Option::<Metric>::from),
        ));
    }
    for version in [TcfVersionValue::V2, TcfVersionValue::ERR] {
        let meter = m.privacy_tcf_request_version.get(&version);
        expected.push((
            format!("privacy.request.tcf.{version}"),
            meter.and_then(Option::<Metric>::from),
        ));
    }
    let requests = m.syncer_requests_meter.get("foo");
    for status in [
        SyncerCookieSyncStatus::OK,
        SyncerCookieSyncStatus::PRIVACY_BLOCKED,
        SyncerCookieSyncStatus::ALREADY_SYNCED,
        SyncerCookieSyncStatus::REJECTED_BY_FILTER,
    ] {
        let meter = requests.and_then(|meters| meters.get(&status));
        expected.push((
            format!("syncer.foo.request.{status}"),
            meter.and_then(Option::<Metric>::from),
        ));
    }
    let sets = m.syncer_sets_meter.get("foo");
    for status in [SyncerSetUidStatus::OK, SyncerSetUidStatus::CLEARED] {
        let meter = sets.and_then(|meters| meters.get(&status));
        expected.push((
            format!("syncer.foo.set.{status}"),
            meter.and_then(Option::<Metric>::from),
        ));
    }
    for overhead in [
        OverheadType::PRE_BIDDER,
        OverheadType::MAKE_AUCTION_RESPONSE,
        OverheadType::MAKE_BIDDER_REQUESTS,
    ] {
        let timer = m.overhead_timer.get(&overhead);
        expected.push((
            format!("request_over_head_time.{overhead}"),
            timer.and_then(Option::<Metric>::from),
        ));
    }
}

/// Go: `TestNewMetrics`.
#[test]
fn test_new_metrics() -> TestResult {
    // Go: metrics/go_metrics_test.go:14 (TestNewMetrics)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["Adapter1", "Adapter2"],
        DisabledMetrics::NONE,
        &["foo"],
        &MODULE_STAGES,
    );

    for (name, metric) in new_metrics_scalars(&m) {
        ensure_contains(&registry, name, metric)?;
    }
    // Go: AmpNoCookieMeter, which the port registers without keeping.
    ensure!(
        matches!(
            registry.get("amp_no_cookie_requests"),
            Some(Metric::Meter(_))
        ),
        "amp_no_cookie_requests is a registered meter"
    );
    for (name, metric) in new_metrics_keyed(&m) {
        ensure_contains(&registry, &name, metric)?;
    }
    ensure_contains_adapter_metrics(&registry, "adapter.adapter1", adapter(&m, "adapter1")?)?;
    ensure_contains_adapter_metrics(&registry, "adapter.adapter2", adapter(&m, "adapter2")?)?;

    for (module, stages) in MODULE_STAGES {
        for &stage in stages {
            let mm = m
                .module_stages
                .get(module)
                .and_then(|s| s.get(stage))
                .ok_or_else(|| format!("no module metrics for {module} {stage}"))?;
            let name = format!("modules.module.{module}.stage.{stage}");
            ensure_contains_module_metrics(&registry, &name, mm)?;
        }
    }
    Ok(())
}

/// Go: `TestRecordBidType`.
#[test]
fn test_record_bid_type() -> TestResult {
    // Go: metrics/go_metrics_test.go:101 (TestRecordBidType)
    let registry = Registry::new();
    let adapter_name = "FOO";
    let lower_case_adapter_name = "foo";
    let m = new_test_metrics(&registry, &[adapter_name], DisabledMetrics::NONE, &[], &[]);
    let labels = AdapterLabels {
        adapter: adapter_name.to_owned(),
        ..AdapterLabels::default()
    };
    let markup = |bid_type: &str| -> Result<(i64, i64), String> {
        let metrics = adapter(&m, lower_case_adapter_name)?
            .markup_metrics
            .get(bid_type)
            .ok_or_else(|| format!("no {bid_type} markup metrics"))?;
        Ok((metrics.adm_meter.count(), metrics.nurl_meter.count()))
    };

    m.record_adapter_bid_received(&labels, "banner", true);
    ensure_eq!(markup("banner")?, (1, 0), "foo Banner Adm/Nurl Bids");

    m.record_adapter_bid_received(&labels, "video", false);
    ensure_eq!(markup("video")?, (0, 1), "foo Video Adm/Nurl Bids");
    Ok(())
}

/// Go: `TestRecordBidTypeDisabledConfig`.
#[test]
fn test_record_bid_type_disabled_config() -> TestResult {
    // Go: metrics/go_metrics_test.go:168 (TestRecordBidTypeDisabledConfig)
    let account_details = disabled(&[DisabledMetric::AccountAdapterDetails]);
    let test_cases = [
        (true, DisabledMetrics::NONE, 1, 0, "banner"),
        (false, DisabledMetrics::NONE, 0, 1, "video"),
        (false, account_details, 0, 1, "video"),
        (true, account_details, 1, 0, "banner"),
    ];
    let adapter_name = "AnyName";
    let lower_case_adapter = "anyname";
    let pub_id = "acct-id";
    for (has_adm, disabled_metrics, expected_adm, expected_nurl, bid_type) in test_cases {
        let registry = Registry::new();
        let m = new_test_metrics(&registry, &[adapter_name], disabled_metrics, &[], &[]);

        let labels = AdapterLabels {
            adapter: adapter_name.to_owned(),
            pub_id: pub_id.to_owned(),
            ..AdapterLabels::default()
        };
        m.record_adapter_bid_received(&labels, bid_type, has_adm);
        let markup = adapter(&m, lower_case_adapter)?
            .markup_metrics
            .get(bid_type)
            .ok_or("no markup metrics")?;
        ensure_eq!(
            markup.adm_meter.count(),
            expected_adm,
            "AnyName Banner Adm Bids"
        );
        ensure_eq!(
            markup.nurl_meter.count(),
            expected_nurl,
            "AnyName Banner Nurl Bids"
        );

        let account_adapters = m.get_account_metrics(pub_id).adapters.len();
        if disabled_metrics.is(DisabledMetric::AccountAdapterDetails) {
            ensure_eq!(account_adapters, 0, "no account adapter metrics");
        } else {
            ensure!(account_adapters != 0, "account adapter metrics");
        }
    }
    Ok(())
}

/// Go: `TestRecordDebugRequest`.
#[test]
fn test_record_debug_request() -> TestResult {
    // Go: metrics/go_metrics_test.go:232 (TestRecordDebugRequest)
    let account_details = DisabledMetric::AccountAdapterDetails;
    let debug_on = disabled(&[account_details]);
    let debug_off = disabled(&[account_details, DisabledMetric::AccountDebug]);
    let test_cases = [
        (debug_on, true, "acct-id", 1, 1),
        (debug_off, false, "acct-id", 0, 0),
        (debug_on, true, PUBLISHER_UNKNOWN, 0, 1),
        (debug_on, false, "acct-id", 0, 0),
    ];
    for (disabled_metrics, debug_enabled, pub_id, expected_account, expected_debug) in test_cases {
        let registry = Registry::new();
        let m = new_test_metrics(&registry, &["AnyName"], disabled_metrics, &[], &[]);

        m.record_debug_request(debug_enabled, pub_id);
        let am = m.get_account_metrics(pub_id);

        ensure_eq!(m.debug_request_meter.count(), expected_debug, "{pub_id}");
        ensure_eq!(am.debug_request_meter.count(), expected_account, "{pub_id}");
    }
    Ok(())
}

/// Go: `TestRecordBidValidationCreativeSize`.
#[test]
fn test_record_bid_validation_creative_size() -> TestResult {
    // Go: metrics/go_metrics_test.go:299 (TestRecordBidValidationCreativeSize)
    let test_cases = [
        (DisabledMetrics::NONE, "acct-id", 1, 1),
        (
            disabled(&[DisabledMetric::AccountAdapterDetails]),
            "acct-id",
            0,
            1,
        ),
    ];
    let adapter_name = "AnyName";
    let lower_case_adapter = "anyname";
    for (disabled_metrics, pub_id, expected_account, expected_adapter) in test_cases {
        let registry = Registry::new();
        let m = new_test_metrics(&registry, &[adapter_name], disabled_metrics, &[], &[]);

        m.record_bid_validation_creative_size_error(adapter_name, pub_id);
        m.record_bid_validation_creative_size_warn(adapter_name, pub_id);
        let am = m.get_account_metrics(pub_id);

        let a = adapter(&m, lower_case_adapter)?;
        ensure_eq!(
            a.bid_validation_creative_size_error_meter.count(),
            expected_adapter,
            "adapter size error"
        );
        ensure_eq!(
            a.bid_validation_creative_size_warn_meter.count(),
            expected_adapter,
            "adapter size warn"
        );
        ensure_eq!(
            am.bid_validation_creative_size_meter.count(),
            expected_account,
            "account size error"
        );
        ensure_eq!(
            am.bid_validation_creative_size_warn_meter.count(),
            expected_account,
            "account size warn"
        );
    }
    Ok(())
}

/// Go: `TestRecordBidValidationSecureMarkup`.
#[test]
fn test_record_bid_validation_secure_markup() -> TestResult {
    // Go: metrics/go_metrics_test.go:343 (TestRecordBidValidationSecureMarkup)
    let test_cases = [
        (DisabledMetrics::NONE, "acct-id", 1, 1),
        (
            disabled(&[DisabledMetric::AccountAdapterDetails]),
            "acct-id",
            0,
            1,
        ),
    ];
    let adapter_name = "AnyName";
    let lower_case_adapter = "anyname";
    for (disabled_metrics, pub_id, expected_account, expected_adapter) in test_cases {
        let registry = Registry::new();
        let m = new_test_metrics(&registry, &[adapter_name], disabled_metrics, &[], &[]);

        m.record_bid_validation_secure_markup_error(adapter_name, pub_id);
        m.record_bid_validation_secure_markup_warn(adapter_name, pub_id);
        let am = m.get_account_metrics(pub_id);

        let a = adapter(&m, lower_case_adapter)?;
        ensure_eq!(
            a.bid_validation_secure_markup_error_meter.count(),
            expected_adapter,
            "adapter secure error"
        );
        ensure_eq!(
            a.bid_validation_secure_markup_warn_meter.count(),
            expected_adapter,
            "adapter secure warn"
        );
        ensure_eq!(
            am.bid_validation_secure_markup_meter.count(),
            expected_account,
            "account secure error"
        );
        ensure_eq!(
            am.bid_validation_secure_markup_warn_meter.count(),
            expected_account,
            "account secure warn"
        );
    }
    Ok(())
}

/// Go: `TestRecordDNSTime`, `TestRecordTLSHandshakeTime`, `TestRecordBidderServerResponseTime`
/// and `TestRecordAdsCertSignTime`: each timer's sum is the duration recorded.
#[test]
fn test_record_timers() -> TestResult {
    // Go: metrics/go_metrics_test.go:387, :415 and :443 (the DNS, TLS and server response times)
    let second = GoDuration::SECOND;
    let durations = [
        GoDuration::times(5, second),
        GoDuration::from_nanos(0),
        GoDuration::from_nanos(500),
        GoDuration::times(5, GoDuration::MILLISECOND),
    ];
    for d in durations {
        let registry = Registry::new();
        let m = new_test_metrics(
            &registry,
            &["AnyName"],
            disabled(&[DisabledMetric::AccountAdapterDetails]),
            &[],
            &[],
        );
        m.record_dns_time(d);
        m.record_tls_handshake_time(d);
        m.record_bidder_server_response_time(d);
        m.record_ads_cert_sign_time(d);
        for (name, timer) in [
            ("DNSLookupTimer", &m.dns_lookup_timer),
            ("TLSHandshakeTimer", &m.tls_handshake_timer),
            ("BidderServerResponseTimer", &m.bidder_server_response_timer),
            ("adsCertSignTimer", &m.ads_cert_sign_timer),
        ] {
            ensure_eq!(timer.snapshot().sum(), d.as_nanos(), "{name}");
        }
    }
    Ok(())
}

/// Go: `TestRecordAdapterConnections`.
#[test]
fn test_record_adapter_connections() -> TestResult {
    // Go: metrics/go_metrics_test.go:474 (TestRecordAdapterConnections)
    let fake_bidder = "fooAdvertising";
    let adapter_name = "AnyName";
    let lower_case_adapter_name = "anyname";
    let s = |n| GoDuration::times(n, GoDuration::SECOND);
    // (adapter, reused, wait, metrics disabled) -> (reused, created, wait)
    let test_cases = [
        ((adapter_name, false, s(5), false), (0, 1, s(5))),
        ((adapter_name, false, s(4), false), (0, 1, s(4))),
        ((adapter_name, true, s(0), false), (1, 0, s(0))),
        ((adapter_name, true, s(5), false), (1, 0, s(5))),
        ((fake_bidder, false, s(0), false), (0, 0, s(0))),
        ((adapter_name, false, s(5), true), (0, 0, s(0))),
    ];
    for (i, ((name, reused, wait, metrics_disabled), (exp_reused, exp_created, exp_wait))) in
        test_cases.into_iter().enumerate()
    {
        let registry = Registry::new();
        let flags =
            DisabledMetrics::NONE.with(DisabledMetric::AdapterConnectionMetrics, metrics_disabled);
        let m = new_test_metrics(&registry, &[adapter_name], flags, &[], &[]);

        m.record_adapter_connections(name, reused, wait);
        let a = adapter(&m, lower_case_adapter_name)?;
        ensure_eq!(
            a.conn_reused.count(),
            exp_reused,
            "Test [{i}] incorrect number of reused connections to adapter"
        );
        ensure_eq!(
            a.conn_created.count(),
            exp_created,
            "Test [{i}] incorrect number of new connections to adapter created"
        );
        ensure_eq!(
            a.conn_wait_time.snapshot().sum(),
            exp_wait.as_nanos(),
            "Test [{i}] incorrect wait time in connection to adapter"
        );
    }
    Ok(())
}

/// Go: `TestNewMetricsWithDisabledConfig`.
#[test]
fn test_new_metrics_with_disabled_config() -> TestResult {
    // Go: metrics/go_metrics_test.go:581 (TestNewMetricsWithDisabledConfig)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["Foo", "bar"],
        disabled(&[
            DisabledMetric::AccountAdapterDetails,
            DisabledMetric::AccountModulesMetrics,
        ]),
        &[],
        &[("foobar", &["entry", "raw"])],
    );
    ensure!(
        m.disabled.is(DisabledMetric::AccountAdapterDetails),
        "Accound adapter metrics should be disabled"
    );
    ensure!(
        m.disabled.is(DisabledMetric::AccountModulesMetrics),
        "Accound modules metrics should be disabled"
    );
    Ok(())
}

/// Go: `TestRecordPrebidCacheRequestTimeWithSuccess` and
/// `TestRecordPrebidCacheRequestTimeWithNotSuccess`.
#[test]
fn test_record_prebid_cache_request_time() -> TestResult {
    // Go: metrics/go_metrics_test.go:589 and :599 (TestRecordPrebidCacheRequestTime*)
    for (success, expected) in [(true, (1, 0)), (false, (0, 1))] {
        let registry = Registry::new();
        let m = new_test_metrics(
            &registry,
            &["Foo"],
            disabled(&[DisabledMetric::AccountAdapterDetails]),
            &[],
            &[],
        );
        m.record_prebid_cache_request_time(success, GoDuration::from_nanos(42));
        ensure_eq!(
            (
                m.prebid_cache_request_timer_success.count(),
                m.prebid_cache_request_timer_error.count()
            ),
            expected,
            "success={success}"
        );
    }
    Ok(())
}

/// Go: `TestRecordStoredDataFetchTime` and `TestRecordStoredDataError`.
#[test]
fn test_record_stored_data() -> TestResult {
    // Go: metrics/go_metrics_test.go:609 and :683 (TestRecordStoredDataFetchTime, ...Error)
    let data_types = [
        StoredDataType::ACCOUNT,
        StoredDataType::AMP,
        StoredDataType::CATEGORY,
        StoredDataType::REQUEST,
        StoredDataType::VIDEO,
    ];
    for data_type in data_types {
        for (fetch_type, error) in [
            (StoredDataFetchType::ALL, StoredDataError::NETWORK),
            (StoredDataFetchType::DELTA, StoredDataError::UNDEFINED),
        ] {
            let registry = Registry::new();
            let m = new_test_metrics(
                &registry,
                &["Foo", "Bar"],
                disabled(&[DisabledMetric::AccountAdapterDetails]),
                &[],
                &[],
            );
            let labels = StoredDataLabels {
                data_type,
                data_fetch_type: fetch_type,
                error,
            };
            m.record_stored_data_fetch_time(labels, GoDuration::from_nanos(500));
            m.record_stored_data_error(labels);

            let timer = m
                .stored_data_fetch_timer
                .get(&data_type)
                .and_then(|timers| timers.get(&fetch_type))
                .ok_or("no fetch timer")?;
            ensure_eq!(
                timer.count(),
                1,
                "stored_{data_type}_fetch_time.{fetch_type}"
            );
            ensure_eq!(
                timer.snapshot().sum(),
                500,
                "stored_{data_type}_fetch_time.{fetch_type}"
            );
            let meter = m
                .stored_data_error_meter
                .get(&data_type)
                .and_then(|meters| meters.get(&error))
                .ok_or("no error meter")?;
            ensure_eq!(meter.count(), 1, "stored_{data_type}_error.{error}");
        }
    }
    Ok(())
}

/// Go: `TestRecordRequestPrivacy`.
#[test]
fn test_record_request_privacy() -> TestResult {
    // Go: metrics/go_metrics_test.go:754 (TestRecordRequestPrivacy)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["Foo", "Bar"],
        disabled(&[DisabledMetric::AccountAdapterDetails]),
        &[],
        &[],
    );
    let ccpa = |enforced, provided| {
        let mut labels = PrivacyLabels::default();
        labels.ccpa.enforced = enforced;
        labels.ccpa.provided = provided;
        labels
    };
    // CCPA
    m.record_request_privacy(ccpa(true, true));
    m.record_request_privacy(ccpa(true, false));
    m.record_request_privacy(ccpa(false, true));
    // COPPA
    m.record_request_privacy(PrivacyLabels {
        coppa_enforced: true,
        ..PrivacyLabels::default()
    });
    // LMT
    m.record_request_privacy(PrivacyLabels {
        lmt_enforced: true,
        ..PrivacyLabels::default()
    });
    // GDPR
    for version in [TcfVersionValue::ERR, TcfVersionValue::V2] {
        let mut labels = PrivacyLabels::default();
        labels.gdpr.enforced = true;
        labels.gdpr.tcf_version = version;
        m.record_request_privacy(labels);
    }

    ensure_eq!(m.privacy_ccpa_request.count(), 2, "CCPA");
    ensure_eq!(m.privacy_ccpa_request_opt_out.count(), 1, "CCPA Opt Out");
    ensure_eq!(m.privacy_coppa_request.count(), 1, "COPPA");
    ensure_eq!(m.privacy_lmt_request.count(), 1, "LMT");
    for (version, what) in [
        (TcfVersionValue::ERR, "TCF Err"),
        (TcfVersionValue::V2, "TCF V2"),
    ] {
        let meter = m
            .privacy_tcf_request_version
            .get(&version)
            .ok_or("no TCF meter")?;
        ensure_eq!(meter.count(), 1, "{what}");
    }
    Ok(())
}

/// Go: `TestRecordAdapterBuyerUIDScrubbed` and `TestRecordAdapterGDPRRequestBlocked`.
#[test]
fn test_record_adapter_buyer_uid_scrubbed_and_gdpr_request_blocked() -> TestResult {
    // Go: metrics/go_metrics_test.go:800 and :842
    let fake_bidder = "fooAdvertising";
    let adapter_name = "AnyName";
    let lower_case_adapter_name = "anyname";
    let test_cases = [
        ("enabled_bidder_found", false, adapter_name, 1),
        ("enabled_bidder_not_found", false, fake_bidder, 0),
        ("disabled", true, adapter_name, 0),
    ];
    for (name, metrics_disabled, adapter_label, expected_count) in test_cases {
        for flag in [
            DisabledMetric::AdapterBuyerUidScrubbed,
            DisabledMetric::AdapterGdprRequestBlocked,
        ] {
            let registry = Registry::new();
            let flags = DisabledMetrics::NONE.with(flag, metrics_disabled);
            let m = new_test_metrics(&registry, &[adapter_name], flags, &[], &[]);
            let a = adapter(&m, lower_case_adapter_name)?;
            let meter = if flag == DisabledMetric::AdapterBuyerUidScrubbed {
                m.record_adapter_buyer_uid_scrubbed(adapter_label);
                &a.buyer_uid_scrubbed
            } else {
                m.record_adapter_gdpr_request_blocked(adapter_label);
                &a.gdpr_request_blocked
            };
            ensure_eq!(meter.count(), expected_count, "{name} {flag:?}");
        }
    }
    Ok(())
}

/// Go: `TestRecordCookieSync` and `TestRecordSetUid`. The Rust label types hold only Go's
/// constants and the empty value, which stands for Go's "unknown status".
#[test]
fn test_record_cookie_sync_and_set_uid() -> TestResult {
    // Go: metrics/go_metrics_test.go:884 and :921 (TestRecordCookieSync, TestRecordSetUid)
    let registry = Registry::new();
    let m = new_test_metrics(&registry, &["Foo", "Bar"], DisabledMetrics::NONE, &[], &[]);

    // Known
    m.record_cookie_sync(CookieSyncStatus::BAD_REQUEST);
    m.record_set_uid(SetUidStatus::OPT_OUT);
    // Unknown
    m.record_cookie_sync(CookieSyncStatus::default());
    m.record_set_uid(SetUidStatus::default());

    ensure_eq!(m.cookie_sync_meter.count(), 2, "CookieSyncMeter");
    for (status, expected) in [
        (CookieSyncStatus::OK, 0),
        (CookieSyncStatus::BAD_REQUEST, 1),
        (CookieSyncStatus::OPT_OUT, 0),
        (CookieSyncStatus::GDPR_HOST_COOKIE_BLOCKED, 0),
    ] {
        let meter = m.cookie_sync_status_meter.get(&status).ok_or("no meter")?;
        ensure_eq!(meter.count(), expected, "cookie_sync_requests.{status}");
    }
    ensure_eq!(m.set_uid_meter.count(), 2, "SetUidMeter");
    for (status, expected) in [
        (SetUidStatus::OK, 0),
        (SetUidStatus::BAD_REQUEST, 0),
        (SetUidStatus::OPT_OUT, 1),
        (SetUidStatus::GDPR_HOST_COOKIE_BLOCKED, 0),
        (SetUidStatus::SYNCER_UNKNOWN, 0),
    ] {
        let meter = m.set_uid_status_meter.get(&status).ok_or("no meter")?;
        ensure_eq!(meter.count(), expected, "setuid_requests.{status}");
    }
    Ok(())
}

/// Go: `TestRecordSyncerRequest` and `TestRecordSyncerSet`.
#[test]
fn test_record_syncer_request_and_set() -> TestResult {
    // Go: metrics/go_metrics_test.go:901 and :939 (TestRecordSyncerRequest, ...Set)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["Adapter1", "Adapter2"],
        DisabledMetrics::NONE,
        &["foo"],
        &[],
    );

    // Known
    m.record_syncer_request("foo", SyncerCookieSyncStatus::OK);
    m.record_syncer_set("foo", SyncerSetUidStatus::CLEARED);
    // Unknown Bidder
    m.record_syncer_request("bar", SyncerCookieSyncStatus::OK);
    m.record_syncer_set("bar", SyncerSetUidStatus::CLEARED);
    // Unknown Status
    m.record_syncer_request("foo", SyncerCookieSyncStatus::default());
    m.record_syncer_set("foo", SyncerSetUidStatus::default());

    let requests = m.syncer_requests_meter.get("foo").ok_or("no foo")?;
    for (status, expected) in [
        (SyncerCookieSyncStatus::OK, 1),
        (SyncerCookieSyncStatus::PRIVACY_BLOCKED, 0),
        (SyncerCookieSyncStatus::ALREADY_SYNCED, 0),
        (SyncerCookieSyncStatus::REJECTED_BY_FILTER, 0),
    ] {
        let meter = requests.get(&status).ok_or("no meter")?;
        ensure_eq!(meter.count(), expected, "syncer.foo.request.{status}");
    }
    let sets = m.syncer_sets_meter.get("foo").ok_or("no foo")?;
    for (status, expected) in [
        (SyncerSetUidStatus::OK, 0),
        (SyncerSetUidStatus::CLEARED, 1),
    ] {
        let meter = sets.get(&status).ok_or("no meter")?;
        ensure_eq!(meter.count(), expected, "syncer.foo.set.{status}");
    }
    ensure!(
        !m.syncer_requests_meter.contains_key("bar"),
        "no metrics for an unknown syncer"
    );
    Ok(())
}

/// Go: `TestStoredResponses`.
#[test]
fn test_stored_responses() -> TestResult {
    // Go: metrics/go_metrics_test.go:957 (TestStoredResponses)
    let test_cases = [
        ("acct-id", true, 0, 1),
        ("acct-id", false, 1, 1),
        (PUBLISHER_UNKNOWN, false, 0, 1),
        (PUBLISHER_UNKNOWN, true, 0, 1),
    ];
    for (pub_id, account_disabled, expected_account, expected_total) in test_cases {
        let registry = Registry::new();
        let flags =
            DisabledMetrics::NONE.with(DisabledMetric::AccountStoredResponses, account_disabled);
        let m = new_test_metrics(&registry, &["AnyName"], flags, &[], &[]);

        m.record_stored_response(pub_id);
        let am = m.get_account_metrics(pub_id);

        ensure_eq!(m.stored_responses_meter.count(), expected_total, "{pub_id}");
        ensure_eq!(
            am.stored_responses_meter.count(),
            expected_account,
            "{pub_id}"
        );
    }
    Ok(())
}

/// Go: `TestRecordAdsCertReqMetric`.
#[test]
fn test_record_ads_cert_req_metric() -> TestResult {
    // Go: metrics/go_metrics_test.go:1038 (TestRecordAdsCertReqMetric)
    for (success, expected_success, expected_failed) in [(false, 0, 1), (true, 1, 0)] {
        let registry = Registry::new();
        let m = new_test_metrics(&registry, &["AnyName"], DisabledMetrics::NONE, &[], &[]);

        m.record_ads_cert_req(success);

        ensure_eq!(
            m.ads_cert_requests_success.count(),
            expected_success,
            "success={success}"
        );
        ensure_eq!(
            m.ads_cert_requests_failure.count(),
            expected_failed,
            "success={success}"
        );
    }
    Ok(())
}

/// Go: `TestRecordModuleAccountMetrics`. Go reuses one registry across the cases.
#[test]
fn test_record_module_account_metrics() -> TestResult {
    // Go: metrics/go_metrics_test.go:1070 (TestRecordModuleAccountMetrics)
    let registry = Registry::new();
    let module = "foobar";
    let stages: &[&str] = &[
        "entrypoint",
        "raw_auction_request",
        "processed_auction_request",
    ];
    let test_cases = [
        ("entrypoint", "", false, 1, 0),
        ("raw_auction_request", "acc-1", false, 1, 1),
        ("processed_auction_request", "acc-1", true, 1, 0),
    ];
    for (stage, pub_id, modules_disabled, expected_module, expected_account) in test_cases {
        let flags =
            DisabledMetrics::NONE.with(DisabledMetric::AccountModulesMetrics, modules_disabled);
        let m = new_test_metrics(&registry, &[], flags, &[], &[(module, stages)]);

        let labels = ModuleLabels {
            module: module.to_owned(),
            stage: stage.to_owned(),
            account_id: pub_id.to_owned(),
        };
        m.record_module_called(&labels, GoDuration::MICROSECOND);
        let am = m.get_account_metrics(pub_id);

        let mm = m
            .module_stages
            .get(module)
            .and_then(|s| s.get(stage))
            .ok_or("no module metrics")?;
        ensure_eq!(mm.call_counter.count(), expected_module, "{stage}");
        if modules_disabled {
            ensure_eq!(am.modules.len(), 0, "Account modules metrics are disabled");
        } else {
            let aam = am.modules.get(module).ok_or("no account module metrics")?;
            ensure_eq!(aam.call_counter.count(), expected_account, "{stage}");
            ensure_eq!(aam.duration_timer.count(), expected_account, "{stage}");
        }
    }
    Ok(())
}

/// Go: `TestRecordOverheadTime`. Go reuses one registry across the cases, so the pre-bidder
/// timer accumulates.
#[test]
fn test_record_overhead_time() -> TestResult {
    // Go: metrics/go_metrics_test.go:1133 (TestRecordOverheadTime)
    let test_cases = [
        (
            "record-pre-bidder-overhead-time-1",
            OverheadType::PRE_BIDDER,
            1,
            500,
        ),
        (
            "record-pre-bidder-overhead-time-2",
            OverheadType::PRE_BIDDER,
            2,
            1000,
        ),
        (
            "record-auction-response-overhead-time",
            OverheadType::MAKE_AUCTION_RESPONSE,
            1,
            500,
        ),
        (
            "record-make-bidder-requests-overhead-time",
            OverheadType::MAKE_BIDDER_REQUESTS,
            1,
            500,
        ),
    ];
    let registry = Registry::new();
    for (name, overhead_type, expected_count, expected_sum) in test_cases {
        let m = new_test_metrics(&registry, &[], DisabledMetrics::NONE, &[], &[]);
        m.record_overhead_time(overhead_type, GoDuration::from_nanos(500));
        let overhead_metrics = m.overhead_timer.get(&overhead_type).ok_or("no timer")?;
        ensure_eq!(overhead_metrics.count(), expected_count, "{name}");
        ensure_eq!(overhead_metrics.snapshot().sum(), expected_sum, "{name}");
    }
    Ok(())
}

/// Go: `TestRecordAdapterPanic`.
#[test]
fn test_record_adapter_panic() -> TestResult {
    // Go: metrics/go_metrics_test.go:1199 (TestRecordAdapterPanic)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["AnyName"],
        disabled(&[
            DisabledMetric::AccountAdapterDetails,
            DisabledMetric::AccountModulesMetrics,
        ]),
        &[],
        &[("foobar", &["entry", "raw"])],
    );
    m.record_adapter_panic(&AdapterLabels {
        adapter: "AnyName".to_owned(),
        ..AdapterLabels::default()
    });
    ensure_eq!(adapter(&m, "anyname")?.panic_meter.count(), 1, "PanicMeter");
    Ok(())
}

/// Go: `TestRecordAdapterPrice` and `TestRecordAdapterTime`.
#[test]
fn test_record_adapter_price_and_time() -> TestResult {
    // Go: metrics/go_metrics_test.go:1208 and :1220 (TestRecordAdapterPrice, ...Time)
    let registry = Registry::new();
    let m = new_test_metrics(
        &registry,
        &["AnyName", "appnexus", "Adapter2"],
        DisabledMetrics::NONE,
        &["foo"],
        &[],
    );
    let labels = AdapterLabels {
        adapter: "AnyName".to_owned(),
        pub_id: "pub1".to_owned(),
        ..AdapterLabels::default()
    };
    m.record_adapter_price(&labels, 1000.0);
    m.record_adapter_time(&labels, GoDuration::from_nanos(1000));

    let a = adapter(&m, "anyname")?;
    ensure_eq!(a.price_histogram.snapshot().max(), 1000, "PriceHistogram");
    ensure_eq!(a.request_timer.snapshot().max(), 1000, "RequestTimer");
    let account = m.get_account_metrics("pub1");
    let aa = account
        .adapters
        .get("anyname")
        .ok_or("no account adapter")?;
    ensure_eq!(
        aa.price_histogram.snapshot().max(),
        1000,
        "account PriceHistogram"
    );
    ensure_eq!(
        aa.request_timer.snapshot().max(),
        1000,
        "account RequestTimer"
    );
    Ok(())
}

/// Go: `TestRecordAdapterRequest`.
#[test]
fn test_record_adapter_request() -> TestResult {
    // Go: metrics/go_metrics_test.go:1232 (TestRecordAdapterRequest)
    let all_errors: BTreeSet<AdapterError> = adapter_errors().iter().copied().collect();
    // (description, bids, pub, errors) -> (no bid, got bid, each error)
    let test_cases = [
        (
            "no-bid",
            AdapterBid::NONE,
            "acc-1",
            BTreeSet::new(),
            (1, 0, 0),
        ),
        (
            "got-bid",
            AdapterBid::PRESENT,
            "acc-2",
            BTreeSet::new(),
            (0, 1, 0),
        ),
        (
            "adapter-errors",
            AdapterBid::default(),
            "acc-1",
            all_errors,
            (0, 0, 1),
        ),
    ];
    for (description, bids, pub_id, errors, (no_bid, got_bid, error_count)) in test_cases {
        let registry = Registry::new();
        let m = new_test_metrics(
            &registry,
            &["AnyName"],
            DisabledMetrics::NONE,
            &["foo"],
            &MODULE_STAGES,
        );
        m.record_adapter_request(&AdapterLabels {
            adapter: "AnyName".to_owned(),
            adapter_bids: bids,
            pub_id: pub_id.to_owned(),
            adapter_errors: errors,
            ..AdapterLabels::default()
        });
        let a = adapter(&m, "anyname")?;
        ensure_eq!(
            (a.no_bid_meter.count(), a.got_bids_meter.count()),
            (no_bid, got_bid),
            "{description}"
        );
        ensure_eq!(a.no_cookie_meter.count(), 0, "{description}");
        for err in adapter_errors() {
            let meter = a.error_meters.get(err).ok_or("no error meter")?;
            ensure_eq!(meter.count(), error_count, "{description}: {err}");
        }
    }
    Ok(())
}
