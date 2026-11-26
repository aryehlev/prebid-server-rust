//! Go: `metrics/config/metrics_test.go`. The assertions Go makes on the go-metrics engine are
//! made on the Prometheus engine here, which is the engine this crate ports.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    sync::Arc,
};

use super::{
    DetailedMetricsEngine, MetricsEngineError, MultiMetricsEngine, NilMetricsEngine,
    new_metrics_engine,
};
use crate::{
    metrics::{
        AdapterBid, AdapterLabels, CacheResult, CookieFlag, CookieSyncStatus, DemandSource,
        FetcherBackendResult, FetcherOperation, FetcherResult, GoDuration, ImpLabels, ImpMediaType,
        Labels, MetricsEngine, ModuleLabels, OverheadType, PrivacyLabels, RequestStatus,
        RequestType, SetUidStatus, StoredDataLabels, SyncerCookieSyncStatus, SyncerSetUidStatus,
        adapter_errors,
    },
    metrics_mock::{Expectation, MetricsCall, MetricsEngineMock},
    options::{self, DisabledMetrics, PrometheusMetrics},
    prometheus::{
        self,
        proto::{Metric, MetricFamily, MetricType},
    },
};

type TestResult = Result<(), Box<dyn Error>>;

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

fn syncer_keys() -> Vec<String> {
    vec!["keyA".to_owned(), "keyB".to_owned()]
}

fn core_bidder_names() -> Result<Vec<String>, Box<dyn Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/golden/core_bidder_names.txt"
    );
    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Go: `TestNilMetricsEngine`.
#[test]
fn nil_metrics_engine() -> TestResult {
    let engine = new_metrics_engine(
        &options::Metrics::default(),
        &[],
        &syncer_keys(),
        &modules_stages(),
    )?;
    ensure_eq!(
        format!("{:?}", engine.metrics_engine),
        "NilMetricsEngine",
        "a NilMetricsEngine"
    );
    ensure!(engine.prometheus_metrics.is_none(), "no Prometheus engine");
    Ok(())
}

/// Go: `TestGoMetricsEngine`. The go-metrics engine is not ported, so an `InfluxDB` host is an
/// error instead of an engine.
#[test]
fn go_metrics_engine_is_not_ported() -> TestResult {
    let mut cfg = options::Metrics::default();
    cfg.influxdb.host = "localhost".to_owned();
    let result = new_metrics_engine(&cfg, &[], &syncer_keys(), &modules_stages());
    ensure!(
        matches!(result, Err(MetricsEngineError::InfluxNotSupported)),
        "an InfluxDB host is rejected"
    );
    Ok(())
}

/// A Prometheus port selects the Prometheus engine alone, which is also the engine recorded into.
#[test]
fn prometheus_metrics_engine() -> TestResult {
    let mut cfg = options::Metrics::default();
    cfg.prometheus.port = 8080;
    cfg.prometheus.namespace = "prebid".to_owned();
    let engine = new_metrics_engine(
        &cfg,
        &core_bidder_names()?,
        &syncer_keys(),
        &modules_stages(),
    )?;
    let prometheus = engine
        .prometheus_metrics
        .clone()
        .ok_or("a Prometheus engine")?;
    engine.record_tmax_timeout();
    let value = counter(&prometheus.gatherer().gather(), "prebid_tmax_timeout", &[])?;
    ensure_eq!(value, 1.0, "recorded through the detailed engine");
    Ok(())
}

/// Go selects the Prometheus engine for any `metrics.prometheus.port` other than 0
/// (`metrics/config/metrics.go`: `Port != 0`), while `PrometheusMetrics.validate` only checks a
/// positive port (`Port > 0`): a negative port builds the engine and passes validation even with
/// a zero timeout.
#[test]
fn negative_prometheus_port_selects_the_engine() -> TestResult {
    let mut cfg = options::Metrics::default();
    cfg.prometheus.port = -1;
    let engine = new_metrics_engine(&cfg, &[], &syncer_keys(), &modules_stages())?;
    ensure!(
        engine.prometheus_metrics.is_some(),
        "a negative port builds the Prometheus engine"
    );
    let mut errs = Vec::new();
    cfg.validate(&mut errs);
    ensure!(
        errs.is_empty(),
        "a negative port is not validated: {errs:?}"
    );
    Ok(())
}

/// An invalid Prometheus namespace is the Go `MustRegister` panic, as an error.
#[test]
fn prometheus_registration_error() -> TestResult {
    let mut cfg = options::Metrics::default();
    cfg.prometheus.port = 8080;
    cfg.prometheus.namespace = "bad-namespace".to_owned();
    let Err(MetricsEngineError::Registration(err)) =
        new_metrics_engine(&cfg, &[], &[], &BTreeMap::new())
    else {
        return Err("an invalid namespace must be a registration error".into());
    };
    let message = err.to_string();
    ensure!(message.contains("is not a valid metric name"), "{message}");
    Ok(())
}

/// The series `name{labels}` of a gather (the labels a subset of the series'), with the type of
/// its family.
fn find<'a>(
    families: &'a [MetricFamily],
    name: &str,
    labels: &[(&str, &str)],
) -> Result<(MetricType, &'a Metric), Box<dyn Error>> {
    let family = families
        .iter()
        .find(|f| f.name() == name)
        .ok_or_else(|| format!("no {name}"))?;
    let metric = family
        .get_metric()
        .iter()
        .find(|m| {
            labels.iter().all(|(k, v)| {
                m.get_label()
                    .iter()
                    .any(|pair| pair.name() == *k && pair.value() == *v)
            })
        })
        .ok_or_else(|| format!("no {name}{labels:?}"))?;
    Ok((family.get_field_type(), metric))
}

/// The value of the counter series `name{labels}` in a gather.
fn counter(
    families: &[MetricFamily],
    name: &str,
    labels: &[(&str, &str)],
) -> Result<f64, Box<dyn Error>> {
    let (kind, metric) = find(families, name, labels)?;
    if kind == MetricType::COUNTER {
        Ok(metric.get_counter().get_value())
    } else {
        Err(format!("{name} is a {kind:?}").into())
    }
}

/// The sample count of the histogram series `name{labels}` in a gather.
fn histogram_count(
    families: &[MetricFamily],
    name: &str,
    labels: &[(&str, &str)],
) -> Result<u64, Box<dyn Error>> {
    let (kind, metric) = find(families, name, labels)?;
    if kind == MetricType::HISTOGRAM {
        Ok(metric.get_histogram().get_sample_count())
    } else {
        Err(format!("{name} is a {kind:?}").into())
    }
}

/// Go: `TestMultiMetricsEngine`, with the Prometheus engine in the place of the go-metrics one.
#[test]
fn multi_metrics_engine() -> TestResult {
    let cfg = PrometheusMetrics {
        port: 8080,
        ..PrometheusMetrics::default()
    };
    let prometheus = Arc::new(prometheus::Metrics::new(
        &cfg,
        DisabledMetrics::NONE,
        &[],
        &modules_stages(),
        &core_bidder_names()?,
    )?);
    let prometheus_engine: Arc<dyn MetricsEngine> = Arc::<prometheus::Metrics>::clone(&prometheus);
    let metrics_engine = MultiMetricsEngine(vec![prometheus_engine, Arc::new(NilMetricsEngine)]);
    record_multi_engine_calls(&metrics_engine);

    let g = prometheus.gatherer().gather();
    check_request_families(&g)?;
    check_adapter_and_cache_families(&g)?;
    check_module_families(&g)
}

/// The calls Go's `TestMultiMetricsEngine` makes.
fn record_multi_engine_calls(metrics_engine: &MultiMetricsEngine) {
    let labels = Labels {
        source: DemandSource::WEB,
        r_type: RequestType::ORTB2_WEB,
        pub_id: "test1".to_owned(),
        cookie_flag: CookieFlag::YES,
        request_status: RequestStatus::OK,
        request_size: 1024,
    };
    let apn_labels = AdapterLabels {
        source: DemandSource::WEB,
        r_type: RequestType::ORTB2_WEB,
        adapter: "appnexus".to_owned(),
        pub_id: "test1".to_owned(),
        cookie_flag: CookieFlag::YES,
        adapter_bids: AdapterBid::NONE,
        ..AdapterLabels::default()
    };
    let pub_labels = AdapterLabels {
        adapter: "pubmatic".to_owned(),
        adapter_bids: AdapterBid::PRESENT,
        ..apn_labels.clone()
    };
    let imp_type_labels = ImpLabels::NONE
        .with(ImpMediaType::BANNER, true)
        .with(ImpMediaType::AUDIO, true)
        .with(ImpMediaType::NATIVE, true);
    let ms = |n| GoDuration::times(n, GoDuration::MILLISECOND);
    for _ in 0..5 {
        metrics_engine.record_request(&labels);
        metrics_engine.record_imps(imp_type_labels);
        metrics_engine.record_request_time(&labels, ms(20));
        metrics_engine.record_adapter_request(&pub_labels);
        metrics_engine.record_adapter_request(&apn_labels);
        metrics_engine.record_adapter_price(&pub_labels, 1.34);
        metrics_engine.record_adapter_bid_received(&pub_labels, "banner", true);
        metrics_engine.record_adapter_time(&pub_labels, ms(20));
        metrics_engine.record_prebid_cache_request_time(true, ms(20));
    }
    for (module, stages) in modules_stages() {
        for stage in stages {
            let module_labels = ModuleLabels {
                module: module.clone(),
                stage,
                account_id: "test1".to_owned(),
            };
            metrics_engine.record_module_called(&module_labels, ms(1));
            metrics_engine.record_module_failed(&module_labels);
            metrics_engine.record_module_success_nooped(&module_labels);
            metrics_engine.record_module_success_updated(&module_labels);
            metrics_engine.record_module_success_rejected(&module_labels);
            metrics_engine.record_module_execution_error(&module_labels);
            metrics_engine.record_module_timeout(&module_labels);
        }
    }
    for r_type in [RequestType::AMP, RequestType::VIDEO] {
        metrics_engine.record_request(&Labels {
            source: DemandSource::WEB,
            r_type,
            pub_id: "test2".to_owned(),
            cookie_flag: CookieFlag::YES,
            request_status: RequestStatus::BLOCKED_APP,
            request_size: 0,
        });
    }
    let video_only = ImpLabels::NONE.with(ImpMediaType::VIDEO, true);
    for _ in 0..3 {
        metrics_engine.record_imps(video_only);
    }
    metrics_engine.record_stored_req_cache_result(CacheResult::MISS, 1);
    metrics_engine.record_stored_imp_cache_result(CacheResult::MISS, 2);
    metrics_engine.record_account_cache_result(CacheResult::MISS, 3);
    metrics_engine.record_stored_req_cache_result(CacheResult::HIT, 4);
    metrics_engine.record_stored_imp_cache_result(CacheResult::HIT, 5);
    metrics_engine.record_account_cache_result(CacheResult::HIT, 6);
    metrics_engine.record_adapter_buyer_uid_scrubbed("appnexus");
    metrics_engine.record_adapter_gdpr_request_blocked("appnexus");
    metrics_engine.record_request_queue_time(false, RequestType::VIDEO, GoDuration::from_nanos(1));
}

fn check_request_families(g: &[MetricFamily]) -> TestResult {
    let requests = |r_type: &str, status: &str| {
        counter(
            g,
            "requests",
            &[("request_type", r_type), ("request_status", status)],
        )
    };
    ensure_eq!(
        requests("openrtb2-web", "ok")?,
        5.0,
        "RequestStatuses.OpenRTB2.OK"
    );
    ensure_eq!(requests("amp", "ok")?, 0.0, "RequestStatuses.AMP.OK");
    ensure_eq!(
        requests("amp", "blockedapp")?,
        1.0,
        "RequestStatuses.AMP.BlockedApp"
    );
    ensure_eq!(requests("video", "ok")?, 0.0, "RequestStatuses.Video.OK");
    ensure_eq!(
        requests("video", "err")?,
        0.0,
        "RequestStatuses.Video.Error"
    );
    ensure_eq!(
        requests("video", "badinput")?,
        0.0,
        "RequestStatuses.Video.BadInput"
    );
    ensure_eq!(
        requests("video", "blockedapp")?,
        1.0,
        "RequestStatuses.Video.BlockedApp"
    );
    ensure_eq!(
        requests("openrtb2-web", "err")?,
        0.0,
        "RequestStatuses.OpenRTB2.Error"
    );
    ensure_eq!(
        requests("openrtb2-web", "badinput")?,
        0.0,
        "RequestStatuses.OpenRTB2.BadInput"
    );
    ensure_eq!(
        requests("openrtb2-web", "blockedapp")?,
        0.0,
        "RequestStatuses.OpenRTB2.BlockedApp"
    );

    let size = histogram_count(g, "request_size_bytes", &[("request_size", "auction")])?;
    ensure_eq!(size, 5, "RequestSizeByEndpoint.Auction");

    let imps = |banner: &str, video: &str, audio: &str, native: &str| {
        counter(
            g,
            "impressions_requests",
            &[
                ("banner", banner),
                ("video", video),
                ("audio", audio),
                ("native", native),
            ],
        )
    };
    ensure_eq!(
        imps("true", "false", "true", "true")?,
        5.0,
        "banner, audio and native imps"
    );
    ensure_eq!(imps("false", "true", "false", "false")?, 3.0, "video imps");

    let cache_write = histogram_count(g, "prebidcache_write_time_seconds", &[("success", "true")])?;
    ensure_eq!(cache_write, 5, "RecordPrebidCacheRequestTime");
    Ok(())
}

fn check_adapter_and_cache_families(g: &[MetricFamily]) -> TestResult {
    let adapter_requests = |adapter: &str, has_bids: &str| {
        counter(
            g,
            "adapter_requests",
            &[
                ("adapter", adapter),
                ("cookie", "exists"),
                ("has_bids", has_bids),
            ],
        )
    };
    ensure_eq!(
        adapter_requests("pubmatic", "true")?,
        5.0,
        "pubmatic.GotBidsMeter"
    );
    ensure_eq!(
        adapter_requests("pubmatic", "false")?,
        0.0,
        "pubmatic.NoBidMeter"
    );
    ensure_eq!(
        adapter_requests("appnexus", "true")?,
        0.0,
        "appnexus.GotBidsMeter"
    );
    ensure_eq!(
        adapter_requests("appnexus", "false")?,
        5.0,
        "appnexus.NoBidMeter"
    );
    for err in adapter_errors() {
        let errors = counter(
            g,
            "adapter_errors",
            &[("adapter", "pubmatic"), ("adapter_error", err.as_str())],
        )?;
        ensure_eq!(errors, 0.0, "pubmatic.Request.ErrorMeter.{err}");
    }

    let queue = |status: &str| {
        histogram_count(
            g,
            "request_queue_time",
            &[("request_type", "video"), ("request_status", status)],
        )
    };
    ensure_eq!(
        queue("requestRejectedLabel")?,
        1,
        "RecordRequestQueueTime.Video.Rejected"
    );
    ensure_eq!(
        queue("requestAcceptedLabel")?,
        0,
        "RecordRequestQueueTime.Video.Accepted"
    );

    for (name, miss, hit) in [
        ("stored_request_cache_performance", 1.0, 4.0),
        ("stored_impressions_cache_performance", 2.0, 5.0),
        ("account_cache_performance", 3.0, 6.0),
    ] {
        ensure_eq!(
            counter(g, name, &[("cache_result", "miss")])?,
            miss,
            "{name} miss"
        );
        ensure_eq!(
            counter(g, name, &[("cache_result", "hit")])?,
            hit,
            "{name} hit"
        );
    }

    let scrubbed = counter(g, "adapter_buyeruids_scrubbed", &[("adapter", "appnexus")])?;
    ensure_eq!(scrubbed, 1.0, "appNexus.BuyerUIDScrubbed");
    let blocked = counter(
        g,
        "adapter_gdpr_requests_blocked",
        &[("adapter", "appnexus")],
    )?;
    ensure_eq!(blocked, 1.0, "appNexus.GDPRRequestBlocked");
    Ok(())
}

fn check_module_families(g: &[MetricFamily]) -> TestResult {
    for (module, stages) in modules_stages() {
        for stage in stages {
            let stage_label = [("stage", stage.as_str())];
            let duration = histogram_count(g, &format!("modules_{module}_duration"), &stage_label)?;
            ensure_eq!(duration, 1, "ModuleMetrics.{module}.{stage}.Duration");
            for suffix in [
                "called",
                "failed",
                "success_noops",
                "success_updates",
                "success_rejects",
                "execution_errors",
                "timeouts",
            ] {
                let value = counter(g, &format!("modules_{module}_{suffix}"), &stage_label)?;
                ensure_eq!(value, 1.0, "ModuleMetrics.{module}.{stage}.{suffix}");
            }
        }
    }
    Ok(())
}

/// Go: `TestV2MultiMetricsEngineForwardsFetcherMetrics`.
#[test]
fn v2_multi_metrics_engine_forwards_fetcher_metrics() -> TestResult {
    let first = Arc::new(MetricsEngineMock::new());
    let second = Arc::new(MetricsEngineMock::new());
    let ms = GoDuration::MILLISECOND;
    for engine in [&first, &second] {
        engine.on(Expectation::call(MetricsCall::FetcherResult(
            "account".to_owned(),
            FetcherResult::HIT,
        ))
        .once());
        engine.on(Expectation::call(MetricsCall::FetcherBackendFetch(
            "account".to_owned(),
            FetcherOperation::GET,
            FetcherBackendResult::OK,
            ms,
        ))
        .once());
    }
    let first_engine: Arc<dyn MetricsEngine> = Arc::<MetricsEngineMock>::clone(&first);
    let second_engine: Arc<dyn MetricsEngine> = Arc::<MetricsEngineMock>::clone(&second);
    let engines = MultiMetricsEngine(vec![first_engine, second_engine]);

    engines.record_fetcher_result("account", FetcherResult::HIT);
    engines.record_fetcher_backend_fetch(
        "account",
        FetcherOperation::GET,
        FetcherBackendResult::OK,
        ms,
    );

    first.assert_expectations()?;
    second.assert_expectations()?;
    Ok(())
}

/// `MultiMetricsEngine`, `NilMetricsEngine` and `DetailedMetricsEngine` forward every method:
/// each call reaches every engine once, in order.
#[test]
fn multi_and_detailed_engines_forward_every_method() -> TestResult {
    let mock = Arc::new(MetricsEngineMock::new());
    let as_engine: Arc<dyn MetricsEngine> = Arc::<MetricsEngineMock>::clone(&mock);
    let multi: Arc<dyn MetricsEngine> = Arc::new(MultiMetricsEngine(vec![
        as_engine,
        Arc::new(NilMetricsEngine),
    ]));
    let detailed = DetailedMetricsEngine {
        metrics_engine: multi,
        prometheus_metrics: None,
    };
    record_every_method(&detailed);
    record_every_method(&NilMetricsEngine);
    let methods: Vec<&str> = mock.calls().iter().map(MetricsCall::method_name).collect();
    let distinct: BTreeSet<&str> = methods.iter().copied().collect();
    ensure!(
        methods.len() == 53 && distinct.len() == 53,
        "53 distinct calls: {methods:?}"
    );
    Ok(())
}

/// Calls each of the 53 `MetricsEngine` methods once.
fn record_every_method(e: &dyn MetricsEngine) {
    let labels = Labels::default();
    let adapter = AdapterLabels::default();
    let module = ModuleLabels::default();
    let d = GoDuration::SECOND;
    e.record_connection_accept(true);
    e.record_tmax_timeout();
    e.record_connection_close(true);
    e.record_request(&labels);
    e.record_imps(ImpLabels::NONE);
    e.record_request_time(&labels, d);
    e.record_overhead_time(OverheadType::PRE_BIDDER, d);
    e.record_adapter_request(&adapter);
    e.record_adapter_connections("a", true, d);
    e.record_dns_time(d);
    e.record_tls_handshake_time(d);
    e.record_bidder_server_response_time(d);
    e.record_adapter_panic(&adapter);
    e.record_adapter_bid_received(&adapter, "banner", true);
    e.record_adapter_price(&adapter, 1.0);
    e.record_adapter_time(&adapter, d);
    e.record_cookie_sync(CookieSyncStatus::OK);
    e.record_syncer_request("k", SyncerCookieSyncStatus::OK);
    e.record_set_uid(SetUidStatus::OK);
    e.record_syncer_set("k", SyncerSetUidStatus::OK);
    e.record_stored_req_cache_result(CacheResult::HIT, 1);
    e.record_stored_imp_cache_result(CacheResult::HIT, 1);
    e.record_account_cache_result(CacheResult::HIT, 1);
    e.record_stored_data_fetch_time(StoredDataLabels::default(), d);
    e.record_stored_data_error(StoredDataLabels::default());
    e.record_fetcher_result("s", FetcherResult::HIT);
    e.record_fetcher_backend_fetch("s", FetcherOperation::GET, FetcherBackendResult::OK, d);
    e.record_prebid_cache_request_time(true, d);
    e.record_request_queue_time(true, RequestType::VIDEO, d);
    e.record_timeout_notice(true);
    e.record_request_privacy(PrivacyLabels::default());
    e.record_adapter_buyer_uid_scrubbed("a");
    e.record_adapter_gdpr_request_blocked("a");
    e.record_debug_request(true, "p");
    e.record_stored_response("p");
    e.record_gvl_list_request();
    e.record_live_gvl_fetch(true);
    e.record_ads_cert_req(true);
    e.record_ads_cert_sign_time(d);
    e.record_bid_validation_creative_size_error("a", "p");
    e.record_bid_validation_creative_size_warn("a", "p");
    e.record_bid_validation_secure_markup_error("a", "p");
    e.record_bid_validation_secure_markup_warn("a", "p");
    e.record_module_called(&module, d);
    e.record_module_failed(&module);
    e.record_module_success_nooped(&module);
    e.record_module_success_updated(&module);
    e.record_module_success_rejected(&module);
    e.record_module_execution_error(&module);
    e.record_module_timeout(&module);
    e.record_adapter_throttled("a");
    e.record_adapter_connection_dial_error("a");
    e.record_adapter_connection_dial_time("a", d);
}
